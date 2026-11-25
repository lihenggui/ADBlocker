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

use super::quoting::unescape;
use super::*;
use crate::rule::action::ActionKind;

#[test]
fn comments_and_headers_are_read_as_comments() {
    for line in ["", "  ! Title: Rules", "[Adblock Plus 2.0]", "# hosts file"] {
        assert!(matches!(read(line).kind, Kind::Comment));
    }
}

#[test]
fn ordinary_network_rules_go_to_the_matcher() {
    let rule = read("  ||example.org^$script,domain=example.net  ");
    assert!(matches!(rule.kind, Kind::Matcher));
    assert_eq!(rule.matcher_input().as_deref(), Some(rule.line));
}

#[test]
fn only_complete_underscore_options_are_ignored() {
    for (original, expected) in [
        ("||ads.com^$_", "||ads.com^"),
        ("||ads.com^$_,__,_____", "||ads.com^"),
        (
            "/ad__asset$__,script,__,domain=site.org,_",
            "/ad__asset$script,domain=site.org",
        ),
        ("0.0.0.0 ads.com$__", "||ads.com^"),
        ("||ads.com^$_=value", "||ads.com^$_=value"),
        ("||ads.com^$~_", "||ads.com^$~_"),
        ("||ads.com^$tag=___", "||ads.com^$tag=___"),
        ("site.test##[href$=___]", "site.test##[href$=___]"),
    ] {
        let rule = read(original);
        assert_eq!(rule.line, original);
        assert_eq!(rule.matcher_input().as_deref(), Some(expected));
    }
    for line in ["$_", "$_,__"] {
        let check = crate::rulecheck::check(line);
        assert_eq!(check.issues[0].text, line);
    }
}

#[test]
fn removing_noops_keeps_unsupported_url_and_source_regexes_rejected() {
    for (line, reason) in [
        (
            r"/^(?!.*(x\.com)).*$/$__,script,domain=a.com",
            Reason::NetworkRegex,
        ),
        (r"*$___,script,domain=/^a\.com$/", Reason::RegexSourceDomain),
    ] {
        let rule = read(line);
        assert!(matches!(&rule.kind, Kind::NotApplied(rejected) if *rejected == reason));
        assert!(rule.matcher_input().is_none());
        assert_eq!(crate::rulecheck::check(line).issues[0].text, line);
    }
}

#[test]
fn every_network_rule_kind_sees_options_after_noops_are_removed() {
    for (suffix, retained) in [("_", 1), ("_,__", 1), ("__,script,___", 2)] {
        let input = format!("||ads.com^$app=com.reader,{suffix}");
        let rule = read(&input);
        let Kind::App(app) = &rule.kind else {
            panic!("{input}")
        };
        let condition = app.condition().unwrap();
        assert_eq!(condition.apps, "com.reader");
        assert_eq!(condition.options.len(), retained - 1);

        let input = format!("@@||ads.com^$elemhide,{suffix}");
        let rule = read(&input);
        let page = rule.page_exception().unwrap();
        assert!(
            !page.project(rule.literal_options()).contains('_'),
            "{input}"
        );

        let input = format!("/ad.js$denyallow=x.com,{suffix},domain=ads.com");
        let rule = read(&input);
        let normalized = rule.matcher_input().unwrap();
        assert!(normalized.contains("to=~x.com"), "{input}");
        assert!(!normalized.contains('_'), "{input}");
    }
}

#[test]
fn denyallow_replaces_only_the_destination_option() {
    for (original, rewritten) in [
        (
            "*$script,domain=a.com|b.com,denyallow=x.com|y.com",
            "*$script,domain=a.com|b.com,to=~x.com|~y.com",
        ),
        (
            "$image,third-party,denyallow=googleapis.com,domain=vipbox1.org",
            "$image,third-party,to=~googleapis.com,domain=vipbox1.org",
        ),
        (
            r"/some-path$/$denyallow=X.COM|bücher.de,script,important",
            r"/some-path$/$to=~x.com|~xn--bcher-kva.de,script,important",
        ),
        (
            "/asset.js$script,denyallow=x.com,mp4,domain=a.com",
            "/asset.js$script,to=~x.com,redirect=noopmp4-1s,domain=a.com",
        ),
        (
            "|https://*:443/asset.js$script,denyallow=x.com,domain=a.com",
            "|https://*:443/asset.js$script,to=~x.com,domain=a.com",
        ),
    ] {
        let rule = read(original);
        assert_eq!(rule.line, original);
        assert_eq!(rule.matcher_input().as_deref(), Some(rewritten));
        assert!(rule.network().unwrap().opt_not_to_domains.is_some());
    }
}

#[test]
fn invalid_denyallow_rules_report_the_restriction_and_leave_no_matcher_rule() {
    use denyallow::Rejection::*;
    for (line, expected) in [
        ("||example.org^$denyallow=x.com", PatternNamesHost),
        ("@@||example.org^$denyallow=x.com", PatternNamesHost),
        (
            "|https://example.org/path$denyallow=x.com",
            PatternNamesHost,
        ),
        ("|https://[::1]/path$denyallow=x.com", PatternNamesHost),
        ("example.org/path$denyallow=x.com", PatternNamesHost),
        ("*$script,~denyallow=x.com", NegatedModifier),
        ("*$script,denyallow=x.com|~y.com", NegatedDomain),
        ("*$script,denyallow=x.*", WildcardTld),
        (r"*$script,denyallow=x.com|/\.(com\|org)$/", RegexDomain),
        ("*$script,denyallow", EmptyDomain),
        ("*$script,denyallow=", EmptyDomain),
        ("*$script,denyallow=x.com|", EmptyDomain),
        ("*$script,denyallow=https://x.com", InvalidDomain),
        ("*$script,to=~y.com,denyallow=x.com", WithTo),
        ("*$app=com.reader,denyallow=x.com,to=y.com", WithTo),
        ("*$denyallow=x.com,denyallow=y.com", Duplicate),
        ("*$script,replace=/x/y/,denyallow=x.com", WithResponseAction),
        (r"*$denyallow=x.com,jsonprune=\$.x", WithResponseAction),
        (
            "*$app=com.reader,denyallow=x.com,replace=/x/y/",
            WithResponseAction,
        ),
    ] {
        let rule = read(line);
        assert!(
            matches!(&rule.kind, Kind::NotApplied(Reason::DenyAllow(reason)) if *reason == expected),
            "{line}"
        );
        assert!(rule.matcher_input().is_none(), "{line}");
        let checked = crate::rulecheck::check(line);
        assert_eq!(checked.issues[0].text, line);
        assert_eq!(
            checked.issues[0].kind,
            crate::protocol_types::IssueKind::Invalid
        );
    }
}

#[test]
fn legacy_redirects_preserve_the_pattern_and_other_options() {
    for (original, projected) in [
        (
            r"/preroll[.]mp4$/$third-party,mp4,media,domain=watch.example,important",
            r"/preroll[.]mp4$/$third-party,redirect=noopmp4-1s,media,domain=watch.example,important",
        ),
        (
            "||example.org/path$empty,xmlhttprequest",
            "||example.org/path$redirect=nooptext,xmlhttprequest",
        ),
    ] {
        let rule = read(original);
        assert_eq!(rule.matcher_input().as_deref(), Some(projected));
        assert!(rule.network().unwrap().is_redirect());
    }
    for line in ["||example.org/empty", "||example.org^$unknown=mp4"] {
        assert_eq!(read(line).matcher_input().as_deref(), Some(line));
    }
}

#[test]
fn the_reader_accepts_both_valueless_redirect_exceptions() {
    for option in ["redirect", "redirect-rule"] {
        let line = format!("@@||example.org/path$script,{option},domain=page.test");
        let rule = read(&line);
        assert!(matches!(rule.matcher_check(), MatcherCheck::Network));
        assert!(rule.redirect_exception);
        assert!(rule.matcher_input().is_none());
        assert_eq!(
            rule.normalized(),
            format!("@@||example.org/path$script,{REDIRECT_EXCEPTION},domain=page.test")
        );

        let blocking = read(line.trim_start_matches("@@"));
        assert!(matches!(blocking.matcher_check(), MatcherCheck::Invalid));
    }
    let unsupported = read("@@||example.org/path$script,rewrite");
    assert!(matches!(unsupported.matcher_check(), MatcherCheck::Invalid));
}

#[test]
fn an_app_rule_retains_its_app_and_other_options() {
    let rule = read(r"||example.org^$app=/a\,b\/c\$\|d/,script");
    let Kind::App(app) = rule.kind else {
        panic!("expected an app rule")
    };
    let condition = app.condition().unwrap();
    assert_eq!(condition.pattern, "||example.org^");
    assert_eq!(condition.apps, r"/a\,b\/c\$\|d/");
    assert_eq!(condition.options, ["script"]);
}

#[test]
fn a_response_modifier_wins_over_app_and_page_modifiers() {
    let rule = read("@@||example.org^$app=com.reader,replace");
    assert!(rule.page_exception().is_none());
    let Kind::Response(response) = rule.kind else {
        panic!("expected a response rule")
    };
    let action = response.action().unwrap();
    assert!(action.exception);
    assert!(action.spec.is_none());
    assert!(action.kind == ActionKind::Replace);
    assert_eq!(action.apps, Some("com.reader"));

    let rule = read("@@||example.org^$app=com.reader,replace,elemhide");
    assert!(matches!(rule.kind, Kind::Response(_)));
    assert!(rule.page_exception().is_none());
}

#[test]
fn a_page_exception_keeps_its_targets_and_matcher_conditions() {
    let rule = read("@@||example.org^$shide,ghide,urlblock,domain=example.net");
    assert!(matches!(rule.kind, Kind::PageException(_)));
    let page = rule.page_exception().unwrap();
    assert!(matches!(page.elements, Some(Elements::All)));
    assert!(page.urlblock);
    assert!(!page.document);
    assert_eq!(
        page.project(rule.literal_options()),
        "@@||example.org^$document,domain=example.net"
    );
}

#[test]
fn app_scope_wins_over_the_page_exception_but_leaves_its_legacy_projection() {
    let rule = read("@@||example.org^$app=com.reader,elemhide");
    assert!(matches!(rule.kind, Kind::App(_)));
    let page = rule.page_exception().unwrap();
    let projected = page.project(rule.literal_options());
    assert_eq!(projected, "@@||example.org^$document,app=com.reader");
    assert!(read(&projected).network().is_none());
}

#[test]
fn page_projections_keep_literal_boundaries_where_app_quoting_differs() {
    let rule = read(r"@@||example.org^$app=/reader\$/,content");
    assert!(matches!(rule.kind, Kind::App(_)));
    let page = rule.page_exception().unwrap();
    assert_eq!(
        page.project(rule.literal_options()),
        r"@@||example.org^$app=/reader\$document,/"
    );
}

#[test]
fn cosmetic_rules_keep_their_own_text_even_if_it_spells_app() {
    let rule = read(r#"example.org##a[href="$app=com.reader"]"#);
    assert!(matches!(rule.kind, Kind::Cosmetic));
    assert_eq!(rule.matcher_input().as_deref(), Some(rule.line));
}

#[test]
fn a_known_unapplied_modifier_names_the_option_that_cannot_be_applied() {
    let rule = read("||example.org^$script,removeheader=X-Test");
    assert!(matches!(
        rule.kind,
        Kind::NotApplied(Reason::Modifier("removeheader"))
    ));
    assert!(matches!(read("example.org$$script").kind, Kind::Html(_)));
}

#[test]
fn each_escape_is_decoded_only_when_the_modifier_asks_for_it() {
    for (escape, decoded, ch) in [
        (r"\,", ",", ','),
        (r"\$", "$", '$'),
        (r"\/", "/", '/'),
        (r"\|", "|", '|'),
    ] {
        assert_eq!(unescape(escape, &[ch]), decoded);
        assert_eq!(unescape(escape, &[]), escape);
    }
    assert_eq!(unescape(r"a\,\d\$\/\|", &[',', '$', '/', '|']), r"a,\d$/|");
}
