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

//! The commands that ask what the user chose and what was done, and change
//! it: the same whatever program runs the engine, and answered the same.
//!
//! With `--json` a command writes what `protocol.rs` makes. A command that
//! changes what the running engine goes by has it read again ([`Host`]).
//! A command or arguments not known here fail as [`Malformed`]. What only
//! one program can do, and what goes on for as long as it is listened to,
//! is that program's to add.

mod allow;
mod app;
mod lists;
mod pause;
mod query;
mod render;
mod rules;
mod settings;
pub mod update;

use std::collections::BTreeSet;
use std::io::{Read, Write};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::apps;
use crate::malformed::Malformed;
use crate::paths::Paths;
use crate::protocol;
use crate::settings::Settings;

/// The program that runs the engine, as the commands need it.
pub trait Host {
    /// The process the engine runs in, if it runs.
    fn running(&self) -> Option<i32>;
    /// Has the running engine read again what the commands change. Does
    /// nothing if none runs.
    fn reload(&self);
    /// A VPN session owns the in-memory pins even without an active tunnel.
    fn manages_pins(&self) -> bool {
        self.running().is_some()
    }
    /// Asks the owner to apply pending removals. An in-process owner can
    /// report a write failure before the command starts waiting for it.
    fn reload_pins(&self) -> Result<()> {
        self.reload();
        Ok(())
    }
    /// The version the user has installed.
    fn version(&self) -> Option<String>;
    /// Since when the program has kept itself turned off, if it has, for
    /// the user to turn on again.
    fn boot_guard(&self) -> Option<u64>;
    /// Whether a running VPN also carries IPv6.
    fn vpn_ipv6(&self) -> Option<bool> {
        None
    }
    /// Why no app can be decrypted where the engine runs, whatever the
    /// settings say, if none can: as `effective.https.reason` names it.
    fn undecryptable(&self) -> Option<&'static str> {
        None
    }
    /// Managed profiles allowed to use their externally installed CA.
    fn trusted_profiles(&self) -> BTreeSet<u32> {
        BTreeSet::new()
    }
    /// Confirmed app selection names in the personal profile, after resolving exclusions.
    fn personal_app_identities(&self, _exclusions: &BTreeSet<&str>) -> Vec<String> {
        Vec::new()
    }

    fn report_app_eligibility(&self, _apps: &mut [protocol::App]) {}
    /// A host that reports active filtering uses zero when its state is unavailable.
    fn reports_filtering_apps(&self) -> bool {
        false
    }
    /// Checks proposed selections and the identities that would be added.
    fn validate_selection(&self, apps: &[&str], _added: &[&str]) -> Result<()> {
        if let Some(app) = apps.iter().find(|app| !apps::valid_package(app)) {
            bail!(Malformed(format!("not a package name: {app:?}")));
        }
        Ok(())
    }
}

/// With no engine running: the commands change the files and nothing more.
#[cfg(test)]
pub struct Nobody;

#[cfg(test)]
impl Host for Nobody {
    fn running(&self) -> Option<i32> {
        None
    }
    fn reload(&self) {}
    fn version(&self) -> Option<String> {
        None
    }
    fn boot_guard(&self) -> Option<u64> {
        None
    }
}

fn print_json(out: &mut dyn Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *out, value)?;
    writeln!(out)?;
    Ok(())
}

pub fn parse_update_args(args: &[&str]) -> Result<update::Options> {
    let (json, args) = args
        .strip_prefix(&["--json"])
        .map_or((false, args), |rest| (true, rest));
    let list = match args {
        [] => None,
        ["--list", url] if crate::sources::valid(url) => Some((*url).to_owned()),
        _ => bail!(Malformed(
            "update takes [--json] [--list <https-url>]".to_owned()
        )),
    };
    Ok(update::Options { json, list })
}

pub fn parse_watch_args<'a>(
    args: &[&'a str],
    valid_app: impl FnOnce(&str) -> bool,
) -> Result<Option<&'a str>> {
    let args = args.strip_prefix(&["--json"]).unwrap_or(args);
    match args {
        [] => Ok(None),
        ["--app", name] if valid_app(name) => Ok(Some(*name)),
        ["--app", name] => bail!(Malformed(format!("not an app identity: {name:?}"))),
        _ => bail!(Malformed("watch takes --json and --app <app>".to_owned())),
    }
}

fn app_identity(name: &str) -> Result<&str> {
    if apps::Identity::parse(name).is_none() {
        bail!(Malformed(format!("not an app identity: {name:?}")));
    }
    Ok(name)
}

/// Does one command: `input` is what it is given to read, where it reads
/// any, and `out` where it answers.
pub fn run(
    paths: &Paths,
    host: &dyn Host,
    args: &[&str],
    input: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<()> {
    run_with_defaults(paths, host, Settings::UNSAID, args, input, out)
}

pub(crate) fn run_with_defaults(
    paths: &Paths,
    host: &dyn Host,
    defaults: Settings,
    args: &[&str],
    input: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<()> {
    let ctx = Ctx {
        paths,
        host,
        defaults,
    };
    let run = match args {
        [] => bail!(Malformed("no command given".to_owned())),
        ["app", "pause" | "resume", ..] => return pause::app(&ctx, args),
        ["app", ..] => app::run,
        ["pause" | "resume", ..] => pause::run,
        [
            "scope" | crate::settings::URL_FILTERING | crate::settings::COSMETIC,
            ..,
        ] => settings::run,
        ["allow", ..] => allow::run,
        ["list" | "lists", ..] => lists::run,
        ["rules", ..] => rules::run,
        ["status" | "overview" | "stats" | "check" | "pinned", ..] => query::run,
        _ => return unknown(args),
    };
    run(&ctx, args, input, out)
}

struct Ctx<'a> {
    paths: &'a Paths,
    host: &'a dyn Host,
    defaults: Settings,
}

impl Ctx<'_> {
    fn settings(&self) -> std::io::Result<Settings> {
        Settings::try_read(&self.paths.settings(), self.defaults)
    }
}

fn unknown(args: &[&str]) -> Result<()> {
    bail!(Malformed(format!("unknown command: {}", args.join(" "))))
}

fn changed(host: &dyn Host) -> Result<()> {
    host.reload();
    Ok(())
}

#[cfg(test)]
mod tests;
