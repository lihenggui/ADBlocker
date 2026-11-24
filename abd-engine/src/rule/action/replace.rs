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

use crate::limits::{MAX_RESPONSE_REGEX, MAX_RESPONSE_RULE, MAX_REWRITE_RESULT};
use crate::rule::options::{delimiter, unescape, unescaped};

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

#[cfg(test)]
pub(crate) fn value_end(value: &str) -> Option<usize> {
    value_fields(value).map(|(_, end)| end)
}

pub(super) struct Parsed {
    pub(super) pattern: String,
    pub(super) replacement: String,
    pub(super) insensitive: bool,
    pub(super) single_line: bool,
}

/// Decode a replacement after checking its quoting and optional flags.
pub(super) fn parse(value: &str) -> Option<Parsed> {
    if unescaped(value, b'$').next().is_some() {
        return None;
    }
    let value = unescape(value, &[',', '$']);
    let value = value.strip_prefix('/')?;
    let (first, end) = value_fields(value)?;
    let flags = &value[end..];
    flags
        .bytes()
        .all(|b| matches!(b, b'i' | b's'))
        .then(|| Parsed {
            pattern: value[..first].to_owned(),
            replacement: unescape(&value[first + 1..end - 1], &['/']),
            insensitive: flags.contains('i'),
            single_line: flags.contains('s'),
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
                append(&mut output, &input[end..])?;
                return Some(output);
            };
            let groups = groups.ok()?;
            let matched = groups.get_match()?;
            append(&mut output, &input[end..matched.start()])?;
            for part in &self.parts {
                match part {
                    Replacement::Text(text) => append(&mut output, text)?,
                    Replacement::Group(n) => {
                        if let Some(found) = groups.get_group(*n) {
                            append(&mut output, &input[found.range()])?;
                        }
                    }
                }
            }
            end = matched.end();
        }
        None
    }
}

/// Quote braces that are literal in list patterns. Braced escapes,
/// character classes and counted repetitions keep their own syntax.
fn translate_pattern(pattern: &str) -> Option<String> {
    let mut translated = String::with_capacity(pattern.len());
    let mut chars = pattern.char_indices().peekable();
    let mut class_depth = 0;
    while let Some((index, ch)) = chars.next() {
        match ch {
            '\\' => {
                let (at, escaped) = chars.next()?;
                let tail = &pattern[at + escaped.len_utf8()..];
                if let Some(braced) = braced_escape(escaped, tail) {
                    let end = at + escaped.len_utf8() + braced.len();
                    translated.push_str(&pattern[index..end]);
                    while chars.next_if(|(at, _)| *at < end).is_some() {}
                    continue;
                }
                translated.push(ch);
                translated.push(escaped);
                continue;
            }
            '[' => class_depth += 1,
            ']' if class_depth != 0 => class_depth -= 1,
            '{' if class_depth == 0 => {
                let tail = &pattern[index + 1..];
                if let Some(count) = repeat_count(tail) {
                    let end = index + 1 + count.len();
                    if pattern.as_bytes().get(end) == Some(&b'+') {
                        return None;
                    }
                    translated.push_str(&pattern[index..end]);
                    while chars.next_if(|(at, _)| *at < end).is_some() {}
                    continue;
                }
                translated.push('\\');
            }
            '}' if class_depth == 0 => translated.push('\\'),
            '*' | '+' | '?' if class_depth == 0 && chars.peek().is_some_and(|&(_, c)| c == '+') => {
                // The engine accepts nested repeats, which have different
                // semantics from a possessive quantifier.
                return None;
            }
            _ => (),
        }
        translated.push(ch);
    }
    Some(translated)
}

/// Preserve braces that belong to an escape accepted by the regex engine.
fn braced_escape(escaped: char, tail: &str) -> Option<&str> {
    if !matches!(escaped, 'p' | 'P' | 'x' | 'u' | 'U' | 'b') {
        return None;
    }
    let (value, _) = tail.strip_prefix('{')?.split_once('}')?;
    if escaped == 'b' && !matches!(value, "start" | "end" | "start-half" | "end-half") {
        return None;
    }
    Some(&tail[..value.len() + 2])
}

/// The part of `{n}`, `{n,}` or `{n,m}` after the opening brace.
fn repeat_count(tail: &str) -> Option<&str> {
    let after_lower = tail.trim_start_matches(|c: char| c.is_ascii_digit());
    if after_lower.len() == tail.len() {
        return None;
    }
    let end = after_lower.strip_prefix(',').map_or(after_lower, |upper| {
        upper.trim_start_matches(|c: char| c.is_ascii_digit())
    });
    end.starts_with('}')
        .then(|| &tail[..tail.len() - end.len() + 1])
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
fn append(output: &mut Vec<u8>, bytes: &[u8]) -> Option<()> {
    if bytes.len() > MAX_REWRITE_RESULT - output.len() {
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
