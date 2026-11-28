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

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail, ensure};

use super::render::{stats_text, status_text};
use super::{Ctx, Host, app_identity, print_json, unknown};
use crate::malformed::Malformed;
use crate::paths::Paths;
use crate::{clock, diagnostics, files, pins, protocol, sources};

/// How long a command waits for the running engine to confirm the removal.
const DONE_TIME: Duration = Duration::from_secs(3);

pub(super) fn run(
    ctx: &Ctx<'_>,
    args: &[&str],
    _input: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<()> {
    let Ctx { paths, host, .. } = *ctx;
    let read_status = || status(ctx, clock::now(), sources::lists_updated(paths));
    match args {
        ["status"] => status_text(&read_status(), out),
        ["status", "--json"] => print_json(out, &read_status()),
        ["overview", "--json", boundaries @ ..] => {
            let boundaries = stats_boundaries(boundaries)?;
            let now = clock::now();
            let lists = protocol::lists(paths);
            let updated = lists.iter().filter_map(|list| list.updated).max();
            print_json(
                out,
                &protocol::Overview {
                    status: status(ctx, now, updated),
                    stats: protocol::stats(paths, now, boundaries)?,
                    lists,
                },
            )
        }
        ["stats"] => stats_text(paths, out),
        ["stats", "--json", boundaries @ ..] => print_json(
            out,
            &protocol::stats(paths, clock::now(), stats_boundaries(boundaries)?)?,
        ),
        ["check", rest @ ..] => check(paths, rest, out),

        ["pinned"] => {
            for p in protocol::pinned(paths, None, clock::now()) {
                writeln!(out, "{} {} {}", p.app, p.host, p.since)?;
            }
            Ok(())
        }
        ["pinned", "--json"] => print_json(out, &protocol::pinned(paths, None, clock::now())),
        ["pinned", "--json", name] => {
            let name = app_identity(name)?;
            print_json(out, &protocol::pinned(paths, Some(name), clock::now()))
        }
        ["pinned", "forget", name, which @ ..] if which.len() <= 1 => {
            forget(paths, host, app_identity(name)?, which.first().copied())
        }
        _ => unknown(args),
    }
}

fn forget(paths: &Paths, on: &dyn Host, package: &str, host: Option<&str>) -> Result<()> {
    let line = match host {
        Some(host) => format!("{package} {host}\n"),
        None => format!("{package}\n"),
    };
    if pins::parse(&line).is_none() {
        bail!(Malformed(format!(
            "not a package and host: {}",
            line.trim_end()
        )));
    }
    let requests = paths.forget();
    {
        let lock = files::lock(&requests)?;
        pins::enqueue(&requests, &line, &lock)?;
        if !on.manages_pins() {
            // A daemon starting concurrently may have read the old file.
            return pins::forget_on_disk(&paths.pinned(), package, host, &lock);
        }
    }
    on.reload_pins().context("pin removal queued")?;
    let started = Instant::now();
    loop {
        if !pins::pending(&requests)? {
            return Ok(());
        }
        ensure!(
            started.elapsed() < DONE_TIME,
            "pin removal queued: the engine has not confirmed persistence"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A recorded URL and its context, or a domain blocked before a URL was read.
fn check(paths: &Paths, args: &[&str], out: &mut dyn Write) -> Result<()> {
    let (json, args) = match args {
        ["--json", rest @ ..] => (true, rest),
        _ => (false, args),
    };
    let (app, args) = match args {
        ["--app", app, rest @ ..] if crate::apps::Identity::parse(app).is_some() => {
            (Some(*app), rest)
        }
        _ => (None, args),
    };
    let found = match args {
        ["--host", host] if !host.starts_with("--") => diagnostics::check_host(paths, app, host)?,
        [url, rest @ ..] if rest.len() <= 2 && !url.starts_with("--") => {
            let (page, kind) = request(url, rest);
            diagnostics::check(paths, app, url, page, kind)?
        }
        _ => return unknown(&[&["check"], args].concat()),
    };
    if json {
        return print_json(out, &found);
    }
    let said = if found.blocked { "blocked" } else { "allowed" };
    writeln!(out, "{said} {}", found.rule.unwrap_or_default())?;
    Ok(())
}

/// The page and kind of a request to `url`, as `check` is given them.
fn request<'a>(url: &'a str, rest: &[&'a str]) -> (&'a str, &'a str) {
    let kind = rest.get(1).copied().unwrap_or("other");
    let page = rest.first().copied().filter(|page| !page.is_empty());
    (
        page.unwrap_or(if kind == "document" { url } else { "" }),
        kind,
    )
}

fn status(ctx: &Ctx<'_>, now: u64, lists_updated: Option<u64>) -> protocol::Status {
    let Ctx { paths, host, .. } = *ctx;
    let mut status = protocol::status(
        paths,
        host.running(),
        host.version(),
        ctx.settings().unwrap_or(ctx.defaults),
        now,
        lists_updated,
    );
    if let Some(ipv6) = host.vpn_ipv6() {
        status.ipv6 = Some(ipv6);
    }
    status.boot_guard = host.boot_guard();
    if host.reports_filtering_apps() {
        status.filtering_apps.get_or_insert(0);
    }
    status
}

fn stats_boundaries(args: &[&str]) -> Result<[i64; protocol::STATS_DAYS + 1]> {
    let malformed = || Malformed("daily statistics need eight ordered Unix timestamps".to_owned());
    let mut boundaries = [0; protocol::STATS_DAYS + 1];
    if args.len() != boundaries.len() {
        return Err(malformed().into());
    }
    for (boundary, arg) in boundaries.iter_mut().zip(args) {
        *boundary = arg.parse().map_err(|_| malformed())?;
    }
    // A date skipped by a time zone has the same start as the next date.
    if !boundaries.is_sorted() {
        return Err(malformed().into());
    }
    Ok(boundaries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pins::take_requests;
    use crate::policy::{self, Policy};

    #[test]
    fn a_host_is_forgotten_in_the_file_when_no_daemon_runs() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let now = crate::clock::now();
        std::fs::write(
            paths.pinned(),
            format!("{now} com.a x.example\n{now} com.a y.example\n{now} com.b x.example\n"),
        )
        .unwrap();
        forget(&paths, &crate::cli::Nobody, "com.a", Some("x.example")).unwrap();
        let pinned = policy::read_pinned(&paths.pinned(), now);
        assert_eq!(pinned.len(), 2);
        forget(&paths, &crate::cli::Nobody, "com.a", None).unwrap();
        let pinned = policy::read_pinned(&paths.pinned(), now);
        assert_eq!(
            pinned.keys().collect::<Vec<_>>(),
            [&("com.b".to_owned(), "x.example".to_owned())]
        );
        assert!(
            forget(
                &paths,
                &crate::cli::Nobody,
                "com.a",
                Some("x.example\n1 com.a y")
            )
            .is_err()
        );
    }

    #[test]
    fn stopped_pin_removal_reports_failed_persistence_and_leaves_the_request_pending() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let now = crate::clock::now();
        let saved = format!("{now} com.a x.example\n{now} com.a y.example\n");
        std::fs::write(paths.pinned(), &saved).unwrap();
        let blocked = paths.pinned().with_extension("new");
        std::fs::create_dir(&blocked).unwrap();

        assert!(forget(&paths, &crate::cli::Nobody, "com.a", Some("x.example")).is_err());
        assert_eq!(std::fs::read_to_string(paths.pinned()).unwrap(), saved);
        assert_eq!(
            std::fs::read_to_string(paths.forget()).unwrap(),
            "com.a x.example\n"
        );

        std::fs::remove_dir(blocked).unwrap();
        let policy = Policy::open(paths.pinned());
        take_requests(&policy, &paths.forget()).unwrap();
        assert!(!paths.forget().exists());
        let pinned = policy::read_pinned(&paths.pinned(), now);
        assert_eq!(
            pinned.keys().collect::<Vec<_>>(),
            [&("com.a".into(), "y.example".into())]
        );
    }

    struct Running<'a> {
        policy: &'a Policy,
        paths: &'a Paths,
    }

    impl Host for Running<'_> {
        fn running(&self) -> Option<i32> {
            Some(1)
        }
        fn reload(&self) {
            let _ = take_requests(self.policy, &self.paths.forget());
        }
        fn version(&self) -> Option<String> {
            None
        }
        fn boot_guard(&self) -> Option<u64> {
            None
        }
    }

    #[test]
    fn running_pin_removal_is_acknowledged_only_after_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let now = crate::clock::now();
        let policy = Policy::open(paths.pinned());
        policy.rejected("com.a", "x.example");
        policy.rejected("com.a", "y.example");
        policy.write().unwrap();
        let blocked = paths.pinned().with_extension("new");
        std::fs::create_dir(&blocked).unwrap();

        let running = Running {
            policy: &policy,
            paths: &paths,
        };
        assert!(forget(&paths, &running, "com.a", Some("x.example")).is_err());
        assert!(paths.forget().exists());
        assert_eq!(policy::read_pinned(&paths.pinned(), now).len(), 2);

        std::fs::remove_dir(blocked).unwrap();
        policy.write().unwrap();
        assert_eq!(policy::read_pinned(&paths.pinned(), now).len(), 2);
        forget(&paths, &running, "com.a", Some("x.example")).unwrap();
        let pinned = policy::read_pinned(&paths.pinned(), now);
        assert_eq!(
            pinned.keys().collect::<Vec<_>>(),
            [&("com.a".into(), "y.example".into())]
        );
        assert!(!paths.forget().exists());
    }
}
