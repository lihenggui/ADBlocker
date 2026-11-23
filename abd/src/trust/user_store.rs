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

//! The user's store as Android keeps it, for a copy to be mounted over.

use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;

use anyhow::{Context as _, Result, ensure};

/// AID_SYSTEM, which owns the user's store.
const SYSTEM_UID: u32 = 1000;

/// Makes sure the user's store is there for a mount, as Android has it.
///
/// It is made when the user first adds a certificate. A mount needs it to
/// be there: made empty, as it is after the user removed the last.
pub(super) fn ensure(store: &Path) -> Result<()> {
    ensure_with(store, restorecon)
}

fn ensure_with(store: &Path, relabel: fn(&Path)) -> Result<()> {
    if absent(store)? {
        match std::fs::create_dir(store) {
            Ok(()) => return made(store, relabel),
            // Android made it meanwhile, for a certificate the user added.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).with_context(|| format!("making {}", store.display())),
        }
    }
    // One that is there was repaired before the daemon started, and the
    // mount needs only a directory.
    ensure!(
        std::fs::symlink_metadata(store)?.file_type().is_dir(),
        "{} is not a directory",
        store.display()
    );
    Ok(())
}

/// Finishes a store just made.
fn made(store: &Path, relabel: fn(&Path)) -> Result<()> {
    // At once: an app may list it from now on.
    if let Err(error) = repair(store) {
        // Left, it would not be made again, and Android could not add the
        // user's certificates to a store it does not own.
        if let Err(removing) = std::fs::remove_dir(store) {
            eprintln!("trust: removing {}: {removing}", store.display());
        }
        return Err(error);
    }
    relabel(store);
    Ok(())
}

/// Gives a store we made the owner and the read and search access Android
/// gives its own, and leaves Android's as they are.
///
/// Every app reads it, filtered or not, and Android's own trust manager
/// throws where it is a directory it cannot list. Made under our umask,
/// as 0.1.0 made it, it is 0700, owned by system or, where that failed,
/// by root, which Android could not write. Android makes neither.
///
/// Through the directory itself, never a link put in its place: root
/// would change whatever it leads to.
pub(super) fn repair(store: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    if absent(store)? {
        return Ok(());
    }
    let dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(store)
        .with_context(|| format!("opening {}", store.display()))?;
    let metadata = dir.metadata()?;
    let mode = metadata.permissions().mode();
    if !ours(metadata.uid(), mode) {
        return Ok(());
    }
    if metadata.uid() == 0 {
        std::os::unix::fs::fchown(&dir, Some(SYSTEM_UID), Some(SYSTEM_UID))
            .with_context(|| format!("giving {} to system", store.display()))?;
    }
    dir.set_permissions(std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("opening {} to every app", store.display()))?;
    Ok(())
}

/// What our umask leaves, before or after the owner was given.
fn ours(uid: u32, mode: u32) -> bool {
    mode & 0o777 == 0o700 && (uid == 0 || uid == SYSTEM_UID)
}

fn absent(store: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(store) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error).with_context(|| format!("reading {}", store.display())),
    }
}

/// Labels a directory we made as the system labels it there, for the copy
/// to take its label. Unlabelled, it keeps what it inherited, which is
/// said and gone on with.
fn restorecon(path: &Path) {
    let labelled = crate::rules::device_command("restorecon")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !labelled
        .as_ref()
        .is_ok_and(std::process::ExitStatus::success)
    {
        let failure = labelled.map_or_else(|e| e.to_string(), |status| status.to_string());
        eprintln!("trust: restorecon {}: {failure}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_store_our_umask_left_is_taken_for_ours() {
        assert!(ours(0, 0o40700));
        assert!(ours(SYSTEM_UID, 0o700));
        // Android's, or another's.
        assert!(!ours(SYSTEM_UID, 0o755));
        assert!(!ours(SYSTEM_UID, 0o750));
        assert!(!ours(0, 0o755));
        assert!(!ours(10_123, 0o700));
    }

    #[test]
    fn a_user_store_only_root_could_list_is_made_readable_by_every_app() {
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir().unwrap();
        // Only the owners our umask could have left a store with.
        if !ours(std::fs::metadata(dir.path()).unwrap().uid(), 0o700) {
            return;
        }
        let store = dir.path().join("cacerts-added");
        let mode = || std::fs::metadata(&store).unwrap().permissions().mode() & 0o777;
        std::fs::create_dir(&store).unwrap();
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
        repair(&store).unwrap();
        assert_eq!(mode(), 0o755);
        // A mode we never made is left as it is.
        for android in [0o775, 0o750] {
            std::fs::set_permissions(&store, std::fs::Permissions::from_mode(android)).unwrap();
            repair(&store).unwrap();
            assert_eq!(mode(), android);
        }
        std::fs::remove_dir(&store).unwrap();
        std::fs::write(&store, "").unwrap();
        assert!(repair(&store).is_err());
    }

    #[test]
    fn a_link_in_place_of_the_user_store_changes_nothing_it_leads_to() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = dir.path().join("cacerts-added");
        std::os::unix::fs::symlink(&elsewhere, &store).unwrap();
        assert!(repair(&store).is_err());
        let mode = std::fs::metadata(&elsewhere).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn there_is_nothing_to_repair_where_there_is_no_user_store() {
        let dir = tempfile::tempdir().unwrap();
        repair(&dir.path().join("cacerts-added")).unwrap();
        assert!(!dir.path().join("cacerts-added").exists());
    }

    /// Relabelling is the device's: nothing to do on the host.
    fn unlabelled(_: &Path) {}

    #[test]
    fn a_user_store_is_made_or_repaired_for_every_app_to_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("cacerts-added");
        let mode = || std::fs::metadata(&store).unwrap().permissions().mode() & 0o777;
        // Made by whoever runs the tests, it is given to system only when
        // that is root, who can.
        ensure_with(&store, unlabelled).unwrap();
        assert_eq!(mode() & 0o055, 0o055);
        // One that is there is left to `repair`, at the daemon's start.
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
        ensure_with(&store, unlabelled).unwrap();
        assert_eq!(mode(), 0o700);
        // Only a directory can be mounted over.
        std::fs::remove_dir(&store).unwrap();
        std::fs::write(&store, "").unwrap();
        assert!(ensure_with(&store, unlabelled).is_err());
        // Whatever cannot be read is not taken for no store at all.
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let inside = blocked.join("cacerts-added");
        if std::fs::symlink_metadata(&inside)
            .is_err_and(|e| e.kind() != std::io::ErrorKind::NotFound)
        {
            let error = ensure_with(&inside, unlabelled).unwrap_err();
            assert!(error.to_string().starts_with("reading"));
            assert!(repair(&inside).is_err());
        }
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}
