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

use std::fs::File;
use std::io;
use std::path::Path;

pub(super) enum CommittedCopy {
    Backup(File),
    Primary(File),
}

pub(super) fn open_with_backup(primary: &Path, backup: &Path) -> io::Result<CommittedCopy> {
    // Pin the primary before checking the backup. If the primary was
    // opened mid-write and the backup is gone, Android has committed it.
    let primary = File::open(primary);
    match File::open(backup) {
        Ok(file) => Ok(CommittedCopy::Backup(file)),
        Err(_) => primary.map(CommittedCopy::Primary),
    }
}

pub(super) fn uses_resilient_atomic_file() -> bool {
    crate::properties::get(c"ro.build.version.sdk")
        .and_then(|value| value.parse::<u32>().ok())
        .is_some_and(|sdk| sdk >= 34)
}
