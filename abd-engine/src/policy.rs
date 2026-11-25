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

//! What to do with a connection, decided from what its ClientHello shows,
//! and the memory of which hosts an app would not talk to through us.
//!
//! An app that pins its certificates will not accept ours. This is found
//! out, not configured: the app aborts the handshake on seeing our
//! certificate, or completes it and closes at once without a request. The
//! host is then passed through for that app.
//!
//! What is remembered is kept by package name, not UID: a UID is given to
//! another app once its app is removed. An entry is forgotten when a
//! process of the app accepts us for the host after all ([`retry`]), after
//! [`KEEP`], and when its app is no longer filtered.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use crate::allow::Allow;
use crate::clienthello::Hello;
use crate::clock::now;
use crate::exclusions::Exclusions;
use crate::filter::AppFilters;
use crate::limits::MAX_APPS_WITH_PAGES;
pub use crate::protocol_types::Reason;

mod retry;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Verdict {
    /// Refuse the connection.
    Block,
    /// Hand it on untouched.
    Passthrough(Reason),
    /// Decrypt it and filter the requests inside.
    Mitm,
}

/// An app, by package name, and a host it would not talk to through us.
type Pinned = (String, String);
type Pins = HashMap<String, HashMap<String, u64>>;

/// How long a host is taken as pinned, in seconds: 30 days.
const KEEP: u64 = 30 * 24 * 3600;
/// The most hosts remembered for one app. An app pins a handful; this is
/// reached only by one that means to fill the table, and then its own
/// oldest are forgotten: what is remembered of other apps is not its to
/// push out.
const MAX_PER_PACKAGE: usize = 256;
/// The most hosts remembered for their certificate, and for how long: a
/// site may change to another kind.
const MAX_SENSITIVE: usize = 4096;
const KEEP_SENSITIVE: u64 = 7 * 24 * 3600;
/// Closes without a request, in a row, before the host is taken as pinned.
/// One could be the user leaving a page as it began to load.
const SILENT_LIMIT: u32 = 3;
/// An app may count 256 hosts, well above the three-close threshold. At
/// that bound it forgets its own oldest host and leaves other apps' counts.
const MAX_SILENT_PER_PACKAGE: usize = 256;
/// At 1,024 hosts total, a new app with no entry waits for a free slot;
/// its connection passes on and no other app loses pin progress.
const MAX_SILENT: usize = 1024;

pub struct Policy {
    /// The apps that have made a request showing a page, since the start.
    /// Kept in memory only: an app is known again by its next page.
    apps_with_pages: Mutex<HashSet<String>>,
    /// When each was found, in seconds since the epoch.
    pinned: Mutex<Pins>,
    /// Connections in a row that completed the handshake and then closed
    /// at once without a request: how a pin checked after the handshake
    /// looks.
    silent: Mutex<Vec<(Pinned, u32)>>,
    /// The tries of pinned hosts given at an app's start. Taken after
    /// `pinned` where both are held.
    retries: Mutex<retry::Retries>,
    /// Whether it was said once that the tries' table is full.
    retries_full: AtomicBool,
    /// Hosts whose server showed an Extended Validation certificate, and
    /// when. Not written to the file: found again by asking the server.
    sensitive: Mutex<HashMap<String, u64>>,
    file: PathBuf,
    load_error: Option<io::ErrorKind>,
    /// A deferred write may only snapshot and replace the file in this
    /// order, including when a command is waiting for a removal.
    writing: Mutex<()>,
    /// Told when there is something to write.
    changed: Notify,
}

/// How long what was found may sit in memory before it is written. A
/// burst of findings is one write.
const WRITE_DELAY: Duration = Duration::from_secs(5);

/// What `file` remembers, without what is too old to keep by `now`.
pub fn read_pinned(file: &Path, now: u64) -> HashMap<Pinned, u64> {
    try_read_pinned(file, now).unwrap_or_default()
}

pub(crate) fn try_read_pinned(file: &Path, now: u64) -> io::Result<HashMap<Pinned, u64>> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(error),
    };
    Ok(text
        .lines()
        .filter_map(|l| {
            let mut f = l.split(' ');
            let when: u64 = f.next()?.parse().ok()?;
            let key = (f.next()?.to_owned(), f.next()?.to_owned());
            fresh(when, now).then_some((key, when))
        })
        .collect())
}

/// Whether a host found pinned at `when` is still taken as pinned at `now`.
/// The time is read from a file, so the sum saturates rather than aborts.
/// One dated ahead of the clock is kept: a clock not yet set at boot must
/// not cost the apps what was found of them.
fn fresh(when: u64, now: u64) -> bool {
    when.saturating_add(KEEP) > now
}

/// The saved table, with the time, package and host on each line.
fn pinned_text<'a>(pinned: impl Iterator<Item = (&'a str, &'a str, u64)>) -> String {
    pinned
        .map(|(package, host, when)| format!("{when} {package} {host}\n"))
        .collect()
}

pub(crate) fn write_pinned_durable(file: &Path, pinned: &HashMap<Pinned, u64>) -> io::Result<()> {
    let text = pinned_text(
        pinned
            .iter()
            .map(|((p, h), &when)| (p.as_str(), h.as_str(), when)),
    );
    crate::files::replace_durable(file, text.as_bytes())
}

fn runtime_pinned_text(pinned: &Pins) -> String {
    pinned_text(pinned.iter().flat_map(|(package, hosts)| {
        hosts
            .iter()
            .map(|(host, &when)| (package.as_str(), host.as_str(), when))
    }))
}

fn forget_pins(pinned: &mut Pins, package: &str, host: Option<&str>) -> bool {
    let Some(host) = host else {
        return pinned.remove(package).is_some();
    };
    let Some(hosts) = pinned.get_mut(package) else {
        return false;
    };
    let removed = hosts.remove(host).is_some();
    if hosts.is_empty() {
        pinned.remove(package);
    }
    removed
}

/// Removes from `pinned` the hosts of `package`, or only `host` of them.
/// Returns whether any was there.
pub fn forget(pinned: &mut HashMap<Pinned, u64>, package: &str, host: Option<&str>) -> bool {
    let before = pinned.len();
    pinned.retain(|(p, h), _| p != package || host.is_some_and(|host| host != h));
    pinned.len() != before
}

impl Policy {
    /// Reads what was remembered, without what is too old to keep.
    pub fn open(file: PathBuf) -> Self {
        let (pinned, load_error) = match try_read_pinned(&file, now()) {
            Ok(pinned) => (pinned, None),
            Err(error) => {
                eprintln!("pins: could not read the saved table: {error}");
                (HashMap::new(), Some(error.kind()))
            }
        };
        let mut by_package = Pins::new();
        for ((package, host), when) in pinned {
            by_package.entry(package).or_default().insert(host, when);
        }
        Self {
            apps_with_pages: Mutex::default(),
            pinned: Mutex::new(by_package),
            silent: Mutex::default(),
            retries: Mutex::default(),
            retries_full: AtomicBool::new(false),
            sensitive: Mutex::default(),
            file,
            load_error,
            writing: Mutex::default(),
            changed: Notify::new(),
        }
    }

    /// A request of `package` showed a page.
    pub(crate) fn note_page(&self, package: &str) {
        let mut apps = self.apps_with_pages.lock().unwrap();
        if apps.len() < MAX_APPS_WITH_PAGES && !apps.contains(package) {
            apps.insert(package.to_owned());
        }
    }

    /// Whether `package` has, or may have: once no more apps can be kept,
    /// any app may be one that was left out.
    pub(crate) fn has_shown_page(&self, package: &str) -> bool {
        let apps = self.apps_with_pages.lock().unwrap();
        apps.len() >= MAX_APPS_WITH_PAGES || apps.contains(package)
    }

    /// `domain_only`: only the app's hosts are judged and nothing of it is
    /// decrypted when URL filtering is off.
    pub fn decide(
        &self,
        package: &str,
        hello: &Hello,
        filters: AppFilters<'_>,
        exclusions: &Exclusions,
        allow: &Allow,
        domain_only: bool,
    ) -> Verdict {
        let Some(host) = hello.sni.as_deref() else {
            // No name: nothing to mint a certificate for that a client
            // checking names would accept.
            return Verdict::Passthrough(Reason::NoName);
        };
        self.decide_named(package, host, filters, exclusions, allow, domain_only)
    }

    /// What is done with a connection of `package` to `host`, and so with
    /// a lookup of the host: one answered as blocked is a connection that
    /// would be refused.
    ///
    /// A host whose requests will be seen waits for them while a web
    /// exception could apply to one. A connection that stays encrypted
    /// has no requests to apply an exception to: its host is judged by the
    /// domain rules, as with URL filtering off.
    pub fn decide_named(
        &self,
        package: &str,
        host: &str,
        filters: AppFilters<'_>,
        exclusions: &Exclusions,
        allow: &Allow,
        domain_only: bool,
    ) -> Verdict {
        if allow.covers(host) {
            return Verdict::Passthrough(Reason::Trusted);
        }
        if domain_only {
            return if filters.blocks_domain(host) {
                Verdict::Block
            } else {
                Verdict::Passthrough(Reason::DomainOnly)
            };
        }
        if filters.blocks_host(host) {
            return Verdict::Block;
        }
        match self.kept_encrypted(package, host, exclusions) {
            Some(_) if filters.blocks_domain(host) => Verdict::Block,
            Some(reason) => Verdict::Passthrough(reason),
            None => Verdict::Mitm,
        }
    }

    /// Why a connection of `package` to `host` is not to be decrypted with
    /// URL filtering on, if it is not.
    fn kept_encrypted(&self, package: &str, host: &str, exclusions: &Exclusions) -> Option<Reason> {
        if exclusions.covers(package, host) {
            return Some(Reason::Excluded);
        }
        if self.is_sensitive(host) {
            return Some(Reason::Sensitive);
        }
        // A try of a pinned host is no part of the decision: it is taken
        // for a connection that this passes through as pinned, and that
        // can be decrypted (`take_try`).
        if self.is_pinned(package, host) {
            return Some(Reason::Pinned);
        }
        // Until every exclusion list has been read, any host may be on one.
        (!exclusions.ready()).then_some(Reason::ExclusionsPending)
    }

    fn is_pinned(&self, package: &str, host: &str) -> bool {
        self.pinned
            .lock()
            .unwrap()
            .get(package)
            .and_then(|hosts| hosts.get(host))
            .is_some_and(|&when| fresh(when, now()))
    }

    /// Takes the try of a pinned host given at its app's start, for a
    /// connection that [`Policy::decide`] passes through as pinned and that
    /// can be decrypted: the ticket the connection ends it with, if there
    /// is a try to take.
    pub fn take_try(&self, package: &str, host: &str) -> Option<u64> {
        if !self.is_pinned(package, host) {
            return None;
        }
        self.retries.lock().unwrap().take((package, host), now())
    }

    /// A process of `package` has started, and trusts us: each host it
    /// pins may be tried once more.
    pub fn process_started(&self, package: &str) {
        let now = now();
        let pinned = self.pinned.lock().unwrap();
        let hosts = pinned
            .get(package)
            .into_iter()
            .flatten()
            .filter(|(_, when)| fresh(**when, now))
            .map(|(host, _)| host.as_str());
        let mut retries = self.retries.lock().unwrap();
        // What was tried of hosts no longer pinned, gone old or pushed out
        // by others of their app, is of no use.
        retries.retain(|p, h| {
            pinned
                .get(p)
                .and_then(|hosts| hosts.get(h))
                .is_some_and(|&when| fresh(when, now))
        });
        // Full only with as many hosts pinned as the table holds: those left
        // out stay pinned, and are tried again after their 30 days.
        if retries.grant(package, hosts, now) && !self.retries_full.swap(true, Relaxed) {
            eprintln!("pins: too many pinned hosts to try again; some wait for their 30 days");
        }
    }

    /// The app refused the try of `ticket`, or its connection ended
    /// without a request. The host was found pinned again: it is kept for
    /// [`KEEP`] from now.
    pub fn try_refused(&self, package: &str, host: &str, ticket: u64) {
        let mut pinned = self.pinned.lock().unwrap();
        if !self
            .retries
            .lock()
            .unwrap()
            .refused((package, host), ticket, now())
        {
            return;
        }
        if let Some(when) = pinned
            .get_mut(package)
            .and_then(|hosts| hosts.get_mut(host))
        {
            *when = now();
            self.changed.notify_one();
        }
    }

    /// The connection of the try of `ticket` is over. If it never came to
    /// show the app our certificate, the try is left for the next one.
    pub fn try_over(&self, package: &str, host: &str, ticket: u64) {
        self.retries
            .lock()
            .unwrap()
            .unresolved((package, host), ticket);
    }

    /// Forgets that `package` pins `host`, or every host it pins, so that
    /// it is decrypted again. Returns whether anything was forgotten.
    pub fn forget(&self, package: &str, host: Option<&str>) -> bool {
        let matches = |p: &str, h: &str| p == package && host.is_none_or(|host| host == h);
        let forgotten = forget_pins(&mut self.pinned.lock().unwrap(), package, host);
        self.silent
            .lock()
            .unwrap()
            .retain(|((p, h), _)| !matches(p, h));
        self.retries.lock().unwrap().retain(|p, h| !matches(p, h));
        forgotten
    }

    /// The server of `host` showed a certificate that says its
    /// connections are not to be decrypted (`tls::extended_validation`).
    /// Remembered, so that the next connection is handed on without first
    /// asking the server again.
    pub fn found_sensitive(&self, host: &str) {
        let mut sensitive = self.sensitive.lock().unwrap();
        if sensitive.len() >= MAX_SENSITIVE {
            // Found again at the cost of one handshake each.
            sensitive.clear();
        }
        sensitive.insert(host.to_owned(), now());
    }

    fn is_sensitive(&self, host: &str) -> bool {
        let sensitive = self.sensitive.lock().unwrap();
        sensitive
            .get(host)
            .is_some_and(|when| when.saturating_add(KEEP_SENSITIVE) > now())
    }

    /// The client refused our certificate during the handshake. Returns
    /// whether the host is newly taken as pinned.
    pub fn rejected(&self, package: &str, host: &str) -> bool {
        self.pin(package, host)
    }

    /// The client completed the handshake and closed at once, without a
    /// request. Returns whether the host is newly taken as pinned.
    pub fn closed_silent(&self, package: &str, host: &str) -> bool {
        let n = {
            let mut silent = self.silent.lock().unwrap();
            if let Some((_, count)) = silent
                .iter_mut()
                .find(|((counted, name), _)| counted == package && name == host)
            {
                *count = count.saturating_add(1);
                *count
            } else {
                let own = silent.iter().filter(|((app, _), _)| app == package).count();
                if own >= MAX_SILENT_PER_PACKAGE {
                    let oldest = silent
                        .iter()
                        .position(|((app, _), _)| app == package)
                        .expect("own count guarantees an entry");
                    silent.remove(oldest);
                } else if silent.len() >= MAX_SILENT {
                    return false;
                }
                silent.push(((package.to_owned(), host.to_owned()), 1));
                1
            }
        };
        n >= SILENT_LIMIT && self.pin(package, host)
    }

    /// The client sent a request: it accepts us for this host. On the try
    /// of `ticket`, it accepts us for a host it was found to pin, which is
    /// then forgotten. Any other connection says nothing of a pin: one
    /// decrypted before the host was pinned may be another client of the
    /// app's, which does not check its pins.
    pub fn spoke(&self, package: &str, host: &str, ticket: Option<u64>) {
        self.silent
            .lock()
            .unwrap()
            .retain(|((p, h), _)| p != package || h != host);
        let Some(ticket) = ticket else { return };
        let mut pinned = self.pinned.lock().unwrap();
        if self
            .retries
            .lock()
            .unwrap()
            .accepted((package, host), ticket)
            && forget_pins(&mut pinned, package, Some(host))
        {
            self.changed.notify_one();
        }
    }

    /// Forgets the apps that are not in `packages`: those no longer
    /// filtered, or removed from the device.
    pub fn keep_only(&self, packages: &BTreeSet<&str>) {
        let mut pinned = self.pinned.lock().unwrap();
        let before = pinned.len();
        pinned.retain(|package, _| packages.contains(package.as_str()));
        if pinned.len() != before {
            self.changed.notify_one();
        }
        self.silent
            .lock()
            .unwrap()
            .retain(|((package, _), _)| packages.contains(package.as_str()));
        self.retries
            .lock()
            .unwrap()
            .retain(|package, _| packages.contains(package));
    }

    fn pin(&self, package: &str, host: &str) -> bool {
        let mut pinned = self.pinned.lock().unwrap();
        let now = now();
        let own = pinned.entry(package.to_owned()).or_default();
        let new = own
            .insert(host.to_owned(), now)
            .is_none_or(|when| !fresh(when, now));
        if own.len() > MAX_PER_PACKAGE {
            let mut by_age: Vec<_> = own.iter().map(|(h, &when)| (when, h)).collect();
            by_age.sort_unstable();
            let expired: Vec<_> = by_age[..own.len() - MAX_PER_PACKAGE]
                .iter()
                .map(|(_, h)| (*h).clone())
                .collect();
            for host in expired {
                own.remove(&host);
            }
        }
        self.changed.notify_one();
        new
    }

    /// Writes the file as a whole, in one step.
    pub fn write(&self) -> io::Result<()> {
        self.check_loaded()?;
        let _writer = self.writing.lock().unwrap();
        let pinned = self.pinned.lock().unwrap().clone();
        crate::files::replace(&self.file, runtime_pinned_text(&pinned).as_bytes())
    }

    /// Keep the active pins until the requested removals can survive a restart.
    pub(crate) fn persist_forget(&self, requests: &[(&str, Option<&str>)]) -> io::Result<()> {
        self.check_loaded()?;
        let _writer = self.writing.lock().unwrap();
        let mut pinned = self.pinned.lock().unwrap().clone();
        for &(package, host) in requests {
            forget_pins(&mut pinned, package, host);
        }
        crate::files::replace_durable(&self.file, runtime_pinned_text(&pinned).as_bytes())?;
        // Connections can learn pins while the file is being written.
        // Preserve unrelated changes and publish the successful removals.
        for &(package, host) in requests {
            self.forget(package, host);
        }
        Ok(())
    }

    fn check_loaded(&self) -> io::Result<()> {
        if let Some(kind) = self.load_error {
            // Preserve an unreadable original until a new process can load it.
            return Err(io::Error::new(
                kind,
                "the saved pin table was unreadable at startup",
            ));
        }
        Ok(())
    }

    /// Writes what is found some time after it is, off the threads that
    /// carry the connections. While nothing is found no timer runs.
    pub fn keep_written(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                self.changed.notified().await;
                tokio::time::sleep(WRITE_DELAY).await;
                let policy = self.clone();
                match tokio::task::spawn_blocking(move || policy.write()).await {
                    Ok(Err(error)) => eprintln!("pins: could not save the table: {error}"),
                    Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                    _ => {}
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Filters;

    fn hello(host: Option<&str>) -> Hello {
        Hello {
            sni: host.map(str::to_owned),
            ..Hello::default()
        }
    }

    fn filters(rules: &str) -> Filters {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("rules.txt"), rules).unwrap();
        Filters::load(&[dir.path()], false).unwrap()
    }

    fn policy() -> (Policy, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (Policy::open(dir.path().join("pinned")), dir)
    }

    #[test]
    fn app_page_history_is_bounded_and_kept_by_identity() {
        let (policy, _dir) = policy();
        policy.note_page("12:com.app");
        policy.note_page("@shared.name");
        assert!(policy.has_shown_page("12:com.app"));
        assert!(policy.has_shown_page("@shared.name"));
        assert!(!policy.has_shown_page("com.app"));

        for n in 2..MAX_APPS_WITH_PAGES {
            policy.note_page(&format!("com.app{n}"));
        }
        policy.note_page("com.extra");
        assert!(policy.has_shown_page("com.app"));
        let kept = policy.apps_with_pages.lock().unwrap();
        assert_eq!(kept.len(), MAX_APPS_WITH_PAGES);
        assert!(!kept.contains("com.extra"));
    }

    #[test]
    fn decrypts_unless_the_host_is_blocked_whole_or_has_no_name() {
        let (policy, _dir) = policy();
        let filters = filters("||ads.example^\n||shop.example^$third-party\n");
        let decide = |host| {
            policy.decide(
                "com.app",
                &hello(host),
                filters.for_app("com.app"),
                &Exclusions::default().assume_ready(),
                &Allow::default(),
                false,
            )
        };
        assert_eq!(decide(Some("news.example")), Verdict::Mitm);
        assert_eq!(decide(Some("ads.example")), Verdict::Block);
        assert_eq!(decide(Some("sub.ads.example")), Verdict::Block);
        // A rule that depends on who asks says nothing about the host.
        assert_eq!(decide(Some("shop.example")), Verdict::Mitm);
        assert_eq!(decide(None), Verdict::Passthrough(Reason::NoName));
    }

    #[test]
    fn a_host_an_app_refused_us_for_is_passed_through_for_that_app_only() {
        let (policy, _dir) = policy();
        let filters = filters("");
        assert!(policy.rejected("com.bank", "api.bank.example"));
        assert!(!policy.rejected("com.bank", "api.bank.example"));
        let decide = |package, host| {
            policy.decide(
                package,
                &hello(Some(host)),
                filters.for_app(package),
                &Exclusions::default().assume_ready(),
                &Allow::default(),
                false,
            )
        };
        assert_eq!(
            decide("com.bank", "api.bank.example"),
            Verdict::Passthrough(Reason::Pinned)
        );
        assert_eq!(decide("com.bank", "www.bank.example"), Verdict::Mitm);
        assert_eq!(decide("com.browser", "api.bank.example"), Verdict::Mitm);
    }

    #[test]
    fn closing_without_a_request_counts_only_in_a_row() {
        let (policy, _dir) = policy();
        assert!(!policy.closed_silent("com.app", "h.example"));
        assert!(!policy.closed_silent("com.app", "h.example"));
        // A request in between: the app accepts us after all.
        policy.spoke("com.app", "h.example", None);
        assert!(!policy.closed_silent("com.app", "h.example"));
        assert!(!policy.closed_silent("com.app", "h.example"));
        assert!(policy.closed_silent("com.app", "h.example"));
    }

    #[test]
    fn an_app_that_fills_the_table_has_only_its_own_counts_forgotten() {
        let (policy, _dir) = policy();
        assert!(!policy.closed_silent("com.app", "h.example"));
        assert!(!policy.closed_silent("com.app", "h.example"));
        for n in 0..=MAX_SILENT {
            policy.closed_silent("com.other", &format!("{n}.example"));
        }
        // The third in a row, as if the other app had done nothing.
        assert!(policy.closed_silent("com.app", "h.example"));
    }

    #[test]
    fn a_new_apps_close_cannot_erase_another_apps_pin_progress_at_capacity() {
        let (policy, _dir) = policy();
        assert!(!policy.closed_silent("com.bank", "api.bank.example"));
        assert!(!policy.closed_silent("com.bank", "api.bank.example"));
        for n in 0..MAX_SILENT - 1 {
            assert!(!policy.closed_silent(&format!("com.other{n}"), "h.example"));
        }
        assert!(!policy.closed_silent("com.new", "h.example"));
        assert!(policy.closed_silent("com.bank", "api.bank.example"));
    }

    #[test]
    fn earlier_other_apps_cannot_erase_a_later_apps_pin_progress_at_capacity() {
        let (policy, _dir) = policy();
        assert!(!policy.closed_silent("com.other0", "h.example"));
        assert!(!policy.closed_silent("com.bank", "api.bank.example"));
        assert!(!policy.closed_silent("com.bank", "api.bank.example"));
        for n in 1..MAX_SILENT - 1 {
            assert!(!policy.closed_silent(&format!("com.other{n}"), "h.example"));
        }
        assert!(!policy.closed_silent("com.new", "h.example"));
        assert!(policy.closed_silent("com.bank", "api.bank.example"));
    }

    #[test]
    fn an_app_at_its_silent_limit_forgets_its_own_oldest_host() {
        let (policy, _dir) = policy();
        assert!(!policy.closed_silent("com.app", "old.example"));
        assert!(!policy.closed_silent("com.app", "old.example"));
        for n in 1..MAX_SILENT_PER_PACKAGE {
            assert!(!policy.closed_silent("com.app", &format!("{n}.example")));
        }
        assert!(!policy.closed_silent("com.app", "new.example"));
        assert!(!policy.closed_silent("com.app", "old.example"));
        assert!(!policy.closed_silent("com.app", "old.example"));
        assert!(policy.closed_silent("com.app", "old.example"));
    }

    #[test]
    fn what_was_found_is_remembered_by_the_next_daemon() {
        let (policy, dir) = policy();
        let before = now();
        policy.rejected("com.bank", "api.bank.example");
        let after = now();
        policy.write().unwrap();
        let file = dir.path().join("pinned");
        let saved = try_read_pinned(&file, after).unwrap();
        let when = saved[&("com.bank".to_owned(), "api.bank.example".to_owned())];
        assert!((before..=after).contains(&when));
        let next = Policy::open(file);
        let verdict = next.decide(
            "com.bank",
            &hello(Some("api.bank.example")),
            filters("").for_app("com.bank"),
            &Exclusions::default().assume_ready(),
            &Allow::default(),
            false,
        );
        assert_eq!(verdict, Verdict::Passthrough(Reason::Pinned));
    }

    #[test]
    fn a_pin_is_forgotten_when_it_is_old_and_can_be_found_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pinned");
        let old = now() - KEEP - 1;
        std::fs::write(
            &file,
            format!(
                "{old} com.bank old.example\n{} com.bank new.example\n",
                now()
            ),
        )
        .unwrap();
        let policy = Policy::open(file);
        let filters = filters("");
        let decide = |host| {
            policy.decide(
                "com.bank",
                &hello(Some(host)),
                filters.for_app("com.bank"),
                &Exclusions::default().assume_ready(),
                &Allow::default(),
                false,
            )
        };
        assert_eq!(decide("old.example"), Verdict::Mitm);
        assert_eq!(decide("new.example"), Verdict::Passthrough(Reason::Pinned));
        assert!(policy.rejected("com.bank", "old.example"));
    }

    #[test]
    fn an_app_no_longer_filtered_is_forgotten() {
        let (policy, dir) = policy();
        policy.rejected("com.gone", "a.example");
        policy.rejected("com.kept", "b.example");
        policy.keep_only(&BTreeSet::from(["com.kept"]));
        policy.write().unwrap();
        let text = std::fs::read_to_string(dir.path().join("pinned")).unwrap();
        assert!(text.contains("com.kept b.example") && !text.contains("com.gone"));
    }

    #[test]
    fn an_app_cannot_push_out_what_is_remembered_of_another() {
        let (policy, _dir) = policy();
        policy.rejected("com.bank", "api.bank.example");
        for n in 0..MAX_PER_PACKAGE + 50 {
            policy.rejected("com.hostile", &format!("h{n}.example"));
        }
        let pinned = policy.pinned.lock().unwrap();
        assert!(pinned["com.bank"].contains_key("api.bank.example"));
        let hostile = pinned["com.hostile"].len();
        assert!(hostile <= MAX_PER_PACKAGE, "{hostile}");
    }

    #[test]
    fn adding_pins_keeps_existing_hosts_until_the_package_reaches_capacity() {
        let (policy, _dir) = policy();
        let old = now().saturating_sub(60);
        policy.rejected("com.app", "old.example");
        *policy
            .pinned
            .lock()
            .unwrap()
            .get_mut("com.app")
            .unwrap()
            .get_mut("old.example")
            .unwrap() = old;
        for n in 1..MAX_PER_PACKAGE {
            assert!(policy.rejected("com.app", &format!("h{n}.example")));
        }
        assert!(policy.is_pinned("com.app", "old.example"));
        assert!(!policy.is_pinned("com.other", "old.example"));
        policy.process_started("com.app");
        let request = String::from("https://old.example");
        let host = &request["https://".len()..];
        assert!(policy.take_try("com.app", host).is_some());
        policy.spoke("com.app", host, None);
        assert!(policy.is_pinned("com.app", host));
        assert!(policy.rejected("com.app", "new.example"));
        assert!(!policy.is_pinned("com.app", host));
        assert_eq!(
            policy.pinned.lock().unwrap()["com.app"].len(),
            MAX_PER_PACKAGE
        );
    }

    #[test]
    fn a_sensitive_host_at_the_end_of_time_is_kept_without_overflow() {
        let (policy, _dir) = policy();
        policy
            .sensitive
            .lock()
            .unwrap()
            .insert("bank.example".to_owned(), u64::MAX);
        assert!(policy.is_sensitive("bank.example"));
    }

    #[test]
    fn a_host_found_sensitive_or_excluded_by_a_list_is_never_decrypted() {
        let (policy, _dir) = policy();
        let filters = filters("");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("banks.txt"), "bank.example\n").unwrap();
        let exclusions = Exclusions::load(dir.path()).assume_ready();
        let decide = |host| {
            let allow = Allow::default();
            policy.decide(
                "com.app",
                &hello(Some(host)),
                filters.for_app("com.app"),
                &exclusions,
                &allow,
                false,
            )
        };
        assert_eq!(
            decide("login.bank.example"),
            Verdict::Passthrough(Reason::Excluded)
        );
        assert_eq!(decide("pay.example"), Verdict::Mitm);
        policy.found_sensitive("pay.example");
        assert_eq!(
            decide("pay.example"),
            Verdict::Passthrough(Reason::Sensitive)
        );
        assert_eq!(decide("other.example"), Verdict::Mitm);
    }

    #[test]
    fn a_host_kept_encrypted_is_judged_by_its_domain_rules_in_full_mode() {
        let (policy, _dir) = policy();
        // The image exception leaves ads.example open only while its
        // requests can still be seen.
        let filters = filters(
            "||ads.example^\n||pass.example^\n@@||pass.example^\n@@||ads.example/downloads/$image\n",
        );
        let decide = |host, exclusions: &Exclusions| {
            let allow = Allow::default();
            policy.decide_named(
                "com.app",
                host,
                filters.for_app("com.app"),
                exclusions,
                &allow,
                false,
            )
        };
        let ready = Exclusions::default().assume_ready();
        assert_eq!(decide("ads.example", &ready), Verdict::Mitm);
        for host in ["ads.example", "pass.example", "other.example"] {
            policy.rejected("com.app", host);
        }
        assert_eq!(decide("ads.example", &ready), Verdict::Block);
        let pinned = Verdict::Passthrough(Reason::Pinned);
        assert_eq!(decide("pass.example", &ready), pinned);
        assert_eq!(decide("other.example", &ready), pinned);
        // Another app does not pin it.
        let other = policy.decide_named(
            "com.other",
            "ads.example",
            filters.for_app("com.other"),
            &ready,
            &Allow::default(),
            false,
        );
        assert_eq!(other, Verdict::Mitm);

        // Until the exclusion lists are read, nothing is decrypted, and so
        // every host is judged by its domain rules.
        let pending = Exclusions::default();
        assert_eq!(decide("ads.example", &pending), Verdict::Block);
        assert_eq!(
            policy.decide_named(
                "com.other",
                "news.example",
                filters.for_app("com.other"),
                &pending,
                &Allow::default(),
                false
            ),
            Verdict::Passthrough(Reason::ExclusionsPending)
        );
    }

    #[test]
    fn a_host_blocked_whole_is_refused_even_if_a_list_excludes_it() {
        let (policy, _dir) = policy();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("l.txt"), "ads.bank.example\n").unwrap();
        let verdict = policy.decide(
            "com.app",
            &hello(Some("ads.bank.example")),
            filters("||ads.bank.example^\n").for_app("com.app"),
            &Exclusions::load(dir.path()).assume_ready(),
            &Allow::default(),
            false,
        );
        assert_eq!(verdict, Verdict::Block);
    }

    fn allow(hosts: &str) -> Allow {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("allow"), hosts).unwrap();
        Allow::load(&dir.path().join("allow"))
    }

    #[test]
    fn a_trusted_host_is_handed_on_even_if_a_rule_blocks_it_whole() {
        let (policy, _dir) = policy();
        let filters = filters("||ads.example^\n");
        let allow = allow("cdn.ads.example\n");
        let decide = |host, domain_only| {
            let hello = hello(Some(host));
            policy.decide(
                "com.app",
                &hello,
                filters.for_app("com.app"),
                &Exclusions::default().assume_ready(),
                &allow,
                domain_only,
            )
        };
        let trusted = Verdict::Passthrough(Reason::Trusted);
        assert_eq!(decide("cdn.ads.example", false), trusted);
        assert_eq!(decide("img.cdn.ads.example", true), trusted);
        assert_eq!(decide("ads.example", false), Verdict::Block);
    }

    #[test]
    fn turning_off_url_filtering_still_blocks_whole_domains() {
        let (policy, _dir) = policy();
        let filters = filters("||ads.example^\n");
        let decide = |host| {
            let (hello, none) = (hello(Some(host)), Exclusions::default().assume_ready());
            policy.decide(
                "com.app",
                &hello,
                filters.for_app("com.app"),
                &none,
                &Allow::default(),
                true,
            )
        };
        assert_eq!(decide("ads.example"), Verdict::Block);
        assert_eq!(
            decide("news.example"),
            Verdict::Passthrough(Reason::DomainOnly)
        );
    }

    #[test]
    fn a_forgotten_pin_is_decrypted_again() {
        let (policy, _dir) = policy();
        policy.rejected("com.bank", "a.example");
        policy.rejected("com.bank", "b.example");
        policy.rejected("com.other", "a.example");
        let decide = |package, host| {
            let (hello, none) = (hello(Some(host)), Exclusions::default().assume_ready());
            policy.decide(
                package,
                &hello,
                filters("").for_app(package),
                &none,
                &Allow::default(),
                false,
            )
        };
        assert!(policy.forget("com.bank", Some("a.example")));
        assert!(!policy.forget("com.bank", Some("a.example")));
        assert_eq!(decide("com.bank", "a.example"), Verdict::Mitm);
        assert_eq!(
            decide("com.bank", "b.example"),
            Verdict::Passthrough(Reason::Pinned)
        );
        assert!(policy.forget("com.bank", None));
        assert_eq!(decide("com.bank", "b.example"), Verdict::Mitm);
        assert_eq!(
            decide("com.other", "a.example"),
            Verdict::Passthrough(Reason::Pinned)
        );
    }

    #[test]
    fn a_pin_dated_at_the_end_of_time_is_read_without_overflow() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pinned");
        let now = now();
        let old = now - KEEP;
        std::fs::write(
            &file,
            format!(
                "{} com.bank max.example\n{old} com.bank old.example\n",
                u64::MAX
            ),
        )
        .unwrap();
        let policy = Policy::open(file);
        let pinned = policy.pinned.lock().unwrap();
        let hosts: Vec<&str> = pinned["com.bank"]
            .keys()
            .map(|host| host.as_str())
            .collect();
        assert_eq!(hosts, ["max.example"]);
        assert!(fresh(u64::MAX, now) && !fresh(0, KEEP));
    }

    #[test]
    fn lines_that_are_not_entries_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pinned");
        std::fs::write(&file, "10145 old.format.example\n\ngarbage\n").unwrap();
        let policy = Policy::open(file);
        assert!(policy.pinned.lock().unwrap().is_empty());
    }
}
