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

//! One more try of a host an app was found to pin, when a process of the
//! app is given the certificates as it starts.
//!
//! An app can stop pinning without anything telling us: it is updated, or
//! the user has a module check its pins against our authority (the LSPosed
//! module in `android/hook`). Its next process is then given the hosts it
//! pinned to try once each: the first connection to one that would be
//! passed through as pinned, and that can be decrypted, is decrypted. A
//! request on it shows that the app accepts us, and the host is no longer
//! taken as pinned. A refusal, or a connection that ends without a
//! request, spends the try: the host stays pinned, and the app has lost
//! that one connection. A connection that never showed the app our
//! certificate leaves the try for the next.
//!
//! Only the connection that holds a try's ticket ends it: another one of
//! the app's, decrypted before the host was pinned, says nothing of it.
//!
//! Kept in memory only. What a try costs is bounded: one connection per
//! host per process start, and for one host no more than one refused try
//! per [`INTERVAL`], however often the app's processes start.

use std::collections::HashMap;

/// The least time, in seconds, between a refused try of a host and the
/// next: an app that crashes as it starts must not lose a connection to
/// each of its hosts at every start.
pub const INTERVAL: u64 = 60;
/// How long, in seconds, a try may be under way: one whose connection never
/// said how it ended counts as refused.
const RESOLVE_TIME: u64 = 120;
/// The most hosts with a try remembered. There are no more than there are
/// pinned hosts; this bounds them should that table grow.
const MAX_ENTRIES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retry {
    /// The next connection to the host may be decrypted.
    Granted,
    /// Decrypted by the connection that holds this ticket, since then.
    Trying { ticket: u64, since: u64 },
    /// Refused at this time.
    Refused { at: u64 },
}

#[derive(Default)]
pub(super) struct Retries {
    entries: HashMap<String, HashMap<String, Retry>>,
    last_ticket: u64,
}

impl Retries {
    /// A process of the app started: each of its pinned `hosts` may be
    /// tried once, except one refused in the last [`INTERVAL`] or under
    /// way now. Returns whether any was left without a try for want of
    /// room.
    pub fn grant<'a>(
        &mut self,
        package: &str,
        hosts: impl IntoIterator<Item = &'a str>,
        now: u64,
    ) -> bool {
        let mut available = MAX_ENTRIES - self.entries.values().map(HashMap::len).sum::<usize>();
        let mut full = false;
        for host in hosts {
            match self
                .entries
                .get_mut(package)
                .and_then(|entries| entries.get_mut(host))
            {
                Some(state) => {
                    state.expire(now);
                    if matches!(*state, Retry::Refused { at } if now >= at.saturating_add(INTERVAL))
                    {
                        *state = Retry::Granted;
                    }
                }
                None if available == 0 => full = true,
                None => {
                    self.entries
                        .entry(package.to_owned())
                        .or_default()
                        .insert(host.to_owned(), Retry::Granted);
                    available -= 1;
                }
            }
        }
        full
    }

    #[cfg(test)]
    fn is_granted(&self, (package, host): (&str, &str)) -> bool {
        self.entries.get(package).and_then(|hosts| hosts.get(host)) == Some(&Retry::Granted)
    }

    /// Takes the try for a connection, if there is one: the ticket it
    /// ends the try with.
    pub fn take(&mut self, (package, host): (&str, &str), now: u64) -> Option<u64> {
        let state = self.entries.get_mut(package)?.get_mut(host)?;
        state.expire(now);
        if *state != Retry::Granted {
            return None;
        }
        self.last_ticket = self.last_ticket.wrapping_add(1);
        let ticket = self.last_ticket;
        *state = Retry::Trying { ticket, since: now };
        Some(ticket)
    }

    /// The app refused the try of `ticket`, or its connection ended without
    /// a request. Returns whether it was the try under way.
    pub fn refused(&mut self, key: (&str, &str), ticket: u64, now: u64) -> bool {
        let Some(state) = self.held(key, ticket) else {
            return false;
        };
        *state = Retry::Refused { at: now };
        true
    }

    /// The app sent a request on the try of `ticket`. Returns whether it
    /// was the try under way, which is then over.
    pub fn accepted(&mut self, (package, host): (&str, &str), ticket: u64) -> bool {
        let Some(hosts) = self.entries.get_mut(package) else {
            return false;
        };
        if !hosts.get(host).is_some_and(|state| state.holds(ticket)) {
            return false;
        }
        hosts.remove(host);
        if hosts.is_empty() {
            self.entries.remove(package);
        }
        true
    }

    /// The connection of `ticket` never showed the app our certificate:
    /// the try is the next connection's.
    pub fn unresolved(&mut self, key: (&str, &str), ticket: u64) {
        if let Some(state) = self.held(key, ticket) {
            *state = Retry::Granted;
        }
    }

    fn held(&mut self, (package, host): (&str, &str), ticket: u64) -> Option<&mut Retry> {
        self.entries
            .get_mut(package)?
            .get_mut(host)
            .filter(|state| state.holds(ticket))
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&str, &str) -> bool) {
        self.entries.retain(|package, hosts| {
            hosts.retain(|host, _| keep(package, host));
            !hosts.is_empty()
        });
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.values().map(HashMap::len).sum()
    }
}

impl Retry {
    fn holds(self, ticket: u64) -> bool {
        matches!(self, Self::Trying { ticket: held, .. } if held == ticket)
    }

    fn expire(&mut self, now: u64) {
        if let Self::Trying { since, .. } = *self
            && now >= since.saturating_add(RESOLVE_TIME)
        {
            *self = Self::Refused { at: since };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(host: &str) -> (&str, &str) {
        ("com.app", host)
    }

    #[test]
    fn a_process_start_grants_each_host_one_try() {
        let mut retries = Retries::default();
        retries.grant("com.app", ["a.example", "b.example"], 1000);
        assert!(retries.is_granted(key("a.example")));
        assert!(retries.is_granted(key("b.example")));
        assert!(!retries.is_granted(("com.other", "a.example")));

        let ticket = retries.take(key("a.example"), 1000);
        assert!(ticket.is_some());
        assert_eq!(retries.take(key("a.example"), 1000), None);
        assert!(!retries.is_granted(key("a.example")));
    }

    #[test]
    fn a_refused_try_is_not_granted_again_before_the_interval() {
        let mut retries = Retries::default();
        retries.grant("com.app", ["a.example"], 1000);
        let ticket = retries.take(key("a.example"), 1000).unwrap();
        assert!(retries.refused(key("a.example"), ticket, 1001));

        // A crash loop: process after process.
        for at in 1002..1001 + INTERVAL {
            retries.grant("com.app", ["a.example"], at);
            assert_eq!(retries.take(key("a.example"), at), None);
        }
        retries.grant("com.app", ["a.example"], 1001 + INTERVAL);
        assert!(retries.take(key("a.example"), 1001 + INTERVAL).is_some());
    }

    #[test]
    fn only_the_connection_of_the_ticket_ends_a_try() {
        let mut retries = Retries::default();
        retries.grant("com.app", ["a.example"], 1000);
        let ticket = retries.take(key("a.example"), 1000).unwrap();
        // A second process starts while the first tries.
        retries.grant("com.app", ["a.example"], 1001);
        assert_eq!(retries.take(key("a.example"), 1001), None);
        assert!(!retries.refused(key("a.example"), ticket + 1, 1002));
        assert!(!retries.accepted(key("a.example"), ticket + 1));
        retries.unresolved(key("a.example"), ticket + 1);
        assert!(!retries.is_granted(key("a.example")));
        assert!(retries.accepted(key("a.example"), ticket));
        assert_eq!(retries.len(), 0);
    }

    #[test]
    fn a_try_that_never_showed_the_app_our_certificate_is_left_for_the_next() {
        let mut retries = Retries::default();
        retries.grant("com.app", ["a.example"], 1000);
        let ticket = retries.take(key("a.example"), 1000).unwrap();
        retries.unresolved(key("a.example"), ticket);
        assert!(retries.take(key("a.example"), 1001).is_some());
    }

    #[test]
    fn a_try_that_never_ends_counts_as_refused_when_it_began() {
        let mut retries = Retries::default();
        retries.grant("com.app", ["a.example"], 1000);
        retries.take(key("a.example"), 1000).unwrap();
        retries.grant("com.app", ["a.example"], 1000 + RESOLVE_TIME);
        assert!(retries.is_granted(key("a.example")), "refused at 1000");
    }

    #[test]
    fn the_table_holds_a_bounded_number_of_hosts_and_says_when_it_is_full() {
        let mut retries = Retries::default();
        let many: Vec<String> = (0..MAX_ENTRIES + 10)
            .map(|n| format!("h{n}.example"))
            .collect();
        assert!(retries.grant("com.app", many.iter().map(String::as_str), 1000));
        assert_eq!(retries.len(), MAX_ENTRIES);
        assert!(!Retries::default().grant("com.app", ["a.example"], 1000));
    }

    /// The policy as connections meet it.
    mod policy {
        use super::super::super::{Policy, Reason, Verdict, read_pinned};
        use crate::allow::Allow;
        use crate::clock::now;
        use crate::exclusions::Exclusions;
        use crate::filter::Filters;

        fn filters(rules: &str) -> Filters {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("rules.txt"), rules).unwrap();
            Filters::load(&[dir.path()], false).unwrap()
        }

        fn policy() -> (Policy, tempfile::TempDir) {
            let dir = tempfile::tempdir().unwrap();
            (Policy::open(dir.path().join("pinned")), dir)
        }

        /// What a connection, or a lookup, of the app is decided.
        fn decide(policy: &Policy, filters: &Filters, package: &str, host: &str) -> Verdict {
            let ready = Exclusions::default().assume_ready();
            policy.decide_named(
                package,
                host,
                filters.for_app(package),
                &ready,
                &Allow::default(),
                false,
            )
        }

        const PINNED: Verdict = Verdict::Passthrough(Reason::Pinned);

        /// A connection that can be decrypted, as `inspect_tls` takes it:
        /// the ticket of its try, if it is one.
        fn try_of(policy: &Policy, filters: &Filters, host: &str) -> Option<u64> {
            (decide(policy, filters, "com.app", host) == PINNED)
                .then(|| policy.take_try("com.app", host))
                .flatten()
        }

        fn saved(dir: &tempfile::TempDir) -> std::collections::HashMap<(String, String), u64> {
            read_pinned(&dir.path().join("pinned"), now())
        }

        fn key(host: &str) -> (String, String) {
            ("com.app".to_owned(), host.to_owned())
        }

        #[test]
        fn a_pinned_host_is_tried_once_when_a_process_of_its_app_starts() {
            let (policy, _dir) = policy();
            let filters = filters("");
            policy.rejected("com.app", "a.example");
            assert_eq!(try_of(&policy, &filters, "a.example"), None);

            policy.process_started("com.app");
            // Decisions, those of lookups too, go by the pin.
            assert_eq!(decide(&policy, &filters, "com.app", "a.example"), PINNED);
            assert!(try_of(&policy, &filters, "a.example").is_some());
            assert_eq!(try_of(&policy, &filters, "a.example"), None);
            // Another app, and another host, are not touched.
            assert_eq!(
                decide(&policy, &filters, "com.other", "a.example"),
                Verdict::Mitm
            );
            policy.rejected("com.app", "b.example");
            assert_eq!(try_of(&policy, &filters, "b.example"), None);
        }

        #[test]
        fn a_request_on_the_try_forgets_the_pin_in_the_saved_table_too() {
            let (policy, dir) = policy();
            let filters = filters("");
            policy.rejected("com.app", "a.example");
            policy.rejected("com.app", "b.example");
            policy.process_started("com.app");
            let ticket = try_of(&policy, &filters, "a.example").unwrap();
            policy.spoke("com.app", "a.example", Some(ticket));
            policy.try_over("com.app", "a.example", ticket);

            assert_eq!(
                decide(&policy, &filters, "com.app", "a.example"),
                Verdict::Mitm
            );
            policy.write().unwrap();
            assert!(!saved(&dir).contains_key(&key("a.example")));
            assert!(saved(&dir).contains_key(&key("b.example")));
        }

        #[test]
        fn a_request_of_a_connection_decrypted_before_the_pin_leaves_it() {
            let (policy, dir) = policy();
            let filters = filters("");
            // A client of the app that does not pin is decrypted; another
            // that pins refuses us meanwhile; then the first asks.
            assert_eq!(
                decide(&policy, &filters, "com.app", "a.example"),
                Verdict::Mitm
            );
            policy.rejected("com.app", "a.example");
            policy.spoke("com.app", "a.example", None);
            assert_eq!(decide(&policy, &filters, "com.app", "a.example"), PINNED);

            // Nor during a try, from any connection but the try's.
            policy.process_started("com.app");
            let ticket = try_of(&policy, &filters, "a.example").unwrap();
            policy.spoke("com.app", "a.example", None);
            policy.spoke("com.app", "a.example", Some(ticket + 1));
            assert_eq!(decide(&policy, &filters, "com.app", "a.example"), PINNED);
            policy.write().unwrap();
            assert!(saved(&dir).contains_key(&key("a.example")));
        }

        #[test]
        fn without_a_process_start_a_pin_is_never_forgotten_by_a_request() {
            // As in VPN mode, which reports no process starts.
            let (policy, _dir) = policy();
            let filters = filters("");
            policy.rejected("com.app", "a.example");
            for ticket in [None, Some(0), Some(1), Some(u64::MAX)] {
                policy.spoke("com.app", "a.example", ticket);
            }
            assert_eq!(policy.take_try("com.app", "a.example"), None);
            assert_eq!(decide(&policy, &filters, "com.app", "a.example"), PINNED);
        }

        #[test]
        fn a_refused_try_keeps_the_pin_renewed_and_is_not_tried_again_soon() {
            let (policy, dir) = policy();
            let filters = filters("");
            policy.rejected("com.app", "a.example");
            policy
                .pinned
                .lock()
                .unwrap()
                .get_mut("com.app")
                .unwrap()
                .insert("a.example".to_owned(), now() - 3600);
            policy.process_started("com.app");
            let ticket = try_of(&policy, &filters, "a.example").unwrap();
            policy.try_refused("com.app", "a.example", ticket);
            policy.try_over("com.app", "a.example", ticket);

            // Another process at once: no second try.
            policy.process_started("com.app");
            assert_eq!(try_of(&policy, &filters, "a.example"), None);
            policy.write().unwrap();
            assert!(
                saved(&dir)[&key("a.example")] + 60 > now(),
                "kept from the refusal on"
            );
            assert!(policy.silent.lock().unwrap().is_empty(), "not counted");
        }

        #[test]
        fn a_try_whose_connection_never_reached_the_app_is_left_for_the_next() {
            let (policy, dir) = policy();
            let filters = filters("");
            policy.rejected("com.app", "a.example");
            let pinned_at = now() - 3600;
            policy
                .pinned
                .lock()
                .unwrap()
                .get_mut("com.app")
                .unwrap()
                .insert("a.example".to_owned(), pinned_at);
            policy.process_started("com.app");
            // The server could not be verified, say: our certificate was
            // never shown.
            let ticket = try_of(&policy, &filters, "a.example").unwrap();
            policy.try_over("com.app", "a.example", ticket);
            assert!(try_of(&policy, &filters, "a.example").is_some());
            policy.write().unwrap();
            assert_eq!(saved(&dir)[&key("a.example")], pinned_at, "not renewed");
        }

        #[test]
        fn a_pinned_host_its_domain_rules_block_stays_refused_with_a_try_pending() {
            let (policy, _dir) = policy();
            let filters = filters("||ads.example^\n@@/downloads/$image\n");
            policy.rejected("com.app", "ads.example");
            policy.process_started("com.app");
            assert_eq!(
                decide(&policy, &filters, "com.app", "ads.example"),
                Verdict::Block
            );
            assert_eq!(try_of(&policy, &filters, "ads.example"), None);
        }

        #[test]
        fn what_was_tried_of_a_host_no_longer_pinned_is_dropped_at_the_next_start() {
            let (policy, _dir) = policy();
            let filters = filters("");
            policy.rejected("com.app", "a.example");
            policy.process_started("com.app");
            let ticket = try_of(&policy, &filters, "a.example").unwrap();
            policy.try_refused("com.app", "a.example", ticket);
            assert_eq!(policy.retries.lock().unwrap().len(), 1);
            // Gone old, as after 30 days.
            policy
                .pinned
                .lock()
                .unwrap()
                .get_mut("com.app")
                .unwrap()
                .insert("a.example".to_owned(), 0);
            policy.process_started("com.other");
            assert_eq!(policy.retries.lock().unwrap().len(), 0);
        }

        #[test]
        fn a_try_goes_with_its_app() {
            let (policy, _dir) = policy();
            let filters = filters("");
            policy.rejected("com.app", "a.example");
            policy.process_started("com.app");
            policy.keep_only(&std::collections::BTreeSet::new());
            policy.rejected("com.app", "a.example");
            assert_eq!(try_of(&policy, &filters, "a.example"), None);
        }
    }
}
