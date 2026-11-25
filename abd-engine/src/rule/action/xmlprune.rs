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

use bytes::Bytes;

use crate::rule::quoting::{unescape, unescaped};
use crate::xpath::{self, XPath};

pub(super) fn parse(value: &str) -> Option<String> {
    if unescaped(value, b'$').next().is_some() || unescaped(value, b',').next().is_some() {
        return None;
    }
    let expression = unescape(value, &[',', '$']);
    XPath::parse(&expression).ok()?;
    Some(expression)
}

pub(super) fn takes(mime: &str) -> bool {
    mime.split_once('/').is_some_and(|(group, subtype)| {
        (matches!(group, "application" | "text") && subtype == "xml") || subtype.ends_with("+xml")
    })
}

pub(crate) struct Compiled(XPath);

pub(super) fn compile(expression: &str) -> Option<Compiled> {
    XPath::parse(expression).ok().map(Compiled)
}

impl Compiled {
    pub(super) fn apply(&self, input: Bytes, within_budget: &dyn Fn() -> bool) -> Option<Bytes> {
        xpath::prune(&self.0, input, within_budget)
    }
}
