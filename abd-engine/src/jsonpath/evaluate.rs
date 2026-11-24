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
//! Select against the original tree before removing anything from it.

use std::collections::HashMap;

use serde_json::{Number, Value};

use super::{Filter, Key, Member, Path, Predicate, Selector, Slice, Step};
use crate::limits::{MAX_JSON_DEPTH, MAX_JSON_VISITS};

/// An object member keeps its name, and an array element keeps its original index.
pub(super) enum Selection<'a> {
    Keep,
    Remove,
    Children(HashMap<Slot<'a>, Selection<'a>>),
}

pub(super) struct Aborted;

struct Walk<'a> {
    remaining: usize,
    within_budget: &'a dyn Fn() -> bool,
}

impl Path {
    pub(super) fn select<'a>(
        &self,
        value: &'a Value,
        within_budget: &dyn Fn() -> bool,
    ) -> Result<Selection<'a>, Aborted> {
        if self.0.is_empty() {
            return Ok(Selection::Keep);
        }
        let mut walk = Walk {
            remaining: MAX_JSON_VISITS,
            within_budget,
        };
        walk.visit()?;
        walk.descend(value, 0, &self.0, &[0], None)
    }
}

impl Walk<'_> {
    fn visit(&mut self) -> Result<(), Aborted> {
        self.remaining = self.remaining.checked_sub(1).ok_or(Aborted)?;
        (self.within_budget)().then_some(()).ok_or(Aborted)
    }

    /// Merge equivalent path states at each node. Overlapping recursive
    /// steps can reach a node in many ways but each selector runs there once.
    fn descend<'a>(
        &mut self,
        value: &'a Value,
        depth: usize,
        steps: &[Step],
        active: &[usize],
        find: Option<&Predicate>,
    ) -> Result<Selection<'a>, Aborted> {
        if active.is_empty() || depth >= MAX_JSON_DEPTH {
            return Ok(Selection::Keep);
        }
        let mut next = Vec::new();
        let mut selected = HashMap::new();
        let len = value.as_array().map_or(0, Vec::len) as i64;
        for (slot, child) in children(value) {
            self.visit()?;
            next.clear();
            let mut matched = false;
            for &at in active {
                let step = &steps[at];
                if step.recursive && next.last() != Some(&at) {
                    next.push(at);
                }
                if self.matches(&step.selector, slot, len, child, depth + 1)? {
                    if at + 1 == steps.len() {
                        matched = find.is_none_or(|predicate| predicate.matches(child));
                    } else {
                        next.push(at + 1);
                    }
                }
            }
            let selection = if matched {
                Selection::Remove
            } else {
                self.descend(child, depth + 1, steps, &next, find)?
            };
            match selection {
                Selection::Keep => (),
                Selection::Remove if find.is_some() => return Ok(Selection::Remove),
                selection => {
                    selected.insert(slot, selection);
                }
            }
        }
        if selected.is_empty() {
            Ok(Selection::Keep)
        } else {
            Ok(Selection::Children(selected))
        }
    }

    fn matches(
        &mut self,
        selector: &Selector,
        slot: Slot<'_>,
        len: i64,
        child: &Value,
        depth: usize,
    ) -> Result<bool, Aborted> {
        Ok(match selector {
            Selector::Wildcard => true,
            Selector::Members(members) => members.iter().any(|part| slot.matches(part, len)),
            Selector::Filter(Filter { key, predicate }) => match key {
                Key::Name(name) => child
                    .get(name)
                    .is_some_and(|value| predicate.matches(value)),
                Key::Path(path) => {
                    self.visit()?;
                    if path.0.is_empty() {
                        predicate.matches(child)
                    } else {
                        matches!(
                            self.descend(child, depth, &path.0, &[0], Some(predicate))?,
                            Selection::Remove
                        )
                    }
                }
            },
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Slot<'a> {
    Name(&'a str),
    Index(usize),
}

fn children(value: &Value) -> impl Iterator<Item = (Slot<'_>, &Value)> {
    let object = value.as_object().into_iter().flatten();
    let array = value.as_array().into_iter().flatten().enumerate();
    object
        .map(|(name, child)| (Slot::Name(name), child))
        .chain(array.map(|(at, child)| (Slot::Index(at), child)))
}

impl Slot<'_> {
    fn matches(self, member: &Member, len: i64) -> bool {
        match (self, member) {
            (Self::Name(name), Member::Name(wanted)) => name == wanted,
            (Self::Index(at), Member::Index(wanted)) => at as i64 == from_end(*wanted, len),
            (Self::Index(at), Member::Slice(slice)) => slice.contains(at as i64, len),
            _ => false,
        }
    }
}

fn from_end(bound: i64, len: i64) -> i64 {
    if bound < 0 { bound + len } else { bound }
}

impl Slice {
    /// Look at membership directly so reverse slices cannot shift other selections.
    fn contains(&self, at: i64, len: i64) -> bool {
        let bound = |bound: Option<i64>, default, low, high| {
            bound.map_or(default, |n| from_end(n, len).clamp(low, high))
        };
        match self.stride.cmp(&0) {
            std::cmp::Ordering::Equal => false,
            std::cmp::Ordering::Greater => {
                let start = bound(self.start, 0, 0, len);
                let end = bound(self.end, len, 0, len);
                (start..end).contains(&at) && (at - start) % self.stride == 0
            }
            std::cmp::Ordering::Less => {
                let start = bound(self.start, len - 1, -1, len - 1);
                let end = bound(self.end, -1, -1, len - 1);
                end < at && at <= start && (at - start) % self.stride == 0
            }
        }
    }
}

impl Predicate {
    fn matches(&self, value: &Value) -> bool {
        match self {
            Self::Has => true,
            Self::Equal(wanted) => match (wanted, value) {
                (Value::Number(a), Value::Number(b)) => numbers_equal(a, b),
                _ => wanted == value,
            },
            Self::Substring(wanted) => value.as_str().is_some_and(|text| text.contains(wanted)),
        }
    }
}

fn numbers_equal(a: &Number, b: &Number) -> bool {
    match (a.as_i128(), b.as_i128()) {
        (Some(a), Some(b)) => a == b,
        (Some(integer), None) => matches_integer(b, integer),
        (None, Some(integer)) => matches_integer(a, integer),
        (None, None) => comparable_float(a).is_some_and(|a| comparable_float(b) == Some(a)),
    }
}

fn matches_integer(float: &Number, integer: i128) -> bool {
    comparable_float(float).is_some_and(|f| f.fract() == 0.0 && f as i128 == integer)
}

/// serde_json stores out-of-range integers as floats. In that range the
/// rounded value cannot distinguish adjacent integers, so decline equality.
fn comparable_float(number: &Number) -> Option<f64> {
    number
        .as_f64()
        .filter(|&f| f > i64::MIN as f64 && f < u64::MAX as f64)
}
