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

//! Makes the filtered apps trust the authority, and nobody else.
//!
//! An app reads the roots it trusts from two directories: the system store
//! (of the conscrypt APEX from Android 14, of /system before), which is
//! read-only, and the user's store. For each, a copy with our root added is
//! kept, and bind-mounted over the directory in the mount namespace of a
//! process of a filtered app, and of no other.
//!
//! Every app process has a mount namespace of its own, from which mounts
//! do not spread: an app that was not chosen, the system, and the user in
//! Settings see the stores as they are. Nothing is added to either store
//! itself, and a restart of the app undoes it.
//!
//! The mounting is done in a process of its own (`abd trust mount`), as
//! only a process with a single thread may enter another mount namespace.
//! Each mount is read-only, and without what a store has no use for
//! (devices, set-uid, execution).

#![allow(unsafe_code, reason = "fork, setns, mount and extended attributes")]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use abd_engine::apps::{PER_USER_RANGE, PERSONAL_APP_UIDS};
use anyhow::{Context, Result, bail, ensure};
use md5::{Digest, Md5};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

mod profile;

pub use self::profile::{PROFILE_STORES, profile_roots};
use crate::paths::{Module as _, Paths};

const APEX_STORE: &str = "/apex/com.android.conscrypt/cacerts";
const SYSTEM_STORE: &str = "/system/etc/security/cacerts";
const USER_STORE: &str = "/data/misc/user/0/cacerts-added";
const STORE_LABEL: &str = "u:object_r:system_security_cacerts_file:s0";
/// What the user's store is labelled where it cannot be asked.
const USER_STORE_LABEL: &str = "u:object_r:misc_user_data_file:s0";
/// AID_SYSTEM, which owns the user's store.
const SYSTEM_UID: u32 = 1000;
const CA_LIMIT: usize = 64 << 10;

/// What `abd trust mount` exits with for a process that is gone: no
/// failure, as there is nobody left to trust anything.
pub const EXIT_GONE: i32 = 2;
/// And for a process that had the mounts already.
pub const EXIT_HAD: i32 = 3;

fn system_store() -> &'static Path {
    if Path::new(APEX_STORE).is_dir() {
        Path::new(APEX_STORE)
    } else {
        Path::new(SYSTEM_STORE)
    }
}

/// One of the two stores: the directory apps read, our copy of it, and the
/// name of our root in it. Where that file is, the copy is mounted.
struct Store {
    target: PathBuf,
    copy: PathBuf,
    root: String,
}

impl Store {
    fn mark(&self) -> PathBuf {
        self.target.join(&self.root)
    }
}

fn stores(paths: &Paths) -> Result<[Store; 2]> {
    let ca = paths.ca();
    let name = |file: &str| -> Result<String> {
        store_name(&std::fs::read(ca.join(file)).with_context(|| file.to_owned())?)
    };
    Ok([
        Store {
            target: system_store().to_owned(),
            copy: paths.store(),
            root: name("system.pem")?,
        },
        Store {
            target: USER_STORE.into(),
            copy: paths.user_store(),
            root: name("user.pem")?,
        },
    ])
}

/// The name a store keeps a certificate under: the first four bytes of the
/// MD5 of its subject, as OpenSSL before 1.0 hashed it, and `.0`.
fn store_name(pem: &[u8]) -> Result<String> {
    let der = CertificateDer::pem_slice_iter(pem)
        .next()
        .context("no certificate")??;
    let (_, cert) = x509_parser::parse_x509_certificate(&der).context("parse certificate")?;
    let digest = Md5::digest(cert.subject().as_raw());
    let hash = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]);
    Ok(format!("{hash:08x}.0"))
}

fn cstring(path: &Path) -> Result<CString> {
    Ok(CString::new(path.as_os_str().as_bytes())?)
}

fn set_label(path: &Path, label: &str) -> Result<()> {
    let p = cstring(path)?;
    // SAFETY: both strings are NUL-terminated and outlive the call, and the
    // value is passed with its own length.
    let r = unsafe {
        libc::setxattr(
            p.as_ptr(),
            c"security.selinux".as_ptr(),
            label.as_ptr().cast(),
            label.len(),
            0,
        )
    };
    ensure!(
        r == 0,
        "label {}: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
    Ok(())
}

fn label_of(path: &Path) -> Option<String> {
    let p = cstring(path).ok()?;
    let mut buf = [0u8; 256];
    // SAFETY: the path is NUL-terminated, and at most the length of `buf`
    // is written to it.
    let n = unsafe {
        libc::getxattr(
            p.as_ptr(),
            c"security.selinux".as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    let label = buf.get(..usize::try_from(n).ok()?)?;
    let label = label.strip_suffix(&[0]).unwrap_or(label);
    Some(std::str::from_utf8(label).ok()?.to_owned())
}

/// Builds the two copies, with the roots of the authority in `paths`.
pub fn prepare(paths: &Paths) -> Result<()> {
    let ca = paths.ca();
    let [system, user] = stores(paths)?;
    let pem = std::fs::read(ca.join("system.pem"))?;
    build_copy(
        &system.target,
        &system.copy,
        &system.root,
        &pem,
        STORE_LABEL,
    )?;
    // Where an empty store would be mounted over the real one, the device
    // would trust nothing.
    ensure!(
        std::fs::read_dir(&system.copy)?.count() > 1,
        "{} is empty",
        system.target.display()
    );

    // The user's store is made when the user first adds a certificate. A
    // mount needs it to be there: made empty, as it is after the user
    // removed the last.
    if std::fs::symlink_metadata(&user.target).is_err() {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .mode(0o755)
            .create(&user.target)?;
        std::os::unix::fs::lchown(&user.target, Some(SYSTEM_UID), Some(SYSTEM_UID))?;
        let _ = std::process::Command::new("restorecon")
            .arg(&user.target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    ensure!(
        std::fs::symlink_metadata(&user.target)?
            .file_type()
            .is_dir(),
        "{} is not a directory",
        user.target.display()
    );
    let label = label_of(&user.target).unwrap_or_else(|| USER_STORE_LABEL.to_owned());
    let pem = std::fs::read(ca.join("user.pem"))?;
    build_copy(&user.target, &user.copy, &user.root, &pem, &label)
}

/// A copy of `store` in `copy` with our root added.
///
/// Made in place, a file at a time and each in one step: the directory may
/// be mounted as the store of a running app, and must at no moment be
/// empty or hold half a certificate.
fn build_copy(store: &Path, copy: &Path, name: &str, pem: &[u8], label: &str) -> Result<()> {
    std::fs::create_dir_all(copy)?;
    let put =
        |name: &std::ffi::OsStr, write: &dyn Fn(&Path) -> std::io::Result<()>| -> Result<()> {
            let tmp = copy.join(".new");
            write(&tmp)?;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
            set_label(&tmp, label)?;
            std::fs::rename(&tmp, copy.join(name))?;
            Ok(())
        };
    let mut names = BTreeSet::new();
    for entry in std::fs::read_dir(store)? {
        let entry = entry?;
        // Certificates only: nothing else belongs in a store, and a link
        // would be followed as root.
        if !entry.file_type()?.is_file() {
            continue;
        }
        put(&entry.file_name(), &|tmp| {
            std::fs::copy(entry.path(), tmp).map(drop)
        })?;
        names.insert(entry.file_name());
    }
    put(name.as_ref(), &|tmp| std::fs::write(tmp, pem))?;
    names.insert(name.into());
    // What is no longer trusted there is not kept trusted here.
    for entry in std::fs::read_dir(copy)? {
        let entry = entry?;
        if !names.contains(&entry.file_name()) {
            std::fs::remove_file(entry.path())?;
        }
    }
    std::fs::set_permissions(copy, std::fs::Permissions::from_mode(0o755))?;
    set_label(copy, label)
}

/// Nothing can be run from a copy, and nothing written to it through the
/// mount, whoever comes to own a file in it.
const READ_ONLY: libc::c_ulong = libc::MS_BIND
    | libc::MS_REMOUNT
    | libc::MS_RDONLY
    | libc::MS_NOSUID
    | libc::MS_NODEV
    | libc::MS_NOEXEC;

/// The two paths of a store and its mark, as the system calls take them.
struct Mount {
    copy: CString,
    target: CString,
    mark: CString,
}

impl Mount {
    fn of(store: &Store) -> Result<Self> {
        Ok(Self {
            copy: cstring(&store.copy)?,
            target: cstring(&store.target)?,
            mark: cstring(&store.mark())?,
        })
    }

    /// Mounts the copy in the namespace this thread is in, unless it is.
    ///
    /// # Safety
    /// Makes system calls only, on strings that outlive them.
    unsafe fn mount(&self) -> bool {
        // SAFETY: the three strings are NUL-terminated and owned by self.
        unsafe {
            if libc::access(self.mark.as_ptr(), libc::F_OK) == 0 {
                return true;
            }
            let null = std::ptr::null();
            if libc::mount(
                self.copy.as_ptr(),
                self.target.as_ptr(),
                null,
                libc::MS_BIND,
                null.cast(),
            ) != 0
            {
                return false;
            }
            if libc::mount(null, self.target.as_ptr(), null, READ_ONLY, null.cast()) != 0 {
                // A copy that could not be made read-only is not left mounted.
                libc::umount2(self.target.as_ptr(), libc::MNT_DETACH);
                return false;
            }
            true
        }
    }

    /// # Safety
    /// As [`Mount::mount`].
    unsafe fn mounted(&self) -> bool {
        // SAFETY: the string is NUL-terminated and owned by self.
        unsafe { libc::access(self.mark.as_ptr(), libc::F_OK) == 0 }
    }

    /// # Safety
    /// As [`Mount::mount`].
    unsafe fn unmount(&self) -> bool {
        // SAFETY: the strings are NUL-terminated and owned by self.
        unsafe { !self.mounted() || libc::umount2(self.target.as_ptr(), libc::MNT_DETACH) == 0 }
    }
}

/// Whether a mount made on `path` in the namespace `mountinfo` describes
/// would pass to other namespaces: whether the mount `path` is in is
/// shared. One made in an app's own namespace is not; one in the first
/// namespace, or in one that shares its mounts, would change what every
/// process sees.
fn spreads(mountinfo: &str, path: &Path) -> bool {
    let mut within: Option<(usize, bool)> = None;
    for line in mountinfo.lines() {
        // id parent major:minor root mount-point options [optional...] - ...
        let mut fields = line.split(' ');
        let Some(point) = fields.nth(4) else { continue };
        if !path.starts_with(point) {
            continue;
        }
        let shared = fields
            .skip(1)
            .take_while(|f| *f != "-")
            .any(|f| f.starts_with("shared:"));
        // The longest mount point, and of equal ones the last, which lies
        // over those before it.
        if within.is_none_or(|(len, _)| point.len() >= len) {
            within = Some((point.len(), shared));
        }
    }
    within.is_none_or(|(_, shared)| shared)
}

/// Opens `name` below the directory `dir`.
fn open_below(dir: &OwnedFd, name: &std::ffi::CStr, flags: i32) -> std::io::Result<OwnedFd> {
    // SAFETY: the name is NUL-terminated; the result is checked.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: just opened, and owned by nothing else.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// A process of an app, held by its directory in /proc and its mount
/// namespace: its number may be given to another once it ends, and
/// everything after is asked of this one.
struct Pinned {
    dir: OwnedFd,
    ns: OwnedFd,
}

/// Holds process `pid`, which must be one of the personal app `uid`;
/// `None` once it has ended.
fn pin(pid: i32, uid: u32) -> Result<Option<Pinned>> {
    ensure!(
        (*PERSONAL_APP_UIDS.start()..PER_USER_RANGE).contains(&uid),
        "{uid} is not a personal app's"
    );
    let Some(dir) = present(std::fs::File::open(format!("/proc/{pid}")))? else {
        return Ok(None);
    };
    let Some(metadata) = present(dir.metadata())? else {
        return Ok(None);
    };
    ensure!(metadata.uid() == uid, "process {pid} is not of {uid}");
    let dir = OwnedFd::from(dir);
    let Some(ns) = present(open_below(&dir, c"ns/mnt", libc::O_RDONLY))? else {
        return Ok(None);
    };
    Ok(Some(Pinned { dir, ns }))
}

/// The mounts of the namespace `ns`, as the process held by `dir` reads
/// them; `None` once it has ended. Fails if the process is in another
/// namespace by then.
fn mountinfo_of(dir: &OwnedFd, ns: &OwnedFd) -> Result<Option<String>> {
    let info = present(open_below(dir, c"mountinfo", libc::O_RDONLY))?
        .map(|file| abd_engine::files::read_limited(std::fs::File::from(file), 1 << 20))
        .transpose()?;
    let Some(current) = present(open_below(dir, c"ns/mnt", libc::O_RDONLY))? else {
        return Ok(None);
    };
    let pinned = std::fs::File::from(ns.try_clone()?).metadata()?;
    let current = std::fs::File::from(current).metadata()?;
    ensure!(
        (pinned.dev(), pinned.ino()) == (current.dev(), current.ino()),
        "mount namespace changed during inspection"
    );
    info.context("live namespace has no mount information")
        .map(Some)
}

/// What mounting in a process came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Mounting {
    Mounted,
    /// It had them all already.
    Had,
    /// It ended before it could be looked at.
    Gone,
}

/// Mounts the copies over the stores in the mount namespace of process
/// `pid`, which must be a process of the app `uid`.
///
/// What `abd trust mount` does. Must be called in a process with a single
/// thread, which is in another mount namespace when this returns: the
/// process ends after it.
pub fn mount_in(paths: &Paths, pid: i32, uid: u32) -> Result<Mounting> {
    let Some(Pinned { dir, ns }) = pin(pid, uid)? else {
        return Ok(Mounting::Gone);
    };
    let stores = stores(paths)?;
    let mounts: Vec<Mount> = stores.iter().map(Mount::of).collect::<Result<_>>()?;

    // Never the namespace every process descends from.
    let first = std::fs::metadata("/proc/1/ns/mnt")?;
    let this = std::fs::File::from(ns.try_clone()?).metadata()?;
    ensure!(
        (this.dev(), this.ino()) != (first.dev(), first.ino()),
        "process {pid} is in the first mount namespace"
    );
    let Some(mountinfo) = mountinfo_of(&dir, &ns)? else {
        return Ok(Mounting::Gone);
    };
    for store in &stores {
        ensure!(
            !spreads(&mountinfo, &store.target),
            "a mount on {} would spread from process {pid}",
            store.target.display()
        );
    }

    // SAFETY: this process has one thread, as the caller sees to. The
    // descriptor is a mount namespace's, held open above.
    let entered = unsafe { libc::setns(ns.as_raw_fd(), libc::CLONE_NEWNS) };
    ensure!(
        entered == 0,
        "entering the namespace: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: system calls on strings owned by `mounts`.
    let had = mounts.iter().all(|mount| unsafe { mount.mounted() });
    for mount in &mounts {
        // SAFETY: system calls on strings owned by `mounts`.
        if !unsafe { mount.mount() } {
            let failed = std::io::Error::last_os_error();
            // Both or neither: an app that trusted one root and not the
            // other would accept us in some of its connections only.
            for mount in &mounts {
                // SAFETY: as above.
                unsafe { mount.unmount() };
            }
            bail!("mounting: {failed}");
        }
    }
    Ok(if had {
        Mounting::Had
    } else {
        Mounting::Mounted
    })
}

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

fn present<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)) => Ok(None),
        Err(e) => Err(e),
    }
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

    #[test]
    fn names_a_certificate_as_openssl_subject_hash_old_does() {
        // ISRG Root X1; `openssl x509 -subject_hash_old` prints 6187b673.
        let pem = include_bytes!("../testdata/isrg-root-x1.pem");
        assert_eq!(store_name(pem).unwrap(), "6187b673.0");
    }

    #[test]
    fn the_roots_are_stored_under_the_names_the_pin_hook_reads() {
        // android/hook/.../LocalAuthority.kt finds the roots by these names.
        let dir = tempfile::tempdir().unwrap();
        abd_engine::ca::Authority::open(dir.path()).unwrap();
        let name = |file: &str| store_name(&std::fs::read(dir.path().join(file)).unwrap()).unwrap();
        assert_eq!(name("system.pem"), "494499dc.0");
        assert_eq!(name("user.pem"), "5554896d.0");
    }

    #[test]
    fn the_copies_are_mounted_from_where_the_pin_hook_expects_them() {
        // android/hook/.../ModuleMounts.kt recognizes the mounts by these
        // directories, as paths within the file system mounted at /data.
        let paths = Paths::new(Path::new(crate::paths::DATA));
        assert_eq!(paths.store(), Path::new("/data/adb/adblocker/store"));
        assert_eq!(
            paths.user_store(),
            Path::new("/data/adb/adblocker/user-store")
        );
    }

    #[test]
    fn managed_mount_commands_refuse_to_enter_a_work_process() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        for error in [
            mount_in(&paths, 1, 1_010_001).unwrap_err(),
            unmount_from(&paths, 1, 1_010_001).unwrap_err(),
        ] {
            assert!(error.to_string().contains("not a personal app"));
        }
    }

    /// The lines of an app's namespace, and of the first.
    const APP: &str = "\
6689 6660 254:5 / / ro,relatime master:1 - ext4 /dev/block/dm-5 ro,seclabel
6750 6689 254:24 / /apex/com.android.conscrypt ro,nodev,noatime - ext4 /dev/block/dm-24 ro,seclabel
6782 6660 254:46 / /data rw,nosuid,nodev,noatime master:34 - ext4 /dev/block/dm-46 rw,seclabel
6783 6782 254:46 /data /data/user/0 rw,nosuid,nodev,noatime master:34 - ext4 /dev/block/dm-46 rw
";
    const FIRST: &str = "\
21 1 254:5 / / ro,relatime shared:1 - ext4 /dev/block/dm-5 ro,seclabel
99 21 254:24 / /apex/com.android.conscrypt ro,nodev,noatime shared:60 - ext4 /dev/block/dm-24 ro
45 21 254:46 / /data rw,nosuid,nodev,noatime shared:34 - ext4 /dev/block/dm-46 rw,seclabel
";

    #[test]
    fn a_mount_in_an_apps_own_namespace_does_not_spread() {
        assert!(!spreads(APP, Path::new(APEX_STORE)));
        assert!(!spreads(APP, Path::new(USER_STORE)));
    }

    #[test]
    fn a_mount_in_a_namespace_that_shares_its_mounts_would_spread() {
        assert!(spreads(FIRST, Path::new(APEX_STORE)));
        assert!(spreads(FIRST, Path::new(USER_STORE)));
        // Shared only where the store is: still spreads.
        let mixed = format!(
            "{APP}7000 6750 254:24 / /apex/com.android.conscrypt ro shared:9 - ext4 x ro\n"
        );
        assert!(spreads(&mixed, Path::new(APEX_STORE)));
    }

    #[test]
    fn where_nothing_can_be_told_a_mount_is_taken_to_spread() {
        assert!(spreads("", Path::new(APEX_STORE)));
        assert!(spreads("garbage\n", Path::new(APEX_STORE)));
        // A mount point that only begins like the path is not above it.
        let other = "1 0 0:1 / /apex/com.android.conscrypt2 ro - ext4 x ro\n";
        assert!(spreads(other, Path::new(APEX_STORE)));
    }

    #[test]
    fn a_copy_is_the_store_with_our_root_and_follows_it_when_built_again() {
        let (store, copy) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let label = label_of(store.path());
        // Without a label to copy (no SELinux here), nothing can be built.
        let Some(label) = label else { return };
        std::fs::write(store.path().join("aaaa.0"), "a").unwrap();
        std::fs::write(store.path().join("bbbb.0"), "b").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", store.path().join("link.0")).unwrap();
        build_copy(store.path(), copy.path(), "ours.0", b"ours", &label).unwrap();
        let names = |dir: &Path| -> BTreeSet<String> {
            std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect()
        };
        assert_eq!(
            names(copy.path()),
            BTreeSet::from(["aaaa.0".into(), "bbbb.0".into(), "ours.0".into()])
        );
        // A root the system no longer trusts.
        std::fs::remove_file(store.path().join("aaaa.0")).unwrap();
        build_copy(store.path(), copy.path(), "ours.0", b"ours", &label).unwrap();
        assert_eq!(
            names(copy.path()),
            BTreeSet::from(["bbbb.0".into(), "ours.0".into()])
        );
    }

    #[test]
    fn nothing_is_mounted_for_a_uid_that_is_not_an_apps() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        abd_engine::ca::Authority::open(&paths.ca()).unwrap();
        for uid in [0, 1000, 2000, 1_001_000] {
            let err = mount_in(&paths, std::process::id() as i32, uid).unwrap_err();
            assert!(err.to_string().contains("not a personal app's"), "{err}");
        }
    }

    #[test]
    fn a_process_that_has_ended_is_gone_to_mounting_as_to_unmounting() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let uid = *PERSONAL_APP_UIDS.start();
        assert_eq!(mount_in(&paths, i32::MAX, uid).unwrap(), Mounting::Gone);
        assert_eq!(
            unmount_from(&paths, i32::MAX, uid).unwrap(),
            Unmounted::Gone
        );
    }

    #[test]
    fn nothing_is_mounted_in_a_process_of_another_uid() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        abd_engine::ca::Authority::open(&paths.ca()).unwrap();
        // This process is not an app's, whatever it is.
        let err = mount_in(&paths, std::process::id() as i32, 10_123).unwrap_err();
        assert!(err.to_string().contains("is not of"), "{err}");
    }
}
