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

//! How one policy of a page is read for what it says of an element put
//! into the page: the style and the script are read the same way.

use std::str::SplitAsciiWhitespace;

use base64::Engine as _;

/// One directive of a policy.
#[derive(Clone)]
pub(super) struct Directive<'a> {
    /// Its place among the parts of the policy between semicolons.
    pub(super) index: usize,
    /// As the policy has it, between its semicolons.
    pub(super) raw: &'a str,
    /// Its name, as written.
    pub(super) name: &'a str,
    /// What it allows.
    pub(super) sources: SplitAsciiWhitespace<'a>,
}

impl Directive<'_> {
    pub(super) fn has(&self, keyword: &str) -> bool {
        self.sources
            .clone()
            .any(|source| source.eq_ignore_ascii_case(keyword))
    }
}

/// The directive named `name`: the first, as a browser takes it.
pub(super) fn named<'a>(policy: &'a str, name: &str) -> Option<Directive<'a>> {
    policy.split(';').enumerate().find_map(|(index, raw)| {
        let mut words = raw.split_ascii_whitespace();
        let first = words.next()?;
        first.eq_ignore_ascii_case(name).then_some(Directive {
            index,
            raw,
            name: first,
            sources: words,
        })
    })
}

/// An element put into a page.
#[derive(Clone, Copy)]
pub(super) enum Element {
    Style,
    Script,
}

/// Which directive an element goes by.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum By {
    /// The one for elements of its kind alone.
    Elements,
    /// The one for everything of its kind.
    Kind,
    /// The one everything falls back on.
    Default,
}

/// The directive an element of `element` goes by: the first of these
/// that the policy has.
pub(super) fn effective(policy: &str, element: Element) -> Option<(Directive<'_>, By)> {
    let (elements, kind) = match element {
        Element::Style => ("style-src-elem", "style-src"),
        Element::Script => ("script-src-elem", "script-src"),
    };
    named(policy, elements)
        .map(|d| (d, By::Elements))
        .or_else(|| named(policy, kind).map(|d| (d, By::Kind)))
        .or_else(|| named(policy, "default-src").map(|d| (d, By::Default)))
}

/// The value of `source` if it is a nonce. Nothing that could end the
/// attribute it is put into.
pub(super) fn nonce(source: &str) -> Option<&str> {
    let (kind, value) = quoted(source)?.split_once('-')?;
    let plain = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'-' | b'_' | b'='));
    (kind.eq_ignore_ascii_case("nonce") && plain).then_some(value)
}

/// Whether `source` names inline elements by a hash or a nonce. A
/// browser then ignores `'unsafe-inline'` in the same directive, so
/// naming one more takes nothing from those the page has.
pub(super) fn hash_or_nonce(source: &str) -> bool {
    if nonce(source).is_some() {
        return true;
    }
    let Some((kind, value)) = quoted(source).and_then(|token| token.split_once('-')) else {
        return false;
    };
    let len = match kind.to_ascii_lowercase().as_str() {
        "sha256" => 32,
        "sha384" => 48,
        "sha512" => 64,
        _ => return false,
    };
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .is_ok_and(|value| value.len() == len)
}

fn quoted(source: &str) -> Option<&str> {
    source.strip_prefix('\'')?.strip_suffix('\'')
}
