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
use crate::rule::action::cookie::Rejection;
use crate::rule::{Kind, Reason};

#[test]
fn cookie_forms_and_qualifiers_match_the_reference() {
    for rule in [
        "||example.org^$cookie",
        "$cookie=",
        "$cookie=__cfduid",
        "$cookie=/__utm[a-z]/",
        "||facebook.com^$third-party,cookie=c_user",
        "||example.org^$cookie=NAME;maxAge=3600;sameSite=lax",
        "$cookie=NAME;sameSite=Strict",
        "$cookie=;maxAge=0",
        "$cookie=/NAME/;maxAge=-1",
        "$cookie=NAME; maxAge = 1; sameSite = none",
        r"$cookie=/^adb\$|adb\.key/,domain=iprima.cz",
        r"$cookie=/a\,b/",
        "$cookie=NAME,domain=example.org|~other.org",
        "$cookie=NAME,~domain=example.org",
        "$cookie=NAME,~third-party",
        "$cookie=NAME,important",
        "@@||example.org^$cookie",
        "@@||example.org^$cookie=concept",
        "@@||example.org^$cookie=/^_ga_/",
    ] {
        assert!(
            crate::rulecheck::check(rule, crate::rulecheck::SourcePermissions::USER)
                .issues
                .is_empty(),
            "{rule}"
        );
    }
    for qualifier in [
        "strict-first-party",
        "strict-third-party",
        "strict1p",
        "strict3p",
        "method=GET",
        "app=com.reader",
        "script",
        "all",
        "first-party",
        "match-case",
        "badfilter",
        "_",
        "replace=/X/Y/",
    ] {
        let rule = format!("||example.org^$cookie=NAME,{qualifier}");
        assert_eq!(
            crate::rulecheck::check(&rule, crate::rulecheck::SourcePermissions::USER)
                .issues
                .len(),
            1,
            "{rule}"
        );
    }
}

#[test]
fn invalid_cookie_values_report_why_they_were_rejected() {
    for (value, expected) in [
        ("bad name", Rejection::InvalidName),
        ("NAME=value", Rejection::InvalidName),
        ("/", Rejection::InvalidExpression),
        ("//", Rejection::InvalidExpression),
        ("/[x/", Rejection::InvalidExpression),
        ("/x/i", Rejection::InvalidExpression),
        ("/a++/", Rejection::InvalidExpression),
        ("/a,b/", Rejection::UnescapedSeparator),
        ("/^name$/", Rejection::UnescapedSeparator),
        ("NAME;", Rejection::InvalidAttribute),
        ("NAME;Secure=true", Rejection::InvalidAttribute),
        ("NAME;maxAge", Rejection::InvalidAttribute),
        ("NAME;maxAge=1;maxAge=2", Rejection::DuplicateAttribute),
        (
            "NAME;sameSite=none;sameSite=lax",
            Rejection::DuplicateAttribute,
        ),
        ("NAME;maxAge=+2", Rejection::InvalidMaxAge),
        ("NAME;maxAge=0.5", Rejection::InvalidMaxAge),
        ("NAME;maxAge=9223372036854775808", Rejection::InvalidMaxAge),
        ("NAME;sameSite=foo", Rejection::InvalidSameSite),
        ("NAME;sameSite=", Rejection::InvalidSameSite),
    ] {
        let text = format!("||example.org^$cookie={value}");
        assert!(
            matches!(rule::read(&text).kind, Kind::NotApplied(Reason::Cookie(reason)) if reason == expected),
            "{text}"
        );
        assert_eq!(
            crate::rulecheck::check(&text, crate::rulecheck::SourcePermissions::USER)
                .issues
                .len(),
            1,
            "{text}"
        );
    }
    let invalid = "@@||example.org^$cookie=NAME;maxAge=1";
    assert!(matches!(
        rule::read(invalid).kind,
        Kind::NotApplied(Reason::Cookie(Rejection::AttributesOnException))
    ));
}

#[test]
fn untrusted_cookie_rules_survive_the_cache_without_admitting_trusted_actions() {
    let dir = tempfile::tempdir().unwrap();
    let lists = dir.path().join("lists");
    let cache = dir.path().join("cache");
    std::fs::create_dir(&lists).unwrap();
    std::fs::write(
        lists.join("untrusted.txt"),
        r"
        ||example.org^$cookie=literal
        ||example.org^$cookie=/pattern/
        @@||example.org^$cookie=/^_ga_/
        ||example.org^$cookie=excluding,~domain=reader.test
        ||example.org^$replace=/X/Y/
        ||example.org^$jsonprune=key
        ||example.org^$removeheader=trace
        ||example.org^$urltransform=/X/Y/
        ||example.org^$hls=ad.ts
    ",
    )
    .unwrap();
    let built = Filters::load_cached(&[&lists], &cache).unwrap();
    let cached = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
    for filters in [built, cached] {
        let f = filters.for_app("com.reader");
        let source = parse_http_page("https://reader.test/").unwrap();
        for (page, expected) in [(Page::NONE, 4), (Page::Named(&source), 3)] {
            let actions = f.response_actions("https://example.org/asset", page, "other", "GET");
            assert_eq!(actions.len(), expected);
            assert!(
                actions
                    .iter()
                    .all(|action| action.kind() == ActionKind::Cookie)
            );
        }
    }
}

#[test]
fn cookie_expressions_use_the_response_rule_budget() {
    let mut patterns = Patterns::default();
    let f = ResponseRules::build_with_budget(
        [
            "||example.org^$cookie=/prefix/",
            "||example.org^$cookie=literal",
        ]
        .into_iter(),
        0,
        &mut patterns,
    )
    .unwrap();
    assert_eq!(f.compiled.len(), 1);
    assert!(f.compiled[0].kind() == ActionKind::Cookie);
}
