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

//! Filtering paused, for every app or for some, until a time or until it
//! is resumed.
//!
//! Paused for every app, there are no rules at all; for one, its UID is
//! left out of them. The daemon keeps running either way, and one timer
//! is armed for the nearest end of a pause, only while there is one.
//!
//! The file, a line each: `<until> <who>`, `who` being `*` for every app or
//! a package, and `until` seconds since the epoch, or `-` until resumed
//! (for every app only: a pause of one app always ends).

use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use crate::files;

/// Stands for every app.
const ALL: &str = "*";

/// The longest pause. Longer than this, the app is better removed.
pub const MAX_MINUTES: u64 = 30 * 24 * 60;

/// The file is written by the commands only; anything far larger is not
/// theirs.
pub const MAX_FILE: usize = 64 << 10;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pauses {
    /// Who, and until when; `None` until resumed.
    entries: Vec<(String, Option<u64>)>,
}

impl Pauses {
    /// What the file says, without the pauses over by `now`; and whether
    /// there were any.
    fn parse(text: &str, now: u64) -> (Self, bool) {
        let mut entries: Vec<(String, Option<u64>)> = Vec::new();
        let mut over = false;
        for line in text.lines() {
            let mut f = line.split_ascii_whitespace();
            let (Some(until), Some(who), None) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            let until = match until {
                "-" if who == ALL => None,
                n => match n.parse::<u64>() {
                    Ok(n) => Some(n),
                    Err(_) => continue,
                },
            };
            if who != ALL && crate::apps::Identity::parse(who).is_none() {
                continue;
            }
            if until.is_some_and(|t| t <= now) {
                over = true;
                continue;
            }
            entries.retain(|(w, _)| w != who);
            entries.push((who.to_owned(), until));
        }
        (Self { entries }, over)
    }

    /// The pauses in `file` that are not over.
    pub fn read(file: &Path, now: u64) -> Self {
        Self::try_read(file, now).unwrap_or_default()
    }

    fn try_read(file: &Path, now: u64) -> std::io::Result<Self> {
        let text = files::read_text(file, MAX_FILE)?;
        Ok(Self::parse(&text, now).0)
    }

    /// As [`Pauses::read`], and removes from the file the pauses that are
    /// over, for the daemon at a start and a reload.
    pub fn load(file: &Path, now: u64) -> std::io::Result<Self> {
        // Without the lock they are left in the file, to be removed the
        // next time: a pause written meanwhile is not to be lost for it.
        let lock = files::lock(file);
        let text = files::read_text(file, MAX_FILE)?;
        let (pauses, over) = Self::parse(&text, now);
        if over && lock.is_ok() {
            // What is over is left out of what was read all the same, and
            // is removed from the file the next time.
            let _ = pauses.write(file);
        }
        Ok(pauses)
    }

    /// Changes the pauses in `file`, with nobody else changing them
    /// between the reading and the writing.
    pub fn change(file: &Path, now: u64, change: impl FnOnce(&mut Self)) -> std::io::Result<()> {
        let _lock = files::lock(file)?;
        let mut pauses = Self::try_read(file, now)?;
        change(&mut pauses);
        pauses.write(file)
    }

    fn write(&self, file: &Path) -> std::io::Result<()> {
        if self.entries.is_empty() {
            return match std::fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let text: String = self
            .entries
            .iter()
            .map(|(who, until)| match until {
                Some(t) => format!("{t} {who}\n"),
                None => format!("- {who}\n"),
            })
            .collect();
        files::replace_durable(file, text.as_bytes())
    }

    /// Whether every app is paused, and until when (`None`: until resumed).
    pub fn all(&self) -> Option<Option<u64>> {
        self.until(ALL)
    }

    /// Until when the app `package` is paused, if it is.
    pub fn app(&self, package: &str) -> Option<u64> {
        self.until(package).flatten()
    }

    pub fn is_paused(&self, package: &str) -> bool {
        self.all().is_some() || self.app(package).is_some()
    }

    fn until(&self, who: &str) -> Option<Option<u64>> {
        self.entries.iter().find(|(w, _)| w == who).map(|(_, t)| *t)
    }

    /// Pauses every app, until `until` or until resumed.
    pub fn pause_all(&mut self, until: Option<u64>) {
        self.set(ALL, until);
    }

    pub fn pause_app(&mut self, package: &str, until: u64) {
        self.set(package, Some(until));
    }

    fn set(&mut self, who: &str, until: Option<u64>) {
        self.entries.retain(|(w, _)| w != who);
        self.entries.push((who.to_owned(), until));
    }

    pub fn resume_all(&mut self) {
        self.entries.retain(|(w, _)| w != ALL);
    }

    pub fn resume_app(&mut self, package: &str) {
        self.entries.retain(|(w, _)| w != package);
    }

    /// When the nearest pause ends, if any is to end.
    pub fn next_end(&self) -> Option<u64> {
        self.entries.iter().filter_map(|(_, t)| *t).min()
    }
}

/// Goes off when the nearest pause ends. Unarmed, it is no timer at all.
#[derive(Default)]
pub struct Timer {
    at: Option<u64>,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl Timer {
    /// Has the timer go off at `at`, in seconds since the epoch, or never.
    /// Armed for that time already, it is left as it is.
    pub fn arm(&mut self, at: Option<u64>, now: u64) {
        if at == self.at && at.is_some() == self.sleep.is_some() {
            return;
        }
        self.at = at;
        // Whole seconds from a time cut to the second: it goes off at
        // `at` or just after, never before.
        self.sleep = at.map(|at| {
            let wait = Duration::from_secs(at.saturating_sub(now));
            Box::pin(tokio::time::sleep(wait))
        });
    }

    #[cfg(test)]
    pub fn is_armed(&self) -> bool {
        self.sleep.is_some()
    }

    /// Returns when the timer goes off, and leaves it unarmed. Unarmed, it
    /// never returns.
    pub async fn fired(&mut self) {
        match &mut self.sleep {
            Some(sleep) => {
                sleep.as_mut().await;
                self.sleep = None;
                self.at = None;
            }
            None => std::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_763_538_000;

    #[test]
    fn a_pause_ends_at_its_time_and_one_of_every_app_may_not() {
        let (p, over) = Pauses::parse("- *\n1763541600 com.news\n", NOW);
        assert!(!over);
        assert_eq!(p.all(), Some(None));
        assert_eq!(p.app("com.news"), Some(1_763_541_600));
        assert_eq!(p.app("com.other"), None);
        assert_eq!(p.next_end(), Some(1_763_541_600));
        // Over by then.
        let (later, over) = Pauses::parse("- *\n1763541600 com.news\n", 1_763_541_600);
        assert!(over && later.app("com.news").is_none() && later.all().is_some());
        // An app is never paused for good.
        assert_eq!(Pauses::parse("- com.news\n", NOW).0, Pauses::default());
    }

    #[test]
    fn pauses_that_are_over_are_ignored_and_removed_by_the_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pauses");
        std::fs::write(&file, format!("{} *\n{} com.a\n", NOW - 1, NOW + 60)).unwrap();
        // A command reads them, and leaves the file as it is.
        assert_eq!(Pauses::read(&file, NOW).all(), None);
        assert!(std::fs::read_to_string(&file).unwrap().contains(" *"));
        let p = Pauses::load(&file, NOW).unwrap();
        assert_eq!((p.all(), p.app("com.a")), (None, Some(NOW + 60)));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            format!("{} com.a\n", NOW + 60)
        );
        // None left: no file.
        Pauses::load(&file, NOW + 60).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn a_pause_survives_being_written_and_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pauses");
        let mut p = Pauses::default();
        p.pause_all(None);
        p.pause_app("com.a", NOW + 60);
        p.pause_app("com.a", NOW + 120);
        p.write(&file).unwrap();
        let read = Pauses::read(&file, NOW);
        assert_eq!(read, p);
        assert_eq!(read.app("com.a"), Some(NOW + 120));
        let mut p = read;
        p.resume_all();
        p.resume_app("com.a");
        assert_eq!(p.next_end(), None);
    }

    #[test]
    fn changing_pauses_leaves_unreadable_choices_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pauses");
        for contents in [vec![0xff], vec![b' '; MAX_FILE + 1]] {
            std::fs::write(&file, &contents).unwrap();
            assert!(Pauses::change(&file, NOW, |p| p.pause_all(None)).is_err());
            assert_eq!(std::fs::read(&file).unwrap(), contents);
            assert!(Pauses::change(&file, NOW, Pauses::resume_all).is_err());
            assert_eq!(std::fs::read(&file).unwrap(), contents);
            assert!(!file.with_extension("new").exists());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn no_timer_runs_while_nothing_is_paused_to_end() {
        let mut timer = Timer::default();
        let mut pauses = Pauses::default();
        timer.arm(pauses.next_end(), NOW);
        assert!(!timer.is_armed());
        // Paused until resumed: nothing to end either.
        pauses.pause_all(None);
        timer.arm(pauses.next_end(), NOW);
        assert!(!timer.is_armed());
        let waited = tokio::time::timeout(Duration::from_secs(365 * 86400), timer.fired()).await;
        assert!(waited.is_err(), "an unarmed timer went off");
    }

    #[tokio::test(start_paused = true)]
    async fn the_timer_goes_off_when_the_nearest_pause_ends() {
        let mut timer = Timer::default();
        let mut pauses = Pauses::default();
        pauses.pause_app("com.a", NOW + 600);
        pauses.pause_app("com.b", NOW + 60);
        timer.arm(pauses.next_end(), NOW);
        let started = tokio::time::Instant::now();
        timer.fired().await;
        assert_eq!(started.elapsed(), Duration::from_secs(60));
        assert!(!timer.is_armed());
        // Armed again for the same time: not reset.
        timer.arm(Some(NOW + 600), NOW);
        tokio::time::advance(Duration::from_secs(300)).await;
        timer.arm(Some(NOW + 600), NOW + 300);
        let started = tokio::time::Instant::now();
        timer.fired().await;
        assert_eq!(started.elapsed(), Duration::from_secs(300));
    }
}
