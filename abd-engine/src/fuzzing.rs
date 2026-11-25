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
use std::{path::Path, str};

use bytes::Bytes;
use hyper::Uri;
use hyper::header::{HeaderMap, HeaderValue, SET_COOKIE};

use crate::filter::Filters;
use crate::jsonpath;
use crate::list_files::{MAX_LIST, MAX_LISTS};
use crate::rule::action::{ActionKind, ResponseAction, cookie, hls};
use crate::rule::{self, Kind};
use crate::{htmlfilter, xpath};

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

pub fn xml_prune(input: &[u8]) {
    static PATHS: LazyLock<[xpath::XPath; 3]> = LazyLock::new(|| {
        [
            "//ad | //comment()",
            "//*/@ad",
            "//item[2]/following-sibling::*",
        ]
        .map(|path| xpath::XPath::parse(path).expect("valid XPath fixture"))
    });
    let body = Bytes::copy_from_slice(input);
    for path in PATHS.iter() {
        let _ = xpath::prune(path, body.clone(), &within_budget);
    }
    if let Some((expression, body)) = expression_and_body(input)
        && let Ok(path) = xpath::XPath::parse(expression)
    {
        let _ = xpath::prune(&path, Bytes::copy_from_slice(body), &within_budget);
    }
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
