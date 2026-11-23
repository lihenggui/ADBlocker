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

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use super::*;

/// A model of the kernel's rules and of the packet filter's tools.
#[derive(Default)]
struct State {
    rules: BTreeSet<Rule>,
    chains: BTreeSet<String>,
    jumps: BTreeSet<String>,
    staged: BTreeMap<String, String>,
    commands: Vec<String>,
    blocked: Vec<&'static str>,
}

#[derive(Clone, Default)]
struct Fake(Rc<RefCell<State>>);

impl Fake {
    fn log(&self, command: String) -> bool {
        let mut state = self.0.borrow_mut();
        let blocked = state.blocked.iter().any(|b| command.starts_with(b));
        state.commands.push(command);
        blocked
    }
}

impl Tools for Fake {
    fn run(&mut self, tool: &str, args: &[&str], input: &str) -> anyhow::Result<i32> {
        if self.log(format!("{tool} {}", args.join(" "))) {
            return Ok(4);
        }
        let mut state = self.0.borrow_mut();
        if let Some(tool) = tool.strip_suffix("-restore") {
            assert!(input.contains(&format!(":{CHAIN} - [0:0]")));
            state.chains.insert(tool.to_owned());
            state.staged.insert(tool.to_owned(), input.to_owned());
            return Ok(0);
        }
        // -w 5 -t mangle <action> <chain> ...
        let jump = format!("{tool} {}", args[5..].join(" "));
        Ok(match args[4] {
            "-S" => i32::from(!state.chains.contains(tool)),
            "-C" => i32::from(!state.jumps.contains(&jump)),
            "-I" => i32::from(!state.jumps.insert(jump)),
            "-D" => i32::from(!state.jumps.remove(&jump)),
            "-F" => 0,
            "-X" => i32::from(!state.chains.remove(tool)),
            other => panic!("unexpected {other}"),
        })
    }

    fn route_present(&mut self, _: Family) -> anyhow::Result<bool> {
        unreachable!("lookup rules do not read the capture route")
    }

    fn set_gate(&mut self, _: bool) -> anyhow::Result<()> {
        unreachable!("lookup rules do not use the capture gate")
    }

    fn gate_open(&mut self) -> anyhow::Result<bool> {
        unreachable!("lookup rules do not use the capture gate")
    }
}

impl Kernel for Fake {
    fn tools(&mut self) -> &mut dyn Tools {
        self
    }

    fn rule(&mut self, add: bool, rule: &Rule) -> io::Result<()> {
        let verb = if add { "add" } else { "del" };
        let family = if rule.v6 { 6 } else { 4 };
        if self.log(format!("rule {family} {verb} {rule:?}")) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let mut state = self.0.borrow_mut();
        if add {
            state.rules.insert(*rule);
        } else {
            state.rules.remove(rule);
        }
        Ok(())
    }

    fn any_rule(&mut self, v6: bool) -> io::Result<bool> {
        let family = if v6 { 6 } else { 4 };
        if self.log(format!("rule {family} del any")) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let mut state = self.0.borrow_mut();
        let found = state.rules.iter().find(|rule| rule.v6 == v6).copied();
        Ok(found.is_some_and(|rule| state.rules.remove(&rule)))
    }
}

fn rules(v6: bool) -> (Rules, Fake) {
    let fake = Fake::default();
    (Rules::new(Box::new(fake.clone()), 40001, v6), fake)
}

fn chosen() -> BTreeSet<u32> {
    BTreeSet::from([10001, 10002, 10005, 1_010_001])
}

fn position(commands: &[String], prefix: &str) -> Option<usize> {
    commands.iter().position(|c| c.starts_with(prefix))
}

#[test]
fn no_rule_or_chain_is_there_while_no_app_is_active() {
    let (mut rules, fake) = rules(true);
    rules.set(&BTreeSet::new(), Families::DualStack).unwrap();
    let state = fake.0.borrow();
    assert!(state.rules.is_empty() && state.chains.is_empty() && state.jumps.is_empty());
    assert!(state.staged.is_empty());
}

#[test]
fn each_range_of_adjacent_uids_gets_a_rule_for_each_protocol_source_and_family() {
    let (mut rules, fake) = rules(true);
    rules.set(&chosen(), Families::DualStack).unwrap();
    let state = fake.0.borrow();
    let ranges = [(10001, 10002), (10005, 10005), (1_010_001, 1_010_001)];
    let mut expected = BTreeSet::new();
    for v6 in [false, true] {
        for uids in ranges {
            for proto in [17, 6] {
                for source in [Source::Unset, Source::Anchor] {
                    expected.insert(Rule {
                        v6,
                        uids,
                        proto,
                        source,
                    });
                }
            }
        }
    }
    assert_eq!(state.rules, expected);
    assert_eq!(state.jumps.len(), 4);
    assert_eq!(state.chains.len(), 2);
}

#[test]
fn tproxy_takes_only_what_came_by_our_route_to_somewhere_else() {
    let v4 = chain(Family::V4, 40001);
    assert!(v4.contains(&format!(
        "-A {CHAIN} -s 127.0.0.1/32 ! -d 127.0.0.0/8 -p udp -j TPROXY --on-ip 127.0.0.1 --on-port 40001"
    )));
    assert!(v4.contains("-p tcp -j TPROXY --on-ip 127.0.0.1 --on-port 40001"));
    let v6 = chain(Family::V6, 40001);
    assert!(v6.contains(&format!(
        "-A {CHAIN} -s {SOURCE_V6}/128 ! -d {SOURCE_V6}/128 -p udp -j TPROXY --on-ip ::1 --on-port 40001"
    )));
    for jump in JUMPS {
        assert_eq!((jump.table, jump.from), ("mangle", "PREROUTING"));
        assert_eq!(jump.packets[..2], ["-i", "lo"]);
        assert_eq!(jump.packets[4..], ["--dport", "53"]);
    }
}

#[test]
fn tproxy_is_in_place_before_any_lookup_is_routed_here() {
    let (mut rules, fake) = rules(true);
    rules.set(&chosen(), Families::DualStack).unwrap();
    let state = fake.0.borrow();
    let last_xtables = state
        .commands
        .iter()
        .rposition(|c| c.contains("tables"))
        .unwrap();
    let first_rule = position(&state.commands, "rule 4 add").unwrap();
    assert!(last_xtables < first_rule);
}

#[test]
fn rules_a_run_before_left_are_withdrawn_before_any_is_added() {
    let (mut rules, fake) = rules(false);
    let stale = Rule {
        v6: false,
        uids: (10200, 10200),
        proto: 17,
        source: Source::Unset,
    };
    fake.0.borrow_mut().rules.insert(stale);
    rules.set(&chosen(), Families::DualStack).unwrap();
    let state = fake.0.borrow();
    assert!(!state.rules.contains(&stale));
    let withdrawn = position(&state.commands, "rule 4 del any").unwrap();
    assert!(withdrawn < position(&state.commands, "rule 4 add").unwrap());
}

#[test]
fn a_changed_selection_keeps_continuing_apps_and_adds_before_it_removes() {
    let (mut rules, fake) = rules(false);
    rules.set(&chosen(), Families::Ipv4).unwrap();
    fake.0.borrow_mut().commands.clear();
    let next = BTreeSet::from([10001, 10002, 10007]);
    rules.set(&next, Families::Ipv4).unwrap();
    let state = fake.0.borrow();
    assert!(state.commands.iter().all(|c| !c.contains("del any")));
    let added = position(&state.commands, "rule 4 add").unwrap();
    let removed = position(&state.commands, "rule 4 del").unwrap();
    assert!(added < removed);
    assert!(!state.commands.iter().any(|c| c.contains("(10001, 10002)")));
    let uids: BTreeSet<_> = state.rules.iter().map(|rule| rule.uids).collect();
    assert_eq!(uids, BTreeSet::from([(10001, 10002), (10007, 10007)]));
}

#[test]
fn an_unchanged_selection_changes_nothing() {
    let (mut rules, fake) = rules(true);
    rules.set(&chosen(), Families::DualStack).unwrap();
    let before = fake.0.borrow().commands.len();
    rules.set(&chosen(), Families::DualStack).unwrap();
    assert_eq!(fake.0.borrow().commands.len(), before);
}

#[test]
fn ipv6_lookups_are_left_alone_without_their_route_or_ipv6_capture() {
    for (v6, families) in [(false, Families::DualStack), (true, Families::Ipv4)] {
        let (mut rules, fake) = rules(v6);
        rules.set(&chosen(), families).unwrap();
        let state = fake.0.borrow();
        assert!(state.rules.iter().all(|rule| !rule.v6));
        assert!(!state.chains.contains("ip6tables"));
    }
    // IPv6 capture ending takes the IPv6 rules and chain away.
    let (mut rules, fake) = rules(true);
    rules.set(&chosen(), Families::DualStack).unwrap();
    rules.set(&chosen(), Families::Ipv4).unwrap();
    let state = fake.0.borrow();
    assert!(state.rules.iter().all(|rule| !rule.v6));
    assert!(!state.chains.contains("ip6tables"));
    assert!(state.chains.contains("iptables"));
}

#[test]
fn the_last_app_paused_takes_every_rule_and_chain_away() {
    let (mut rules, fake) = rules(true);
    rules.set(&chosen(), Families::DualStack).unwrap();
    fake.0.borrow_mut().commands.clear();
    rules.set(&BTreeSet::new(), Families::DualStack).unwrap();
    let state = fake.0.borrow();
    assert!(state.rules.is_empty() && state.chains.is_empty() && state.jumps.is_empty());
    let last_rule = state
        .commands
        .iter()
        .rposition(|c| c.starts_with("rule"))
        .unwrap();
    let first_xtables = position(&state.commands, "ip").unwrap();
    assert!(last_rule < first_xtables);
}

#[test]
fn a_failed_tproxy_update_leaves_no_lookup_routed_here() {
    let (mut rules, fake) = rules(true);
    rules
        .set(&BTreeSet::from([10001]), Families::DualStack)
        .unwrap();
    fake.0.borrow_mut().blocked = vec!["ip6tables-restore"];
    assert!(rules.set(&chosen(), Families::DualStack).is_err());
    assert!(fake.0.borrow().rules.is_empty());

    fake.0.borrow_mut().blocked.clear();
    rules.set(&chosen(), Families::DualStack).unwrap();
    assert_eq!(fake.0.borrow().rules.len(), 24);
}

#[test]
fn a_failed_rule_leaves_none_of_the_others() {
    let (mut rules, fake) = rules(false);
    fake.0.borrow_mut().blocked = vec!["rule 4 add Rule { v6: false, uids: (10005"];
    assert!(rules.set(&chosen(), Families::Ipv4).is_err());
    assert!(fake.0.borrow().rules.is_empty());
}

#[test]
fn removal_withdraws_every_rule_before_the_first_xtables_lock() {
    let (mut rules, fake) = rules(true);
    rules.set(&chosen(), Families::DualStack).unwrap();
    {
        let mut state = fake.0.borrow_mut();
        state.blocked = vec!["iptables "];
        state.commands.clear();
    }
    assert!(rules.remove().is_err());
    let state = fake.0.borrow();
    assert!(state.rules.is_empty());
    let first_xtables = position(&state.commands, "ip").unwrap();
    assert!(
        state.commands[..first_xtables]
            .iter()
            .all(|c| c.starts_with("rule"))
    );
    // The other family is cleaned up even though one is locked.
    assert!(!state.chains.contains("ip6tables"));
    assert!(state.chains.contains("iptables"));
}

#[test]
fn removal_from_nothing_known_takes_away_what_a_crashed_run_left() {
    let (mut rules, fake) = rules(true);
    {
        let mut state = fake.0.borrow_mut();
        for v6 in [false, true] {
            state.rules.insert(Rule {
                v6,
                uids: (10001, 10001),
                proto: 6,
                source: Source::Anchor,
            });
        }
        state.chains.insert("iptables".to_owned());
        state.jumps.insert(format!(
            "iptables PREROUTING -i lo -p udp --dport 53 -j {CHAIN}"
        ));
    }
    rules.remove().unwrap();
    let state = fake.0.borrow();
    assert!(state.rules.is_empty() && state.chains.is_empty() && state.jumps.is_empty());
}
