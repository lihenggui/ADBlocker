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

//! Pattern syntax handed to the external network matcher.

use std::borrow::Cow;

use regex::bytes::RegexBuilder;

/// Full URL regexes are compiled lazily by adblock-rust. Check them before
/// enabling a rule that was previously rejected for its noop option.
pub(super) fn network_pattern_supported(pattern: &str, match_case: bool) -> bool {
    let pattern = pattern.strip_prefix("@@").unwrap_or(pattern);
    let pattern = pattern
        .strip_prefix("||")
        .or_else(|| pattern.strip_prefix('|'))
        .unwrap_or(pattern);
    let pattern = pattern.strip_suffix('|').unwrap_or(pattern);
    let Some(body) = pattern
        .strip_prefix('/')
        .and_then(|pattern| pattern.strip_suffix('/'))
    else {
        return true;
    };
    // Match the matcher’s full-regex syntax and its default size limit.
    let mut body = body.replace(r"\/", "/").replace(r"\:", ":");
    if !match_case {
        body.make_ascii_lowercase();
    }
    RegexBuilder::new(&body).unicode(false).build().is_ok()
}

/// The condition matcher only accepts `$match-case` on a full regex.
pub(super) fn case_sensitive_pattern(pattern: &str) -> Cow<'_, str> {
    if pattern.len() > 1 && pattern.starts_with('/') && pattern.ends_with('/') {
        return Cow::Borrowed(pattern);
    }
    let mut regex = String::from("/");
    let pattern = if let Some(host_pattern) = pattern.strip_prefix("||") {
        regex.push_str(r"^https?://(?:[^./:?#]+\.)*(?i:");
        let end = host_pattern
            .find(['/', '^', ':', '|'])
            .unwrap_or(host_pattern.len());
        let (host, tail) = host_pattern.split_at(end);
        write_pattern(&mut regex, host, r"[^/:?#]*");
        regex.push(')');
        tail
    } else if let Some(pattern) = pattern.strip_prefix('|') {
        regex.push('^');
        pattern
    } else {
        pattern
    };
    let tail = pattern.strip_suffix('|');
    write_pattern(&mut regex, tail.unwrap_or(pattern), ".*");
    if tail.is_some() {
        regex.push('$');
    }
    regex.push('/');
    Cow::Owned(regex)
}

fn write_pattern(regex: &mut String, pattern: &str, wildcard: &str) {
    let mut parts = pattern.split_inclusive(['*', '^']).peekable();
    while let Some(part) = parts.next() {
        if let Some(text) = part.strip_suffix('*') {
            regex.push_str(&regex::escape(text));
            regex.push_str(wildcard);
        } else if let Some(text) = part.strip_suffix('^') {
            regex.push_str(&regex::escape(text));
            regex.push_str(if parts.peek().is_none() {
                r"(?:[^\w.%-]|$)"
            } else {
                r"[^\w.%-]"
            });
        } else {
            regex.push_str(&regex::escape(part));
        }
    }
}

#[cfg(test)]
mod tests;
