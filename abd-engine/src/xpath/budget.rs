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

use crate::limits::{MAX_XPATH_TEXT_WORK, MAX_XPATH_VISITS};

pub(super) type Result<T> = std::result::Result<T, ()>;

pub(super) struct Budget<'a> {
    visits: usize,
    bytes: usize,
    within_budget: &'a dyn Fn() -> bool,
}

impl<'budget> Budget<'budget> {
    pub(super) fn new(within_budget: &'budget dyn Fn() -> bool) -> Self {
        Self {
            visits: MAX_XPATH_VISITS,
            bytes: MAX_XPATH_TEXT_WORK,
            within_budget,
        }
    }

    pub(super) fn check(&self) -> Result<()> {
        (self.within_budget)().then_some(()).ok_or(())
    }

    pub(super) fn visit(&mut self) -> Result<()> {
        self.visits = self.visits.checked_sub(1).ok_or(())?;
        self.check()
    }

    pub(super) fn text(&mut self, bytes: usize) -> Result<()> {
        self.bytes = self.bytes.checked_sub(bytes).ok_or(())?;
        self.check()
    }
}
