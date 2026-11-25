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

//! Translate and compile list expressions used by the engine's body and header rules.

use regex_automata::Input;
use regex_automata::meta::{Cache, Regex};
use regex_automata::util::syntax;

use crate::limits::{
    MAX_HTML_REGEX, MAX_REGEX_FALLBACK_REPEAT, MAX_REGEX_FALLBACK_SCRATCH, MAX_RESPONSE_REGEX_CACHE,
};

#[derive(Clone, Copy)]
pub(super) enum RegexInput {
    Text,
    Bytes,
    Cookie,
}

pub(super) enum CompileError {
    Unsupported,
    LookAround,
    Limit,
}

pub(super) fn compile(
    pattern: &str,
    input: RegexInput,
    insensitive: bool,
    single_line: bool,
    limit: usize,
) -> Result<Regex, CompileError> {
    let pattern = normalize_pattern(pattern, matches!(input, RegexInput::Cookie), insensitive)
        .ok_or(CompileError::Unsupported)?;
    compile_normalized(&pattern, input, insensitive, single_line, limit)
}

pub(super) fn compile_normalized(
    pattern: &str,
    input: RegexInput,
    insensitive: bool,
    single_line: bool,
    limit: usize,
) -> Result<Regex, CompileError> {
    let text = matches!(input, RegexInput::Text);
    let cache_limit = if limit == MAX_HTML_REGEX {
        limit
    } else {
        MAX_RESPONSE_REGEX_CACHE
    };
    let syntax = syntax::Config::new()
        .utf8(text)
        .case_insensitive(insensitive)
        .dot_matches_new_line(single_line);
    let regex = Regex::builder()
        .configure(
            Regex::config()
                .nfa_size_limit(Some(limit))
                .utf8_empty(!matches!(input, RegexInput::Cookie))
                .hybrid_cache_capacity(cache_limit),
        )
        .syntax(syntax)
        .build(pattern)
        .map_err(|err| {
            if err.size_limit().is_some() {
                CompileError::Limit
            } else if err
                .syntax_error()
                .is_some_and(|err| err.to_string().contains("look-around"))
            {
                CompileError::LookAround
            } else {
                CompileError::Unsupported
            }
        })?;
    if (limit == MAX_HTML_REGEX && regex.memory_usage() > limit)
        || (regex.create_cache().memory_usage() > MAX_REGEX_FALLBACK_SCRATCH
            && long_repetition(pattern, &syntax))
    {
        return Err(CompileError::Limit);
    }
    Ok(regex)
}

/// An expanded repetition can churn through the lazy DFA's cache and
/// trigger NFA fallback. Large scratch space alone also occurs with
/// short Unicode repetitions, so look for a long repeated expression.
fn long_repetition(pattern: &str, syntax: &syntax::Config) -> bool {
    let Ok(hir) = syntax::parse_with(pattern, syntax) else {
        return true;
    };
    let mut pending = vec![&hir];
    while let Some(part) = pending.pop() {
        let subs = part.kind().subs();
        if let [inner] = subs
            && part
                .properties()
                .minimum_len()
                .is_some_and(|len| len > MAX_REGEX_FALLBACK_REPEAT)
            && inner.properties().minimum_len() != part.properties().minimum_len()
        {
            return true;
        }
        pending.extend(subs);
    }
    false
}

/// The caller keeps each cache for one evaluation of its compiled expression.
pub(crate) fn matches_with(regex: &Regex, cache: &mut Cache, text: &[u8]) -> bool {
    regex
        .search_half_with(cache, &Input::new(text).earliest(true))
        .is_some()
}

/// Retain a bounded amount of scratch across all expressions in one pass.
const MAX_RETAINED_SCRATCH: usize = 8 << 20;

#[derive(Default)]
pub(crate) struct SearchBudget {
    retained_bytes: usize,
}

pub(crate) type MatchCache = Option<(Box<Cache>, usize)>;

impl SearchBudget {
    pub(crate) fn matches(&mut self, regex: &Regex, slot: &mut MatchCache, text: &[u8]) -> bool {
        let mut cache = match slot.take() {
            Some((cache, size)) => {
                self.retained_bytes -= size;
                cache
            }
            None => Box::new(regex.create_cache()),
        };
        let matched = matches_with(regex, &mut cache, text);
        let size = cache.memory_usage();
        if self.retained_bytes + size <= MAX_RETAINED_SCRATCH {
            self.retained_bytes += size;
            *slot = Some((cache, size));
        }
        matched
    }
}

#[cfg(test)]
pub(crate) fn matches(regex: &Regex, text: &[u8]) -> bool {
    matches_with(regex, &mut regex.create_cache(), text)
}

pub(crate) fn normalize_pattern(
    pattern: &str,
    byte_dot: bool,
    insensitive: bool,
) -> Option<String> {
    let mut translated = String::with_capacity(pattern.len());
    let mut chars = pattern.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        let tail = &pattern[index + ch.len_utf8()..];
        let end = match ch {
            '\\' => {
                let (at, escaped) = chars.next()?;
                let rest = escape(
                    &mut translated,
                    escaped,
                    false,
                    &pattern[at + escaped.len_utf8()..],
                )?;
                pattern.len() - rest.len()
            }
            '[' => {
                pattern.len() - character_class(&mut translated, tail, byte_dot, insensitive)?.len()
            }
            ']' | '}' => {
                translated.push('\\');
                translated.push(ch);
                continue;
            }
            '.' if byte_dot => {
                translated.push_str("(?-u:.)");
                continue;
            }
            '{' => {
                let Some(count) = repeat_count(tail) else {
                    translated.push_str(r"\{");
                    continue;
                };
                let end = index + 1 + count.len();
                if pattern.as_bytes().get(end) == Some(&b'+') {
                    return None;
                }
                translated.push_str(&pattern[index..end]);
                end
            }
            '*' | '+' | '?' if tail.starts_with('+') => return None,
            _ => {
                translated.push(ch);
                continue;
            }
        };
        while chars.next_if(|(at, _)| *at < end).is_some() {}
    }
    Some(translated)
}

fn character_class<'a>(
    out: &mut String,
    tail: &'a str,
    byte_dot: bool,
    insensitive: bool,
) -> Option<&'a str> {
    if let Some(rest) = tail.strip_prefix(']') {
        out.push_str(r"[^\s\S]");
        return Some(rest);
    }
    if let Some(rest) = tail.strip_prefix("^]") {
        out.push_str(if byte_dot { "(?-u:(?s:.))" } else { "(?s:.)" });
        return Some(rest);
    }
    let (negated, mut rest) = tail
        .strip_prefix('^')
        .map_or((false, tail), |rest| (true, rest));
    let mut class = String::from("[");
    let (mut word, mut non_word) = (false, false);
    loop {
        let ch = rest.chars().next()?;
        rest = &rest[ch.len_utf8()..];
        match ch {
            ']' => break,
            '\\' => {
                let escaped = rest.chars().next()?;
                rest = &rest[escaped.len_utf8()..];
                if insensitive && matches!(escaped, 'w' | 'W') {
                    word |= escaped == 'w';
                    non_word |= escaped == 'W';
                    // Keep a set operand so a range using it remains unsupported.
                    class.push_str(r"[^\s\S]");
                } else {
                    rest = escape(&mut class, escaped, true, rest)?;
                }
            }
            '[' | '&' | '~' => {
                class.push('\\');
                class.push(ch);
            }
            _ => class.push(ch),
        }
    }
    class.push(']');
    if word || non_word {
        // Fold the other operands first. The word sets themselves must not
        // gain Unicode case equivalents of ASCII letters under /i.
        let folded =
            syntax::parse_with(&class, &syntax::Config::new().case_insensitive(true)).ok()?;
        out.push_str("(?-i:[");
        if negated {
            out.push('^');
        }
        out.push_str(&folded.to_string());
        if word {
            out.push_str("A-Za-z0-9_");
        }
        if non_word {
            out.push_str("[^A-Za-z0-9_]");
        }
        out.push_str("])");
    } else {
        if negated {
            class.insert(1, '^');
        }
        out.push_str(&class);
    }
    Some(rest)
}

fn escape<'a>(out: &mut String, ch: char, in_class: bool, tail: &'a str) -> Option<&'a str> {
    // ECMAScript §22.2.1 CharacterEscape, CharacterClassEscape and
    // IdentityEscape, including Annex B's non-Unicode IdentityEscape.
    // Keep explicit braced PCRE properties and hex escapes supported by
    // the engine. Backreferences and legacy octal escapes remain unsupported.
    let class = match ch.to_ascii_lowercase() {
        'w' => Some("A-Za-z0-9_"),
        'd' => Some("0-9"),
        's' => Some(
            r"\x09-\x0d\x20\u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}",
        ),
        _ => None,
    };
    if let Some(class) = class {
        // Turning off Unicode case folding keeps i from adding Kelvin
        // sign and long s to ASCII word classes. An insensitive bracket
        // class handles those operands in character_class.
        let ascii_word = matches!(ch, 'w' | 'W') && !in_class;
        if ascii_word {
            out.push_str("(?-i:");
        }
        out.push('[');
        if ch.is_ascii_uppercase() {
            out.push('^');
        }
        out.push_str(class);
        out.push(']');
        if ascii_word {
            out.push(')');
        }
    } else if !in_class && matches!(ch, 'b' | 'B') {
        out.push_str(if ch == 'b' { r"(?-u:\b)" } else { r"(?-u:\B)" });
    } else if in_class && ch == 'b' {
        out.push_str(r"\x08");
    } else if ch == 'v' {
        out.push_str(r"\x0b");
    } else if let Some(braced) = braced_escape(ch, tail) {
        out.push('\\');
        out.push(ch);
        out.push_str(braced);
        return Some(&tail[braced.len()..]);
    } else if ch == 'c' && tail.starts_with(|c: char| c.is_ascii_alphabetic()) {
        use std::fmt::Write;
        write!(out, r"\x{:02x}", tail.as_bytes()[0] % 32).expect("writing to a string");
        return Some(&tail[1..]);
    } else if ch == '0' && !tail.starts_with(|c: char| c.is_ascii_digit()) {
        out.push_str(r"\x00");
    } else if ch.is_ascii_digit() {
        return None;
    } else if matches!(ch, 'f' | 'n' | 'r' | 't') || hex_escape(ch, tail) {
        out.push('\\');
        out.push(ch);
    } else {
        if regex::escape(&ch.to_string()).starts_with('\\')
            || (in_class && matches!(ch, '-' | '&' | '~'))
        {
            out.push('\\');
        }
        out.push(ch);
    }
    Some(tail)
}

fn hex_escape(ch: char, tail: &str) -> bool {
    let digits = match ch {
        'x' => 2,
        'u' => 4,
        _ => return false,
    };
    tail.as_bytes()
        .get(..digits)
        .is_some_and(|bytes| bytes.iter().all(u8::is_ascii_hexdigit))
}

fn braced_escape(escaped: char, tail: &str) -> Option<&str> {
    if !matches!(escaped, 'p' | 'P' | 'x' | 'u') {
        return None;
    }
    let (value, _) = tail.strip_prefix('{')?.split_once('}')?;
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

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use super::*;

    #[test]
    #[ignore = "a measurement, run by hand"]
    fn measures_reusing_a_list_regex_cache() {
        let regex = compile(
            r"(?:[a-z]+-[0-9]+)+-tracker",
            RegexInput::Text,
            false,
            false,
            MAX_HTML_REGEX,
        )
        .ok()
        .unwrap();
        const MATCHES: usize = 100_000;
        let measure = |search: &mut dyn FnMut(&[u8]) -> bool| {
            let start = Instant::now();
            let count = (0..MATCHES)
                .map(|index| {
                    let text: &[u8] = if index % 2 == 0 {
                        b"source-012-tracker"
                    } else {
                        b"source-012-content"
                    };
                    search(black_box(text))
                })
                .filter(|&found| found)
                .count();
            assert_eq!(count, MATCHES / 2);
            start.elapsed().as_nanos() as f64 / MATCHES as f64
        };
        let fresh = measure(&mut |text| matches(&regex, text));
        let (mut budget, mut cache) = (SearchBudget::default(), None);
        let reused = measure(&mut |text| budget.matches(&regex, &mut cache, text));
        println!(
            "list regex: {fresh:.1} ns/match with fresh caches, {reused:.1} ns/match with reuse"
        );
    }
}
