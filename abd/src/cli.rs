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

//! The commands the module's scripts, the app and the user run, other than
//! the daemon itself and what only the module runs (`main.rs`).
//!
//! Most are the engine's (`abd_engine::cli`), answered here on standard
//! output with the daemon for the engine that runs. The two that go on for
//! as long as they are listened to are this program's. A command or
//! arguments nobody knows exit with status 2, which the app takes for a
//! module older than itself.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io::Write as _;
use std::sync::Arc;

use abd_engine::{ca, live, protocol, runtime};
use anyhow::{Result, ensure};

use crate::paths::{Module as _, Paths};
use crate::{android, handoff, lifecycle, system_roots};

pub const USAGE: &str = "\
usage: abd <command>, as root

  overview --json <start1> ... <start8>   status, daily counts and lists together
  status [--json]                     whether the daemon runs, and what it holds
  stats                               all-time filtering counters
  stats --json <start1> ... <start8>    seven days of counts, starts in Unix seconds
  app list [--json]                   show filtering apps and their effective levels
  app selections --json               show saved choices, exclusions and scope
  app exclude|include <app>            change a personal exclusion in All apps
  app configure-exclusions            replace personal exclusions from standard input
  scope chosen|all                    filter chosen apps or all eligible personal apps
  app profiles --json                 list work profiles and eligible packages
  app eligibility --json              report personal app eligibility
  app configure                       replace the chosen apps from standard input
  app configure --keep-work           replace personal choices and add work choices
  app add <app>                      choose an app to filter
  app remove <app>                    stop filtering it
  app pause <app> --minutes <n>        stop filtering an app for a while
  app resume <app>                    app is a package or @shared-user, optionally prefixed by profile serial
  ca export                           print the public CA for work-profile provisioning
  pause [--minutes <n>]               stop filtering every app, for a while or until resumed
  resume
  url-filtering on|off                filter URLs for all filtered apps where available
  cosmetic on|off                     hide on decrypted pages what the lists name, changing the pages
  allow add|remove <host>             trust a host: never decrypted, never blocked
  allow list [--json]
  lists [--json]                      the filter lists
  list configure                    read subscription choices from standard input
  list add|remove|enable|disable <url>
  update [--json]                     download the filter lists
  update [--json] --list <url>         download one enabled filter list
  rules get                           print your own rules
  rules set                           replace them with standard input
  rules append [--unique]             append input, or one rule only if new
  rules check [--json]                the rules on standard input that are not applied
  check [--json] <url> [<page> [<kind>]]   what the lists say of a request
  pinned [--json [<app>]]             the hosts apps would not let us read
  pinned forget <app> [<host>]        have them read again
  watch [--json] [--app <app>]        what happens, as it happens, until standard input ends
  reload                              have the daemon read everything again
  handoff begin|rollback|finish <uuid>  manage a checkpoint for a switch to root
  run | trust remove | unhook | module-stop   run by the module
";

/// The daemon, as the engine's commands need it.
struct Daemon<'a> {
    paths: &'a Paths,
    alias: Option<(&'a str, &'a str)>,
}

impl abd_engine::cli::Host for Daemon<'_> {
    fn running(&self) -> Option<i32> {
        lifecycle::daemon(self.paths)
    }
    fn reload(&self) {
        lifecycle::reload(self.paths);
    }
    /// The module's, from the `module.prop` that came with this program.
    fn version(&self) -> Option<String> {
        let program = std::env::current_exe().ok()?;
        protocol::version(&program)
    }
    /// When `module/service.sh` turned the module off, as it left it said.
    fn boot_guard(&self) -> Option<u64> {
        let said = abd_engine::files::read_text(&self.paths.boot_guard(), 64).ok()?;
        said.trim().parse().ok()
    }
    fn undecryptable(&self) -> Option<&'static str> {
        let ready = abd_engine::state::read(&self.paths.state())
            .is_some_and(|state| state.exclusions_ready);
        (!ready).then_some("exclusions_pending")
    }
    fn trusted_profiles(&self) -> BTreeSet<u32> {
        self.running()
            .and_then(|pid| {
                abd_engine::state::read(&self.paths.state()).filter(|state| state.pid == pid as u32)
            })
            .map(|state| state.profile_tls)
            .unwrap_or_default()
    }
    fn personal_app_identities(&self, exclusions: &BTreeSet<&str>) -> Vec<String> {
        android::personal_app_identities(exclusions)
            .unwrap_or_default()
            .into_values()
            .collect()
    }
    fn report_app_eligibility(&self, listed: &mut [protocol::App]) {
        android::report_app_eligibility(listed);
    }
    fn reports_filtering_apps(&self) -> bool {
        true
    }
    fn validate_selection(&self, _selected: &[&str], added: &[&str]) -> Result<()> {
        if let Some((member, choice)) = self.alias {
            ensure!(
                android::canonical_choices(&[member])? == [choice],
                "Android app identity changed during selection"
            );
        }
        android::validate_selection(added)
    }
}

pub fn run(paths: &Paths, args: &[&str]) -> Result<()> {
    prune_if_listing(paths, args);
    let Canonical { args, alias } = canonicalize_args(paths, args)?;
    let args: Vec<_> = args.iter().map(|arg| arg.as_ref()).collect();
    let alias = alias.map(|(member, index)| (member, args[index]));
    match args.as_slice() {
        ["handoff", rest @ ..] => handoff::run(paths, rest),
        ["app", "profiles", "--json"] => android::print_profiles(std::io::stdout().lock()),
        ["app", "eligibility", "--json"] => {
            android::print_personal_eligibility(std::io::stdout().lock())
        }
        ["app", "configure"] => configure_apps(paths, abd_engine::apps::Configure::Replace),
        ["app", "configure", "--keep-work"] => {
            configure_apps(paths, abd_engine::apps::Configure::KeepWork)
        }
        ["ca", "export"] => ca_export(paths),
        ["update", rest @ ..] => update(paths, rest),
        ["watch", rest @ ..] => watch(paths, rest),
        _ => abd_engine::cli::run(
            paths,
            &Daemon { paths, alias },
            &args,
            &mut std::io::stdin().lock(),
            &mut std::io::stdout().lock(),
        ),
    }
}

fn prune_if_listing(paths: &Paths, args: &[&str]) {
    if matches!(
        args,
        ["app", "list"]
            | ["app", "list", "--json"]
            | ["status"]
            | ["status", "--json"]
            | ["overview", "--json", ..]
    ) && android::prune_orphans(paths).unwrap_or(false)
    {
        lifecycle::reload(paths);
    }
}

struct Canonical<'a> {
    args: Vec<Cow<'a, str>>,
    alias: Option<(&'a str, usize)>,
}

fn canonicalize_args<'a>(paths: &Paths, args: &[&'a str]) -> Result<Canonical<'a>> {
    // Member commands and explicit group commands address the same saved
    // choice. Keep exact removal available for an old bare selection.
    let target = match args {
        ["app", "add", name] => Some((2, android::canonical_choice(name)?)),
        ["app", "remove", name]
            if android::selection(&paths.apps())
                .is_ok_and(|text| abd_engine::apps::named(&text).contains(name)) =>
        {
            None
        }
        ["app", "remove" | "resume" | "exclude" | "include", name]
        | ["app", "pause", name, "--minutes", _]
        | ["pinned", "--json", name]
        | ["pinned", "forget", name]
        | ["pinned", "forget", name, _]
        | ["watch", "--app", name] => Some((2, android::canonical_existing(name)?)),
        ["watch", "--json", "--app", name] => Some((3, android::canonical_existing(name)?)),
        _ => None,
    };
    let mut arguments: Vec<_> = args.iter().copied().map(Cow::Borrowed).collect();
    let mut alias = None;
    if let Some((index, token)) = target {
        if let ["app", "add", member] = args
            && *member != token
        {
            alias = Some((*member, index));
        }
        arguments[index] = Cow::Owned(token);
    }
    Ok(Canonical {
        args: arguments,
        alias,
    })
}

fn configure_apps(paths: &Paths, scope: abd_engine::apps::Configure) -> Result<()> {
    let selection =
        abd_engine::files::read_limited(std::io::stdin().lock(), abd_engine::apps::MAX_FILE)?;
    let requested: Vec<_> = selection.lines().collect();
    let selection = android::canonical_choices(&requested)?;
    abd_engine::apps::configure(&paths.apps(), &selection.join("\n"), scope, |_, added| {
        if requested
            .iter()
            .copied()
            .ne(selection.iter().map(String::as_str))
        {
            ensure!(
                android::canonical_choices(&requested)? == selection,
                "Android app identities changed during selection"
            );
        }
        android::validate_selection(added)
    })?;
    lifecycle::reload(paths);
    Ok(())
}

fn ca_export(paths: &Paths) -> Result<()> {
    ca::Authority::open(&paths.ca())?;
    let public = abd_engine::files::read_bytes(&paths.ca().join("user.pem"), 64 << 10)?;
    std::io::stdout().lock().write_all(&public)?;
    Ok(())
}

fn update(paths: &Paths, args: &[&str]) -> Result<()> {
    let options = abd_engine::cli::parse_update_args(args)?;
    ca::Authority::open(&paths.ca())?;
    let roots = Arc::new(system_roots::read(paths)?);
    runtime::build()?.block_on(abd_engine::cli::update::run(paths, roots, options, || {
        lifecycle::reload(paths);
    }))
}

fn watch(paths: &Paths, args: &[&str]) -> Result<()> {
    let app = abd_engine::cli::parse_watch_args(args, |name| {
        abd_engine::apps::Identity::parse(name).is_some()
    })?;
    live::watch(&paths.watch(), app, lifecycle::daemon(paths).is_some())
}

#[cfg(test)]
mod tests {
    use abd_engine::cli::Host as _;

    use super::*;

    #[test]
    fn says_since_when_the_module_keeps_itself_off_as_the_script_left_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        assert_eq!(
            Daemon {
                paths: &paths,
                alias: None
            }
            .boot_guard(),
            None
        );
        // As `date +%s` writes it.
        std::fs::write(paths.boot_guard(), "1763538000\n").unwrap();
        assert_eq!(
            Daemon {
                paths: &paths,
                alias: None
            }
            .boot_guard(),
            Some(1_763_538_000)
        );
        std::fs::write(paths.boot_guard(), "not a time\n").unwrap();
        assert_eq!(
            Daemon {
                paths: &paths,
                alias: None
            }
            .boot_guard(),
            None
        );
    }
}
