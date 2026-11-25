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

//! The matchers for segment URLs and tags in an HLS playlist.

use bytes::Bytes;
use hyper::Uri;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::limits::{MAX_RESPONSE_REGEX, MAX_RESPONSE_RULE};
use crate::playlist;
use crate::rule::quoting::{delimiter, unescape, unescaped};
use crate::rule::regex::translate_pattern;

mod pattern;

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Spec(String);

pub(crate) struct Compiled {
    regex: Regex,
    target: Target,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Url,
    HostAnchoredUrl,
    Tag,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Rejection {
    EmptyPattern,
    UnescapedSeparator,
    InnerModifiers,
    MissingDelimiter,
    InvalidFlags,
    UnsupportedPattern,
}

enum Value<'a> {
    Basic(&'a str),
    Regex {
        pattern: &'a str,
        insensitive: bool,
        tags: bool,
    },
}

pub(super) fn parse(value: &str) -> Result<Spec, Rejection> {
    fields(value)?;
    Ok(Spec(value.to_owned()))
}

fn fields(value: &str) -> Result<Value<'_>, Rejection> {
    if value.is_empty() {
        return Err(Rejection::EmptyPattern);
    }
    if unescaped(value, b'$').next().is_some() {
        return Err(Rejection::InnerModifiers);
    }
    if unescaped(value, b',').next().is_some() {
        return Err(Rejection::UnescapedSeparator);
    }
    let Some(regex) = value.strip_prefix('/') else {
        if delimiter(value).is_some() {
            return Err(Rejection::UnescapedSeparator);
        }
        return Ok(Value::Basic(value));
    };
    let end = delimiter(regex).ok_or(Rejection::MissingDelimiter)?;
    let (pattern, flags) = (&regex[..end], &regex[end + 1..]);
    if pattern.is_empty() {
        return Err(Rejection::EmptyPattern);
    }
    if !flags.bytes().all(|flag| matches!(flag, b'i' | b't')) {
        return Err(Rejection::InvalidFlags);
    }
    Ok(Value::Regex {
        pattern,
        insensitive: flags.contains('i'),
        tags: flags.contains('t'),
    })
}

impl Spec {
    pub(super) fn compile(&self) -> Result<Compiled, Rejection> {
        if self.0.len() > MAX_RESPONSE_RULE {
            return Err(Rejection::UnsupportedPattern);
        }
        compile(&self.0)
    }
}

fn compile(value: &str) -> Result<Compiled, Rejection> {
    let (pattern, insensitive, target) = match fields(value)? {
        Value::Basic(value) => (
            pattern::translate(&unescape(value, &['/', ',', '$']))?,
            true,
            if value.starts_with("||") {
                Target::HostAnchoredUrl
            } else {
                Target::Url
            },
        ),
        Value::Regex {
            pattern,
            insensitive,
            tags,
        } => (
            translate_pattern(&unescape(pattern, &[',', '$']))
                .ok_or(Rejection::UnsupportedPattern)?,
            insensitive,
            if tags { Target::Tag } else { Target::Url },
        ),
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(insensitive)
        .size_limit(MAX_RESPONSE_REGEX)
        .dfa_size_limit(0)
        .build()
        .map_err(|_| Rejection::UnsupportedPattern)?;
    Ok(Compiled { regex, target })
}

impl Compiled {
    fn matches(&self, text: &str) -> bool {
        match self.target {
            Target::HostAnchoredUrl => {
                pattern::from_host(text).is_some_and(|url| self.regex.is_match(url))
            }
            Target::Url | Target::Tag => self.regex.is_match(text),
        }
    }
}

/// Charge the full per-expression allowance because regex does not expose
/// the compiled heap. Disabling the lazy DFA avoids a retained DFA cache.
pub(super) const REGEX_CHARGE: usize = MAX_RESPONSE_REGEX;

pub(crate) fn takes(mime: &str) -> bool {
    matches!(
        mime,
        "application/vnd.apple.mpegurl"
            | "application/x-mpegurl"
            | "audio/mpegurl"
            | "audio/x-mpegurl"
    )
}

pub(crate) fn apply(
    body: Bytes,
    base: &Uri,
    rules: &[&Compiled],
    within_budget: &dyn Fn() -> bool,
) -> Option<Bytes> {
    let search = |text: &str, tags: bool| {
        for rule in rules
            .iter()
            .filter(|rule| (rule.target == Target::Tag) == tags)
        {
            if !within_budget() {
                return None;
            }
            if rule.matches(text) {
                return Some(true);
            }
        }
        Some(false)
    };
    let has_url_rules = rules.iter().any(|rule| rule.target != Target::Tag);
    let result = playlist::edit(
        &body,
        |tag| search(tag, true),
        |line| {
            if !has_url_rules {
                return Some(false);
            }
            if search(line, false)? {
                return Some(true);
            }
            // The literal line can still match if it cannot be resolved.
            match playlist::uri::resolve(base, line) {
                Some(url) if url != line => search(&url, false),
                _ => Some(false),
            }
        },
        within_budget,
    )?;
    Some(match result {
        std::borrow::Cow::Borrowed(_) => body,
        std::borrow::Cow::Owned(edited) => Bytes::from(edited),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_options_select_tags_and_case_insensitivity_independently() {
        for (value, target, matched) in [
            ("/ad/", Target::Url, false),
            ("/ad/i", Target::Url, true),
            ("/ad/t", Target::Tag, false),
            ("/ad/it", Target::Tag, true),
            ("/ad/ti", Target::Tag, true),
        ] {
            let compiled = compile(value).unwrap();
            assert!(compiled.target == target, "{value}");
            assert_eq!(compiled.matches("AD"), matched, "{value}");
        }
        assert!(compile(r"/a\{b\/c\,d\$/").unwrap().matches("a{b/c,d"));
        assert!(compile("/a{b/").unwrap().matches("a{b"));
    }

    #[test]
    fn invalid_values_have_a_rejection_and_never_compile() {
        for (value, expected) in [
            ("", Rejection::EmptyPattern),
            ("//t", Rejection::EmptyPattern),
            ("ad/path", Rejection::UnescapedSeparator),
            ("/a,b/", Rejection::UnescapedSeparator),
            ("ad$script", Rejection::InnerModifiers),
            ("/ad$/t", Rejection::InnerModifiers),
            ("/ad", Rejection::MissingDelimiter),
            ("/ad/s", Rejection::InvalidFlags),
            ("||^", Rejection::UnsupportedPattern),
            ("/(?=ad)/", Rejection::UnsupportedPattern),
            ("/a++/", Rejection::UnsupportedPattern),
            ("/\\w{100000}/", Rejection::UnsupportedPattern),
        ] {
            assert_eq!(compile(value).err(), Some(expected), "{value}");
        }
        assert!(Spec("x".repeat(MAX_RESPONSE_RULE + 1)).compile().is_err());
    }
}
