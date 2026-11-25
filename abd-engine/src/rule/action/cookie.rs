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

//! Cookie selectors and modifications from filter rules.

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::limits::{MAX_RESPONSE_REGEX, MAX_RESPONSE_RULE};
use crate::rule::quoting::{delimiter, unescape, unescaped};
use crate::rule::regex::translate_pattern;

mod date;
mod headers;
pub(crate) use headers::{request, response};

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Spec {
    selector: Selector,
    change: Change,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum Selector {
    Any,
    Name(String),
    Pattern(String),
}

enum Matcher {
    Any,
    Name(String),
    Pattern(Regex),
}

pub(crate) struct Compiled {
    selector: Matcher,
    change: Change,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum Change {
    Suppress,
    Exempt,
    Attributes(Attributes),
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct Attributes {
    max_age: Option<i64>,
    same_site: Option<SameSite>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum SameSite {
    Strict,
    Lax,
    None,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Rejection {
    TooLong,
    InvalidName,
    InvalidExpression,
    UnescapedSeparator,
    InvalidAttribute,
    DuplicateAttribute,
    InvalidMaxAge,
    InvalidSameSite,
    AttributesOnException,
}

pub(crate) fn parse(value: &str, exception: bool) -> Result<Spec, Rejection> {
    if value.len() > MAX_RESPONSE_RULE {
        return Err(Rejection::TooLong);
    }
    let (selector, attributes) = fields(value.trim())?;
    let change = if exception {
        if attributes.is_some() {
            return Err(Rejection::AttributesOnException);
        }
        Change::Exempt
    } else if let Some(attributes) = attributes {
        Change::Attributes(Attributes::parse(attributes)?)
    } else {
        Change::Suppress
    };
    Ok(Spec { selector, change })
}

fn fields(value: &str) -> Result<(Selector, Option<&str>), Rejection> {
    if let Some(regex) = value.strip_prefix('/') {
        let end = delimiter(regex).ok_or(Rejection::InvalidExpression)?;
        let (pattern, tail) = (&regex[..end], regex[end + 1..].trim_start());
        if pattern.is_empty() {
            return Err(Rejection::InvalidExpression);
        }
        if b",$"
            .iter()
            .any(|&byte| unescaped(pattern, byte).next().is_some())
        {
            return Err(Rejection::UnescapedSeparator);
        }
        let attributes = if tail.is_empty() {
            None
        } else {
            Some(tail.strip_prefix(';').ok_or(Rejection::InvalidExpression)?)
        };
        let pattern = translate_pattern(&unescape(pattern, &['/', ',', '$']))
            .ok_or(Rejection::InvalidExpression)?;
        return Ok((Selector::Pattern(pattern), attributes));
    }
    let (name, attributes) = match value.split_once(';') {
        Some((name, attributes)) => (name.trim_end(), Some(attributes)),
        None => (value, None),
    };
    let selector = match name {
        "" => Selector::Any,
        name if cookie_name(name) => Selector::Name(name.to_owned()),
        _ => return Err(Rejection::InvalidName),
    };
    Ok((selector, attributes))
}

impl Attributes {
    fn parse(attributes: &str) -> Result<Self, Rejection> {
        let mut parsed = Self::default();
        for attribute in attributes.split(';') {
            let (key, value) = attribute
                .split_once('=')
                .ok_or(Rejection::InvalidAttribute)?;
            match key.trim() {
                "maxAge" if parsed.max_age.is_none() => {
                    parsed.max_age = Some(
                        age_digits(value.trim())
                            .and_then(|age| age.parse().ok())
                            .ok_or(Rejection::InvalidMaxAge)?,
                    );
                }
                "sameSite" if parsed.same_site.is_none() => {
                    parsed.same_site = Some(SameSite::parse(value.trim())?);
                }
                "maxAge" | "sameSite" => return Err(Rejection::DuplicateAttribute),
                _ => return Err(Rejection::InvalidAttribute),
            }
        }
        Ok(parsed)
    }
}

impl SameSite {
    fn parse(value: &str) -> Result<Self, Rejection> {
        match value.to_ascii_lowercase().as_str() {
            "strict" => Ok(Self::Strict),
            "lax" => Ok(Self::Lax),
            "none" => Ok(Self::None),
            _ => Err(Rejection::InvalidSameSite),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "Strict",
            Self::Lax => "Lax",
            Self::None => "None",
        }
    }
}

fn age_digits(value: &str) -> Option<&str> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(value)
}

fn cookie_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

impl Spec {
    pub(super) fn compile(&self) -> Result<Compiled, Rejection> {
        let selector = match &self.selector {
            Selector::Any => Matcher::Any,
            Selector::Name(name) if cookie_name(name) => Matcher::Name(name.clone()),
            Selector::Name(_) => return Err(Rejection::InvalidName),
            Selector::Pattern(pattern) if pattern.len() <= MAX_RESPONSE_RULE * 2 => {
                Matcher::Pattern(
                    RegexBuilder::new(pattern)
                        .unicode(false)
                        .size_limit(MAX_RESPONSE_REGEX)
                        .dfa_size_limit(0)
                        .build()
                        .map_err(|_| Rejection::InvalidExpression)?,
                )
            }
            Selector::Pattern(_) => return Err(Rejection::TooLong),
        };
        Ok(Compiled {
            selector,
            change: self.change,
        })
    }

    pub(crate) fn validate(&self) -> Result<(), Rejection> {
        // Catch unsupported regex syntax while the reader can still name the reason.
        if matches!(self.selector, Selector::Pattern(_)) {
            self.compile()?;
        }
        Ok(())
    }
}

impl Compiled {
    pub(super) fn regex_bytes(&self) -> usize {
        if matches!(self.selector, Matcher::Pattern(_)) {
            MAX_RESPONSE_REGEX
        } else {
            0
        }
    }

    fn matches(&self, name: &str) -> bool {
        match &self.selector {
            Matcher::Any => true,
            Matcher::Name(expected) => expected == name,
            Matcher::Pattern(regex) => regex.is_match(name),
        }
    }
}
