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

use hyper::{HeaderMap, header::CONTENT_TYPE};

/// A single, well-formed media type, without parameters.
pub(crate) fn read(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let mime = values.next()?.to_str().ok()?.split(';').next()?.trim();
    if values.next().is_some() {
        return None;
    }
    let (group, subtype) = mime.split_once('/')?;
    [group, subtype]
        .into_iter()
        .all(|token| {
            !token.is_empty()
                && token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        })
        .then_some(mime)
}

/// Lowercase types whose bodies a response rule can treat as text.
pub(crate) fn text(mime: &str) -> bool {
    let Some((group, subtype)) = mime.split_once('/') else {
        return false;
    };
    group == "text"
        || subtype.ends_with("+json")
        || subtype.ends_with("+xml")
        || group == "application"
            && matches!(
                subtype,
                "json"
                    | "javascript"
                    | "x-javascript"
                    | "xml"
                    | "xhtml+xml"
                    | "x-www-form-urlencoded"
                    | "vnd.apple.mpegurl"
                    | "x-mpegurl"
            )
}
