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

//! Where everything is kept on the device: what the engine keeps
//! (`abd_engine::paths`), and beside it what only the module has.

use std::path::PathBuf;

pub use abd_engine::paths::Paths;

/// The module's data, readable by root only.
pub const DATA: &str = "/data/adb/adblocker";

/// The module's own files in the directory.
pub trait Module {
    fn supervisor_pid(&self) -> PathBuf;
    fn pid(&self) -> PathBuf;
    /// The copy of the system store with our root in it.
    fn store(&self) -> PathBuf;
    /// The copy of the user's store with our second root in it.
    fn user_store(&self) -> PathBuf;
    /// When the module turned itself off because two boots in a row did
    /// not finish,
    /// in seconds since the epoch: written by `module/service.sh`, and
    /// removed by it the next time it starts the module.
    fn boot_guard(&self) -> PathBuf;
}

impl Module for Paths {
    fn supervisor_pid(&self) -> PathBuf {
        self.dir().join("supervisor.pid")
    }
    fn pid(&self) -> PathBuf {
        self.dir().join("abd.pid")
    }
    fn store(&self) -> PathBuf {
        self.dir().join("store")
    }
    fn user_store(&self) -> PathBuf {
        self.dir().join("user-store")
    }
    fn boot_guard(&self) -> PathBuf {
        self.dir().join("boot-guard")
    }
}
