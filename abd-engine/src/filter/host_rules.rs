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

//! Projects unconditional host rules and the guards that defer early blocks.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use adblock::filters::network::{NetworkFilter, NetworkFilterMask as Mask};
use adblock::lists::{FilterSet, ParseOptions};

use super::{domain_named, is_network_exception, network, option_name, opts, to_domains};
use crate::{host::suffixes, public_suffix};

/// Whether `host` is a name somebody registered, or one below such a name.
///
/// A rule for anything wider, `||com^` or `||co.uk^`, would have every
/// connection of the filtered apps to that part of the internet refused:
/// no list means that, and one that says it is mistaken or hostile.
fn registrable(host: &str) -> bool {
    let plain = host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    plain
        // A suffix the list of public ones does not know is taken for one
        // label, so a name below it still has a root.
        && public_suffix::Name::parse(host).is_some_and(|name| name.root().is_some())
}

/// `||host^` with nothing else, or a hosts-file line; and what takes
/// such a rule back, an exception (`@@||host^`) or `$badfilter`, without
/// which a host the lists allow after all would still be refused whole.
pub(super) fn host_only(line: &str) -> Option<String> {
    whole_rule(line).map(|(rule, _)| rule.into_owned())
}

pub(super) fn whole_rule(line: &str) -> Option<(Cow<'_, str>, &str)> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("@@").unwrap_or(line).strip_prefix("||") {
        let rest = rest.strip_suffix("$badfilter").unwrap_or(rest);
        let host = rest.strip_suffix('^')?;
        return registrable(host).then_some((Cow::Borrowed(line), host));
    }
    let mut f = line.split_ascii_whitespace();
    match (f.next(), f.next(), f.next()) {
        (Some("0.0.0.0" | "127.0.0.1"), Some(host), None) if registrable(host) => {
            Some((Cow::Owned(format!("||{host}^")), host))
        }
        _ => None,
    }
}

fn name_matches_host_pattern(host: &str) -> bool {
    !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// What `pattern` begins with, up to where a host name would end.
fn host_token(pattern: &str) -> &str {
    &pattern[..pattern
        .find(['/', '^', '?', ':', '|', '$'])
        .unwrap_or(pattern.len())]
}

/// Whether `host` is a host pattern with a wildcard, such as `google.*`
/// or `cdn*.site.example`, and not a wildcard alone.
fn wildcard_host(host: &str) -> bool {
    host.contains('*')
        && host.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'*')
        })
        && host.bytes().any(|b| b.is_ascii_alphanumeric())
}

/// Writes to `out` a guard for the host that the exception `line` names,
/// at least as wide as the host part of the exception, and returns whether
/// it names one. An exception names a host by a host-anchored pattern, or
/// by a pattern that begins with a host name.
fn exception_host(line: &str, rule: &NetworkFilter<'_>, out: &mut String) -> bool {
    if let Some(pattern) = line.strip_prefix("@@||") {
        let token = host_token(pattern);
        if wildcard_host(token) {
            push_guard(out, token, "^");
            return true;
        }
        let Some(name) = rule.hostname.as_deref() else {
            return false;
        };
        if rule.mask.contains(Mask::IS_HOSTNAME_REGEX) || !name_matches_host_pattern(name) {
            return false;
        }
        // Without a separator the rule can also match part of a hostname.
        let suffix = (rule.mask.contains(Mask::IS_RIGHT_ANCHOR) && rule.filter.iter().len() == 0)
            || (rule.mask.contains(Mask::IS_LEFT_ANCHOR)
                && rule
                    .filter
                    .iter()
                    .next()
                    .is_some_and(|path| path.starts_with('/')));
        push_guard(out, name, if suffix { "^" } else { "" });
        return true;
    }
    let Some(pattern) = line.strip_prefix("@@") else {
        return false;
    };
    let pattern = pattern.strip_prefix('|').unwrap_or(pattern);
    let authority = ["https://", "http://", "://"]
        .into_iter()
        .find_map(|scheme| pattern.strip_prefix(scheme));
    // A leading hostname counts for its own host even when the pattern
    // could also match in the path on another host.
    let token = host_token(authority.unwrap_or(pattern).trim_start_matches('/'));
    if authority.is_some()
        && let Some(label) = token.strip_suffix('.')
        && !label.contains('.')
        && name_matches_host_pattern(label)
        && label.bytes().any(|b| b.is_ascii_lowercase())
    {
        push_guard(out, token, "*^");
        return true;
    }
    let absolute = rule.mask.contains(Mask::IS_LEFT_ANCHOR)
        && (pattern.starts_with("https://") || pattern.starts_with("http://"))
        && authority.is_some_and(|authority| !authority.starts_with('/'));
    if name_matches_host_pattern(token) && (absolute || has_known_suffix(token)) {
        push_guard(out, token, "^");
        return true;
    }
    false
}

/// Whether `host` ends in a top-level name, which tells a host name at the
/// beginning of a pattern from a file name such as `ads.js`.
fn has_known_suffix(host: &str) -> bool {
    host.rsplit_once('.').is_some_and(|(_, suffix)| {
        suffix.len() >= 2
            && suffix.bytes().all(|b| b.is_ascii_lowercase())
            && public_suffix::is_known_top_level(suffix)
    })
}

fn push_guard(out: &mut String, name: &str, ending: &str) {
    out.push_str("@@||");
    out.push_str(name);
    out.push_str(ending);
    out.push('\n');
}

pub(super) struct HostRules {
    pub(super) whole: FilterSet,
    /// The hosts that exceptions name.
    pub(super) guards: FilterSet,
    /// The same of the exceptions for named pages, which are kept apart:
    /// they can apply only to an app that shows pages.
    pub(super) page_guards: Option<FilterSet>,
    /// The hosts that exceptions name and only a rule for apps blocks
    /// whole: guards for those apps, in the two kinds above.
    pub(super) app_guards: Option<FilterSet>,
    pub(super) app_page_guards: Option<FilterSet>,
    /// The budget left guards out: no host may be refused early.
    pub(super) guards_incomplete: bool,
    pub(super) count: usize,
}

/// The hosts that rules for apps block whole, each with the ending that
/// gives a guard the apps of its rule.
pub(super) type AppHostBlocks = HashMap<String, Vec<String>>;

impl HostRules {
    pub(super) fn project<'a>(
        lists: impl Iterator<Item = &'a str>,
        disabled: &HashSet<u64>,
        app_blocks: &AppHostBlocks,
        guard_budget: usize,
    ) -> Self {
        let mut hosts = FilterSet::new(false);
        let mut blocked = HashSet::new();
        let mut host_rules = 0;
        let mut scope_rules = String::new();
        let mut page_rules = String::new();
        for text in lists {
            let mut only = String::new();
            for line in text.lines().map(str::trim) {
                let whole = whole_rule(line);
                let exception = line.starts_with("@@");
                let Some(rule) = whole
                    .as_ref()
                    .map(|(candidate, _)| candidate.as_ref())
                    .or(exception.then_some(line))
                    .and_then(network)
                else {
                    continue;
                };
                if disabled.contains(&rule.get_id()) {
                    continue;
                }
                if let Some((candidate, original)) = &whole
                    && !rule.is_badfilter()
                {
                    only.push_str(candidate);
                    only.push('\n');
                    host_rules += 1;
                    if !exception && let Some(name) = rule.hostname.as_deref() {
                        // Reuse the source text except where the parser normalized it.
                        blocked.insert(if name == *original {
                            Cow::Borrowed(*original)
                        } else {
                            Cow::Owned(name.to_owned())
                        });
                    }
                }
                if exception && guards_exception(line, &rule) {
                    let out = if rule.opt_domains.is_some() {
                        &mut page_rules
                    } else {
                        &mut scope_rules
                    };
                    project_guard(line, &rule, out);
                }
            }
            hosts.add_filter_list(only, ParseOptions::default());
        }
        let mut remaining = guard_budget;
        let kept =
            kept_guards(&scope_rules, &blocked, app_blocks, &mut remaining).and_then(|scope| {
                let pages = kept_guards(&page_rules, &blocked, app_blocks, &mut remaining)?;
                Some((scope, pages))
            });
        let guards_incomplete = kept.is_none();
        let (scope, pages) = kept.unwrap_or_default();
        Self {
            whole: hosts,
            guards: guard_set(scope.shared),
            page_guards: optional_guard_set(pages.shared),
            app_guards: optional_guard_set(scope.apps),
            app_page_guards: optional_guard_set(pages.apps),
            guards_incomplete,
            count: host_rules,
        }
    }
}

fn guard_set(rules: String) -> FilterSet {
    let mut set = FilterSet::new(false);
    set.add_filter_list(rules, ParseOptions::default());
    set
}

fn optional_guard_set(rules: String) -> Option<FilterSet> {
    (!rules.is_empty()).then(|| guard_set(rules))
}

/// The guards that stand: for every app, and for the apps of a rule.
#[derive(Default)]
struct KeptGuards {
    shared: String,
    apps: String,
}

/// Of the guard `candidates`, those whose own host a rule blocks whole. An
/// exception for a host that no rule blocks whole is not one to a
/// whole-host block: it does not keep a host below the same domain from
/// being refused. `None` when the kept guards exceed what `remaining`
/// allows.
fn kept_guards(
    candidates: &str,
    blocked: &HashSet<Cow<'_, str>>,
    app_blocks: &AppHostBlocks,
    remaining: &mut usize,
) -> Option<KeptGuards> {
    let mut kept = KeptGuards::default();
    for candidate in candidates.lines() {
        // These lines come from push_guard, so the host can be read
        // without parsing the list rule again.
        let host = candidate.strip_prefix("@@||")?.trim_end_matches('^');
        if host.contains('*') || block_suffixes(host).any(|name| blocked.contains(name)) {
            let before = kept.shared.len();
            push_guard(&mut kept.shared, host, "^");
            *remaining = remaining.checked_sub(kept.shared.len() - before)?;
        } else if !app_blocks.is_empty() {
            // A web exception may still take back a whole-host block for
            // an app. Keep it with that block's app condition.
            for ending in block_suffixes(host)
                .filter_map(|name| app_blocks.get(name))
                .flatten()
            {
                let before = kept.apps.len();
                push_guard(&mut kept.apps, host, ending);
                *remaining = remaining.checked_sub(kept.apps.len() - before)?;
            }
        }
    }
    Some(kept)
}

/// `host` and the names above it down to the registrable one: the hosts
/// of the whole-host rules that would block it.
fn block_suffixes(host: &str) -> impl Iterator<Item = &str> {
    let minimum = public_suffix::Name::parse(host)
        .and_then(|name| name.root())
        .map_or(usize::MAX, str::len);
    suffixes(host).take_while(move |name| name.len() >= minimum)
}

/// Whether `rule` is an exception that could allow an HTTPS request, and
/// so one a host may have to be left alone for.
pub(super) fn guards_exception(line: &str, rule: &NetworkFilter<'_>) -> bool {
    is_network_exception(rule)
        && rule.mask.contains(Mask::FROM_HTTPS)
        && !(rule.mask.contains(Mask::IS_LEFT_ANCHOR) && line.starts_with("@@|http://"))
        && !opts(line).any(|option| option_name(option) == "tag")
}

/// What an exception was found to guard.
pub(super) enum GuardCoverage {
    /// The hosts written out as guards, which may be none.
    NamedHosts,
    /// It names no host, and can apply to any. Among the rules for every
    /// app this guards nothing; an exception for an app that names no
    /// host is meant to open every host to that app (`app_rules.rs`).
    Unbounded,
}

/// Writes the guards of the exception `line` to `scope_rules`.
pub(super) fn project_guard(
    line: &str,
    rule: &NetworkFilter<'_>,
    scope_rules: &mut String,
) -> GuardCoverage {
    if exception_host(line, rule, scope_rules) {
        GuardCoverage::NamedHosts
    } else if let Some(domains) = &rule.opt_to_domains {
        let mut scope = to_domains(line)
            .filter(|domain| name_matches_host_pattern(domain))
            .filter(|domain| domain_named(domains, domain))
            .collect::<Vec<_>>();
        scope.sort_unstable();
        scope.dedup();
        for domain in &scope {
            push_guard(scope_rules, domain, "^");
        }
        if scope.len() == domains.len() {
            GuardCoverage::NamedHosts
        } else {
            GuardCoverage::Unbounded
        }
    } else {
        GuardCoverage::Unbounded
    }
}
