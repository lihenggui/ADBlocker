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

use crate::list_regex::normalize_pattern;

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
            normalize_pattern(pattern, false, false).as_deref(),
            Some(expected),
            "{pattern}"
        );
    }
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn patterns_with_no_rewritten_constructs_are_preserved(
            literal in "[a-zA-Z0-9_ -]{0,40}",
            fragment in prop::sample::select(vec![
                "", "(?:x|y)+", "[{}]", "a{1,4}?", r"\{2\}", r"\p{Greek}", r"\x{61}",
            ]),
        ) {
            let pattern = format!("^{fragment}{literal}$");
            let translated = normalize_pattern(&pattern, false, false);
            prop_assert_eq!(translated.as_deref(), Some(pattern.as_str()));
        }
    }
}

#[test]
fn list_escapes_and_classes_keep_their_javascript_meaning() {
    for (pattern, matches, misses) in [
        (
            r"\<script\>[\s\S]*?\<\/script\>",
            "<script>\nx</script>",
            "<script>x",
        ),
        (r"\q\_\<\>", "q_<>", "q_a"),
        (r"^[^]]$", "\n]", "a"),
        (r"^[]$", "", "a"),
        (
            r"^\w{20,40}=\w{20,40}$",
            "abcdefghijklmnopqrst=abcdefghijklmnopqrst",
            "ébcdefghijklmnopqrst=abcdefghijklmnopqrst",
        ),
        (r"^[\w\d]+$", "a_1", "é١"),
        (r"^[\W\D]+$", "é١", "1"),
        (r"^\bword\b$", "word", "Sword"),
        (r"^é\bword\bé\Bé$", "éwordéé", "éwordéz"),
        (r"^\s$", "\u{feff}", "\u{85}"),
        (r"^[\S]$", "\u{85}", "\u{feff}"),
    ] {
        let translated = normalize_pattern(pattern, false, false).unwrap();
        let text = regex::Regex::new(&translated).unwrap();
        let bytes = regex::bytes::Regex::new(&translated).unwrap();
        if pattern != r"^[]$" {
            assert!(text.is_match(matches), "{pattern} on {matches:?}");
            assert!(
                bytes.is_match(matches.as_bytes()),
                "{pattern} on {matches:?}"
            );
        } else {
            assert!(!text.is_match(matches));
            assert!(!bytes.is_match(matches.as_bytes()));
        }
        assert!(!text.is_match(misses), "{pattern} on {misses:?}");
        assert!(
            !bytes.is_match(misses.as_bytes()),
            "{pattern} on {misses:?}"
        );
    }
}

#[test]
fn case_insensitive_word_classes_keep_non_ascii_case_equivalents_outside_the_set() {
    use crate::limits::MAX_RESPONSE_REGEX;
    use crate::list_regex::{RegexInput, compile, matches};
    for (pattern, matching, missed) in [
        (r"^[\w-]+$", "asK-_9", "aſK"),
        (r"^[^\w]+$", "ſKé", "_"),
        (r"^[\Wq]+$", "ſKqQ", "s"),
    ] {
        let regex = compile(pattern, RegexInput::Text, true, false, MAX_RESPONSE_REGEX)
            .ok()
            .unwrap();
        assert!(matches(&regex, matching.as_bytes()), "{pattern}");
        assert!(!matches(&regex, missed.as_bytes()), "{pattern}");
    }
}
