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

//! Hiding what a blocked request leaves behind on a page: the lists name
//! the elements, and a style that hides them is put into the page as it
//! is passed on.
//!
//! Cosmetic filtering only changes a page where its head can be found
//! (`place`). Selectors must be safe inside a style element (`style`).
//! Scriptlets, trusted JavaScript rules and procedural actions share a
//! bounded script element. Where the page restricts inline styles or
//! scripts, their hashes are added to the appropriate policies (`csp`).

mod csp;
mod script;
pub(crate) use script::{
    JAVASCRIPT_END, JAVASCRIPT_START, Prefix, Preparation, assemble as script, safe_raw_text,
};

use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use base64::Engine as _;
use bytes::Bytes;
use hyper::body::{Body, Frame};
use hyper::header::{
    CONTENT_ENCODING, CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY_REPORT_ONLY, CONTENT_TYPE,
    HeaderMap, HeaderValue,
};
use hyper::{Response, StatusCode};

use csp::{By, Element};

/// The longest selector taken from a list, and the most of them together
/// that are put into one page. A page of a site the lists know well has
/// some hundreds, a few kilobytes.
const MAX_SELECTOR: usize = 1024;
const MAX_STYLE: usize = 128 << 10;

/// How much of a page is looked through for the beginning of its head,
/// and so the most of one that is held back at a time. A head begins
/// within the first few hundred bytes.
const MAX_HELD: usize = crate::html::MAX_CHARSET_PREFIX;

const HIDE: &str = "{display:none!important}\n";

/// Whether a selector can be put into a page as it is: nothing in it ends
/// the style element or the rule it is the selector of, or begins one of
/// another kind.
fn safe(selector: &str) -> bool {
    !selector.is_empty()
        && selector.len() <= MAX_SELECTOR
        && !selector.starts_with('@')
        && !selector.contains("/*")
        && !selector
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '{' | '}'))
}

fn ascii_selector(selector: String) -> String {
    if selector.is_ascii() {
        return selector;
    }
    let mut ascii = String::with_capacity(selector.len());
    let mut escaped = false;
    for c in selector.chars() {
        if c.is_ascii() {
            ascii.push(c);
        } else {
            // Reuse a backslash already escaping this code point.
            if !escaped {
                ascii.push('\\');
            }
            write!(ascii, "{:06x} ", u32::from(c)).unwrap();
        }
        escaped = c == '\\' && !escaped;
    }
    ascii
}

/// The style that hides what `selectors` name, or none if there is
/// nothing to hide. A rule each: one the browser does not understand is
/// then left out by it alone.
pub fn style(selectors: impl IntoIterator<Item = String>) -> Option<String> {
    let mut selectors: Vec<String> = selectors.into_iter().filter(|s| safe(s)).collect();
    // The same style for the same page each time, whatever order the
    // lists gave them in.
    selectors.sort_unstable();
    let mut css = String::new();
    for selector in selectors {
        let selector = ascii_selector(selector);
        if css.len() + selector.len() + HIDE.len() > MAX_STYLE {
            break;
        }
        css += &selector;
        css += HIDE;
    }
    (!css.is_empty()).then_some(css)
}

/// Whether a response is a page a style can be put into: a whole one, in
/// HTML, and not packed. The request asked for it unpacked; a server that
/// packed it all the same is passed on as it is.
pub fn takes<B>(resp: &Response<B>) -> bool {
    let header = |name| resp.headers().get(name).and_then(|v| v.to_str().ok());
    let html = header(CONTENT_TYPE).is_some_and(|t| {
        let kind = t.split(';').next().unwrap_or("").trim();
        kind.eq_ignore_ascii_case("text/html")
    });
    let plain = match resp.headers().get(CONTENT_ENCODING) {
        None => true,
        Some(v) => v.as_bytes().eq_ignore_ascii_case(b"identity"),
    };
    resp.status() == StatusCode::OK && html && plain
}

pub use crate::html::{Place, place};

/// `'sha256-…'`, as a page names an inline style it allows.
fn source(css: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, css.as_bytes());
    let encoded = base64::engine::general_purpose::STANDARD.encode(digest);
    format!("'sha256-{encoded}'")
}

/// One policy with the style allowed by it, if the policy would not have
/// allowed it.
fn permit_in(policy: &str, source: &str) -> Option<String> {
    let (directive, by) = csp::effective(policy, Element::Style)?;
    // Every inline style is allowed already; naming one by its hash would
    // take that away from the page's own.
    if directive.has("'unsafe-inline'") && !directive.sources.clone().any(csp::hash_or_nonce) {
        return None;
    }
    let mut allowed: Vec<&str> = (directive.sources)
        .filter(|s| !s.eq_ignore_ascii_case("'none'"))
        .collect();
    allowed.push(source);
    let mut directives: Vec<String> = policy.split(';').map(|d| d.trim().to_owned()).collect();
    if by == By::Default {
        // Not added to what everything falls back on: scripts go by it too.
        directives.push(format!("style-src {}", allowed.join(" ")));
    } else {
        directives[directive.index] = format!("{} {}", directive.name, allowed.join(" "));
    }
    directives.retain(|d| !d.is_empty());
    Some(directives.join("; "))
}

/// Has the page allow the style, where it says which inline styles it
/// allows and this would not be one. Nothing else is allowed by it: the
/// hash is that of this text. A policy that cannot be read is left alone,
/// and the style is then not applied by the browser.
pub fn permit(headers: &mut HeaderMap, css: &str) {
    let source = source(css);
    for name in [CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY_REPORT_ONLY] {
        let values: Vec<HeaderValue> = headers.get_all(&name).iter().cloned().collect();
        let mut changed = false;
        let new: Vec<HeaderValue> = values
            .iter()
            .map(|value| {
                let Ok(text) = value.to_str() else {
                    return value.clone();
                };
                // A header may hold several policies; each must allow it.
                let policies: Vec<String> = text
                    .split(',')
                    .map(|policy| match permit_in(policy, &source) {
                        Some(permitting) => {
                            changed = true;
                            permitting
                        }
                        None => policy.trim().to_owned(),
                    })
                    .collect();
                HeaderValue::from_str(&policies.join(", ")).unwrap_or_else(|_| value.clone())
            })
            .collect();
        if changed {
            headers.remove(&name);
            for value in new {
                headers.append(&name, value);
            }
        }
    }
}

/// How a page lets an inline style in that it does not name by its hash:
/// what one put in after the page has been looked through must go by,
/// when the policy has long been sent.
#[derive(Debug, PartialEq, Eq)]
pub enum Inline {
    /// Any is let in.
    Any,
    /// One that carries this nonce, which the page's own styles carry.
    Nonce(String),
    /// None that could be told of now.
    No,
}

/// What one policy has for a style element.
fn inline_in(policy: &str) -> Inline {
    let Some((directive, _)) = csp::effective(policy, Element::Style) else {
        return Inline::Any;
    };
    if let Some(nonce) = directive.sources.clone().find_map(csp::nonce) {
        return Inline::Nonce(nonce.to_owned());
    }
    if directive.has("'unsafe-inline'") && !directive.sources.clone().any(csp::hash_or_nonce) {
        Inline::Any
    } else {
        Inline::No
    }
}

/// How the page lets in an inline style not named by its hash: by every
/// policy it has, or not at all. Nothing is changed for it: a page that
/// lets none in has the rules that need one left out.
pub fn inline(headers: &HeaderMap) -> Inline {
    let mut all = Inline::Any;
    for value in headers.get_all(CONTENT_SECURITY_POLICY) {
        let Ok(text) = value.to_str() else {
            return Inline::No;
        };
        for policy in text.split(',') {
            all = match (all, inline_in(policy)) {
                (Inline::Any, one) => one,
                (all, Inline::Any) => all,
                (Inline::Nonce(a), Inline::Nonce(b)) if a == b => Inline::Nonce(a),
                _ => return Inline::No,
            };
        }
    }
    all
}

/// The most classes, and ids, kept of one page, and the longest: more or
/// longer than any page has that means them.
const MAX_NAMES: usize = 4096;
const MAX_NAME: usize = 64;

/// How much of a tag is held when it comes in pieces. One longer is not
/// looked into.
const MAX_TAG: usize = 4096;

/// The classes and ids a page has, gathered as it passes.
///
/// Not by reading the page as a browser does: every tag is looked into
/// for `class` and `id`, whatever it is and wherever. What is taken for a
/// name without being one hides nothing, there being nothing of that name
/// on the page; a name missed leaves something shown.
#[derive(Default)]
pub struct Names {
    pub classes: HashSet<String>,
    pub ids: HashSet<String>,
    /// The beginning of a tag whose end has not come yet.
    tag: Vec<u8>,
}

impl Names {
    fn keep(into: &mut HashSet<String>, name: &[u8]) {
        let plain = !name.is_empty()
            && name.len() <= MAX_NAME
            && name
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
        // Repeated classes and ids can be looked up while still borrowed.
        if plain
            && into.len() < MAX_NAMES
            && let Ok(name) = std::str::from_utf8(name)
            && !into.contains(name)
        {
            into.insert(name.to_owned());
        }
    }

    /// Takes the names of one tag, from after its `<` to before its `>`.
    fn tag(&mut self, tag: &[u8]) {
        let mut at = 0;
        while at < tag.len() {
            // An attribute begins after white space.
            if !tag[at].is_ascii_whitespace() {
                at += 1;
                continue;
            }
            let rest = &tag[at + 1..];
            let (into, len) = if rest.len() >= 5 && rest[..5].eq_ignore_ascii_case(b"class") {
                (&mut self.classes, 5)
            } else if rest.len() >= 2 && rest[..2].eq_ignore_ascii_case(b"id") {
                (&mut self.ids, 2)
            } else {
                at += 1;
                continue;
            };
            let mut value = rest[len..].trim_ascii_start();
            at += 1 + len;
            let Some(after) = value.strip_prefix(b"=") else {
                continue;
            };
            value = after.trim_ascii_start();
            let value = match value.first() {
                Some(quote @ (b'"' | b'\'')) => {
                    let inside = &value[1..];
                    &inside[..inside
                        .iter()
                        .position(|b| b == quote)
                        .unwrap_or(inside.len())]
                }
                _ => {
                    &value[..value
                        .iter()
                        .position(u8::is_ascii_whitespace)
                        .unwrap_or(value.len())]
                }
            };
            for name in value.split(u8::is_ascii_whitespace) {
                Self::keep(into, name);
            }
        }
    }

    /// Looks through the next piece of the page.
    pub fn scan(&mut self, mut bytes: &[u8]) {
        loop {
            // In a tag begun in a piece before: up to its end.
            if !self.tag.is_empty() {
                match bytes.iter().position(|b| *b == b'>') {
                    Some(end) if self.tag.len() + end <= MAX_TAG => {
                        let mut tag = std::mem::take(&mut self.tag);
                        tag.extend_from_slice(&bytes[..end]);
                        self.tag(&tag[1..]);
                        bytes = &bytes[end + 1..];
                    }
                    Some(end) => {
                        self.tag.clear();
                        bytes = &bytes[end + 1..];
                    }
                    None if self.tag.len() + bytes.len() <= MAX_TAG => {
                        self.tag.extend_from_slice(bytes);
                        return;
                    }
                    // Too long to be looked into: let go, and what is
                    // left of it taken for text.
                    None => {
                        self.tag.clear();
                        return;
                    }
                }
            }
            let Some(start) = bytes.iter().position(|b| *b == b'<') else {
                return;
            };
            match bytes[start..].iter().position(|b| *b == b'>') {
                Some(len) => {
                    self.tag(&bytes[start + 1..start + len]);
                    bytes = &bytes[start + len + 1..];
                }
                None => {
                    if bytes.len() - start <= MAX_TAG {
                        self.tag.extend_from_slice(&bytes[start..]);
                    }
                    return;
                }
            }
        }
    }
}

/// The style for the names a page was found to have, if any is hidden.
pub type ByName = Box<dyn FnOnce(&Names) -> Option<String> + Send + Sync>;

/// A page passed on with what hides the lists' elements put in: one style
/// where its head begins, and one at its end for what goes by the classes
/// and ids it was found to have. A page with no head written out is
/// passed on as it came.
pub struct Inject<B> {
    inner: B,
    /// Whether it is still to be found where the head begins.
    placing: bool,
    /// The style for the head, until it is put in or found to have no
    /// place.
    element: Option<Bytes>,
    /// The beginning of the page, held until that is known: `MAX_HELD`
    /// at most.
    held: Vec<u8>,
    /// What is gathered for the style at the end, and what makes it.
    names: Option<(Names, ByName)>,
    /// What that style must carry to be let in.
    nonce: Option<String>,
    ready: VecDeque<Frame<Bytes>>,
    ended: bool,
}

impl<B> Inject<B> {
    /// `css` goes into the head. `by_name` is asked at the end of the
    /// page, and what it gives goes there, carrying `nonce` if the page
    /// lets a style in by one.
    pub fn new(
        inner: B,
        css: Option<&str>,
        by_name: Option<ByName>,
        nonce: Option<String>,
    ) -> Self {
        Self {
            inner,
            placing: true,
            element: css.map(|css| Bytes::from(format!("<style>{css}</style>"))),
            held: Vec::new(),
            names: by_name.map(|by_name| (Names::default(), by_name)),
            nonce,
            ready: VecDeque::new(),
            ended: false,
        }
    }

    /// Passes on what was held, with the style after its first `at` bytes
    /// if there is a place for it. A page with none is not one to put
    /// anything into, at its end either.
    fn release(&mut self, at: Option<usize>) {
        self.placing = false;
        let element = self.element.take();
        let mut held = std::mem::take(&mut self.held);
        match at {
            Some(at) => {
                if let Some(element) = element {
                    held.splice(at..at, element);
                }
            }
            None => self.names = None,
        }
        if !held.is_empty() {
            self.ready.push_back(Frame::data(Bytes::from(held)));
        }
    }

    /// The page has come to its end: what was held is passed on, and the
    /// style for what it was found to have after it.
    fn finish(&mut self) {
        if self.placing {
            self.release(None);
        }
        if let Some((names, by_name)) = self.names.take()
            && let Some(css) = by_name(&names)
        {
            let element = match &self.nonce {
                Some(nonce) => format!("<style nonce=\"{nonce}\">{css}</style>"),
                None => format!("<style>{css}</style>"),
            };
            self.ready.push_back(Frame::data(Bytes::from(element)));
        }
    }
}

impl<B> Body for Inject<B>
where
    B: Body<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, B::Error>>> {
        let this = &mut *self;
        loop {
            if let Some(frame) = this.ready.pop_front() {
                return Poll::Ready(Some(Ok(frame)));
            }
            if this.ended {
                return Poll::Ready(None);
            }
            // Nothing more to do to the page: passed on as it comes.
            if !this.placing && this.names.is_none() {
                return Pin::new(&mut this.inner).poll_frame(cx);
            }
            match ready!(Pin::new(&mut this.inner).poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(mut data) => {
                        if let Some((names, _)) = &mut this.names {
                            names.scan(&data);
                        }
                        if !this.placing {
                            this.ready.push_back(Frame::data(data));
                            continue;
                        }
                        let take = data.len().min(MAX_HELD - this.held.len());
                        this.held.extend_from_slice(&data.split_to(take));
                        match place(&this.held) {
                            Place::At(at) => this.release(Some(at)),
                            Place::More if this.held.len() < MAX_HELD => {}
                            Place::More | Place::Nowhere => this.release(None),
                        }
                        if !data.is_empty() {
                            this.ready.push_back(Frame::data(data));
                        }
                    }
                    // What ends the page: there is no more of it to come.
                    Err(trailers) => {
                        this.finish();
                        this.ready.push_back(trailers);
                    }
                },
                Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                None => {
                    this.finish();
                    this.ended = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::{BodyExt, StreamBody};

    use super::*;

    fn css(selectors: &[&str]) -> Option<String> {
        style(selectors.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn a_style_hides_each_selector_by_a_rule_of_its_own() {
        assert_eq!(
            css(&[".b > a", "#a"]).unwrap(),
            "#a{display:none!important}\n.b > a{display:none!important}\n"
        );
        assert_eq!(css(&[]), None);
    }

    #[tokio::test]
    async fn css_escapes_keep_the_style_hash_valid_on_windows_1252() {
        use http_body_util::Full;

        let css = css(&[r#".café[data-label="廣🦀"]"#, r".\é"]).unwrap();
        assert_eq!(
            css,
            ".\\0000e9 {display:none!important}\n\
             .caf\\0000e9 [data-label=\"\\005ee3 \\01f980 \"]{display:none!important}\n"
        );
        assert!(css.is_ascii());
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=windows-1252"),
        );
        headers.insert(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("style-src 'none'"),
        );
        permit(&mut headers, &css);
        let original = Bytes::from_static(b"<head></head><p class=caf\xe9>");
        let body = Inject::new(Full::new(original.clone()), Some(&css), None, None);
        let result = body.collect().await.unwrap().to_bytes();
        let emitted = result
            .strip_prefix(b"<head><style>")
            .unwrap()
            .strip_suffix(b"</style></head><p class=caf\xe9>")
            .unwrap();
        let emitted = std::str::from_utf8(emitted).unwrap();
        assert_eq!(css, emitted);
        assert_eq!(
            headers[CONTENT_SECURITY_POLICY],
            format!("style-src {}", source(emitted))
        );
    }

    #[test]
    fn a_list_cannot_put_anything_but_selectors_into_a_page() {
        for hostile in [
            "a</style><script>alert(1)</script>",
            "a{} body{display:none} b",
            "a}",
            "@import url(https://evil.example/x.css)",
            "a /* b",
            "a\nb",
            "",
            &"a".repeat(MAX_SELECTOR + 1),
        ] {
            assert_eq!(css(&[hostile]), None, "{hostile:?}");
        }
        // Left out by itself: the others still hide.
        assert_eq!(
            css(&["a</style>", ".ad"]).unwrap(),
            ".ad{display:none!important}\n"
        );
        // What a selector may well hold.
        assert!(css(&["div[data-x=\"a > b\"]:not(.c) + a[href^='https://x.example/']"]).is_some());
    }

    #[test]
    fn a_style_is_no_longer_than_its_bound() {
        let many = (0..100_000).map(|n| format!(".ad-{n}"));
        let css = style(many).unwrap();
        assert!(css.len() <= MAX_STYLE && css.ends_with(HIDE));
    }

    #[test]
    fn the_style_goes_right_after_the_tag_that_begins_the_head() {
        for (html, want) in [
            ("<head>", Place::At(6)),
            (
                "<!DOCTYPE html><html lang=\"en\"><HEAD ><title>",
                Place::At(38),
            ),
            (
                "\u{feff} \n<!-- a <head> in a comment -->\n<html>\n<head>x",
                Place::At(49),
            ),
            // Not known yet.
            ("", Place::More),
            ("<!doctype html>\n", Place::More),
            ("<", Place::More),
            ("<!-", Place::More),
            ("<!-- never ended", Place::More),
            ("<he", Place::More),
            ("<head data-x=\"1\"", Place::More),
            // No head written out: not a page to put anything into.
            ("<html><body>", Place::Nowhere),
            ("<script>var a = \"<head>\";</script>", Place::Nowhere),
            ("{\"json\": true}", Place::Nowhere),
            ("<header>", Place::Nowhere),
            ("<aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", Place::Nowhere),
        ] {
            assert_eq!(place(html.as_bytes()), want, "{html:?}");
        }
    }

    async fn passed_on(chunks: &[&str]) -> String {
        let frames: Vec<Result<Frame<Bytes>, std::convert::Infallible>> = chunks
            .iter()
            .map(|c| Ok(Frame::data(Bytes::from((*c).to_owned()))))
            .collect();
        let body = StreamBody::new(futures_lite(frames));
        let all = Inject::new(body, Some("S"), None, None)
            .collect()
            .await
            .unwrap()
            .to_bytes();
        String::from_utf8(all.to_vec()).unwrap()
    }

    /// A stream of what is given, with no crate for it.
    fn futures_lite<T: Unpin>(items: Vec<T>) -> impl futures_core::Stream<Item = T> + Unpin {
        struct Each<T>(VecDeque<T>);
        impl<T: Unpin> futures_core::Stream for Each<T> {
            type Item = T;
            fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<T>> {
                Poll::Ready(self.0.pop_front())
            }
        }
        Each(items.into())
    }

    #[tokio::test]
    async fn a_page_is_passed_on_with_the_style_in_its_head_however_it_arrives() {
        let want = "<!doctype html><html><head><style>S</style><title>t</title></head><body>b";
        let page = "<!doctype html><html><head><title>t</title></head><body>b";
        assert_eq!(passed_on(&[page]).await, want);
        // A byte at a time.
        let bytes: Vec<String> = page.chars().map(String::from).collect();
        let bytes: Vec<&str> = bytes.iter().map(String::as_str).collect();
        assert_eq!(passed_on(&bytes).await, want);
        assert_eq!(
            passed_on(&["<!doctype html><html><he", "ad>rest"]).await,
            "<!doctype html><html><head><style>S</style>rest"
        );
    }

    #[tokio::test]
    async fn a_page_with_no_place_for_it_is_passed_on_as_it_came() {
        for chunks in [
            &["<html><body>", "text"][..],
            &["{\"a\":", "1}"],
            &[],
            &["<!doctype html>"],
            &["<!-- ", "never ended"],
        ] {
            assert_eq!(passed_on(chunks).await, chunks.concat(), "{chunks:?}");
        }
        // Not held back without end for a head that does not come.
        let long = format!("<!--{}", "x".repeat(2 * MAX_HELD));
        assert_eq!(passed_on(&[&long, "-->"]).await, format!("{long}-->"));
    }

    fn permitted(policy: &str) -> Option<String> {
        permit_in(policy, "'sha256-X'")
    }

    #[test]
    fn a_page_that_names_its_styles_is_made_to_name_this_one_too() {
        assert_eq!(
            permitted("default-src 'self'; style-src 'self' https://cdn.example").unwrap(),
            "default-src 'self'; style-src 'self' https://cdn.example 'sha256-X'"
        );
        assert_eq!(
            permitted("style-src 'self'; style-src-elem 'nonce-abc'").unwrap(),
            "style-src 'self'; style-src-elem 'nonce-abc' 'sha256-X'"
        );
        // Nothing allowed: this and nothing else.
        assert_eq!(
            permitted("STYLE-SRC 'none'").unwrap(),
            "STYLE-SRC 'sha256-X'"
        );
        // What scripts fall back on is left as it is.
        assert_eq!(
            permitted("default-src 'self'; script-src 'none'").unwrap(),
            "default-src 'self'; script-src 'none'; style-src 'self' 'sha256-X'"
        );
        // An inline style that a nonce already keeps out.
        assert_eq!(
            permitted("style-src 'unsafe-inline' 'nonce-abc'").unwrap(),
            "style-src 'unsafe-inline' 'nonce-abc' 'sha256-X'"
        );
    }

    #[test]
    fn a_page_that_allows_the_style_already_is_left_as_it_is() {
        // Naming one by its hash would forbid the page's own.
        assert_eq!(permitted("style-src 'self' 'unsafe-inline'"), None);
        assert_eq!(permitted("default-src * 'unsafe-inline'"), None);
        // Says nothing of styles.
        assert_eq!(permitted("script-src 'self'; frame-ancestors 'none'"), None);
        assert_eq!(permitted(""), None);
    }

    #[test]
    fn a_source_that_only_looks_like_a_hash_leaves_every_inline_style_allowed() {
        // 'unsafe-inline' still holds beside these: one more named by its
        // hash would forbid the page's own.
        for policy in [
            "style-src 'unsafe-inline' 'sha-garbage'",
            "style-src 'unsafe-inline' 'sha256-abc='",
            "style-src 'unsafe-inline' 'nonce-'",
        ] {
            assert_eq!(permitted(policy), None, "{policy}");
        }
    }

    #[test]
    fn every_policy_of_a_page_is_made_to_allow_it() {
        let mut headers = HeaderMap::new();
        headers.append(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("style-src 'self', script-src 'self'"),
        );
        headers.append(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'"),
        );
        headers.append(
            CONTENT_SECURITY_POLICY_REPORT_ONLY,
            HeaderValue::from_static("style-src 'unsafe-inline'"),
        );
        permit(&mut headers, "a{display:none!important}\n");
        let hash = source("a{display:none!important}\n");
        let said: Vec<&str> = headers
            .get_all(CONTENT_SECURITY_POLICY)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(
            said,
            [
                format!("style-src 'self' {hash}, script-src 'self'"),
                format!("default-src 'none'; style-src {hash}"),
            ]
        );
        assert_eq!(
            headers[CONTENT_SECURITY_POLICY_REPORT_ONLY],
            "style-src 'unsafe-inline'"
        );
    }

    #[test]
    fn the_hash_is_the_one_a_browser_makes_of_the_style() {
        // SHA-256 of the empty string, in base64.
        assert_eq!(
            source(""),
            "'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='"
        );
    }

    fn names(page: &[&str]) -> (Vec<String>, Vec<String>) {
        let mut names = Names::default();
        for piece in page {
            names.scan(piece.as_bytes());
        }
        let sorted = |set: &HashSet<String>| {
            let mut all: Vec<String> = set.iter().cloned().collect();
            all.sort();
            all
        };
        (sorted(&names.classes), sorted(&names.ids))
    }

    #[test]
    fn the_classes_and_ids_of_a_page_are_gathered_as_it_passes() {
        let page = "<html><body class=\"home dark home\"><div id=main CLASS='ad-box  wide'>\
                    <p id=main class='plain home'>class=\"in-text\"</p>\
                    <a href=\"/x?id=7\" data-id=\"no\">";
        let want = (
            vec!["ad-box", "dark", "home", "plain", "wide"],
            vec!["main"],
        );
        let got = names(&[page]);
        assert_eq!(got.0, want.0);
        assert_eq!(got.1, want.1);
        // The same however the page comes in pieces.
        for at in 0..page.len() {
            let (a, b) = page.split_at(at);
            assert_eq!(names(&[a, b]), got, "cut at {at}");
        }
        let bytes: Vec<String> = page.chars().map(String::from).collect();
        let bytes: Vec<&str> = bytes.iter().map(String::as_str).collect();
        assert_eq!(names(&bytes), got);
    }

    /// For looking into why a page is not changed as expected:
    /// `ABD_PAGE=<file> cargo test -- --ignored --nocapture looks_through`.
    #[test]
    #[ignore = "a look at one page, run by hand"]
    fn looks_through_a_page_from_a_file() {
        let Some(file) = std::env::var_os("ABD_PAGE") else {
            return;
        };
        let page = std::fs::read(file).unwrap();
        println!(
            "head begins: {:?}",
            place(&page[..page.len().min(MAX_HELD)])
        );
        let mut names = Names::default();
        for piece in page.chunks(1371) {
            names.scan(piece);
        }
        let mut ids: Vec<&String> = names.ids.iter().collect();
        ids.sort();
        println!(
            "{} classes, {} ids: {ids:?}",
            names.classes.len(),
            ids.len()
        );
    }

    #[test]
    fn what_is_gathered_of_a_page_is_bounded() {
        // No more names than the bound, none longer, none of other stuff.
        let mut page = format!(
            "<i class=\"{} a{{b}} e:f café\" id=\"invalid/id\">",
            "x".repeat(MAX_NAME + 1)
        );
        for n in 0..2 * MAX_NAMES {
            page += &format!("<i class=\"c{n} c{n}\" id=\"i{n}\">");
        }
        let mut names = Names::default();
        names.scan(page.as_bytes());
        assert_eq!(names.classes.len(), MAX_NAMES);
        assert_eq!(names.ids.len(), MAX_NAMES);
        assert!(names.classes.contains(&format!("c{}", MAX_NAMES - 1)));
        assert!(names.ids.contains(&format!("i{}", MAX_NAMES - 1)));
        assert!(!names.classes.contains(&format!("c{MAX_NAMES}")));
        assert!(!names.ids.contains(&format!("i{MAX_NAMES}")));
        // A tag with no end is not held without one.
        let mut names = Names::default();
        names.scan(format!("<i {}", "x".repeat(3 * MAX_TAG)).as_bytes());
        names.scan(b" class=\"late\">");
        assert!(names.tag.len() <= MAX_TAG);
        assert!(names.classes.is_empty());
    }

    async fn passed_on_with_names(chunks: &[&str], nonce: Option<&str>) -> String {
        let frames: Vec<Result<Frame<Bytes>, std::convert::Infallible>> = chunks
            .iter()
            .map(|c| Ok(Frame::data(Bytes::from((*c).to_owned()))))
            .collect();
        let body = StreamBody::new(futures_lite(frames));
        // Hides whatever has the class `ad`.
        let by_name: ByName =
            Box::new(|names| names.classes.contains("ad").then(|| "N".to_owned()));
        let nonce = nonce.map(str::to_owned);
        let all = Inject::new(body, None, Some(by_name), nonce)
            .collect()
            .await
            .unwrap()
            .to_bytes();
        String::from_utf8(all.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn what_goes_by_a_name_the_page_has_is_hidden_by_a_style_at_its_end() {
        let page = [
            "<html><head></head><body><div cla",
            "ss=\"ad\">x</div></body></html>",
        ];
        assert_eq!(
            passed_on_with_names(&page, None).await,
            format!("{}<style>N</style>", page.concat())
        );
        // Carrying what the page lets a style in by.
        assert_eq!(
            passed_on_with_names(&page, Some("abc")).await,
            format!("{}<style nonce=\"abc\">N</style>", page.concat())
        );
        // Nothing of that name: as it came.
        let plain = ["<html><head></head><body><div class=\"content\">x</div>"];
        assert_eq!(passed_on_with_names(&plain, None).await, plain.concat());
        // No head written out: not a page to put anything into.
        let headless = ["<div class=\"ad\">x</div>"];
        assert_eq!(
            passed_on_with_names(&headless, None).await,
            headless.concat()
        );
    }

    #[test]
    fn a_style_at_the_end_goes_by_what_every_policy_of_the_page_lets_in() {
        let of = |policies: &[&'static str]| {
            let mut headers = HeaderMap::new();
            for policy in policies {
                headers.append(CONTENT_SECURITY_POLICY, HeaderValue::from_static(policy));
            }
            inline(&headers)
        };
        assert_eq!(of(&[]), Inline::Any);
        assert_eq!(of(&["script-src 'self'"]), Inline::Any);
        assert_eq!(of(&["style-src 'self' 'unsafe-inline'"]), Inline::Any);
        let nonce = Inline::Nonce("r4nd0m+/=".to_owned());
        assert_eq!(of(&["default-src 'self' 'nonce-r4nd0m+/='"]), nonce);
        assert_eq!(
            of(&[
                "style-src 'nonce-r4nd0m+/='",
                "script-src 'none', img-src *"
            ]),
            nonce
        );
        // Named by hash, or not let in at all: nothing to go by now.
        assert_eq!(of(&["style-src 'self'"]), Inline::No);
        assert_eq!(
            of(&[
                "style-src 'unsafe-inline' 'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='"
            ]),
            Inline::No
        );
        assert_eq!(of(&["default-src 'none'"]), Inline::No);
        // Two policies, each with a nonce of its own.
        assert_eq!(
            of(&["style-src 'nonce-a', style-src 'nonce-b'"]),
            Inline::No
        );
        // A nonce that could end the attribute it would be put into.
        assert_eq!(of(&["style-src 'nonce-a\"><script>'"]), Inline::No);
    }

    #[test]
    fn only_a_whole_plain_page_in_html_is_changed() {
        let resp = |status: u16, kind: &str, encoding: Option<&str>| {
            let mut r = Response::builder()
                .status(status)
                .header(CONTENT_TYPE, kind);
            if let Some(e) = encoding {
                r = r.header(CONTENT_ENCODING, e);
            }
            r.body(()).unwrap()
        };
        assert!(takes(&resp(200, "text/html", None)));
        assert!(takes(&resp(
            200,
            "Text/HTML; charset=utf-8",
            Some("identity")
        )));
        assert!(!takes(&resp(200, "text/html", Some("gzip"))));
        assert!(!takes(&resp(206, "text/html", None)));
        assert!(!takes(&resp(304, "text/html", None)));
        assert!(!takes(&resp(200, "application/json", None)));
        assert!(!takes(&resp(200, "text/htmlx", None)));
    }
}
