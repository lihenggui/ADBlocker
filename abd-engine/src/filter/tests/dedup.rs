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
use crate::filter::{AppFilters, Page, parse_http_page};
use crate::list_url::file_name;

const CORPUS: &str = include_str!("../../../../testdata/rules/corpus.txt");
const TRUSTED: &str = "https://filters.adtidy.org/android/filters/11.txt";

fn trusted_list() -> String {
    format!("lists/{}", file_name(TRUSTED))
}

fn build(lists: &[(&str, &str)], debug: bool, guard_budget: usize) -> Filters {
    Filters::build(
        lists
            .iter()
            .map(|&(path, text)| Ok((Path::new(path), text.to_owned()))),
        None,
        debug,
        guard_budget,
    )
    .unwrap()
}

fn inputs(lists: &[(&str, &str)], debug: bool) -> Vec<(String, String)> {
    crate::public_suffix::install();
    let disabled = badfilters(lists.iter().map(|&(_, text)| text));
    let mut compilation = Compilation::new(debug, 0);
    lists
        .iter()
        .map(|&(path, text)| {
            let permissions = SourcePermissions::for_path(Path::new(path));
            let (_, matcher, _) = compilation.project_list(text, permissions, &disabled);
            let input = (
                matcher.text().to_owned(),
                compilation.hosts.current().to_owned(),
            );
            compilation.hosts.finish_list();
            compilation
                .matcher
                .finish_list(matcher, permissions.matcher());
            input
        })
        .collect()
}

#[test]
fn a_bare_host_anchor_in_two_lists_is_compiled_once() {
    let anchor = "||ads.example^\n";
    let later = "||ads.example^\n0.0.0.0 ads.example\n||ads.example^$_\n||other.example^\n";
    let trusted = trusted_list();
    let lists = [("a.txt", anchor), (trusted.as_str(), later)];
    let projected = inputs(&lists, false);
    assert_eq!(projected[0], (anchor.to_owned(), anchor.to_owned()));
    let other = "||other.example^\n";
    assert_eq!(projected[1], (other.to_owned(), other.to_owned()));

    let filters = build(&lists, false, MAX_LISTS);
    assert_eq!((filters.rules, filters.host_rules), (5, 5));
}

#[test]
fn a_path_rule_or_selector_in_two_lists_is_kept_twice() {
    let rules = "||ads.example/banner^\n||ads.example^$script\n||ads*.example^\n##.banner\n";
    let lists = [("a.txt", rules), ("b.txt", rules)];
    for projected in inputs(&lists, false) {
        assert_eq!(projected, (rules.to_owned(), String::new()));
    }
}

#[test]
fn punycode_host_anchors_stay_on_the_matcher_parse_path() {
    let punycode = "||xn--.example^\n||xn--fiqs8s.example^\n||sub.Xn--fiqs8s.example^\n";
    let rules = format!("{punycode}||中国.example^\n");
    let projected = inputs(&[("a.txt", &rules), ("b.txt", &rules)], false);
    assert_eq!(projected[0].0, rules);
    assert_eq!(projected[1].0, punycode);
}

#[test]
fn a_debug_build_keeps_every_copy() {
    let rules = "||ads.example^\n@@||sub.ads.example^\n||ads.example/banner^\n##.banner\n";
    let whole = "||ads.example^\n@@||sub.ads.example^\n";
    let lists = [("a.txt", rules), ("b.txt", rules)];
    for projected in inputs(&lists, true) {
        assert_eq!(projected, (rules.to_owned(), whole.to_owned()));
    }
}

#[test]
fn a_host_anchor_in_two_lists_preserves_filtering_and_rule_counts() {
    let rules = "||ads.example^\n##.banner\n";
    let lists = [("a.txt", rules), ("b.txt", rules)];
    let both = build(&lists, false, MAX_LISTS);
    assert_eq!((both.rules, both.host_rules), (4, 2));

    let classes = HashSet::from(["banner".to_owned()]);
    let hidden = |filters: &Filters| {
        let page = filters.hiding("https://site.example/");
        filters.hiding_by_name(&page, &classes, &HashSet::new())
    };
    assert_eq!(
        hidden(&both).as_deref(),
        Some(".banner{display:none!important}\n")
    );
    for list in lists {
        let alone = build(&[list], false, MAX_LISTS);
        assert_eq!(hidden(&both), hidden(&alone));
        for (host, blocked) in [("ads.example", true), ("other.example", false)] {
            assert_eq!(both.blocks_host(host), blocked);
            assert_eq!(both.blocks_host(host), alone.blocks_host(host));
            assert_eq!(both.blocks_domain(host), alone.blocks_domain(host));
            let url = format!("https://{host}/banner");
            assert_eq!(both.check(&url, "", "image", "GET").blocked, blocked);
            assert_eq!(
                both.check(&url, "", "image", "GET").blocked,
                alone.check(&url, "", "image", "GET").blocked
            );
        }
    }
}

#[test]
fn a_guard_uses_the_parsed_hostname_across_lists() {
    let guarded = build(
        &[
            ("a.txt", "||ADS.example^\n"),
            ("b.txt", "@@||ads.example/allowed\n"),
        ],
        false,
        MAX_LISTS,
    );
    assert!(guarded.blocks_domain("ads.example"));
    assert!(!guarded.blocks_host("ads.example"));
}

#[test]
fn a_scriptlet_in_a_trusted_and_an_untrusted_list_still_runs_from_the_trusted_one() {
    let rules = "site.example##+js(trusted-set, fixture, json:42)";
    let lists = [("untrusted.txt", rules), (&trusted_list(), rules)];
    let url = "https://site.example/";
    let untrusted = build(&lists[..1], false, MAX_LISTS);
    assert!(untrusted.hiding(url).script.is_none());

    let both = build(&lists, false, MAX_LISTS);
    let script = both.hiding(url).script.unwrap();
    assert_eq!(
        script
            .matches(r#"trustedSetConstant("fixture", "json:42")"#)
            .count(),
        1
    );
}

#[test]
fn a_badfilter_in_one_list_disables_a_rule_duplicated_in_another() {
    let lists = [
        ("a.txt", "||ads.example^\n||ads.example^$badfilter\n"),
        ("b.txt", "||ads.example^\n||other.example^\n"),
    ];
    let filters = build(&lists, false, MAX_LISTS);
    assert_eq!((filters.rules, filters.host_rules), (4, 1));
    assert!(filters.blocks_host("other.example"));
    assert!(!filters.blocks_host("ads.example"));
    assert!(!filters.blocks_domain("ads.example"));
    assert!(
        !filters
            .check("https://ads.example/", "", "image", "GET")
            .blocked
    );
}

#[test]
fn a_duplicated_exception_still_guards_a_whole_host_block() {
    let exception = "@@||sub.ads.example^\n";
    let lists = [
        ("a.txt", "||ads.example^\n@@||sub.ads.example^\n"),
        ("b.txt", exception),
    ];
    assert_eq!(
        inputs(&lists, false)[1],
        (exception.to_owned(), String::new())
    );
    let filters = build(&lists, false, MAX_LISTS);
    assert_eq!(filters.host_rules, 3);
    assert!(filters.blocks_host("ads.example"));
    assert!(!filters.blocks_host("sub.ads.example"));
    assert!(filters.blocks_domain("ads.example"));
    assert!(!filters.blocks_domain("sub.ads.example"));

    // Both occurrences count towards the guard budget, so early checks fall back.
    let limited = build(&lists, false, exception.len());
    assert!(limited.guards_incomplete);
    assert!(!limited.blocks_host("ads.example"));
    assert!(limited.blocks_domain("ads.example"));
}

fn compare_requests(normal: &Filters, debug: &Filters) {
    for (url, kind) in [
        ("https://ads.example/banner.js", "script"),
        ("https://ads.example/allowed", "image"),
        ("https://cdn.example/ads.css", "stylesheet"),
        ("https://example.org/static/fonts/ad.woff", "font"),
        ("http://tracker.example/tracker42.js", "script"),
        (
            "https://example.org/ads/?utm_source=one&keep=two",
            "document",
        ),
        ("https://site.example/tracker42.js", "script"),
        ("https://example.org/rec", "xmlhttprequest"),
        (
            "https://example.org/wp-content/plugins/koko-analytics/",
            "script",
        ),
    ] {
        for page in ["", "https://example.org/", "https://private.example.org/"] {
            for method in ["GET", "POST"] {
                let actual = normal.check(url, page, kind, method);
                let expected = debug.check(url, page, kind, method);
                assert_eq!(
                    (actual.blocked, actual.redirect, actual.rewritten_url),
                    (expected.blocked, expected.redirect, expected.rewritten_url),
                    "{method} {url} from {page} ({kind})"
                );
            }
        }
    }
}

#[test]
fn debug_and_regular_builds_filter_the_corpus_in_both_trust_orders() {
    let (network, cosmetic) = CORPUS
        .split_once("\n! Cosmetic selectors and exceptions\n")
        .unwrap();
    let trusted = trusted_list();
    let mut lists = [
        ("a.txt", CORPUS),
        (trusted.as_str(), cosmetic),
        ("b.txt", network),
    ];
    for _ in 0..2 {
        let normal = build(&lists, false, MAX_LISTS);
        let debug = build(&lists, true, MAX_LISTS);
        compare_requests(&normal, &debug);
        compare_hosts_and_hiding(&normal, &debug);
        lists.swap(0, 1);
    }
}

fn compare_hosts_and_hiding(normal: &Filters, debug: &Filters) {
    let classes = HashSet::from(["ad".to_owned(), "advert".to_owned()]);
    let ids = HashSet::from(["advert".to_owned()]);
    for host in [
        "ads.example",
        "sub.ads.example",
        "cdn.example",
        "example.org",
        "tracker.example",
        "site.example",
        "unrelated.test",
    ] {
        let url = format!("https://{host}/");
        let actual = normal.hiding(&url);
        let expected = debug.hiding(&url);
        assert_eq!(
            normal.hiding_by_name(&actual, &classes, &ids),
            debug.hiding_by_name(&expected, &classes, &ids),
            "{url}"
        );
        assert_eq!(
            (actual.css, actual.script, actual.by_name),
            (expected.css, expected.script, expected.by_name),
            "{url}"
        );
        assert_eq!(normal.blocks_host(host), debug.blocks_host(host), "{host}");
        assert_eq!(
            normal.blocks_domain(host),
            debug.blocks_domain(host),
            "{host}"
        );
    }
}

#[test]
fn a_debug_build_reports_the_original_text_of_rewritten_rules() {
    let first = "||ads.example^$_";
    let lists = [
        ("a.txt", first),
        ("b.txt", "||ads.example^$____\n||second.example^$_\n"),
        ("c.txt", first),
    ];
    let filters = build(&lists, true, MAX_LISTS);
    for (host, original) in [
        ("ads.example", first),
        ("second.example", "||second.example^$_"),
    ] {
        let check = filters.check(&format!("https://{host}/"), "", "image", "GET");
        assert!(check.blocked);
        assert_eq!(check.rule.as_deref(), Some(original));
    }

    let lists = [
        ("a.txt", "||ads.example^\n/banner[0-9]+/\n/tracker[0-9]+/\n"),
        ("b.txt", first),
    ];
    let filters = build(&lists, true, MAX_LISTS);
    let check = filters.check("https://site.example/banner1", "", "image", "GET");
    assert_eq!(check.rule.as_deref(), Some("/banner[0-9]+/"));
}

#[test]
fn duplicated_regex_and_wildcard_patterns_still_match() {
    let rules = "/ad[0-9]{3}/\n*a^*\n";
    let filters = build(&[("a.txt", rules), ("b.txt", rules)], false, MAX_LISTS);
    for url in ["https://site.example/ad123", "https://site.example/a/"] {
        assert!(filters.check(url, "", "script", "GET").blocked, "{url}");
    }
}

#[test]
fn duplicated_path_exceptions_still_allow_matching_requests() {
    let lists = [
        ("a.txt", "||site.example^\n@@||site.example/*/marker^\n"),
        ("b.txt", "@@||site.example/*/marker^\n"),
    ];
    let filters = build(&lists, false, MAX_LISTS);
    assert!(
        !filters
            .check("https://site.example/pad/marker*", "", "script", "GET")
            .blocked
    );
}

#[test]
fn debug_builds_preserve_the_selected_original_line() {
    let lists = [
        ("a.txt", "||alpha.net^\n0.0.0.0 alpha.example\n"),
        ("b.txt", "||alpha.example^\n"),
    ];
    let filters = build(&lists, true, MAX_LISTS);
    let result = filters.check("https://alpha.example/", "", "image", "GET");
    assert!(result.blocked);
    assert_eq!(result.rule.as_deref(), Some("||alpha.example^"));
}

#[test]
fn duplicated_cosmetic_selectors_preserve_hiding_by_name_output() {
    let rule = "##.banner > span\n";
    let lists = [("a.txt", rule), ("b.txt", rule)];
    let classes = HashSet::from(["banner".to_owned()]);
    let filters = build(&lists, false, MAX_LISTS);
    let page = filters.hiding("https://site.example/");
    assert_eq!(
        filters.hiding_by_name(&page, &classes, &HashSet::new()),
        Some(".banner > span{display:none!important}\n".repeat(2))
    );
}

#[test]
fn app_and_page_exceptions_guard_whole_host_blocks_across_lists() {
    let blocks = "||app.example^\n||page.example^\n";
    let lists = [
        ("a.txt", blocks),
        ("b.txt", blocks),
        (
            "exceptions.txt",
            "@@||app.example/allowed$app=com.reader\n@@||page.example/allowed$domain=news.example\n",
        ),
    ];
    for debug in [false, true] {
        let filters = build(&lists, debug, MAX_LISTS);
        let reader = filters.for_app_with_pages("com.reader", true);
        let native_reader = filters.for_app_with_pages("com.reader", false);
        let other = filters.for_app_with_pages("com.other", true);

        assert!(!reader.blocks_host("app.example"));
        assert!(reader.blocks_domain("app.example"));
        assert!(other.blocks_host("app.example"));
        let app_url = "https://app.example/allowed";
        let blocked = |app: &AppFilters<'_>, url, page| {
            app.check_with_page(url, page, "script", "GET").blocked
        };
        assert!(!blocked(&reader, app_url, Page::NONE));
        assert!(blocked(&other, app_url, Page::NONE));

        assert!(!reader.blocks_host("page.example"));
        assert!(reader.blocks_domain("page.example"));
        assert!(native_reader.blocks_host("page.example"));
        assert!(reader.blocks_unread("page.example"));
        let page_url = "https://page.example/allowed";
        let news = parse_http_page("https://news.example/").unwrap();
        assert!(!blocked(&reader, page_url, Page::Named(&news)));
        assert!(blocked(&reader, page_url, Page::NONE));
    }
}

#[test]
fn a_path_with_an_asterisk_matches_the_build_with_the_later_anchor_removed() {
    let repeated = [
        ("a.txt", "||alpha.net^\n||site.org^\n||example.org^\n"),
        ("b.txt", "||alpha.net^\n||site.example/*/alpha^\n"),
    ];
    let once = [repeated[0], ("b.txt", "||site.example/*/alpha^\n")];
    let blocked = |lists: &[(&str, &str)]| {
        build(lists, false, MAX_LISTS)
            .check("https://site.example/pad/alpha*", "", "script", "GET")
            .blocked
    };
    assert_eq!(blocked(&repeated), blocked(&once));
}

#[test]
fn a_bare_anchor_repeated_in_one_list_matches_the_list_without_the_repeat() {
    let repeated =
        "||alpha.net^\n||site.org^\n||example.org^\n||alpha.net^\n||site.example/*/alpha^\n";
    let once = "||alpha.net^\n||site.org^\n||example.org^\n||site.example/*/alpha^\n";
    let deduplicated = build(&[("a.txt", repeated)], false, MAX_LISTS);
    let without_repeat = build(&[("a.txt", once)], false, MAX_LISTS);
    let blocked = |filters: &Filters, url| filters.check(url, "", "script", "GET").blocked;

    let url = "https://site.example/pad/alpha*";
    assert_eq!(blocked(&deduplicated, url), blocked(&without_repeat, url));
    for url in ["https://alpha.net/", "https://site.example/x/alpha"] {
        assert!(blocked(&deduplicated, url), "{url}");
        assert!(blocked(&without_repeat, url), "{url}");
    }
}

#[test]
fn a_long_host_matches_the_build_with_the_later_anchor_removed() {
    let repeated = [
        ("a.txt", "||alpha.net^\n||alpha.example^\n"),
        ("b.txt", "||alpha.example^\n"),
    ];
    let once = [repeated[0], ("b.txt", "")];
    // The request's token buffer has room for alpha but runs out before example.
    let host = format!("{}alpha.example", "aa.".repeat(253));
    let url = format!("https://{host}/");
    let decisions = |lists: &[(&str, &str)]| {
        let filters = build(lists, false, MAX_LISTS);
        (
            filters.blocks_domain(&host),
            filters.check(&url, "", "script", "GET").blocked,
        )
    };
    assert_eq!(decisions(&repeated), decisions(&once));
}
