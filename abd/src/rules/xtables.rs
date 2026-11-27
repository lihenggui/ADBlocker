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

//! Installation and removal of packet filter chains and their jumps.

use super::{Family, Tools};

const LOCK_WAIT: &str = "5";
const NOT_THERE: i32 = 1;

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

fn single(tools: &mut dyn Tools, tool: &str, action: &[&str]) -> anyhow::Result<i32> {
    let args: Vec<&str> = ["-w", LOCK_WAIT].iter().chain(action).copied().collect();
    tools.run(tool, &args, "")
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
