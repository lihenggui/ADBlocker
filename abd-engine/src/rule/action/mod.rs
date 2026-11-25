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

pub(crate) mod cookie;
pub(crate) mod hls;
pub(crate) mod html;
mod prune;
mod removeheader;
mod replace;
mod urltransform;

#[cfg(test)]
pub(crate) use replace::value_end;

/// The cache representation and identity of an action.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ActionSpec {
    Prune(String),
    Hls(hls::Spec),
    Replace {
        pattern: String,
        replacement: String,
        insensitive: bool,
        single_line: bool,
    },
    RemoveHeader(removeheader::Spec),
    UrlTransform(urltransform::Spec),
    RemoveParamException(String),
    Cookie(cookie::Spec),
}

/// A compiled action that can safely be shared among concurrent jobs.
pub(crate) enum ResponseAction {
    Prune(prune::Compiled),
    Hls(hls::Compiled),
    Replace(replace::Compiled),
    RemoveHeader(removeheader::Compiled),
    UrlTransform(urltransform::Compiled),
    RemoveParamException(String),
    Cookie(cookie::Compiled),
}

pub(crate) enum Place<'a> {
    Request(&'a HeaderName),
    RequestUrl,
    ResponseHead(&'a HeaderName),
    RequestAndResponseHead,
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
    // Sort body actions in the order in which rewrite applies them.
    Prune,
    Hls,
    Replace,
    RemoveHeader,
    UrlTransform,
    RemoveParamException,
    Cookie,
}

impl ActionKind {
    pub(crate) fn parse(self, value: &str) -> Option<ActionSpec> {
        match self {
            Self::Prune => prune::parse(value).map(ActionSpec::Prune),
            Self::Cookie => cookie::parse(value, false).ok().map(ActionSpec::Cookie),
            Self::Hls => hls::parse(value).ok().map(ActionSpec::Hls),
            Self::RemoveHeader => removeheader::parse(value)
                .ok()
                .map(ActionSpec::RemoveHeader),
            Self::UrlTransform => urltransform::parse(value).map(ActionSpec::UrlTransform),
            Self::RemoveParamException => {
                parameter_name(value).then(|| ActionSpec::RemoveParamException(value.to_owned()))
            }
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
            Self::Hls => value.is_none(),
            Self::Replace | Self::UrlTransform => replace::disables(value),
            Self::RemoveHeader => value.is_none(),
            Self::RemoveParamException => false,
            Self::Cookie => value.is_none_or(|value| value.trim().is_empty()),
        }
    }

    fn overrides_block(self) -> bool {
        match self {
            Self::Prune => prune::overrides_block(),
            Self::Replace => replace::overrides_block(),
            Self::Hls
            | Self::RemoveHeader
            | Self::UrlTransform
            | Self::RemoveParamException
            | Self::Cookie => false,
        }
    }

    pub(crate) fn deduplicates(self) -> bool {
        match self {
            Self::Prune | Self::RemoveHeader | Self::UrlTransform | Self::Cookie => false,
            Self::Hls | Self::Replace | Self::RemoveParamException => true,
        }
    }

    pub(crate) fn disabled_by(self, page: PageExceptions) -> bool {
        page.document
            || match self {
                Self::Prune | Self::Hls => page.content || page.urlblock,
                Self::Replace => page.content,
                Self::RemoveHeader
                | Self::UrlTransform
                | Self::RemoveParamException
                | Self::Cookie => page.urlblock,
            }
    }

    pub(crate) fn takes(self, mime: &str) -> bool {
        match self {
            Self::Hls => hls::takes(mime),
            Self::Prune | Self::Replace => crate::media_type::text(mime),
            Self::RemoveHeader | Self::UrlTransform | Self::RemoveParamException | Self::Cookie => {
                false
            }
        }
    }
}

impl ActionSpec {
    /// Reject a pattern that cannot be searched within the per-expression limit.
    pub(crate) fn compile(&self) -> Option<ResponseAction> {
        match self {
            Self::Cookie(spec) => spec.compile().ok().map(ResponseAction::Cookie),
            Self::Prune(expression) => prune::compile(expression).map(ResponseAction::Prune),
            Self::Hls(spec) => spec.compile().map(ResponseAction::Hls),
            Self::RemoveHeader(spec) => spec.compile().ok().map(ResponseAction::RemoveHeader),
            Self::UrlTransform(spec) => spec.compile().map(ResponseAction::UrlTransform),
            Self::RemoveParamException(name) => {
                parameter_name(name).then(|| ResponseAction::RemoveParamException(name.clone()))
            }
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
            Self::Cookie(_) => ActionKind::Cookie,
            Self::Prune(_) => ActionKind::Prune,
            Self::Hls(_) => ActionKind::Hls,
            Self::Replace(_) => ActionKind::Replace,
            Self::RemoveHeader(_) => ActionKind::RemoveHeader,
            Self::UrlTransform(_) => ActionKind::UrlTransform,
            Self::RemoveParamException(_) => ActionKind::RemoveParamException,
        }
    }

    pub(crate) fn place(&self) -> Place<'_> {
        match self {
            Self::Cookie(_) => Place::RequestAndResponseHead,
            Self::Prune(_) | Self::Hls(_) | Self::Replace(_) => Place::ResponseBody,
            Self::UrlTransform(_) | Self::RemoveParamException(_) => Place::RequestUrl,
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
            Self::Prune(_) | Self::RemoveHeader(_) | Self::RemoveParamException(_) => 0,
            Self::Cookie(action) => action.regex_bytes(),
            Self::Hls(_) => hls::REGEX_CHARGE,
            Self::Replace(action) => action.regex_bytes(),
            Self::UrlTransform(action) => action.regex_bytes(),
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
            // HLS actions operate on the same playlist in one pass.
            Self::Hls(_)
            | Self::RemoveHeader(_)
            | Self::UrlTransform(_)
            | Self::RemoveParamException(_)
            | Self::Cookie(_) => None,
        }
    }
}

pub(super) fn parameter_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
