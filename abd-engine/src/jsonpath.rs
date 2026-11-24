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
//! The restricted JSONPath language used by response rules.

//! The path language of AdGuard's `$jsonprune`: Goessner's JSONPath without
//! script expressions, with the three filters AdGuard defines (`has`,
//! `key-eq`, `key-substr`), whose key may itself be a path.
//!
//! A path selects items of a JSON document, and what it selects is removed
//! from its parent. Selecting never fails: a name that is absent or an
//! index out of range selects nothing. No third-party implementation is
//! used: none has these filters, and a general one brings script
//! expressions, which a list must not be able to have run.

mod document;
mod evaluate;
mod parse;

use serde_json::Value;

pub(crate) use document::prune;

/// A parsed expression: the steps from the root, `$`.
pub(crate) struct Path(Vec<Step>);

/// `.selector`, or `..selector`, which applies to the item and to every
/// item below it.
struct Step {
    recursive: bool,
    selector: Selector,
}

enum Selector {
    /// `*`: every child.
    Wildcard,
    /// A name, or a bracket of names, indexes and slices.
    Members(Vec<Member>),
    /// `[?(...)]`: the children for which the filter holds.
    Filter(Filter),
}

enum Member {
    Name(String),
    /// Counted from the end when negative.
    Index(i64),
    Slice(Slice),
}

/// `start:end:step` as in Python; a step of zero selects nothing.
struct Slice {
    start: Option<i64>,
    end: Option<i64>,
    stride: i64,
}

struct Filter {
    key: Key,
    predicate: Predicate,
}

enum Key {
    /// A member of the child, which then has to be an object.
    Name(String),
    /// A path evaluated with the child as its root.
    Path(Path),
}

enum Predicate {
    /// `has`: the key is there.
    Has,
    /// `key-eq`: its value equals this one, of the same JSON type.
    Equal(Value),
    /// `key-substr`: its value is a string that contains this one.
    Substring(String),
}

#[cfg(test)]
mod tests;
