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

/// Quote braces that are literal in list patterns. Braced escapes,
/// character classes and counted repetitions keep their own syntax.
pub(crate) fn translate_pattern(pattern: &str) -> Option<String> {
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
