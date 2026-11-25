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

//! Translate the list regex dialect before compiling a response expression.

use std::borrow::Cow;

use regex::bytes::RegexBuilder;

/// Full URL regexes are compiled lazily by adblock-rust. Check them before
/// enabling a rule that was previously rejected for its noop option.
pub(super) fn network_pattern_supported(pattern: &str, match_case: bool) -> bool {
    let pattern = pattern.strip_prefix("@@").unwrap_or(pattern);
    let pattern = pattern
        .strip_prefix("||")
        .or_else(|| pattern.strip_prefix('|'))
        .unwrap_or(pattern);
    let pattern = pattern.strip_suffix('|').unwrap_or(pattern);
    let Some(body) = pattern
        .strip_prefix('/')
        .and_then(|pattern| pattern.strip_suffix('/'))
    else {
        return true;
    };
    // Match the matcher’s full-regex syntax and its default size limit.
    let mut body = body.replace(r"\/", "/").replace(r"\:", ":");
    if !match_case {
        body.make_ascii_lowercase();
    }
    RegexBuilder::new(&body).unicode(false).build().is_ok()
}

/// Translate and validate with the consuming regex engine and its limits.
pub(super) fn compile_pattern<T, E>(
    pattern: &str,
    rejection: E,
    compile: impl FnOnce(&str) -> Result<T, E>,
) -> Result<T, E> {
    let translated = normalize_pattern(pattern).ok_or(rejection)?;
    compile(&translated)
}

/// Quote braces that are literal in list patterns. Braced escapes,
/// character classes and counted repetitions keep their own syntax.
pub(super) fn normalize_pattern(pattern: &str) -> Option<String> {
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

/// The condition matcher only accepts `$match-case` on a full regex.
pub(super) fn case_sensitive_pattern(pattern: &str) -> Cow<'_, str> {
    if pattern.len() > 1 && pattern.starts_with('/') && pattern.ends_with('/') {
        return Cow::Borrowed(pattern);
    }
    let mut regex = String::from("/");
    let pattern = if let Some(host_pattern) = pattern.strip_prefix("||") {
        regex.push_str(r"^https?://(?:[^./:?#]+\.)*(?i:");
        let end = host_pattern
            .find(['/', '^', ':', '|'])
            .unwrap_or(host_pattern.len());
        let (host, tail) = host_pattern.split_at(end);
        write_pattern(&mut regex, host, r"[^/:?#]*");
        regex.push(')');
        tail
    } else if let Some(pattern) = pattern.strip_prefix('|') {
        regex.push('^');
        pattern
    } else {
        pattern
    };
    let tail = pattern.strip_suffix('|');
    write_pattern(&mut regex, tail.unwrap_or(pattern), ".*");
    if tail.is_some() {
        regex.push('$');
    }
    regex.push('/');
    Cow::Owned(regex)
}

fn write_pattern(regex: &mut String, pattern: &str, wildcard: &str) {
    let mut parts = pattern.split_inclusive(['*', '^']).peekable();
    while let Some(part) = parts.next() {
        if let Some(text) = part.strip_suffix('*') {
            regex.push_str(&regex::escape(text));
            regex.push_str(wildcard);
        } else if let Some(text) = part.strip_suffix('^') {
            regex.push_str(&regex::escape(text));
            regex.push_str(if parts.peek().is_none() {
                r"(?:[^\w.%-]|$)"
            } else {
                r"[^\w.%-]"
            });
        } else {
            regex.push_str(&regex::escape(part));
        }
    }
}

#[cfg(test)]
mod tests;
