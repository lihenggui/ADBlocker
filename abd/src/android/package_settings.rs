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
use std::path::Path;

use abd_engine::files;
use anyhow::{Context as _, Result};

use super::atomic_file::{CommittedCopy, open_with_backup, uses_resilient_atomic_file};
use super::package_inventory::{SharedUsers, shared_users};

pub(crate) const PRIMARY: &str = "packages.xml";
pub(crate) const BACKUP: &str = "packages-backup.xml";
const RESERVE: &str = "packages.xml.reservecopy";
const MAX_PACKAGE_SETTINGS: usize = 16 << 20;

pub(super) fn read_shared_users() -> Result<SharedUsers> {
    read_from(Path::new("/data/system"), uses_resilient_atomic_file())
        .context("Android shared-user registry unavailable")
}

fn read_from(dir: &Path, has_reserve: bool) -> Option<SharedUsers> {
    let primary = dir.join(PRIMARY);
    let backup = dir.join(BACKUP);
    let reserve = dir.join(RESERVE);
    // Android 12–13 Settings and Android 14+ ResilientAtomicFile prefer an
    // openable backup. When Android opens one, it discards the other copies,
    // so a corrupt backup cannot authorize using them in its place.
    //
    // Try once more if neither open caught a usable copy during that window.
    for _ in 0..2 {
        match open_with_backup(&primary, &backup) {
            Ok(CommittedCopy::Backup(file)) => return parse(file),
            Ok(CommittedCopy::Primary(file)) => {
                if let Some(users) = parse(file) {
                    return Some(users);
                }
            }
            Err(_) => {}
        }
        // Android 14+ retries with its reserve after a failed primary read.
        if has_reserve && let Ok(file) = File::open(&reserve) {
            if let Ok(backup) = File::open(&backup) {
                return parse(backup);
            }
            if let Some(users) = parse(file) {
                return Some(users);
            }
        }
    }
    None
}

fn parse(file: File) -> Option<SharedUsers> {
    shared_users(&files::read_file_bytes(file, MAX_PACKAGE_SETTINGS).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android::package_inventory::package_owners;

    const NEW: &str = r#"<packages><package name="com.member" sharedUserId="10001"/><shared-user name="com.new" userId="10001"/></packages>"#;
    const OLD: &str = r#"<packages><package name="com.member" sharedUserId="10001"/><shared-user name="com.old" userId="10001"/></packages>"#;
    const PARTIAL: &str = "<packages><shared-user";

    #[test]
    fn selects_the_authoritative_copy_for_each_settings_state() {
        let owners = package_owners("com.member 10001");
        for (primary, backup, reserve, legacy, resilient) in [
            (NEW, "", "", Some("com.new"), Some("com.new")),
            (NEW, "", PARTIAL, Some("com.new"), Some("com.new")),
            (NEW, OLD, NEW, Some("com.old"), Some("com.old")),
            (PARTIAL, OLD, PARTIAL, Some("com.old"), Some("com.old")),
            ("", OLD, "", Some("com.old"), Some("com.old")),
            (PARTIAL, "", OLD, None, Some("com.old")),
            ("", "", OLD, None, Some("com.old")),
            (NEW, PARTIAL, OLD, None, None),
            (PARTIAL, "", PARTIAL, None, None),
            (PARTIAL, PARTIAL, PARTIAL, None, None),
            ("", "", "", None, None),
        ] {
            let dir = tempfile::tempdir().unwrap();
            for (name, body) in [(PRIMARY, primary), (BACKUP, backup), (RESERVE, reserve)] {
                if !body.is_empty() {
                    std::fs::write(dir.path().join(name), body).unwrap();
                }
            }
            for (has_reserve, expected) in [(false, legacy), (true, resilient)] {
                let users = read_from(dir.path(), has_reserve);
                assert_eq!(
                    users
                        .as_ref()
                        .map(|users| users.still_owns(10001, &owners, |_| true).unwrap()),
                    expected,
                    "primary={primary:?}, backup={backup:?}, reserve={reserve:?}, modern={has_reserve}",
                );
            }
        }
    }
}
