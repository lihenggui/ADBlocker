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

//! Quoting shared by the option reader and action values.

/// AdGuard quotes option separators with an immediately preceding backslash.
pub(crate) fn unescaped(text: &str, byte: u8) -> impl DoubleEndedIterator<Item = usize> {
    text.bytes().enumerate().filter_map(move |(i, b)| {
        (b == byte && (i == 0 || text.as_bytes()[i - 1] != b'\\')).then_some(i)
    })
}

/// A slash is quoted when an odd number of backslashes precedes it.
pub(crate) fn delimiter(part: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, byte) in part.bytes().enumerate() {
        match byte {
            b'/' if !escaped => return Some(index),
            b'\\' => escaped = !escaped,
            _ => escaped = false,
        }
    }
    None
}

/// Remove only the quoting of this layer and leave regex escapes intact.
pub(crate) fn unescape(part: &str, chars: &[char]) -> String {
    let mut iter = part.chars().peekable();
    let mut output = String::with_capacity(part.len());
    while let Some(ch) = iter.next() {
        if ch == '\\' && iter.peek().is_some_and(|next| chars.contains(next)) {
            output.push(iter.next().expect("peeked character"));
        } else {
            output.push(ch);
        }
    }
    output
}
