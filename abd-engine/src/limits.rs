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

//! How many connections are carried at a time, in all and for one app.
//!
//! Every connection holds descriptors and a task for as long as an app or
//! a server cares to keep it. Without a bound, one of either could
//! use up the descriptors of the daemon, which would then accept nothing
//! with its rules in place: every filtered app would lose its network.

#![allow(unsafe_code, reason = "getrlimit and setrlimit")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The most connections carried at a time, descriptors allowing.
const MAX: usize = 4096;
/// The descriptors one connection may hold: its two sockets, and the two
/// pipes between them when it is handed on (`relay.rs`).
const EACH: usize = 6;
/// Descriptors kept for everything that is not a connection: the
/// listeners, the lists being read, the log, the tools of the rules.
const SPARE: usize = 128;

pub struct Limits {
    max: usize,
    /// For one app: no more than half, so that no app can leave the
    /// others without any.
    max_each: usize,
    /// The connections carried, by UID. Their sum is at most `max`.
    carried: Mutex<(usize, HashMap<u32, usize>)>,
}

/// A place among the connections carried, given back when dropped.
pub struct Slot {
    limits: Arc<Limits>,
    uid: u32,
}

impl Limits {
    /// For a process that may have `descriptors` open.
    pub fn new(descriptors: u64) -> Arc<Self> {
        let room = usize::try_from(descriptors).unwrap_or(usize::MAX);
        let max = (room.saturating_sub(SPARE) / EACH).clamp(1, MAX);
        Arc::new(Self {
            max,
            max_each: max.div_ceil(2),
            carried: Mutex::default(),
        })
    }

    /// A place for a connection of `uid`, if there is one.
    pub fn take(self: &Arc<Self>, uid: u32) -> Option<Slot> {
        let mut carried = self.carried.lock().unwrap();
        let (all, each) = &mut *carried;
        let own = each.entry(uid).or_insert(0);
        if *all >= self.max || *own >= self.max_each {
            if *own == 0 {
                each.remove(&uid);
            }
            return None;
        }
        *all += 1;
        *own += 1;
        Some(Slot {
            limits: self.clone(),
            uid,
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut carried = self.limits.carried.lock().unwrap();
        let (all, each) = &mut *carried;
        *all -= 1;
        if let Some(own) = each.get_mut(&self.uid) {
            *own -= 1;
            // No entry is kept for an app with no connection: the table
            // is as large as the apps that have one.
            if *own == 0 {
                each.remove(&self.uid);
            }
        }
    }
}

/// Lets the process open as many descriptors as the system allows it at
/// most, and returns how many that is.
pub fn raise_descriptors() -> u64 {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid rlimit for both calls to read or write.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return 1024;
        }
        let wanted = libc::rlimit {
            rlim_cur: limit.rlim_max,
            rlim_max: limit.rlim_max,
        };
        if libc::setrlimit(libc::RLIMIT_NOFILE, &wanted) == 0 {
            return wanted.rlim_cur;
        }
    }
    limit.rlim_cur
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_no_more_than_the_descriptors_allow() {
        let limits = Limits::new((SPARE + 4 * EACH) as u64);
        let slots: Vec<_> = (0..10).filter_map(|uid| limits.take(10000 + uid)).collect();
        assert_eq!(slots.len(), 4);
        drop(slots);
        assert!(limits.take(10000).is_some());
    }

    #[test]
    fn one_app_cannot_take_every_place() {
        let limits = Limits::new((SPARE + 8 * EACH) as u64);
        let greedy: Vec<_> = (0..8).filter_map(|_| limits.take(10001)).collect();
        assert_eq!(greedy.len(), 4);
        // Another app still has room.
        assert!(limits.take(10002).is_some());
    }

    #[test]
    fn a_place_given_back_can_be_taken_again_and_leaves_nothing_behind() {
        let limits = Limits::new(1 << 20);
        for _ in 0..3 {
            let slot = limits.take(10001).unwrap();
            drop(slot);
        }
        let carried = limits.carried.lock().unwrap();
        assert_eq!(carried.0, 0);
        assert!(carried.1.is_empty());
    }

    #[test]
    fn is_of_some_use_however_few_descriptors_there_are() {
        assert!(Limits::new(0).take(10001).is_some());
    }

    #[test]
    fn raising_the_descriptors_returns_what_the_process_now_has() {
        assert!(raise_descriptors() >= 64);
    }
}
