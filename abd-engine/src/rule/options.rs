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

//! Option boundaries and quoting shared by the rule reader and action values.

use super::modifiers::{
    APP, CSP, DENYALLOW, HLS, NETWORK, PRUNE, REPLACE, URLTRANSFORM, is_noop,
    is_removeheader_option,
};
use super::quoting::{delimiter, unescaped};

/// Split options after skipping the two slash-delimited replacement fields.
fn options(text: &str, quoting: Quoting) -> impl Iterator<Item = &str> {
    let mut remaining = Some(text);
    std::iter::from_fn(move || {
        let text = remaining?;
        let (start, comma) = match quoting {
            Quoting::Literal => (0, text.find(',')),
            Quoting::AdGuard => {
                let start = custom_value_end(text).unwrap_or(0);
                (start, unescaped(&text[start..], b',').next())
            }
        };
        let Some(comma) = comma else {
            remaining = None;
            return Some(text);
        };
        let end = start + comma;
        remaining = Some(&text[end + 1..]);
        Some(&text[..end])
    })
}

#[derive(Clone, Copy)]
pub(super) enum Quoting {
    Literal,
    AdGuard,
}

#[derive(Clone, Copy)]
pub(crate) struct OptionToken<'a> {
    pub(crate) raw: &'a str,
    pub(crate) raw_name: &'a str,
    pub(crate) value: Option<&'a str>,
}

impl<'a> OptionToken<'a> {
    pub(crate) fn name(&self) -> &'a str {
        self.raw_name.trim_start_matches('~')
    }
}

pub(super) struct Options<'a> {
    pub(super) pattern: &'a str,
    pub(super) suffix: &'a str,
    spans: Vec<(usize, Span)>,
    pub(super) noop: bool,
}

/// Store positions within the suffix so even a long list of options
/// needs only one borrowed string.
#[derive(Clone, Copy)]
struct Span {
    end: usize,
    name_end: usize,
}

impl Span {
    fn token(self, suffix: &str, start: usize) -> OptionToken<'_> {
        let Self { end, name_end } = self;
        OptionToken {
            raw: &suffix[start..end],
            raw_name: &suffix[start..name_end],
            value: (name_end < end).then(|| &suffix[name_end + 1..end]),
        }
    }
}

fn spans(suffix: &str, quoting: Quoting) -> impl Iterator<Item = (usize, Span)> {
    let mut start = 0;
    options(suffix, quoting).map(move |raw| {
        let end = start + raw.len();
        let name_end = raw.find('=').map_or(end, |offset| start + offset);
        let previous = start;
        start = end + 1;
        (previous, Span { end, name_end })
    })
}

impl<'a> Options<'a> {
    pub(super) fn read(line: &'a str, quoting: Quoting) -> Option<Self> {
        let at = match quoting {
            Quoting::Literal => line.rfind('$'),
            Quoting::AdGuard => option_separator(line),
        }?;
        Some(Self::split(&line[..at], &line[at + 1..], quoting))
    }

    /// The older page and host projections see every comma and the last dollar.
    pub(super) fn also_literal(&self) -> bool {
        !self.suffix.contains('$') && self.tokens().all(|token| !token.raw.contains(','))
    }

    pub(super) fn split(pattern: &'a str, suffix: &'a str, quoting: Quoting) -> Self {
        let mut noop = false;
        let spans = spans(suffix, quoting)
            .filter(|(start, span)| {
                // AdGuard says a noop "can appear in a rule as often as needed."
                // Discard it in any position, even if only the pattern remains.
                let skip = is_noop(&suffix[*start..span.end]);
                noop |= skip;
                !skip
            })
            .collect();
        Self {
            pattern,
            suffix,
            spans,
            noop,
        }
    }

    pub(super) fn tokens(&self) -> impl Iterator<Item = OptionToken<'a>> + '_ {
        (0..self.spans.len()).map(|index| self.token(index))
    }

    pub(super) fn token(&self, index: usize) -> OptionToken<'a> {
        let (start, span) = self.spans[index];
        span.token(self.suffix, start)
    }

    pub(super) fn write<'b>(&'b self, option: impl Fn(OptionToken<'b>) -> &'b str) -> String {
        let mut line = self.pattern.to_owned();
        let mut separator = '$';
        for token in self.tokens() {
            line.push(separator);
            line.push_str(option(token));
            separator = ',';
        }
        line
    }

    pub(super) fn without_noops(&self) -> Option<String> {
        self.noop.then(|| self.write(|token| token.raw))
    }

    pub(super) fn written_len(&self) -> usize {
        self.pattern.len()
            + if self.noop {
                self.tokens()
                    .map(|token| token.raw.len() + 1)
                    .sum::<usize>()
            } else {
                self.suffix.len() + 1
            }
    }
}

fn replacement_fields(text: &str) -> Option<&str> {
    text.strip_prefix(REPLACE)?.strip_prefix("=/")
}

pub(super) fn source_regex_end(option: &str) -> Option<usize> {
    let value = option
        .strip_prefix("domain=")
        .or_else(|| option.strip_prefix("from="))?;
    let body = value.strip_prefix('~').unwrap_or(value).strip_prefix('/')?;
    let end = option.len() - body.len() + delimiter(body)? + 1;
    (end == option.len() || option.as_bytes().get(end) == Some(&b',')).then_some(end)
}

fn custom_value_end(option: &str) -> Option<usize> {
    replacement_fields(option)
        .map(|value| option.len() - value.len() + option_value_end(value))
        .or_else(|| source_regex_end(option))
}

/// A regular expression that forms the condition can contain dollar signs.
pub(crate) fn regex_option_separator(line: &str) -> Option<usize> {
    let pattern = line.strip_prefix("@@").unwrap_or(line);
    let close = delimiter(pattern.strip_prefix('/')?)?;
    let after = line.len() - pattern.len() + close + 2;
    (line.as_bytes().get(after) == Some(&b'$')).then_some(after)
}

/// Find the last unquoted dollar outside a regular expression condition or
/// a replacement, app or denyallow value. The value parsers check quoting
/// separately, so malformed quoting stays with the rule that contains it.
pub(crate) fn option_separator(line: &str) -> Option<usize> {
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
    let mut owned_options = false;
    let bytes = line.as_bytes();
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        cursor += 1;
        if cursor >= 2 && bytes[cursor - 2] == b'\\' {
            continue;
        }
        if byte == b'$' {
            if !owned_options {
                last = Some(cursor - 1);
            }
        } else if byte != b',' || last.is_none() {
            continue;
        }
        let option = &line[cursor..];
        if is_removeheader_option(option)
            || [APP, CSP, DENYALLOW, HLS, NETWORK, URLTRANSFORM]
                .into_iter()
                .any(|name| {
                    option
                        .trim_start_matches('~')
                        .strip_prefix(name)
                        .is_some_and(|tail| tail.is_empty() || tail.starts_with(['=', ',']))
                })
        {
            owned_options = true;
        }
        if let Some(end) = custom_value_end(option) {
            cursor += end;
        } else if line[cursor..]
            .strip_prefix(PRUNE)
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
pub(crate) fn option_value_end(value: &str) -> usize {
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
        || (tail.starts_with('_') && is_noop(tail.split(',').next().unwrap_or_default()))
}

#[cfg(test)]
pub(crate) fn option_texts(text: &str) -> impl Iterator<Item = &str> {
    options(text, Quoting::AdGuard)
}

pub(super) fn literal_options(suffix: &str) -> impl Iterator<Item = &str> {
    options(suffix, Quoting::Literal)
}

pub(super) fn literal_tokens(suffix: &str) -> impl Iterator<Item = OptionToken<'_>> {
    spans(suffix, Quoting::Literal).map(|(start, span)| span.token(suffix, start))
}
