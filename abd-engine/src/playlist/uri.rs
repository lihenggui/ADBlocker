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

//! Resolve a segment reference against the playlist's HTTP request URL.

use std::borrow::Cow;

use hyper::Uri;

pub(crate) fn valid_scheme(scheme: &str) -> bool {
    scheme
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
}

pub(crate) fn resolve(base: &Uri, line: &str) -> Option<String> {
    let (reference, fragment) = split(line, '#');
    let (path, query) = split(reference, '?');
    let (scheme, rest) = scheme_and_path(path);
    let (scheme, authority, path, query) = if let Some(rest) = rest.strip_prefix("//") {
        let end = rest.find('/').unwrap_or(rest.len());
        let (authority, path) = rest.split_at(end);
        if authority.is_empty() {
            return None;
        }
        (
            scheme.or_else(|| base.scheme_str())?,
            authority,
            Cow::Borrowed(path),
            query,
        )
    } else {
        if scheme.is_some() {
            // A scheme without an authority is already absolute. Its literal
            // form has been searched, and it does not inherit anything.
            return None;
        }
        let (path, query) = inherit_path(base, rest, query);
        (base.scheme_str()?, base.authority()?.as_str(), path, query)
    };
    let mut url = format!("{scheme}://{authority}");
    normalize_path(&mut url, &path);
    if let Some(query) = query {
        url.push('?');
        url.push_str(query);
    }
    if let Some(fragment) = fragment {
        url.push('#');
        url.push_str(fragment);
    }
    Some(url)
}

fn split(text: &str, delimiter: char) -> (&str, Option<&str>) {
    text.split_once(delimiter)
        .map_or((text, None), |(first, second)| (first, Some(second)))
}

fn scheme_and_path(path: &str) -> (Option<&str>, &str) {
    match path.split_once(':') {
        Some((scheme, rest)) if valid_scheme(scheme) => (Some(scheme), rest),
        _ => (None, path),
    }
}

fn inherit_path<'a>(
    base: &'a Uri,
    path: &'a str,
    query: Option<&'a str>,
) -> (Cow<'a, str>, Option<&'a str>) {
    if path.is_empty() {
        (Cow::Borrowed(base.path()), query.or_else(|| base.query()))
    } else if path.starts_with('/') {
        (Cow::Borrowed(path), query)
    } else {
        let directory = base
            .path()
            .rsplit_once('/')
            .map_or("", |(directory, _)| directory);
        (Cow::Owned(format!("{directory}/{path}")), query)
    }
}

/// RFC 3986 §5.2.4 for paths under an authority. Keep consecutive slashes
/// and a terminal slash, including the one implied by a final dot segment.
fn normalize_path(url: &mut String, path: &str) {
    let Some(path) = path.strip_prefix('/') else {
        return;
    };
    let start = url.len();
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        match part {
            "." | ".." => {
                if part == ".."
                    && let Some(last) = url[start..].rfind('/')
                {
                    url.truncate(start + last);
                }
                if parts.peek().is_none() {
                    url.push('/');
                }
            }
            _ => {
                url.push('/');
                url.push_str(part);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_references_inherit_only_the_applicable_base_components() {
        let base: Uri = "https://media.example:8443/a/b/index.m3u8?playlist=1"
            .parse()
            .unwrap();
        for (line, expected) in [
            ("../ad.ts", "https://media.example:8443/a/ad.ts"),
            (
                "/ad.ts?part=1#t=0",
                "https://media.example:8443/ad.ts?part=1#t=0",
            ),
            ("//ads.example/a/../ad.ts", "https://ads.example/ad.ts"),
            ("https://ads.example/./ad.ts", "https://ads.example/ad.ts"),
            (
                "?part=1",
                "https://media.example:8443/a/b/index.m3u8?part=1",
            ),
            (
                "#t=0",
                "https://media.example:8443/a/b/index.m3u8?playlist=1#t=0",
            ),
            (
                "./part//../clip.ts",
                "https://media.example:8443/a/b/part/clip.ts",
            ),
            ("../../../../..", "https://media.example:8443/"),
        ] {
            assert_eq!(resolve(&base, line).as_deref(), Some(expected), "{line}");
        }
        assert!(resolve(&base, "data:audio/mp4,opaque").is_none());
    }
}
