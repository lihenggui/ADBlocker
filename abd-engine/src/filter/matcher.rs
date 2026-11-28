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

//! Matcher input and the original text of rewritten rules in debug engines.

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::sourcemap::FilterRuleDebugInfo;
use serde::{Deserialize, Serialize};

use super::SeenAnchors;
use crate::rule::{Kind, Rule, is_bare_host_anchor};

/// Original lines for the rules rewritten before they reach adblock-rust.
/// The input list budget bounds this data, and only debug engines keep it.
#[derive(Default, Deserialize, Serialize)]
pub(crate) struct OriginalLines(Vec<(u32, u32, String)>);

impl OriginalLines {
    pub(crate) fn get(&self, rule: &FilterRuleDebugInfo) -> Option<&str> {
        let location = rule.source_location.as_ref()?;
        let at = self
            .0
            .binary_search_by_key(&(location.source_index, location.line_number), |entry| {
                (entry.0, entry.1)
            })
            .ok()?;
        Some(&self.0[at].2)
    }

    pub(crate) fn shown(&self, rule: FilterRuleDebugInfo) -> Option<String> {
        self.get(&rule).map(str::to_owned).or(rule.raw_line)
    }
}

pub(crate) struct MatcherList {
    text: String,
    original: Vec<(u32, String)>,
    lines: u32,
}

impl MatcherList {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            text: String::with_capacity(capacity),
            original: Vec::new(),
            lines: 0,
        }
    }

    pub(crate) fn push(&mut self, line: &str, original: Option<&str>) {
        if let Some(original) = original {
            self.original.push((self.lines, original.to_owned()));
        }
        self.text.push_str(line);
        self.text.push('\n');
        self.lines += 1;
    }

    #[cfg(test)]
    pub(super) fn text(&self) -> &str {
        &self.text
    }
}

pub(crate) struct MatcherLists<'a> {
    set: FilterSet,
    debug: bool,
    originals: OriginalLines,
    anchors: SeenAnchors<'a>,
}

impl<'a> MatcherLists<'a> {
    pub(crate) fn new(debug: bool) -> Self {
        Self::with_anchor_capacity(debug, 0)
    }

    pub(super) fn with_anchor_capacity(debug: bool, capacity: usize) -> Self {
        Self {
            set: FilterSet::new(debug),
            debug,
            originals: OriginalLines::default(),
            anchors: SeenAnchors::with_capacity(capacity),
        }
    }

    /// Returns true for a bare source anchor, which has no options to project.
    pub(crate) fn add(&mut self, list: &mut MatcherList, source: &Rule<'a>) -> bool {
        let native = matches!(source.kind, Kind::Matcher);
        if !self.debug && native && is_bare_host_anchor(source.line) {
            if self.anchors.insert(source.line, source.line) {
                list.push(source.line, None);
            }
            return true;
        }
        let Some(line) = source.matcher_input() else {
            return false;
        };
        // Reject unsafe calls before their helpers can consume the page's
        // budget. The raw text still includes controls adblock may discard.
        if source.is_scriptlet() && !crate::cosmetic::safe_raw_text(&line) {
            return false;
        }
        // A native rule can only change the spelling of its options.
        if !self.debug && !native && is_bare_host_anchor(&line) {
            if self.anchors.insert(&line, source.line) {
                list.push(&line, None);
            }
            return false;
        }
        let original = if self.debug {
            source
                .original()
                .or_else(|| (line != source.line).then_some(source.line))
        } else {
            None
        };
        list.push(&line, original);
        false
    }

    pub(crate) fn finish_list(&mut self, list: MatcherList, options: ParseOptions) {
        let source = self.set.add_filter_list(list.text, options).source_index as u32;
        self.originals.0.extend(
            list.original
                .into_iter()
                .map(|(line, text)| (source, line, text)),
        );
    }

    /// Once the lists are projected, the matcher needs only its inputs and source map.
    pub(super) fn finish_projection(self) -> MatcherLists<'static> {
        MatcherLists {
            set: self.set,
            debug: self.debug,
            originals: self.originals,
            anchors: SeenAnchors::with_capacity(0),
        }
    }

    pub(crate) fn compile(self) -> (Engine, OriginalLines) {
        drop(self.anchors);
        // The optimizer can combine rules and erase their source locations.
        let engine = if self.originals.0.is_empty() {
            Engine::new_with_filter_set(self.set)
        } else {
            Engine::new_with_filter_set_no_optimize(self.set)
        };
        (engine, self.originals)
    }
}
