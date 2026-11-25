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

//! Read address rules separately from URL and host rules.

use std::net::{IpAddr, SocketAddr};

use regex::RegexBuilder;

use super::modifiers::{APP, Compatibility, Kind, NETWORK, lookup};
use super::options::Options;
use crate::limits::{MAX_ADDRESS_EXPRESSION, MAX_ADDRESS_REGEX};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rejection {
    Modifiers,
    Endpoint,
    Expression,
}

pub(crate) enum Pattern<'a> {
    Exact(IpAddr, Option<u16>),
    Expression(&'a str),
}

pub(crate) struct AddressRule<'a> {
    pub(crate) pattern: Pattern<'a>,
    pub(crate) apps: Option<&'a str>,
    pub(crate) important: bool,
    pub(crate) exception: bool,
}

impl<'a> AddressRule<'a> {
    pub(super) fn read(line: &str, options: &Options<'a>) -> Option<Result<Self, Rejection>> {
        let Kind::Address(compatibility) = lookup(NETWORK)?.kind else {
            return None;
        };
        (options.tokens().any(|token| token.name() == NETWORK)
            && (super::options::regex_option_separator(line).is_some()
                || super::network_condition(line, options.pattern)))
        .then(|| Self::parse(options, compatibility))
    }

    fn parse(options: &Options<'a>, compatibility: Compatibility) -> Result<Self, Rejection> {
        let mut network = false;
        let mut apps = None;
        let mut important = false;
        if options.noop {
            return Err(Rejection::Modifiers);
        }
        for token in options.tokens() {
            if !compatibility.accepts(token.raw_name, token.value) {
                return Err(Rejection::Modifiers);
            }
            match token.raw_name {
                NETWORK if !network => network = true,
                APP if apps.is_none() => apps = token.value,
                "important" if !important => important = true,
                _ => return Err(Rejection::Modifiers),
            }
        }
        let exception = options.pattern.starts_with("@@");
        let pattern = options
            .pattern
            .strip_prefix("@@")
            .unwrap_or(options.pattern);
        Ok(Self {
            pattern: Pattern::parse(pattern)?,
            apps,
            important,
            exception,
        })
    }
}

impl<'a> Pattern<'a> {
    fn parse(text: &'a str) -> Result<Self, Rejection> {
        if let Some(body) = text.strip_prefix('/') {
            let expression = body.strip_suffix('/').ok_or(Rejection::Expression)?;
            return valid_expression(expression)
                .then_some(Self::Expression(expression))
                .ok_or(Rejection::Expression);
        }
        // The reference does not show these prefixes, but AdGuard's lists use them.
        let text = text
            .strip_prefix("||")
            .or_else(|| text.strip_prefix('|'))
            .unwrap_or(text);
        let text = text.strip_suffix('^').unwrap_or(text);
        if let Ok(ip) = text.parse::<std::net::Ipv4Addr>() {
            return Ok(Self::Exact(ip.into(), None));
        }
        if let Some(ip) = text.strip_prefix('[').and_then(|ip| ip.strip_suffix(']')) {
            let ip = ip
                .parse::<std::net::Ipv6Addr>()
                .map_err(|_| Rejection::Endpoint)?;
            return Ok(Self::Exact(ip.into(), None));
        }
        // A zone ID identifies a local interface, not a remote IP.
        if !text.contains('%')
            && let Ok(endpoint) = text.parse::<SocketAddr>()
        {
            return Ok(Self::Exact(endpoint.ip(), Some(endpoint.port())));
        }
        Err(Rejection::Endpoint)
    }
}

pub(crate) fn valid_expression(body: &str) -> bool {
    body.len() <= MAX_ADDRESS_EXPRESSION
        && super::quoting::delimiter(body).is_none()
        && RegexBuilder::new(body)
            .size_limit(MAX_ADDRESS_REGEX)
            .dfa_size_limit(MAX_ADDRESS_REGEX)
            .build()
            .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{Kind, Reason, read};

    #[test]
    fn exact_patterns_accept_both_ip_families_with_optional_ports_and_separators() {
        for (text, address, port) in [
            ("192.0.2.1", "192.0.2.1", None),
            ("192.0.2.1^", "192.0.2.1", None),
            ("192.0.2.1:443", "192.0.2.1", Some(443)),
            ("192.0.2.1:443^", "192.0.2.1", Some(443)),
            ("|192.0.2.1:443^", "192.0.2.1", Some(443)),
            ("||192.0.2.1^", "192.0.2.1", None),
            ("[2001:db8::1]", "2001:db8::1", None),
            ("[2001:0DB8:0:0:0:0:0:1]^", "2001:db8::1", None),
            ("[2001:db8::1]:443", "2001:db8::1", Some(443)),
            ("[2001:0DB8:0:0:0:0:0:1]:443^", "2001:db8::1", Some(443)),
            ("|[2001:db8::1]", "2001:db8::1", None),
            ("||[2001:db8::1]:443^", "2001:db8::1", Some(443)),
        ] {
            let line = format!("{text}$network");
            let Kind::Address(rule) = read(&line).kind else {
                panic!("{line}")
            };
            assert!(
                matches!(rule.pattern, Pattern::Exact(ip, p) if ip == address.parse::<IpAddr>().unwrap() && p == port)
            );
        }
    }

    #[test]
    fn address_options_are_read_after_the_complete_expression() {
        let line = r"@@/^192\.0\.2\.1:(443|8443)$|##/$important,app=/com\.reader\$/,network";
        let parsed = read(line);
        let Kind::Address(ref rule) = parsed.kind else {
            panic!("{line}")
        };
        assert!(matches!(
            rule.pattern,
            Pattern::Expression(r"^192\.0\.2\.1:(443|8443)$|##")
        ));
        assert_eq!(rule.apps, Some(r"/com\.reader\$/"));
        assert!(rule.exception && rule.important);
        assert!(parsed.matcher_input().is_none());
        assert!(parsed.network().is_none());
    }

    fn rejected(line: &str, reason: Rejection) {
        let rule = read(line);
        assert!(
            matches!(&rule.kind, Kind::NotApplied(Reason::Address(found)) if *found == reason),
            "{line}"
        );
        assert!(rule.matcher_input().is_none(), "{line}");
    }

    #[test]
    fn hostnames_cidrs_wildcards_and_malformed_endpoints_are_invalid() {
        for pattern in [
            "ads.example",
            "||ads.example^",
            "|||192.0.2.1^",
            "||192.0.2.1|",
            "||192.0.2.1/path",
            "||192.0.2.*^",
            r"|/192\.0\.2\.1/",
            "192.0.2.0/24",
            "[2001:db8::]/32",
            "192.0.2.*",
            "192.0.2.999",
            "192.0.2.1:",
            "192.0.2.1:65536",
            "192.0.2.1^^",
            "2001:db8::1",
            "[2001:db8::1",
            "[fe80::1%1]:443",
            "https://192.0.2.1",
        ] {
            rejected(&format!("{pattern}$network"), Rejection::Endpoint);
        }
    }

    #[test]
    fn malformed_and_overbudget_address_expressions_are_invalid() {
        for expr in ["/[a-/", "/unterminated", r"/\w{100000}/"] {
            rejected(&format!("{expr}$network"), Rejection::Expression);
        }
        let long = "a".repeat(MAX_ADDRESS_EXPRESSION + 1);
        rejected(&format!("/{long}/$network"), Rejection::Expression);
    }

    #[test]
    fn address_rules_reject_every_option_outside_their_qualifiers() {
        for suffix in [
            "network,script",
            "image,network",
            "domain=site.test,network",
            "network,badfilter",
            "network,_",
            "network,denyallow=a.test",
            "network,unknown",
            "network,match-case",
            "network,replace=/a/b/",
            "network,app",
            "network,app=",
            "network,~app=com.app",
            "network,important=yes",
            "network,~important",
            "~network",
            "network=yes",
            "network,network",
            "network,important,important",
            "network,app=com.a,app=com.b",
        ] {
            rejected(&format!("192.0.2.1${suffix}"), Rejection::Modifiers);
        }
    }
}
