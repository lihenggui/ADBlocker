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

use crate::rule::Rule;

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

    pub(super) fn shown(&self, rule: FilterRuleDebugInfo) -> Option<String> {
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

    pub(super) fn push(&mut self, line: &str, original: Option<&str>) {
        if let Some(original) = original {
            self.original.push((self.lines, original.to_owned()));
        }
        self.text.push_str(line);
        self.text.push('\n');
        self.lines += 1;
    }
}

pub(crate) struct MatcherLists {
    set: FilterSet,
    debug: bool,
    originals: OriginalLines,
}

impl MatcherLists {
    pub(crate) fn new(debug: bool) -> Self {
        Self {
            set: FilterSet::new(debug),
            debug,
            originals: OriginalLines::default(),
        }
    }

    pub(crate) fn add(&self, list: &mut MatcherList, source: &Rule<'_>) {
        let Some(line) = source.matcher_input() else {
            return;
        };
        // Reject unsafe calls before their helpers can consume the page's
        // budget. The raw text still includes controls adblock may discard.
        if source.is_scriptlet() && !crate::cosmetic::safe_raw_text(&line) {
            return;
        }
        list.push(&line, source.original().filter(|_| self.debug));
    }

    pub(crate) fn finish_list(&mut self, list: MatcherList, options: ParseOptions) {
        let source = self.set.add_filter_list(list.text, options).source_index as u32;
        self.originals.0.extend(
            list.original
                .into_iter()
                .map(|(line, text)| (source, line, text)),
        );
    }

    pub(crate) fn compile(self) -> (Engine, OriginalLines) {
        // The optimizer can combine rules and erase their source locations.
        let engine = if self.originals.0.is_empty() {
            Engine::new_with_filter_set(self.set)
        } else {
            Engine::new_with_filter_set_no_optimize(self.set)
        };
        (engine, self.originals)
    }
}
