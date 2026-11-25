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

const STREAM: &str = "/GetPlaybackResources?*%2CCuepointPlaylist$all,urltransform=/%2CCuepointPlaylist//,domain=amazon.com|amazon.co.jp|amazon.de|amazon.co.uk|primevideo.com";
const GOG: &str = r"||af.gog.com/game/$urltransform=/^https:\/\/af\.gog\.com\/game\//https:\/\/www.gog\.com\/game\//";
const PCT: &str = r"/^https?:\/\/(?:[a-z0-9-]+\.)*?aff\.example\.com\/visit\?url=/$urltransform=/^https?:\/\/(?:[a-z0-9-]+\.)*?aff\.example\.com\/visit\?url=([^&]*).*/\$1/|pct";
const B64: &str = r"/^https?:\/\/(?:[a-z0-9-]+\.)*?aff\.example\.com\/visit\?url=/$urltransform=/^https?:\/\/(?:[a-z0-9-]+\.)*?aff\.example\.com\/visit\?url=([^&]*).*/\$1/|b64|/[?&]ref=[^&]*//";

fn transformed(filters: &Filters, url: &str, page: Page<'_>, kind: &str, method: &str) -> String {
    let mut result = url.to_owned();
    for action in filters
        .for_app("com.reader")
        .response_actions(url, page, kind, method)
    {
        if let ResponseAction::UrlTransform(action) = action.as_ref() {
            let start = if action.whole_url {
                0
            } else {
                let authority = result.find("://").unwrap() + 3;
                authority + result[authority..].find('/').unwrap()
            };
            let changed = action.apply(&result[start..]).unwrap().into_owned();
            result.replace_range(start.., &changed);
        }
    }
    result
}

#[test]
fn both_real_base_rules_and_the_reference_affiliate_rules_transform_their_urls() {
    for (rule, url, expected, kind) in [
        (
            STREAM,
            "https://www.amazon.com/GetPlaybackResources?x=1%2CCuepointPlaylist",
            "https://www.amazon.com/GetPlaybackResources?x=1",
            "media",
        ),
        (
            GOG,
            "https://af.gog.com/game/slug",
            "https://www.gog.com/game/slug",
            "document",
        ),
        (
            PCT,
            "https://www.aff.example.com/visit?url=https%3A%2F%2Fwww.somestore.com%2F&ref=ref-123",
            "https://www.somestore.com/",
            "document",
        ),
        (
            B64,
            "https://www.aff.example.com/visit?url=aHR0cHM6Ly93d3cuc29tZXN0b3JlLmNvbS8/cmVmPXJlZi0xMjM=",
            "https://www.somestore.com/",
            "document",
        ),
    ] {
        assert!(crate::rulecheck::check(rule).issues.is_empty(), "{rule}");
        assert_eq!(
            transformed(&filters(rule), url, Page::Itself, kind, "GET"),
            expected
        );
    }
}

#[test]
fn transforms_default_to_documents_and_all_and_negated_types_qualify() {
    const URL: &str = "https://example.org/X";
    for (options, types) in [
        ("", ["document"].as_slice()),
        (",all", &["document", "image", "other"]),
        (",all,image,~document", &["image", "other"]),
        (",image", &["image"]),
        (",~image", &["document", "other"]),
        (",document,image", &["document", "image"]),
    ] {
        let f = filters(&format!("||example.org^$urltransform=/X/Y/{options}"));
        for kind in ["document", "image", "other"] {
            let expected = if types.contains(&kind) {
                "https://example.org/Y"
            } else {
                URL
            };
            assert_eq!(transformed(&f, URL, Page::NONE, kind, "GET"), expected);
        }
    }
}

#[test]
fn compatible_qualifiers_match_the_method_source_and_destination() {
    let f = filters(
        r"/X$urltransform=/X/Y/,third-party,domain=page.org,to=example.org,method=GET,match-case,important",
    );
    let page = parse_http_page("https://page.org/").unwrap();
    let url = "https://example.org/X";
    assert_eq!(
        transformed(&f, url, Page::Named(&page), "document", "GET"),
        "https://example.org/Y"
    );
    for (url, page, method) in [
        (url, Page::NONE, "GET"),
        (url, Page::Named(&page), "POST"),
        ("https://a.test/X", Page::Named(&page), "GET"),
        ("https://example.org/x", Page::Named(&page), "GET"),
    ] {
        assert_eq!(transformed(&f, url, page, "document", method), url);
    }
    let f = filters("/track$urltransform=/X/Y/,denyallow=example.org");
    for (url, expected) in [
        ("https://example.org/track/X", "https://example.org/track/X"),
        ("https://a.test/track/X", "https://a.test/track/Y"),
    ] {
        assert_eq!(
            transformed(&f, url, Page::NONE, "document", "GET"),
            expected
        );
    }
    let f = filters("||example.org/X$urltransform=/x/y/i,match-case");
    let lower = "https://example.org/x";
    assert_eq!(transformed(&f, lower, Page::NONE, "document", "GET"), lower);
    assert_eq!(
        transformed(&f, url, Page::NONE, "document", "GET"),
        "https://example.org/y"
    );
}

#[test]
fn incompatible_modifiers_and_malformed_values_do_not_become_blocks() {
    for suffix in [
        "popup",
        "app=com.reader",
        "content",
        "first-party",
        "removeparam=id",
        "replace=/a/b/",
        "tag=tracking",
        "_",
    ] {
        let rule = format!("||example.org/X$urltransform=/X/Y/,{suffix}");
        assert_eq!(crate::rulecheck::check(&rule).issues.len(), 1, "{rule}");
        let url = "https://example.org/X";
        let f = filters(&rule);
        assert_eq!(transformed(&f, url, Page::Itself, "document", "GET"), url);
        assert!(!f.check(url, "", "document", "GET").blocked);
    }
    for value in ["/x/y$oops", "/x/a,b/", "/x/y/|bad"] {
        let rule = format!("||example.org/X$urltransform={value}");
        assert_eq!(crate::rulecheck::check(&rule).issues.len(), 1, "{rule}");
        let f = filters(&rule);
        assert!(
            !f.check("https://example.org/X", "", "document", "GET")
                .blocked
        );
    }
}

#[test]
fn exceptions_follow_the_reference_and_rules_run_in_lexicographical_order() {
    let f = filters(
        "||example.org^$urltransform=/Z/Y/\n\
                     @@||example.org/page/*$urltransform=/Z/Y/\n\
                     ||example.org^$urltransform=/X/Y/\n\
                     ||example.org^$urltransform=/A/Q/\n\
                     ||example.org^$urltransform=/Q/R/\n\
                     @@||example.org/off^$urltransform",
    );
    for (path, expected) in [
        ("/XZA", "/YYR"),
        ("/page/XZA", "/page/YZR"),
        ("/off/XZA", "/off/XZA"),
    ] {
        let url = format!("https://example.org{path}");
        assert_eq!(
            transformed(&f, &url, Page::Itself, "document", "GET"),
            format!("https://example.org{expected}")
        );
    }
    let f = filters("||example.org^$urltransform=/X/Y/\n||example.org/X$urltransform=/X/Y/");
    let url = "https://example.org/XX";
    assert_eq!(
        transformed(&f, url, Page::Itself, "document", "GET"),
        "https://example.org/YY"
    );
}

#[test]
fn only_document_and_urlblock_page_exceptions_disable_transforms() {
    for (exception, applies) in [
        ("", true),
        ("$content", true),
        ("$document", false),
        ("$urlblock", false),
    ] {
        let f = filters(&format!(
            "||example.org^$urltransform=/X/Y/\n@@||example.org^{exception}\n@@||page.org^{exception}"
        ));
        let page = parse_http_page("https://page.org/").unwrap();
        for page in [Page::Itself, Page::Named(&page)] {
            let url = "https://example.org/X";
            let expected = if applies {
                "https://example.org/Y"
            } else {
                url
            };
            assert_eq!(transformed(&f, url, page, "document", "GET"), expected);
        }
    }
}

#[test]
fn badfilter_disables_a_transform_in_either_option_order() {
    let rule = "||example.org^$urltransform=/X/Y/,all\n\
                ||example.org^$all,badfilter,urltransform=/X/Y/\n\
                ||example.org^$urltransform=/Z/Y/,all";
    assert!(crate::rulecheck::check(rule).issues.is_empty());
    assert_eq!(
        transformed(
            &filters(rule),
            "https://example.org/XZ",
            Page::Itself,
            "image",
            "GET"
        ),
        "https://example.org/XY"
    );
}

#[test]
fn only_trusted_transforms_and_exceptions_survive_the_cache() {
    let temp = tempfile::tempdir().unwrap();
    let lists = temp.path().join("lists");
    let rules = temp.path().join("rules");
    let cache = temp.path().join("cache");
    for dir in [&lists, &rules] {
        std::fs::create_dir(dir).unwrap();
    }
    let catalog = "https://filters.adtidy.org/android/filters/11.txt";
    std::fs::write(
        lists.join(file_name(catalog)),
        "||stock.org^$urltransform=/X/Y/\n@@||stock.org/skip$urltransform=/X/Y/",
    )
    .unwrap();
    std::fs::write(
        lists.join(untrusted_file_name(catalog).unwrap()),
        "||custom.org^$urltransform=/X/Y/\n@@||stock.org^$urltransform\n||stock.org^$urltransform=/X/Y/,badfilter",
    ).unwrap();
    std::fs::write(rules.join("user.txt"), "||user.org^$urltransform=/X/Y/").unwrap();
    let built = Filters::load_cached(&[&lists, &rules], &cache).unwrap();
    let cached = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
    for filter in [built, cached] {
        for (url, expected) in [
            ("https://stock.org/X", "https://stock.org/Y"),
            ("https://stock.org/skip/X", "https://stock.org/skip/X"),
            ("https://user.org/X", "https://user.org/Y"),
            ("https://custom.org/X", "https://custom.org/X"),
        ] {
            assert_eq!(
                transformed(&filter, url, Page::Itself, "document", "GET"),
                expected
            );
        }
    }
}
