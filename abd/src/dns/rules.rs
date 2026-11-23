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

//! The rules that bring the filtered apps' lookups here.
//!
//! netd sends an app's lookups from sockets it gives to the app: the
//! packet filter sees them as root's, but policy routing sees the app. So
//! a policy rule for each range of active UIDs, protocol and family sends
//! port 53 to our table, whose local route delivers to the loopback
//! interface; a TPROXY rule there hands what came by that route to the
//! listener.
//!
//! The policy rules go in last and come out first: TPROXY without them
//! does nothing, but they without TPROXY would leave the apps' lookups
//! with nobody to answer them. They need no packet filter lock.

use std::collections::BTreeSet;
use std::io;

use anyhow::{Context as _, ensure};

use super::netlink::{self, Netlink, Rule, Source};
use super::{CHAIN, SOURCE_V4, SOURCE_V6};
use crate::rules::{Families, Family, LOCK_WAIT, NOT_THERE, Tools, single, uid_ranges};

/// The most rules of a family: one for each protocol and source of each
/// of the most UID ranges capture takes, and room for what a run before
/// left.
const MAX_RULES: usize = 4 * 4096 + 64;
const SOURCES: [Source; 2] = [Source::Unset, Source::Anchor];
const PROTOCOLS: [u8; 2] = [libc::IPPROTO_UDP as u8, libc::IPPROTO_TCP as u8];
const JUMPS: [&[&str]; 2] = [
    &["-i", "lo", "-p", "udp", "--dport", "53"],
    &["-i", "lo", "-p", "tcp", "--dport", "53"],
];

/// What changes the kernel's rules.
pub(super) trait Kernel {
    fn tools(&mut self) -> &mut dyn Tools;
    /// Adds a rule, or deletes it; one already as asked is no failure.
    fn rule(&mut self, add: bool, rule: &Rule) -> io::Result<()>;
    /// Deletes a rule of ours of the family, whichever; false if none is left.
    fn any_rule(&mut self, v6: bool) -> io::Result<bool>;
}

pub(super) struct System {
    tools: crate::rules::System,
    netlink: Netlink,
}

impl System {
    pub(super) fn open() -> io::Result<Self> {
        Ok(Self {
            tools: crate::rules::System,
            netlink: Netlink::open()?,
        })
    }
}

impl Kernel for System {
    fn tools(&mut self) -> &mut dyn Tools {
        &mut self.tools
    }

    fn rule(&mut self, add: bool, rule: &Rule) -> io::Result<()> {
        let already = if add { libc::EEXIST } else { libc::ENOENT };
        match self.netlink.change(netlink::rule(add, rule)) {
            Err(error) if error.raw_os_error() == Some(already) => Ok(()),
            done => done,
        }
    }

    fn any_rule(&mut self, v6: bool) -> io::Result<bool> {
        match self.netlink.change(netlink::any_rule(v6)) {
            Ok(()) => Ok(true),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chain {
    Unknown,
    Absent,
    Linked,
}

/// The rules in the kernel, as far as they are known.
pub(super) struct Rules {
    kernel: Box<dyn Kernel>,
    /// Where TPROXY delivers.
    port: u16,
    /// Whether IPv6 lookups have a route to be brought by.
    v6: bool,
    /// None when what is there is not known: all of ours go first.
    installed: Option<BTreeSet<Rule>>,
    chains: [Chain; 2],
}

impl Rules {
    pub(super) fn new(kernel: Box<dyn Kernel>, port: u16, v6: bool) -> Self {
        Self {
            kernel,
            port,
            v6,
            installed: None,
            chains: [Chain::Unknown; 2],
        }
    }

    /// Where TPROXY is to deliver, and whether IPv6 lookups have a route.
    pub(super) fn serve(&mut self, port: u16, v6: bool) {
        self.port = port;
        self.v6 = v6;
    }

    fn wanted(&self, uids: &BTreeSet<u32>, families: Families) -> BTreeSet<Rule> {
        let v6 = self.v6 && families == Families::DualStack;
        let ranges = uid_ranges(uids);
        [false, true]
            .into_iter()
            .filter(|&family| !family || v6)
            .flat_map(|family| {
                ranges.iter().flat_map(move |&uids| {
                    PROTOCOLS.into_iter().flat_map(move |proto| {
                        SOURCES.map(|source| Rule {
                            v6: family,
                            uids,
                            proto,
                            source,
                        })
                    })
                })
            })
            .collect()
    }

    /// Brings the lookups of `uids` here, and only theirs. A failure
    /// leaves none brought.
    pub(super) fn set(&mut self, uids: &BTreeSet<u32>, families: Families) -> anyhow::Result<()> {
        let wanted = self.wanted(uids, families);
        let settled = Family::BOTH.iter().all(|&family| {
            let chain = self.chains[family as usize];
            chain
                == if has(&wanted, family) {
                    Chain::Linked
                } else {
                    Chain::Absent
                }
        });
        if settled && self.installed.as_ref() == Some(&wanted) {
            return Ok(());
        }
        let applied = self.apply(&wanted);
        if applied.is_err()
            && let Err(error) = self.withdraw()
        {
            return applied.context(format!("also failed to withdraw: {error:#}"));
        }
        applied
    }

    fn apply(&mut self, wanted: &BTreeSet<Rule>) -> anyhow::Result<()> {
        ensure!(
            wanted.len() <= 2 * MAX_RULES,
            "too many UID ranges for lookup rules"
        );
        for family in Family::BOTH {
            if has(wanted, family) {
                let linked = self.chains[family as usize] == Chain::Linked;
                self.chains[family as usize] = Chain::Unknown;
                install_chain(self.kernel.tools(), family, self.port, linked)?;
                self.chains[family as usize] = Chain::Linked;
            }
        }
        let current = match self.installed.take() {
            Some(current) => current,
            None => {
                self.withdraw()?;
                self.installed.take().unwrap_or_default()
            }
        };
        for rule in wanted.difference(&current) {
            self.kernel
                .rule(true, rule)
                .context("adding a lookup rule")?;
        }
        for rule in current.difference(wanted) {
            self.kernel
                .rule(false, rule)
                .context("removing a lookup rule")?;
        }
        self.installed = Some(wanted.clone());
        for family in Family::BOTH {
            if !has(wanted, family) && self.chains[family as usize] != Chain::Absent {
                self.chains[family as usize] = Chain::Unknown;
                remove_chain(self.kernel.tools(), family)?;
                self.chains[family as usize] = Chain::Absent;
            }
        }
        Ok(())
    }

    /// Takes every rule of ours away, whatever is known of them: the
    /// lookups are the system's again.
    pub(super) fn withdraw(&mut self) -> anyhow::Result<()> {
        self.installed = None;
        for v6 in [false, true] {
            let mut left = true;
            for _ in 0..MAX_RULES {
                left = self
                    .kernel
                    .any_rule(v6)
                    .context("withdrawing lookup rules")?;
                if !left {
                    break;
                }
            }
            ensure!(!left, "lookup rules are still there");
        }
        self.installed = Some(BTreeSet::new());
        Ok(())
    }

    /// Withdraws the rules, then takes the TPROXY rules away. Both
    /// families are tried whatever happens to either.
    pub(super) fn remove(&mut self) -> anyhow::Result<()> {
        let withdrawn = self.withdraw();
        let chains = Family::BOTH.map(|family| {
            self.chains[family as usize] = Chain::Unknown;
            let removed = remove_chain(self.kernel.tools(), family);
            if removed.is_ok() {
                self.chains[family as usize] = Chain::Absent;
            }
            removed
        });
        let chains: anyhow::Result<()> = chains.into_iter().collect();
        withdrawn.and(chains)
    }
}

fn has(rules: &BTreeSet<Rule>, family: Family) -> bool {
    rules.iter().any(|rule| rule.v6 == (family == Family::V6))
}

/// What came by our route, from the source it gives, to anywhere but
/// this device: to the listener at `port`.
fn chain(family: Family, port: u16) -> String {
    let (source, not_to, on) = match family {
        Family::V4 => (
            format!("{SOURCE_V4}/32"),
            "127.0.0.0/8".to_owned(),
            "127.0.0.1",
        ),
        Family::V6 => (
            format!("{SOURCE_V6}/128"),
            format!("{SOURCE_V6}/128"),
            "::1",
        ),
    };
    let mut text = format!("*mangle\n:{CHAIN} - [0:0]\n");
    for proto in ["udp", "tcp"] {
        text.push_str(&format!(
            "-A {CHAIN} -s {source} ! -d {not_to} -p {proto} -j TPROXY --on-ip {on} --on-port {port}\n"
        ));
    }
    text.push_str("COMMIT\n");
    text
}

fn jump(
    tools: &mut dyn Tools,
    family: Family,
    action: &str,
    packets: &[&str],
) -> anyhow::Result<i32> {
    let mut args = vec!["-t", "mangle", action, "PREROUTING"];
    args.extend_from_slice(packets);
    args.extend_from_slice(&["-j", CHAIN]);
    single(tools, family.tool(), &args)
}

fn install_chain(
    tools: &mut dyn Tools,
    family: Family,
    port: u16,
    linked: bool,
) -> anyhow::Result<()> {
    let restore = family.restore();
    let code = tools.run(
        restore,
        &["--noflush", "-w", LOCK_WAIT],
        &chain(family, port),
    )?;
    ensure!(code == 0, "{restore} failed ({code})");
    if !linked {
        for packets in JUMPS {
            match jump(tools, family, "-C", packets)? {
                0 => {}
                NOT_THERE => {
                    let code = jump(tools, family, "-I", packets)?;
                    ensure!(code == 0, "{} -I failed ({code})", family.tool());
                }
                code => anyhow::bail!("{} -C failed ({code})", family.tool()),
            }
        }
    }
    Ok(())
}

fn remove_chain(tools: &mut dyn Tools, family: Family) -> anyhow::Result<()> {
    let tool = family.tool();
    match single(tools, tool, &["-t", "mangle", "-S", CHAIN])? {
        0 => {}
        NOT_THERE => return Ok(()),
        code => anyhow::bail!("{tool} -S failed ({code})"),
    }
    for packets in JUMPS {
        for _ in 0..8 {
            match jump(tools, family, "-D", packets)? {
                0 => {}
                NOT_THERE => break,
                code => anyhow::bail!("{tool} -D failed ({code})"),
            }
        }
    }
    for action in ["-F", "-X"] {
        let code = single(tools, tool, &["-t", "mangle", action, CHAIN])?;
        ensure!(
            code == 0 || code == NOT_THERE,
            "{tool} {action} failed ({code})"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
