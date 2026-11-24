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

//! Response rules decide what to change in the body of a matched request.
//! Each rule uses its number as a CSP directive to get every URL match
//! from adblock-rust. Exceptions are resolved here after checking types
//! and apps, which the CSP query does not know.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use adblock::Engine;
use adblock::lists::{FilterParseError, FilterSet, ParseOptions, RuleTypes, parse_filter};
use adblock::request::Request;
use serde::{Deserialize, Serialize};

use super::{Page, RequestRules, app_names, app_package, network};
use crate::limits::{MAX_RESPONSE_REGEX_BYTES, MAX_RESPONSE_RULE, MAX_RESPONSE_RULES};
use crate::response_action::{ActionSpec, ResponseAction};

/// The order of these names defines the resource-type bits in the cache.
const TYPES: &[&str] = &[
    "script",
    "xmlhttprequest",
    "document",
    "subdocument",
    "image",
    "stylesheet",
    "font",
    "media",
    "object",
    "other",
    "websocket",
    "ping",
];

/// Give aliases of a resource type the same bit.
fn kind_bit(kind: &str) -> Option<u16> {
    let kind = match kind {
        "xhr" => "xmlhttprequest",
        "doc" => "document",
        "css" => "stylesheet",
        _ => kind,
    };
    TYPES.iter().position(|name| *name == kind).map(|i| 1 << i)
}

/// Conditions that must hold for a match returned by the CSP query.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Qualifiers {
    /// Bits of `TYPES` that qualify, or zero when no positive type is given.
    include: u16,
    /// Bits of `TYPES` that disqualify even when a positive type matches.
    exclude: u16,
    /// Lowercase package patterns, which can only match a known package.
    apps: Option<String>,
}

/// Whether an option belongs to the qualifiers or to the network matcher.
enum Qualifier {
    Taken,
    NetworkOption,
    Invalid,
}

impl Qualifiers {
    /// Keep app and resource-type conditions out of the CSP rule.
    fn take(&mut self, option: &str) -> Qualifier {
        if let Some(names) = option.strip_prefix("app=") {
            if self.apps.is_some() {
                return Qualifier::Invalid;
            }
            self.apps = app_names(names);
            return if self.apps.is_some() {
                Qualifier::Taken
            } else {
                Qualifier::Invalid
            };
        }
        let (name, negated) = option
            .strip_prefix('~')
            .map_or((option, false), |name| (name, true));
        let Some(bit) = kind_bit(name) else {
            return Qualifier::NetworkOption;
        };
        if negated {
            self.exclude |= bit;
        } else {
            self.include |= bit;
        }
        Qualifier::Taken
    }

    /// Require a package identity for app options, even for negated names.
    fn matches(&self, kind: &str, package: Option<&str>) -> bool {
        let bit = kind_bit(kind).unwrap_or(0);
        (self.include == 0 || self.include & bit != 0)
            && self.exclude & bit == 0
            && self
                .apps
                .as_ref()
                .is_none_or(|names| package.is_some_and(|name| matches_app(names, name)))
    }
}

/// An excluded package wins over an included one.
fn matches_app(names: &str, package: &str) -> bool {
    let mut positive = false;
    let mut included = false;
    for name in names.split('|') {
        if let Some(name) = name.strip_prefix('~') {
            if name.eq_ignore_ascii_case(package) {
                return false;
            }
        } else {
            positive = true;
            included |= name.eq_ignore_ascii_case(package);
        }
    }
    !positive || included
}

/// AdGuard quotes option separators with an immediately preceding backslash.
fn unescaped(text: &str, byte: u8) -> impl DoubleEndedIterator<Item = usize> {
    text.bytes().enumerate().filter_map(move |(i, b)| {
        (b == byte && (i == 0 || text.as_bytes()[i - 1] != b'\\')).then_some(i)
    })
}

/// Split options after skipping the two slash-delimited replacement fields.
fn options(text: &str) -> impl Iterator<Item = &str> {
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

fn is_action(option: &str) -> bool {
    option == "replace" || option.starts_with("replace=")
}

/// Separate a network response rule from its options and leave cosmetic rules alone.
fn split(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if crate::rule_text::is_comment(line) || !line.contains("replace") {
        return None;
    }
    let at = option_separator(line)?;
    let suffix = &line[at + 1..];
    if !options(suffix).any(is_action) {
        return None;
    }
    let parse = ParseOptions {
        rule_types: RuleTypes::CosmeticOnly,
        ..ParseOptions::default()
    };
    (!line[..at].contains('#')
        || matches!(
            parse_filter(line, false, parse),
            Err(FilterParseError::Unsupported)
        ))
    .then_some((&line[..at], suffix))
}

/// A regular expression that forms the condition can contain dollar signs.
fn regex_option_separator(line: &str) -> Option<usize> {
    let pattern = line.strip_prefix("@@").unwrap_or(line);
    let close = delimiter(pattern.strip_prefix('/')?)?;
    let after = line.len() - pattern.len() + close + 2;
    (line.as_bytes().get(after) == Some(&b'$')).then_some(after)
}

/// Find the last unquoted dollar outside a regular expression condition or
/// a replacement value. The value parser checks dollar quoting separately.
fn option_separator(line: &str) -> Option<usize> {
    let mut cursor = regex_option_separator(line).unwrap_or(0);
    let mut last = None;
    let bytes = line.as_bytes();
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        cursor += 1;
        if cursor >= 2 && bytes[cursor - 2] == b'\\' {
            continue;
        }
        if byte == b'$' {
            last = Some(cursor - 1);
        } else if byte != b',' || last.is_none() {
            continue;
        }
        if let Some(value) = line[cursor..].strip_prefix("replace=/") {
            cursor = line.len() - value.len() + option_value_end(value);
        }
    }
    last
}

/// Response rules are excluded from the ordinary blocking and cosmetic engines.
pub(super) fn is_response_rule(line: &str) -> bool {
    split(line).is_some()
}

/// Skip both fields. uBlock accepts a raw comma within a field when
/// it cannot begin another option.
fn option_value_end(value: &str) -> usize {
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
fn delimiter(part: &str) -> Option<usize> {
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

/// Find the start of the flags after both slash-delimited fields.
fn value_end(value: &str) -> Option<usize> {
    let first = delimiter(value)? + 1;
    Some(first + delimiter(&value[first..])? + 1)
}

/// Remove only the quoting of this layer and leave regex escapes intact.
fn unescape(part: &str, chars: &[char]) -> String {
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

/// Decode a replacement after checking its quoting and optional flags.
fn action(value: &str) -> Option<ActionSpec> {
    if unescaped(value, b'$').next().is_some() {
        return None;
    }
    let value = unescape(value, &[',', '$']);
    let value = value.strip_prefix('/')?;
    let first = delimiter(value)?;
    let end = value_end(value)?;
    let flags = &value[end..];
    flags
        .bytes()
        .all(|b| matches!(b, b'i' | b's'))
        .then(|| ActionSpec::Replace {
            pattern: value[..first].to_owned(),
            replacement: unescape(&value[first + 1..end - 1], &['/']),
            insensitive: flags.contains('i'),
            single_line: flags.contains('s'),
        })
}

/// A source rule before its condition and action are assigned cache indices.
struct Parsed {
    pattern: String,
    options: Vec<String>,
    action: Option<ActionSpec>,
    qualifiers: Qualifiers,
    exception: bool,
}

impl Parsed {
    /// Separate the condition, qualifiers and action before engine compilation.
    fn parse(pattern: &str, options_text: &str) -> Option<Self> {
        if pattern.len() + options_text.len() >= MAX_RESPONSE_RULE {
            return None;
        }
        let exception = pattern.starts_with("@@");
        let mut qualifiers = Qualifiers::default();
        let mut rest = Vec::new();
        let mut found = None;
        for option in options(options_text) {
            if is_action(option) {
                if found.is_some() {
                    return None;
                }
                found = Some(match option.strip_prefix("replace=") {
                    Some(value) if !value.is_empty() => Some(action(value)?),
                    _ if exception => None,
                    _ => return None,
                });
            } else {
                match qualifiers.take(option) {
                    Qualifier::Taken => (),
                    Qualifier::NetworkOption => rest.push(option.to_owned()),
                    Qualifier::Invalid => return None,
                }
            }
        }
        let parsed = Self {
            pattern: pattern.strip_prefix("@@").unwrap_or(pattern).to_owned(),
            options: rest,
            action: found?,
            qualifiers,
            exception,
        };
        let projected = parsed.project(0);
        network(&projected).filter(|rule| rule.is_csp() && !rule.is_badfilter())?;
        Some(parsed)
    }

    /// Write a condition with a directive that indexes its entry in the table.
    fn project(&self, id: usize) -> String {
        let mut condition = format!("{}${}", self.pattern, self.options.join(","));
        if !self.options.is_empty() {
            condition.push(',');
        }
        condition.push_str(&format!("csp={id}"));
        condition
    }
}

/// Distinguish an invalid response rule from a line belonging to another parser.
pub(crate) fn check(line: &str) -> Option<bool> {
    let (pattern, options) = split(line)?;
    Some(
        Parsed::parse(pattern, options)
            .is_some_and(|p| p.action.is_none_or(|action| action.compile().is_some())),
    )
}

/// What remains to be checked for a numbered condition returned by the engine.
#[derive(Serialize, Deserialize)]
struct Rule {
    qualifiers: Qualifiers,
    /// The action index, absent only on an exception that takes back every action.
    action: Option<usize>,
    exception: bool,
}

/// The action specs and numbered conditions stored alongside the engine.
#[derive(Serialize, Deserialize)]
struct Table {
    actions: Vec<ActionSpec>,
    /// In alphabetical rule order, which is also the order in which actions run.
    rules: Vec<Rule>,
}

/// The condition matcher and the actions it can select.
pub(super) struct ResponseRules {
    pub(super) engine: Engine,
    table: Table,
    /// In the order of the specs in the table.
    compiled: Vec<Arc<ResponseAction>>,
}

impl ResponseRules {
    /// Keep the alphabetically first rules, subject to the count and memory budgets.
    pub(super) fn build<'a>(sources: impl Iterator<Item = &'a str>) -> Option<Self> {
        Self::build_with_budget(sources, MAX_RESPONSE_REGEX_BYTES)
    }

    fn build_with_budget<'a>(
        sources: impl Iterator<Item = &'a str>,
        mut budget: usize,
    ) -> Option<Self> {
        let mut lines = BTreeSet::new();
        for line in sources.flat_map(str::lines).map(str::trim) {
            if line.len() <= MAX_RESPONSE_RULE && is_response_rule(line) {
                lines.insert(line);
                if lines.len() > MAX_RESPONSE_RULES {
                    lines.pop_last();
                }
            }
        }
        let parsed: Vec<_> = lines
            .into_iter()
            .filter_map(|line| {
                let (pattern, options) = split(line)?;
                Parsed::parse(pattern, options)
            })
            .collect();
        let mut ids = HashMap::new();
        let mut actions = Vec::new();
        let mut compiled = Vec::new();
        for spec in parsed
            .iter()
            .filter(|p| !p.exception)
            .filter_map(|p| p.action.as_ref())
        {
            ids.entry(spec).or_insert_with(|| {
                let action = compile_with_budget(spec, &mut budget)?;
                let id = actions.len();
                actions.push(spec.clone());
                compiled.push(action);
                Some(id)
            });
        }
        if actions.is_empty() {
            return None;
        }
        let mut conditions = String::new();
        let mut rules = Vec::new();
        for p in &parsed {
            let action = match &p.action {
                Some(spec) => match ids.get(spec) {
                    Some(Some(id)) => Some(*id),
                    _ => continue,
                },
                None => None,
            };
            conditions.push_str(&p.project(rules.len()));
            conditions.push('\n');
            rules.push(Rule {
                qualifiers: p.qualifiers.clone(),
                action,
                exception: p.exception,
            });
        }
        let mut set = FilterSet::new(false);
        set.add_filter_list(conditions, ParseOptions::default());
        Some(Self {
            engine: Engine::new_with_filter_set(set),
            table: Table { actions, rules },
            compiled,
        })
    }

    /// Keep action data beside the engine so the source lists need not be read again.
    pub(super) fn serialized(&self) -> Option<Vec<u8>> {
        serde_json::to_vec(&self.table).ok()
    }

    /// Recompile cached actions under the same budget as the source lists.
    pub(super) fn cached(engine: Engine, bytes: &[u8]) -> Option<Self> {
        let table: Table = serde_json::from_slice(bytes).ok()?;
        if table.rules.len() > MAX_RESPONSE_RULES
            || table.actions.is_empty()
            || table.actions.len() > table.rules.len()
            || table.rules.iter().any(|rule| match rule.action {
                Some(id) => id >= table.actions.len(),
                None => !rule.exception,
            })
        {
            return None;
        }
        let mut budget = MAX_RESPONSE_REGEX_BYTES;
        let compiled = table
            .actions
            .iter()
            .map(|spec| compile_with_budget(spec, &mut budget))
            .collect::<Option<_>>()?;
        Some(Self {
            engine,
            table,
            compiled,
        })
    }

    /// Return distinct actions in source rule order. A matching exception
    /// with a value removes that action. One without a value removes all.
    pub(super) fn actions(
        &self,
        request: &Request,
        kind: &str,
        app: &str,
    ) -> Vec<Arc<ResponseAction>> {
        let Some(tokens) = self.engine.get_csp_directives(request) else {
            return Vec::new();
        };
        let package = app_package(app);
        let mut matched: Vec<_> = tokens
            .split(',')
            .filter_map(|token| {
                let id = token.parse::<usize>().ok()?;
                let rule = self.table.rules.get(id)?;
                rule.qualifiers.matches(kind, package).then_some((id, rule))
            })
            .collect();
        matched.sort_unstable_by_key(|(id, _)| *id);
        let mut seen = vec![false; self.compiled.len()];
        for (_, rule) in matched.iter().filter(|(_, rule)| rule.exception) {
            let Some(id) = rule.action else {
                return Vec::new();
            };
            seen[id] = true;
        }
        matched
            .into_iter()
            .filter(|(_, rule)| !rule.exception)
            .filter_map(|(_, rule)| {
                let id = rule.action?;
                (!std::mem::replace(&mut seen[id], true)).then(|| self.compiled[id].clone())
            })
            .collect()
    }
}

/// Charge each expression once, including the auxiliary compiled matchers.
fn compile_with_budget(spec: &ActionSpec, remaining: &mut usize) -> Option<Arc<ResponseAction>> {
    let action = spec.compile()?;
    *remaining = remaining.checked_sub(action.regex_bytes())?;
    Some(Arc::new(action))
}

impl super::AppFilters<'_> {
    /// Resolve page exceptions before asking for actions in the request context.
    pub(crate) fn response_actions(
        &self,
        url: &str,
        page: Page<'_>,
        kind: &str,
        method: &str,
    ) -> Vec<Arc<ResponseAction>> {
        let Some(rules) = &self.filters.responses else {
            return Vec::new();
        };
        if self
            .filters
            .page_exceptions
            .excepts_response(url, page, method)
        {
            return Vec::new();
        }
        RequestRules::request_with_page(url, page, "document", method)
            .map_or_else(Vec::new, |request| {
                rules.actions(&request, kind, self.app_name)
            })
    }
}

#[cfg(test)]
mod tests;
