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

use super::modifiers::CSP;
use super::options::Options;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rejection {
    Empty,
    InvalidValue,
    Reporting,
    Negated,
    Method,
}

pub(super) fn validate(options: &Options<'_>) -> Result<(), Rejection> {
    for token in options.tokens().filter(|token| token.name() == CSP) {
        if token.raw_name != CSP {
            return Err(Rejection::Negated);
        }
        if options.tokens().any(|token| token.name() == "method") {
            return Err(Rejection::Method);
        }
        let value = token.value.unwrap_or_default();
        if value.trim().is_empty() && !options.pattern.starts_with("@@") {
            return Err(Rejection::Empty);
        }
        if !value.is_ascii()
            || value.contains([',', '$'])
            || hyper::header::HeaderValue::from_str(value).is_err()
        {
            return Err(Rejection::InvalidValue);
        }
        if value.split(';').any(|directive| {
            directive
                .split_ascii_whitespace()
                .next()
                .and_then(|name| name.get(..7))
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("report-"))
        }) {
            return Err(Rejection::Reporting);
        }
    }
    Ok(())
}

// The matcher forbids content-type modifiers on CSP but can index methods.
// Use that condition only in the CSP query, where the two kinds are distinct.
pub(crate) const DOCUMENT_METHOD: &str = "GET";
pub(crate) const SUBDOCUMENT_METHOD: &str = "POST";

pub(super) fn project(options: &Options<'_>) -> Option<String> {
    let subdocument = |token: super::options::OptionToken<'_>| {
        token.raw_name == "subdocument" && token.value.is_none()
    };
    (options.tokens().any(|token| token.raw_name == CSP) && options.tokens().any(subdocument)).then(
        || {
            options.write(|token| {
                if subdocument(token) {
                    "method=POST"
                } else {
                    token.raw
                }
            })
        },
    )
}
