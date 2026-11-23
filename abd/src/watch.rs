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

//! Watches Android packages, managed user state, and profile trust stores.
//!
//! The rules match UIDs, and a UID is given to another app once its app is
//! removed: without this, a new app would be filtered for having the
//! number of one the user chose and has since removed.
//!
//! The package manager replaces `/data/system/packages.list` by renaming a
//! new file over it, so the directory is watched, not the file. Nothing
//! runs between changes: the wait is on the descriptor.

#![allow(unsafe_code, reason = "inotify and child process setup")]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};

use crate::android::package_settings::{BACKUP, PRIMARY};

const DIR: &str = "/data/system";
const FILE: &[u8] = b"packages.list";
const PUBLISHED: u32 = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO;
const REMOVED: u32 = libc::IN_DELETE | libc::IN_MOVED_FROM;
const CHANGES: u32 = libc::IN_MOVED_TO
    | libc::IN_MOVED_FROM
    | libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_CLOSE_WRITE;
/// `struct inotify_event` before its name: wd, mask, cookie, len.
const EVENT_HEADER: usize = 16;

const FIRST_RETRY: Duration = Duration::from_secs(2);
const MAX_RETRY: Duration = Duration::from_secs(60);

/// Retries a failed Android lookup while it is relevant, without polling
/// healthy or intentionally unavailable profiles.
pub struct Retry {
    delay: Duration,
    timer: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl Retry {
    pub fn new() -> Self {
        Self {
            delay: FIRST_RETRY,
            timer: None,
        }
    }

    pub fn arm(&mut self, unavailable: bool) {
        if !unavailable {
            self.timer = None;
            self.delay = FIRST_RETRY;
        } else if self.timer.is_none() {
            self.timer = Some(Box::pin(tokio::time::sleep(self.delay)));
            self.delay = (self.delay * 2).min(MAX_RETRY);
        }
    }

    pub async fn fired(&mut self) {
        if let Some(timer) = &mut self.timer {
            timer.await;
            self.timer = None;
        } else {
            std::future::pending::<()>().await;
        }
    }
}

#[derive(Default)]
pub struct Change {
    pub users: bool,
    pub package_owners: bool,
    pub trust: BTreeSet<u32>,
    pub profile_users: BTreeSet<u32>,
}

pub struct Packages {
    fd: AsyncFd<OwnedFd>,
    system: i32,
    settings_backup: bool,
    user_dir: PathBuf,
    users: Option<i32>,
    profile_packages: BTreeMap<i32, u32>,
    profile_ids: BTreeSet<u32>,
    /// Serial and whether the watch is inside a certificate directory.
    trust: BTreeMap<i32, (u32, bool)>,
}

impl Packages {
    pub fn watch() -> std::io::Result<Self> {
        Self::watch_system(Path::new(DIR))
    }

    fn watch_system(dir: &Path) -> std::io::Result<Self> {
        // SAFETY: a system call with no pointers; the result is checked.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: just opened, and owned by nothing else.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let path = CString::new(dir.as_os_str().as_encoded_bytes())?;
        // SAFETY: the NUL-terminated path outlives the call.
        let system = unsafe { libc::inotify_add_watch(fd.as_raw_fd(), path.as_ptr(), CHANGES) };
        if system < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: the descriptor is owned by the AsyncFd from here on,
        // and so stays open for as long as it is registered.
        let registered = unsafe { AsyncFd::register_with_interest(fd, Interest::READABLE) };
        let mut this = Self {
            fd: registered?,
            system,
            settings_backup: dir.join(BACKUP).exists(),
            user_dir: dir.join("users"),
            users: None,
            profile_packages: BTreeMap::new(),
            profile_ids: BTreeSet::new(),
            trust: BTreeMap::new(),
        };
        this.users = this.watch_dir(&this.user_dir);
        if let Some(owner) = this.watch_dir(&this.user_dir.join("0")) {
            this.profile_packages.insert(owner, 0);
        }
        Ok(this)
    }

    fn watch_dir(&self, dir: &Path) -> Option<i32> {
        let path = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).ok()?;
        // SAFETY: NUL-terminated path on a descriptor owned by this watcher.
        let wd = unsafe {
            libc::inotify_add_watch(
                self.fd.as_raw_fd(),
                path.as_ptr(),
                CHANGES | libc::IN_ONLYDIR,
            )
        };
        (wd >= 0).then_some(wd)
    }

    /// Watches work profile package state and certificate stores. Only an
    /// observable installation can be captured and only observable trust
    /// can permit decryption.
    pub fn set_profiles(
        &mut self,
        profiles: impl IntoIterator<Item = crate::android::Profile>,
    ) -> (BTreeSet<u32>, BTreeSet<u32>) {
        let mut installed = BTreeSet::new();
        let mut trusted = BTreeSet::new();
        let mut packages = BTreeMap::new();
        let mut trust = BTreeMap::new();
        self.users = self.watch_dir(&self.user_dir);
        if let Some(owner) = self.watch_dir(&self.user_dir.join("0")) {
            packages.insert(owner, 0);
        }
        if self.users.is_some() {
            for profile in profiles {
                let id = profile.id;
                let id_text = id.to_string();
                let Some(wd) = self.watch_dir(&self.user_dir.join(&id_text)) else {
                    continue;
                };
                packages.insert(wd, id);
                installed.insert(id);
                let misc = Path::new(crate::trust::PROFILE_STORES).join(&id_text);
                if let Some(user) = self.watch_dir(&misc) {
                    trust.insert(user, (profile.serial, false));
                    if let Some(added) = self.watch_dir(&misc.join("cacerts-added")) {
                        trust.insert(added, (profile.serial, true));
                        let removed = misc.join("cacerts-removed");
                        match self.watch_dir(&removed) {
                            Some(wd) => {
                                trust.insert(wd, (profile.serial, true));
                            }
                            None if matches!(removed.try_exists(), Ok(false)) => {}
                            None => continue,
                        }
                        trusted.insert(id);
                    }
                }
            }
        }
        for wd in self
            .profile_packages
            .keys()
            .chain(self.trust.keys())
            .copied()
        {
            if !packages.contains_key(&wd) && !trust.contains_key(&wd) {
                // SAFETY: releases a watch on the descriptor owned here.
                unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), wd as _) };
            }
        }
        self.profile_packages = packages;
        self.profile_ids.clone_from(&installed);
        self.trust = trust;
        (installed, trusted)
    }

    pub fn observes_packages(&self, user: u32) -> bool {
        self.profile_packages.values().any(|&id| id == user)
    }

    pub fn includes_profile(&self, id: u32) -> bool {
        self.profile_ids.contains(&id)
    }

    pub fn has_profiles(&self) -> bool {
        !self.profile_ids.is_empty()
    }

    /// Returns once Android updates packages, managed users, or their trust.
    pub async fn changed(&mut self) -> Change {
        loop {
            let mut buf = [0u8; 4096];
            let n = {
                let Ok(mut ready) = self.fd.readable().await else {
                    // Nothing to wait on any more: never, rather than at once
                    // and for ever.
                    return std::future::pending().await;
                };
                // SAFETY: reads at most the length of `buf` into it.
                let n =
                    unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 {
                    ready.clear_ready();
                    continue;
                }
                n
            };
            let mut changed = Change::default();
            for (wd, mask, name) in events(&buf[..n as usize]) {
                if wd == -1 {
                    // Inotify overflow makes every cached result suspect.
                    self.settings_backup = self.user_dir.with_file_name(BACKUP).exists();
                    changed.users = true;
                    changed.package_owners = true;
                    changed
                        .trust
                        .extend(self.trust.values().map(|(serial, _)| *serial));
                }
                changed.package_owners |= wd == self.system
                    && (name == FILE || self.package_settings_changed(mask, name));
                changed.users |=
                    Some(wd) == self.users && user_records_changed(&self.user_dir, mask, name);
                if let Some(&user) = self.profile_packages.get(&wd)
                    && (name.is_empty()
                        || name == b"package-restrictions.xml"
                            && mask & (PUBLISHED | libc::IN_DELETE) != 0)
                {
                    changed.profile_users.insert(user);
                }
                if let Some((serial, inside)) = self.trust.get(&wd)
                    && (*inside
                        || name.is_empty()
                        || matches!(name, b"cacerts-added" | b"cacerts-removed"))
                {
                    changed.trust.insert(*serial);
                }
            }
            if changed.users
                || changed.package_owners
                || !changed.profile_users.is_empty()
                || !changed.trust.is_empty()
            {
                return changed;
            }
        }
    }

    fn package_settings_changed(&mut self, mask: u32, name: &[u8]) -> bool {
        // Track the backup in event order. Looking it up on disk at the
        // primary's close can see a later deletion and signal twice.
        if name == BACKUP.as_bytes() {
            if mask & REMOVED != 0 {
                self.settings_backup = false;
                return true;
            }
            if mask & (libc::IN_MOVED_TO | libc::IN_CREATE) != 0 {
                self.settings_backup = true;
            }
        }
        // The primary's move to the backup and the reserve refresh are
        // intermediate steps. With no backup, the primary close publishes
        // the initial write and deletion signals a lost primary.
        name == PRIMARY.as_bytes()
            && !self.settings_backup
            && mask & (PUBLISHED | libc::IN_DELETE) != 0
    }
}

/// With ResilientAtomicFile, deleting .backup publishes the replacement.
/// Older AtomicFile writers publish by renaming .new over the primary.
fn user_records_changed(dir: &Path, mask: u32, name: &[u8]) -> bool {
    if name.is_empty() {
        return true; // The watched directory is gone.
    }
    let Ok(name) = std::str::from_utf8(name) else {
        return false;
    };
    let removed = mask & (libc::IN_DELETE | libc::IN_MOVED_FROM) != 0;
    let primary = name
        .strip_suffix(".backup")
        .or_else(|| name.strip_suffix(".bak"))
        .unwrap_or(name);
    let record = primary == "userlist.xml"
        || primary
            .strip_suffix(".xml")
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()));
    if !record {
        return false;
    }
    if name != primary {
        // A backup can be discarded after publishing, or removed when
        // deleting a user. Both change the copy visible to the reader.
        return removed;
    }
    if removed {
        let backup = dir.join(primary).with_added_extension("backup");
        return !matches!(backup.try_exists(), Ok(true));
    }
    mask & PUBLISHED != 0
}

/// Activity Manager records runtime start and stop transitions independently
/// of persistent profile metadata. A filtered event stream avoids polling.
pub struct UserStates {
    _process: tokio::process::Child,
    output: BufReader<tokio::process::ChildStdout>,
}

impl UserStates {
    pub fn watch() -> std::io::Result<Self> {
        let mut command = tokio::process::Command::new("/system/bin/logcat");
        command
            .args([
                "-b",
                "events",
                "-v",
                "brief",
                "-v",
                "uid",
                "-v",
                "printable",
                "-T",
                "1",
                "-s",
                "am_user_state_changed:I",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let daemon = std::process::id();
        // SAFETY: only system calls run in the child before exec. The
        // event reader must die even if the supervisor kills the daemon.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // The daemon could already have died before the prctl.
                if libc::getppid() as u32 != daemon {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        let mut process = command.spawn()?;
        let output = BufReader::new(
            process
                .stdout
                .take()
                .ok_or_else(|| std::io::Error::other("no user state event stream"))?,
        );
        Ok(Self {
            _process: process,
            output,
        })
    }

    pub async fn changed(&mut self) -> Option<(u32, u32)> {
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = (&mut self.output)
                .take(256)
                .read_until(b'\n', &mut line)
                .await
                .ok()?;
            if n == 0 {
                return None;
            }
            if let Some(id) = std::str::from_utf8(&line).ok().and_then(user_state_id) {
                return Some(id);
            }
        }
    }
}

fn user_state_id(line: &str) -> Option<(u32, u32)> {
    let (source, payload) = line.split_once("): ")?;
    let (_, sender) = source.split_once("/am_user_state_changed(")?;
    let (uid, pid) = sender.split_once(':')?;
    // An app can write to the event log, including with a known tag.
    // Only accept the kernel-provided UID of the Android system server.
    if !matches!(uid.trim(), "1000" | "system") || pid.trim().parse::<u32>().is_err() {
        return None;
    }
    let (id, state) = payload
        .trim()
        .strip_prefix('[')?
        .strip_suffix(']')?
        .split_once(',')?;
    let state: u32 = state.trim().parse().ok()?;
    if state > 5 {
        return None;
    }
    let id = id.trim().parse().ok().filter(|id| *id != 0)?;
    Some((id, state))
}

/// Tells when a process of a filtered app starts.
///
/// The system makes a directory for every process of an app as it starts
/// it, below one for the app's UID, in the control group tree. Both are
/// watched: the tree for the directory of a UID, which is not there while
/// the app has no process, and that directory for those of its processes.
pub struct Starts {
    fd: AsyncFd<OwnedFd>,
    root: PathBuf,
    /// The watch on the tree, and those on the directories of UIDs.
    tree: i32,
    apps: HashMap<i32, u32>,
    wanted: BTreeSet<u32>,
    /// The apps seen to start a process and not yet told of.
    found: BTreeSet<u32>,
}

impl Starts {
    /// Watches the control group tree at `root`.
    pub fn watch(root: &Path) -> std::io::Result<Self> {
        // SAFETY: a system call with no pointers; the result is checked.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: just opened, and owned by nothing else.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let tree = add_watch(&fd, root)?;
        // SAFETY: the descriptor is owned by the AsyncFd from here on,
        // and so stays open for as long as it is registered.
        let fd = unsafe { AsyncFd::register_with_interest(fd, Interest::READABLE) }?;
        Ok(Self {
            fd,
            root: root.to_owned(),
            tree,
            apps: HashMap::new(),
            wanted: BTreeSet::new(),
            found: BTreeSet::new(),
        })
    }

    /// Watches for the processes of `uids` and of no others.
    pub fn set(&mut self, uids: &BTreeSet<u32>) {
        let fd = self.fd.get_ref();
        self.apps.retain(|watch, uid| {
            let keep = uids.contains(uid);
            if !keep {
                // SAFETY: a system call on a descriptor we own.
                unsafe { libc::inotify_rm_watch(fd.as_raw_fd(), *watch as _) };
            }
            keep
        });
        self.wanted.clone_from(uids);
        for uid in uids {
            self.watch_app(*uid);
        }
    }

    /// Watches the directory of `uid`, if it is there. Watching one that
    /// is watched already changes nothing.
    fn watch_app(&mut self, uid: u32) {
        let dir = self.root.join(format!("uid_{uid}"));
        if let Ok(watch) = add_watch(self.fd.get_ref(), &dir) {
            self.apps.insert(watch, uid);
        }
    }

    /// Returns the UID of a filtered app when a process of it starts.
    pub async fn started(&mut self) -> u32 {
        loop {
            // Each of several in a buffer in its turn; one for an app is
            // enough, as all its processes are gone through for any.
            if let Some(uid) = self.found.pop_first() {
                return uid;
            }
            let Ok(mut ready) = self.fd.readable().await else {
                return std::future::pending().await;
            };
            let mut buf = [0u8; 4096];
            // SAFETY: reads at most the length of `buf` into it.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                ready.clear_ready();
                continue;
            }
            drop(ready);
            for (watch, mask, name) in events(&buf[..n as usize]) {
                let name = std::str::from_utf8(name).unwrap_or_default();
                if mask & libc::IN_IGNORED != 0 {
                    // The app's last process ended and its directory went
                    // with the watch; its next start makes a new one.
                    self.apps.remove(&watch);
                } else if watch == self.tree {
                    // The first process of an app: its directory is new.
                    let uid = name.strip_prefix("uid_").and_then(|u| u.parse().ok());
                    if let Some(uid) = uid.filter(|uid| self.wanted.contains(uid)) {
                        self.watch_app(uid);
                        self.found.insert(uid);
                    }
                } else if name.starts_with("pid_")
                    && let Some(uid) = self.apps.get(&watch)
                {
                    self.found.insert(*uid);
                }
            }
        }
    }
}

fn add_watch(fd: &OwnedFd, dir: &Path) -> std::io::Result<i32> {
    let path = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes())?;
    // SAFETY: the path is NUL-terminated and outlives the call.
    let watch = unsafe {
        libc::inotify_add_watch(
            fd.as_raw_fd(),
            path.as_ptr(),
            libc::IN_CREATE | libc::IN_ONLYDIR,
        )
    };
    if watch < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(watch)
}

/// The watches, masks and file names in a buffer of inotify events.
fn events(mut buf: &[u8]) -> impl Iterator<Item = (i32, u32, &[u8])> {
    std::iter::from_fn(move || {
        let header = buf.get(..EVENT_HEADER)?;
        let watch = i32::from_ne_bytes(header[0..4].try_into().ok()?);
        let mask = u32::from_ne_bytes(header[4..8].try_into().ok()?);
        let len = u32::from_ne_bytes(header[12..16].try_into().ok()?) as usize;
        let name = buf.get(EVENT_HEADER..EVENT_HEADER.checked_add(len)?)?;
        buf = &buf[EVENT_HEADER + len..];
        // Padded with NULs to the alignment of the next event.
        let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
        Some((watch, mask, &name[..end]))
    })
}

/// The file names in a buffer of inotify events.
#[cfg(test)]
fn names(buf: &[u8]) -> impl Iterator<Item = &[u8]> {
    events(buf).map(|(_, _, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &[u8], padded: usize) -> Vec<u8> {
        let mut e = vec![0u8; EVENT_HEADER];
        e[12..16].copy_from_slice(&(padded as u32).to_ne_bytes());
        e.extend_from_slice(name);
        e.resize(EVENT_HEADER + padded, 0);
        e
    }

    #[test]
    fn reads_the_names_of_the_events_in_a_buffer() {
        let mut buf = event(b"packages.list", 16);
        buf.extend(event(b"other", 8));
        buf.extend(event(b"", 0));
        let found: Vec<&[u8]> = names(&buf).collect();
        assert_eq!(found, [&b"packages.list"[..], b"other", b""]);
    }

    #[tokio::test]
    async fn package_settings_rewrites_publish_once_when_the_backup_is_deleted() {
        for uses_reserve in [false, true] {
            let system = tempfile::tempdir().unwrap();
            let dir = system.path();
            let owner = dir.join("users/0");
            std::fs::create_dir_all(&owner).unwrap();
            let signal = || std::fs::write(owner.join("package-restrictions.xml"), b"").unwrap();
            let primary = dir.join(PRIMARY);
            let backup = dir.join(BACKUP);
            let reserve = dir.join("packages.xml.reservecopy");
            std::fs::write(&primary, b"previous").unwrap();
            if uses_reserve {
                std::fs::write(&reserve, b"previous").unwrap();
            }
            let mut packages = Packages::watch_system(dir).unwrap();

            std::fs::rename(&primary, &backup).unwrap();
            if uses_reserve {
                std::fs::remove_file(&reserve).unwrap();
            }
            let replacement = std::fs::File::create(&primary).unwrap();
            let copy = uses_reserve.then(|| std::fs::File::create(&reserve).unwrap());
            signal();
            let started = packages.changed().await;
            assert!(started.profile_users.contains(&0));
            assert!(!started.package_owners);

            drop(replacement);
            signal();
            let main_closed = packages.changed().await;
            assert!(main_closed.profile_users.contains(&0));
            assert!(!main_closed.package_owners);

            std::fs::remove_file(&backup).unwrap();
            signal();
            assert!(packages.changed().await.package_owners);

            drop(copy);
            signal();
            let finished = packages.changed().await;
            assert!(finished.profile_users.contains(&0));
            assert!(!finished.package_owners);
        }
    }

    #[tokio::test]
    async fn package_settings_initial_write_publishes_on_close() {
        let system = tempfile::tempdir().unwrap();
        let mut packages = Packages::watch_system(system.path()).unwrap();
        std::fs::write(system.path().join(PRIMARY), b"initial").unwrap();
        let change = tokio::time::timeout(Duration::from_secs(1), packages.changed()).await;
        assert!(change.unwrap().package_owners);
    }

    #[tokio::test]
    async fn a_user_record_rewrite_is_observed_when_the_backup_is_deleted() {
        use std::io::Write as _;

        let system = tempfile::tempdir().unwrap();
        let users = system.path().join("users");
        let owner = users.join("0");
        std::fs::create_dir_all(&owner).unwrap();
        let user = users.join("10.xml");
        let backup = users.join("10.xml.backup");
        std::fs::write(
            &user,
            br#"<user id="10" serialNumber="37" profileGroupId="0" flags="32"/>"#,
        )
        .unwrap();
        let mut packages = Packages::watch_system(system.path()).unwrap();

        std::fs::rename(&user, &backup).unwrap();
        let mut replacement = std::fs::File::create(&user).unwrap();
        replacement
            .write_all(br#"<user id="10" serialNumber="37""#)
            .unwrap();
        // This unrelated watched write is queued after the rename and
        // creation, so we can drain those events without a timer.
        std::fs::write(owner.join("package-restrictions.xml"), b"<packages/>").unwrap();
        let before_publication = packages.changed().await;
        assert!(before_publication.profile_users.contains(&0));
        assert!(!before_publication.users);

        replacement
            .write_all(br#" profileGroupId="0" flags="160"/>"#)
            .unwrap();
        drop(replacement);
        // Drain the primary close separately. The reader may still see
        // the backup at that point, so its deletion must signal again.
        std::fs::write(owner.join("package-restrictions.xml"), b"<packages/>").unwrap();
        assert!(packages.changed().await.profile_users.contains(&0));

        std::fs::remove_file(&backup).unwrap();
        let change = tokio::time::timeout(Duration::from_secs(1), packages.changed()).await;
        assert!(change.unwrap().users);
    }

    #[tokio::test]
    async fn a_user_roster_staged_by_atomic_file_is_observed_at_rename() {
        let system = tempfile::tempdir().unwrap();
        let users = system.path().join("users");
        let owner = users.join("0");
        std::fs::create_dir_all(&owner).unwrap();
        let mut packages = Packages::watch_system(system.path()).unwrap();
        let staged = users.join("userlist.xml.new");

        std::fs::write(&staged, b"<users/>").unwrap();
        std::fs::write(owner.join("package-restrictions.xml"), b"<packages/>").unwrap();
        let change = packages.changed().await;
        assert!(change.profile_users.contains(&0));
        assert!(!change.users);

        std::fs::rename(staged, users.join("userlist.xml")).unwrap();
        let change = tokio::time::timeout(Duration::from_secs(1), packages.changed()).await;
        assert!(change.unwrap().users);
    }

    #[tokio::test]
    async fn a_deleted_or_moved_away_record_without_a_backup_is_reported() {
        for removed_by_rename in [false, true] {
            let system = tempfile::tempdir().unwrap();
            let users = system.path().join("users");
            std::fs::create_dir(&users).unwrap();
            let record = users.join("10.xml");
            std::fs::write(&record, b"<user/>").unwrap();
            let mut packages = Packages::watch_system(system.path()).unwrap();

            if removed_by_rename {
                std::fs::rename(record, system.path().join("moved.xml")).unwrap();
            } else {
                std::fs::remove_file(record).unwrap();
            }
            std::fs::write(system.path().join("packages.list"), b"").unwrap();
            assert!(packages.changed().await.users);
        }
    }

    #[test]
    fn user_state_events_select_the_runtime_user_number() {
        assert_eq!(
            user_state_id("I/am_user_state_changed( 1000:  561): [10,3]\n"),
            Some((10, 3))
        );
        assert_eq!(
            user_state_id("I/am_user_state_changed(system:  561): [10, 4]\n"),
            Some((10, 4))
        );
        assert_eq!(
            user_state_id("I/am_user_state_changed(11089:  561): [10,4]\n"),
            None
        );
        assert_eq!(
            user_state_id("I/am_user_state_changed( 1000:  561): [0,3]\n"),
            None
        );
        assert_eq!(
            user_state_id("I/am_user_state_changed( 1000:  561): [10, ?]\n"),
            None
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_lookup_backs_off_and_success_cancels_the_retry() {
        let mut retry = Retry::new();
        retry.arm(true);
        let first = tokio::time::Instant::now();
        retry.fired().await;
        assert_eq!(first.elapsed(), FIRST_RETRY);
        retry.arm(true);
        let next = tokio::time::Instant::now();
        retry.fired().await;
        assert_eq!(next.elapsed(), FIRST_RETRY * 2);
        retry.arm(true);
        retry.arm(false);
        assert!(retry.timer.is_none());
        assert_eq!(retry.delay, FIRST_RETRY);
    }

    #[tokio::test]
    async fn a_process_of_a_filtered_app_starting_is_told_and_of_another_is_not() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::create_dir(tree.path().join("uid_10001")).unwrap();
        let mut starts = Starts::watch(tree.path()).unwrap();
        starts.set(&BTreeSet::from([10001, 10002]));
        // Another app's, then one of a filtered app that has processes.
        std::fs::create_dir(tree.path().join("uid_10009")).unwrap();
        std::fs::create_dir(tree.path().join("uid_10009/pid_5")).unwrap();
        std::fs::create_dir(tree.path().join("uid_10001/pid_6")).unwrap();
        assert_eq!(starts.started().await, 10001);
        // The first process of a filtered app that had none.
        std::fs::create_dir(tree.path().join("uid_10002")).unwrap();
        assert_eq!(starts.started().await, 10002);
        std::fs::create_dir(tree.path().join("uid_10002/pid_7")).unwrap();
        assert_eq!(starts.started().await, 10002);
        // No longer filtered.
        starts.set(&BTreeSet::from([10002]));
        std::fs::create_dir(tree.path().join("uid_10001/pid_8")).unwrap();
        std::fs::create_dir(tree.path().join("uid_10002/pid_9")).unwrap();
        assert_eq!(starts.started().await, 10002);
    }

    #[tokio::test]
    async fn the_watch_of_an_app_whose_last_process_ended_is_forgotten() {
        let tree = tempfile::tempdir().unwrap();
        let app = tree.path().join("uid_10001");
        std::fs::create_dir(&app).unwrap();
        let mut starts = Starts::watch(tree.path()).unwrap();
        starts.set(&BTreeSet::from([10001]));
        for _ in 0..3 {
            std::fs::remove_dir(&app).unwrap();
            std::fs::create_dir(&app).unwrap();
            assert_eq!(starts.started().await, 10001);
        }
        assert_eq!(starts.apps.len(), 1);
    }

    #[test]
    fn an_event_cut_short_ends_the_reading() {
        let mut buf = event(b"packages.list", 16);
        // Says it has a name longer than what follows.
        buf.extend(event(b"x", 4)[..EVENT_HEADER + 1].iter());
        let len = buf.len();
        buf[len - 5] = 200;
        assert!(names(&buf).count() <= 2);
        assert_eq!(names(&buf[..7]).count(), 0);
        let huge = {
            let mut e = vec![0u8; EVENT_HEADER];
            e[12..16].copy_from_slice(&u32::MAX.to_ne_bytes());
            e
        };
        assert_eq!(names(&huge).count(), 0);
    }
}
