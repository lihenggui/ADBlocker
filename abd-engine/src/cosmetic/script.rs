// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Mercury Li
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 3 as
// published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use encoding_rs::{Encoding, UTF_8};
use http_body_util::BodyExt;
use hyper::body::{Body, Frame};
use hyper::header::{CONTENT_SECURITY_POLICY, HeaderMap, HeaderValue};

use crate::html::{HeadCharset, encoding, find_ascii, head_charset, transport_charset};
use tokio::time::{Instant, timeout_at};

use super::csp::{self, By, Element};
use super::{MAX_HELD, Place, place, source};

// A typical page needs a few KiB of helpers and actions. Large sets of
// matching list rules must not download another page's worth of JavaScript.
const MAX_SCRIPT: usize = 256 << 10;
const MAX_ACTIONS: usize = 128;
const MAX_ACTION: usize = 4096;
const HEAD_WAIT: Duration = Duration::from_millis(250);

const SCRIPTLET_START: &str = "(() => {\nconst scriptletGlobals = {};\n";
const SCRIPTLET_END: &str = "\n})();\n";
const CALL_START: &str = "try {";
const CALL_END: &str = "\n} catch ( e ) { }\n";
pub(crate) const JAVASCRIPT_START: &str = "(function(){\n";
pub(crate) const JAVASCRIPT_END: &str = "\n})();";

pub(crate) fn assemble(scriptlets: String, actions: HashSet<String>) -> Option<String> {
    let scriptlets = scriptlet_code(&scriptlets);
    let mut actions: Vec<_> = actions
        .into_iter()
        .filter(|action| action.len() <= MAX_ACTION)
        .collect();
    actions.sort_unstable();
    let mut script = String::new();
    let runtime = include_str!("../../resources/procedural.js").trim_end();
    let remaining = MAX_SCRIPT - scriptlets.as_ref().map_or(0, String::len);
    for action in actions.into_iter().take(MAX_ACTIONS) {
        let action = escape_action(&action);
        if !safe_raw_text(&action) {
            continue;
        }
        let prefix = if script.is_empty() {
            runtime.len() + "([".len()
        } else {
            1 // comma
        };
        if script.len() + prefix + action.len() + "]);\n".len() > remaining {
            break;
        }
        if script.is_empty() {
            script.push_str(runtime);
            script.push_str("([");
        } else {
            script.push(',');
        }
        script.push_str(&action);
    }
    if !script.is_empty() {
        script += "]);\n";
    }
    if let Some(scriptlets) = scriptlets {
        script += &scriptlets;
    }
    (!script.is_empty() && script.len() <= MAX_SCRIPT && safe_raw_text(&script)).then_some(script)
}

fn escape_action(action: &str) -> String {
    // Serialize as a JS data literal without HTML-significant
    // characters or dependence on the page's character encoding.
    let mut escaped = String::with_capacity(action.len());
    for c in action.chars() {
        if c == '<' || !c.is_ascii() {
            for unit in c.encode_utf16(&mut [0; 2]) {
                write!(escaped, "\\u{unit:04x}").unwrap();
            }
        } else {
            escaped.push(c);
        }
    }
    escaped
}

fn scriptlet_code(mut generated: &str) -> Option<String> {
    if generated.is_empty() {
        return None;
    }
    let wrapper = SCRIPTLET_START.len() + SCRIPTLET_END.len();
    if generated.len() + wrapper <= MAX_SCRIPT && safe_raw_text(generated) {
        return Some([SCRIPTLET_START, generated, SCRIPTLET_END].concat());
    }

    // adblock-rust emits helper definitions before its try/catch blocks.
    // Walk backward so try/catch inside a helper is never split.
    let mut calls = Vec::new();
    while let Some(block) = generated.strip_suffix(CALL_END) {
        let (start, call) = split_call(block)?;
        generated = start.strip_suffix(CALL_START)?;
        if safe_raw_text(call) {
            calls.push(call);
        }
    }
    if calls.is_empty() || !safe_raw_text(generated) {
        return None;
    }
    let overhead = wrapper + generated.len();
    let call_overhead = CALL_START.len() + 1 + CALL_END.len();
    let size = |call: &str| call_overhead + call.len();
    if overhead + calls.iter().map(|call| size(call)).sum::<usize>() > MAX_SCRIPT {
        calls.sort_unstable();
    } else {
        calls.reverse();
    }
    let mut script = [SCRIPTLET_START, generated].concat();
    let start = script.len();
    for call in calls {
        if script.len() + size(call) + SCRIPTLET_END.len() <= MAX_SCRIPT {
            script += CALL_START;
            script.push('\n');
            script += call;
            script += CALL_END;
        }
    }
    if script.len() == start {
        return None;
    }
    script += SCRIPTLET_END;
    Some(script)
}

fn split_call(block: &str) -> Option<(&str, &str)> {
    let Some(body) = block.strip_suffix(JAVASCRIPT_END) else {
        // Bundled scriptlet calls escape newlines in their arguments.
        return block.rsplit_once('\n');
    };
    // Each JavaScript rule body occupies exactly one line between its wrappers.
    let (start, _) = body.rsplit_once('\n')?;
    let start = start
        .strip_suffix(JAVASCRIPT_START.trim_end())?
        .strip_suffix('\n')?;
    Some((start, &block[start.len() + 1..]))
}

pub(crate) fn safe_raw_text(script: &str) -> bool {
    // Reject an end tag and the legacy script data escape state, even
    // inside a quoted JS argument. The body emitted here is what we hash.
    !script.contains(['\r', '\0'])
        && !script.contains("<!--")
        && find_ascii(script.as_bytes(), b"</script").is_none()
}

struct Head {
    at: usize,
    charset: Option<String>,
}

enum Inspect {
    Ready(Head),
    More,
    Skip,
}

fn inspect(bytes: &[u8], header_charset: Option<&str>) -> Inspect {
    let at = match place(bytes) {
        Place::At(at) => at,
        Place::More => return Inspect::More,
        Place::Nowhere => return Inspect::Skip,
    };
    // A BOM or transport declaration takes precedence over a meta charset.
    if Encoding::for_bom(bytes).is_some() || header_charset.is_some() {
        return Inspect::Ready(Head { at, charset: None });
    }
    match head_charset(&bytes[at..], true) {
        HeadCharset::Found(charset) => Inspect::Ready(Head { at, charset }),
        HeadCharset::More => Inspect::More,
        HeadCharset::Ambiguous => Inspect::Skip,
    }
}

// An error means the entire script must be skipped. None means this
// policy already permits it. Keep original directives and source tokens
// verbatim, including 'strict-dynamic' and 'none'.
fn permit_policy(policy: &str, hash: &str) -> Result<Option<String>, ()> {
    if csp::named(policy, "require-trusted-types-for").is_some()
        || csp::named(policy, "sandbox").is_some_and(|sandbox| !sandbox.has("allow-scripts"))
    {
        return Err(());
    }
    let Some((directive, by)) = csp::effective(policy, Element::Script) else {
        return Ok(None);
    };
    let words = directive.sources.clone();
    if words.clone().any(|word| word == hash) {
        return Ok(None);
    }
    if directive.has("'unsafe-inline'")
        && !directive.has("'strict-dynamic'")
        && !words.clone().any(csp::hash_or_nonce)
    {
        // Adding a hash here would take away permission for the page's
        // own inline scripts, so keep the existing allow-all directive.
        return Ok(None);
    }
    let raw = directive.raw;
    let rewritten = if by == By::Elements {
        let end = raw.as_ptr() as usize - policy.as_ptr() as usize + raw.trim_end().len();
        format!("{} {hash}{}", &policy[..end], &policy[end..])
    } else {
        // Scope the allowance to script elements so it cannot affect
        // event handlers, eval, workers or another default-src consumer.
        let mut new = policy.to_owned();
        new.push_str("; script-src-elem");
        for word in words {
            new.push(' ');
            new.push_str(word);
        }
        new.push(' ');
        new.push_str(hash);
        new
    };
    Ok(Some(rewritten))
}

fn rewrite_headers(headers: &mut HeaderMap, hash: &str) -> Option<()> {
    let mut updated = Vec::new();
    for header in headers.get_all(CONTENT_SECURITY_POLICY) {
        let text = header.to_str().ok()?;
        let mut policies = Vec::new();
        let mut changed = false;
        for policy in text.split(',') {
            let rewrite = permit_policy(policy, hash).ok()?;
            changed |= rewrite.is_some();
            policies.push(rewrite.unwrap_or_else(|| policy.to_owned()));
        }
        updated.push(if changed {
            HeaderValue::from_str(&policies.join(",")).ok()?
        } else {
            header.clone()
        });
    }
    headers.remove(CONTENT_SECURITY_POLICY);
    for header in updated {
        headers.append(CONTENT_SECURITY_POLICY, header);
    }
    Some(())
}

fn rewrite(
    head: Head,
    bytes: &[u8],
    headers: &HeaderMap,
    header_charset: Option<&str>,
    script: &str,
    css: Option<&str>,
) -> Option<(HeaderMap, Vec<u8>)> {
    let encoding = encoding(bytes, header_charset.or(head.charset.as_deref()))?;
    if encoding.is_some_and(|e| !e.is_ascii_compatible())
        || (encoding != Some(UTF_8) && !script.is_ascii())
    {
        return None;
    }
    let hash = source(script);
    let mut changed = headers.clone();
    rewrite_headers(&mut changed, &hash)?;
    let mut replaced = bytes[..head.at].to_vec();
    replaced.extend_from_slice(b"<script>");
    replaced.extend_from_slice(script.as_bytes());
    replaced.extend_from_slice(b"</script>");
    if let Some(css) = css {
        replaced.extend_from_slice(b"<style>");
        replaced.extend_from_slice(css.as_bytes());
        replaced.extend_from_slice(b"</style>");
    }
    // Meta policies govern content encountered after their tags. The
    // injection point is before every meta tag that the parser can accept.
    replaced.extend_from_slice(&bytes[head.at..]);
    Some((changed, replaced))
}

/// Replay the consumed prefix and at most one buffered frame before the
/// untouched stream. A slow, incomplete or ambiguous head gets CSS only.
pub(crate) enum Preparation {
    Injected,
    CssOnly,
    NoHead,
}

pub(crate) struct Prefix<B: Body> {
    inner: B,
    frames: VecDeque<Result<Frame<Bytes>, B::Error>>,
}

impl<B: Body<Data = Bytes> + Unpin> Prefix<B> {
    pub(crate) fn new(inner: B) -> Self {
        Self {
            inner,
            frames: VecDeque::new(),
        }
    }

    pub(crate) async fn prepare(
        &mut self,
        headers: &mut HeaderMap,
        script: &str,
        css: Option<&str>,
        rewriting: bool,
    ) -> Preparation {
        let mut held = Vec::new();
        // The rewrite applies its own collection and work deadlines before
        // it can deliver anything. Start our short head wait at that point.
        let mut deadline = (!rewriting).then(|| Instant::now() + HEAD_WAIT);
        let header_charset = transport_charset(headers);
        let head = loop {
            match inspect(&held, header_charset) {
                Inspect::Ready(head) => break Some(head),
                Inspect::Skip => break None,
                Inspect::More
                    if held.len() == MAX_HELD
                        || deadline.is_some_and(|end| Instant::now() >= end) =>
                {
                    break None;
                }
                Inspect::More => {}
            }
            let frame = match deadline {
                Some(end) => timeout_at(end, self.inner.frame()).await,
                None => {
                    let frame = self.inner.frame().await;
                    deadline = Some(Instant::now() + HEAD_WAIT);
                    Ok(frame)
                }
            };
            match frame {
                Ok(Some(Ok(frame))) => match frame.into_data() {
                    Ok(mut data) => {
                        let take = data.len().min(MAX_HELD - held.len());
                        held.extend_from_slice(&data.split_to(take));
                        if !data.is_empty() {
                            self.frames.push_back(Ok(Frame::data(data)));
                        }
                    }
                    Err(trailers) => {
                        self.frames.push_back(Ok(trailers));
                        break None;
                    }
                },
                Ok(Some(Err(error))) => {
                    self.frames.push_back(Err(error));
                    break None;
                }
                Ok(None) | Err(_) => break None,
            }
        };
        let mut result = match place(&held) {
            Place::Nowhere => Preparation::NoHead,
            Place::More if held.len() >= MAX_HELD => Preparation::NoHead,
            _ => Preparation::CssOnly,
        };
        if let Some((changed, bytes)) =
            head.and_then(|head| rewrite(head, &held, headers, header_charset, script, css))
        {
            *headers = changed;
            held = bytes;
            result = Preparation::Injected;
        }
        if !held.is_empty() {
            self.frames.push_front(Ok(Frame::data(Bytes::from(held))));
        }
        result
    }
}

impl<B: Body<Data = Bytes> + Unpin> Body for Prefix<B>
where
    B::Error: Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, B::Error>>> {
        if let Some(frame) = self.frames.pop_front() {
            return Poll::Ready(Some(frame));
        }
        Pin::new(&mut self.inner).poll_frame(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::Full;
    use hyper::header::CONTENT_SECURITY_POLICY_REPORT_ONLY;
    use hyper::header::CONTENT_TYPE;

    #[tokio::test]
    async fn script_hashes_are_scoped_to_elements_in_each_header_and_meta_policies_stay_intact() {
        let start = r#"<!doctype html><html><head data-value=">">"#;
        let rest = r#"<!-- <meta http-equiv=CSP> --><meta other="kept" HTTP-EQUIV="Content-Security-Policy" CONTENT='default-src &#39;self&#39;; require-trusted-types-for &NewLine;&#39;script&#39;'><script src=page.js></script></head><body>x"#;
        let mut headers = HeaderMap::new();
        headers.append(CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; object-src 'none', script-src-elem 'strict-dynamic'; style-src 'none'"));
        headers.append(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("script-src 'self'; script-src-attr 'none'"),
        );
        headers.append(
            CONTENT_SECURITY_POLICY_REPORT_ONLY,
            HeaderValue::from_static("script-src 'none'"),
        );
        let mut body = Prefix::new(Full::new(Bytes::from(format!("{start}{rest}"))));
        let script = "window.fixture = 42;\n";
        assert!(matches!(
            body.prepare(&mut headers, script, Some("div{}"), false)
                .await,
            Preparation::Injected
        ));
        let hash = source(script);
        assert_eq!(
            body.collect().await.unwrap().to_bytes(),
            format!("{start}<script>{script}</script><style>div{{}}</style>{rest}")
        );
        let policies: Vec<_> = headers
            .get_all(CONTENT_SECURITY_POLICY)
            .iter()
            .map(|h| h.to_str().unwrap())
            .collect();
        assert_eq!(
            policies,
            [
                format!(
                    "default-src 'self'; object-src 'none'; script-src-elem 'self' {hash}, script-src-elem 'strict-dynamic' {hash}; style-src 'none'"
                ),
                format!("script-src 'self'; script-src-attr 'none'; script-src-elem 'self' {hash}"),
            ]
        );
        assert_eq!(
            headers[CONTENT_SECURITY_POLICY_REPORT_ONLY],
            "script-src 'none'"
        );
    }

    #[tokio::test]
    async fn trusted_types_and_sandbox_in_response_headers_leave_the_script_out() {
        for policy in [
            "require-trusted-types-for 'script'; script-src 'self'",
            "sandbox; script-src 'self'",
        ] {
            let before = "<html><head></head><body>original";
            let mut headers = HeaderMap::new();
            headers.insert(
                CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(policy).unwrap(),
            );
            let mut body = Prefix::new(Full::new(Bytes::from_static(before.as_bytes())));
            assert!(matches!(
                body.prepare(&mut headers, "window.fixture = 42", Some("div{}"), false)
                    .await,
                Preparation::CssOnly
            ));
            assert_eq!(headers[CONTENT_SECURITY_POLICY], policy);
            assert_eq!(body.collect().await.unwrap().to_bytes(), before);
        }
    }

    #[tokio::test]
    async fn a_transport_charset_only_needs_the_start_of_the_head() {
        let start = "<head>";
        let rest = "<template><meta charset=iso-2022-jp></template>";
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        let mut body = Prefix::new(Full::new(Bytes::from(format!("{start}{rest}"))));
        let script = "window.fixture = 42;";
        assert!(matches!(
            body.prepare(&mut headers, script, None, false).await,
            Preparation::Injected
        ));
        assert_eq!(
            body.collect().await.unwrap().to_bytes(),
            format!("{start}<script>{script}</script>{rest}")
        );
    }

    #[tokio::test]
    async fn a_gbk_page_gets_the_ascii_script_and_keeps_its_own_bytes() {
        for (header, declaration) in [
            (Some("text/html; charset=GBK"), ""),
            (None, "<meta charset=gbk>"),
        ] {
            let mut headers = HeaderMap::new();
            if let Some(header) = header {
                headers.insert(CONTENT_TYPE, HeaderValue::from_static(header));
            }
            headers.insert(
                CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("script-src 'self'"),
            );
            let suffix = [declaration.as_bytes(), b"</head><body>\xb9\xe3\xb8\xe6"].concat();
            let mut body = Prefix::new(Full::new(Bytes::from(
                [b"<head>".as_slice(), &suffix].concat(),
            )));
            let script = "window.fixture = 42;";
            assert!(matches!(
                body.prepare(&mut headers, script, None, false).await,
                Preparation::Injected
            ));
            assert_eq!(
                body.collect().await.unwrap().to_bytes(),
                [
                    format!("<head><script>{script}</script>").as_bytes(),
                    &suffix
                ]
                .concat()
            );
            assert_eq!(
                headers[CONTENT_SECURITY_POLICY],
                format!(
                    "script-src 'self'; script-src-elem 'self' {}",
                    source(script)
                )
            );
        }
    }

    #[tokio::test]
    async fn the_script_is_skipped_in_incompatible_charsets_or_when_its_bytes_would_change() {
        let page = b"<head></head><body>original";
        for (encoding, script) in [
            ("UTF-16LE", "window.fixture = 42"),
            ("UTF-16BE", "window.fixture = 42"),
            ("ISO-2022-JP", "window.fixture = 42"),
            ("iso-2022-kr", "window.fixture = 42"), // replacement label
            ("GBK", "window.fixture = '漢'"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_str(&format!("text/html; charset={encoding}")).unwrap(),
            );
            let mut body = Prefix::new(Full::new(Bytes::from_static(page)));
            assert!(matches!(
                body.prepare(&mut headers, script, None, false).await,
                Preparation::CssOnly
            ));
            assert_eq!(body.collect().await.unwrap().to_bytes(), page.as_slice());
        }
    }

    #[test]
    fn procedural_actions_are_escaped_and_only_the_first_128_small_actions_fit() {
        let actions = (0..=MAX_ACTIONS)
            .rev()
            .map(|i| format!(r#"{{"index":"{i:03}"}}"#))
            .chain([format!(r#"{{"index":"-{}"}}"#, "x".repeat(MAX_ACTION))])
            .collect();
        let script = assemble(String::new(), actions).unwrap();
        assert!(script.contains(r#"{"index":"000"}"#));
        assert!(script.contains(r#"{"index":"127"}"#));
        assert!(!script.contains(r#"{"index":"128"}"#));
        assert!(!script.contains(r#"{"index":"-"#));
        assert_eq!(script.matches(r#"{"index":"#).count(), MAX_ACTIONS);

        let script = assemble(
            String::new(),
            HashSet::from([r#"{"selector":[{"type":"has-text","arg":"</script>漢"}]}"#.to_owned()]),
        )
        .unwrap();
        assert!(script.contains(r#"\u003c/script>\u6f22"#));
        assert!(safe_raw_text(&script) && script.is_ascii());
    }

    #[test]
    fn script_budget_trims_whole_calls_in_a_stable_order_and_reserves_them_before_actions() {
        let block = |arg: &str| format!("{CALL_START}\nfixture(\"{arg}\"){CALL_END}");
        let kept = block(&"a".repeat(MAX_SCRIPT / 2));
        let dropped = block(&"b".repeat(MAX_SCRIPT / 2));
        let small = block(r#"c\n} catch ( e ) { }\ntry {\n"#);
        let deps = "function fixture(arg) {\n    try { globalThis.fixture = arg; } catch {}\n}\n";
        let forward = assemble(format!("{deps}{kept}{dropped}{small}"), HashSet::new()).unwrap();
        let reverse = assemble(format!("{deps}{small}{dropped}{kept}"), HashSet::new()).unwrap();
        assert_eq!(forward, reverse);
        assert_eq!(
            forward,
            format!("{SCRIPTLET_START}{deps}{kept}{small}{SCRIPTLET_END}")
        );

        let actions = (0..MAX_ACTIONS)
            .map(|i| format!(r#"{{"index":"{i:03}","arg":"{}"}}"#, "漢".repeat(1000)))
            .collect();
        let combined = assemble(format!("{deps}{kept}{small}"), actions).unwrap();
        assert!(combined.len() <= MAX_SCRIPT);
        assert!(combined.ends_with(&forward));
        assert!(combined.contains(r#"{"index":"000","#));
        assert!(!combined.contains(r#"{"index":"127","#));
        let json = combined
            .strip_prefix(include_str!("../../resources/procedural.js").trim_end())
            .unwrap()
            .strip_prefix('(')
            .unwrap()
            .strip_suffix(&forward)
            .unwrap()
            .strip_suffix(");\n")
            .unwrap();
        serde_json::from_str::<Vec<serde_json::Value>>(json).unwrap();
    }

    #[tokio::test]
    async fn the_placement_cannot_use_a_head_beyond_the_prefix_limit() {
        let before = format!("{}<head></head>", " ".repeat(MAX_HELD));
        let mut body = Prefix::new(Full::new(Bytes::from(before.clone())));
        assert!(matches!(
            body.prepare(&mut HeaderMap::new(), "script", None, false)
                .await,
            Preparation::NoHead
        ));
        assert_eq!(body.collect().await.unwrap().to_bytes(), before);
    }

    #[test]
    fn adding_a_hash_does_not_disable_a_policy_that_already_allows_inline_scripts() {
        let hash = source("script");
        assert_eq!(
            permit_policy("script-src 'self' 'unsafe-inline'", &hash),
            Ok(None)
        );
        assert_eq!(
            permit_policy(
                "script-src 'strict-dynamic' 'unsafe-inline' 'nonce-cGFnZQ=='",
                &hash
            ),
            Ok(Some(format!(
                "script-src 'strict-dynamic' 'unsafe-inline' 'nonce-cGFnZQ=='; script-src-elem 'strict-dynamic' 'unsafe-inline' 'nonce-cGFnZQ==' {hash}"
            )))
        );
    }
}
