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

use hyper::header::HeaderName;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Removal<T> {
    Response(T),
    Request(T),
}

pub(crate) type Spec = Removal<String>;
pub(crate) type Compiled = Removal<HeaderName>;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Rejection {
    InvalidName,
    ProtectedByAdGuard,
    RequiredForRequest,
    RequiredForResponse,
}

// From the AdGuard filter reference's `$removeheader` restrictions.
const PROTECTED: &[&str] = &[
    "access-control-allow-origin",
    "access-control-allow-credentials",
    "access-control-allow-headers",
    "access-control-allow-methods",
    "access-control-expose-headers",
    "access-control-max-age",
    "access-control-request-headers",
    "access-control-request-method",
    "origin",
    "timing-allow-origin",
    "allow",
    "cross-origin-embedder-policy",
    "cross-origin-opener-policy",
    "cross-origin-resource-policy",
    "content-security-policy",
    "content-security-policy-report-only",
    "expect-ct",
    "feature-policy",
    "origin-isolation",
    "strict-transport-security",
    "upgrade-insecure-requests",
    "x-content-type-options",
    "x-download-options",
    "x-frame-options",
    "x-permitted-cross-domain-policies",
    "x-powered-by",
    "x-xss-protection",
    "public-key-pins",
    "public-key-pins-report-only",
    "sec-websocket-key",
    "sec-websocket-extensions",
    "sec-websocket-accept",
    "sec-websocket-protocol",
    "sec-websocket-version",
    "p3p",
    "sec-fetch-mode",
    "sec-fetch-dest",
    "sec-fetch-site",
    "sec-fetch-user",
    "referrer-policy",
    "content-type",
    "content-length",
    "accept",
    "accept-encoding",
    "host",
    "connection",
    "transfer-encoding",
    "upgrade",
];

// Requests need routing, framing, decoding and the controls for upgrades, trailers and 100-continue.
const REQUEST: &[&str] = &[
    "host",
    "content-encoding",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
    "expect",
];
// Responses need framing and the controls for decoding, upgrades and trailers.
const RESPONSE: &[&str] = &[
    // Also protect page security policies beyond the reference's list.
    "permissions-policy",
    "clear-site-data",
    "content-length",
    "transfer-encoding",
    "content-encoding",
    "connection",
    "upgrade",
    "trailer",
];

pub(super) fn parse(value: &str) -> Result<Spec, Rejection> {
    let compiled = match value.split_once(':') {
        Some((prefix, name)) if prefix.eq_ignore_ascii_case("request") => {
            compile_name(name, REQUEST, Rejection::RequiredForRequest).map(Removal::Request)
        }
        _ => compile_name(value, RESPONSE, Rejection::RequiredForResponse).map(Removal::Response),
    }?;
    Ok(match compiled {
        Removal::Request(name) => Removal::Request(name.as_str().to_owned()),
        Removal::Response(name) => Removal::Response(name.as_str().to_owned()),
    })
}

impl Spec {
    pub(super) fn compile(&self) -> Result<Compiled, Rejection> {
        match self {
            Self::Request(name) => {
                compile_name(name, REQUEST, Rejection::RequiredForRequest).map(Removal::Request)
            }
            Self::Response(name) => {
                compile_name(name, RESPONSE, Rejection::RequiredForResponse).map(Removal::Response)
            }
        }
    }
}

fn compile_name(name: &str, required: &[&str], reason: Rejection) -> Result<HeaderName, Rejection> {
    let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| Rejection::InvalidName)?;
    if PROTECTED.contains(&name.as_str()) {
        return Err(Rejection::ProtectedByAdGuard);
    }
    if required.contains(&name.as_str()) {
        return Err(reason);
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_encoding_and_response_security_policies_are_protected() {
        for name in [
            "permissions-policy",
            "clear-site-data",
            "request:content-encoding",
        ] {
            assert!(parse(name).is_err(), "{name}");
        }
    }

    #[test]
    fn names_are_http_tokens_and_values_identify_direction_and_name() {
        assert!(parse("LoCaTiOn") == Ok(Removal::Response("location".into())));
        assert!(parse("ReQuEsT:LoCaTiOn") == Ok(Removal::Request("location".into())));
        for value in [
            "",
            "request:",
            "response:x-test",
            "x test",
            "x,foo",
            "é",
            "request: x-test",
        ] {
            assert!(
                matches!(parse(value), Err(Rejection::InvalidName)),
                "{value}"
            );
        }
    }

    #[test]
    fn the_reference_protects_headers_in_both_directions() {
        for &name in PROTECTED {
            let name = name.to_ascii_uppercase();
            assert!(
                matches!(parse(&name), Err(Rejection::ProtectedByAdGuard)),
                "{name}"
            );
            assert!(
                matches!(
                    parse(&format!("request:{name}")),
                    Err(Rejection::ProtectedByAdGuard)
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn transport_headers_are_protected_in_the_direction_that_uses_them() {
        for name in ["te", "expect"] {
            assert!(
                matches!(
                    parse(&format!("request:{name}")),
                    Err(Rejection::RequiredForRequest)
                ),
                "{name}"
            );
            assert!(parse(name).is_ok(), "{name}");
        }
        assert!(matches!(
            parse("trailer"),
            Err(Rejection::RequiredForResponse)
        ));
        assert!(parse("request:trailer").is_ok());
        assert!(matches!(
            parse("content-encoding"),
            Err(Rejection::RequiredForResponse)
        ));
        assert!(matches!(
            parse("request:content-encoding"),
            Err(Rejection::RequiredForRequest)
        ));
    }
}
