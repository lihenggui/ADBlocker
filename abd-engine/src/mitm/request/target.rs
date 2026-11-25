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

use std::borrow::Cow;

use hyper::body::Incoming;
use hyper::header::HOST;
use hyper::http::uri::{Authority, PathAndQuery};
use hyper::{Method, Request, Response, StatusCode};

use super::response::empty;
use super::{Body, InspectedSite, Shared};
use crate::filter::AppFilters;
use crate::stats::Counter;

pub(in crate::mitm) fn clear_target(
    method: &Method,
    target: &str,
    host: &str,
) -> Option<(String, String)> {
    crate::public_suffix::install();
    if *method == Method::CONNECT
        || !target.starts_with('/')
        || target.contains('#')
        || host.contains('@')
    {
        return None;
    }
    let path: PathAndQuery = target.parse().ok()?;
    let authority: Authority = host.parse().ok()?;
    if authority.as_str().ends_with(':')
        || (authority.port().is_some() && authority.port_u16() != Some(80))
    {
        return None;
    }
    let url = format!("http://{authority}{path}");
    adblock::url_parser::parse_url(&url)?;
    let hostname = authority
        .host()
        .strip_suffix('.')
        .unwrap_or(authority.host())
        .to_ascii_lowercase();
    Some((url, hostname))
}

/// Each request must still belong to the site verified at the TLS handshake.
fn address<B>(req: &Request<B>, sni: &str, port: u16) -> Option<String> {
    if req
        .uri()
        .scheme_str()
        .is_some_and(|scheme| scheme != "https")
    {
        return None;
    }
    let mut hosts = req.headers().get_all(HOST).iter();
    let host = hosts.next().map(|value| value.to_str()).transpose().ok()?;
    if hosts.next().is_some() {
        return None;
    }
    let authority = req.uri().authority().map(Authority::as_str);
    for raw in [authority, host].into_iter().flatten() {
        let value: Authority = raw.parse().ok()?;
        if !value
            .host()
            .strip_suffix('.')
            .unwrap_or(value.host())
            .eq_ignore_ascii_case(sni)
            || raw.contains('@')
            || (raw != value.host() && value.port_u16() != Some(port))
        {
            return None;
        }
    }
    let path = req.uri().path_and_query().map_or("/", |p| p.as_str());
    Some(format!(
        "https://{}{path}",
        authority.or(host).unwrap_or(sni)
    ))
}

pub(super) fn guard_site(
    shared: &Shared,
    version: hyper::Version,
    selected: bool,
    url_filtering: bool,
) -> Option<Response<Body>> {
    if let Some(site) = &shared.https {
        let Shared { ctx, package, .. } = shared;
        let host = &site.name;
        let exclusions = ctx.lists.exclusions.read().unwrap();
        if !selected
            || !ctx.can_inspect_https(url_filtering)
            || !exclusions.ready()
            || site
                .profile
                .as_ref()
                .is_some_and(|(serial, tls)| !ctx.selection.profile_tls_is_current(*serial, tls))
            || exclusions.covers(package, host)
            || ctx.lists.allow.read().unwrap().covers(host)
        {
            // A request on a previously terminated connection cannot be
            // handed back to the origin as the app's original TLS session.
            // Ending the upstream driver also asks the downstream HTTP/2
            // driver to send GOAWAY. Further requests can use a fresh session.
            site.stop.abort();
            let mut response = empty(StatusCode::SERVICE_UNAVAILABLE);
            if version == hyper::Version::HTTP_11 {
                response.headers_mut().insert(
                    hyper::header::CONNECTION,
                    hyper::header::HeaderValue::from_static("close"),
                );
            }
            return Some(response);
        }
    }
    None
}

pub(super) fn guard_domain(
    shared: &Shared,
    filters: &AppFilters<'_>,
    host: Option<&str>,
    url_filtering: bool,
) -> Option<Response<Body>> {
    let host = host.filter(|_| !url_filtering)?;
    if !filters.blocks_domain(host) {
        return None;
    }
    let Shared { ctx, package, .. } = shared;
    ctx.services
        .stats
        .count(Some(package), Counter::HostsBlocked);
    ctx.services.live.host_blocked(package, host);
    Some(empty(StatusCode::FORBIDDEN))
}

/// The address a request is for and its host: for a decrypted one, the
/// site's, borrowed from it.
pub(super) fn target_of<'s>(
    req: &Request<Incoming>,
    site: Option<&'s InspectedSite>,
) -> Result<Option<(String, Cow<'s, str>)>, StatusCode> {
    let target = if let Some(site) = site {
        let Some(url) = address(req, &site.name, site.port) else {
            return Err(StatusCode::MISDIRECTED_REQUEST);
        };
        Some((url, Cow::Borrowed(site.name.as_str())))
    } else if req.uri().authority().is_none() && req.uri().scheme().is_none() {
        let mut hosts = req.headers().get_all(HOST).iter();
        let host = hosts.next().and_then(|h| h.to_str().ok());
        let path = req.uri().path_and_query().map(PathAndQuery::as_str);
        if hosts.next().is_none() {
            path.zip(host)
                .and_then(|(path, host)| clear_target(req.method(), path, host))
                .map(|(url, host)| (url, Cow::Owned(host)))
        } else {
            None
        }
    } else {
        None
    };
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HOST as HOST_HEADER;

    const HOST: &str = "site.test";

    #[test]
    fn an_https_request_can_only_name_the_verified_site() {
        let get = |uri: &str, host: &str| {
            Request::get(uri)
                .header(HOST_HEADER, host)
                .body(())
                .unwrap()
        };
        assert_eq!(
            address(&get("/a?b=1", "site.test"), HOST, 443).as_deref(),
            Some("https://site.test/a?b=1")
        );
        for req in [
            get("/a", "other.test"),
            get("https://other.test/a", HOST),
            get("https://site.test/a", "other.test"),
            get("http://site.test/a", HOST),
            get("/a", "site.test:81"),
            get("/a", "user@site.test"),
        ] {
            assert!(address(&req, HOST, 443).is_none());
        }
    }
}
