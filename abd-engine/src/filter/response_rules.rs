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
use adblock::lists::{FilterSet, ParseOptions};
use serde::{Deserialize, Serialize};

use super::app_patterns::{Condition, Patterns, Prepared, Scope};
use super::rule_options::{
    delimiter, network_condition, option_separator, options, unescape, unescaped,
};
use super::{Page, RequestRules, network};
use crate::limits::{MAX_RESPONSE_REGEX_BYTES, MAX_RESPONSE_RULE, MAX_RESPONSE_RULES};
use crate::response_action::{ActionKind, ActionSpec, ResponseAction};

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
#[derive(Default, Serialize, Deserialize)]
struct Qualifiers {
    /// Bits of `TYPES` that qualify, or zero when no positive type is given.
    include: u16,
    /// Bits of `TYPES` that disqualify even when a positive type matches.
    exclude: u16,
    /// The app condition can only match a known package.
    apps: Option<Condition>,
}

/// Whether an option belongs to the qualifiers or to the network matcher.
enum Qualifier {
    Taken,
    NetworkOption,
}

impl Qualifiers {
    /// Keep app and resource-type conditions out of the CSP rule.
    fn take(&mut self, option: &str) -> Qualifier {
        // AdGuard's reference omits $all for $jsonprune, but its lists use it.
        if option == "all" {
            return Qualifier::Taken;
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
    fn matches(&self, kind: &str, scope: Option<&Scope<'_>>) -> bool {
        let bit = kind_bit(kind).unwrap_or(0);
        (self.include == 0 || self.include & bit != 0)
            && self.exclude & bit == 0
            && self
                .apps
                .as_ref()
                .is_none_or(|condition| scope.is_some_and(|app| condition.matches(app)))
    }
}

fn action_kind(option: &str) -> Option<ActionKind> {
    match option.split('=').next()? {
        "jsonprune" => Some(ActionKind::Prune),
        "replace" => Some(ActionKind::Replace),
        _ => None,
    }
}

/// Separate a network response rule from its options and leave cosmetic rules alone.
fn split(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if crate::rule_text::is_comment(line)
        || !(line.contains("replace") || line.contains("jsonprune"))
    {
        return None;
    }
    let at = option_separator(line)?;
    let suffix = &line[at + 1..];
    if !options(suffix).any(|option| action_kind(option).is_some()) {
        return None;
    }
    network_condition(line, &line[..at]).then_some((&line[..at], suffix))
}

/// Response rules are excluded from the ordinary blocking and cosmetic engines.
pub(super) fn is_response_rule(line: &str) -> bool {
    split(line).is_some()
}

/// Find the start of the flags after both slash-delimited fields.
fn value_end(value: &str) -> Option<usize> {
    let first = delimiter(value)? + 1;
    Some(first + delimiter(&value[first..])? + 1)
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

/// Reject unsupported qualifiers before projecting away any app or type options.
fn prune_option(option: &str) -> bool {
    let (name, value) = option
        .split_once('=')
        .map_or((option, None), |(n, v)| (n, Some(v)));
    match name {
        "domain" | "from" | "app" => value.is_some_and(|v| !v.is_empty()),
        "third-party" | "3p" | "first-party" | "1p" | "~third-party" | "~3p" | "~first-party"
        | "~1p" | "all" | "important" | "match-case" | "xmlhttprequest" | "xhr" => value.is_none(),
        _ => false,
    }
}

/// A source rule before its condition and action are assigned cache indices.
struct Parsed {
    pattern: String,
    options: Vec<String>,
    action: Option<ActionSpec>,
    kind: ActionKind,
    qualifiers: Qualifiers,
    apps: Option<String>,
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
        let mut apps = None;
        for option in options(options_text) {
            if let Some(kind) = action_kind(option) {
                if found.is_some() {
                    return None;
                }
                let value = match option.split_once('=') {
                    Some((_, value)) if !value.is_empty() => Some(match kind {
                        ActionKind::Replace => action(value)?,
                        ActionKind::Prune => {
                            if unescaped(value, b'$').next().is_some() {
                                return None;
                            }
                            let expression = unescape(value, &[',', '$']);
                            crate::jsonpath::Path::parse(&expression)?;
                            ActionSpec::Prune(expression)
                        }
                    }),
                    None if exception => None,
                    Some((_, "")) if exception && kind == ActionKind::Replace => None,
                    _ => return None,
                };
                found = Some((kind, value));
            } else if let Some(names) = option.strip_prefix("app=") {
                if apps.replace(names.to_owned()).is_some() {
                    return None;
                }
            } else {
                match qualifiers.take(option) {
                    Qualifier::Taken => (),
                    Qualifier::NetworkOption => rest.push(option.to_owned()),
                }
            }
        }
        let (kind, action) = found?;
        if kind == ActionKind::Prune
            && options(options_text)
                .any(|option| action_kind(option).is_none() && !prune_option(option))
        {
            return None;
        }
        let parsed = Self {
            pattern: pattern.strip_prefix("@@").unwrap_or(pattern).to_owned(),
            options: rest,
            kind,
            action,
            qualifiers,
            apps,
            exception,
        };
        let projected = parsed.project(0);
        network(&projected).filter(|rule| rule.is_csp() && !rule.is_badfilter())?;
        Some(parsed)
    }

    fn prepare_apps(&self, patterns: &mut Patterns) -> Option<Option<Prepared>> {
        match &self.apps {
            Some(names) => Some(Some(patterns.prepare(names)?)),
            None => Some(None),
        }
    }

    fn keep_apps(&mut self, patterns: &mut Patterns, prepared: Option<Prepared>) {
        if let Some(prepared) = prepared {
            self.qualifiers.apps = Some(Condition::new(&prepared.encoded));
            patterns.keep(prepared);
        }
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
pub(crate) fn check(line: &str, patterns: &mut Patterns) -> Option<bool> {
    let (pattern, options) = split(line)?;
    Some(Parsed::parse(pattern, options).is_some_and(|mut parsed| {
        if !parsed
            .action
            .as_ref()
            .is_none_or(|action| action.compile().is_some())
        {
            return false;
        }
        let Some(apps) = parsed.prepare_apps(patterns) else {
            return false;
        };
        parsed.keep_apps(patterns, apps);
        true
    }))
}

/// What remains to be checked for a numbered condition returned by the engine.
#[derive(Serialize, Deserialize)]
struct Rule {
    qualifiers: Qualifiers,
    /// The action index or the kind suppressed by an unvalued exception.
    action: RuleAction,
    exception: bool,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
enum RuleAction {
    One(usize),
    All(ActionKind),
}

/// The action specs and numbered conditions stored alongside the engine.
#[derive(Serialize, Deserialize)]
struct Table {
    actions: Vec<ActionSpec>,
    /// In alphabetical rule order, preserved within each action kind.
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
    pub(super) fn build<'a>(
        sources: impl Iterator<Item = &'a str>,
        patterns: &mut Patterns,
    ) -> Option<Self> {
        Self::build_with_budget(sources, MAX_RESPONSE_REGEX_BYTES, patterns)
    }

    fn build_with_budget<'a>(
        sources: impl Iterator<Item = &'a str>,
        mut budget: usize,
        patterns: &mut Patterns,
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
        let mut parsed: Vec<_> = lines
            .into_iter()
            .filter_map(|line| {
                let (pattern, options) = split(line)?;
                Parsed::parse(pattern, options)
            })
            .collect();
        let mut ids = HashMap::new();
        let mut actions = Vec::new();
        let mut compiled = Vec::new();
        parsed.retain_mut(|rule| {
            if rule.exception {
                return true;
            }
            let Some(apps) = rule.prepare_apps(patterns) else {
                return false;
            };
            let spec = rule
                .action
                .as_ref()
                .expect("a blocking response rule has an action");
            let id = ids.entry(spec.clone()).or_insert_with(|| {
                let action = compile_with_budget(spec, &mut budget)?;
                let id = actions.len();
                actions.push(spec.clone());
                compiled.push(action);
                Some(id)
            });
            if id.is_none() {
                return false;
            }
            rule.keep_apps(patterns, apps);
            true
        });
        if actions.is_empty() {
            return None;
        }
        let mut conditions = String::new();
        let mut rules = Vec::new();
        for mut p in parsed {
            let action = match &p.action {
                Some(spec) => match ids.get(spec) {
                    Some(Some(id)) => RuleAction::One(*id),
                    _ => continue,
                },
                None => RuleAction::All(p.kind),
            };
            if p.exception {
                let Some(apps) = p.prepare_apps(patterns) else {
                    continue;
                };
                p.keep_apps(patterns, apps);
            }
            conditions.push_str(&p.project(rules.len()));
            conditions.push('\n');
            rules.push(Rule {
                qualifiers: p.qualifiers,
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
                RuleAction::One(id) => id >= table.actions.len(),
                RuleAction::All(_) => !rule.exception,
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

    /// Resolve exceptions before applying any actions. Unvalued exceptions
    /// suppress only their own kind.
    pub(super) fn actions(
        &self,
        tokens: &str,
        kind: &str,
        scope: Option<&Scope<'_>>,
        skip_prune: bool,
    ) -> Vec<Arc<ResponseAction>> {
        let mut matched: Vec<_> = tokens
            .split(',')
            .filter_map(|token| {
                let id = token.parse::<usize>().ok()?;
                let rule = self.table.rules.get(id)?;
                rule.qualifiers.matches(kind, scope).then_some((id, rule))
            })
            .collect();
        if matched.is_empty() {
            return Vec::new();
        }
        matched.sort_unstable_by_key(|(id, _)| *id);
        let mut seen = vec![false; self.compiled.len()];
        let mut skip_prune = skip_prune;
        let mut skip_replace = false;
        for (_, rule) in matched.iter().filter(|(_, rule)| rule.exception) {
            match rule.action {
                RuleAction::One(id) => seen[id] = true,
                RuleAction::All(ActionKind::Prune) => skip_prune = true,
                RuleAction::All(ActionKind::Replace) => skip_replace = true,
            }
        }
        let mut prune = Vec::new();
        let mut replace = Vec::new();
        for (_, rule) in matched.into_iter().filter(|(_, rule)| !rule.exception) {
            let RuleAction::One(id) = rule.action else {
                continue;
            };
            let action = &self.compiled[id];
            match action.kind() {
                ActionKind::Prune if !skip_prune && !seen[id] => prune.push(action.clone()),
                ActionKind::Replace if !skip_replace && !std::mem::replace(&mut seen[id], true) => {
                    replace.push(action.clone());
                }
                _ => (),
            }
        }
        // AdGuard does not specify an order between kinds. Prune JSON
        // first so replacements can operate on the remaining text.
        prune.extend(replace);
        prune
    }
}

/// Charge the compiled regex heap once per distinct action.
fn compile_with_budget(spec: &ActionSpec, remaining: &mut usize) -> Option<Arc<ResponseAction>> {
    let action = spec.compile()?;
    *remaining = remaining.checked_sub(action.regex_bytes())?;
    Some(Arc::new(action))
}

/// The matching URL conditions before applying type qualifiers and exceptions.
pub(crate) struct ResponseMatches<'a> {
    rules: &'a ResponseRules,
    tokens: String,
    scope: Option<&'a Scope<'a>>,
    skip_prune: bool,
}

impl ResponseMatches<'_> {
    pub(crate) fn actions(&self, kind: &str) -> Vec<Arc<ResponseAction>> {
        self.rules
            .actions(&self.tokens, kind, self.scope, self.skip_prune)
    }
}

impl super::AppFilters<'_> {
    #[cfg(test)]
    pub(crate) fn response_actions(
        &self,
        url: &str,
        page: Page<'_>,
        kind: &str,
        method: &str,
    ) -> Vec<Arc<ResponseAction>> {
        self.response_matches(url, page, method)
            .map_or_else(Vec::new, |matched| matched.actions(kind))
    }

    /// Resolve page exceptions before looking up the URL in the request context.
    pub(crate) fn response_matches(
        &self,
        url: &str,
        page: Page<'_>,
        method: &str,
    ) -> Option<ResponseMatches<'_>> {
        let rules = self.filters.responses.as_ref()?;
        let exceptions = self.filters.page_exceptions.response(url, page, method);
        if exceptions.all {
            return None;
        }
        let request = RequestRules::request_with_page(url, page, "document", method)?;
        let tokens = rules.engine.get_csp_directives(&request)?;
        Some(ResponseMatches {
            rules,
            tokens,
            scope: self.scope.as_ref(),
            skip_prune: exceptions.prune,
        })
    }
}

#[cfg(test)]
mod tests;
