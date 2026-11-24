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

//! Rules are checked against the same matcher that uses them at runtime.

use super::*;

fn pruned(f: &Filters, url: &str, page: Page<'_>, input: &str) -> String {
    let mut bytes = input.as_bytes().to_vec();
    for action in f
        .for_app("com.reader")
        .response_actions(url, page, "xhr", "GET")
    {
        bytes = action.apply(&bytes, &|| true).unwrap();
    }
    String::from_utf8(bytes).unwrap()
}

#[test]
fn jsonprune_matches_the_five_reference_examples() {
    for (rule, input, output) in [
        (
            r#"||example.org^$jsonprune=\$..[one\, "two three"]"#,
            r#"{"one":1,"two":{"two three":23,"four five":45}}"#,
            r#"{"two":{"four five":45}}"#,
        ),
        (
            r"||example.org^$jsonprune=\$.a[?(has ad_origin)]",
            r#"{"a":[{"ad_origin":"https://example.org","b":42},{"b":1234}]}"#,
            r#"{"a":[{"b":1234}]}"#,
        ),
        (
            r"||example.org^$jsonprune=\$.*.*[?(key-eq 'Some key' 'Some value')]",
            r#"{"a":{"b":{"c":{"Some key":"Some value"},"d":{"Some key":"Other value"}}},"e":{"f":[{"Some key":"Some value"},{"Some key":"Other value"}]}}"#,
            r#"{"a":{"b":{"d":{"Some key":"Other value"}}},"e":{"f":[{"Some key":"Other value"}]}}"#,
        ),
        (
            r#"||example.org^$jsonprune=\$.elems[?(has "\$.a.b.c")]"#,
            r#"{"elems":[{"a":{"b":{"c":123}},"k":"v"},{"d":{"e":{"f":123}},"k1":"v1"}]}"#,
            r#"{"elems":[{"d":{"e":{"f":123}},"k1":"v1"}]}"#,
        ),
        (
            r#"||example.org^$jsonprune=\$.elems[?(key-eq "\$.a.b.c" "abc")]"#,
            r#"{"elems":[{"a":{"b":{"c":123}}},{"a":{"b":{"c":"abc"}}}]}"#,
            r#"{"elems":[{"a":{"b":{"c":123}}}]}"#,
        ),
    ] {
        assert_eq!(
            pruned(&filters(rule), "https://example.org/", Page::NONE, input),
            output,
            "{rule}"
        );
    }
}

#[test]
fn jsonprune_accepts_the_real_list_syntax() {
    for rule in [
        r"||www.youtube.com/youtubei/v1/player?$jsonprune=\$..[adPlacements\, adSlots\, playerAds]",
        r#"||api.bilibili.com/x/web-interface/wbi/index/top/feed/rcmd$jsonprune=\$.data.item[?(has "\$..business_info.ad_desc")]"#,
        r"||video.qq.com/trpc.vector_layout.page_view.PageService/getPage$jsonprune=\$.data.CardList..children_list.ad_list",
        r#"||video.qq.com/trpc.vector_layout.page_view.PageService/getPage$jsonprune=\$.data.CardList[?(has "\$.params.advertiser_name")]"#,
        r"||assets.msn.cn/service/news/feed/pages/$jsonprune=\$.sections..subSections..cards..[?(key-substr 'type' 'nativead')]",
        r"||msn.cn/resolver/api/resolve/$jsonprune=\$.configs['GalleryPage/ntpmsn'].properties.slideshowWCSettings.interstitialNativeAds",
        r"||msn.cn/resolver/api/resolve/$jsonprune=\$.configs['SlideShow/ntpmsn'].properties.additionalChildExperiences..children..childExperienceConfigInfo.configRef[?(key-substr '\$' 'NativeAd')]",
        r"||x.com/i/api/graphql/*/HomeTimeline$jsonprune=\$.data.home.home_timeline_urt.instructions..entries..[?(key-substr 'entryId' 'promoted-tweet-')]",
        r#"||example.com/feed$jsonprune=\$.data.publicDynamicConfig.campaign.feed.*..[?(key-eq "\$.isAd" true)]"#,
        r"||example.com/player$jsonprune=\$.state_machine.tracks[?(key-eq 'content_type' 'AD')]",
        r"||example.com/breaks$jsonprune=\$.adBreaks.*",
    ] {
        assert_eq!(check(rule), Some(true), "{rule}");
    }
}

#[test]
fn jsonprune_only_accepts_its_compatible_modifiers() {
    for option in [
        "domain=site.org",
        "from=site.org|~sub.site.org",
        "3p",
        "1p",
        "third-party",
        "first-party",
        "~3p",
        "~1p",
        "~third-party",
        "~first-party",
        "app=com.reader",
        "important",
        "xhr",
        "xmlhttprequest",
    ] {
        let rule = format!(r"||example.org^${option},jsonprune=\$.ad");
        assert_eq!(check(&rule), Some(true), "{rule}");
    }
    for option in [
        "strict-third-party",
        "strict-first-party",
        "script",
        "document",
        "content",
        "urlblock",
        "~xhr",
        "method=GET",
        "to=site.org",
        "badfilter",
        "replace=/a/b/",
        "jsonprune=\\$.other",
        "domain",
        "xhr=value",
    ] {
        let rule = format!(r"||example.org^${option},jsonprune=\$.ad");
        assert_eq!(check(&rule), Some(false), "{rule}");
    }
}

#[test]
fn rulecheck_accepts_prune_actions_and_reports_malformed_values() {
    let checked = crate::rulecheck::check(
        r"||example.org^$jsonprune=\$.ad
@@||example.org^$jsonprune
@@||example.org^$jsonprune=\$.ad
||example.org^$jsonprune=$.ad
||example.org^$jsonprune=\$..[a,b]
||example.org^$jsonprune=\$..
@@||example.org^$jsonprune=\$[?(script a)]
@@||example.org^$jsonprune=
||example.org^$jsonprune
||example.org^$jsonprune=\$..[a,$script",
    );
    assert_eq!(checked.rules, 10);
    assert_eq!(
        checked
            .issues
            .iter()
            .map(|issue| (issue.line, issue.kind))
            .collect::<Vec<_>>(),
        (4..=10)
            .map(|line| (line, IssueKind::Invalid))
            .collect::<Vec<_>>()
    );
}

#[test]
fn prune_exceptions_match_the_kind_and_unescaped_value() {
    let f = filters(
        r#"||example.org^$jsonprune=\$.[a\,b]
||example.org^$jsonprune=\$.c
||example.org^$replace=/original/changed/
@@||example.org/base
@@||example.org/no-prune$jsonprune
@@||example.org/no-replace$replace
@@||example.org/one$jsonprune=\$.[a\,b]
@@||example.org/spaced$jsonprune=\$.[a\, b]
"#,
    );
    let input = r#"{"a":1,"b":2,"c":3,"txt":"original"}"#;
    for (path, expected) in [
        ("base", r#"{"txt":"changed"}"#),
        ("no-prune", r#"{"a":1,"b":2,"c":3,"txt":"changed"}"#),
        ("no-replace", r#"{"txt":"original"}"#),
        ("one", r#"{"a":1,"b":2,"txt":"changed"}"#),
        ("spaced", r#"{"txt":"changed"}"#),
    ] {
        let url = format!("https://example.org/{path}");
        assert_eq!(pruned(&f, &url, Page::NONE, input), expected, "{path}");
    }
}

#[test]
fn prune_rules_run_in_lexical_order_then_replacements_run() {
    let f = filters(
        r"||example.org^$jsonprune=\$.items[1]
/order$replace=/items/output/
||example.org^$jsonprune=\$.items[0]
/order$jsonprune=\$.items[0]
",
    );
    let input = r#"{"items":[0,1,2,3,4]}"#;
    assert_eq!(
        pruned(&f, "https://example.org/order", Page::NONE, input),
        r#"{"output":[2,4]}"#
    );
}

#[test]
fn jsonprune_cache_keeps_trust_and_page_exceptions() {
    let temp = tempfile::tempdir().unwrap();
    let lists = temp.path().join("lists");
    let rules = temp.path().join("rules");
    let cache = temp.path().join("cache");
    for dir in [&lists, &rules] {
        std::fs::create_dir(dir).unwrap();
    }
    let catalog = "https://filters.adtidy.org/android/filters/11.txt";
    for (file, host) in [
        (lists.join("arbitrary.txt"), "untrusted.org"),
        (lists.join(file_name(catalog)), "stock.org"),
        (rules.join("user.txt"), "user.org"),
    ] {
        std::fs::write(
            file,
            format!(
                r"||{host}^$jsonprune=\$.ad
||{host}^$replace=/original/changed/
@@||{host}/keep$jsonprune=\$.ad
@@||no-content.org^$content
@@||no-document.org^$document
@@||no-urlblock.org^$urlblock
"
            ),
        )
        .unwrap();
    }
    let built = Filters::load_cached(&[&lists, &rules], &cache).unwrap();
    let cached = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
    let input = r#"{"ad":"original"}"#;
    for f in [built, cached] {
        for (url, page, expected) in [
            ("https://untrusted.org/", None, input),
            ("https://stock.org/", None, "{}"),
            ("https://user.org/", None, "{}"),
            ("https://user.org/keep", None, r#"{"ad":"changed"}"#),
            ("https://user.org/", Some("https://no-content.org/"), input),
            ("https://user.org/", Some("https://no-document.org/"), input),
            (
                "https://user.org/",
                Some("https://no-urlblock.org/"),
                r#"{"ad":"changed"}"#,
            ),
        ] {
            let page = page.map(|url| parse_http_page(url).unwrap());
            assert_eq!(
                pruned(
                    &f,
                    url,
                    page.as_ref().map_or(Page::NONE, Page::Named),
                    input
                ),
                expected,
                "{url}"
            );
        }
    }
}

#[test]
fn adblock_rust_accepts_match_case_for_regex_conditions_only() {
    assert_eq!(
        check(r"||example.org/Query$match-case,jsonprune=\$.ad"),
        Some(false)
    );
    let rule = r"/Query[0-9]+/$match-case,jsonprune=\$.ad";
    assert_eq!(check(rule), Some(true));
    let f = filters(rule);
    let input = r#"{"ad":1}"#;
    assert_eq!(
        pruned(&f, "https://example.org/Query42", Page::NONE, input),
        "{}"
    );
    assert_eq!(
        pruned(&f, "https://example.org/query42", Page::NONE, input),
        input
    );
}

#[test]
fn all_adds_no_content_type_restriction_to_response_rules() {
    let rules = r"/GetPlaybackResources?$jsonprune=\$.vodPlaybackUrls.result.playbackUrls.cuepoints,all,domain=amazon.com|amazon.co.jp|amazon.de|amazon.co.uk|primevideo.com
/GetPlaybackResources?$jsonprune=\$.cuepointPlaylist,all,domain=amazon.com|amazon.co.jp|amazon.de|amazon.co.uk|primevideo.com
/GetPlaybackResources?$replace=/original/changed/,all
";
    for rule in rules.lines() {
        assert_eq!(check(rule), Some(true), "{rule}");
    }
    let f = filters(rules);
    let page = parse_http_page("https://www.amazon.com/watch").unwrap();
    let url = "https://stream.example.org/GetPlaybackResources?id=1";
    let input = br#"{"vodPlaybackUrls":{"result":{"playbackUrls":{"cuepoints":[1]}}},"cuepointPlaylist":[],"title":"original"}"#;
    for kind in ["xhr", "media", "document", "other"] {
        let mut body = input.to_vec();
        for action in f
            .for_app("com.reader")
            .response_actions(url, Page::Named(&page), kind, "GET")
        {
            body = action.apply(&body, &|| true).unwrap();
        }
        assert_eq!(
            body, br#"{"vodPlaybackUrls":{"result":{"playbackUrls":{}}},"title":"changed"}"#,
            "{kind}"
        );
    }
}
