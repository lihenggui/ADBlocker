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

//! Where the engine keeps what it needs, in a directory it is given.

use std::path::{Path, PathBuf};

pub struct Paths(PathBuf);

impl Paths {
    pub fn new(dir: &Path) -> Self {
        Self(dir.to_owned())
    }
    /// The certificate authority.
    pub fn ca(&self) -> PathBuf {
        self.0.join("ca")
    }
    /// The filter lists, one `*.txt` each.
    pub fn lists(&self) -> PathBuf {
        self.0.join("lists")
    }
    /// The user's own rules, one `*.txt` each. Nothing here is downloaded
    /// or removed by an update.
    pub fn rules(&self) -> PathBuf {
        self.0.join("rules")
    }
    /// The lists as the engine holds them, kept to be read faster.
    pub fn cache(&self) -> PathBuf {
        self.0.join("cache")
    }
    /// The addresses the lists are downloaded from, one a line.
    pub fn sources(&self) -> PathBuf {
        self.0.join("sources")
    }
    /// The addresses of the lists of hosts never decrypted, one a line.
    pub fn exclusion_sources(&self) -> PathBuf {
        self.0.join("exclusion-sources")
    }
    /// Those lists, one `*.txt` each.
    pub fn exclusions(&self) -> PathBuf {
        self.0.join("exclusions")
    }
    /// The hosts apps would not talk to through us.
    pub fn pinned(&self) -> PathBuf {
        self.0.join("pinned")
    }
    pub fn stats(&self) -> PathBuf {
        self.0.join("stats")
    }
    /// The packages whose traffic is filtered, one a line, each followed by
    /// `hosts` if only its hosts are judged.
    pub fn apps(&self) -> PathBuf {
        self.0.join("apps")
    }
    /// The user's own rules as `abd rules` reads and writes them.
    pub fn user_rules(&self) -> PathBuf {
        self.rules().join("user.txt")
    }
    /// What is known of each filter list: its title, its rules, when it
    /// was last updated. Not in `lists`, whose being empty is what tells
    /// the module that no list was ever fetched.
    pub fn list_info(&self) -> PathBuf {
        self.0.join("list-info")
    }
    /// Until when filtering is paused, for every app or for some.
    pub fn pauses(&self) -> PathBuf {
        self.0.join("pauses")
    }
    /// The hosts the user trusts: never decrypted, never blocked.
    pub fn allow(&self) -> PathBuf {
        self.0.join("allow")
    }
    /// What the user agreed to have done, for every app at once.
    pub fn settings(&self) -> PathBuf {
        self.0.join("settings")
    }
    /// What the running daemon says of itself, for `abd status`.
    pub fn state(&self) -> PathBuf {
        self.0.join("state")
    }
    /// Pinned hosts to forget, for the running daemon to read.
    pub fn forget(&self) -> PathBuf {
        self.0.join("forget")
    }
    /// Where the daemon tells `abd watch` what happens as it happens.
    pub fn watch(&self) -> PathBuf {
        self.0.join("watch.sock")
    }
    /// The directory itself, for what else is kept in it by whoever runs
    /// the engine.
    pub fn dir(&self) -> &Path {
        &self.0
    }
}
