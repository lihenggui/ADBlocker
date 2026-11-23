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

//! Forgetting hosts taken for pinned, so that they are decrypted again.
//!
//! The running daemon holds the table and writes it over the file: a host
//! removed from the file alone would be back with its next write. So a
//! command asks it, in a file of requests it reads when told to reload,
//! and at its start, should it start meanwhile. With no daemon running,
//! the command changes the table's file itself as well; the request it
//! leaves is done again by the next daemon, to no effect.

use std::io;
use std::path::Path;

use anyhow::{Context as _, Result, ensure};

use crate::files;
use crate::policy::{self, Policy};

/// Requests are a line each, of a package and maybe a host; this is
/// thousands of them, far more than are made between two reloads.
const MAX_REQUESTS: usize = 64 << 10;

/// One request: a package, and a host of it or all of them.
pub(crate) fn parse(line: &str) -> Option<(&str, Option<&str>)> {
    let mut f = line.split_ascii_whitespace();
    let package = f
        .next()
        .filter(|p| crate::apps::Identity::parse(p).is_some())?;
    let host = match f.next() {
        Some(host) if crate::host::is_host(host) => Some(host),
        Some(_) => return None,
        None => None,
    };
    f.next().is_none().then_some((package, host))
}

/// Append a removal while holding the data lock.
pub(crate) fn enqueue(file: &Path, line: &str, _lock: &files::Lock) -> Result<()> {
    let mut pending = files::read_text(file, MAX_REQUESTS)?;
    if !pending.is_empty() && !pending.ends_with('\n') {
        pending.push('\n');
    }
    ensure!(
        pending.len() + line.len() <= MAX_REQUESTS,
        "too many requests to forget waiting for the daemon"
    );
    pending.push_str(line);
    files::replace_durable(file, pending.as_bytes())?;
    Ok(())
}

/// Apply the removal to the saved table when no engine owns it.
pub(crate) fn forget_on_disk(
    file: &Path,
    package: &str,
    host: Option<&str>,
    _lock: &files::Lock,
) -> Result<()> {
    let mut pinned = policy::try_read_pinned(file, crate::clock::now())
        .context("pin removal queued: reading pinned hosts")?;
    if policy::forget(&mut pinned, package, host) {
        policy::write_pinned_durable(file, &pinned)
            .context("pin removal queued: saving pinned hosts")?;
    }
    Ok(())
}

pub(crate) fn pending(file: &Path) -> io::Result<bool> {
    file.try_exists()
}

/// Removes requests only after the engine's updated table is on disk.
pub fn take_requests(policy: &Policy, file: &Path) -> io::Result<()> {
    if !file.try_exists()? {
        return Ok(());
    }
    let _lock = files::lock(file)?;
    let text = files::read_text(file, MAX_REQUESTS)?;
    let requests: Vec<_> = text.lines().filter_map(parse).collect();
    if !requests.is_empty() {
        policy.persist_forget(&requests)?;
    }
    files::remove_if_present(file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;

    #[test]
    fn the_daemon_does_what_was_asked_once_and_writes_its_table() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let policy = Policy::open(paths.pinned());
        policy.rejected("com.a", "x.example");
        policy.rejected("com.a", "y.example");
        std::fs::write(paths.forget(), "com.a x.example\nnot a request at all\n").unwrap();
        take_requests(&policy, &paths.forget()).unwrap();
        let pinned = policy::read_pinned(&paths.pinned(), crate::clock::now());
        assert_eq!(
            pinned.keys().collect::<Vec<_>>(),
            [&("com.a".to_owned(), "y.example".to_owned())]
        );
        assert!(!paths.forget().exists());
        // Nothing asked: nothing done.
        take_requests(&policy, &paths.forget()).unwrap();
    }
}
