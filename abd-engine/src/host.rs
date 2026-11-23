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

/// A host name or an address, and nothing a rule could hide in.
pub fn is_host(host: &str) -> bool {
    !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'))
}

/// The literal host and its nonempty parents, in lookup order.
pub(crate) fn suffixes(host: &str) -> impl Iterator<Item = &str> {
    std::iter::once(host).chain(
        host.match_indices('.')
            .map(|(dot, _)| &host[dot + 1..])
            .take_while(|parent| !parent.is_empty()),
    )
}
