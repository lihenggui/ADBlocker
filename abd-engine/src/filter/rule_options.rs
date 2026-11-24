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

//! Option boundaries and quoting shared by app conditions and response rules.

use adblock::lists::{FilterParseError, ParseOptions, RuleTypes, parse_filter};

/// AdGuard quotes option separators with an immediately preceding backslash.
pub(super) fn unescaped(text: &str, byte: u8) -> impl DoubleEndedIterator<Item = usize> {
    text.bytes().enumerate().filter_map(move |(i, b)| {
        (b == byte && (i == 0 || text.as_bytes()[i - 1] != b'\\')).then_some(i)
    })
}

/// Split options after skipping the two slash-delimited replacement fields.
pub(super) fn options(text: &str) -> impl Iterator<Item = &str> {
    let mut remaining = Some(text);
    std::iter::from_fn(move || {
        let text = remaining?;
        let start = text.strip_prefix("replace=/").map_or(0, |value| {
            text.len() - value.len() + option_value_end(value)
        });
        let Some(comma) = unescaped(&text[start..], b',').next() else {
            remaining = None;
            return Some(text);
        };
        let end = start + comma;
        remaining = Some(&text[end + 1..]);
        Some(&text[..end])
    })
}

pub(super) fn network_condition(line: &str, pattern: &str) -> bool {
    let parse = ParseOptions {
        rule_types: RuleTypes::CosmeticOnly,
        ..ParseOptions::default()
    };
    !pattern.contains('#')
        || matches!(
            parse_filter(line, false, parse),
            Err(FilterParseError::Unsupported)
        )
}

/// A regular expression that forms the condition can contain dollar signs.
pub(super) fn regex_option_separator(line: &str) -> Option<usize> {
    let pattern = line.strip_prefix("@@").unwrap_or(line);
    let close = delimiter(pattern.strip_prefix('/')?)?;
    let after = line.len() - pattern.len() + close + 2;
    (line.as_bytes().get(after) == Some(&b'$')).then_some(after)
}

/// Find the last unquoted dollar outside a regular expression condition or
/// a replacement or app value. Both value parsers check dollar quoting
/// separately, so malformed quoting stays with the rule that contains it.
pub(super) fn option_separator(line: &str) -> Option<usize> {
    let mut cursor = regex_option_separator(line).unwrap_or(0);
    if cursor == 0 && line.ends_with('/') {
        let pattern = line.strip_prefix("@@").unwrap_or(line);
        if let Some(body) = pattern.strip_prefix('/')
            && delimiter(body).is_some_and(|end| end + 1 == body.len())
        {
            return None;
        }
    }
    let mut last = None;
    let mut app_options = false;
    let bytes = line.as_bytes();
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        cursor += 1;
        if cursor >= 2 && bytes[cursor - 2] == b'\\' {
            continue;
        }
        if byte == b'$' {
            if !app_options {
                last = Some(cursor - 1);
            }
        } else if byte != b',' || last.is_none() {
            continue;
        }
        let option = &line[cursor..];
        if option
            .trim_start_matches('~')
            .strip_prefix("app")
            .is_some_and(|tail| tail.is_empty() || tail.starts_with(['=', ',']))
        {
            app_options = true;
        }
        if let Some(value) = option.strip_prefix("replace=/") {
            cursor = line.len() - value.len() + option_value_end(value);
        } else if line[cursor..]
            .strip_prefix("jsonprune")
            .is_some_and(|tail| tail.is_empty() || tail.starts_with(['=', ',']))
        {
            // Keep malformed quoting in the response rule so it cannot
            // turn into a block by moving the option separator.
            return last;
        }
    }
    last
}

/// Skip both fields. uBlock accepts a raw comma within a field when
/// it cannot begin another option.
pub(super) fn option_value_end(value: &str) -> usize {
    let mut slashes = 0;
    let mut escaped = false;
    for (index, byte) in value.bytes().enumerate() {
        match byte {
            b',' if (index == 0 || value.as_bytes()[index - 1] != b'\\')
                && starts_option(&value[index + 1..]) =>
            {
                return index;
            }
            b'/' if !escaped => {
                slashes += 1;
                if slashes == 2 {
                    return index + 1;
                }
            }
            _ => (),
        }
        escaped = byte == b'\\' && !escaped;
    }
    value.len()
}

/// A raw comma can separate an option with a name, a value or a noop.
fn starts_option(tail: &str) -> bool {
    let name = tail.strip_prefix('~').unwrap_or(tail);
    let after =
        name.trim_start_matches(|c: char| c.is_ascii_lowercase() || matches!(c, '1' | '3' | '-'));
    (after.len() < name.len() && (after.is_empty() || after.starts_with(['=', ','])))
        || (tail.starts_with('_') && {
            let end = tail.trim_start_matches('_');
            end.is_empty() || end.starts_with(',')
        })
}

/// A slash is quoted when an odd number of backslashes precedes it.
pub(super) fn delimiter(part: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, byte) in part.bytes().enumerate() {
        match byte {
            b'/' if !escaped => return Some(index),
            b'\\' => escaped = !escaped,
            _ => escaped = false,
        }
    }
    None
}

/// Remove only the quoting of this layer and leave regex escapes intact.
pub(super) fn unescape(part: &str, chars: &[char]) -> String {
    let mut iter = part.chars().peekable();
    let mut output = String::with_capacity(part.len());
    while let Some(ch) = iter.next() {
        if ch == '\\' && iter.peek().is_some_and(|next| chars.contains(next)) {
            output.push(iter.next().expect("peeked character"));
        } else {
            output.push(ch);
        }
    }
    output
}
