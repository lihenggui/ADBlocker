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

use std::collections::HashSet;
use std::sync::LazyLock;

use adblock::Engine;
use adblock::filters::network::NetworkFilter;
use adblock::utils::fast_hash;

use super::host_token;
use super::modifiers::{DENYALLOW, Kind, lookup};
use super::options::{OptionToken, Options, literal_tokens};
use crate::host::is_host;
use crate::public_suffix::is_known_top_level;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Rejection {
    PatternNamesHost,
    NegatedModifier,
    NegatedDomain,
    WildcardTld,
    RegexDomain,
    EmptyDomain,
    InvalidDomain,
    Duplicate,
    WithTo,
    WithResponseAction,
}

pub(super) struct Condition(String);

impl Condition {
    pub(super) fn read(options: &Options<'_>) -> Result<Option<Self>, Rejection> {
        let mut tokens = options.tokens();
        let Some(token) = tokens.find(|token| token.name() == DENYALLOW) else {
            return Ok(None);
        };
        for other in options.tokens() {
            match lookup(other.name()).map(|modifier| modifier.kind) {
                Some(Kind::Destination) => return Err(Rejection::WithTo),
                Some(Kind::Action(..)) => return Err(Rejection::WithResponseAction),
                _ => (),
            }
        }
        if tokens.any(|token| token.name() == DENYALLOW) {
            return Err(Rejection::Duplicate);
        }
        if names_host(options.pattern) {
            return Err(Rejection::PatternNamesHost);
        }
        if token.raw_name != DENYALLOW {
            return Err(Rejection::NegatedModifier);
        }
        let mut rewritten = String::from("to=");
        for (index, domain) in token.value.unwrap_or_default().split('|').enumerate() {
            if index != 0 {
                rewritten.push('|');
            }
            rewritten.push('~');
            rewritten.push_str(&literal_domain(domain)?);
        }
        Ok(Some(Self(rewritten)))
    }

    pub(super) fn option<'a>(&'a self, token: OptionToken<'a>) -> &'a str {
        if token.name() == DENYALLOW {
            &self.0
        } else {
            token.raw
        }
    }

    pub(super) fn rewrite(&self, options: &Options<'_>) -> String {
        options.write(|token| self.option(token))
    }
}

fn literal_domain(domain: &str) -> Result<String, Rejection> {
    if domain.is_empty() {
        return Err(Rejection::EmptyDomain);
    }
    if domain.starts_with('~') {
        return Err(Rejection::NegatedDomain);
    }
    if domain.starts_with('/') {
        return Err(Rejection::RegexDomain);
    }
    if domain.ends_with(".*") {
        return Err(Rejection::WildcardTld);
    }
    // The matcher hashes $to values as given, and request hosts as lowercase ASCII.
    let host = idna::domain_to_ascii(domain).map_err(|_| Rejection::InvalidDomain)?;
    if !is_host(&host) || host.contains(':') || host.split('.').any(str::is_empty) {
        return Err(Rejection::InvalidDomain);
    }
    Ok(host)
}

fn names_host(pattern: &str) -> bool {
    let pattern = pattern.strip_prefix("@@").unwrap_or(pattern);
    if pattern.starts_with("||") {
        return true;
    }
    // A URL authority names a destination even without a host anchor. For
    // bare patterns require a known TLD, so a file name such as ads.js is
    // still a pattern. A path or regex can match on any host.
    let pattern = pattern.strip_prefix('|').unwrap_or(pattern);
    if let Some((scheme, authority)) = pattern.split_once("://")
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.' | b'*'))
    {
        let authority = authority
            .split(['/', '^', '?', '|'])
            .next()
            .unwrap_or_default();
        let host = authority.rsplit('@').next().unwrap_or_default();
        // An IPv6 literal names a host. Ignore the port on other authorities
        // so a wildcard host on a fixed port still covers any domain.
        return host.starts_with('[')
            || host
                .split(':')
                .next()
                .is_some_and(|name| name.chars().any(char::is_alphanumeric));
    }
    let host = host_token(pattern);
    host.rsplit_once('.')
        .is_some_and(|(name, tld)| is_host(name) && is_known_top_level(&tld.to_ascii_lowercase()))
}

/// Parsing succeeds for a negated `$to` rule the matcher cannot use.
pub(crate) fn matcher_applies(line: &str, rule: NetworkFilter<'_>) -> bool {
    if rule.is_badfilter() {
        return true;
    }
    if let Some(domains) = &rule.opt_domains {
        if line.contains('*') && wildcard_sources(line, domains) {
            return false;
        }
        if domains.len() == 1 && rule.opt_not_domains.is_none() {
            return true;
        }
    }
    // The builder can drop a negative-only $to rule that would land in the
    // catch-all bucket. Build just this rule without list metadata so the
    // matcher itself decides whether the compiled engine is empty.
    static EMPTY_SIZE: LazyLock<usize> = LazyLock::new(|| {
        Engine::new_with_parsed_rules(vec![], vec![])
            .serialize()
            .len()
    });
    Engine::new_with_parsed_rules(vec![rule], vec![])
        .serialize()
        .len()
        != *EMPTY_SIZE
}

fn wildcard_sources(line: &str, domains: &[u64]) -> bool {
    let Some((_, suffix)) = line.rsplit_once('$') else {
        return false;
    };
    // The matcher hashes wildcard source domains literally. A rule with
    // only these positive hashes cannot match a request from a real host.
    let wildcard_hashes: HashSet<_> = literal_tokens(suffix)
        .filter(|option| matches!(option.name(), "domain" | "from"))
        .filter_map(|option| option.value)
        .flat_map(|value| value.split('|'))
        .filter(|domain| !domain.starts_with('~') && domain.contains('*'))
        .map(fast_hash)
        .collect();
    domains
        .iter()
        .all(|domain| wildcard_hashes.contains(domain))
}
