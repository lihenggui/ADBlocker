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

//! The cached specifications and compiled implementations of response actions.

use serde::{Deserialize, Serialize};

mod prune;
mod replace;

#[cfg(test)]
pub(crate) use replace::value_end;

/// The cache representation and identity of an action.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ActionSpec {
    Prune(String),
    Replace {
        pattern: String,
        replacement: String,
        insensitive: bool,
        single_line: bool,
    },
}

/// A compiled action that can safely be shared among concurrent jobs.
pub(crate) enum ResponseAction {
    Prune(prune::Compiled),
    Replace(replace::Compiled),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum ActionKind {
    // Sort pruning first so replacements see the remaining text.
    Prune,
    Replace,
}

impl ActionKind {
    pub(crate) fn parse(self, value: &str) -> Option<ActionSpec> {
        match self {
            Self::Prune => prune::parse(value).map(ActionSpec::Prune),
            Self::Replace => {
                let replace::Parsed {
                    pattern,
                    replacement,
                    insensitive,
                    single_line,
                } = replace::parse(value)?;
                Some(ActionSpec::Replace {
                    pattern,
                    replacement,
                    insensitive,
                    single_line,
                })
            }
        }
    }

    pub(crate) fn disables(self, value: Option<&str>) -> bool {
        match self {
            Self::Prune => prune::disables(value),
            Self::Replace => replace::disables(value),
        }
    }

    fn overrides_block(self) -> bool {
        match self {
            Self::Prune => prune::overrides_block(),
            Self::Replace => replace::overrides_block(),
        }
    }

    pub(crate) fn deduplicates(self) -> bool {
        match self {
            Self::Prune => false,
            Self::Replace => true,
        }
    }
}

impl ActionSpec {
    /// Reject a pattern that cannot be searched within the per-expression limit.
    pub(crate) fn compile(&self) -> Option<ResponseAction> {
        match self {
            Self::Prune(expression) => prune::compile(expression).map(ResponseAction::Prune),
            Self::Replace {
                pattern,
                replacement,
                insensitive,
                single_line,
            } => replace::compile(pattern, replacement, *insensitive, *single_line)
                .map(ResponseAction::Replace),
        }
    }
}

impl ResponseAction {
    pub(crate) fn kind(&self) -> ActionKind {
        match self {
            Self::Prune(_) => ActionKind::Prune,
            Self::Replace(_) => ActionKind::Replace,
        }
    }

    pub(crate) fn overrides_block(&self) -> bool {
        self.kind().overrides_block()
    }

    /// Heap used by the compiled matcher. Search scratch space belongs to
    /// the blocking job, so an idle action keeps none of it.
    pub(crate) fn regex_bytes(&self) -> usize {
        match self {
            Self::Prune(_) => 0,
            Self::Replace(action) => action.regex_bytes(),
        }
    }

    /// `None` abandons the whole rewrite on overflow, timeout or cancellation.
    pub(crate) fn apply(&self, input: &[u8], within_budget: &dyn Fn() -> bool) -> Option<Vec<u8>> {
        if !within_budget() {
            return None;
        }
        match self {
            Self::Prune(action) => action.apply(input, within_budget),
            Self::Replace(action) => action.apply(input, within_budget),
        }
    }
}
