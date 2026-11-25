// SPDX-License-Identifier: GPL-3.0-only
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

//! The body transformation for the HTML rules selected for this document.

use std::sync::Arc;

use bytes::Bytes;

use super::PageExceptions;
use crate::htmlfilter::{self, Compiled};

pub(crate) fn disabled_by(page: PageExceptions) -> bool {
    page.document || page.content
}

pub(crate) fn takes(mime: &str) -> bool {
    crate::media_type::text(mime)
}

/// The host index supplies one operation for all the rules of a document.
pub(crate) struct Action {
    pub(crate) rules: Vec<Arc<Compiled>>,
    pub(crate) charset: Option<String>,
}

impl Action {
    pub(crate) fn apply(&self, body: Bytes, within_budget: &dyn Fn() -> bool) -> Option<Bytes> {
        htmlfilter::apply(body, &self.rules, self.charset.as_deref(), within_budget)
    }
}
