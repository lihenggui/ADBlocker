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

use regex_automata::meta::Regex;
use regex_automata::util::{iter::Searcher, syntax};
use regex_automata::{Input, MatchKind};
use serde::{Deserialize, Serialize};

use crate::limits::{MAX_RESPONSE_REGEX, MAX_RESPONSE_RULE, MAX_REWRITE_RESULT};
use crate::rule::quoting::{delimiter, unescape, unescaped};
use crate::rule::regex::translate_pattern;

pub(crate) struct Compiled {
    regex: Regex,
    parts: Vec<Replacement>,
}

enum Replacement {
    Text(Vec<u8>),
    Group(usize),
}

pub(super) fn disables(value: Option<&str>) -> bool {
    matches!(value, None | Some(""))
}

pub(super) fn overrides_block() -> bool {
    true
}

/// Find the start of the flags after both slash-delimited fields.
fn value_fields(value: &str) -> Option<(usize, usize)> {
    let first = delimiter(value)?;
    let end = first + 2 + delimiter(&value[first + 1..])?;
    Some((first, end))
}

pub(crate) fn value_end(value: &str) -> Option<usize> {
    value_fields(value).map(|(_, end)| end)
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) struct Parsed {
    pub(super) pattern: String,
    pub(super) replacement: String,
    pub(super) insensitive: bool,
    pub(super) single_line: bool,
}

/// Decode a replacement after checking its quoting and optional flags.
pub(super) fn parse(value: &str) -> Option<Parsed> {
    parse_substitution(value, b"is", &['/']).map(|(parsed, _)| parsed)
}

pub(super) fn parse_substitution(
    value: &str,
    allowed: &[u8],
    replacement_escapes: &[char],
) -> Option<(Parsed, bool)> {
    if unescaped(value, b'$').next().is_some() {
        return None;
    }
    let value = unescape(value, &[',', '$']);
    let value = value.strip_prefix('/')?;
    let (first, end) = value_fields(value)?;
    let flags = &value[end..];
    flags.bytes().all(|b| allowed.contains(&b)).then(|| {
        (
            Parsed {
                pattern: value[..first].to_owned(),
                replacement: unescape(&value[first + 1..end - 1], replacement_escapes),
                insensitive: flags.contains('i'),
                single_line: flags.contains('s'),
            },
            flags.contains('g'),
        )
    })
}

pub(super) fn compile(
    pattern: &str,
    replacement: &str,
    insensitive: bool,
    single_line: bool,
) -> Option<Compiled> {
    if pattern.len().saturating_add(replacement.len()) > MAX_RESPONSE_RULE {
        return None;
    }
    let regex = Regex::builder()
        .configure(
            Regex::config()
                .nfa_size_limit(Some(MAX_RESPONSE_REGEX))
                .match_kind(MatchKind::LeftmostFirst)
                .utf8_empty(false)
                .hybrid_cache_capacity(2 << 20),
        )
        .syntax(
            syntax::Config::new()
                .utf8(false)
                .case_insensitive(insensitive)
                .dot_matches_new_line(single_line),
        )
        .build(&translate_pattern(pattern)?)
        .ok()?;
    Some(Compiled {
        regex,
        parts: replacement_parts(replacement),
    })
}

impl Compiled {
    pub(super) fn regex_bytes(&self) -> usize {
        self.regex.memory_usage()
    }

    pub(super) fn apply(&self, input: &[u8], within_budget: &dyn Fn() -> bool) -> Option<Vec<u8>> {
        self.substitute(input, true, MAX_REWRITE_RESULT, within_budget)
    }

    pub(super) fn substitute(
        &self,
        input: &[u8],
        global: bool,
        limit: usize,
        within_budget: &dyn Fn() -> bool,
    ) -> Option<Vec<u8>> {
        let regex = &self.regex;
        let mut output = Vec::new();
        let mut end = 0;
        let mut cache = regex.create_cache();
        let mut matches = Searcher::new(Input::new(input)).into_captures_iter(
            regex.create_captures(),
            |input, captures| {
                regex.search_captures_with(&mut cache, input, captures);
                Ok(())
            },
        );
        while within_budget() {
            let Some(groups) = matches.next() else {
                append(&mut output, &input[end..], limit)?;
                return Some(output);
            };
            let groups = groups.ok()?;
            let matched = groups.get_match()?;
            append(&mut output, &input[end..matched.start()], limit)?;
            for part in &self.parts {
                match part {
                    Replacement::Text(text) => append(&mut output, text, limit)?,
                    Replacement::Group(n) => {
                        if let Some(found) = groups.get_group(*n) {
                            append(&mut output, &input[found.range()], limit)?;
                        }
                    }
                }
            }
            end = matched.end();
            if !global {
                append(&mut output, &input[end..], limit)?;
                return Some(output);
            }
        }
        None
    }
}

/// Only single-digit references from `$1` to `$9` interpolate captures.
fn replacement_parts(replacement: &str) -> Vec<Replacement> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (index, pair) in replacement.as_bytes().windows(2).enumerate() {
        if let [b'$', digit @ b'1'..=b'9'] = pair {
            if start < index {
                parts.push(Replacement::Text(
                    replacement.as_bytes()[start..index].to_vec(),
                ));
            }
            parts.push(Replacement::Group(usize::from(digit - b'0')));
            start = index + 2;
        }
    }
    if start < replacement.len() {
        parts.push(Replacement::Text(replacement.as_bytes()[start..].to_vec()));
    }
    parts
}

/// Refuse growth before allocating or writing any bytes past the result limit.
fn append(output: &mut Vec<u8>, bytes: &[u8], limit: usize) -> Option<()> {
    if bytes.len() > limit - output.len() {
        return None;
    }
    output.extend_from_slice(bytes);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replacement_decodes_escapes_and_compiles_and_applies_captures() {
        let Some(Parsed {
            pattern,
            replacement,
            insensitive,
            single_line,
        }) = parse(r"/(a\,b)/\$1\//i")
        else {
            panic!("expected a replacement");
        };
        let compiled = compile(&pattern, &replacement, insensitive, single_line).unwrap();
        assert_eq!(
            compiled.apply(b"A,B", &|| true).as_deref(),
            Some(b"A,B/".as_slice())
        );
        assert!(compiled.apply(b"A,B", &|| false).is_none());
        assert!(parse(r"/a/$1/").is_none());
        assert!(compile("(", "", false, false).is_none());
    }

    #[test]
    fn an_unvalued_or_empty_replace_exception_disables_replacements() {
        assert!(disables(None));
        assert!(disables(Some("")));
        assert!(!disables(Some("/a/b/")));
        assert!(overrides_block());
    }

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
                translate_pattern(pattern).as_deref(),
                Some(expected),
                "{pattern}"
            );
        }
    }
}
