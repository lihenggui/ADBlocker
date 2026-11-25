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

/// The most app identities remembered for having shown a page.
pub(crate) const MAX_APPS_WITH_PAGES: usize = 4096;

/// Distinct wildcard and regex app entries kept in one list generation.
pub(crate) const MAX_APP_PATTERNS: usize = 128;
/// The longest quoted app expression, before compiling it.
pub(crate) const MAX_APP_EXPRESSION: usize = 4 << 10;
/// The heap limit for each app expression's compiled NFA and lazy DFA cache.
pub(crate) const MAX_APP_REGEX: usize = 64 << 10;
/// Packages whose pattern matches are remembered in one list generation.
pub(crate) const MAX_APP_PATTERN_PACKAGES: usize = 4096;

/// Address rules kept per list generation. Later rules over the limit are skipped.
pub(crate) const MAX_ADDRESS_RULES: usize = 4096;
/// Address expression source bytes. Longer expressions are invalid.
pub(crate) const MAX_ADDRESS_EXPRESSION: usize = MAX_APP_EXPRESSION;
/// Heap for an individual address expression. Larger expressions are invalid.
pub(crate) const MAX_ADDRESS_REGEX: usize = MAX_APP_REGEX;
/// Compiled address set heap. Later expressions that would exceed it are skipped.
pub(crate) const MAX_ADDRESS_REGEX_SET: usize = 32 << 20;

/// The longest JavaScript rule body, in bytes. Longer bodies are skipped.
pub(crate) const MAX_JS_RULE: usize = 16 << 10;
/// Distinct JavaScript bodies kept in one generation. Rules needing another are skipped.
pub(crate) const MAX_JS_SCRIPTS: usize = 4096;
/// Bytes of distinct JavaScript bodies kept in one generation. Rules that would exceed it are skipped.
pub(crate) const MAX_JS_TABLE_BYTES: usize = 4 << 20;

/// The most response rules kept from a generation of lists. The compiled
/// expressions share `MAX_RESPONSE_REGEX_BYTES` and each NFA fits `MAX_RESPONSE_REGEX`.
pub(crate) const MAX_RESPONSE_RULES: usize = 1024;
/// The combined compiled expression heap allowed in one generation.
pub(crate) const MAX_RESPONSE_REGEX_BYTES: usize = 32 << 20;
/// The largest response rule retained or compiled.
pub(crate) const MAX_RESPONSE_RULE: usize = 8 << 10;
/// Longest URL accepted by a request transformation, before and after editing.
pub(crate) const MAX_TRANSFORM_URL: usize = 8 << 10;
/// The longest `$jsonprune` expression, unescaped. A rule with a longer
/// one is invalid.
pub(crate) const MAX_JSON_PATH: usize = 4 << 10;
/// The most paths nested in the filters of one expression. A rule with
/// more is invalid.
pub(crate) const MAX_JSON_PATH_NESTING: usize = 8;
/// How deep below the root a document is walked. What lies deeper is left
/// as it is.
pub(crate) const MAX_JSON_DEPTH: usize = 64;
/// The most items one expression may visit in one document. Past it the
/// response is passed on unchanged.
pub(crate) const MAX_JSON_VISITS: usize = 250_000;
/// The most heap used by one response NFA. Counted Unicode repetitions
/// expand into UTF-8 transitions for each repetition.
pub(crate) const MAX_RESPONSE_REGEX: usize = 512 << 10;
/// Source size, selector depth, and generation limits for HTML filtering.
pub(crate) const MAX_HTML_RULE: usize = 32 << 10;
pub(crate) const MAX_HTML_CHAIN: usize = 32;
pub(crate) const MAX_HTML_CONDITIONS: usize = 32;
pub(crate) const MAX_HTML_RULES: usize = 4096;
pub(crate) const MAX_HTML_REGEXES: usize = 512;
pub(crate) const MAX_HTML_RULES_PER_PAGE: usize = 128;
/// A deeper subtree is skipped. Exhausting either work limit abandons the rewrite.
pub(crate) const MAX_HTML_DEPTH: usize = 128;
pub(crate) const MAX_HTML_ELEMENTS: usize = 100_000;
pub(crate) const MAX_HTML_DECODED: usize = 4 << 20;
/// The largest original response that can be rewritten on a phone.
pub(crate) const MAX_REWRITE_BODY: usize = 2 << 20;
/// The most lines in a playlist that can be edited, including its header.
pub(crate) const MAX_PLAYLIST_LINES: usize = 32_768;
/// The most bytes a replacement may produce.
pub(crate) const MAX_REWRITE_RESULT: usize = 2 * MAX_REWRITE_BODY;
/// The most time an upstream body can be held for rewriting.
pub(crate) const REWRITE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
/// The time allowed for a body to wait for replacement work.
pub(crate) const REWRITE_WORK: std::time::Duration = std::time::Duration::from_secs(1);
/// The most responses held for rewriting, and the most blocking jobs alive.
pub(crate) const MAX_REWRITES: usize = 4;

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
