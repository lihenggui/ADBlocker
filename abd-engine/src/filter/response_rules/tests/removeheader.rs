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

use super::*;
use crate::rule::action::Place;

fn removals(
    filters: &Filters,
    url: &str,
    page: Page<'_>,
    kind: &str,
    package: &str,
) -> Vec<String> {
    let mut headers: Vec<_> = filters
        .for_app(package)
        .response_actions(url, page, kind, "GET")
        .iter()
        .filter_map(|action| match action.place() {
            Place::Request(name) => Some(format!("request:{name}")),
            Place::ResponseHead(name) => Some(name.to_string()),
            Place::RequestUrl | Place::ResponseBody | Place::RequestAndResponseHead => None,
        })
        .collect();
    headers.sort();
    headers
}

#[test]
fn the_reference_examples_remove_both_headers_except_on_the_exempt_path() {
    let text = "||example.org^$removeheader=Refresh
                ||example.org^$removeheader=Location
                @@||example.org/path/$removeheader
                ||example.org^$RemoveHeader=REQUEST:X-Client-Data,xmlhttprequest
                ||example.org^$removeheader=X-Trace$Id";
    assert!(
        crate::rulecheck::check(text, crate::rulecheck::SourcePermissions::USER)
            .issues
            .is_empty()
    );
    let f = filters(text);
    let headers = |url, kind| removals(&f, url, Page::NONE, kind, "com.reader");
    assert_eq!(
        headers("https://sub.example.org/asset", "xmlhttprequest"),
        ["location", "refresh", "request:x-client-data", "x-trace$id"]
    );
    assert_eq!(
        headers("https://example.org/path/asset", "xmlhttprequest"),
        Vec::<String>::new()
    );
    assert_eq!(
        headers("https://example.org/other", "script"),
        ["location", "refresh", "x-trace$id"]
    );
}

#[test]
fn removeheader_accepts_only_the_reference_modifiers() {
    for valid in [
        "domain=reader.org",
        "from=reader.org",
        "third-party",
        "3p",
        "~third-party",
        "strict-third-party",
        "strict3p",
        "strict-first-party",
        "strict1p",
        "app=com.reader",
        "important",
        "match-case",
        "script",
        "xmlhttprequest",
        "xhr",
        "document",
        "doc",
        "subdocument",
        "image",
        "stylesheet",
        "css",
        "font",
        "media",
        "object",
        "other",
        "websocket",
        "ping",
        "~script",
    ] {
        let rule = format!("||example.org/Path$removeheader=x-test,{valid}");
        assert!(
            crate::rulecheck::check(&rule, crate::rulecheck::SourcePermissions::USER)
                .issues
                .is_empty(),
            "{rule}"
        );
    }
    for invalid in [
        "_",
        "to=example.org",
        "denyallow=example.org",
        "all",
        "method=GET",
        "content",
        "urlblock",
        "elemhide",
        "popup",
        "webrtc",
        "object-subrequest",
        "header=x-foo",
        "badfilter",
        "first-party",
        "~strict-third-party",
        "app",
        "important=yes",
        "replace=/x/y/",
        "removeheader=x-other",
        "~removeheader=x-other",
    ] {
        let rule = format!("||example.org^$removeheader=x-test,{invalid}");
        assert_eq!(
            crate::rulecheck::check(&rule, crate::rulecheck::SourcePermissions::USER)
                .issues
                .len(),
            1,
            "{rule}"
        );
    }
    let malformed = "||example.org^$removeheader
                     ||example.org^$removeheader=
                     @@||example.org^$removeheader=
                     ||example.org^$removeheader=sec-fetch-dest";
    assert_eq!(
        crate::rulecheck::check(malformed, crate::rulecheck::SourcePermissions::USER)
            .issues
            .len(),
        4
    );
}

#[test]
fn an_invalid_noop_rule_leaves_the_matching_valid_rule_enabled() {
    let f = filters(
        "||example.org^$_,removeheader=location
         ||example.org^$removeheader=location",
    );
    assert_eq!(
        removals(
            &f,
            "https://example.org/",
            Page::NONE,
            "other",
            "com.reader"
        ),
        ["location"]
    );
}

#[test]
fn header_tokens_can_contain_network_and_cosmetic_delimiters() {
    let f = filters("example.org$removeheader=request:X-Trace#$#Id");
    assert_eq!(
        removals(
            &f,
            "https://example.org/",
            Page::NONE,
            "other",
            "com.reader"
        ),
        ["request:x-trace#$#id"]
    );
}

#[test]
fn case_sensitive_patterns_and_supported_qualifiers_limit_removals() {
    let f = filters(
        "||example.org/Query*^$removeheader=x-case,match-case
         /Query[0-9]+/$removeheader=x-regex,match-case
         ||example.org^$removeheader=x-normal,third-party
         ||example.org^$removeheader=x-third,strict3p
         ||example.org^$removeheader=x-first,strict1p
         ||example.org^$removeheader=x-app,app=com.reader,domain=sub.example.org,~image",
    );
    let sub = parse_http_page("https://sub.example.org/page").unwrap();
    let same = parse_http_page("https://example.org/page").unwrap();
    let outside = parse_http_page("https://outside.org/page").unwrap();
    let url = "https://example.org/Query42?term=1";
    let headers = |url, page, kind, package| removals(&f, url, page, kind, package);
    assert_eq!(
        headers(url, Page::Named(&sub), "script", "com.reader"),
        ["x-app", "x-case", "x-regex", "x-third"]
    );
    assert_eq!(
        headers(url, Page::Named(&sub), "image", "com.reader"),
        ["x-case", "x-regex", "x-third"]
    );
    assert_eq!(
        headers(url, Page::Named(&sub), "script", "com.other"),
        ["x-case", "x-regex", "x-third"]
    );
    assert_eq!(
        headers(
            "https://example.org/query42",
            Page::Named(&same),
            "other",
            "com.reader"
        ),
        ["x-first"]
    );
    assert_eq!(
        headers(
            "https://another.org/Query42",
            Page::Named(&same),
            "other",
            "com.reader"
        ),
        ["x-regex"]
    );
    assert_eq!(
        headers(url, Page::Named(&outside), "other", "com.reader"),
        ["x-case", "x-normal", "x-regex", "x-third"]
    );
    assert_eq!(
        headers(
            "https://example.org/other",
            Page::NONE,
            "other",
            "com.reader"
        ),
        ["x-first"]
    );
}

#[test]
fn exceptions_are_scoped_by_action_kind_direction_and_page() {
    let f = filters(
        "||example.org^$removeheader=location
         ||example.org^$removeheader=request:location
         ||example.org^$replace=/X/Y/
         @@||example.org/plain^
         @@||example.org/all^$removeheader
         @@||example.org/response^$removeheader=Location
         @@||example.org/request^$removeheader=Request:LOCATION
         @@||example.org/replace^$replace
         @@||pages.org/document^$document
         @@||pages.org/urlblock^$urlblock
         @@||pages.org/content^$content",
    );
    let headers = |path| {
        removals(
            &f,
            &format!("https://example.org/{path}"),
            Page::NONE,
            "other",
            "com.reader",
        )
    };
    assert_eq!(headers("plain"), ["location", "request:location"]);
    assert!(headers("all").is_empty());
    assert_eq!(headers("response"), ["request:location"]);
    assert_eq!(headers("request"), ["location"]);
    assert_eq!(headers("replace"), ["location", "request:location"]);
    let has_replace = |path| {
        f.for_app("com.reader")
            .response_actions(
                &format!("https://example.org/{path}"),
                Page::NONE,
                "other",
                "GET",
            )
            .iter()
            .any(|a| a.kind() == ActionKind::Replace)
    };
    assert!(has_replace("all"));
    assert!(!has_replace("replace"));
    for path in ["document", "urlblock", "content"] {
        let page = parse_http_page(&format!("https://pages.org/{path}")).unwrap();
        let headers = removals(
            &f,
            "https://example.org/asset",
            Page::Named(&page),
            "other",
            "com.reader",
        );
        assert_eq!(!headers.is_empty(), path == "content", "{path}");
    }
}

#[test]
fn the_cache_preserves_removals_and_exceptions_from_trusted_sources_only() {
    let temp = tempfile::tempdir().unwrap();
    let lists = temp.path().join("lists");
    let rules = temp.path().join("rules");
    for dir in [&lists, &rules] {
        std::fs::create_dir(dir).unwrap();
    }
    let base = "https://filters.adtidy.org/android/filters/2.txt";
    for (file, host) in [
        (lists.join(file_name(base)), "catalog.org"),
        (
            lists.join(
                untrusted_file_name("https://filters.adtidy.org/android/filters/11.txt").unwrap(),
            ),
            "alternate.org",
        ),
        (lists.join("custom.txt"), "custom.org"),
        (rules.join("user.txt"), "user.org"),
    ] {
        std::fs::write(
            file,
            format!(
                "||{host}^$removeheader=Location
                     ||{host}^$removeheader=request:X-Client-Data,strict1p
                     @@||{host}/off^$removeheader=location
                     @@||page.org^$urlblock"
            ),
        )
        .unwrap();
    }
    let cache = temp.path().join("cache");
    let built = Filters::load_cached(&[&lists, &rules], &cache).unwrap();
    let cached = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
    for f in [built, cached] {
        let page = parse_http_page("https://page.org/").unwrap();
        let headers = |host, path, page| {
            removals(
                &f,
                &format!("https://{host}/{path}"),
                page,
                "other",
                "com.reader",
            )
        };
        for host in ["catalog.org", "user.org"] {
            assert_eq!(
                headers(host, "", Page::NONE),
                ["location", "request:x-client-data"]
            );
            assert_eq!(headers(host, "off", Page::NONE), ["request:x-client-data"]);
            assert!(headers(host, "", Page::Named(&page)).is_empty());
        }
        for host in ["alternate.org", "custom.org"] {
            assert!(headers(host, "", Page::NONE).is_empty());
        }
    }
}
