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

use bytes::Bytes;
use hyper::Uri;

use super::*;
use crate::rule::action::hls;

fn edited(f: &Filters, url: &str, page: Page<'_>, kind: &str, input: &str) -> Bytes {
    let actions = f
        .for_app("com.reader")
        .response_actions(url, page, kind, "GET");
    let rules: Vec<_> = actions
        .iter()
        .filter_map(|action| match action.as_ref() {
            ResponseAction::Hls(rule) => Some(rule),
            _ => None,
        })
        .collect();
    let base: Uri = url.parse().unwrap();
    hls::apply(
        Bytes::copy_from_slice(input.as_bytes()),
        &base,
        &rules,
        &|| true,
    )
    .unwrap()
}

#[test]
fn the_adguard_reference_playlist_is_edited_exactly_as_shown() {
    let input = r"#EXTM3U
#EXT-X-TARGETDURATION:10
#EXTINF,5
preroll.ts
#UPLYNK-SEGMENT:abc123,ad
#UPLYNK-KEY:aabb1122
#EXT-X-DISCONTINUITY
#EXTINF,10
01.ts
#EXTINF,10
02.ts
#UPLYNK-SEGMENT:abc123,segment
#UPLYNK-KEY:ccdd2233
#EXT-X-DISCONTINUITY
#EXTINF,10
01.ts
#EXTINF,10
02.ts
#EXT-X-ENDLIST
";
    let rules = r"||example.org^$hls=preroll
||example.org^$hls=/#UPLYNK-SEGMENT:.*\,ad/t
";
    let expected = r"#EXTM3U
#EXT-X-TARGETDURATION:10
#UPLYNK-SEGMENT:abc123,segment
#UPLYNK-KEY:ccdd2233
#EXT-X-DISCONTINUITY
#EXTINF,10
01.ts
#EXTINF,10
02.ts
#EXT-X-ENDLIST
";
    assert_eq!(
        edited(
            &filters(rules),
            "https://example.org/",
            Page::NONE,
            "media",
            input
        ),
        expected
    );
}

#[test]
fn all_three_reference_rule_values_remove_their_matching_segments() {
    for (line, segment) in [
        (
            r"||example.org^$hls=\/videoplayback^?*&source=dclk_video_ads",
            "videoplayback/?a=1&source=dclk_video_ads\n",
        ),
        (
            r"||example.org^$hls=/\/videoplayback\/?\?.*\&source=dclk_video_ads/i",
            "https://cdn.example/VIDEOplayback?a=1&source=DCLK_VIDEO_ADS\n",
        ),
        (
            r"||example.org^$hls=/#UPLYNK-SEGMENT:.*\,ad/t",
            "#UPLYNK-SEGMENT:abc123,ad\nad.ts\n#UPLYNK-SEGMENT:abc123,segment\n",
        ),
    ] {
        assert_eq!(check(line, &mut Patterns::default()), Some(true), "{line}");
        let input = format!("#EXTM3U\n{segment}keep.ts\n");
        let expected = if segment.starts_with('#') {
            "#EXTM3U\n#UPLYNK-SEGMENT:abc123,segment\nkeep.ts\n"
        } else {
            "#EXTM3U\nkeep.ts\n"
        };
        assert_eq!(
            edited(
                &filters(line),
                "https://example.org/hls/index.m3u8",
                Page::NONE,
                "media",
                &input
            ),
            expected,
            "{line}"
        );
    }
}

#[test]
fn patterns_can_match_either_absolute_urls_or_the_literal_lines() {
    let rules = r"||example.org^$hls=||example.org\/ads\/
||example.org^$hls=|pre-roll.ts|
||example.org^$hls=/^https:\/\/ads\.example\//";
    let input = "#EXTM3U\n../ads/relative.ts\nhttps://example.org/ads/absolute.ts\n\
                 pre-roll.ts\n//ads.example/other.ts\nkeep.ts?name=pre-roll.ts\n";
    assert_eq!(
        edited(
            &filters(rules),
            "https://example.org/hls/index.m3u8",
            Page::NONE,
            "media",
            input
        ),
        "#EXTM3U\nkeep.ts?name=pre-roll.ts\n"
    );
}

#[test]
fn a_cosmetic_marker_in_the_hls_value_is_matched_as_text() {
    let rules = "example.org$hls=/#%#/t";
    let input = "#EXTM3U\n#EXTINF:1,#%#ad\nad.ts\n#EXTINF:10,\nkeep.ts\n";
    assert_eq!(
        edited(
            &filters(rules),
            "https://example.org/",
            Page::NONE,
            "media",
            input
        ),
        "#EXTM3U\n#EXTINF:10,\nkeep.ts\n"
    );
}

#[test]
fn hls_accepts_only_supported_qualifiers_and_valid_values() {
    for option in [
        "domain=site.org",
        "third-party",
        "~third-party",
        "app=com.reader",
        "important",
        "match-case",
        "xmlhttprequest",
    ] {
        let line = format!("||example.org/Video${option},hls=ads");
        assert_eq!(check(&line, &mut Patterns::default()), Some(true), "{line}");
    }
    for option in [
        "strict-third-party",
        "strict-first-party",
        "first-party",
        "all",
        "media",
        "document",
        "content",
        "urlblock",
        "method=GET",
        "badfilter",
        "replace=/x/y/",
        "hls=another",
        "_",
    ] {
        let line = format!("||example.org^${option},hls=ads");
        assert_eq!(
            check(&line, &mut Patterns::default()),
            Some(false),
            "{line}"
        );
    }
    for rule in [
        "||example.org^$hls",
        "||example.org^$hls=",
        "@@||example.org^$hls=",
        "||example.org^$hls=ads$script",
        "||example.org^$hls=/ads$/t",
        "||example.org^$hls=/ads,tag/t",
        "||example.org^$hls=ads/clip",
        "||example.org^$hls=/ads/g",
        "||example.org^$hls=/[a/",
        "@@||example.org^$hls=/[a/",
    ] {
        assert_eq!(check(rule, &mut Patterns::default()), Some(false), "{rule}");
    }
}

#[test]
fn the_request_condition_can_require_case_page_party_app_and_type() {
    let rules = "||example.org/Video$hls=ads,match-case,app=com.reader,domain=site.org,third-party,xmlhttprequest,important";
    let filters = filters(rules);
    let site = parse_http_page("https://site.org/").unwrap();
    let other = parse_http_page("https://other.org/").unwrap();
    let input = "#EXTM3U\nads.ts\nkeep.ts\n";
    for (path, page, kind, expected) in [
        ("Video", &site, "xmlhttprequest", "#EXTM3U\nkeep.ts\n"),
        ("video", &site, "xmlhttprequest", input),
        ("Video", &other, "xmlhttprequest", input),
        ("Video", &site, "media", input),
    ] {
        let url = format!("https://example.org/{path}");
        assert_eq!(
            edited(&filters, &url, Page::Named(page), kind, input),
            expected
        );
    }
    assert!(
        filters
            .for_app("com.other")
            .response_actions(
                "https://example.org/Video",
                Page::Named(&site),
                "xmlhttprequest",
                "GET"
            )
            .is_empty()
    );
}

#[test]
fn hls_trust_and_exceptions_survive_the_compiled_cache() {
    let temp = tempfile::tempdir().unwrap();
    let lists = temp.path().join("lists");
    let rules = temp.path().join("rules");
    let cache = temp.path().join("cache");
    for dir in [&lists, &rules] {
        std::fs::create_dir(dir).unwrap();
    }
    let catalog = "https://filters.adtidy.org/android/filters/11.txt";
    std::fs::write(
        lists.join("arbitrary.txt"),
        "||untrusted.org^$hls=first\n@@||stock.org^$hls\n",
    )
    .unwrap();
    std::fs::write(lists.join(file_name(catalog)), "||stock.org^$hls=first\n").unwrap();
    std::fs::write(
        rules.join("user.txt"),
        r"||example.org^$hls=first
||example.org^$hls=/second/
@@||example.org/basic
@@||example.org/all$hls
@@||example.org/first$hls=first
@@||example.org/second$hls=/second/
@@||example.org/different$hls=/second/i
@@||document.site^$document
@@||content.site^$content
@@||urlblock.site^$urlblock
",
    )
    .unwrap();

    let built = Filters::load_cached(&[&lists, &rules], &cache).unwrap();
    let cached = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
    let input = "#EXTM3U\nfirst.ts\nsecond.ts\nkeep.ts\n";
    for f in [built, cached] {
        for (url, page, expected) in [
            ("https://untrusted.org/", None, input),
            ("https://stock.org/", None, "#EXTM3U\nsecond.ts\nkeep.ts\n"),
            ("https://example.org/basic", None, "#EXTM3U\nkeep.ts\n"),
            ("https://example.org/all", None, input),
            (
                "https://example.org/first",
                None,
                "#EXTM3U\nfirst.ts\nkeep.ts\n",
            ),
            (
                "https://example.org/second",
                None,
                "#EXTM3U\nsecond.ts\nkeep.ts\n",
            ),
            ("https://example.org/different", None, "#EXTM3U\nkeep.ts\n"),
            (
                "https://example.org/",
                Some("https://document.site/"),
                input,
            ),
            ("https://example.org/", Some("https://content.site/"), input),
            (
                "https://example.org/",
                Some("https://urlblock.site/"),
                input,
            ),
        ] {
            let page = page.map(|url| parse_http_page(url).unwrap());
            assert_eq!(
                edited(
                    &f,
                    url,
                    page.as_ref().map_or(Page::NONE, Page::Named),
                    "media",
                    input
                ),
                expected,
                "{url}"
            );
        }
    }
}
