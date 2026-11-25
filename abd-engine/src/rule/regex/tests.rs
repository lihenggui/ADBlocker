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

#[test]
fn literal_braces_are_quoted_and_repeat_counts_keep_their_meaning() {
    for (pattern, expected) in [
        ("a{b", r"a\{b"),
        (r"\(\){return", r"\(\)\{return"),
        ("({adv:).(,q:!0)", r"(\{adv:).(,q:!0)"),
        (".{280,400}.*", ".{280,400}.*"),
        (
            r#"(^.{1,150}\(\{).*?"bunch".*(}\);)"#,
            r#"(^.{1,150}\(\{).*?"bunch".*(\}\);)"#,
        ),
        (r"[{}][\{\}][{]", r"[{}][\{\}][{]"),
        (r"\{2\}\\{2}", r"\{2\}\\{2}"),
        (r"(a){2}[b]{3,}[c]{4,5}", r"(a){2}[b]{3,}[c]{4,5}"),
        (r"\p{Greek}{2}\x{7D}\u{7B}", r"\p{Greek}{2}\x{7D}\u{7B}"),
        ("{a{2}}", r"\{a{2}\}"),
        ("{x{y}z", r"\{x\{y\}z"),
        ("a{2,3", r"a\{2,3"),
        ("a{2,x}", r"a\{2,x\}"),
        ("x}y{", r"x\}y\{"),
    ] {
        assert_eq!(
            normalize_pattern(pattern).as_deref(),
            Some(expected),
            "{pattern}"
        );
    }
}

mod properties {
    use proptest::prelude::*;
    use regex::RegexBuilder;

    use super::*;
    use crate::limits::MAX_RESPONSE_REGEX;
    use crate::test_support::arbitrary_text;

    fn pattern() -> impl Strategy<Value = String> {
        let fragment = prop::sample::select(vec![
            "^ad",
            ".*",
            "(?:x|y)",
            "[a-z]",
            r"\{",
            "{return",
            "}",
            "a{1,4}",
            "[{}]",
            r"\p{Greek}",
            r"\w{999999}",
            r"\b{start}",
            "a++",
            "(?=x)",
            "(",
            "[",
            r"\",
        ]);
        prop_oneof![
            arbitrary_text(),
            prop::collection::vec(fragment, 0..8).prop_map(|pieces| pieces.concat()),
        ]
    }

    proptest! {
        #[test]
        fn translating_and_compiling_returns_only_patterns_accepted_at_the_size_limit(
            pattern in pattern(),
        ) {
            let result = compile_pattern(&pattern, (), |translated| {
                RegexBuilder::new(translated)
                    .size_limit(MAX_RESPONSE_REGEX)
                    .build()
                    .map_err(|_| ())
            });
            if let Ok(compiled) = result {
                prop_assert!(RegexBuilder::new(compiled.as_str())
                    .size_limit(MAX_RESPONSE_REGEX).build().is_ok());
            }
        }

        #[test]
        fn patterns_with_no_rewritten_constructs_are_preserved(
            literal in "[a-zA-Z0-9_ -]{0,40}",
            fragment in prop::sample::select(vec![
                "", "(?:x|y)+", "[{}]", "a{1,4}?", r"\{2\}", r"\p{Greek}", r"\x{61}",
            ]),
        ) {
            let pattern = format!("^{fragment}{literal}$");
            let translated = normalize_pattern(&pattern);
            prop_assert_eq!(translated.as_deref(), Some(pattern.as_str()));
        }
    }
}
