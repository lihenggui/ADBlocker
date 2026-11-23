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

//! Sees to it that the processes of a filtered app trust the authority
//! before any of them is shown a certificate of ours.
//!
//! The stores are mounted in a process when the first connection of its
//! app arrives, whatever becomes of that connection, and before anything
//! is sent on it in either direction. Not when a connection is first to be
//! decrypted: an app reads the roots it trusts once, at the first
//! certificate it is shown, and that may be the certificate of a server
//! whose connection was handed on. An app whose processes cannot be given
//! the mounts is not decrypted.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, OwnedMutexGuard, Semaphore};

use crate::trust::{EXIT_GONE, EXIT_HAD};
use abd_engine::mitm::Trusting;

/// A process, by its number and by when it was started: the number alone
/// is given to another process once this one has ended.
type Process = (i32, i64, i64);

/// Where the system keeps a directory for every process of an app, below
/// one for the app's UID: the control groups it puts them in.
pub const GROUPS: [&str; 3] = ["/sys/fs/cgroup", "/sys/fs/cgroup/apps", "/acct"];

/// How long the mounting in one process may take.
const MOUNT_TIME: Duration = Duration::from_secs(5);
/// How long a connection or a start waits for its app's turn before
/// going on without it: as long as one mounting may take.
const TURN_TIME: Duration = MOUNT_TIME;
const SCAN_TIME: Duration = Duration::from_secs(1);
const MAX_CACHED_UIDS: usize = 1024;

#[derive(Default)]
struct Groups {
    generation: u64,
    members: HashMap<u32, (String, Arc<[String]>)>,
}

struct Mounted {
    inspected_at: Instant,
    processes: HashSet<Process>,
}

pub struct Mounts {
    /// This program, run again to do the mounting (`abd trust mount`).
    program: PathBuf,
    /// The processes that have the mounts, by UID: an entry for each app
    /// that was ever decrypted for, of which there are as many as the
    /// user chose. Changed only by the holder of the app's turn.
    done: std::sync::Mutex<HashMap<u32, Mounted>>,
    /// A turn for each app given the mounts, held while they are given to
    /// more of its processes or taken away, so that two connections of an
    /// app do not both set about it. One app never waits for another's.
    turns: std::sync::Mutex<HashMap<u32, Arc<Mutex<()>>>>,
    /// What was found for each UID when its processes were last gone
    /// through, and what its directory was like then. The directory
    /// changes when a process of the app starts or ends: until it does,
    /// one look at it answers for a connection, without going through the
    /// processes again.
    seen: std::sync::Mutex<HashMap<u32, (State, bool)>>,
    directories: std::sync::Mutex<HashMap<u32, Arc<Path>>>,
    groups: std::sync::Mutex<Groups>,
    /// Until when each app is given time to take its new roots in, and
    /// when a process of it was last given the mounts.
    settling: std::sync::Mutex<HashMap<u32, Instant>>,
    mounted: std::sync::Mutex<HashMap<u32, Instant>>,
    /// The filtered apps, as the package each UID belongs to.
    apps: std::sync::RwLock<Arc<BTreeMap<u32, String>>>,
    /// Told the app, by the name it is filtered under, when a process of it
    /// has been given the mounts as it started ([`Mounts::started`]), before
    /// it read its roots. Not when a process is given them late, at a
    /// connection: it may hold the roots it read before.
    trusting: Box<dyn Fn(&str) + Send + Sync>,
}

/// How long an app is not decrypted after a process of it was given the
/// mounts late: when a connection of it was already there. It may have
/// read its roots before; it is told that they changed, and reads them
/// again, and a certificate of ours that reached it first would be
/// refused. A process given the mounts as it started has read nothing
/// yet, and is not kept waiting.
const SETTLE_TIME: Duration = Duration::from_secs(1);

/// How long after a process was given the mounts its app is not held to
/// what it refuses: see [`Trusting::Newly`].
const NEW_TIME: Duration = Duration::from_secs(10);

/// How often, and how far apart, a starting process is looked at again
/// until it is the app's: the system starts it as root and makes it the
/// app's a moment later, and only then is it in a namespace of its own.
const START_TRIES: u32 = 40;
const START_WAIT: Duration = Duration::from_millis(25);

/// What an app is told when the roots it trusts have changed
/// (`KeyChain.ACTION_TRUST_STORE_CHANGED`).
const TRUST_STORE_CHANGED: &str = "android.security.action.TRUST_STORE_CHANGED";

/// A directory as it was: when it last changed.
type State = (u64, i64, i64);

/// The directory of the processes of `uid`, if the device keeps one.
fn directory(uid: u32) -> Option<(Arc<Path>, State)> {
    GROUPS
        .iter()
        .map(|root| Path::new(root).join(format!("uid_{uid}")))
        .find_map(|dir| state(&dir).map(|stamp| (Arc::from(dir), stamp)))
}

fn state(dir: &Path) -> Option<State> {
    let meta = std::fs::metadata(dir).ok()?;
    meta.is_dir()
        .then_some((meta.ino(), meta.mtime(), meta.mtime_nsec()))
}

/// The processes with a directory in `dir`, which is the directory of
/// their app's UID.
fn processes_in(dir: &Path) -> Option<Vec<Process>> {
    let entries = std::fs::read_dir(dir).ok()?;
    Some(
        entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let pid = name.to_str()?.strip_prefix("pid_")?.parse().ok()?;
                // Made when the process was: another with the same number
                // has another directory.
                let meta = entry.metadata().ok()?;
                Some((pid, meta.ctime(), meta.ctime_nsec()))
            })
            .collect(),
    )
}

/// The processes of `uid`, from the list of all there are. For devices
/// that keep no directory per app.
fn processes_by_owner(uid: u32) -> Vec<Process> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse().ok()?;
            let meta = entry.metadata().ok()?;
            (meta.uid() == uid).then(|| (pid, meta.ctime(), meta.ctime_nsec()))
        })
        .collect()
}

fn processes(uid: u32) -> Vec<Process> {
    GROUPS
        .iter()
        .find_map(|root| processes_in(&Path::new(root).join(format!("uid_{uid}"))))
        .unwrap_or_else(|| processes_by_owner(uid))
}

async fn scan<T: Send + 'static>(read: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    static SCANNERS: Semaphore = Semaphore::const_new(2);
    tokio::time::timeout(SCAN_TIME, async {
        let slot = SCANNERS.acquire().await.ok()?;
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            read()
        })
        .await
        .ok()
    })
    .await
    .ok()?
}

impl Mounts {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            program: std::env::current_exe()?,
            done: std::sync::Mutex::default(),
            turns: std::sync::Mutex::default(),
            seen: std::sync::Mutex::default(),
            directories: std::sync::Mutex::default(),
            groups: std::sync::Mutex::default(),
            settling: std::sync::Mutex::default(),
            mounted: std::sync::Mutex::default(),
            apps: std::sync::RwLock::default(),
            trusting: Box::new(|_| {}),
        })
    }

    /// Has `trusting` told of each process given the mounts as it started.
    #[must_use]
    pub fn telling(mut self, trusting: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.trusting = Box::new(trusting);
        self
    }

    fn directory(&self, uid: u32) -> Option<(Arc<Path>, State)> {
        let cached = self.directories.lock().unwrap().get(&uid).cloned();
        if let Some(dir) = cached
            && let Some(stamp) = state(&dir)
        {
            return Some((dir, stamp));
        }
        let resolved = directory(uid);
        let mut cache = self.directories.lock().unwrap();
        cache.remove(&uid);
        if let Some((dir, _)) = &resolved
            && cache.len() < MAX_CACHED_UIDS
        {
            cache.insert(uid, dir.clone());
        }
        resolved
    }

    /// Whether every process of `uid` has the stores mounted, having
    /// mounted them where they were not.
    pub async fn ensure(&self, uid: u32) -> Trusting {
        if !self.is_given(uid)
            || !self.all_mounted(uid).await
            || !self.is_given(uid)
            || self.is_settling(uid)
        {
            return Trusting::No;
        }
        let newly = self
            .mounted
            .lock()
            .unwrap()
            .get(&uid)
            .is_some_and(|at| at.elapsed() < NEW_TIME);
        if newly {
            Trusting::Newly
        } else {
            Trusting::Yes
        }
    }

    async fn all_mounted(&self, uid: u32) -> bool {
        // Read before the processes are gone through: one that starts
        // meanwhile changes the directory, and is found by the next look.
        let directory = self.directory(uid);
        let before = directory.as_ref().map(|(_, state)| *state);
        if let (Some(now), Some((then, trusted))) =
            (before, self.seen.lock().unwrap().get(&uid).copied())
            && now == then
        {
            return trusted;
        }
        let generation = self.groups.lock().unwrap().generation;
        let Some((trusted, mounted)) = self.go_through(uid, directory.map(|(dir, _)| dir)).await
        else {
            return false;
        };
        if mounted {
            // Late: a connection of the app is here already.
            self.settling
                .lock()
                .unwrap()
                .insert(uid, Instant::now() + SETTLE_TIME);
            self.tell(uid).await;
        }
        let groups = self.groups.lock().unwrap();
        if groups.generation != generation {
            return false;
        }
        let mut seen = self.seen.lock().unwrap();
        match before {
            Some(state) => {
                seen.insert(uid, (state, trusted));
            }
            // No directory to look at: gone through every time.
            None => {
                seen.remove(&uid);
            }
        }
        trusted
    }

    /// A process of `uid` has just started: gives it the mounts as soon as
    /// it is the app's, before it has shown itself on the network.
    pub async fn started(&self, uid: u32) {
        for _ in 0..START_TRIES {
            if !self.is_given(uid) {
                return;
            }
            if let Some((trusted, mounted)) = self
                .go_through(uid, self.directory(uid).map(|(dir, _)| dir))
                .await
            {
                if mounted {
                    // Before the app is told, which takes a while, so that its
                    // first connection finds what is given to a new process.
                    if let Some(name) = self.apps.read().unwrap().get(&uid).cloned() {
                        (self.trusting)(&name);
                    }
                    self.tell(uid).await;
                }
                if trusted {
                    // What a connection goes by is found out again, by it.
                    self.seen.lock().unwrap().remove(&uid);
                    return;
                }
            }
            tokio::time::sleep(START_WAIT).await;
        }
    }

    /// Has the filtered apps be these: those to be decrypted, and no
    /// others. An app whose hosts only are judged is never asked to trust
    /// us.
    pub fn set_apps(&self, apps: Arc<BTreeMap<u32, String>>) {
        let mut current = self.apps.write().unwrap();
        if *current != apps {
            self.invalidate_groups();
            *current = apps;
        }
    }

    pub fn invalidate_groups(&self) {
        let mut groups = self.groups.lock().unwrap();
        groups.generation = groups.generation.wrapping_add(1);
        groups.members.clear();
        self.seen.lock().unwrap().clear();
    }

    async fn group_members(&self, uid: u32, group: String) -> Option<(u64, Arc<[String]>)> {
        let generation = {
            let groups = self.groups.lock().unwrap();
            if let Some((name, members)) = groups.members.get(&uid)
                && name == &group
            {
                return Some((groups.generation, members.clone()));
            }
            groups.generation
        };
        let (group, members) = scan(move || {
            let members = crate::android::personal_group_members(uid, &group)?;
            Some((group, members))
        })
        .await??;
        let members: Arc<[String]> = Arc::from(members);
        let mut groups = self.groups.lock().unwrap();
        if groups.generation != generation {
            return None;
        }
        if groups.members.len() < MAX_CACHED_UIDS {
            groups.members.insert(uid, (group, members.clone()));
        }
        Some((generation, members))
    }

    pub fn has_groups(&self) -> bool {
        self.apps
            .read()
            .unwrap()
            .values()
            .any(|name| name.starts_with('@'))
    }

    /// Revokes mounting when the UID no longer belongs to the chosen identity.
    pub fn retain_apps(&self, mut keep: impl FnMut(u32, &str) -> bool) -> bool {
        let mut apps = self.apps.write().unwrap();
        let before = apps.len();
        Arc::make_mut(&mut apps).retain(|&uid, package| keep(uid, package));
        apps.len() != before
    }

    /// Takes the mounts away from the processes of the apps that are no
    /// longer among those to be decrypted: one set to have its hosts
    /// judged only, or no longer filtered, is to see nothing of the
    /// module, and not only from its next start on.
    ///
    /// Called after `set_apps`. An app that is among them again by the
    /// time this runs keeps what it has.
    pub async fn release(&self) {
        // Those being given the mounts now hold a turn, and may have no
        // entry yet: they are waited for, then taken from.
        let apps = self.apps.read().unwrap().clone();
        let mut dropped: BTreeSet<u32> = self.done.lock().unwrap().keys().copied().collect();
        dropped.extend(self.turns.lock().unwrap().keys());
        dropped.retain(|uid| !apps.contains_key(uid));
        for uid in dropped {
            // Waited for without a bound of its own: whoever holds the
            // turn is held to `MOUNT_TIME` for each process, and an app no
            // longer given must not keep the mounts until another release.
            let turn = self.turns.lock().unwrap().entry(uid).or_default().clone();
            let _turn = turn.lock_owned().await;
            if self.is_given(uid) {
                continue;
            }
            let Some(mounted) = self.done.lock().unwrap().remove(&uid) else {
                continue;
            };
            self.seen.lock().unwrap().remove(&uid);
            self.settling.lock().unwrap().remove(&uid);
            self.mounted.lock().unwrap().remove(&uid);
            for (pid, ..) in mounted.processes {
                // One that could not be taken away stays until the
                // process ends, as all of them did before.
                self.run("unmount", pid, uid).await;
            }
        }
        self.forget_turns();
    }

    /// Waits for the turn of `uid`, for at most [`TURN_TIME`].
    async fn turn(&self, uid: u32) -> Option<OwnedMutexGuard<()>> {
        let turn = self.turns.lock().unwrap().entry(uid).or_default().clone();
        tokio::time::timeout(TURN_TIME, turn.lock_owned())
            .await
            .ok()
    }

    /// Keeps the turns of the apps given the mounts and of those somebody
    /// holds or waits for: no more than there are apps and connections.
    fn forget_turns(&self) {
        let apps = self.apps.read().unwrap().clone();
        self.turns
            .lock()
            .unwrap()
            .retain(|uid, turn| apps.contains_key(uid) || Arc::strong_count(turn) > 1);
    }

    fn is_given(&self, uid: u32) -> bool {
        self.apps.read().unwrap().contains_key(&uid)
    }

    fn is_settling(&self, uid: u32) -> bool {
        let mut settling = self.settling.lock().unwrap();
        match settling.get(&uid) {
            Some(until) if Instant::now() < *until => true,
            Some(_) => {
                settling.remove(&uid);
                false
            }
            None => false,
        }
    }

    /// Tells the app of `uid` that its roots changed.
    async fn tell(&self, uid: u32) {
        self.mounted.lock().unwrap().insert(uid, Instant::now());
        let package = self.apps.read().unwrap().get(&uid).cloned();
        let Some(package) = package else { return };
        let packages: Arc<[_]> = if package.starts_with('@') {
            let Some((_, members)) = self.group_members(uid, package).await else {
                return;
            };
            members
        } else {
            Arc::from([package])
        };
        let _ = tokio::time::timeout(MOUNT_TIME, async {
            for package in packages.iter() {
                if !self.is_given(uid) {
                    break;
                }
                let mut command = tokio::process::Command::new("/system/bin/cmd");
                command
                    .args([
                        "activity",
                        "broadcast",
                        "--user",
                        "0",
                        "-a",
                        TRUST_STORE_CHANGED,
                        "-p",
                        package,
                    ])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true);
                // Apps that retain earlier roots will be found pinned if
                // they refuse the new authority.
                let _ = command.status().await;
            }
        })
        .await;
    }

    /// Goes through the processes of `uid`. Returns whether all have the
    /// mounts, and whether any was given them just now.
    async fn go_through(&self, uid: u32, directory: Option<Arc<Path>>) -> Option<(bool, bool)> {
        let group = self
            .apps
            .read()
            .unwrap()
            .get(&uid)
            .filter(|name| name.starts_with('@'))
            .cloned();
        let group_generation = if let Some(group) = group {
            let (generation, _) = self.group_members(uid, group).await?;
            Some(generation)
        } else {
            None
        };
        let (inspected_at, now) = scan(move || {
            let inspected_at = Instant::now();
            let processes = directory
                .as_deref()
                .and_then(processes_in)
                .unwrap_or_else(|| processes(uid));
            (inspected_at, processes)
        })
        .await?;
        // Only the holder of the turn changes the app's entry in `done`.
        let _turn = self.turn(uid).await?;
        let newer = self
            .done
            .lock()
            .unwrap()
            .get(&uid)
            .is_some_and(|last| last.inspected_at > inspected_at);
        if !self.is_given(uid)
            || group_generation
                .is_some_and(|generation| generation != self.groups.lock().unwrap().generation)
            || newer
        {
            return None;
        }
        let had = self
            .done
            .lock()
            .unwrap()
            .remove(&uid)
            .map(|mounted| mounted.processes)
            .unwrap_or_default();
        let (have, all, mounted) = self.give(uid, now, &had).await;
        // No process at all is no app to show a certificate to: one
        // started outside the system's own way of starting apps, whose
        // processes are not where they are looked for.
        let trusted = all && !have.is_empty();
        self.done.lock().unwrap().insert(
            uid,
            Mounted {
                inspected_at,
                processes: have,
            },
        );
        Some((trusted, mounted))
    }

    /// Gives the mounts to each of the processes `now` of `uid` that `had`
    /// not got them. Returns those that have them, whether all do, and
    /// whether any was given them just now.
    async fn give(
        &self,
        uid: u32,
        now: Vec<Process>,
        had: &HashSet<Process>,
    ) -> (HashSet<Process>, bool, bool) {
        let mut have = HashSet::new();
        let (mut all, mut mounted) = (true, false);
        for process in now {
            if had.contains(&process) {
                have.insert(process);
                continue;
            }
            if !self.is_given(uid) {
                all = false;
                continue;
            }
            match self.mount(process.0, uid).await {
                Some(0) => {
                    have.insert(process);
                    mounted = true;
                }
                // Given them by a daemon before this one.
                Some(EXIT_HAD) => {
                    have.insert(process);
                }
                // Ended meanwhile: nobody left to trust anything.
                Some(EXIT_GONE) => {}
                _ => all = false,
            }
        }
        (have, all, mounted)
    }

    /// What `abd trust mount` exited with for process `pid` of `uid`.
    async fn mount(&self, pid: i32, uid: u32) -> Option<i32> {
        self.run("mount", pid, uid).await
    }

    /// What `abd trust <what>` exited with for process `pid` of `uid`:
    /// run as a process of its own, since only one with a single thread
    /// can enter another's mount namespace.
    async fn run(&self, what: &str, pid: i32, uid: u32) -> Option<i32> {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(["trust", what, &pid.to_string(), &uid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let status = tokio::time::timeout(MOUNT_TIME, command.status()).await;
        status.ok()?.ok()?.code()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_uid_directory_does_not_inherit_cached_mount_state() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("uid_10000");
        std::fs::create_dir(&directory).unwrap();
        let before = state(&directory).unwrap();
        std::fs::rename(&directory, root.path().join("previous")).unwrap();
        std::fs::create_dir(&directory).unwrap();
        assert_ne!(state(&directory).unwrap().0, before.0);
    }

    #[test]
    fn invalidating_group_members_also_invalidates_mount_validation() {
        let mounts = Mounts::new().unwrap();
        let uid = 10_000;
        mounts.groups.lock().unwrap().members.insert(
            uid,
            (
                "@example".to_owned(),
                Arc::from(["com.example.app".to_owned()]),
            ),
        );
        mounts.seen.lock().unwrap().insert(uid, ((1, 1, 1), true));
        mounts.invalidate_groups();
        let groups = mounts.groups.lock().unwrap();
        assert!(groups.members.is_empty());
        assert_eq!(groups.generation, 1);
        assert!(mounts.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn the_processes_of_an_app_are_those_with_a_directory_in_its_own() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "pid_100",
            "pid_2345",
            "cgroup.procs",
            "pid_",
            "pid_x",
            "other_7",
        ] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        let mut pids: Vec<i32> = processes_in(dir.path())
            .unwrap()
            .into_iter()
            .map(|p| p.0)
            .collect();
        pids.sort_unstable();
        assert_eq!(pids, [100, 2345]);
        assert!(processes_in(&dir.path().join("missing")).is_none());
    }

    #[test]
    fn a_process_started_again_under_the_same_number_is_another() {
        let dir = tempfile::tempdir().unwrap();
        let pid = dir.path().join("pid_100");
        std::fs::create_dir(&pid).unwrap();
        let first = processes_in(dir.path()).unwrap();
        std::fs::remove_dir(&pid).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::create_dir(&pid).unwrap();
        assert_ne!(first, processes_in(dir.path()).unwrap());
    }

    #[test]
    fn a_directory_is_seen_to_change_when_a_process_starts_or_ends() {
        let dir = tempfile::tempdir().unwrap();
        let before = state(dir.path()).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::create_dir(dir.path().join("pid_7")).unwrap();
        let with = state(dir.path()).unwrap();
        assert_ne!(before, with);
        std::thread::sleep(Duration::from_millis(20));
        std::fs::remove_dir(dir.path().join("pid_7")).unwrap();
        assert_ne!(with, state(dir.path()).unwrap());
    }

    #[test]
    fn this_process_is_among_those_of_its_owner() {
        // SAFETY: cannot fail and touches no memory.
        #[allow(unsafe_code)]
        let me = unsafe { libc::getuid() };
        let mine = processes_by_owner(me);
        assert!(mine.iter().any(|p| p.0 == std::process::id() as i32));
    }

    #[tokio::test]
    async fn an_app_not_given_is_never_mounted_in() {
        let mounts = Mounts::new().unwrap();
        // This test's own processes: there to be gone through, were it given.
        // SAFETY: cannot fail and touches no memory.
        #[allow(unsafe_code)]
        let me = unsafe { libc::getuid() };
        mounts.set_apps(Arc::new(BTreeMap::from([(me + 1, "com.other".to_owned())])));
        assert_eq!(mounts.ensure(me).await, Trusting::No);
        mounts.started(me).await;
        assert!(mounts.done.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_uid_that_loses_its_unique_owner_can_no_longer_receive_mounts() {
        let mounts = Mounts::new().unwrap();
        let uid = 10001;
        mounts.set_apps(Arc::new(BTreeMap::from([(uid, "com.shared".to_owned())])));
        assert!(mounts.retain_apps(|_, _| false));
        assert!(!mounts.is_given(uid));
        assert_eq!(mounts.ensure(uid).await, Trusting::No);
        mounts.started(uid).await;
        assert!(mounts.done.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn one_app_being_given_the_mounts_does_not_hold_up_another() {
        let mounts = Mounts::new().unwrap();
        let _first = mounts.turn(10001).await.unwrap();
        let start = tokio::time::Instant::now();
        assert!(mounts.turn(10002).await.is_some());
        assert_eq!(start.elapsed(), Duration::ZERO);
        // The same app's next waits for no longer than the turn time.
        assert!(mounts.turn(10001).await.is_none());
        assert_eq!(start.elapsed(), TURN_TIME);
    }

    #[tokio::test(start_paused = true)]
    async fn a_connection_waiting_for_its_apps_turn_too_long_is_not_decrypted() {
        let mounts = Mounts::new().unwrap();
        // This test's own processes: there to be gone through.
        // SAFETY: cannot fail and touches no memory.
        #[allow(unsafe_code)]
        let me = unsafe { libc::getuid() };
        mounts.set_apps(Arc::new(BTreeMap::from([(me, "com.example".to_owned())])));
        let _held = mounts.turn(me).await.unwrap();
        let start = tokio::time::Instant::now();
        assert_eq!(mounts.ensure(me).await, Trusting::No);
        assert!(start.elapsed() >= TURN_TIME);
        assert!(mounts.done.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_release_takes_from_an_app_no_longer_given_while_another_holds_its_turn() {
        let mounts = Mounts::new().unwrap();
        mounts.set_apps(Arc::new(BTreeMap::from([(10002, "com.kept".to_owned())])));
        mounts.done.lock().unwrap().insert(
            10001,
            Mounted {
                inspected_at: Instant::now(),
                processes: HashSet::new(),
            },
        );
        let _held = mounts.turn(10002).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), mounts.release())
            .await
            .unwrap();
        assert!(mounts.done.lock().unwrap().is_empty());
        // Only the turns of the apps still given are kept.
        let turns: Vec<u32> = mounts.turns.lock().unwrap().keys().copied().collect();
        assert_eq!(turns, [10002]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_release_waits_out_whoever_holds_the_turn_of_an_app_no_longer_given() {
        let mounts = Arc::new(Mounts::new().unwrap());
        mounts.done.lock().unwrap().insert(
            10001,
            Mounted {
                inspected_at: Instant::now(),
                processes: HashSet::new(),
            },
        );
        let held = mounts.turn(10001).await.unwrap();
        let releasing = tokio::spawn({
            let mounts = mounts.clone();
            async move { mounts.release().await }
        });
        // Longer than a connection would wait: the app is not left with
        // the mounts for a release that may never come.
        tokio::time::sleep(TURN_TIME * 3).await;
        assert!(!releasing.is_finished());
        assert!(!mounts.done.lock().unwrap().is_empty());
        drop(held);
        releasing.await.unwrap();
        assert!(mounts.done.lock().unwrap().is_empty());
        assert!(mounts.turns.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_app_with_no_process_to_be_found_is_not_trusted_to_trust_us() {
        let mounts = Mounts::new().unwrap();
        mounts.set_apps(Arc::new(BTreeMap::from([(
            4_000_000_000,
            "com.none".to_owned(),
        )])));
        // No such UID has a process here.
        assert_eq!(mounts.ensure(4_000_000_000).await, Trusting::No);
    }
}
