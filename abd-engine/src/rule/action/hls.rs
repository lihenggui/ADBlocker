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
use regex_automata::meta::Regex;
use serde::{Deserialize, Serialize};

use crate::limits::{MAX_RESPONSE_REGEX, MAX_RESPONSE_RULE};
use crate::list_regex::{self, MatchCache, RegexInput, SearchBudget};
use crate::playlist;
use crate::rule::quoting::{delimiter, unescape, unescaped};

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
    let (regex, target) = match fields(value)? {
        Value::Basic(value) => (
            list_regex::compile_normalized(
                &pattern::translate(&unescape(value, &['/', ',', '$']))?,
                RegexInput::Text,
                true,
                false,
                MAX_RESPONSE_REGEX,
            )
            .map_err(|_| Rejection::UnsupportedPattern)?,
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
            list_regex::compile(
                &unescape(pattern, &[',', '$']),
                RegexInput::Text,
                insensitive,
                false,
                MAX_RESPONSE_REGEX,
            )
            .map_err(|_| Rejection::UnsupportedPattern)?,
            if tags { Target::Tag } else { Target::Url },
        ),
    };
    Ok(Compiled { regex, target })
}

impl Compiled {
    pub(super) fn regex_bytes(&self) -> usize {
        self.regex.memory_usage()
    }

    fn matches(&self, text: &str, cache: &mut MatchCache, budget: &mut SearchBudget) -> bool {
        let text = if self.target == Target::HostAnchoredUrl {
            let Some(url) = pattern::from_host(text) else {
                return false;
            };
            url
        } else {
            text
        };
        budget.matches(&self.regex, cache, text.as_bytes())
    }
}

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
    let (mut tags, mut urls): (Vec<_>, Vec<_>) = rules
        .iter()
        .map(|&rule| (rule, None))
        .partition(|(rule, _)| rule.target == Target::Tag);
    let (mut tag_budget, mut url_budget) = (SearchBudget::default(), SearchBudget::default());
    let search = |text: &str, rules: &mut [(&Compiled, MatchCache)], budget: &mut SearchBudget| {
        for (rule, cache) in rules {
            if !within_budget() {
                return None;
            }
            if rule.matches(text, cache, budget) {
                return Some(true);
            }
        }
        Some(false)
    };
    let result = playlist::edit(
        &body,
        |tag| search(tag, &mut tags, &mut tag_budget),
        |line| {
            if urls.is_empty() {
                return Some(false);
            }
            if search(line, &mut urls, &mut url_budget)? {
                return Some(true);
            }
            // The literal line can still match if it cannot be resolved.
            match playlist::uri::resolve(base, line) {
                Some(url) if url != line => search(&url, &mut urls, &mut url_budget),
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
            assert_eq!(
                compiled.matches("AD", &mut None, &mut SearchBudget::default()),
                matched,
                "{value}"
            );
        }
        assert!(compile(r"/a\{b\/c\,d\$/").unwrap().matches(
            "a{b/c,d",
            &mut None,
            &mut SearchBudget::default()
        ));
        assert!(
            compile("/a{b/")
                .unwrap()
                .matches("a{b", &mut None, &mut SearchBudget::default())
        );
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

#[test]
fn an_expensive_playlist_expression_is_rejected_or_finishes_within_the_work_budget() {
    let Ok(rule) = compile("/[ab]*a[ab]{5000}c/") else {
        return;
    };
    let body = Bytes::from(format!("#EXTM3U\n{}", "a".repeat((2 << 20) - 8)));
    let base = "https://site.test/playlist.m3u8".parse().unwrap();
    let start = std::time::Instant::now();
    assert_eq!(
        apply(body.clone(), &base, &[&rule], &|| {
            start.elapsed() < crate::limits::REWRITE_WORK
        }),
        Some(body)
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
}
