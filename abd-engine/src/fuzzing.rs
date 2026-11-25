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

//! Entry points for the standalone parser fuzz targets.

use std::sync::{Arc, LazyLock};
use std::{hint::black_box, path::Path, str};

use bytes::Bytes;
use hyper::Uri;
use hyper::header::{
    CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY_REPORT_ONLY, HeaderMap, HeaderValue,
    SET_COOKIE,
};

use crate::filter::Filters;
use crate::htmlfilter;
use crate::jsonpath;
use crate::list_files::{MAX_LIST, MAX_LISTS};
use crate::rule::action::{ActionKind, ResponseAction, cookie, hls};
use crate::rule::{self, Kind};
use crate::{clienthello, cosmetic, dns, quic};

#[cfg(test)]
#[path = "../../fuzz/replay.rs"]
mod replay;

// Let each parser reach its own limits without a clock-dependent cutoff.
// libFuzzer imposes a timeout on each input.
fn within_budget() -> bool {
    true
}

fn expression_and_body(input: &[u8]) -> Option<(&str, &[u8])> {
    let mut parts = input.splitn(2, |&byte| byte == b'\n');
    let expression = str::from_utf8(parts.next()?).ok()?;
    Some((expression, parts.next()?))
}

pub fn json_prune(input: &[u8]) {
    static PATHS: LazyLock<[jsonpath::Path; 4]> = LazyLock::new(|| {
        ["$..ad", "$.items[?(has ad)]", "$[::-2]", "$.ads.*"]
            .map(|path| jsonpath::Path::parse(path).expect("valid JSONPath fixture"))
    });
    for path in PATHS.iter() {
        let _ = jsonpath::prune(path, input, &within_budget);
    }
    if let Some((expression, body)) = expression_and_body(input)
        && let Some(path) = jsonpath::Path::parse(expression)
    {
        let _ = jsonpath::prune(&path, body, &within_budget);
    }
}

pub fn html_filter(input: &[u8]) {
    static RULES: LazyLock<[Arc<htmlfilter::Compiled>; 4]> = LazyLock::new(|| {
        [
            "example.org$$div[id=ad]",
            "example.org$$script",
            "example.org$$table > tbody > tr[data-ad]",
            "example.org##^p:contains(/sponsor/i)",
        ]
        .map(|line| {
            let Kind::Html(rule) = rule::read(line).kind else {
                panic!("invalid HTML rule fixture: {line}");
            };
            Arc::new(rule.compiled)
        })
    });
    let body = Bytes::copy_from_slice(input);
    for (charset, xhtml) in [(None, false), (Some("windows-1252"), false), (None, true)] {
        let _ = htmlfilter::apply(body.clone(), &*RULES, charset, xhtml, &within_budget);
    }
}

pub fn playlist(input: &[u8]) {
    static BASE: LazyLock<Uri> =
        LazyLock::new(|| "https://media.example/hls/list.m3u8".parse().unwrap());
    static RULES: LazyLock<[hls::Compiled; 3]> = LazyLock::new(|| {
        ["ad.ts", "/sponsor/it", r"||media.example\/ads\/"].map(|value| {
            let ResponseAction::Hls(rule) = compile_action(ActionKind::Hls, value) else {
                panic!("invalid playlist rule fixture: {value}");
            };
            rule
        })
    });
    let _ = hls::apply(
        Bytes::copy_from_slice(input),
        &BASE,
        &[&RULES[0], &RULES[1], &RULES[2]],
        &within_budget,
    );
}

fn compile_action(kind: ActionKind, value: &str) -> ResponseAction {
    kind.parse(value)
        .and_then(|spec| spec.compile_checked())
        .expect("valid action fixture")
}

pub fn set_cookie(input: &[u8]) {
    static RULES: LazyLock<[cookie::Compiled; 2]> = LazyLock::new(|| {
        ["tracking", ";maxAge=60;sameSite=lax"].map(|value| {
            let ResponseAction::Cookie(rule) = compile_action(ActionKind::Cookie, value) else {
                panic!("invalid cookie rule fixture: {value}");
            };
            rule
        })
    });
    if let Ok(value) = HeaderValue::from_bytes(input) {
        let mut headers = HeaderMap::new();
        headers.insert(SET_COOKIE, value);
        cookie::response(&mut headers, RULES.iter());
    }
}

pub fn rule_list(input: &[u8]) {
    if input.len() <= MAX_LIST
        && let Ok(text) = str::from_utf8(input)
    {
        // User-rule permissions exercise response actions and scripts too.
        let entry = Ok((Path::new("rules/fuzz.txt"), text.to_owned()));
        let _ = Filters::build(std::iter::once(entry), None, false, MAX_LISTS)
            .expect("in-memory list read succeeds");
    }
}

pub fn clienthello(input: &[u8]) {
    black_box(clienthello::parse(input));
}

pub fn dns(input: &[u8]) {
    if let Some(query) = dns::Question::parse(input, false) {
        black_box(query.host());
        for truncated in [false, true] {
            let reply = query.local_answer(60, truncated);
            assert!(dns::is_answer_to(input, &reply));
        }
    } else if let Some(reply) = dns::Question::parse(input, true) {
        black_box(reply.host());
    } else if let Some(reply) = dns::refusal(input) {
        assert!(dns::is_answer_to(input, &reply));
    }
}

pub fn quic(input: &[u8]) {
    black_box(quic::is_initial(input));
}

pub fn cosmetic(input: &[u8]) {
    let mut parts = input.splitn(4, |&byte| byte == b'\n');
    let (policy, report_only, selector, html) =
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(policy), Some(report_only), Some(selector), Some(html)) => {
                (policy, report_only, selector, html)
            }
            _ => (input, input, input, input),
        };
    // The head and a tag's attributes can arrive in different body frames.
    black_box(cosmetic::place(&html[..html.len().min(17)]));
    black_box(cosmetic::place(html));
    let mut names = cosmetic::Names::default();
    for chunk in html.chunks(17) {
        names.scan(chunk);
    }
    black_box(names);

    let css = str::from_utf8(selector).ok().and_then(|selector| {
        black_box(cosmetic::safe_selector(selector));
        cosmetic::style([selector.to_owned()])
    });
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (CONTENT_SECURITY_POLICY, policy),
        (CONTENT_SECURITY_POLICY_REPORT_ONLY, report_only),
    ] {
        if !value.is_empty()
            && let Ok(value) = HeaderValue::from_bytes(value)
        {
            headers.append(name, value);
        }
    }
    black_box(cosmetic::inline(&headers));
    cosmetic::permit(
        &mut headers,
        css.as_deref().unwrap_or(".ad{display:none!important}"),
    );
    black_box(headers);
}

#[cfg(test)]
#[test]
fn engine_fuzz_seeds_replay_without_panicking() {
    let targets = [
        ("json_prune", json_prune as fn(&[u8])),
        ("html_filter", html_filter),
        ("playlist", playlist),
        ("set_cookie", set_cookie),
        ("rule_list", rule_list),
        ("clienthello", clienthello),
        ("dns", dns),
        ("quic", quic),
        ("cosmetic", cosmetic),
    ];
    for (target, run) in targets {
        replay::seeds(target, run);
    }
}
