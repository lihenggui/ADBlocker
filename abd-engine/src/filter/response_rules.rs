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

//! Response rules select changes to a matched request or its response.
//! Each rule uses its number as a CSP directive to get every URL match
//! from adblock-rust. Exceptions are resolved here after checking types
//! and apps, which the CSP query does not know.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::utils::Hash;
use serde::{Deserialize, Serialize};

use super::app_patterns::{Condition, Patterns, Prepared, Scope};
use super::{Page, RequestRules};
use crate::limits::{MAX_RESPONSE_REGEX_BYTES, MAX_RESPONSE_RULE, MAX_RESPONSE_RULES};
use crate::rule::action::{ActionKind, ActionSpec, PageExceptions, ResponseAction};
use crate::rule::{self, Action, Code, ResponseRule, RuleReason, kind_bit};

/// Conditions that must hold for a match returned by the CSP query.
#[derive(Default, Serialize, Deserialize)]
struct Qualifiers {
    /// Resource-type bits that qualify, or zero when no positive type is given.
    include: u16,
    /// Resource-type bits that disqualify even when a positive type matches.
    exclude: u16,
    /// The app condition can only match a known package.
    apps: Option<Condition>,
    strict_third_party: Option<bool>,
    exclude_destinations: Vec<Hash>,
}

impl Qualifiers {
    /// Require a package identity for app options, even for negated names.
    fn matches(&self, kind: &str, context: &ResponseMatches<'_>) -> bool {
        let bit = kind_bit(kind).unwrap_or(0);
        (self.include == 0 || self.include & bit != 0)
            && self.exclude & bit == 0
            && self
                .strict_third_party
                .is_none_or(|required| required == context.strict_third_party)
            && self
                .apps
                .as_ref()
                .is_none_or(|condition| context.scope.is_some_and(|app| condition.matches(app)))
            && (self.exclude_destinations.is_empty()
                || !context
                    .destinations
                    .iter()
                    .any(|hash| self.exclude_destinations.binary_search(hash).is_ok()))
    }
}

/// A source rule before its condition and action are assigned cache indices.
struct Parsed<'a> {
    pattern: Cow<'a, str>,
    options: String,
    action: Option<ActionSpec>,
    kind: ActionKind,
    qualifiers: Qualifiers,
    apps: Option<String>,
    exception: bool,
    transform_identity: Option<(String, bool)>,
}

impl<'a> Parsed<'a> {
    fn from_rule(rule: &ResponseRule<'a>) -> Result<Self, RuleReason> {
        let Action {
            pattern,
            options,
            spec,
            kind,
            include,
            exclude,
            apps,
            strict_third_party,
            exception,
        } = rule.action()?;
        let mut parsed = Self {
            pattern,
            options: options.join(","),
            action: spec,
            kind,
            apps: apps.map(str::to_owned),
            exception,
            transform_identity: rule.transform_identity(),
            qualifiers: Qualifiers {
                include,
                exclude,
                apps: None,
                strict_third_party,
                exclude_destinations: Vec::new(),
            },
        };
        let projected = parsed.project(0);
        let network = rule::parse_network(&projected)
            .filter(|rule| rule.is_csp() && !rule.is_badfilter())
            .ok_or(Code::ResponseCondition)?;
        // The CSP index can drop a condition with only excluded destinations.
        // Defer those predicates to qualifiers, using the matcher's own hashes.
        if kind == ActionKind::UrlTransform
            && network.opt_to_domains.is_none()
            && let Some(excluded) = network.opt_not_to_domains
        {
            parsed.options = parsed
                .options
                .split(',')
                .filter(|opt| !opt.starts_with("to="))
                .collect::<Vec<_>>()
                .join(",");
            parsed.qualifiers.exclude_destinations = excluded;
        }
        Ok(parsed)
    }

    fn prepare_apps(&self, patterns: &mut Patterns) -> Option<Option<Prepared>> {
        match self.apps.as_deref() {
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
        let mut condition = format!("{}${}", self.pattern, self.options);
        if !self.options.is_empty() {
            condition.push(',');
        }
        condition.push_str(&format!("csp={id}"));
        condition
    }
}

pub(crate) fn validate(rule: &ResponseRule<'_>, patterns: &mut Patterns) -> Result<(), RuleReason> {
    let mut parsed = Parsed::from_rule(rule)?;
    if let Some(action) = &parsed.action {
        action.compile_checked()?;
    }
    let apps = parsed.prepare_apps(patterns).ok_or(Code::AppPattern)?;
    parsed.keep_apps(patterns, apps);
    Ok(())
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

/// Keep exceptions before actions, and use text order within each group.
#[derive(Default)]
pub(super) struct ResponseProjections<'a> {
    lines: BTreeMap<(bool, Cow<'a, str>), Option<Parsed<'a>>>,
}

impl<'a> ResponseProjections<'a> {
    pub(super) fn add(&mut self, line: &'a str, response: ResponseRule<'a>) {
        let line = response.normalized(line);
        if line.len() > MAX_RESPONSE_RULE {
            return;
        }
        let key = (!line.starts_with("@@"), line);
        if self.lines.len() < MAX_RESPONSE_RULES
            || self
                .lines
                .last_key_value()
                .is_some_and(|(last, _)| &key < last)
        {
            self.lines
                .entry(key)
                .or_insert_with(|| Parsed::from_rule(&response).ok());
            if self.lines.len() > MAX_RESPONSE_RULES {
                self.lines.pop_last();
            }
        }
    }

    pub(super) fn compile(self, patterns: &mut Patterns) -> Option<ResponseRules> {
        self.compile_with_budget(MAX_RESPONSE_REGEX_BYTES, patterns)
    }

    fn compile_with_budget(self, budget: usize, patterns: &mut Patterns) -> Option<ResponseRules> {
        let disabled: BTreeSet<_> = self
            .lines
            .values()
            .flatten()
            .filter_map(|rule| {
                let (identity, badfilter) = rule.transform_identity.as_ref()?;
                badfilter.then(|| identity.clone())
            })
            .collect();
        let rules = self
            .lines
            .into_values()
            .flatten()
            .filter(|rule| {
                rule.transform_identity
                    .as_ref()
                    .is_none_or(|(identity, badfilter)| !badfilter && !disabled.contains(identity))
            })
            .collect();
        ResponseRules::compile(rules, budget, patterns)
    }
}

impl ResponseRules {
    fn compile(
        mut parsed: Vec<Parsed<'_>>,
        mut budget: usize,
        patterns: &mut Patterns,
    ) -> Option<Self> {
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
    fn actions(
        &self,
        context: &ResponseMatches<'_>,
        kind: &str,
        include_transforms: bool,
    ) -> Vec<Arc<ResponseAction>> {
        let mut matched: Vec<_> = context
            .tokens
            .split(',')
            .filter_map(|token| {
                let id = token.parse::<usize>().ok()?;
                let rule = self.table.rules.get(id)?;
                let action_kind = match rule.action {
                    RuleAction::One(id) => self.compiled[id].kind(),
                    RuleAction::All(kind) => kind,
                };
                if !include_transforms
                    && matches!(
                        action_kind,
                        ActionKind::UrlTransform | ActionKind::RemoveParamException
                    )
                {
                    return None;
                }
                rule.qualifiers.matches(kind, context).then_some((id, rule))
            })
            .collect();
        if matched.is_empty() {
            return Vec::new();
        }
        matched.sort_unstable_by_key(|(id, _)| *id);
        let mut seen = vec![false; self.compiled.len()];
        let mut skipped = BTreeSet::new();
        for (_, rule) in matched.iter().filter(|(_, rule)| rule.exception) {
            match rule.action {
                RuleAction::One(id) => seen[id] = true,
                RuleAction::All(kind) => {
                    skipped.insert(kind);
                }
            }
        }
        let mut actions = Vec::new();
        for (_, rule) in matched.into_iter().filter(|(_, rule)| !rule.exception) {
            let RuleAction::One(id) = rule.action else {
                continue;
            };
            let action = &self.compiled[id];
            let kind = action.kind();
            if !kind.disabled_by(context.page) && !skipped.contains(&kind) && !seen[id] {
                seen[id] = kind.deduplicates();
                actions.push(action.clone());
            }
        }
        actions.sort_by_key(|action| action.kind());
        actions
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
    page: PageExceptions,
    strict_third_party: bool,
    destinations: Vec<Hash>,
}

impl ResponseMatches<'_> {
    pub(crate) fn actions(&self, kind: &str, include_transforms: bool) -> Vec<Arc<ResponseAction>> {
        self.rules.actions(self, kind, include_transforms)
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
            .map_or_else(Vec::new, |matched| matched.actions(kind, true))
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
        if exceptions.document {
            return None;
        }
        let request = RequestRules::request_with_page(url, page, "document", method)?;
        let tokens = rules.engine.get_csp_directives(&request)?;
        let strict_third_party = match page {
            Page::Named(source) => !source
                .hostname()
                .trim_end_matches('.')
                .eq_ignore_ascii_case(request.hostname.trim_end_matches('.')),
            Page::Itself => false,
            Page::Unnamed { cross_site } => cross_site,
        };
        Some(ResponseMatches {
            rules,
            tokens,
            scope: self.scope.as_ref(),
            page: exceptions,
            strict_third_party,
            destinations: request.hostname_hashes.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests;
