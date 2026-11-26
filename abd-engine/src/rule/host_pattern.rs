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

//! Host-anchor and hosts-file patterns and their hostname options.

use std::borrow::Cow;

use super::options::{OptionToken, Options, Quoting};

pub(crate) fn canonical_host_options(line: &str) -> Cow<'_, str> {
    let Some(options) = Options::read(line, Quoting::AdGuard) else {
        return Cow::Borrowed(line);
    };
    let canonical = |token: OptionToken<'_>| {
        matches!(token.name(), "domain" | "from" | "to" | "denyallow")
            && token.value.is_some_and(|value| {
                !value.contains('/') && value.bytes().any(|byte| byte.is_ascii_uppercase())
            })
    };
    if !options.tokens().any(canonical) {
        return Cow::Borrowed(line);
    }
    let mut normalized = options.pattern.to_owned();
    let mut separator = '$';
    for token in options.tokens() {
        normalized.push(separator);
        if canonical(token) {
            normalized.push_str(token.raw_name);
            normalized.push('=');
            normalized.push_str(&token.value.unwrap().to_ascii_lowercase());
        } else {
            normalized.push_str(token.raw);
        }
        separator = ',';
    }
    Cow::Owned(normalized)
}

pub(crate) enum ExceptionHost<'a> {
    Anchored(&'a str),
    Leading {
        host: &'a str,
        has_scheme: bool,
        absolute: bool,
    },
}

/// What a leading or anchored rule pattern says before the path.
pub(super) fn host_token(pattern: &str) -> &str {
    &pattern[..pattern
        .find(['/', '^', '?', ':', '|', '$'])
        .unwrap_or(pattern.len())]
}

fn registrable(host: &str) -> bool {
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && crate::public_suffix::Name::parse(host).is_some_and(|name| name.root().is_some())
}

pub(super) fn hosts_rule(line: &str) -> Option<&str> {
    if !line.starts_with('0') && !line.starts_with('1') {
        return None;
    }
    let mut fields = line.split_ascii_whitespace();
    match (fields.next(), fields.next(), fields.next()) {
        (Some("0.0.0.0" | "127.0.0.1"), Some(host), None) if registrable(host) => Some(host),
        _ => None,
    }
}

pub(super) fn host_pattern(host: &str) -> Cow<'_, str> {
    Cow::Owned(format!("||{host}^"))
}

// Bare anchors draw all their tokens from hostname labels. In adblock-rust,
// rule counts can still change bucket selection, and request tokenization
// skips tokens beside '*' and is bounded.
pub(crate) fn is_bare_host_anchor(line: &str) -> bool {
    line.strip_prefix("||")
        .and_then(|host| host.strip_suffix('^'))
        .is_some_and(crate::host::has_hostname_labels)
}

/// A host anchor with no request conditions, or an equivalent hosts-file line.
pub(super) fn whole_host<'a>(
    line: &'a str,
    hosts_file: Option<&'a str>,
) -> Option<(Cow<'a, str>, &'a str)> {
    if let Some(rest) = line.strip_prefix("@@").unwrap_or(line).strip_prefix("||") {
        let rest = rest.strip_suffix("$badfilter").unwrap_or(rest);
        let host = rest.strip_suffix('^')?;
        return registrable(host).then_some((Cow::Borrowed(line), host));
    }
    let host = hosts_file?;
    Some((host_pattern(host), host))
}

pub(crate) fn whole_pattern(line: &str) -> Option<(Cow<'_, str>, &str)> {
    let line = line.trim();
    whole_host(line, hosts_rule(line))
}
