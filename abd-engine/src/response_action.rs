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

//! A body action describes one change to a response. Its specification
//! can be shared by several conditions and kept in the compiled cache.
//! Each kind of action owns its compilation and execution, so the
//! matcher and the body collector can serve other action kinds.

use regex_automata::meta::Regex;
use regex_automata::util::{iter::Searcher, syntax};
use regex_automata::{Input, MatchKind};
use serde::{Deserialize, Serialize};

use crate::limits::{MAX_RESPONSE_REGEX, MAX_RESPONSE_RULE, MAX_REWRITE_RESULT};

/// The cache representation and identity of an action.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ActionSpec {
    Replace {
        pattern: String,
        replacement: String,
        insensitive: bool,
        single_line: bool,
    },
}

/// A compiled action that can safely be shared among concurrent jobs.
pub(crate) enum ResponseAction {
    Replace {
        regex: Regex,
        parts: Vec<Replacement>,
    },
}

/// Pieces of the replacement that distinguish literal text from capture references.
pub(crate) enum Replacement {
    Text(Vec<u8>),
    Group(usize),
}

impl ActionSpec {
    /// Reject a pattern that cannot be searched within the per-expression limit.
    pub(crate) fn compile(&self) -> Option<ResponseAction> {
        match self {
            Self::Replace {
                pattern,
                replacement,
                insensitive,
                single_line,
            } => {
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
                            .case_insensitive(*insensitive)
                            .dot_matches_new_line(*single_line),
                    )
                    .build(&translate_pattern(pattern)?)
                    .ok()?;
                Some(ResponseAction::Replace {
                    regex,
                    parts: replacement_parts(replacement),
                })
            }
        }
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

impl ResponseAction {
    /// Heap used by the compiled matcher. Search scratch space belongs to
    /// the blocking job, so an idle action keeps none of it.
    pub(crate) fn regex_bytes(&self) -> usize {
        match self {
            Self::Replace { regex, .. } => regex.memory_usage(),
        }
    }

    /// `None` abandons the whole rewrite on overflow, timeout or cancellation.
    pub(crate) fn apply(&self, input: &[u8], within_budget: &dyn Fn() -> bool) -> Option<Vec<u8>> {
        if !within_budget() {
            return None;
        }
        match self {
            Self::Replace { regex, parts } => {
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
                    for part in parts {
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
    }
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
    use super::translate_pattern;

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
