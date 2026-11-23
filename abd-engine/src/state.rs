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

//! What the running daemon says of itself, for `abd status`: written when
//! it starts and each time it applies what it was told, never on a timer.
//! What is said is the daemon's only while that daemon runs, which the
//! command finds out for itself.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub pid: u32,
    /// Seconds since the epoch.
    pub started: u64,
    pub rules: usize,
    pub host_rules: usize,
    pub exclusions: usize,
    #[serde(default)]
    pub exclusions_ready: bool,
    /// Profiles whose externally provisioned TLS trust is ready to use.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub profile_tls: BTreeSet<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filtering_apps: Option<usize>,
    /// Whether IPv6 connections are filtered: not while they lead nowhere.
    pub ipv6: bool,
}

static STARTED: OnceLock<u64> = OnceLock::new();

/// When this daemon started: the first time this is asked.
pub fn started() -> u64 {
    *STARTED.get_or_init(crate::clock::now)
}

/// A failed publication invalidates the previous snapshot even for the
/// same PID. The next state change can publish it again.
pub fn write(file: &Path, state: &State) -> anyhow::Result<()> {
    let publish = || -> anyhow::Result<()> {
        let text = serde_json::to_vec(state)?;
        crate::files::replace(file, &text)?;
        Ok(())
    };
    if let Err(error) = publish() {
        if let Err(cleanup) = crate::files::remove_if_present(file) {
            return Err(error.context(format!(
                "could not invalidate the previous snapshot: {cleanup}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

pub fn read(file: &Path) -> Option<State> {
    let text = crate::files::read_text(file, 4096).ok()?;
    serde_json::from_str(&text).ok()
}
