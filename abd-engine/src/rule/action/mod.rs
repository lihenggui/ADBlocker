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

use hyper::header::HeaderName;
use serde::{Deserialize, Serialize};

pub(crate) mod html;
mod prune;
pub(super) mod removeheader;
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
    RemoveHeader(removeheader::Spec),
}

/// A compiled action that can safely be shared among concurrent jobs.
pub(crate) enum ResponseAction {
    Prune(prune::Compiled),
    Replace(replace::Compiled),
    RemoveHeader(removeheader::Compiled),
}

pub(crate) enum Place<'a> {
    Request(&'a HeaderName),
    ResponseHead(&'a HeaderName),
    ResponseBody,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct PageExceptions {
    pub(crate) document: bool,
    pub(crate) content: bool,
    pub(crate) urlblock: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum ActionKind {
    // Sort pruning first so replacements see the remaining text.
    Prune,
    Replace,
    RemoveHeader,
}

impl ActionKind {
    pub(crate) fn parse(self, value: &str) -> Option<ActionSpec> {
        match self {
            Self::Prune => prune::parse(value).map(ActionSpec::Prune),
            Self::RemoveHeader => removeheader::parse(value)
                .ok()
                .map(ActionSpec::RemoveHeader),
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
            Self::RemoveHeader => value.is_none(),
        }
    }

    fn overrides_block(self) -> bool {
        match self {
            Self::Prune => prune::overrides_block(),
            Self::Replace => replace::overrides_block(),
            Self::RemoveHeader => false,
        }
    }

    pub(crate) fn deduplicates(self) -> bool {
        match self {
            Self::Prune | Self::RemoveHeader => false,
            Self::Replace => true,
        }
    }

    pub(crate) fn disabled_by(self, page: PageExceptions) -> bool {
        page.document
            || match self {
                Self::Prune => page.content || page.urlblock,
                Self::Replace => page.content,
                Self::RemoveHeader => page.urlblock,
            }
    }
}

impl ActionSpec {
    /// Reject a pattern that cannot be searched within the per-expression limit.
    pub(crate) fn compile(&self) -> Option<ResponseAction> {
        match self {
            Self::Prune(expression) => prune::compile(expression).map(ResponseAction::Prune),
            Self::RemoveHeader(spec) => spec.compile().ok().map(ResponseAction::RemoveHeader),
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
            Self::RemoveHeader(_) => ActionKind::RemoveHeader,
        }
    }

    pub(crate) fn place(&self) -> Place<'_> {
        match self {
            Self::Prune(_) | Self::Replace(_) => Place::ResponseBody,
            Self::RemoveHeader(removeheader::Removal::Request(name)) => Place::Request(name),
            Self::RemoveHeader(removeheader::Removal::Response(name)) => Place::ResponseHead(name),
        }
    }

    pub(crate) fn overrides_block(&self) -> bool {
        self.kind().overrides_block()
    }

    /// Heap used by the compiled matcher. Search scratch space belongs to
    /// the blocking job, so an idle action keeps none of it.
    pub(crate) fn regex_bytes(&self) -> usize {
        match self {
            Self::Prune(_) | Self::RemoveHeader(_) => 0,
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
            Self::RemoveHeader(_) => None,
        }
    }
}
