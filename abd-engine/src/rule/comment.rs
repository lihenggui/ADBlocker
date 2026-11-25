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

//! Shared syntax for list headers and comments.

/// Whether `line` is no rule: blank, or a comment as adblock-rust takes
/// one.
pub fn is_comment(line: &str) -> bool {
    let l = line.trim();
    l.is_empty()
        || l.starts_with('!')
        || l.starts_with("[Adblock")
        || l.strip_prefix('#')
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

/// The lines of `list` that are rules.
pub fn count(list: &str) -> usize {
    super::preprocess::lines(list)
        .filter(|line| !is_comment(line))
        .count()
}
