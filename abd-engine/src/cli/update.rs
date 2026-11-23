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

//! Render list update progress for the daemon and the VPN command stream.

use std::io::{self, Write as _};
use std::sync::Arc;

use anyhow::{Context as _, Result};

use crate::paths::Paths;
use crate::protocol_types::{ListState, Update};
use crate::update::{self, ListKind};

pub struct Options {
    pub(super) json: bool,
    pub(super) list: Option<String>,
}

/// Fetches the filter lists and the lists of hosts never decrypted, each
/// from the addresses in its sources file. A list that cannot be fetched
/// keeps its previous copy. Calls `reload` if any lists changed.
///
/// With `json`, what becomes of each filter list is printed as it happens,
/// an event a line (`Update`); the lists of hosts never
/// decrypted are told of on standard error, as text. If progress cannot be
/// delivered, the update still finishes and reloads before reporting the error.
pub async fn run(
    paths: &Paths,
    roots: Arc<rustls::RootCertStore>,
    options: Options,
    reload: impl FnOnce(),
) -> Result<()> {
    run_reporting(paths, roots, options, reload, |said| {
        let mut out = io::stdout().lock();
        writeln!(out, "{said}")?;
        out.flush()
    })
    .await
}

async fn run_reporting(
    paths: &Paths,
    roots: Arc<rustls::RootCertStore>,
    options: Options,
    reload: impl FnOnce(),
    mut print: impl FnMut(&str) -> io::Result<()>,
) -> Result<()> {
    // Output is independent of list publication. Keep the first error and
    // leave the rest of the update free of writes to the failed stream.
    let mut delivery = Ok(());
    let mut line = |said: &str| {
        if delivery.is_ok() {
            delivery = print(said);
        }
    };
    if run_to(paths, roots, options, &mut line).await? > 0 {
        reload();
    }
    delivery.context("writing update progress")
}

/// As [`run`], saying each line to `line` for a host with no standard output.
/// Returns how many lists changed.
pub async fn run_to(
    paths: &Paths,
    roots: Arc<rustls::RootCertStore>,
    options: Options,
    line: &mut dyn FnMut(&str),
) -> Result<usize> {
    let Options { json, list } = options;
    let mut report = |kind, event| {
        if json && matches!(kind, ListKind::Filter) {
            if let Ok(said) = serde_json::to_string(&event) {
                line(&said);
            }
        } else if let Some(said) = said(&event) {
            if json {
                eprintln!("{said}");
            } else {
                line(&said);
            }
        }
    };
    let (filters, exclusions) = update::refresh(paths, roots, list.as_deref(), &mut report).await?;
    if json {
        let ready = crate::exclusions::Exclusions::load(&paths.exclusions()).ready();
        report(ListKind::Filter, filters.done(ready, exclusions.failed));
    }
    Ok(filters.changed() + exclusions.changed())
}

/// An event as `abd update` without `--json` says it.
fn said(event: &Update) -> Option<String> {
    match event {
        Update::List {
            url,
            state,
            rules,
            error,
        } => match state {
            ListState::Fetching => None,
            ListState::Updated => Some(format!("fetched {url} ({} rules)", rules.unwrap_or(0))),
            ListState::Unchanged => Some(format!("unchanged {url}")),
            ListState::Failed => Some(format!("skipped {url}: {}", error.as_deref().unwrap_or(""))),
        },
        Update::Start { .. } | Update::Done { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn losing_the_progress_stream_fails_the_command_after_changes_are_reloaded() {
        for (fail_at, kind) in [
            ("start", io::ErrorKind::BrokenPipe),
            ("done", io::ErrorKind::WriteZero),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let paths = Paths::new(dir.path());
            std::fs::write(paths.sources(), "").unwrap();
            std::fs::write(paths.exclusion_sources(), "").unwrap();
            std::fs::create_dir_all(paths.lists()).unwrap();
            let stale = paths
                .lists()
                .join(update::file_name("https://first.example/list.txt"));
            std::fs::write(&stale, "||old.example^\n").unwrap();
            let mut written = Vec::new();
            let mut reloaded = false;

            let error = run_reporting(
                &paths,
                Arc::new(rustls::RootCertStore::empty()),
                Options {
                    json: true,
                    list: None,
                },
                || {
                    assert!(!stale.exists());
                    reloaded = true;
                },
                |line| {
                    let event: serde_json::Value = serde_json::from_str(line).unwrap();
                    let event = event["event"].as_str().unwrap();
                    written.push(event.to_owned());
                    if event == fail_at {
                        Err(kind.into())
                    } else {
                        Ok(())
                    }
                },
            )
            .await
            .unwrap_err();

            assert!(reloaded);
            assert_eq!(error.downcast_ref::<io::Error>().unwrap().kind(), kind);
            assert_eq!(written.first().map(String::as_str), Some("start"));
            assert_eq!(written.last().map(String::as_str), Some(fail_at));
            assert_eq!(written.len(), if fail_at == "start" { 1 } else { 2 });
        }
    }
}
