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

use std::io::Write;

use anyhow::{Context as _, Result};

use crate::paths::Paths;
use crate::{clock, protocol, stats};

/// A time, as the user reads it: in UTC, to the minute.
fn when(secs: u64) -> String {
    let Ok(t) = time::OffsetDateTime::from_unix_timestamp(secs.try_into().unwrap_or(i64::MAX))
    else {
        return secs.to_string();
    };
    format!(
        "{}-{:02}-{:02} {:02}:{:02} UTC",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute()
    )
}

pub(super) fn status_text(s: &protocol::Status, out: &mut dyn Write) -> Result<()> {
    if let Some(since) = s.boot_guard {
        writeln!(
            out,
            "turned off: two boots in a row did not finish ({})",
            when(since)
        )?;
    }
    match s.pid {
        Some(pid) => writeln!(out, "daemon: running (pid {pid})")?,
        None => writeln!(out, "daemon: not running")?,
    }
    writeln!(
        out,
        "version: {}",
        s.version.as_deref().unwrap_or("unknown")
    )?;
    writeln!(out, "apps: {}", s.apps)?;
    if let Some(active) = s.filtering_apps {
        writeln!(out, "filtering apps: {active}")?;
    }
    match (s.paused, s.resume_at) {
        (false, _) => writeln!(out, "paused: no")?,
        (true, Some(at)) => writeln!(out, "paused: until {}", when(at))?,
        (true, None) => writeln!(out, "paused: until resumed")?,
    }
    writeln!(
        out,
        "URL filtering: {}",
        if s.url_filtering { "yes" } else { "no" }
    )?;
    writeln!(out, "hiding: {}", if s.cosmetic { "yes" } else { "no" })?;
    if let (Some(rules), Some(hosts)) = (s.rules, s.host_rules) {
        writeln!(out, "rules: {rules}, {hosts} of them whole-host")?;
    }
    match s.lists_updated {
        Some(at) => writeln!(out, "lists updated: {}", when(at))?,
        None => writeln!(out, "lists updated: never")?,
    }
    Ok(())
}

/// The totals, a counter a line as `<name> <n>`, which scripts read.
pub(super) fn stats_text(paths: &Paths, out: &mut dyn Write) -> Result<()> {
    let s = stats::Snapshot::read(&paths.stats(), clock::now())
        .context("reading the saved statistics")?;
    writeln!(out, "since {}", when(s.since))?;
    // In the order they were always printed in.
    for i in [1, 3, 4, 2, 0, 5, 6] {
        writeln!(out, "{} {}", stats::NAMES[i], s.totals[i])?;
    }
    Ok(())
}
