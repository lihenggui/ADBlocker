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

//! The plain lookups of the filtered apps: brought here by policy routing
//! (`rules.rs`) through a device only this process holds (`anchor.rs`),
//! delivered by TPROXY (`listen.rs`), and judged by the engine
//! (`serve.rs`).
//!
//! Whatever goes wrong here leaves lookups as they were without us: a
//! failure to set up runs the daemon without filtering lookups, and a
//! daemon that dies takes the device, and so the routes, with it.

mod anchor;
mod listen;
mod netlink;
mod rules;
mod serve;
mod sys;

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use abd_engine::mitm::Ctx;
use anyhow::{Context as _, ensure};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use self::netlink::{Netlink, Rule, Source};
use crate::capture::MARK_BYPASS;
use crate::rules::Families;

/// The routing table of the anchor's routes. Android numbers its own from
/// 1000, the kernel's are 253 to 255, and TCP capture's is 233.
pub const TABLE: u32 = 234;
/// The priority of our rules: before TCP capture's (100) and netd's, which
/// start at 10000.
pub const PREF: u32 = 99;
/// The anchor device.
pub const DEVICE: &str = "abd_dns";
/// The mangle chain where TPROXY takes the lookups.
const CHAIN: &str = "adb_dns_in";
const DNS_PORT: u16 = 53;
/// The sources our routes give a lookup, by which TPROXY knows it: the
/// loopback address, and for IPv6 a unique local one only the anchor has.
const SOURCE_V4: Ipv4Addr = Ipv4Addr::LOCALHOST;
const SOURCE_V6: Ipv6Addr = Ipv6Addr::new(0xfd5c, 0x2b8e, 0xe1a7, 0, 0, 0, 0, 0x53);
/// A UID no socket has, for asking the kernel what a rule can match.
const PROBE_UID: u32 = u32::MAX - 1;

/// Lookup capture, while it runs.
pub struct Dns {
    running: Option<Running>,
}

struct Running {
    rules: rules::Rules,
    serving: JoinHandle<()>,
    failures: Failures,
}

/// Changes to the rules that failed in a row. Each change is tried again
/// at the next change of the apps, with no timer of its own; after
/// `MAX_FAILURES`, lookup capture ends until the daemon starts again.
#[derive(Default)]
struct Failures(u32);

const MAX_FAILURES: u32 = 3;

/// What to say of a change to the rules, and whether to go on.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Nothing to say.
    Quiet,
    /// The first failure in a row.
    Failed,
    /// A success after failures.
    Recovered,
    /// Too many failures: lookup capture ends.
    GiveUp,
}

impl Failures {
    fn record(&mut self, succeeded: bool) -> Outcome {
        let before = std::mem::take(&mut self.0);
        if succeeded {
            return if before > 0 {
                Outcome::Recovered
            } else {
                Outcome::Quiet
            };
        }
        self.0 = before + 1;
        match self.0 {
            1 => Outcome::Failed,
            n if n >= MAX_FAILURES => Outcome::GiveUp,
            _ => Outcome::Quiet,
        }
    }
}

impl Dns {
    /// Sets lookup capture up, with no app's lookups brought yet. What a
    /// run before left is taken away first. A failure is said and leaves
    /// the daemon filtering without it.
    pub fn start(ctx: &Arc<Ctx>) -> Self {
        let running = open(ctx)
            .inspect(|_| eprintln!("dns: the filtered apps' lookups are filtered"))
            .inspect_err(|error| eprintln!("dns: {error:#}; lookups are not filtered"))
            .ok();
        Self { running }
    }

    /// Brings the lookups of `uids` here, and only theirs.
    pub fn set(&mut self, uids: &BTreeSet<u32>, families: Families) {
        let Some(running) = &mut self.running else {
            return;
        };
        if running.serving.is_finished() {
            self.end();
            return;
        }
        let set = running.rules.set(uids, families);
        match running.failures.record(set.is_ok()) {
            Outcome::Quiet => {}
            Outcome::Recovered => eprintln!("dns: lookups are filtered again"),
            Outcome::Failed => {
                if let Err(error) = &set {
                    eprintln!("dns: {error:#}; lookups are not filtered for now");
                }
            }
            Outcome::GiveUp => {
                eprintln!("dns: the rules keep failing; lookups are not filtered until a restart");
                self.end();
            }
        }
    }

    /// Ends lookup capture: the rules go, and with the serving task the
    /// device and its routes.
    fn end(&mut self) {
        if let Some(mut running) = self.running.take() {
            if let Err(error) = running.rules.remove() {
                eprintln!("dns: {error:#}");
            }
            running.serving.abort();
        }
    }

    /// Takes our routing rules away: lookups are the system's again. It
    /// needs no lock, and comes before anything that waits for one.
    pub fn withdraw(&mut self) -> anyhow::Result<()> {
        match &mut self.running {
            Some(running) => running.rules.withdraw(),
            None => Ok(()),
        }
    }

    /// Takes everything else of ours away, after `withdraw`.
    pub fn remove(&mut self) -> anyhow::Result<()> {
        match &mut self.running {
            Some(running) => running.rules.remove(),
            None => Ok(()),
        }
    }
}

fn open(ctx: &Arc<Ctx>) -> anyhow::Result<Running> {
    let mut rules = rules::Rules::new(Box::new(rules::System::open()?), 0, false);
    let mut netlink = Netlink::open().context("opening rtnetlink")?;
    rules.withdraw()?;
    remove_device(&mut netlink)?;
    check_kernel(&mut netlink)?;
    let anchor = anchor::Anchor::open(&mut netlink)?;
    let listeners = listen::bind(anchor.v6).context("listening for lookups")?;
    rules.serve(listeners.port, listeners.v6);
    let stop = Arc::new(Notify::new());
    let host = Arc::new(listen::System::new(ctx.clone(), stop.clone()));
    let serving = tokio::spawn(listen::serve(host, stop, listeners, anchor));
    Ok(Running {
        rules,
        serving,
        failures: Failures::default(),
    })
}

/// Whether the kernel keeps what a rule of ours matches by. One that does
/// not know a port or a protocol in a rule takes it for a rule without
/// them, which would bring every packet of the app here. Asked of a rule
/// for a UID nobody has, while our table is still empty.
fn check_kernel(netlink: &mut Netlink) -> anyhow::Result<()> {
    let probe = Rule {
        v6: false,
        uids: (PROBE_UID, PROBE_UID),
        proto: libc::IPPROTO_UDP as u8,
        source: Source::Unset,
    };
    netlink
        .change(netlink::rule(true, &probe))
        .context("adding a routing rule")?;
    let listed = netlink.our_rules(false);
    let removed = netlink.change(netlink::rule(false, &probe));
    let listed = listed.context("listing routing rules")?;
    removed.context("removing a routing rule")?;
    ensure!(
        listed.iter().any(|rule| rule.uids == Some(probe.uids)
            && rule.proto == Some(probe.proto)
            && rule.ports == Some((DNS_PORT, DNS_PORT))
            && rule.mask == Some(MARK_BYPASS)),
        "this kernel cannot route by port and protocol"
    );
    Ok(())
}

fn remove_device(netlink: &mut Netlink) -> anyhow::Result<()> {
    match netlink.change(netlink::link_delete()) {
        Err(error) if error.raw_os_error() != Some(libc::ENODEV) => {
            Err(error).context("removing the anchor device")
        }
        _ => Ok(()),
    }
}

/// Takes our routing rules away with no daemon running, from any state:
/// before anything that waits for the packet filter's lock.
pub fn withdraw() -> anyhow::Result<()> {
    rules::Rules::new(Box::new(rules::System::open()?), 0, false).withdraw()
}

/// Takes what else may be left of lookup capture away, with no daemon
/// running: the TPROXY rules, the table's routes and the device. Each is
/// tried whatever happens to the others.
pub fn clear() -> anyhow::Result<()> {
    let chains = match rules::System::open() {
        Ok(system) => rules::Rules::new(Box::new(system), 0, false).remove(),
        // Without netlink our routing rules cannot be seen gone, but the
        // TPROXY rules go all the same, as after a failed withdrawal.
        Err(error) => {
            let removed: anyhow::Result<()> = rules::remove_tproxy(&mut crate::rules::System)
                .into_iter()
                .collect();
            removed.and(Err(error).context("opening rtnetlink"))
        }
    };
    let routes = Netlink::open()
        .context("opening rtnetlink")
        .and_then(|mut netlink| remove_routes(&mut netlink));
    chains.and(routes)
}

/// The table's routes and the device they go through.
fn remove_routes(netlink: &mut Netlink) -> anyhow::Result<()> {
    let routes = [false, true].map(|v6| match netlink.change(netlink::no_route(v6)) {
        Err(error) if !matches!(error.raw_os_error(), Some(libc::ESRCH | libc::ENOENT)) => {
            Err(error).context("removing a lookup route")
        }
        _ => Ok(()),
    });
    let device = remove_device(netlink);
    let routes: anyhow::Result<()> = routes.into_iter().collect();
    routes.and(device)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failing_rule_change_is_said_once_and_given_up_after_a_bound() {
        let mut failures = Failures::default();
        assert_eq!(failures.record(true), Outcome::Quiet);
        assert_eq!(failures.record(false), Outcome::Failed);
        assert_eq!(failures.record(true), Outcome::Recovered);
        assert_eq!(failures.record(true), Outcome::Quiet);
        let said: Vec<_> = (0..MAX_FAILURES).map(|_| failures.record(false)).collect();
        assert_eq!(said[0], Outcome::Failed);
        assert!(said[1..said.len() - 1].iter().all(|o| *o == Outcome::Quiet));
        assert_eq!(said.last(), Some(&Outcome::GiveUp));
    }
}
