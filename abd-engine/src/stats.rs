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

//! Counts of what was done, for the user to see that it works: in all since
//! counting began, and by app and by hour for the last days. Counts only:
//! what hosts and addresses an app talks to is never written anywhere.
//!
//! The file, a line each: `since <seconds>`, `<counter> <n>` for the
//! totals, and `<hour> <package> <n> <n> <n> <n> <n>` for an app's counts
//! in an hour, in the order of [`HOURLY`]. An hour is seconds since the
//! epoch divided by 3600, in UTC. The stats command adds hours up into
//! days using the app's local day boundaries.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{
    AtomicBool, AtomicU64,
    Ordering::{Acquire, Relaxed, Release},
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use crate::clock;

/// What is counted. The first five are counted by app and hour as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Counter {
    /// Requests in decrypted connections that were blocked.
    RequestsBlocked,
    /// Connections refused because every request to the host is blocked.
    HostsBlocked,
    /// Requests in decrypted connections that were let through.
    RequestsAllowed,
    /// Connections handed on untouched.
    Passed,
    /// Connections decrypted.
    Decrypted,
    /// Hosts found pinned by an app.
    Pinned,
    /// Connections refused: not come by the rules, of no known app, or
    /// more than can be carried.
    Refused,
}

/// The names of the counters, in the order of [`Counter`].
pub const NAMES: [&str; 7] = [
    "requests_blocked",
    "hosts_blocked",
    "requests_allowed",
    "passed",
    "decrypted",
    "pinned",
    "refused",
];

/// The counters kept by app and hour.
pub const HOURLY: usize = 5;

/// The hours kept: 8 days, this one included.
pub const KEEP_HOURS: u32 = 8 * 24;

/// The most apps counted by hour. Only the apps the user chose are counted
/// at all, and an app cannot add another: the table grows with the user's
/// choices of the last [`KEEP_HOURS`], a few dozen apps at the very most.
/// At this bound, with every app busy every hour, it is under 2 MiB in
/// memory and a file of about as much. Beyond it an app is counted in the
/// totals only, until hours of others have aged out: no app's counts are
/// pushed out by another's.
const MAX_APPS: usize = 128;

/// Counts of one app, by hour.
type Hours = BTreeMap<u32, [u64; HOURLY]>;

/// What the file holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub since: u64,
    /// In the order of [`Counter`].
    pub totals: [u64; 7],
    pub apps: HashMap<String, Hours>,
}

impl Snapshot {
    /// What `file` holds, without the hours too old to keep. Missing, it
    /// is counting that begins now.
    pub fn read(file: &Path, now: u64) -> io::Result<Self> {
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let mut s = Self::parse(&text);
        if s.since == 0 {
            s.since = now;
        }
        s.prune(now);
        Ok(s)
    }

    fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let mut fields = line.split_ascii_whitespace();
            let (Some(key), Some(value)) = (fields.next(), fields.next()) else {
                continue;
            };
            let Some(requests) = fields.next() else {
                if key == "since" {
                    s.since = value.parse().unwrap_or(0);
                } else if let (Some(i), Ok(n)) =
                    (NAMES.iter().position(|&name| name == key), value.parse())
                {
                    s.totals[i] = n;
                }
                continue;
            };
            let Ok(hour) = key.parse() else { continue };
            let (Some(hosts), Some(allowed), Some(passed), Some(decrypted), None) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                continue;
            };
            let counts =
                [requests, hosts, allowed, passed, decrypted].map(|n| n.parse().unwrap_or(0));
            if let Some(hours) = s.apps.get_mut(value) {
                hours.insert(hour, counts);
            } else if s.apps.len() < MAX_APPS && crate::apps::Identity::parse(value).is_some() {
                s.apps
                    .insert(value.to_owned(), Hours::from([(hour, counts)]));
            }
        }
        s
    }

    fn prune(&mut self, now: u64) {
        prune(&mut self.apps, now);
    }

    fn text(&self) -> String {
        use std::fmt::Write as _;
        let mut text = format!("since {}\n", self.since);
        for (name, n) in NAMES.iter().zip(self.totals) {
            let _ = writeln!(text, "{name} {n}");
        }
        let mut apps: Vec<_> = self.apps.iter().collect();
        apps.sort_unstable_by_key(|(package, _)| *package);
        for (package, hours) in apps {
            for (hour, [requests, hosts, allowed, passed, decrypted]) in hours {
                let _ = writeln!(
                    text,
                    "{hour} {package} {requests} {hosts} {allowed} {passed} {decrypted}"
                );
            }
        }
        text
    }
}

/// Forgets the hours older than [`KEEP_HOURS`], and those ahead of the
/// clock: it was set back.
fn prune(apps: &mut HashMap<String, Hours>, now: u64) {
    let hour = hour_of(now);
    let oldest = hour.saturating_sub(KEEP_HOURS - 1);
    for hours in apps.values_mut() {
        hours.retain(|h, _| (oldest..=hour.saturating_add(1)).contains(h));
    }
    apps.retain(|_, hours| !hours.is_empty());
}

fn hour_of(secs: u64) -> u32 {
    u32::try_from(secs / 3600).unwrap_or(u32::MAX)
}

pub struct Stats {
    /// The totals, in the order of [`Counter`].
    totals: [AtomicU64; 7],
    since: u64,
    apps: Mutex<HashMap<String, Hours>>,
    /// A runtime without a verified starting point must leave the saved
    /// totals for a subsequent run to recover.
    load_error: Option<io::ErrorKind>,
    changed: Notify,
    /// Whether something was counted since the counts were last written:
    /// the writer is woken for the first count of many, not for each.
    unwritten: AtomicBool,
    /// Held from before the snapshot to the file replacement, so the
    /// final snapshot follows any write already in progress.
    writing_finished: Mutex<bool>,
}

impl Default for Stats {
    fn default() -> Self {
        Self::from(Snapshot {
            since: clock::now(),
            ..Snapshot::default()
        })
    }
}

/// How long counts may sit in memory before they are written.
const WRITE_DELAY: Duration = Duration::from_secs(30);
const MAX_WRITE_DELAY: Duration = Duration::from_secs(3600);

fn next_write_delay(delay: Duration, result: io::Result<()>) -> Duration {
    match result {
        Ok(()) => WRITE_DELAY,
        Err(error) => {
            if delay == WRITE_DELAY {
                eprintln!("stats: could not save counts; will retry with backoff: {error}");
            }
            (delay * 2).min(MAX_WRITE_DELAY)
        }
    }
}

impl Stats {
    /// Goes on from what `file` holds.
    pub fn open(file: &Path) -> Self {
        match Snapshot::read(file, clock::now()) {
            Ok(saved) => Self::from(saved),
            Err(error) => {
                eprintln!("stats: could not read the saved snapshot: {error}");
                Self {
                    load_error: Some(error.kind()),
                    ..Self::default()
                }
            }
        }
    }

    fn from(s: Snapshot) -> Self {
        Self {
            totals: s.totals.map(AtomicU64::new),
            since: s.since,
            apps: Mutex::new(s.apps),
            load_error: None,
            changed: Notify::new(),
            unwritten: AtomicBool::new(false),
            writing_finished: Mutex::new(false),
        }
    }

    /// Counts one of `what`, for the filtered app `package` if it is one.
    pub fn count(&self, package: Option<&str>, what: Counter) {
        self.count_at(package, what, clock::now());
    }

    fn count_at(&self, package: Option<&str>, what: Counter, now: u64) {
        let i = what as usize;
        self.totals[i].fetch_add(1, Relaxed);
        if let Some(package) = package.filter(|_| i < HOURLY) {
            let mut apps = self.apps.lock().unwrap();
            if !apps.contains_key(package) && apps.len() < MAX_APPS {
                apps.insert(package.to_owned(), Hours::new());
            }
            if let Some(hours) = apps.get_mut(package) {
                hours.entry(hour_of(now)).or_default()[i] += 1;
                // Hours age out as they are written; a clock that jumps
                // about between two writes adds no more than this.
                while hours.len() > KEEP_HOURS as usize + 2 {
                    hours.pop_first();
                }
            }
        }
        if !self.unwritten.swap(true, Release) {
            self.changed.notify_one();
        }
    }

    /// The total of `what` since counting began.
    #[cfg(test)]
    pub fn total(&self, what: Counter) -> u64 {
        self.totals[what as usize].load(Relaxed)
    }

    fn snapshot(&self, now: u64) -> Snapshot {
        let mut apps = self.apps.lock().unwrap();
        prune(&mut apps, now);
        Snapshot {
            since: self.since,
            totals: std::array::from_fn(|i| self.totals[i].load(Relaxed)),
            apps: apps.clone(),
        }
    }

    fn write(&self, file: &Path) -> io::Result<()> {
        if let Some(kind) = self.load_error {
            return Err(io::Error::new(
                kind,
                "the saved statistics snapshot was unreadable at startup",
            ));
        }
        // Take all counts published so far. A count during the write
        // can wake the writer for the next one.
        self.unwritten.swap(false, Acquire);
        let text = self.snapshot(clock::now()).text();
        let written = crate::files::replace(file, text.as_bytes());
        if written.is_err() {
            // Retry with the next batch even if traffic has stopped.
            self.unwritten.store(true, Release);
            self.changed.notify_one();
        }
        written
    }

    fn write_if_running(&self, file: &Path) -> io::Result<bool> {
        let finished = self.writing_finished.lock().unwrap();
        if *finished {
            return Ok(false);
        }
        self.write(file)?;
        Ok(true)
    }

    /// Writes the final snapshot after any periodic write in progress.
    pub fn finish_writing(&self, file: &Path) -> io::Result<()> {
        let mut finished = self.writing_finished.lock().unwrap();
        *finished = true;
        self.write(file)
    }

    /// Writes the counts some time after they change. While nothing is
    /// counted nothing is written and no timer runs.
    pub fn keep_written(self: Arc<Self>, file: PathBuf) {
        if self.load_error.is_none() {
            tokio::spawn(self.write_periodically(file));
        }
    }

    async fn write_periodically(self: Arc<Self>, file: PathBuf) {
        let file: Arc<Path> = file.into();
        let mut delay = WRITE_DELAY;
        loop {
            self.changed.notified().await;
            tokio::time::sleep(delay).await;
            let (stats, file) = (self.clone(), file.clone());
            match tokio::task::spawn_blocking(move || stats.write_if_running(&file)).await {
                Ok(Ok(true)) => delay = next_write_delay(delay, Ok(())),
                Ok(Err(error)) => delay = next_write_delay(delay, Err(error)),
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                _ => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 489_872 * 3600 + 100;

    #[test]
    fn pruning_keeps_the_last_hour_without_overflow() {
        let hour = u32::MAX;
        let oldest = hour - KEEP_HOURS + 1;
        let mut apps = HashMap::from([(
            "com.a".to_owned(),
            Hours::from([
                (hour, [1; HOURLY]),
                (oldest, [2; HOURLY]),
                (oldest - 1, [3; HOURLY]),
            ]),
        )]);
        prune(&mut apps, u64::from(hour) * 3600);
        assert_eq!(
            apps["com.a"].keys().copied().collect::<Vec<_>>(),
            [oldest, hour]
        );
    }

    #[test]
    fn repeated_write_errors_back_off_until_a_write_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("missing");
        let file = parent.join("stats");
        let stats = Stats::default();
        stats.count(None, Counter::Refused);
        let mut delay = WRITE_DELAY;
        for seconds in [60, 120, 240, 480, 960, 1920, 3600, 3600] {
            let result = stats.write(&file);
            assert!(result.is_err());
            assert!(stats.unwritten.load(Acquire));
            delay = next_write_delay(delay, result);
            assert_eq!(delay, Duration::from_secs(seconds));
        }
        std::fs::create_dir(&parent).unwrap();
        assert_eq!(next_write_delay(delay, stats.write(&file)), WRITE_DELAY);
        assert!(!stats.unwritten.load(Acquire));
        assert_eq!(
            Snapshot::read(&file, clock::now()).unwrap().totals[Counter::Refused as usize],
            1
        );
    }

    #[test]
    fn counts_go_to_the_app_and_the_hour_they_happened_in() {
        let stats = Stats::default();
        stats.count_at(Some("com.a"), Counter::RequestsBlocked, NOW - 3600);
        stats.count_at(Some("com.a"), Counter::RequestsBlocked, NOW);
        stats.count_at(Some("com.a"), Counter::Decrypted, NOW);
        stats.count_at(Some("com.b"), Counter::Passed, NOW);
        // Of no app, or not counted by hour: in the totals only.
        stats.count_at(None, Counter::Refused, NOW);
        stats.count_at(Some("com.b"), Counter::Pinned, NOW);
        let s = stats.snapshot(NOW);
        assert_eq!(
            s.apps,
            HashMap::from([
                (
                    "com.a".to_owned(),
                    Hours::from([(489_871, [1, 0, 0, 0, 0]), (489_872, [1, 0, 0, 0, 1])]),
                ),
                (
                    "com.b".to_owned(),
                    Hours::from([(489_872, [0, 0, 0, 1, 0])])
                ),
            ])
        );
        assert_eq!(s.totals, [2, 0, 0, 1, 1, 1, 1]);
    }

    #[test]
    fn eight_days_of_hours_are_kept_and_no_more() {
        let stats = Stats::default();
        let day = 24 * 3600;
        stats.count_at(Some("com.a"), Counter::Passed, NOW - 8 * day);
        stats.count_at(Some("com.a"), Counter::Passed, NOW - 8 * day + 3600);
        stats.count_at(Some("com.old"), Counter::Passed, NOW - 9 * day);
        let s = stats.snapshot(NOW);
        assert_eq!(
            s.apps["com.a"],
            Hours::from([(489_872 - 191, [0, 0, 0, 1, 0])])
        );
        // An app with no hours left frees its place; its totals stay.
        assert!(!s.apps.contains_key("com.old"));
        assert_eq!(s.totals[Counter::Passed as usize], 3);
    }

    #[test]
    fn apps_beyond_the_bound_are_counted_in_the_totals_and_push_out_no_other() {
        let stats = Stats::default();
        for n in 0..MAX_APPS + 10 {
            stats.count_at(Some(&format!("com.app{n}")), Counter::Passed, NOW);
        }
        stats.count_at(Some("com.app0"), Counter::Passed, NOW);
        let s = stats.snapshot(NOW);
        assert_eq!(s.apps.len(), MAX_APPS);
        assert_eq!(s.apps["com.app0"][&489_872][Counter::Passed as usize], 2);
        assert!(!s.apps.contains_key(&format!("com.app{MAX_APPS}")));
        assert_eq!(s.totals[Counter::Passed as usize], MAX_APPS as u64 + 11);
    }

    #[test]
    fn one_app_holds_no_more_hours_than_are_kept_whatever_the_clock_does() {
        let stats = Stats::default();
        for h in 0..1000 {
            stats.count_at(Some("com.a"), Counter::Passed, h * 3600);
        }
        assert!(stats.apps.lock().unwrap()["com.a"].len() <= KEEP_HOURS as usize + 2);
    }

    #[test]
    fn the_next_daemon_goes_on_from_what_was_written() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stats");
        let stats = Stats::default();
        let now = clock::now();
        stats.count_at(Some("com.a"), Counter::HostsBlocked, now);
        stats.count_at(None, Counter::Refused, now);
        stats.finish_writing(&file).unwrap();
        let next = Stats::open(&file);
        assert_eq!(next.since, stats.since);
        assert_eq!(next.total(Counter::HostsBlocked), 1);
        assert_eq!(next.total(Counter::Refused), 1);
        next.count_at(Some("com.a"), Counter::HostsBlocked, now);
        let s = next.snapshot(now);
        assert_eq!(
            s.apps["com.a"],
            Hours::from([(hour_of(now), [0, 2, 0, 0, 0])])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_stats_write_keeps_counts_dirty_and_queues_a_retry() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stats");
        let blocked = file.with_extension("new");
        let stats = Stats::default();
        stats.count(None, Counter::Passed);
        stats.changed.notified().await;
        std::fs::create_dir(&blocked).unwrap();
        assert!(stats.write(&file).is_err());

        assert!(stats.unwritten.load(Acquire));
        tokio::time::timeout(Duration::ZERO, stats.changed.notified())
            .await
            .unwrap();
        assert!(!file.exists());

        std::fs::remove_dir(blocked).unwrap();
        stats.count(None, Counter::Passed);
        stats.finish_writing(&file).unwrap();
        assert_eq!(
            stats.snapshot(clock::now()).totals[Counter::Passed as usize],
            2
        );
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("passed 2\n")
        );
        assert!(!stats.unwritten.load(Acquire));
    }

    #[test]
    fn a_runtime_does_not_replace_an_unreadable_startup_count_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stats");
        std::fs::write(&file, [0xff]).unwrap();
        let stats = Stats::open(&file);
        stats.count(None, Counter::Passed);
        assert!(stats.finish_writing(&file).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), [0xff]);
    }

    #[test]
    fn the_counts_of_an_older_daemon_are_read_as_totals() {
        let s = Snapshot::parse("hosts_blocked 5\npassed 3\nrefused 1\ngarbage\n");
        assert_eq!(s.totals, [0, 5, 0, 3, 0, 0, 1]);
        assert!(s.apps.is_empty());
    }

    #[test]
    fn hour_first_records_are_read_and_written_in_package_and_hour_order() {
        let snapshot = Snapshot::parse(
            "since 42\npassed 15\n1 com.a 1 2 3 4 5\n1 com.z 5 4 3 2 1\n2 com.a 2 3 4 5 6\n",
        );
        let written = snapshot.text();
        assert_eq!(
            written.lines().skip(NAMES.len() + 1).collect::<Vec<_>>(),
            [
                "1 com.a 1 2 3 4 5",
                "2 com.a 2 3 4 5 6",
                "1 com.z 5 4 3 2 1",
            ]
        );
        assert_eq!(Snapshot::parse(&written), snapshot);
    }

    #[test]
    fn malformed_records_and_duplicate_hours_follow_the_existing_file_format() {
        let snapshot = Snapshot::parse(concat!(
            "since 42\nsince bad\nsince 8 extra\n",
            "passed 15\npassed bad\npassed 3 extra\nunknown 10\n",
            "1 com.app 9 9 9 9 9\n  1\tcom.app 1 bad 3 4 5 \r\n",
            "2 com.app 1 2 3 4\n3 com.app 1 2 3 4 5 6\n",
            "4 invalid/name 1 2 3 4 5\ninvalid-hour com.app 1 2 3 4 5\n",
        ));
        assert_eq!(snapshot.since, 0);
        assert_eq!(snapshot.totals, [0, 0, 0, 15, 0, 0, 0]);
        assert_eq!(
            snapshot.apps,
            HashMap::from([("com.app".to_owned(), Hours::from([(1, [1, 0, 3, 4, 5])]))])
        );
    }

    #[test]
    fn an_existing_package_is_still_read_when_the_app_table_is_full() {
        use std::fmt::Write as _;
        let mut text = String::new();
        for n in 0..MAX_APPS + 1 {
            writeln!(text, "1 com.app{n} 1 2 3 4 5").unwrap();
        }
        text.push_str("2 com.app0 2 3 4 5 6\n");
        let snapshot = Snapshot::parse(&text);
        assert_eq!(snapshot.apps.len(), MAX_APPS);
        assert!(!snapshot.apps.contains_key(&format!("com.app{MAX_APPS}")));
        assert_eq!(snapshot.apps["com.app0"][&2], [2, 3, 4, 5, 6]);
    }

    #[test]
    fn a_periodic_write_queued_at_shutdown_leaves_the_final_snapshot_in_place() {
        use std::future::Future as _;
        use std::task::{Context, Waker};

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let blocking = runtime.spawn_blocking(move || {
            let _ = wait.recv();
        });
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stats");
        let stats = Arc::new(Stats::default());
        let now = clock::now();

        runtime.block_on(async {
            stats.count_at(Some("com.app"), Counter::Passed, now);
            let mut writer = std::pin::pin!(stats.clone().write_periodically(file.clone()));
            let mut context = Context::from_waker(Waker::noop());
            assert!(writer.as_mut().poll(&mut context).is_pending());
            tokio::time::advance(WRITE_DELAY + Duration::from_millis(1)).await;
            assert!(writer.as_mut().poll(&mut context).is_pending());
            assert!(!file.exists());

            stats.count_at(Some("com.app"), Counter::RequestsBlocked, now);
            stats.finish_writing(&file).unwrap();
            let final_snapshot = Snapshot::read(&file, now).unwrap();
            assert_eq!(final_snapshot.totals, [1, 0, 0, 1, 0, 0, 0]);
            assert_eq!(
                final_snapshot.apps["com.app"][&hour_of(now)],
                [1, 0, 0, 1, 0]
            );
            stats.count(None, Counter::Refused);

            release.send(()).unwrap();
            tokio::time::resume();
            tokio::time::timeout(Duration::from_secs(5), async {
                blocking.await.unwrap();
                writer.await;
            })
            .await
            .unwrap();
            assert_eq!(Snapshot::read(&file, now).unwrap(), final_snapshot);
            assert!(!file.with_extension("new").exists());
        });
    }
}
