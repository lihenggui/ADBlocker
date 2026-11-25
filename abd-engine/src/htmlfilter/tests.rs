// SPDX-License-Identifier: GPL-3.0-only
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
use crate::rule::{self, Kind};

fn rules(text: &str) -> Vec<Arc<Compiled>> {
    text.lines()
        .map(|line| {
            let Kind::Html(parsed) = rule::read(line).kind else {
                panic!("rejected {line}");
            };
            Arc::new(parsed.compiled)
        })
        .collect()
}

fn check(selector: &str, html: &str, expected: &str) {
    let body = Bytes::copy_from_slice(html.as_bytes());
    assert_eq!(
        apply(body, &rules(selector), None, &|| true).unwrap(),
        expected,
        "{selector}"
    );
}

#[test]
fn reference_attribute_examples_remove_elements_with_a_substring_or_attribute() {
    check(
        r#"example.org$$script[data-src="banner"]"#,
        r#"<script data-src="/banner.js"></script><script data-src="/main.js"></script>"#,
        r#"<script data-src="/main.js"></script>"#,
    );
    check(
        "example.org$$div[some_attribute]",
        r#"<div some_attribute="some_value"></div><div some_attribute></div><div></div>"#,
        "<div></div>",
    );
}

#[test]
fn reference_contains_examples_match_literal_text_or_regexes() {
    for (selector, content) in [
        ("script:contains(Adverts)", "new Adverts()"),
        ("div:contains(Sponsored by)", "Sponsored by someone"),
        (r"script:contains(/ad_system_\d+/)", "ad_system_123"),
        (r#"div:contains("Sponsored by")"#, r#"a "Sponsored by" b"#),
        (
            "script:contains((function(g,b,a,c,e,d)",
            "(function(g,b,a,c,e,d){})",
        ),
    ] {
        let tag = selector.split(':').next().unwrap();
        check(
            &format!("example.com$${selector}"),
            &format!("<{tag}>{content}</{tag}><{tag}>stay</{tag}>"),
            &format!("<{tag}>stay</{tag}>"),
        );
    }
    check(
        r#"$$div:contains("Sponsored by")"#,
        "<div>Sponsored by</div>",
        "<div>Sponsored by</div>",
    );
    check(
        "$$script:contains( advert )",
        "<script>advert</script><script> advert </script>",
        "<script>advert</script>",
    );
}

#[test]
fn all_three_pseudo_names_and_multiple_conditions_work() {
    for name in ["contains", "-abp-contains", "has-text"] {
        check(
            &format!("$$script:{name}(one):contains(two)"),
            "<script>one</script><script>two one</script>",
            "<script>one</script>",
        );
    }
    check("$$script:contains(foo)bar)", "<script>foo)bar</script>", "");
}

#[test]
fn reference_content_attributes_match_quotes_globs_and_serialized_inner_html() {
    check(
        r#"$$script[tag-content="banner"]"#,
        "<script type=\"text/javascript\">\n    document.write('<div>banner text</div>\" />');\n</script>",
        "",
    );
    check(
        r#"$$script[tag-content="alert(""this is ad"")"]"#,
        r#"<script>alert("this is ad")</script><script>alert('this is ad')</script>"#,
        r#"<script>alert('this is ad')</script>"#,
    );
    check(
        r#"$$script[wildcard=""banner""]"#,
        r#"<script>"banner"</script>"#,
        "",
    );
    check(
        r#"$$script[wildcard="*banner*text*"]"#,
        "<script>\nbanner first\ntext second\n</script><script>text then banner</script>",
        "<script>text then banner</script>",
    );
    check(
        r#"$$div[wildcard="a?c"]"#,
        "<div>abc</div><div>a漢c</div><div>abcd</div>",
        "<div>abcd</div>",
    );
    check(
        r#"$$div[tag-content="<b>banner</b>"]"#,
        "<div><b>banner</b></div><div>banner</div>",
        "<div>banner</div>",
    );
}

#[test]
fn reference_length_pairs_are_equivalent_including_boundaries() {
    let before =
        [10, 399, 400, 401, 800, 801].map(|n| format!("<div>banner{}</div>", ".".repeat(n - 6)));
    for (left, right, kept) in [
        (
            r#"div[tag-content="banner"]"#,
            "div:contains(banner)",
            vec![],
        ),
        (
            r#"div[wildcard="*banner*"]"#,
            "div:contains(/^.*banner.*$/s)",
            vec![],
        ),
        (
            r#"div[min-length="400"]"#,
            "div:contains(/^(?=.{400,}$).*/s)",
            vec![0, 1],
        ),
        (
            r#"div[min-length="400"][max-length="800"]"#,
            "div:contains(/^(?=.{400,800}$).*/s)",
            vec![0, 1, 5],
        ),
        (
            r#"div[tag-content="banner"][max-length="400"]"#,
            "div:contains(banner):contains(/^(?=.{0,400}$).*/s)",
            vec![3, 4, 5],
        ),
        (
            r#"div[tag-content="banner"][min-length="400"]"#,
            "div:contains(banner):contains(/^(?=.{400,}$).*/s)",
            vec![0, 1],
        ),
    ] {
        let expected: String = kept.iter().map(|&i| before[i].as_str()).collect();
        check(&format!("$${left}"), &before.concat(), &expected);
        check(&format!("$${right}"), &before.concat(), &expected);
    }
}

#[test]
fn a_length_attribute_accepts_values_above_the_regex_quantifier_limit() {
    let block = |length: usize| format!("<div>banner{}</div>", "x".repeat(length - 6));
    let kept = block(300_001);
    check(
        r#"$$div[tag-content="banner"][max-length="300000"]"#,
        &format!("{}{kept}", block(300_000)),
        &kept,
    );
}

#[test]
fn look_aheads_match_whole_inner_html_with_nested_groups_and_classes() {
    check(
        r"$$script:contains(/^(?=.{4,12}$)(?=.*[)](?:b|c).*$).*/is)",
        "<script>a)b\n</script><script>a)B</script><script>abcdefghijkl)b</script><script>xyzz</script>",
        "<script>a)B</script><script>abcdefghijkl)b</script><script>xyzz</script>",
    );
    check(
        "$$div:contains(/^(?=a|b$).*/s)",
        "<div>a</div><div>b</div><div>ab</div>",
        "<div>ab</div>",
    );
}

#[test]
fn prefix_look_aheads_and_the_remainder_must_all_match_from_the_start() {
    check(
        "$$div:contains(/^(?=ad)(?=.*sponsor)ad.*end$/is)",
        "<div>AD\nSponsor\nEND</div><div>xAD Sponsor END</div><div>ad sponsor end more</div><div>ad end</div>",
        "<div>xAD Sponsor END</div><div>ad sponsor end more</div><div>ad end</div>",
    );
    for remainder in ["", ".*", ".*$"] {
        check(
            &format!("$$div:contains(/^(?=ad){remainder}/s)"),
            "<div>ad\ntext</div><div>x ad</div>",
            "<div>x ad</div>",
        );
    }
}

#[test]
fn whole_look_aheads_and_literal_dollars_keep_their_distinct_meanings() {
    check(
        "$$div:contains(/^(?=ad$)a/)",
        "<div>ad</div><div>adv</div>",
        "<div>adv</div>",
    );
    check(
        r"$$div:contains(/^(?=ad\$)a/)",
        "<div>ad$text</div><div>ad</div>",
        "<div>ad</div>",
    );
}

#[test]
fn a_length_look_ahead_obeys_the_regex_dot_all_flag() {
    let body = "<div>广告</div><div>a\n</div><div>abc</div>";
    check(
        "$$div:contains(/^(?=.{2}$).*/)",
        body,
        "<div>a\n</div><div>abc</div>",
    );
    check("$$div:contains(/^(?=.{2}$).*/s)", body, "<div>abc</div>");
    check(
        "$$div:contains(/^(?=a).*$/)",
        "<div>a\nb</div><div>ab</div>",
        "<div>a\nb</div>",
    );
}

#[test]
fn ordinary_attribute_operators_match_substrings_prefixes_and_suffixes() {
    let first = r#"<script src="/ads/script.js"></script>"#;
    let second = r#"<script src="https://example.org/ads/script.js?version=1"></script>"#;
    let third = r#"<script src="/ADS/script.js?version=1"></script>"#;
    let body = format!("{first}{second}{third}");
    for (selector, expected) in [
        (r#"$$script[src="/ads/"]"#, third.to_owned()),
        (r#"$$script[src*="/ads/"]"#, third.to_owned()),
        (r#"$$script[SRC ^= "/ads/"]"#, format!("{second}{third}")),
        (r#"$$script[src$=.js]"#, format!("{second}{third}")),
    ] {
        check(selector, &body, &expected);
    }
}

#[test]
fn a_child_must_match_each_direct_ancestor_and_may_have_no_tag() {
    check(
        r#"$$main > [class="ad"] > script[src=x]"#,
        "<main><div class=ad><script src=/x.js></script><p><script src=/x.js></script></p></div></main>",
        "<main><div class=ad><p><script src=/x.js></script></p></div></main>",
    );
}

#[test]
fn every_raw_element_ends_only_at_its_own_end_tag() {
    for tag in [
        "script", "style", "textarea", "title", "xmp", "iframe", "noembed", "noframes",
    ] {
        let raw = format!(
            "<{tag}>const x = '</div><div data-ad></div></{tag}ing>';</{}>",
            tag.to_ascii_uppercase()
        );
        check(
            "$$div[data-ad]",
            &format!("<div>{raw}<div data-ad>x</div></div>"),
            &format!("<div>{raw}</div>"),
        );
        check(
            &format!("$${tag}:contains(</div>)"),
            &format!("<div>{raw}</div>"),
            "<div></div>",
        );
    }
}

#[test]
fn void_and_self_closed_elements_have_empty_content() {
    for name in [
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source",
        "track", "wbr", "thing",
    ] {
        let slash = if name == "thing" { "/" } else { "" };
        check(
            &format!("$${name}[max-length=0]"),
            &format!("<main><{name}{slash}>keep</main>"),
            "<main>keep</main>",
        );
    }
    check(
        "$$div[max-length=0]",
        "<div attr=value/>still content</div>",
        "<div attr=value/>still content</div>",
    );
}

#[test]
fn open_at_end_elements_have_no_known_extent() {
    check(
        "$$div\n$$script",
        "<div><script src=x>ad",
        "<div><script src=x>ad",
    );
    check("$$div\n$$span", "<div><span>ad</span>", "<div>");
}

#[test]
fn a_stray_end_tag_leaves_open_elements_alone() {
    check(
        "$$div:contains(</p>)",
        "</div><div></p>ad</div></div>",
        "</div></div>",
    );
}

#[test]
fn an_end_tag_closes_the_nearest_match_and_its_unclosed_children() {
    check(
        "$$main > div[data-ad]",
        "<main><div data-ad><span>ad</main>",
        "<main></main>",
    );
    check(
        "$$div[data-ad]",
        "<div><div data-ad>ad</div>keep</div>",
        "<div>keep</div>",
    );
    check(
        "$$div > span",
        "<div><span>ad</div><span>keep</span>",
        "<div></div><span>keep</span>",
    );
}

#[test]
fn nested_removals_merge_without_touching_adjacent_bytes() {
    check(
        "$$div\n$$script\n$$meta",
        "start<div><meta><div><script>x</script></div></div><meta>end",
        "startend",
    );
}

#[test]
fn comments_doctype_and_cdata_are_not_element_markup() {
    let declarations = r#"<!DOCTYPE html PUBLIC "> <div>doctype</div>"><!-- <div>comment</div> --><![CDATA[<div>data</div>]]>"#;
    check(
        "$$div",
        &format!("{declarations}<div>ad</div>"),
        declarations,
    );
}

#[test]
fn attribute_and_tag_names_ignore_ascii_case_but_values_are_as_written() {
    check(
        r#"$$DIV[DATA-ID="a""b"]"#,
        r#"<DiV DaTa-Id='x>a"bc'>ad</DiV><div data-id='A"b'>keep</div>"#,
        r#"<div data-id='A"b'>keep</div>"#,
    );
    check(
        r#"$$[id="&quot;"]"#,
        r#"<p id='&quot;'></p><p id='"'></p>"#,
        r#"<p id='"'></p>"#,
    );
}

#[test]
fn a_gbk_page_matches_chinese_content_and_counts_characters_not_bytes() {
    for (header, meta) in [(None, "<meta charset=gbk>"), (Some("GBK"), "")] {
        let make = |html: &str| Bytes::from(encoding_rs::GBK.encode(html).0.into_owned());
        let start = format!("<head>{meta}</head><body>");
        let body = make(&format!("{start}<div>广告</div><div>内容</div></body>"));
        let selected = rules(r#"$$div[tag-content="广告"][min-length=2][max-length=2]"#);
        assert_eq!(
            apply(body, &selected, header, &|| true).unwrap(),
            make(&format!("{start}<div>内容</div></body>"))
        );
    }
}

#[test]
fn utf_16_and_utf_32_boms_and_declarations_pass_through() {
    for (bytes, label) in [
        (b"\xff\xfe<\0d\0i\0v\0>\0".as_slice(), None),
        (b"\xfe\xff\0<\0d\0i\0v\0>", Some("UTF-8")),
        (b"\0\0\xfe\xff\0\0\0<", None),
        (b"\xff\xfe\0\0<\0\0\0", None),
        (b"<div>ad</div>", Some("UTF-16")),
        (b"<div>ad</div>", Some("UTF-32LE")),
        (b"<meta charset=utf-16><div>ad</div>", None),
    ] {
        let original = Bytes::from_static(bytes);
        let result = apply(original.clone(), &rules("$$div"), label, &|| true).unwrap();
        assert_eq!(result.as_ptr(), original.as_ptr());
        assert_eq!(result, original);
    }
}

#[test]
fn elements_beyond_the_depth_bound_are_skipped_and_shallower_siblings_are_tested() {
    let start = "<div>".repeat(MAX_HTML_DEPTH);
    let end = "</div>".repeat(MAX_HTML_DEPTH);
    let deep = "<div data-ad><div data-ad>deep</div></div>";
    check(
        "$$div[data-ad]",
        &format!("<div data-ad>first</div>{start}{deep}{end}<div data-ad>last</div>"),
        &format!("{start}{deep}{end}"),
    );
}

#[test]
fn excessive_element_visits_cancel_even_an_earlier_cut() {
    let body = Bytes::from(format!(
        "<div>first</div>{}",
        "<br>".repeat(MAX_HTML_ELEMENTS)
    ));
    assert!(apply(body, &rules("$$div"), None, &|| true).is_none());
}

#[test]
fn excessive_total_content_decoding_cancels_the_whole_edit() {
    let depth = 8;
    let body = Bytes::from(format!(
        "{}{}{}",
        "<div>".repeat(depth),
        "x".repeat(MAX_HTML_DECODED / depth + 1),
        "</div>".repeat(depth)
    ));
    assert!(apply(body, &rules("$$div:contains(y)"), None, &|| true).is_none());
}

#[test]
fn cancellation_discards_pending_cuts() {
    let count = std::cell::Cell::new(0);
    let check = || {
        count.set(count.get() + 1);
        count.get() <= 3
    };
    assert!(
        apply(
            Bytes::from_static(b"<div>ad</div><p>keep</p>"),
            &rules("$$div"),
            None,
            &check
        )
        .is_none()
    );
}

#[test]
fn unchanged_results_return_the_original_allocation() {
    let original = Bytes::from(b"<p>kept</p>".to_vec());
    for selected in [Vec::new(), rules("$$div")] {
        let passed = apply(original.clone(), &selected, None, &|| true).unwrap();
        assert_eq!(passed.as_ptr(), original.as_ptr());
        assert_eq!(passed, original);
    }
}

mod properties {
    use proptest::prelude::*;

    use super::*;
    use crate::test_support::{arbitrary_bytes, assert_removal, mutate};

    fn document() -> impl Strategy<Value = Bytes> {
        let item = prop_oneof![
            ("[a-z]{0,16}", any::<bool>()).prop_map(|(text, marked)| {
                let class = if marked { "ad" } else { "keep" };
                format!("<div class='{class}'><span>{text}</span></div>")
            }),
            prop::sample::select(vec![
                "<aside class=ad><img class=ad src='x'><p>promo</p></aside>",
                "<script>const text = '<div class=ad>';</script>",
                "<style>x { color: red; }</style>",
                "<!-- <div class=ad> -->",
                "<BR CLASS=ad>",
                "<div class=ad><span>missing close</div>",
                "plain text &lt;&gt;",
                "<<div class=ad></div>img class=ad>",
            ])
            .prop_map(str::to_owned),
        ];
        let html = (
            prop::collection::vec(item, 0..16),
            prop::option::of((any::<prop::sample::Index>(), prop::option::of(any::<u8>()))),
        )
            .prop_map(|(items, mutation)| {
                mutate(
                    format!(
                        "<!doctype html><html><body>{}</body></html>",
                        items.concat()
                    )
                    .into_bytes(),
                    mutation,
                )
            });
        prop_oneof![arbitrary_bytes(), html].prop_map(Bytes::from)
    }

    fn selector(tag: Option<&str>, marked: bool) -> Selector {
        Selector {
            tag: tag.map(str::to_owned),
            attributes: if marked {
                vec![Attribute {
                    name: "class".to_owned(),
                    value: Some(AttributeValue::Contains("ad".to_owned())),
                }]
            } else {
                Vec::new()
            },
        }
    }

    fn rule() -> impl Strategy<Value = (Option<&'static str>, bool, bool, Option<&'static str>)> {
        (
            prop::sample::select(vec![None, Some("div"), Some("p"), Some("img")]),
            any::<bool>(),
            any::<bool>(),
            prop::option::of(prop::sample::select(vec!["promo", "span", "ad"])),
        )
    }

    #[test]
    fn a_cut_cannot_turn_literal_text_into_a_new_tag() {
        let input = Bytes::from_static(
            b"<!doctype html><html><body><<div class=ad></div>img class=ad></body></html>",
        );
        let rules = [Arc::new(Compiled {
            steps: vec![selector(None, true)],
            content: Vec::new(),
        })];
        assert_eq!(
            apply(input.clone(), &rules, Some("utf-8"), &|| true),
            Some(input)
        );
    }

    proptest! {
        #[test]
        fn filtering_returns_original_bytes_with_only_removals(
            input in document(), rules in prop::collection::vec(rule(), 1..4),
        ) {
            let rules: Vec<_> = rules.into_iter().map(|(tag, marked, nested, text)| {
                let mut steps = Vec::new();
                if nested {
                    steps.push(selector(Some("aside"), false));
                }
                steps.push(selector(tag, marked));
                Arc::new(Compiled {
                    steps,
                    content: text.into_iter().map(|s| Content::Contains(s.to_owned())).collect(),
                })
            }).collect();
            if let Some(output) = apply(input.clone(), &rules, Some("utf-8"), &|| true) {
                assert_removal(&input, &output)?;
            }
        }

        #[test]
        fn no_rules_or_no_matching_elements_leaves_every_byte_intact(input in document()) {
            let absent = Arc::new(Compiled {
                steps: vec![selector(Some("absent-property-test-element"), false)],
                content: Vec::new(),
            });
            for rules in [&[][..], &[absent][..]] {
                prop_assert_eq!(apply(input.clone(), rules, Some("utf-8"), &|| true), Some(input.clone()));
            }
        }

        #[test]
        fn single_step_structural_rules_remove_nothing_more_on_a_second_pass(
            input in document(),
            tags in prop::collection::vec(
                (prop::sample::select(vec![None, Some("div"), Some("p"), Some("img"), Some("span")]),
                 any::<bool>()),
                1..4,
            ),
        ) {
            // Content predicates can become true after a descendant is cut
            // (for example when a surviving parent's text gets shorter).
            let rules: Vec<_> = tags.into_iter().map(|(tag, marked)| Arc::new(Compiled {
                steps: vec![selector(tag, marked)],
                content: Vec::new(),
            })).collect();
            if let Some(output) = apply(input, &rules, Some("utf-8"), &|| true) {
                prop_assert_eq!(apply(output.clone(), &rules, Some("utf-8"), &|| true), Some(output));
            }
        }
    }
}
