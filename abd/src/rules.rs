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

//! Selected app sockets are matched by full UID. The TCP capture mark
//! reroutes packets through a separate loopback table and TPROXY delivers
//! them with their original addresses. UDP follows its Android route.
//!
//! A shared quota match gates TCP marking. Stop matching before removing
//! the loopback routing rules or waiting for the xtables lock. A stale
//! MARK alone would still make the kernel reroute, losing the original
//! route selected by Android for the app.

use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::{ErrorKind, Write as _},
    path::Path,
    process::{Command, Stdio},
};

use anyhow::Context as _;

use crate::capture::{MARK_BYPASS, MARK_CAPTURE};

/// The routing table that delivers to `lo`. Android numbers its own from
/// 1000, one for each interface, and the kernel's are 253 to 255.
const TABLE: &str = "233";
/// Before netd's rules, which start at 10000.
const PREF: &str = "100";

const OUT: &str = "adb_uid_out";
const MARK: &str = "adb_mark";
const PRE: &str = "adb_proxy_in";
const GUARD: &str = "adb_capture_guard";
const QUIC: &str = "adb_quic_out";

const GATE_NAME: &str = "abd_capture";
// In packet mode the charge is one. Some kernels require quota > charge.
const GATE_OPEN: u8 = 2;
const CHAINS: [(&str, &str); 5] = [
    ("mangle", OUT),
    ("mangle", MARK),
    ("mangle", PRE),
    ("mangle", GUARD),
    ("filter", QUIC),
];

const TOOLS: &str = "/system/bin";
const LOCK_WAIT: &str = "5";
const NOT_THERE: i32 = 1;
const MAX_UIDS: usize = 4096;
const MAX_RULES_BYTES: usize = 1024 * 1024;

/// The two address families, each with tables of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    pub(crate) const BOTH: [Family; 2] = [Family::V4, Family::V6];

    pub(crate) fn tool(self) -> &'static str {
        match self {
            Family::V4 => "iptables",
            Family::V6 => "ip6tables",
        }
    }

    /// The tool that replaces whole chains of the family.
    pub(crate) fn restore(self) -> &'static str {
        match self {
            Family::V4 => "iptables-restore",
            Family::V6 => "ip6tables-restore",
        }
    }

    /// What `ip` takes to mean the family.
    fn ip(self) -> &'static str {
        match self {
            Family::V4 => "-4",
            Family::V6 => "-6",
        }
    }

    fn loopback(self) -> &'static str {
        match self {
            Family::V4 => "127.0.0.1",
            Family::V6 => "::1",
        }
    }

    /// Destinations that are never a server on the internet: the device
    /// itself and the networks it is attached to.
    fn local(self) -> &'static [&'static str] {
        match self {
            Family::V4 => &[
                "127.0.0.0/8",
                "10.0.0.0/8",
                "172.16.0.0/12",
                "192.168.0.0/16",
                "169.254.0.0/16",
            ],
            Family::V6 => &["::1/128", "fe80::/10", "fc00::/7"],
        }
    }
}

/// Runs the tools of the packet filter and of routing.
pub(crate) trait Tools {
    fn run(&mut self, tool: &str, args: &[&str], input: &str) -> anyhow::Result<i32>;
    fn route_present(&mut self, family: Family) -> anyhow::Result<bool>;
    fn set_gate(&mut self, enabled: bool) -> anyhow::Result<()>;
    fn gate_open(&mut self) -> anyhow::Result<bool>;
}

pub(crate) struct System;

impl System {
    fn gate_value(path: &Path) -> anyhow::Result<u8> {
        let counter = File::open(path).context("reading capture gate")?;
        let actual = abd_engine::files::read_limited(counter, 32)?;
        actual.trim().parse().context("invalid capture gate value")
    }
}

/// A tool of the system's, never one found first elsewhere in the path,
/// and killed if it runs past its time: none may hold up its caller.
pub(crate) fn device_command(tool: &str) -> Command {
    let mut command = Command::new(Path::new(TOOLS).join("timeout"));
    let seconds = if tool == "ip" { "2" } else { "8" };
    command
        .args(["-k", "1", seconds])
        .arg(Path::new(TOOLS).join(tool));
    command
}

impl Tools for System {
    fn run(&mut self, tool: &str, args: &[&str], input: &str) -> anyhow::Result<i32> {
        let mut child = device_command(tool)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("running {tool}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            // If the tool does not read the whole input, its status reports
            // what failed.
            let _ = stdin.write_all(input.as_bytes());
        }
        child
            .wait()
            .with_context(|| format!("waiting for {tool}"))?
            .code()
            .with_context(|| format!("{tool} did not finish"))
    }

    fn route_present(&mut self, family: Family) -> anyhow::Result<bool> {
        let capture = mark(MARK_CAPTURE);
        let mut child = device_command("ip")
            .args([
                family.ip(),
                "rule",
                "show",
                "fwmark",
                &capture,
                "lookup",
                TABLE,
                "pref",
                PREF,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("reading policy routing rules")?;
        let stdout = child.stdout.take().context("policy routing output")?;
        let result = abd_engine::files::read_limited(stdout, MAX_RULES_BYTES);
        anyhow::ensure!(
            child.wait()?.success(),
            "reading policy routing rules failed"
        );
        Ok(!result?.trim().is_empty())
    }

    fn set_gate(&mut self, enabled: bool) -> anyhow::Result<()> {
        let path = Path::new("/proc/net/xt_quota").join(GATE_NAME);
        let mut counter = match OpenOptions::new().write(true).open(&path) {
            Ok(counter) => counter,
            Err(e) if !enabled && e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).context("opening capture gate"),
        };
        let value = if enabled { GATE_OPEN } else { 0 };
        counter
            .write_all(format!("{value}\n").as_bytes())
            .context("writing capture gate")?;
        anyhow::ensure!(
            Self::gate_value(&path)? == value,
            "capture gate did not change"
        );
        Ok(())
    }

    fn gate_open(&mut self) -> anyhow::Result<bool> {
        let path = Path::new("/proc/net/xt_quota").join(GATE_NAME);
        Ok(Self::gate_value(&path)? == GATE_OPEN)
    }
}

fn mark(bits: u32) -> String {
    format!("{bits:#x}/{bits:#x}")
}

/// A range includes only adjacent UIDs in the chosen set.
pub(crate) fn uid_ranges(uids: &BTreeSet<u32>) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for &uid in uids {
        match ranges.last_mut() {
            Some((_, end)) if end.checked_add(1) == Some(uid) => *end = uid,
            _ => ranges.push((uid, uid)),
        }
    }
    ranges
}

/// Named chains are replaced by restore; the remaining table is retained.
fn chains(family: Family, port: u16, ranges: &[(u32, u32)]) -> String {
    use std::fmt::Write as _;

    let (capture, bypass) = (mark(MARK_CAPTURE), mark(MARK_BYPASS));
    let mut text = format!(
        "*mangle\n\
         :{OUT} - [0:0]\n\
         :{MARK} - [0:0]\n\
         :{PRE} - [0:0]\n\
         :{GUARD} - [0:0]\n\
         -A {OUT} -m mark --mark {bypass} -j RETURN\n"
    );
    for net in family.local() {
        writeln!(text, "-A {OUT} -d {net} -j RETURN").unwrap();
    }
    // Goto stops the search after the first match, then returns from OUT
    // so the remaining OUTPUT rules still run.
    for &(start, end) in ranges {
        writeln!(
            text,
            "-A {OUT} -m owner --uid-owner {start}-{end} -g {MARK}"
        )
        .unwrap();
    }
    // Rules create the shared named counter closed. They read it once per
    // matching packet without consuming its value.
    let gate = format!("-m quota2 --name {GATE_NAME} --quota 0 --packets --no-change");
    let ours = format!("{MARK_CAPTURE:#x}/{:#x}", MARK_CAPTURE | MARK_BYPASS);
    write!(
        text,
        "-A {MARK} {gate} -j MARK --set-xmark {capture}\n\
         -A {PRE} -p tcp -m multiport --dports 80,443 -m mark --mark {ours} \
         -j TPROXY --on-ip {lo} --on-port {port} --tproxy-mark {capture}\n\
         -A {GUARD} -m mark --mark {ours} -j DROP\n\
         COMMIT\n",
        lo = family.loopback(),
    )
    .unwrap();
    writeln!(text, "*filter\n:{QUIC} - [0:0]").unwrap();
    writeln!(text, "-A {QUIC} -m mark --mark {bypass} -j RETURN").unwrap();
    for net in family.local() {
        writeln!(text, "-A {QUIC} -d {net} -j RETURN").unwrap();
    }
    let unreachable = match family {
        Family::V4 => "icmp-port-unreachable",
        Family::V6 => "icmp6-port-unreachable",
    };
    for &(start, end) in ranges {
        writeln!(
            text,
            "-A {QUIC} -m owner --uid-owner {start}-{end} {gate} -j REJECT --reject-with {unreachable}"
        )
        .unwrap();
    }
    text.push_str("COMMIT\n");
    text
}

/// A rule in a chain of the system's that sends packets to one of ours.
pub(crate) struct Jump {
    pub(crate) table: &'static str,
    pub(crate) from: &'static str,
    pub(crate) packets: &'static [&'static str],
    pub(crate) chain: &'static str,
}

impl Jump {
    fn run(&self, tools: &mut dyn Tools, tool: &str, action: &str) -> anyhow::Result<i32> {
        let mut args = vec!["-t", self.table, action, self.from];
        args.extend_from_slice(self.packets);
        args.extend_from_slice(&["-j", self.chain]);
        single(tools, tool, &args)
    }
}

const JUMPS: [Jump; 4] = [
    Jump {
        table: "mangle",
        from: "PREROUTING",
        packets: &["-i", "lo", "-p", "tcp"],
        chain: PRE,
    },
    // An independently removed policy rule must not let a marked packet
    // escape the loopback route, for example on a VPN's underlying network.
    Jump {
        table: "mangle",
        from: "POSTROUTING",
        packets: &["!", "-o", "lo", "-p", "tcp"],
        chain: GUARD,
    },
    Jump {
        table: "filter",
        from: "OUTPUT",
        packets: &["!", "-o", "lo", "-p", "udp", "--dport", "443"],
        chain: QUIC,
    },
    // OUTPUT sees the original route. In particular, an address assigned
    // to this device is routed through lo before we set our mark.
    Jump {
        table: "mangle",
        from: "OUTPUT",
        packets: &[
            "!",
            "-o",
            "lo",
            "-p",
            "tcp",
            "-m",
            "multiport",
            "--dports",
            "80,443",
        ],
        chain: OUT,
    },
];

fn single(tools: &mut dyn Tools, tool: &str, action: &[&str]) -> anyhow::Result<i32> {
    let args: Vec<&str> = ["-w", LOCK_WAIT].iter().chain(action).copied().collect();
    tools.run(tool, &args, "")
}

fn arm(tools: &mut dyn Tools, family: Family) -> anyhow::Result<()> {
    let ip = family.ip();
    let code = tools.run(
        "ip",
        &[
            ip, "route", "replace", "local", "default", "dev", "lo", "table", TABLE,
        ],
        "",
    )?;
    anyhow::ensure!(code == 0, "ip {ip} route replace failed ({code})");
    let capture = mark(MARK_CAPTURE);
    let code = tools.run(
        "ip",
        &[
            ip, "rule", "add", "fwmark", &capture, "lookup", TABLE, "pref", PREF,
        ],
        "",
    )?;
    anyhow::ensure!(code == 0, "ip {ip} rule add failed ({code})");
    Ok(())
}

/// A failed deletion may mean that the rule was already gone. Confirm
/// that distinction before changing selectors or rearming the gate.
fn disarm(tools: &mut dyn Tools, family: Family) -> anyhow::Result<()> {
    let capture = mark(MARK_CAPTURE);
    let args = [
        family.ip(),
        "rule",
        "del",
        "fwmark",
        &capture,
        "lookup",
        TABLE,
        "pref",
        PREF,
    ];
    for _ in 0..8 {
        if !matches!(tools.run("ip", &args, ""), Ok(0)) {
            break;
        }
    }
    let result = (|| {
        anyhow::ensure!(
            !tools.route_present(family)?,
            "could not remove the {} capture routing rule",
            family.ip()
        );
        Ok(())
    })();
    if result.is_err() {
        // The route itself is an independent way of disabling the gate.
        // Keep the error so nobody publishes active capture or rearms it.
        remove_route(tools, family);
    }
    result
}

fn disarm_all(tools: &mut dyn Tools) -> anyhow::Result<()> {
    let gate = tools.set_gate(false);
    let results = Family::BOTH.map(|family| disarm(tools, family));
    gate.and(results.into_iter().collect())
}

fn remove_route(tools: &mut dyn Tools, family: Family) {
    // If there is no route, the tool reports an error.
    let _ = tools.run(
        "ip",
        &[
            family.ip(),
            "route",
            "del",
            "local",
            "default",
            "dev",
            "lo",
            "table",
            TABLE,
        ],
        "",
    );
}

/// Writes the chains of `text` in one restore, then links each of
/// `jumps` that is not there yet, in order.
pub(crate) fn install_chains(
    tools: &mut dyn Tools,
    family: Family,
    text: &str,
    jumps: &[Jump],
    linked: bool,
) -> anyhow::Result<()> {
    let tool = family.tool();
    let restore = family.restore();
    let code = tools.run(restore, &["--noflush", "-w", LOCK_WAIT], text)?;
    anyhow::ensure!(code == 0, "{restore} failed ({code})");
    if !linked {
        for jump in jumps {
            match jump.run(tools, tool, "-C")? {
                0 => {}
                NOT_THERE => {
                    let code = jump.run(tools, tool, "-I")?;
                    anyhow::ensure!(code == 0, "{tool} -I failed ({code})");
                }
                code => anyhow::bail!("{tool} -C failed ({code})"),
            }
        }
    }
    Ok(())
}

/// All jumps go before deleting their chains, in the order given. Stop on
/// a family whose lock could not be acquired; the caller still cleans up
/// the other family.
pub(crate) fn remove_chains<'a>(
    tools: &mut dyn Tools,
    family: Family,
    chains: &[(&'static str, &'static str)],
    jumps: impl Iterator<Item = &'a Jump>,
) -> anyhow::Result<()> {
    let tool = family.tool();
    let mut present = Vec::new();
    for &(table, chain) in chains {
        if listed(tools, tool, table, chain)? {
            present.push((table, chain));
        }
    }
    for jump in jumps {
        if !present.contains(&(jump.table, jump.chain)) {
            continue;
        }
        for _ in 0..8 {
            match jump.run(tools, tool, "-D")? {
                0 => {}
                NOT_THERE => break,
                code => anyhow::bail!("{tool} -D failed ({code})"),
            }
        }
    }
    // The chain was there, so a failure here is one only if it still is:
    // the same status means it was gone, deleted by somebody else, or that
    // it could not be, as when a jump the deletions missed refers to it.
    for action in ["-F", "-X"] {
        for &(table, chain) in &present {
            let code = single(tools, tool, &["-t", table, action, chain])?;
            anyhow::ensure!(
                code == 0 || (code == NOT_THERE && !listed(tools, tool, table, chain)?),
                "{tool} {action} failed ({code})"
            );
        }
    }
    Ok(())
}

fn listed(tools: &mut dyn Tools, tool: &str, table: &str, chain: &str) -> anyhow::Result<bool> {
    match single(tools, tool, &["-t", table, "-S", chain])? {
        0 => Ok(true),
        NOT_THERE => Ok(false),
        code => anyhow::bail!("{tool} -S failed ({code})"),
    }
}

pub fn unhook() -> anyhow::Result<()> {
    Rules::with(Box::new(System), 0).remove()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Unknown,
    Absent,
    Linked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Families {
    Ipv4,
    DualStack,
}

impl From<bool> for Families {
    fn from(ipv6: bool) -> Self {
        if ipv6 { Self::DualStack } else { Self::Ipv4 }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Owners {
    Confirmed,
    Unconfirmed,
}

struct Active {
    uids: BTreeSet<u32>,
    families: Families,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Capture {
    Keep,
    Rearm,
}

struct Plan<'a> {
    uids: &'a BTreeSet<u32>,
    families: Families,
    capture: Capture,
}

impl<'a> Plan<'a> {
    fn new(
        active: Option<&Active>,
        uids: &'a BTreeSet<u32>,
        families: Families,
        owners: Owners,
    ) -> Option<Self> {
        if active.is_some_and(|old| {
            old.uids == *uids
                && old.families == families
                && (owners == Owners::Confirmed || uids.is_empty())
        }) {
            return None;
        }
        let capture = if owners == Owners::Confirmed
            && !uids.is_empty()
            && active.is_some_and(|old| !old.uids.is_empty() && old.families == families)
        {
            Capture::Keep
        } else {
            Capture::Rearm
        };
        Some(Self {
            uids,
            families,
            capture,
        })
    }

    fn wants(&self, family: Family) -> bool {
        !self.uids.is_empty() && (family == Family::V4 || self.families == Families::DualStack)
    }
}

/// The last confirmed active set and the chains staged in the kernel.
pub struct Rules {
    tools: Box<dyn Tools>,
    active: Option<Active>,
    state: [State; 2],
    port: u16,
}

impl Rules {
    pub fn start(port: u16) -> anyhow::Result<Self> {
        anyhow::ensure!(
            [
                "timeout",
                "ip",
                "iptables",
                "ip6tables",
                "iptables-restore",
                "ip6tables-restore"
            ]
            .iter()
            .all(|name| Path::new(TOOLS).join(name).exists()),
            "this device has no packet filter tools"
        );
        Ok(Self::with(Box::new(System), port))
    }

    fn with(tools: Box<dyn Tools>, port: u16) -> Self {
        Self {
            tools,
            active: None,
            state: [State::Unknown; 2],
            port,
        }
    }

    /// Filter the apps of `uids` in the chosen address families.
    /// An update may leave capture running when every previous UID still
    /// belongs to the confirmed chosen app, including an app being paused.
    pub fn set(
        &mut self,
        uids: &BTreeSet<u32>,
        families: Families,
        owners: Owners,
    ) -> anyhow::Result<()> {
        let Some(plan) = Plan::new(self.active.as_ref(), uids, families, owners) else {
            return Ok(());
        };
        self.active = None;
        if plan.capture == Capture::Rearm {
            disarm_all(self.tools.as_mut())?;
        }
        if let Err(error) = self.apply(&plan) {
            return match disarm_all(self.tools.as_mut()) {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("also failed to disarm capture: {cleanup:#}")))
                }
            };
        }
        self.active = Some(Active {
            uids: uids.clone(),
            families,
        });
        Ok(())
    }

    fn apply(&mut self, plan: &Plan<'_>) -> anyhow::Result<()> {
        anyhow::ensure!(
            plan.uids.len() <= MAX_UIDS,
            "too many selected UIDs for capture"
        );
        let wanted = || {
            Family::BOTH
                .into_iter()
                .filter(|&family| plan.wants(family))
        };
        if plan.capture == Capture::Keep {
            // Both existing capture paths must still be ready before
            // either family starts taking traffic from a newly chosen UID.
            for family in wanted() {
                anyhow::ensure!(
                    self.tools.route_present(family)?,
                    "the {} capture routing rule disappeared",
                    family.ip()
                );
            }
        }
        let ranges = uid_ranges(plan.uids);
        for family in Family::BOTH {
            self.stage_family(family, plan, &ranges)?;
        }
        if plan.capture == Capture::Keep {
            anyhow::ensure!(
                self.tools.gate_open()?,
                "capture gate closed during rule update"
            );
        } else {
            for family in wanted() {
                arm(self.tools.as_mut(), family)?;
            }
            if !plan.uids.is_empty() {
                self.tools.set_gate(true)?;
            }
        }
        Ok(())
    }

    fn stage_family(
        &mut self,
        family: Family,
        plan: &Plan<'_>,
        ranges: &[(u32, u32)],
    ) -> anyhow::Result<()> {
        let state = &mut self.state[family as usize];
        let before = std::mem::replace(state, State::Unknown);
        if plan.wants(family) {
            install_chains(
                self.tools.as_mut(),
                family,
                &chains(family, self.port, ranges),
                &JUMPS,
                before == State::Linked,
            )?;
            *state = State::Linked;
        } else {
            if before != State::Absent {
                remove_chains(self.tools.as_mut(), family, &CHAINS, JUMPS.iter().rev())?;
                remove_route(self.tools.as_mut(), family);
            }
            *state = State::Absent;
        }
        Ok(())
    }

    pub fn remove(&mut self) -> anyhow::Result<()> {
        self.active = None;
        let disarmed = disarm_all(self.tools.as_mut());
        for family in Family::BOTH {
            remove_route(self.tools.as_mut(), family);
        }
        let cleanup = Family::BOTH.map(|family| {
            let result = remove_chains(self.tools.as_mut(), family, &CHAINS, JUMPS.iter().rev());
            self.state[family as usize] = if result.is_ok() {
                State::Absent
            } else {
                State::Unknown
            };
            result
        });
        let cleanup: anyhow::Result<()> = cleanup.into_iter().collect();
        disarmed.and(cleanup)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

    use super::*;

    #[derive(Default)]
    struct Kernel {
        routes: [bool; 2],
        gates: [bool; 2],
        capture_gate: bool,
        reset_gate_on_restore: bool,
        staged: BTreeMap<String, String>,
        commands: Vec<String>,
        blocked: Vec<&'static str>,
        jumps: BTreeSet<String>,
        chains: BTreeSet<(String, String, String)>,
        /// Chains somebody else deletes as soon as they are listed.
        taken: BTreeSet<&'static str>,
    }

    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<Kernel>>);

    impl Tools for Fake {
        fn run(&mut self, tool: &str, args: &[&str], input: &str) -> anyhow::Result<i32> {
            let mut kernel = self.0.borrow_mut();
            let command = format!("{tool} {}", args.join(" "));
            let blocked = kernel
                .blocked
                .iter()
                .any(|blocked| command.starts_with(blocked));
            kernel.commands.push(command);
            if blocked {
                return Ok(4);
            }
            if tool == "ip" {
                let family = usize::from(args[0] == "-6");
                return Ok(match (args[1], args[2]) {
                    ("rule", "add") => {
                        assert!(kernel.routes[family]);
                        kernel.gates[family] = true;
                        0
                    }
                    ("rule", "del") => i32::from(!std::mem::take(&mut kernel.gates[family])),
                    ("route", "replace") => {
                        kernel.routes[family] = true;
                        0
                    }
                    ("route", "del") => i32::from(!std::mem::take(&mut kernel.routes[family])),
                    _ => panic!("unexpected route change {args:?}"),
                });
            }
            if let Some(tool) = tool.strip_suffix("-restore") {
                if kernel.reset_gate_on_restore {
                    kernel.capture_gate = false;
                }
                let mut table = "";
                for line in input.lines() {
                    if let Some(name) = line.strip_prefix('*') {
                        table = name;
                    } else if let Some(name) = line.strip_prefix(':') {
                        let chain = name.split_ascii_whitespace().next().unwrap();
                        kernel
                            .chains
                            .insert((tool.to_owned(), table.to_owned(), chain.to_owned()));
                    }
                }
                kernel.staged.insert(tool.to_owned(), input.to_owned());
                return Ok(0);
            }
            let key = (tool.to_owned(), args[3].to_owned(), args[5].to_owned());
            let jump = format!("{tool} {} {}", args[3], args[5..].join(" "));
            let referred = kernel.jumps.iter().any(|jump| {
                jump.starts_with(&format!("{tool} {} ", args[3]))
                    && jump.ends_with(&format!(" -j {}", args[5]))
            });
            Ok(match args[4] {
                "-S" => {
                    let listed = kernel.chains.contains(&key);
                    if kernel.taken.remove(args[5]) {
                        kernel.chains.remove(&key);
                    }
                    i32::from(!listed)
                }
                "-C" => i32::from(!kernel.jumps.contains(&jump)),
                "-I" => {
                    kernel.jumps.insert(jump);
                    0
                }
                "-D" => i32::from(!kernel.jumps.remove(&jump)),
                "-F" => i32::from(!kernel.chains.contains(&key)),
                // Like the kernel, refuse a chain that a rule still jumps to.
                "-X" if referred => 1,
                "-X" => i32::from(!kernel.chains.remove(&key)),
                _ => panic!("unexpected xtables change {tool} {args:?}"),
            })
        }

        fn route_present(&mut self, family: Family) -> anyhow::Result<bool> {
            let mut kernel = self.0.borrow_mut();
            let command = format!("ip {} rule show", family.ip());
            let blocked = kernel
                .blocked
                .iter()
                .any(|blocked| command.starts_with(blocked));
            kernel.commands.push(command);
            anyhow::ensure!(!blocked, "could not read policy rules");
            Ok(kernel.gates[family as usize])
        }

        fn set_gate(&mut self, enabled: bool) -> anyhow::Result<()> {
            let mut kernel = self.0.borrow_mut();
            let command = if enabled { "gate on" } else { "gate off" };
            kernel.commands.push(command.to_owned());
            anyhow::ensure!(
                !kernel.blocked.contains(&command),
                "could not write capture gate"
            );
            assert!(!enabled || kernel.gates[0]);
            kernel.capture_gate = enabled;
            Ok(())
        }

        fn gate_open(&mut self) -> anyhow::Result<bool> {
            let mut kernel = self.0.borrow_mut();
            kernel.commands.push("gate read".to_owned());
            Ok(kernel.capture_gate)
        }
    }

    fn rules() -> (Rules, Fake) {
        let fake = Fake::default();
        (Rules::with(Box::new(fake.clone()), 40000), fake)
    }

    fn chosen() -> BTreeSet<u32> {
        BTreeSet::from([10001, 1_010_001])
    }

    #[test]
    fn empty_selection_does_not_install_capture() {
        let (mut rules, fake) = rules();
        rules
            .set(&BTreeSet::new(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        let kernel = fake.0.borrow();
        assert!(kernel.chains.is_empty() && kernel.jumps.is_empty());
        assert_eq!(kernel.gates, [false; 2]);
        assert!(kernel.staged.is_empty());
    }

    #[test]
    fn both_families_are_staged_before_either_route_is_armed() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        let kernel = fake.0.borrow();
        assert_eq!(kernel.gates, [true; 2]);
        assert_eq!(kernel.chains.len(), 10);
        assert_eq!(kernel.jumps.len(), 8);
        let last_iptables = kernel
            .commands
            .iter()
            .rposition(|c| c.contains("tables"))
            .unwrap();
        let first_arm = kernel
            .commands
            .iter()
            .position(|c| c.contains("rule add"))
            .unwrap();
        let last_arm = kernel
            .commands
            .iter()
            .rposition(|c| c.contains("rule add"))
            .unwrap();
        let open = kernel.commands.iter().position(|c| c == "gate on").unwrap();
        assert!(last_iptables < first_arm && last_arm < open);
        for input in kernel.staged.values() {
            assert!(input.contains("--uid-owner 10001-10001"));
            assert!(input.contains("--uid-owner 1010001-1010001"));
            assert!(!input.contains("--object-pinned"));
        }
    }

    #[test]
    fn owners_cover_only_the_chosen_full_uid_ranges() {
        let uids = BTreeSet::from([10001, 10002, 10004, 1_010_001, u32::MAX]);
        assert_eq!(
            uid_ranges(&uids),
            [
                (10001, 10002),
                (10004, 10004),
                (1_010_001, 1_010_001),
                (u32::MAX, u32::MAX)
            ]
        );
        let text = chains(Family::V6, 40000, &uid_ranges(&uids));
        assert_eq!(text.matches(&format!("-g {MARK}")).count(), 4);
        assert!(text.contains(&format!("--uid-owner 10001-10002 -g {MARK}")));
        assert!(text.contains("-j TPROXY --on-ip ::1 --on-port 40000"));
        assert!(text.contains(&format!("--mark {} -j RETURN", mark(MARK_BYPASS))));
    }

    #[test]
    fn tcp_marking_requires_the_gate_and_a_nonloopback_route() {
        let mark = JUMPS.iter().find(|jump| jump.chain == OUT).unwrap();
        assert_eq!(mark.from, "OUTPUT");
        assert!(mark.packets.starts_with(&["!", "-o", "lo", "-p", "tcp"]));
        let text = chains(Family::V4, 40000, &uid_ranges(&chosen()));
        let gate = format!("-m quota2 --name {GATE_NAME} --quota 0 --packets --no-change");
        let marking = text.lines().find(|line| line.contains("-j MARK")).unwrap();
        assert!(marking.contains(&gate));
    }

    #[test]
    fn udp_443_rejection_follows_the_shared_gate_and_active_apps_in_both_families() {
        let (mut rules, fake) = rules();
        let jump = JUMPS.iter().find(|jump| jump.chain == QUIC).unwrap();
        assert_eq!(jump.table, "filter");
        assert_eq!(jump.from, "OUTPUT");
        assert_eq!(
            jump.packets,
            ["!", "-o", "lo", "-p", "udp", "--dport", "443"]
        );
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        for (family, reject) in [
            (Family::V4, "icmp-port-unreachable"),
            (Family::V6, "icmp6-port-unreachable"),
        ] {
            let kernel = fake.0.borrow();
            let text = &kernel.staged[family.tool()];
            assert!(text.contains(&format!(
                "-A {QUIC} -m mark --mark {} -j RETURN",
                mark(MARK_BYPASS)
            )));
            for uid in chosen() {
                assert!(text.contains(&format!(
                    "-A {QUIC} -m owner --uid-owner {uid}-{uid} -m quota2 --name {GATE_NAME} --quota 0 --packets --no-change -j REJECT --reject-with {reject}"
                )));
            }
        }
        rules
            .set(
                &BTreeSet::from([1_010_001]),
                Families::DualStack,
                Owners::Confirmed,
            )
            .unwrap();
        assert!(fake.0.borrow().staged.values().all(|text| {
            text.lines()
                .filter(|line| line.contains("REJECT"))
                .all(|line| line.contains("1010001"))
        }));
    }

    #[test]
    fn a_marked_packet_cannot_escape_from_the_vpn_if_the_route_is_lost() {
        let guard = JUMPS.iter().find(|jump| jump.chain == GUARD).unwrap();
        assert_eq!(guard.from, "POSTROUTING");
        assert_eq!(guard.packets, ["!", "-o", "lo", "-p", "tcp"]);
        let ours = format!("{MARK_CAPTURE:#x}/{:#x}", MARK_CAPTURE | MARK_BYPASS);
        for family in Family::BOTH {
            let text = chains(family, 40000, &uid_ranges(&chosen()));
            assert!(text.contains(&format!("-A {GUARD} -m mark --mark {ours} -j DROP")));
        }
    }

    #[test]
    fn an_unchanged_selection_performs_no_kernel_writes() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        let before = fake.0.borrow().commands.len();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        assert_eq!(fake.0.borrow().commands.len(), before);
    }

    #[test]
    fn confirmed_pause_and_resume_leave_both_capture_paths_running() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        let work_only = BTreeSet::from([1_010_001]);
        for (selection, personal) in [(&work_only, false), (&chosen(), true)] {
            fake.0.borrow_mut().commands.clear();
            rules
                .set(selection, Families::DualStack, Owners::Confirmed)
                .unwrap();
            let kernel = fake.0.borrow();
            assert!(kernel.capture_gate);
            assert_eq!(kernel.gates, [true; 2]);
            assert!(kernel.commands.iter().all(|command| {
                command.contains("rule show")
                    || command.contains("tables-restore")
                    || command == "gate read"
            }));
            assert_eq!(kernel.commands.len(), 5);
            assert!(kernel.staged.values().all(|text| {
                text.contains("--uid-owner 1010001")
                    && text.contains("--uid-owner 10001") == personal
            }));
        }
    }

    #[test]
    fn a_capture_gate_reset_during_a_live_update_leaves_capture_disarmed() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        fake.0.borrow_mut().reset_gate_on_restore = true;
        let work_only = BTreeSet::from([1_010_001]);
        assert!(
            rules
                .set(&work_only, Families::DualStack, Owners::Confirmed)
                .is_err()
        );
        let kernel = fake.0.borrow();
        assert!(!kernel.capture_gate);
        assert_eq!(kernel.gates, [false; 2]);
    }

    #[test]
    fn an_unconfirmed_owner_cannot_be_replaced_while_capture_is_live() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        fake.0.borrow_mut().commands.clear();
        // Android can reuse a UID even when the numeric set stays the same.
        rules
            .set(&chosen(), Families::DualStack, Owners::Unconfirmed)
            .unwrap();
        let kernel = fake.0.borrow();
        assert_eq!(
            kernel.commands.first().map(String::as_str),
            Some("gate off")
        );
        assert_eq!(kernel.commands.last().map(String::as_str), Some("gate on"));
    }

    #[test]
    fn an_app_cannot_enter_a_live_capture_with_a_missing_family_route() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        {
            let mut kernel = fake.0.borrow_mut();
            kernel.gates[1] = false;
            kernel.commands.clear();
        }
        let wanted = BTreeSet::from([10001, 10003, 1_010_001]);
        assert!(
            rules
                .set(&wanted, Families::DualStack, Owners::Confirmed)
                .is_err()
        );
        let kernel = fake.0.borrow();
        assert!(!kernel.capture_gate);
        assert_eq!(kernel.gates, [false; 2]);
        assert!(!kernel.commands.iter().any(|c| c.contains("tables-restore")));
    }

    #[test]
    fn removal_disarms_both_families_before_the_first_xtables_lock() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        {
            let mut kernel = fake.0.borrow_mut();
            kernel.blocked = vec!["iptables "];
            kernel.commands.clear();
        }
        assert!(rules.remove().is_err());
        let kernel = fake.0.borrow();
        assert_eq!(
            kernel.commands.first().map(String::as_str),
            Some("gate off")
        );
        assert!(!kernel.capture_gate);
        assert_eq!(kernel.gates, [false; 2]);
        assert_eq!(kernel.routes, [false; 2]);
        assert!(kernel.chains.iter().all(|(tool, _, _)| tool == "iptables"));
        // Stale kernel UID selectors are still installed throughout the
        // lock contention, but the MARK cannot execute.
        assert!(kernel.jumps.iter().any(|jump| jump.ends_with(OUT)));
        let first_xtables = kernel
            .commands
            .iter()
            .position(|c| c.contains("tables"))
            .unwrap();
        assert!(
            kernel.commands[..first_xtables]
                .iter()
                .any(|c| c.starts_with("ip -6 rule del"))
        );
    }

    #[test]
    fn locked_update_disarms_both_families_and_retries_the_same_selection() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        fake.0.borrow_mut().blocked = vec!["ip6tables-restore "];
        let remaining = BTreeSet::from([10001]);
        assert!(
            rules
                .set(&remaining, Families::DualStack, Owners::Confirmed)
                .is_err()
        );
        assert_eq!(fake.0.borrow().gates, [false; 2]);
        assert!(!fake.0.borrow().capture_gate);

        fake.0.borrow_mut().blocked.clear();
        rules
            .set(&remaining, Families::DualStack, Owners::Confirmed)
            .unwrap();
        let kernel = fake.0.borrow();
        assert_eq!(kernel.gates, [true; 2]);
        assert!(
            kernel
                .staged
                .values()
                .all(|text| !text.contains("--uid-owner 1010001"))
        );
    }

    #[test]
    fn routing_failure_after_one_family_arms_leaves_no_gate_active() {
        let (mut rules, fake) = rules();
        fake.0.borrow_mut().blocked = vec!["ip -6 rule add "];
        assert!(
            rules
                .set(&chosen(), Families::DualStack, Owners::Confirmed)
                .is_err()
        );
        assert_eq!(fake.0.borrow().gates, [false; 2]);
    }

    #[test]
    fn a_failed_quota_write_cannot_leave_the_capture_routes_armed() {
        let (mut rules, fake) = rules();
        fake.0.borrow_mut().blocked = vec!["gate on"];
        assert!(
            rules
                .set(&chosen(), Families::DualStack, Owners::Confirmed)
                .is_err()
        );
        let kernel = fake.0.borrow();
        assert_eq!(kernel.gates, [false; 2]);
        assert!(!kernel.capture_gate);
    }

    #[test]
    fn a_route_is_removed_if_disarming_its_rule_cannot_be_confirmed() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        fake.0.borrow_mut().blocked = vec!["ip -4 rule del", "ip -4 rule show"];
        assert!(
            rules
                .set(&chosen(), Families::Ipv4, Owners::Confirmed)
                .is_err()
        );
        let kernel = fake.0.borrow();
        assert_eq!(kernel.gates, [true, false]);
        assert!(!kernel.routes[0]);
    }

    #[test]
    fn a_chain_a_jump_still_refers_to_is_not_reported_removed() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        // A jump that the deletions do not match, as one made by hand.
        fake.0
            .borrow_mut()
            .jumps
            .insert(format!("iptables mangle OUTPUT -j {OUT}"));
        assert!(rules.remove().is_err());
        let kernel = fake.0.borrow();
        assert!(!kernel.capture_gate);
        assert_eq!(kernel.gates, [false; 2]);
        assert!(kernel.chains.contains(&(
            "iptables".to_owned(),
            "mangle".to_owned(),
            OUT.to_owned()
        )));
        // The other family is cleaned up all the same.
        assert!(kernel.chains.iter().all(|(tool, _, _)| tool == "iptables"));
    }

    #[test]
    fn a_chain_somebody_else_deleted_meanwhile_counts_as_removed() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        fake.0.borrow_mut().taken = BTreeSet::from([OUT, QUIC]);
        rules.remove().unwrap();
        let kernel = fake.0.borrow();
        assert!(kernel.chains.is_empty() && kernel.jumps.is_empty());
    }

    #[test]
    fn ipv4_can_run_without_capturing_ipv6() {
        let (mut rules, fake) = rules();
        rules
            .set(&chosen(), Families::DualStack, Owners::Confirmed)
            .unwrap();
        rules
            .set(&chosen(), Families::Ipv4, Owners::Confirmed)
            .unwrap();
        let kernel = fake.0.borrow();
        assert_eq!(kernel.gates, [true, false]);
        assert_eq!(kernel.routes, [true, false]);
        assert!(kernel.chains.iter().all(|(tool, _, _)| tool == "iptables"));
    }
}
