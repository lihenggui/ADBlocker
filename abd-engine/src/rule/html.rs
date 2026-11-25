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

//! CoreLibs HTML element selectors. After `^`, a run of positive look-aheads
//! can be checked independently: `^(?=P$)` becomes `^(?:P)$` and `^(?=P)`
//! becomes `^(?:P)`. The remaining pattern `R` must also match as `^(?:R)`.
//! AdGuard's table of equivalents uses the end-anchored form for lengths.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use super::quoting::delimiter;
use crate::htmlfilter::{Attribute, AttributeValue, Compiled, Content, Selector};
use crate::limits::{
    MAX_HTML_CHAIN, MAX_HTML_CONDITIONS, MAX_HTML_REGEX, MAX_HTML_REGEX_BYTES, MAX_HTML_REGEXES,
    MAX_HTML_RULE, MAX_HTML_RULES, MAX_RESPONSE_RULE,
};
use crate::list_regex::{self, CompileError, RegexInput};
use crate::public_suffix::Name;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rejection {
    Size,
    Domain,
    Selector,
    Attribute,
    AttributeOperator,
    ContentAttribute,
    ChildContent,
    PseudoClass,
    ResponseHeader,
    RegexFlags,
    Regex,
    LookAround,
    Length,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Spec {
    pub(crate) selector: String,
    pub(crate) exception: bool,
    pub(crate) domains: Vec<Domain>,
    pub(crate) steps: Vec<Step>,
}

pub(crate) struct Rule {
    pub(crate) spec: Spec,
    pub(crate) compiled: Compiled,
}

pub(crate) struct Budget {
    rules: usize,
    expressions: usize,
    bytes: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            rules: MAX_HTML_RULES,
            expressions: MAX_HTML_REGEXES,
            bytes: MAX_HTML_REGEX_BYTES,
        }
    }
}

impl Budget {
    pub(crate) fn keep(&mut self, rule: &Compiled) -> Result<(), crate::rule::Code> {
        let (expressions, bytes) = rule.regex_cost();
        let rules = self
            .rules
            .checked_sub(1)
            .ok_or(crate::rule::Code::HtmlGenerationLimit)?;
        let expressions = self
            .expressions
            .checked_sub(expressions)
            .ok_or(crate::rule::Code::HtmlGenerationLimit)?;
        let bytes = self
            .bytes
            .checked_sub(bytes)
            .ok_or(crate::rule::Code::HtmlGenerationLimit)?;
        *self = Self {
            rules,
            expressions,
            bytes,
        };
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Domain {
    pub(crate) name: String,
    pub(crate) exclude: bool,
    pub(crate) wildcard: bool,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Step {
    element: Selector,
    pub(crate) content: Vec<Condition>,
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Condition {
    Contains(String),
    Matches {
        pattern: String,
        insensitive: bool,
        single_line: bool,
    },
    Glob(String),
    Length {
        min: Option<usize>,
        max: Option<usize>,
        allow_newlines: bool,
    },
}

pub(super) struct Fields<'a> {
    domains: &'a str,
    selector: &'a str,
    exception: bool,
    ublock: bool,
}

/// The first rule delimiter establishes the syntax. A later marker can
/// be part of a network option, CSS selector, script or HTML condition.
pub(super) fn fields(line: &str) -> Option<Fields<'_>> {
    if line.starts_with(['|', '/', '@']) {
        return None;
    }
    let at = line.find(['$', '#'])?;
    let (domains, rest) = line.split_at(at);
    if domains.contains(['/', '?', '^', '=', ':', '\\', '|']) {
        return None;
    }
    [("$$", false), ("$@$", true), ("##^", false), ("#@#^", true)]
        .into_iter()
        .find_map(|(marker, exception)| {
            Some(Fields {
                domains: domains.trim(),
                selector: rest.strip_prefix(marker)?.trim(),
                exception,
                ublock: marker.starts_with('#'),
            })
        })
}

pub(super) fn read(line: &str, fields: Fields<'_>) -> Result<Rule, Rejection> {
    let Fields {
        domains,
        selector,
        exception,
        ublock,
    } = fields;
    if line.len() > MAX_HTML_RULE || selector.len() > MAX_RESPONSE_RULE {
        return Err(Rejection::Size);
    }
    if selector.starts_with("responseheader(") && ublock {
        return Err(Rejection::ResponseHeader);
    }
    let domains = if domains.is_empty() {
        Vec::new()
    } else {
        domains.split(',').map(domain).collect::<Result<_, _>>()?
    };
    let steps = chain(selector, ublock)?;
    let spec = Spec {
        selector: selector.to_owned(),
        exception,
        domains,
        steps,
    };
    let compiled = spec.compile()?;
    Ok(Rule { spec, compiled })
}

fn domain(text: &str) -> Result<Domain, Rejection> {
    let text = text.trim();
    let (name, exclude) = text.strip_prefix('~').map_or((text, false), |s| (s, true));
    let (name, wildcard) = name.strip_suffix(".*").map_or((name, false), |s| (s, true));
    let name = idna::domain_to_ascii_strict(name).map_err(|_| Rejection::Domain)?;
    let name = name.strip_suffix('.').unwrap_or(&name);
    if (wildcard && name.contains('.'))
        || (Name::parse(name).is_none() && (wildcard || name.parse::<IpAddr>().is_err()))
    {
        return Err(Rejection::Domain);
    }
    Ok(Domain {
        name: name.to_ascii_lowercase(),
        exclude,
        wildcard,
    })
}

fn chain(mut text: &str, ublock: bool) -> Result<Vec<Step>, Rejection> {
    let mut steps = Vec::new();
    loop {
        let (step, rest) = step(text.trim_start(), ublock)?;
        steps.push(step);
        if rest.is_empty() {
            return Ok(steps);
        }
        text = rest.strip_prefix('>').ok_or(Rejection::Selector)?;
        if !steps.last().unwrap().content.is_empty() {
            return Err(Rejection::ChildContent);
        }
        if steps.len() == MAX_HTML_CHAIN {
            return Err(Rejection::Size);
        }
    }
}

fn name_prefix<'a>(text: &'a str, extra: &[u8]) -> (&'a str, &'a str) {
    let end = text
        .bytes()
        .position(|b| !b.is_ascii_alphanumeric() && !extra.contains(&b))
        .unwrap_or(text.len());
    text.split_at(end)
}

fn step(text: &str, ublock: bool) -> Result<(Step, &str), Rejection> {
    let (tag, mut rest) = if text.starts_with(|c: char| c.is_ascii_alphabetic()) {
        name_prefix(text, b"-_")
    } else {
        ("", text)
    };
    let mut step = Step {
        element: Selector {
            tag: (!tag.is_empty()).then(|| tag.to_ascii_lowercase()),
            attributes: Vec::new(),
        },
        content: Vec::new(),
    };
    while let Some(attribute) = rest.strip_prefix('[') {
        rest = read_attribute(attribute, &mut step, ublock)?;
    }
    while let Some(pseudo) = rest.strip_prefix(':') {
        let (name, arg) = name_prefix(pseudo, b"-");
        if !matches!(name, "contains" | "-abp-contains" | "has-text") {
            return Err(Rejection::PseudoClass);
        }
        rest = contains(arg, &mut step.content)?;
        if step.content.len() > MAX_HTML_CONDITIONS {
            return Err(Rejection::Size);
        }
    }
    if step.element.tag.is_none() && step.element.attributes.is_empty() && step.content.is_empty() {
        return Err(Rejection::Selector);
    }
    Ok((step, rest.trim_start()))
}

fn read_attribute<'a>(text: &'a str, step: &mut Step, ublock: bool) -> Result<&'a str, Rejection> {
    let (name, tail) = name_prefix(text.trim_start(), b"-_:.");
    if name.is_empty() {
        return Err(Rejection::Attribute);
    }
    let tail = tail.trim_start();
    let (operator, tail) = match tail.as_bytes() {
        [b'=', ..] => (Some(b'='), &tail[1..]),
        [operator @ (b'*' | b'^' | b'$'), b'=', ..] => (Some(*operator), &tail[2..]),
        [operator @ (b'~' | b'|'), b'=', ..] if ublock => (Some(*operator), &tail[2..]),
        [b'~' | b'|', b'=', ..] => return Err(Rejection::AttributeOperator),
        _ => (None, tail),
    };
    let (value, rest) = if operator.is_some() {
        let (value, rest) =
            attribute_value(tail.trim_start(), name.eq_ignore_ascii_case("wildcard"))?;
        (Some(value), rest)
    } else {
        (None, tail)
    };
    let rest = rest
        .trim_start()
        .strip_prefix(']')
        .ok_or(Rejection::Attribute)?;
    let name = name.to_ascii_lowercase();
    let special = matches!(
        name.as_str(),
        "tag-content" | "wildcard" | "min-length" | "max-length"
    );
    if !special {
        let value = value.map(|value| match operator {
            Some(b'=') if ublock => AttributeValue::Exact(value),
            Some(b'~') => AttributeValue::Word(value),
            Some(b'|') => AttributeValue::DashPrefix(value),
            Some(b'*' | b'^' | b'$') if ublock && value.is_empty() => AttributeValue::Never,
            Some(b'^') => AttributeValue::StartsWith(value),
            Some(b'$') => AttributeValue::EndsWith(value),
            _ => AttributeValue::Contains(value),
        });
        step.element.attributes.push(Attribute { name, value });
        return Ok(rest);
    }
    let value = value.ok_or(Rejection::ContentAttribute)?;
    if operator != Some(b'=') {
        return Err(Rejection::AttributeOperator);
    }
    let condition = match name.as_str() {
        "tag-content" => Condition::Contains(value),
        "wildcard" => Condition::Glob(value),
        _ => {
            let n = value.parse().map_err(|_| Rejection::Length)?;
            Condition::Length {
                min: (name == "min-length").then_some(n),
                max: (name == "max-length").then_some(n),
                allow_newlines: true,
            }
        }
    };
    step.content.push(condition);
    Ok(rest)
}

fn attribute_value(text: &str, wildcard: bool) -> Result<(String, &str), Rejection> {
    let Some(mut rest) = text.strip_prefix('"') else {
        let (value, _) = text.split_once(']').ok_or(Rejection::Attribute)?;
        let value = value.trim_end();
        if value.is_empty() || value.starts_with('\'') || value.contains(['[', '"']) {
            return Err(Rejection::Attribute);
        }
        return Ok((value.to_owned(), &text[value.len()..]));
    };
    if wildcard
        && let Some(inner) = rest.strip_prefix('"')
        && let Some((value, tail)) = inner.split_once("\"\"")
        && !value.contains('"')
        && tail.trim_start().starts_with(']')
    {
        return Ok((format!("\"{value}\""), tail));
    }
    let mut value = String::new();
    loop {
        let (part, tail) = rest.split_once('"').ok_or(Rejection::Attribute)?;
        value.push_str(part);
        if let Some(after_quote) = tail.strip_prefix('"') {
            value.push('"');
            rest = after_quote;
        } else if tail.trim_start().starts_with(']') {
            return Ok((value, tail));
        } else {
            return Err(Rejection::Attribute);
        }
    }
}

fn contains<'a>(text: &'a str, conditions: &mut Vec<Condition>) -> Result<&'a str, Rejection> {
    let text = text.strip_prefix('(').ok_or(Rejection::PseudoClass)?;
    if let Some(slash) = text.strip_prefix('/')
        && let Some(end) = delimiter(slash)
        && let Some((flags, tail)) = slash[end + 1..].split_once(')')
        && flags.bytes().all(|b| b.is_ascii_alphabetic())
    {
        regex(&slash[..end], flags, conditions)?;
        return Ok(tail);
    }

    let mut depth = 0;
    let mut fallback = None;
    for (at, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                let tail = &text[at + 1..];
                if tail.is_empty() || tail.starts_with(':') || tail.trim_start().starts_with('>') {
                    fallback = Some((at, tail));
                    if depth == 0 {
                        break;
                    }
                }
                depth -= 1;
            }
            _ => (),
        }
    }
    let (end, tail) = fallback.ok_or(Rejection::PseudoClass)?;
    conditions.push(Condition::Contains(text[..end].to_owned()));
    Ok(tail)
}

fn regex(pattern: &str, flags: &str, conditions: &mut Vec<Condition>) -> Result<(), Rejection> {
    if !flags.bytes().all(|b| matches!(b, b'i' | b's')) {
        return Err(Rejection::RegexFlags);
    }
    let single_line = flags.contains('s');
    let matches = |pattern| Condition::Matches {
        pattern,
        insensitive: flags.contains('i'),
        single_line,
    };
    let Some(mut rest) = pattern
        .strip_prefix('^')
        .filter(|rest| rest.starts_with("(?="))
    else {
        conditions.push(matches(pattern.to_owned()));
        return Ok(());
    };
    while let Some(body) = rest.strip_prefix("(?=") {
        let end = assertion_end(body).ok_or(Rejection::LookAround)?;
        let assertion = &body[..end];
        let condition = if let Some(inner) = without_end_anchor(assertion) {
            length(inner, single_line)?.unwrap_or_else(|| matches(format!("^(?:{inner})$")))
        } else {
            matches(format!("^(?:{assertion})"))
        };
        conditions.push(condition);
        rest = &body[end + 1..];
    }
    if !(rest.is_empty() || (single_line && matches!(rest, ".*" | ".*$"))) {
        conditions.push(matches(format!("^(?:{rest})")));
    }
    Ok(())
}

fn without_end_anchor(pattern: &str) -> Option<&str> {
    pattern.strip_suffix('$').filter(|s| {
        s.bytes()
            .rev()
            .take_while(|&b| b == b'\\')
            .count()
            .is_multiple_of(2)
    })
}

/// Find the assertion's closing parenthesis, including regex groups and
/// bracket classes inside the asserted expression.
fn assertion_end(text: &str) -> Option<usize> {
    let mut groups = 0;
    let mut classes = 0;
    let mut bytes = text.bytes().enumerate().peekable();
    while let Some((at, byte)) = bytes.next() {
        match byte {
            b'\\' => {
                bytes.next()?;
            }
            b'[' if classes == 0 => classes = 1,
            b']' if classes > 0 => classes -= 1,
            b'(' if classes == 0 => groups += 1,
            b')' if classes == 0 && groups == 0 => return Some(at),
            b')' if classes == 0 => groups -= 1,
            _ => (),
        }
    }
    None
}

fn length(text: &str, allow_newlines: bool) -> Result<Option<Condition>, Rejection> {
    let Some(count) = text.strip_prefix(".{").and_then(|s| s.strip_suffix('}')) else {
        return Ok(None);
    };
    if !count.bytes().all(|b| b.is_ascii_digit() || b == b',') {
        return Ok(None);
    }
    let number = |s: &str| {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Rejection::Length);
        }
        s.parse::<u16>()
            .map(usize::from)
            .map_err(|_| Rejection::Length)
    };
    let (min, max) = match count.split_once(',') {
        Some((min, "")) => (number(min)?, None),
        Some((min, max)) => (number(min)?, Some(number(max)?)),
        None => {
            let n = number(count)?;
            (n, Some(n))
        }
    };
    if max.is_some_and(|max| min > max) {
        return Err(Rejection::Length);
    }
    Ok(Some(Condition::Length {
        min: Some(min),
        max,
        allow_newlines,
    }))
}

impl Spec {
    pub(crate) fn compile(&self) -> Result<Compiled, Rejection> {
        let (last, parents) = self.steps.split_last().ok_or(Rejection::Selector)?;
        if parents.iter().any(|step| !step.content.is_empty()) {
            return Err(Rejection::ChildContent);
        }
        if self.selector.len() > MAX_RESPONSE_RULE
            || self.steps.len() > MAX_HTML_CHAIN
            || last.content.len() > MAX_HTML_CONDITIONS
        {
            return Err(Rejection::Size);
        }
        let content = last
            .content
            .iter()
            .map(Content::compile)
            .collect::<Result<_, _>>()?;
        let steps = self.steps.iter().map(|step| step.element.clone()).collect();
        Ok(Compiled { steps, content })
    }
}

fn regex_rejection(error: CompileError) -> Rejection {
    match error {
        CompileError::Limit => Rejection::Size,
        CompileError::LookAround => Rejection::LookAround,
        CompileError::Unsupported => Rejection::Regex,
    }
}

impl Content {
    fn compile(condition: &Condition) -> Result<Self, Rejection> {
        Ok(match condition {
            Condition::Contains(text) => Self::Contains(text.clone()),
            Condition::Matches {
                pattern,
                insensitive,
                single_line,
            } => Self::Matches(
                list_regex::compile(
                    pattern,
                    RegexInput::Text,
                    *insensitive,
                    *single_line,
                    MAX_HTML_REGEX,
                )
                .map_err(regex_rejection)?,
            ),
            Condition::Glob(glob) => {
                let mut pattern = String::from(r"\A");
                for (index, part) in glob.split('*').enumerate() {
                    if index != 0 {
                        pattern.push_str(".*");
                    }
                    for (index, literal) in part.split('?').enumerate() {
                        if index != 0 {
                            pattern.push('.');
                        }
                        pattern.push_str(&::regex::escape(literal));
                    }
                }
                pattern.push_str(r"\z");
                Self::Glob(
                    list_regex::compile_normalized(
                        &pattern,
                        RegexInput::Text,
                        false,
                        true,
                        MAX_HTML_REGEX,
                    )
                    .map_err(regex_rejection)?,
                )
            }
            Condition::Length {
                min,
                max,
                allow_newlines,
            } => Self::Length {
                min: *min,
                max: *max,
                allow_newlines: *allow_newlines,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{self, Kind, Reason};

    #[test]
    fn recognized_markers_own_only_their_selector_text() {
        for text in [
            " example.org$$  SCRIPT[DATA-SRC]  ",
            "example.org$@$SCRIPT[DATA-SRC]",
            "example.org##^SCRIPT[DATA-SRC]",
            "example.org#@#^SCRIPT[DATA-SRC]",
        ] {
            let read = rule::read(text);
            assert!(read.matcher_input().is_none());
            assert!(read.network().is_none());
            let Kind::Html(rule) = read.kind else {
                panic!("rejected {text}");
            };
            assert_eq!(rule.spec.selector, "SCRIPT[DATA-SRC]");
            assert_eq!(rule.spec.steps[0].element.tag.as_deref(), Some("script"));
            assert_eq!(rule.spec.steps[0].element.attributes[0].name, "data-src");
            assert_eq!(rule.spec.exception, text.contains('@'));
        }
        for text in [
            r#"example.org##a[data-value="$$script"]"#,
            r"||example.org^$replace=/\$\$/x/",
            r"/abc\$\$def/",
        ] {
            assert!(!matches!(rule::read(text).kind, Kind::Html(_)), "{text}");
        }
    }

    #[test]
    fn rejected_selectors_name_the_construct_that_failed() {
        for (text, expected) in [
            ("example.*.org$$script", Rejection::Domain),
            ("$$", Rejection::Selector),
            ("$$div.ad", Rejection::Selector),
            ("$$div[=hi]", Rejection::Attribute),
            ("$$div[class~=ad]", Rejection::AttributeOperator),
            ("$$div[lang|=en]", Rejection::AttributeOperator),
            ("$$div[tag-content*=ad]", Rejection::AttributeOperator),
            ("$$div[wildcard^=ad]", Rejection::AttributeOperator),
            ("$$div[min-length$=2]", Rejection::AttributeOperator),
            ("$$div[max-length*=2]", Rejection::AttributeOperator),
            ("$$div[tag-content]", Rejection::ContentAttribute),
            ("$$div[max-length=nan]", Rejection::Length),
            ("$$div:contains(ad) > script", Rejection::ChildContent),
            ("$$div[wildcard=\"*x\"] > script", Rejection::ChildContent),
            ("$$div:first-child", Rejection::PseudoClass),
            ("$$script:contains(unterminated", Rejection::PseudoClass),
            ("##^responseheader(CSP)", Rejection::ResponseHeader),
            ("$$script:contains(/x/m)", Rejection::RegexFlags),
            ("$$script:contains(/(/)", Rejection::Regex),
            ("$$script:contains(/(?<=x)y/)", Rejection::LookAround),
            ("$$script:contains(/^(?!x$).*/s)", Rejection::LookAround),
            ("$$script:contains(/^x(?=y)/s)", Rejection::LookAround),
            ("$$script:contains(/^(?=x)x(?=y)/s)", Rejection::LookAround),
            ("$$script:contains(/^(?=x)(?!y).*/s)", Rejection::LookAround),
            (
                "$$script:contains(/^(?=x(?<=x)).*/s)",
                Rejection::LookAround,
            ),
            ("$$script:contains(/^(?=.{65536,}$).*/s)", Rejection::Length),
        ] {
            let read = rule::read(text);
            assert!(
                matches!(
                    read.kind,
                    Kind::NotApplied(Reason::HtmlFiltering(reason)) if reason == expected
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn assertion_end_skips_escaped_parentheses() {
        let assertion = r"prefix\(value\)";
        let with_tail = format!("{assertion})suffix");
        assert_eq!(assertion_end(&with_tail), Some(assertion.len()));
        assert_eq!(assertion_end(assertion), None);
    }

    #[test]
    fn assertion_end_ignores_parentheses_inside_character_classes() {
        for assertion in [r"[)][^()\]]", r"(?:[])literal", r"(?:[^])literal", r"[[(]"] {
            let with_tail = format!("{assertion})suffix");
            assert_eq!(
                assertion_end(&with_tail),
                Some(assertion.len()),
                "{assertion}"
            );
        }
        assert_eq!(assertion_end(r"[)]"), None);
    }

    #[test]
    fn assertion_end_balances_nested_groups() {
        let assertion = r"(?:foo(bar)|(?i:baz))";
        let with_tail = format!("{assertion})suffix");
        assert_eq!(assertion_end(&with_tail), Some(assertion.len()));
        assert_eq!(assertion_end(r"(?:foo(bar)"), None);
    }

    #[test]
    fn android_list_selectors_keep_their_content_and_ancestor_conditions() {
        for text in [
            "example.org$$script:contains(getPopunderCookie):contains(/^(?=.{0,2500}$).*/s)",
            r"example.org$$script:contains(/^(?=.{1000,2000}$)(?=.*tips\[1\].*document\.write\(tips.*$).*/s)",
            r#"example.org$$script[tag-content="banner"][max-length="400"]"#,
            r#"example.org$$div[class="ad"] > script[src="x"]"#,
            r"ign.com$$script:contains(/^(?=.{10000,30000}$)(?=.*'iframe-network').*/s)",
            r#"mobile01.com$$script:contains(/^(?=.{0,550}$)(?=.*\$\.cookie\("fullscreenad").*/s)"#,
            r#"buienalarm.nl$$script[src*="html-load.com"]"#,
        ] {
            let Kind::Html(_) = rule::read(text).kind else {
                panic!("rejected {text}");
            };
        }
    }
}
