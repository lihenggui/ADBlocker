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

mod bookstore;

fn edited(f: &Filters, url: &str, page: Page<'_>, kind: &str, input: &str) -> Bytes {
    let mut body = Bytes::copy_from_slice(input.as_bytes());
    for action in f
        .for_app("com.reader")
        .response_actions(url, page, kind, "GET")
    {
        if action.kind() == ActionKind::XmlPrune {
            body = action.apply(body, &|| true).unwrap();
        }
    }
    body
}

#[test]
fn xmlprune_matches_each_bookstore_example() {
    use bookstore::*;
    for (rule, input, expected, removed) in [
        (
            "||example.org^$xmlprune=/bookstore/book[position() mod 2 = 1]",
            BOOKS,
            ODD_BOOKS_REMOVED,
            &[0, 2][..],
        ),
        (
            "||example.org^$xmlprune=/bookstore/book[year = 2003]",
            BOOKS,
            BOOKS_FROM_2003_REMOVED,
            &[2, 3][..],
        ),
        (
            "||example.org^$xmlprune=//*/@*",
            ATTRIBUTES,
            ATTRIBUTES_REMOVED,
            &[][..],
        ),
    ] {
        let actual = edited(
            &filters(rule),
            "https://example.org/books.xml",
            Page::NONE,
            "other",
            input,
        );
        let actual = std::str::from_utf8(&actual).unwrap();
        if removed.is_empty() {
            assert_eq!(actual, expected, "{rule}");
        } else {
            // The reference reindents the surviving books. Verify its content
            // and verify separately that the source indentation is preserved.
            assert_eq!(indented_lines(actual), indented_lines(expected), "{rule}");
            let document = roxmltree::Document::parse(input).unwrap();
            let books: Vec<_> = document
                .root_element()
                .children()
                .filter(roxmltree::Node::is_element)
                .collect();
            let mut just_the_cuts = input.to_owned();
            for &index in removed.iter().rev() {
                just_the_cuts.replace_range(books[index].range(), "");
            }
            assert_eq!(actual, just_the_cuts, "{rule}");
        }
    }
}

fn indented_lines(xml: &str) -> Vec<&str> {
    xml.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

#[test]
fn xmlprune_requires_escaped_delimiters_and_reference_qualifiers() {
    let accepted = [
        r#"||example.org^$xmlprune=//a[@k='cost\$\, retail'],domain=page.org,third-party,app=com.reader,important,match-case,xmlhttprequest"#,
        "||example.org^$~third-party,xmlprune=//a",
        "@@||example.org^$xmlprune",
        r"||example.org^$xmlprune=//a[contains(.\,'#%#')]",
        r"@@||example.org^$xmlprune=//a[contains(.\,'ad')]",
    ];
    let rejected = [
        r"||example.org^$xmlprune=//a[@k='cost$']",
        r"||example.org^$app=com.reader,xmlprune=//a[@k='cost$']",
        r"||example.org^$xmlprune=//a[@k='cost, retail']",
        r"||example.org^$xmlprune=//a[\$v]",
        "||example.org^$xmlprune",
        "||example.org^$xmlprune=",
        "@@||example.org^$xmlprune=",
        "||example.org^$xmlprune=//a,strict-third-party",
        "||example.org^$xmlprune=//a,strict-first-party",
        "||example.org^$xmlprune=//a,script",
        "||example.org^$xmlprune=//a,all",
        "||example.org^$xmlprune=//a,first-party",
        "||example.org^$xmlprune=//a,_",
        "||example.org^$xmlprune=//a,~xmlhttprequest",
    ];
    for line in accepted {
        assert!(crate::rulecheck::check(line).issues.is_empty(), "{line}");
    }
    for line in rejected {
        assert_eq!(crate::rulecheck::check(line).issues.len(), 1, "{line}");
    }
    let rule = r"||example.org^$xmlprune=//a[@k='cost\$\, retail']";
    assert_eq!(
        edited(
            &filters(rule),
            "https://example.org/",
            Page::NONE,
            "other",
            "<r><a k='cost$, retail'/><a k='keep'/></r>"
        ),
        "<r><a k='keep'/></r>"
    );
}

#[test]
fn xmlprune_applies_matching_rules_in_lexicographical_order() {
    let input = "<r><a>first</a><a>second</a><a>third</a></r>";
    let rules = "||example.org^$xmlprune=/r/a[2]\n||example.org^$xmlprune=/r/a[1]";
    assert_eq!(
        edited(
            &filters(rules),
            "https://example.org/",
            Page::NONE,
            "other",
            input
        ),
        "<r><a>second</a></r>",
    );

    let rules = "||example.org^$xmlprune=/r/a[1]\n||example.org/path$xmlprune=/r/a[1]";
    assert_eq!(
        edited(
            &filters(rules),
            "https://example.org/path",
            Page::NONE,
            "other",
            input
        ),
        "<r><a>third</a></r>",
    );
}

#[test]
fn xmlprune_exceptions_are_kind_and_value_specific() {
    let rules = r"||example.org^$xmlprune=//a
||example.org^$xmlprune=//b
@@||example.org/basic
@@||example.org/value$xmlprune=//a
@@||example.org/other-value$xmlprune=//c
@@||example.org/all$xmlprune
@@||example.org/other-kind$jsonprune
@@||document.org^$document
@@||content.org^$content
@@||urlblock.org^$urlblock";
    let filters = filters(rules);
    let input = "<r><a/><b/></r>";
    for (path, expected) in [
        ("basic", "<r></r>"),
        ("other-value", "<r></r>"),
        ("value", "<r><a/></r>"),
        ("all", input),
        ("other-kind", "<r></r>"),
    ] {
        let url = format!("https://example.org/{path}");
        assert_eq!(
            edited(&filters, &url, Page::NONE, "other", input),
            expected,
            "{path}"
        );
    }
    for exception in ["document.org", "content.org", "urlblock.org"] {
        let page = parse_http_page(&format!("https://{exception}/")).unwrap();
        assert_eq!(
            edited(
                &filters,
                "https://example.org/",
                Page::Named(&page),
                "other",
                input
            ),
            input
        );
    }
}

#[test]
fn trusted_xmlprune_rules_and_exceptions_survive_a_cache_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let lists = temp.path().join("lists");
    let rules = temp.path().join("rules");
    for dir in [&lists, &rules] {
        std::fs::create_dir(dir).unwrap();
    }
    let catalog = "https://filters.adtidy.org/android/filters/11.txt";
    for (path, host) in [
        (lists.join("custom.txt"), "custom.org"),
        (
            lists.join(untrusted_file_name(catalog).unwrap()),
            "untrusted.org",
        ),
        (lists.join(file_name(catalog)), "stock.org"),
        (rules.join("user.txt"), "user.org"),
    ] {
        std::fs::write(
            path,
            format!("||{host}^$xmlprune=//a\n||{host}^$xmlprune=//b\n@@||{host}/off$xmlprune\n@@||{host}/part$xmlprune=//a"),
        ).unwrap();
    }
    let cache = temp.path().join("cache");
    let built = Filters::load_cached(&[&lists, &rules], &cache).unwrap();
    let cached = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
    let input = "<r><a/><b/></r>";
    for f in [built, cached] {
        for (host, base, part) in [
            ("custom.org", input, input),
            ("untrusted.org", input, input),
            ("stock.org", "<r></r>", "<r><a/></r>"),
            ("user.org", "<r></r>", "<r><a/></r>"),
        ] {
            for (path, expected) in [("", base), ("part", part), ("off", input)] {
                let url = format!("https://{host}/{path}");
                assert_eq!(
                    edited(&f, &url, Page::NONE, "other", input),
                    expected,
                    "{url}"
                );
            }
        }
    }
}
