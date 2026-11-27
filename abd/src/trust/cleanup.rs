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

//! Detaches certificate store copies and removes them once no namespace uses them.

#![allow(unsafe_code, reason = "fork, setns and unmount")]

use std::collections::BTreeMap;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use super::{
    APEX_STORE, CA_LIMIT, Pinned, SYSTEM_STORE, USER_STORE, mountinfo_of, open_below, pin, present,
    store_name,
};
use crate::paths::{Module as _, Paths};

/// A copy can remain mounted after its public root or its PEM has been removed.
struct CopyTarget {
    path: CString,
    identity: (u64, u64),
    device: String,
}

fn copy_targets(paths: &Paths) -> Result<Vec<CopyTarget>> {
    let mut targets = Vec::new();
    for (copy, destinations) in [
        (paths.store(), &[APEX_STORE, SYSTEM_STORE][..]),
        (paths.user_store(), &[USER_STORE][..]),
    ] {
        let Some(metadata) = present(std::fs::symlink_metadata(&copy))? else {
            continue;
        };
        ensure!(
            metadata.is_dir(),
            "certificate store copy is not a directory"
        );
        let dev = metadata.dev();
        let device = format!("{}:{}", libc::major(dev), libc::minor(dev));
        for destination in destinations {
            targets.push(CopyTarget {
                path: CString::new(*destination)?,
                identity: (dev, metadata.ino()),
                device: device.clone(),
            });
        }
    }
    Ok(targets)
}

/// A mount on the backing device might be ours even when obscured by
/// another mount. We may detach it only after checking its directory identity.
fn candidates<'a>(
    mountinfo: &str,
    targets: &'a [CopyTarget],
) -> Result<Vec<(&'a CopyTarget, usize)>> {
    ensure!(
        !mountinfo.is_empty(),
        "mount namespace has no mount information"
    );
    let mut counts = vec![0; targets.len()];
    for line in mountinfo.lines() {
        let mut fields = line.split_ascii_whitespace();
        let device = fields.nth(2).context("missing mount device")?;
        let point = fields.nth(1).context("missing mount point")?;
        ensure!(
            fields.any(|field| field == "-"),
            "incomplete mount information"
        );
        for (target, count) in targets.iter().zip(&mut counts) {
            if device == target.device && point.as_bytes() == target.path.as_bytes() {
                *count += 1;
            }
        }
    }
    Ok(targets
        .iter()
        .zip(counts)
        .filter(|(_, count)| *count > 0)
        .collect())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Unmounted {
    Absent,
    Gone,
    StillMounted,
}

/// Takes the copies away from a process of the app's UID when the app is
/// no longer to be decrypted. Hold its proc directory across the UID and
/// namespace checks so a reused PID cannot redirect the cleanup.
pub fn unmount_from(paths: &Paths, pid: i32, uid: u32) -> Result<Unmounted> {
    let Some(Pinned { dir, ns }) = pin(pid, uid)? else {
        return Ok(Unmounted::Gone);
    };
    let targets = copy_targets(paths)?;
    inspect_namespace(&dir, &ns, &targets, true)
}

fn inspect_namespace(
    dir: &OwnedFd,
    ns: &OwnedFd,
    targets: &[CopyTarget],
    detach: bool,
) -> Result<Unmounted> {
    let Some(info) = mountinfo_of(dir, ns)? else {
        return Ok(Unmounted::Gone);
    };
    let remaining = candidates(&info, targets)?;
    if remaining.is_empty() {
        return Ok(Unmounted::Absent);
    }
    if !detach {
        return Ok(Unmounted::StillMounted);
    }
    detach_in(ns, &remaining)
}

fn detach_in(ns: &OwnedFd, targets: &[(&CopyTarget, usize)]) -> Result<Unmounted> {
    // SAFETY: this command is single-threaded. The child only calls system
    // calls on descriptors and strings prepared before the fork, then exits.
    unsafe {
        let child = libc::fork();
        if child < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if child == 0 {
            if libc::setns(ns.as_raw_fd(), libc::CLONE_NEWNS) != 0 {
                libc::_exit(2);
            }
            let mut status = 0;
            for (target, count) in targets {
                let mut actual = std::mem::MaybeUninit::<libc::stat>::uninit();
                if libc::stat(target.path.as_ptr(), actual.as_mut_ptr()) != 0 {
                    status = 2;
                    continue;
                }
                let actual = actual.assume_init();
                if (actual.st_dev, actual.st_ino) != target.identity {
                    status = 2;
                    continue;
                }
                // With stacked mounts, detaching the visible one does not
                // establish what is underneath.
                if libc::umount2(target.path.as_ptr(), libc::MNT_DETACH) != 0 || *count > 1 {
                    status = status.max(1);
                }
            }
            libc::_exit(status);
        }

        let mut status = 0;
        while libc::waitpid(child, &mut status, 0) != child {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e.into());
            }
        }
        ensure!(libc::WIFEXITED(status), "namespace cleanup was interrupted");
        match libc::WEXITSTATUS(status) {
            0 => Ok(Unmounted::Absent),
            1 => Ok(Unmounted::StillMounted),
            _ => bail!("could not verify namespace cleanup"),
        }
    }
}

/// The second scan must see a clean snapshot. A process may have inherited
/// its namespace's mounts just before the first scan detached them.
fn scan_namespaces(proc: &Path, targets: &[CopyTarget], detach: bool) -> Result<bool> {
    const MAX_NAMESPACES: usize = 16_384;
    let mut seen = BTreeMap::new();
    let mut inspect = |dir: &OwnedFd| -> Result<Option<bool>> {
        let Some(ns) = present(open_below(dir, c"ns/mnt", libc::O_RDONLY))? else {
            return Ok(None);
        };
        let ns = std::fs::File::from(ns);
        let meta = ns.metadata()?;
        let id = (meta.dev(), meta.ino());
        if let Some(&absent) = seen.get(&id) {
            return Ok(Some(absent));
        }
        ensure!(seen.len() < MAX_NAMESPACES, "too many mount namespaces");
        let absent = match inspect_namespace(dir, &ns.into(), targets, detach)? {
            Unmounted::Gone => return Ok(None),
            Unmounted::Absent => true,
            Unmounted::StillMounted => false,
        };
        seen.insert(id, absent);
        Ok(Some(absent))
    };

    let mut complete = true;
    for entry in std::fs::read_dir(proc)? {
        let entry = entry?;
        if !entry.file_name().as_bytes().iter().all(u8::is_ascii_digit) {
            continue;
        }
        let mut inspect_process = || -> Result<bool> {
            let Some(dir) = present(std::fs::File::open(entry.path()))? else {
                return Ok(true);
            };
            let dir = OwnedFd::from(dir);
            if let Some(absent) = inspect(&dir)? {
                return Ok(absent);
            }

            // An exited leader may still have live threads. Open their
            // proc entries through the held directory, never a reused PID.
            let Some(tasks) = present(open_below(&dir, c"task", libc::O_RDONLY))? else {
                return Ok(true);
            };
            let path = format!("/proc/self/fd/{}", tasks.as_raw_fd());
            let mut absent = true;
            for thread in std::fs::read_dir(path)? {
                let thread = CString::new(thread?.file_name().as_bytes())?;
                if let Some(dir) = present(open_below(&tasks, &thread, libc::O_RDONLY))? {
                    absent &= inspect(&dir)?.unwrap_or(true);
                }
            }
            Ok(absent)
        };
        complete &= inspect_process().unwrap_or(false);
    }
    Ok(complete)
}

fn remove_public_root(copy: &Path, pem: &Path) -> Result<()> {
    let Some(metadata) = present(std::fs::symlink_metadata(copy))? else {
        return Ok(());
    };
    ensure!(
        metadata.is_dir(),
        "certificate store copy is not a directory"
    );
    let name = store_name(&abd_engine::files::read_bytes(pem, CA_LIMIT)?)?;
    let root = copy.join(name);
    present(std::fs::remove_file(&root))?;
    ensure!(
        present(std::fs::symlink_metadata(root))?.is_none(),
        "certificate store root is still present"
    );
    Ok(())
}

/// Returns true only when the copies were verified unused and removed.
/// Otherwise keep the stores for live namespaces and remove their public roots.
///
/// Must be called in a process with a single thread.
pub fn remove(paths: &Paths, keep: bool) -> Result<bool> {
    let removed = remove_in(paths, keep, Path::new("/proc"))?;
    if removed {
        // The user's store, if it was made by us and never used.
        let _ = std::fs::remove_dir(USER_STORE);
    }
    Ok(removed)
}

fn remove_in(paths: &Paths, keep: bool, proc: &Path) -> Result<bool> {
    let cleared = (|| -> Result<bool> {
        let targets = copy_targets(paths)?;
        if !targets.is_empty() {
            let _ = scan_namespaces(proc, &targets, true);
            if keep || !scan_namespaces(proc, &targets, false)? {
                return Ok(false);
            }
        } else if keep {
            return Ok(false);
        }
        // The copies may be absent on a first or repeated uninstall.
        present(std::fs::remove_dir_all(paths.store()))?;
        present(std::fs::remove_dir_all(paths.user_store()))?;
        Ok(true)
    })();
    if matches!(cleared, Ok(true)) {
        return cleared;
    }

    // Try each root even if verification or removal of the other failed.
    eprintln!(
        "trust: certificate stores could not be verified removed; keeping any remaining copies"
    );
    let mut roots_removed = true;
    for (copy, pem) in [
        (paths.store(), "system.pem"),
        (paths.user_store(), "user.pem"),
    ] {
        if let Err(error) = remove_public_root(&copy, &paths.ca().join(pem)) {
            eprintln!("trust: could not remove the public root identified by {pem}: {error:#}");
            roots_removed = false;
        }
    }
    cleared?;
    ensure!(roots_removed, "public root removal could not be verified");
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use abd_engine::apps::PERSONAL_APP_UIDS;

    use super::*;

    #[test]
    fn removal_never_reports_success_when_a_store_cannot_be_inspected() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::write(paths.store(), "unexpected file").unwrap();

        assert!(remove(&paths, false).is_err());
        assert!(paths.store().is_file());
    }

    fn stores_for_removal(paths: &Paths) -> [PathBuf; 2] {
        abd_engine::ca::Authority::open(&paths.ca()).unwrap();
        [
            (paths.store(), "system.pem"),
            (paths.user_store(), "user.pem"),
        ]
        .map(|(copy, pem)| {
            std::fs::create_dir(&copy).unwrap();
            std::fs::write(copy.join("another.0"), "keep").unwrap();
            let pem = std::fs::read(paths.ca().join(pem)).unwrap();
            let root = copy.join(store_name(&pem).unwrap());
            std::fs::write(&root, pem).unwrap();
            root
        })
    }

    #[test]
    fn an_unreadable_proc_listing_keeps_both_copies_without_our_roots() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let roots = stores_for_removal(&paths);
        assert!(remove_in(&paths, false, &dir.path().join("missing-proc")).is_err());
        assert!(paths.store().join("another.0").is_file());
        assert!(paths.user_store().join("another.0").is_file());
        assert!(roots.iter().all(|root| !root.exists()));
    }

    #[test]
    fn forced_retention_reports_a_failed_unlink_and_still_removes_the_other_root() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let [system, user] = stores_for_removal(&paths);
        std::fs::remove_file(&system).unwrap();
        std::fs::create_dir(&system).unwrap();
        assert!(remove_in(&paths, true, dir.path()).is_err());
        assert!(system.is_dir());
        assert!(!user.exists());
        std::fs::remove_dir(&system).unwrap();
        assert!(!remove_in(&paths, true, dir.path()).unwrap());
    }

    #[test]
    fn unreadable_public_pem_keeps_unverified_stores_and_still_removes_the_other_root() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let [system, user] = stores_for_removal(&paths);
        std::fs::write(paths.ca().join("system.pem"), "invalid PEM").unwrap();
        assert!(remove_in(&paths, true, dir.path()).is_err());
        assert!(system.is_file());
        assert!(!user.exists());
    }

    #[test]
    fn only_a_verified_clean_scan_allows_recursive_store_removal() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        stores_for_removal(&paths);
        let proc = dir.path().join("proc");
        let process = proc.join("1234");
        std::fs::create_dir_all(process.join("ns")).unwrap();
        std::fs::write(process.join("ns/mnt"), "").unwrap();
        let targets = copy_targets(&paths).unwrap();
        let target = &targets[0];
        let info = process.join("mountinfo");
        std::fs::write(
            &info,
            format!(
                "10 1 {} /copy {APEX_STORE} ro - ext4 dev ro\n",
                target.device
            ),
        )
        .unwrap();
        assert!(!remove_in(&paths, false, &proc).unwrap());
        assert!(paths.store().join("another.0").is_file());
        // Same device elsewhere does not name one of our destinations.
        std::fs::write(
            &info,
            format!("10 1 {} / / ro - ext4 dev ro\n", target.device),
        )
        .unwrap();
        assert!(remove_in(&paths, false, &proc).unwrap());
        assert!(!paths.store().exists() && !paths.user_store().exists());
    }

    #[test]
    fn a_failed_namespace_entry_cannot_confirm_that_a_mount_was_removed() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir(paths.store()).unwrap();
        let targets = copy_targets(&paths).unwrap();
        let process = dir.path().join("process");
        std::fs::create_dir_all(process.join("ns")).unwrap();
        // A regular file lets inspection pin the namespace entry but forces setns to fail.
        std::fs::write(process.join("ns/mnt"), "").unwrap();
        std::fs::write(
            process.join("mountinfo"),
            format!(
                "10 1 {} /copy {APEX_STORE} ro - ext4 dev ro\n",
                targets[0].device
            ),
        )
        .unwrap();
        let proc_dir = OwnedFd::from(std::fs::File::open(process).unwrap());
        let namespace = open_below(&proc_dir, c"ns/mnt", libc::O_RDONLY).unwrap();

        assert_eq!(
            inspect_namespace(&proc_dir, &namespace, &targets, false).unwrap(),
            Unmounted::StillMounted,
        );
        let failure = inspect_namespace(&proc_dir, &namespace, &targets, true).unwrap_err();
        assert_eq!(failure.to_string(), "could not verify namespace cleanup");
    }

    #[test]
    fn an_unreadable_live_namespace_is_unverifiable_but_a_disappeared_pid_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let proc = dir.path().join("proc");
        let process = proc.join("1234");
        std::fs::create_dir_all(&process).unwrap();
        std::fs::write(process.join("ns"), "").unwrap();
        assert!(!scan_namespaces(&proc, &[], false).unwrap());
        std::fs::remove_file(process.join("ns")).unwrap();
        std::fs::create_dir(process.join("ns")).unwrap();
        std::fs::write(process.join("ns/mnt"), "").unwrap();
        assert!(!scan_namespaces(&proc, &[], false).unwrap());
        assert_eq!(
            unmount_from(&paths, i32::MAX, *PERSONAL_APP_UIDS.start()).unwrap(),
            Unmounted::Gone
        );
    }
}
