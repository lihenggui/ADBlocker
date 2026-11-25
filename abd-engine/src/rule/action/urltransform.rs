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

//! The reference describes Perl-like substitution and shows `i`. Its MV3
//! limitation says “The `/g` (global) flag is ignored.” For CoreLibs,
//! substitute the first match by default and every match with `g`. Share
//! `i` and `s` with `$replace`.

use std::borrow::Cow;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD_PAD_INDIFFERENT, URL_SAFE_PAD_INDIFFERENT};
use serde::{Deserialize, Serialize};

use super::replace::{self, Parsed};
use crate::limits::{MAX_RESPONSE_REGEX_BYTES, MAX_RESPONSE_RULE, MAX_TRANSFORM_URL};
use crate::rule::quoting::unescaped;

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Spec {
    whole_url: bool,
    steps: Vec<StepSpec>,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum StepSpec {
    Substitute { pattern: Parsed, global: bool },
    Base64,
    Percent,
}

pub(crate) struct Compiled {
    pub(crate) whole_url: bool,
    steps: Vec<Step>,
}

enum Step {
    Substitute {
        regex: Box<replace::Compiled>,
        global: bool,
    },
    Base64,
    Percent,
}

/// Only a pipe following both slash-delimited fields can start another step.
fn split_step(value: &str) -> Option<(&str, Option<&str>)> {
    let fields = match value.strip_prefix('/') {
        Some(fields) => 1 + replace::value_end(fields)?,
        None => 0,
    };
    Some(match value[fields..].find('|') {
        Some(pipe) => (&value[..fields + pipe], Some(&value[fields + pipe + 1..])),
        None => (value, None),
    })
}

pub(super) fn parse(value: &str) -> Option<Spec> {
    if unescaped(value, b',').next().is_some() {
        return None;
    }
    let whole_url = value.starts_with("/^http");
    let mut remaining = Some(value);
    let mut steps = Vec::new();
    while let Some(value) = remaining {
        let (step, next) = split_step(value)?;
        steps.push(match step {
            "b64" => StepSpec::Base64,
            "pct" => StepSpec::Percent,
            _ => {
                // AdGuard Base also quotes a literal dot in the replacement URL.
                let (pattern, global) = replace::parse_substitution(step, b"gis", &['/', '.'])?;
                StepSpec::Substitute { pattern, global }
            }
        });
        remaining = next;
    }
    Some(Spec { whole_url, steps })
}

impl Spec {
    pub(super) fn compile(&self) -> Option<Compiled> {
        if self.steps.is_empty()
            || self.steps.iter().map(StepSpec::size).sum::<usize>() > MAX_RESPONSE_RULE
        {
            return None;
        }
        let mut remaining = MAX_RESPONSE_REGEX_BYTES;
        let mut steps = Vec::with_capacity(self.steps.len());
        for spec in &self.steps {
            let step = spec.compile()?;
            remaining = remaining.checked_sub(step.regex_bytes())?;
            steps.push(step);
        }
        Some(Compiled {
            whole_url: self.whole_url,
            steps,
        })
    }
}

impl StepSpec {
    fn size(&self) -> usize {
        match self {
            Self::Substitute { pattern, .. } => {
                3 + pattern.pattern.len() + pattern.replacement.len()
            }
            Self::Base64 | Self::Percent => 3,
        }
    }

    fn compile(&self) -> Option<Step> {
        Some(match self {
            Self::Substitute { pattern, global } => Step::Substitute {
                regex: Box::new(replace::compile(
                    &pattern.pattern,
                    &pattern.replacement,
                    pattern.insensitive,
                    pattern.single_line,
                )?),
                global: *global,
            },
            Self::Base64 => Step::Base64,
            Self::Percent => Step::Percent,
        })
    }
}

impl Compiled {
    pub(super) fn regex_bytes(&self) -> usize {
        self.steps.iter().map(Step::regex_bytes).sum()
    }

    /// An unsuccessful decode keeps the input to that step. Excessive
    /// substitution output abandons the entire rewrite.
    pub(crate) fn apply<'a>(&self, input: &'a str) -> Option<Cow<'a, str>> {
        if input.len() > MAX_TRANSFORM_URL {
            return None;
        }
        let mut output = Cow::Borrowed(input);
        for step in &self.steps {
            let value = match step {
                Step::Substitute { regex, global } => {
                    let bytes =
                        regex
                            .substitute(output.as_bytes(), *global, MAX_TRANSFORM_URL, &|| true)?;
                    String::from_utf8(bytes).ok()
                }
                Step::Base64 => decode(base64(output.as_bytes())),
                Step::Percent => decode(percent(output.as_bytes())),
            };
            if let Some(value) = value
                && value != output
            {
                output = Cow::Owned(value);
            }
        }
        Some(output)
    }
}

impl Step {
    fn regex_bytes(&self) -> usize {
        match self {
            Self::Substitute { regex, .. } => regex.regex_bytes(),
            Self::Base64 | Self::Percent => 0,
        }
    }
}

fn decode(bytes: Option<Vec<u8>>) -> Option<String> {
    let value = String::from_utf8(bytes?).ok()?;
    (!value.chars().any(char::is_control)).then_some(value)
}

fn base64(input: &[u8]) -> Option<Vec<u8>> {
    STANDARD_PAD_INDIFFERENT
        .decode(input)
        .or_else(|_| URL_SAFE_PAD_INDIFFERENT.decode(input))
        .ok()
}

fn percent(mut input: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(input.len());
    while let Some((&byte, rest)) = input.split_first() {
        input = rest;
        if byte == b'%' {
            let [high, low, rest @ ..] = rest else {
                return None;
            };
            decoded.push(
                ((char::from(*high).to_digit(16)? << 4) | char::from(*low).to_digit(16)?) as u8,
            );
            input = rest;
        } else {
            decoded.push(byte);
        }
    }
    Some(decoded)
}

#[cfg(test)]
#[test]
fn the_url_transform_rejects_excessive_regex_fallback_work_at_load() {
    let spec = parse("/[ab]*a[ab]{5000}c//").unwrap();
    assert!(spec.compile().is_none());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transformed(value: &str, input: &str) -> String {
        parse(value)
            .unwrap()
            .compile()
            .unwrap()
            .apply(input)
            .unwrap()
            .into_owned()
    }

    #[test]
    fn pipes_in_both_substitution_fields_stay_inside_the_step() {
        assert_eq!(split_step("/a|b/c/|pct"), Some(("/a|b/c/", Some("pct"))));
        assert_eq!(transformed(r"/(a|b)/\$1|c\//|pct", "/b%21"), "/b|c/!");
        assert_eq!(transformed(r"/(a\,b\$)/\$1\//", "/a,b"), "/a,b/");
        for invalid in ["/a/b/|", "pct||b64", "/a/b|pct", "/a/$1/", "/a/b/m"] {
            assert!(parse(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn the_reference_substitution_is_case_insensitive_and_uses_captures() {
        assert_eq!(
            transformed(r"/(pref\/).*\/(suf)/\$1\$2/i", "/PREF/remove/SUF"),
            "/PREF/SUF"
        );
    }

    #[test]
    fn substitutions_replace_the_first_match_unless_they_are_global() {
        assert_eq!(transformed("/x/y/i", "/XxX"), "/yxX");
        assert_eq!(transformed("/x/y/gi", "/XxX"), "/yyy");
        assert_eq!(transformed("/a.b/x/s", "/a\nb"), "/x");
    }

    #[test]
    fn unsuccessful_steps_pass_their_input_to_the_next_step() {
        for (value, input) in [
            ("/miss/changed/", "/original"),
            ("pct", "/%Q1"),
            ("pct", "/%"),
            ("pct", "/%FF"),
            ("pct", "/%0D"),
            ("pct", "/%C2%85"),
            ("b64", "!!!!"),
            ("b64", "/w=="),
            ("b64", "DQo="),
        ] {
            assert_eq!(transformed(value, input), input, "{value} {input}");
        }
        assert_eq!(transformed("pct|/bad/good/", "/bad%X"), "/good%X");
        assert_eq!(transformed("pct", "a+b%2Bc"), "a+b+c");
    }

    #[test]
    fn base64_decodes_each_alphabet_with_or_without_padding() {
        for encoded in ["4KC+", "4KC-", "YS8=", "YS8"] {
            let expected = if encoded.starts_with('4') {
                "࠾"
            } else {
                "a/"
            };
            assert_eq!(transformed("b64", encoded), expected);
        }
    }

    #[test]
    fn oversized_inputs_and_expanding_global_substitutions_are_abandoned() {
        let transform = parse("/a/aaa/g").unwrap().compile().unwrap();
        assert!(
            transform
                .apply(&"a".repeat(MAX_TRANSFORM_URL + 1))
                .is_none()
        );
        assert!(transform.apply(&"a".repeat(MAX_TRANSFORM_URL)).is_none());
    }
}
