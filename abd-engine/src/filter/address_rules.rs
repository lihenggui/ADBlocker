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

//! Decide whether to refuse or bypass a remote endpoint.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use regex::{RegexSet, RegexSetBuilder};
use serde::{Deserialize, Serialize};

use super::app_patterns::{Condition, Patterns, Scope};
use crate::limits::{MAX_ADDRESS_REGEX, MAX_ADDRESS_REGEX_SET, MAX_ADDRESS_RULES};
use crate::rule::address::{AddressRule, Pattern, valid_expression};
use crate::rule::{Code, RuleReason};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressDecision {
    NoMatch,
    Block,
    Pass,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
enum Priority {
    Block,
    Pass,
    ImportantBlock,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    app: Option<Condition>,
    port: Option<u16>,
    priority: Priority,
}

impl Entry {
    fn matches(&self, port: u16, scope: Option<&Scope<'_>>) -> bool {
        self.port.is_none_or(|p| p == port)
            && self
                .app
                .as_ref()
                .is_none_or(|app| scope.is_some_and(|scope| app.matches(scope)))
    }
}

type Exact = HashMap<IpAddr, Vec<Entry>>;

#[derive(Default)]
pub(crate) struct AddressProjections {
    exact: Exact,
    expressions: Vec<String>,
    entries: Vec<Entry>,
    count: usize,
    dropped: usize,
    patterns: Patterns,
}

impl AddressProjections {
    pub(crate) fn add(&mut self, rule: &AddressRule<'_>) -> Result<(), RuleReason> {
        if self.count >= MAX_ADDRESS_RULES {
            self.dropped += 1;
            return Err(Code::AddressLimit.into());
        }
        let app = if let Some(names) = rule.apps {
            let prepared = self.patterns.prepare(names).ok_or(Code::AppPattern)?;
            let app = Condition::new(&prepared.encoded);
            self.patterns.keep(prepared);
            Some(app)
        } else {
            None
        };
        let priority = match (rule.exception, rule.important) {
            (true, _) => Priority::Pass,
            (false, true) => Priority::ImportantBlock,
            (false, false) => Priority::Block,
        };
        let mut entry = Entry {
            app,
            port: None,
            priority,
        };
        match rule.pattern {
            Pattern::Exact(ip, port) => {
                entry.port = port;
                self.exact.entry(ip).or_default().push(entry);
            }
            Pattern::Expression(expression) => {
                self.expressions.push(expression.to_owned());
                self.entries.push(entry);
            }
        }
        self.count += 1;
        Ok(())
    }

    pub(super) fn finish(mut self) -> AddressRules {
        let regex = compile_prefix(&self.expressions);
        let compiled = regex.as_ref().map_or(0, RegexSet::len);
        self.dropped += self.entries.len() - compiled;
        self.entries.truncate(compiled);
        if self.dropped != 0 {
            eprintln!(
                "lists: skipped {} rules at the address rule limits",
                self.dropped
            );
        }
        self.patterns.report_dropped();
        AddressRules::new(self.exact, regex, self.entries, self.patterns)
    }
}

fn compile(expressions: &[String]) -> Option<RegexSet> {
    RegexSetBuilder::new(expressions)
        .size_limit(MAX_ADDRESS_REGEX_SET)
        .dfa_size_limit(MAX_ADDRESS_REGEX)
        .build()
        .ok()
}

fn compile_prefix(expressions: &[String]) -> Option<RegexSet> {
    if expressions.is_empty() {
        return None;
    }
    if let Some(set) = compile(expressions) {
        return Some(set);
    }
    // Every expression compiled on its own. At the set's heap limit,
    // keep the longest prefix that fits, in list order.
    let mut kept = None;
    let (mut low, mut high) = (1, expressions.len());
    while low < high {
        let middle = low.midpoint(high);
        if let Some(set) = compile(&expressions[..middle]) {
            kept = Some(set);
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    kept
}

#[derive(Default)]
pub(super) struct AddressRules {
    exact: Exact,
    regex: Option<RegexSet>,
    entries: Vec<Entry>,
    has_apps: bool,
    patterns: Patterns,
}

impl AddressRules {
    fn new(exact: Exact, regex: Option<RegexSet>, entries: Vec<Entry>, patterns: Patterns) -> Self {
        let has_apps = exact
            .values()
            .flatten()
            .chain(&entries)
            .any(|e| e.app.is_some());
        Self {
            exact,
            regex,
            entries,
            has_apps,
            patterns,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.entries.is_empty()
    }

    pub(super) fn decide(&self, endpoint: SocketAddr, app: Option<&str>) -> AddressDecision {
        let exact = self.exact.get(&endpoint.ip()).into_iter().flatten();
        let matches = self.regex.as_ref().map(|regex| {
            // AdGuard's reference leaves the input text unspecified. Test both forms
            // so an end anchor can delimit the IP alone and port expressions still work.
            let endpoint = SocketAddr::new(endpoint.ip(), endpoint.port()).to_string();
            let (address, _) = endpoint
                .rsplit_once(':')
                .expect("socket address includes a port");
            (regex.matches(address), regex.matches(&endpoint))
        });
        let expressions = matches
            .iter()
            .flat_map(|(address, endpoint)| {
                address
                    .iter()
                    .chain(endpoint.iter().filter(|&id| !address.matched(id)))
            })
            .map(|id| &self.entries[id]);
        let mut candidates = exact.chain(expressions).peekable();
        let scope = if self.has_apps && candidates.peek().is_some() {
            app.and_then(|app| self.patterns.scope(app))
        } else {
            None
        };
        let mut priority = None;
        for entry in candidates.filter(|entry| entry.matches(endpoint.port(), scope.as_ref())) {
            if entry.priority == Priority::ImportantBlock {
                return AddressDecision::Block;
            }
            priority = priority.max(Some(entry.priority));
        }
        match priority {
            Some(Priority::Pass) => AddressDecision::Pass,
            Some(_) => AddressDecision::Block,
            None => AddressDecision::NoMatch,
        }
    }

    pub(super) fn serialized(&self) -> Vec<u8> {
        let expressions = self.regex.as_ref().map_or(&[][..], RegexSet::patterns);
        serde_json::to_vec(&(
            &self.exact,
            expressions,
            &self.entries,
            self.patterns.keys(),
        ))
        .expect("address rules are serializable")
    }

    pub(super) fn cached(bytes: &[u8]) -> Option<Self> {
        let (exact, expressions, entries, apps): (Exact, Vec<String>, Vec<Entry>, Vec<String>) =
            serde_json::from_slice(bytes).ok()?;
        if expressions.len() != entries.len()
            || exact.values().map(Vec::len).sum::<usize>() + entries.len() > MAX_ADDRESS_RULES
            || !expressions
                .iter()
                .all(|expression| valid_expression(expression))
        {
            return None;
        }
        let regex = if expressions.is_empty() {
            None
        } else {
            Some(compile(&expressions)?)
        };
        Some(Self::new(exact, regex, entries, Patterns::from_keys(apps)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Filters;
    use crate::rule::{Kind, read};

    fn filters(rules: &str) -> Filters {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("rules.txt"), rules).unwrap();
        Filters::load(&[dir.path()], false).unwrap()
    }

    fn example(rule: &str, result: AddressDecision, matches: &[&str], misses: &[&str]) {
        let filters = filters(rule);
        let app = filters.for_app("com.app");
        for endpoint in matches {
            assert_eq!(
                app.address_decision(endpoint.parse().unwrap()),
                result,
                "{endpoint}"
            );
        }
        for endpoint in misses {
            assert_eq!(
                app.address_decision(endpoint.parse().unwrap()),
                AddressDecision::NoMatch,
                "{endpoint}"
            );
        }
    }

    #[test]
    fn the_reference_ipv4_port_rule_matches_only_the_exact_port() {
        example(
            "174.129.166.49:3478^$network",
            AddressDecision::Block,
            &["174.129.166.49:3478"],
            &["174.129.166.49:34788", "174.129.166.50:3478"],
        );
    }

    #[test]
    fn the_reference_ipv6_port_rule_matches_equivalent_spellings() {
        example(
            "[2001:4860:4860::8888]:443^$network",
            AddressDecision::Block,
            &[
                "[2001:4860:4860::8888]:443",
                "[2001:4860:4860:0:0:0:0:8888]:443",
            ],
            &["[2001:4860:4860::8888]:444", "[2001:4860:4860::8844]:443"],
        );
    }

    #[test]
    fn the_reference_address_rule_matches_every_port() {
        example(
            "174.129.166.49$network",
            AddressDecision::Block,
            &["174.129.166.49:1", "174.129.166.49:65535"],
            &["174.129.166.50:1234"],
        );
    }

    #[test]
    fn the_reference_exception_passes_every_port() {
        example(
            "@@174.129.166.49$network",
            AddressDecision::Pass,
            &["174.129.166.49:80", "174.129.166.49:65535"],
            &["174.129.166.50:80"],
        );
    }

    #[test]
    fn the_reference_port_expression_searches_ipv4_and_ipv6_endpoints() {
        example(
            "/.+:3[0-9]{4}/$network",
            AddressDecision::Block,
            &["8.8.8.8:30000", "[2001:db8::1]:39999"],
            &[
                "8.8.8.8:29999",
                "8.8.8.8:40000",
                "[2001:db8::1]:40000",
                "[2001:db8::3000]:80",
            ],
        );
    }

    #[test]
    fn the_reference_ip_expression_matches_both_named_addresses() {
        example(
            "/8.8.8.(:?8|4)/$network",
            AddressDecision::Block,
            &["8.8.8.8:53", "8.8.8.4:80"],
            &["8.8.8.5:53", "8.8.4.4:80"],
        );
    }

    #[test]
    fn an_end_anchored_ipv4_expression_matches_its_address_range_on_any_port() {
        let filters = filters(r"/^23\.109\.170\.(18[7-9]|19[0-2])$/$network");
        let app = filters.for_app("com.app");
        for port in [0, 53, 443, 3478, u16::MAX] {
            for (octet, decision) in [
                (186, AddressDecision::NoMatch),
                (187, AddressDecision::Block),
                (192, AddressDecision::Block),
                (193, AddressDecision::NoMatch),
            ] {
                assert_eq!(
                    app.address_decision(([23, 109, 170, octet], port).into()),
                    decision
                );
            }
        }
    }

    #[test]
    fn an_end_anchored_ipv6_expression_matches_the_bracketed_address() {
        example(
            r"/^\[2001:db8::1\]$/$network",
            AddressDecision::Block,
            &["[2001:0db8:0:0:0:0:0:1]:53", "[2001:db8::1]:65535"],
            &["[2001:db8::2]:53"],
        );
    }

    #[test]
    fn the_four_anchored_address_rules_from_the_lists_block_their_ips() {
        // lists/2.txt and live/base-android.txt each contain these two lines.
        let rules = "||103.179.189.35^$network\n||88.208.46.222^$network\n".repeat(2);
        example(
            &rules,
            AddressDecision::Block,
            &[
                "103.179.189.35:80",
                "103.179.189.35:443",
                "88.208.46.222:80",
                "88.208.46.222:443",
            ],
            &["103.179.189.36:80", "88.208.46.223:443"],
        );
    }

    #[test]
    fn expressions_see_the_standard_collapsed_endpoint_and_respect_anchors() {
        example(
            r"/^\[2001:db8::1\]:443$/$network",
            AddressDecision::Block,
            &["[2001:0db8:0:0:0:0:0:1]:443"],
            &["[2001:db8::1]:4430"],
        );
    }

    #[test]
    fn address_rules_match_names_prefixes_expressions_and_negated_apps() {
        let filters = filters(
            "192.0.2.1:1001$network,app=COM.reader\n\
             192.0.2.1:1002$network,app=com.news*\n\
             /:1003$/$network,app=/^com\\.news\\d+\\$/\n\
             192.0.2.1:1004$network,app=~com.reader\n\
             192.0.2.1:1005$network",
        );
        for (app, ports) in [
            ("com.reader", &[1001, 1005][..]),
            ("12:com.reader", &[1001, 1005]),
            ("com.news2", &[1002, 1003, 1004, 1005]),
            ("com.newsdesk", &[1002, 1004, 1005]),
            ("com.other", &[1004, 1005]),
            ("@com.shared", &[1005]),
            ("12:@com.shared", &[1005]),
        ] {
            for port in 1001..=1005 {
                assert_eq!(
                    filters
                        .for_app(app)
                        .address_decision(([192, 0, 2, 1], port).into()),
                    if ports.contains(&port) {
                        AddressDecision::Block
                    } else {
                        AddressDecision::NoMatch
                    },
                    "{app}:{port}",
                );
            }
        }
    }

    #[test]
    fn exceptions_win_unless_a_matching_important_rule_blocks_the_endpoint() {
        let filters = filters(
            "192.0.2.1$network\n\
             @@/:443$/$network,app=com.reader\n\
             /^192\\.0\\.2\\.1$/$network,important,app=com.reader\n\
             @@192.0.2.1:80$network\n\
             192.0.2.1:4430$network,important",
        );
        let app = filters.for_app("com.reader");
        assert_eq!(
            app.address_decision("192.0.2.1:443".parse().unwrap()),
            AddressDecision::Block
        );
        assert_eq!(
            filters
                .for_app("com.other")
                .address_decision("192.0.2.1:80".parse().unwrap()),
            AddressDecision::Pass
        );
        assert_eq!(
            app.address_decision("192.0.2.2:443".parse().unwrap()),
            AddressDecision::Pass
        );
        assert_eq!(
            filters
                .for_app("com.other")
                .address_decision("192.0.2.2:443".parse().unwrap()),
            AddressDecision::NoMatch
        );
    }

    #[test]
    fn later_address_rules_are_skipped_when_the_generation_is_full() {
        let mut addresses = AddressProjections::default();
        for i in 0..=MAX_ADDRESS_RULES {
            let text = format!("192.0.2.1:{i}$network");
            let Kind::Address(rule) = read(&text).kind else {
                panic!("{text}");
            };
            assert_eq!(addresses.add(&rule).is_ok(), i < MAX_ADDRESS_RULES);
        }
        assert_eq!(addresses.dropped, 1);
        let addresses = addresses.finish();
        assert_eq!(
            addresses.decide(([192, 0, 2, 1], MAX_ADDRESS_RULES as u16 - 1).into(), None),
            AddressDecision::Block
        );
        assert_eq!(
            addresses.decide(([192, 0, 2, 1], MAX_ADDRESS_RULES as u16).into(), None),
            AddressDecision::NoMatch
        );
    }

    #[test]
    fn exact_and_expression_rules_and_their_app_patterns_survive_the_cache() {
        let lists = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::write(
            lists.path().join("rules.txt"),
            "192.0.2.1$network,app=com.reader*\n@@/:443$/$network,app=/reader\\d/",
        )
        .unwrap();
        let built = Filters::load_cached(&[lists.path()], cache.path()).unwrap();
        let cached = Filters::read_cache(cache.path(), built.generation.unwrap()).unwrap();
        for filters in [built, cached] {
            let app = filters.for_app("com.reader2");
            assert_eq!(
                app.address_decision("192.0.2.1:80".parse().unwrap()),
                AddressDecision::Block
            );
            assert_eq!(
                app.address_decision("192.0.2.1:443".parse().unwrap()),
                AddressDecision::Pass
            );
            assert_eq!(
                filters
                    .for_app("com.other")
                    .address_decision("192.0.2.1:443".parse().unwrap()),
                AddressDecision::NoMatch
            );
        }
    }
}
