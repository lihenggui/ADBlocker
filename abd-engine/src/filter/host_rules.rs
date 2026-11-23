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

use std::collections::HashSet;

use adblock::filters::network::{NetworkFilter, NetworkFilterMask as Mask};
use adblock::lists::{FilterSet, ParseOptions};

use super::{domain_named, is_network_exception, network, option_name, opts, to_domains};
use crate::list_files::MAX_LISTS;

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
        && crate::public_suffix::Name::parse(host).is_some_and(|name| name.root().is_some())
}

/// `||host^` with nothing else, or a hosts-file line; and what takes
/// such a rule back, an exception (`@@||host^`) or `$badfilter`, without
/// which a host the lists allow after all would still be refused whole.
pub(super) fn host_only(line: &str) -> Option<String> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("@@").unwrap_or(line).strip_prefix("||") {
        let rest = rest.strip_suffix("$badfilter").unwrap_or(rest);
        let host = rest.strip_suffix('^')?;
        return registrable(host).then(|| line.to_owned());
    }
    let mut f = line.split_ascii_whitespace();
    match (f.next(), f.next(), f.next()) {
        (Some("0.0.0.0" | "127.0.0.1"), Some(host), None) if registrable(host) => {
            Some(format!("||{host}^"))
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

/// A necessary destination for an exception that starts with a specific
/// name. A hostname anchor without a final separator can also match at
/// the beginning or in the middle of a name.
fn exception_host<'a>(line: &'a str, rule: &'a NetworkFilter<'_>) -> Option<(&'a str, bool)> {
    if rule.mask.contains(Mask::IS_HOSTNAME_ANCHOR) {
        let name = rule.hostname.as_deref()?;
        if !rule.mask.contains(Mask::IS_HOSTNAME_REGEX) && name_matches_host_pattern(name) {
            let suffix = (rule.mask.contains(Mask::IS_RIGHT_ANCHOR)
                && rule.filter.iter().len() == 0)
                || (rule.mask.contains(Mask::IS_LEFT_ANCHOR)
                    && rule
                        .filter
                        .iter()
                        .next()
                        .is_some_and(|path| path.starts_with('/')));
            return Some((name, suffix));
        }
    }
    if rule.mask.contains(Mask::IS_LEFT_ANCHOR) {
        let pattern = line.strip_prefix("@@|")?;
        let authority = pattern
            .strip_prefix("https://")
            .or_else(|| pattern.strip_prefix("http://"))?;
        let end = authority.find(['/', ':', '^', '?', '|'])?;
        let host = &authority[..end];
        // A port or path terminates the hostname. A question mark can
        // also occur as a separator in the query of an absolute URL.
        if name_matches_host_pattern(host) {
            return Some((host, true));
        }
    }
    None
}

pub(super) struct HostRules {
    pub(super) whole: FilterSet,
    /// Absent when an exception can apply to any host.
    pub(super) guards: Option<FilterSet>,
    pub(super) count: usize,
}

impl HostRules {
    pub(super) fn project<'a>(
        lists: impl Iterator<Item = &'a str>,
        disabled: &HashSet<u64>,
    ) -> Self {
        let mut hosts = FilterSet::new(false);
        let mut guards = FilterSet::new(false);
        let mut host_rules = 0;
        let mut guard_bytes = 0;
        let mut any_host_exception = false;
        for text in lists {
            let mut only = String::new();
            let mut scope_rules = String::new();
            for line in text.lines().map(str::trim) {
                if let Some(candidate) = host_only(line)
                    && let Some(rule) = network(&candidate)
                    && !rule.is_badfilter()
                    && !disabled.contains(&rule.get_id())
                {
                    only.push_str(&candidate);
                    only.push('\n');
                    host_rules += 1;
                }
                if any_host_exception || !line.starts_with("@@") {
                    continue;
                }
                any_host_exception = project_guard(line, disabled, &mut scope_rules);
            }
            hosts.add_filter_list(only, ParseOptions::default());
            guard_bytes += scope_rules.len();
            // A projection larger than the source budget is already too
            // broad to justify optimizing the hostname decision.
            any_host_exception |= guard_bytes > MAX_LISTS;
            if !any_host_exception {
                guards.add_filter_list(scope_rules, ParseOptions::default());
            }
        }
        Self {
            whole: hosts,
            guards: (!any_host_exception).then_some(guards),
            count: host_rules,
        }
    }
}

/// Builds a guard for the exception's necessary destination. Returns true
/// when the destination cannot be narrowed to specific host patterns.
fn project_guard(line: &str, disabled: &HashSet<u64>, scope_rules: &mut String) -> bool {
    let Some(rule) = network(line) else {
        return false;
    };
    if !is_network_exception(&rule)
        || !rule.mask.contains(Mask::FROM_HTTPS)
        || (rule.mask.contains(Mask::IS_LEFT_ANCHOR) && line.starts_with("@@|http://"))
        || disabled.contains(&rule.get_id())
        || opts(line).any(|option| option_name(option) == "tag")
    {
        return false;
    }
    if let Some((name, suffix)) = exception_host(line, &rule) {
        scope_rules.push_str("@@||");
        scope_rules.push_str(name);
        if suffix {
            scope_rules.push('^');
        }
        scope_rules.push('\n');
        false
    } else if let Some(domains) = &rule.opt_to_domains {
        let mut scope = to_domains(line)
            .filter(|domain| name_matches_host_pattern(domain))
            .filter(|domain| domain_named(domains, domain))
            .collect::<Vec<_>>();
        scope.sort_unstable();
        scope.dedup();
        let incomplete = scope.len() != domains.len();
        for domain in scope {
            scope_rules.push_str("@@||");
            scope_rules.push_str(domain);
            scope_rules.push_str("^\n");
        }
        incomplete
    } else {
        true
    }
}
