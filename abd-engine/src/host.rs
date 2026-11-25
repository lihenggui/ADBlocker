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

use std::net::Ipv6Addr;

use hyper::http::uri::Authority;

/// A host name or an address, and nothing a rule could hide in.
pub fn is_host(host: &str) -> bool {
    !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'))
}

pub(crate) struct CanonicalAuthority {
    pub host: String,
    pub port: u16,
    pub text: String,
    pub explicit_port: bool,
}

/// The one spelling used for a request authority in policy and rule URLs.
pub(crate) fn canonical_authority(
    raw: &str,
    scheme: &str,
    connection_port: u16,
) -> Option<CanonicalAuthority> {
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    if raw.contains('@') || raw.ends_with(':') {
        return None;
    }
    let authority: Authority = raw.parse().ok()?;
    let named = authority.host();
    let host = if let Some(address) = named.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        format!("[{}]", address.parse::<Ipv6Addr>().ok()?)
    } else {
        let name = named.strip_suffix('.').unwrap_or(named);
        if !is_host(name)
            || name.contains(':')
            || name.starts_with('.')
            || name.ends_with('.')
            || name.contains("..")
        {
            return None;
        }
        name.to_ascii_lowercase()
    };
    let explicit_port = authority.port().is_some();
    if authority.as_str() != named && !explicit_port {
        return None;
    }
    let port = if explicit_port {
        authority.port_u16()?
    } else {
        connection_port
    };
    let text = if port == default_port {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    Some(CanonicalAuthority {
        host,
        port,
        text,
        explicit_port,
    })
}

pub(crate) fn canonical_http_url(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = if scheme.eq_ignore_ascii_case("http") {
        "http"
    } else if scheme.eq_ignore_ascii_case("https") {
        "https"
    } else {
        return None;
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let default_port = if scheme == "http" { 80 } else { 443 };
    let authority = canonical_authority(&rest[..end], scheme, default_port)?;
    let tail = &rest[end..];
    let path = if tail.starts_with('/') {
        tail.to_owned()
    } else {
        format!("/{tail}")
    };
    Some(format!("{scheme}://{}{path}", authority.text))
}

/// The literal host and its nonempty parents, in lookup order.
pub(crate) fn suffixes(host: &str) -> impl Iterator<Item = &str> {
    std::iter::once(host).chain(
        host.match_indices('.')
            .map(|(dot, _)| &host[dot + 1..])
            .take_while(|parent| !parent.is_empty()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_spellings_share_one_host_and_default_port() {
        for scheme in ["http", "https"] {
            let default = if scheme == "http" { 80 } else { 443 };
            for raw in [
                "site.test",
                "SITE.TEST",
                "site.test.",
                "SITE.TEST.",
                &format!("SITE.TEST.:{default}"),
            ] {
                let name = canonical_authority(raw, scheme, default).unwrap();
                assert_eq!(name.host, "site.test");
                assert_eq!(name.port, default);
                assert_eq!(name.text, "site.test");
            }
        }
        let named = canonical_authority("SITE.TEST:8443", "https", 443).unwrap();
        assert_eq!(
            (named.host.as_str(), named.port, named.text.as_str()),
            ("site.test", 8443, "site.test:8443")
        );
        assert_eq!(
            canonical_authority("[2001:0DB8::0001]:443", "https", 443)
                .unwrap()
                .text,
            "[2001:db8::1]"
        );
    }

    #[test]
    fn malformed_and_non_ascii_authorities_are_refused() {
        for raw in [
            "site.test..",
            "SITE.TEST..:443",
            "site..test",
            "site.test:",
            "user@site.test",
            "sité.test",
            "[2001:db8::zz]",
        ] {
            assert!(canonical_authority(raw, "https", 443).is_none(), "{raw}");
        }
        assert_eq!(
            canonical_http_url("HTTPS://SITE.TEST.:443/ads?q=1").as_deref(),
            Some("https://site.test/ads?q=1")
        );
    }
}
