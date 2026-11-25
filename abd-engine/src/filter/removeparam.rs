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

use std::collections::HashSet;

pub(crate) fn supported_method(method: &str) -> bool {
    matches!(method, "GET" | "HEAD" | "OPTIONS" | "POST")
}

fn query(url: &str) -> Option<(&str, &str, &str)> {
    let end = url.find('#').unwrap_or(url.len());
    let (path, query) = url[..end].split_once('?')?;
    Some((path, query, &url[end..]))
}

fn populated_name(pair: &str) -> Option<&str> {
    let (name, value) = pair.split_once('=')?;
    (!value.is_empty()).then_some(name)
}

/// Each matcher removes every occurrence of a populated key it matches.
/// Combine their removals while leaving retained parameters in wire order.
pub(super) fn merge(original: &str, web: String, app: String) -> Option<String> {
    let web_keys = retained_keys(&web);
    let app_keys = retained_keys(&app);
    rebuild(original, |name| {
        web_keys.contains(name) && app_keys.contains(name)
    })
}

/// The native matcher does not handle valued exceptions. Put exempted
/// parameters back using the original spelling and position.
pub(crate) fn preserve(original: &str, rewritten: &str, exempt: &HashSet<&str>) -> Option<String> {
    let kept = retained_keys(rewritten);
    rebuild(original, |name| {
        kept.contains(name) || exempt.contains(name)
    })
}

fn retained_keys(url: &str) -> HashSet<&str> {
    query(url)
        .into_iter()
        .flat_map(|(_, query, _)| query.split('&').filter_map(populated_name))
        .collect()
}

fn rebuild(original: &str, keep: impl Fn(&str) -> bool) -> Option<String> {
    let (path, query, fragment) = query(original)?;
    let query = query
        .split('&')
        .filter(|pair| populated_name(pair).is_none_or(&keep))
        .collect::<Vec<_>>()
        .join("&");
    let mut result = String::with_capacity(original.len());
    result.push_str(path);
    if !query.is_empty() {
        result.push('?');
        result.push_str(&query);
    }
    result.push_str(fragment);
    Some(result)
}
