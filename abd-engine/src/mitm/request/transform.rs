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
use std::collections::HashSet;
use std::net::Ipv6Addr;
use std::sync::Arc;

use hyper::header::HeaderValue;
use hyper::http::uri::PathAndQuery;
use hyper::{Method, Uri};

use crate::filter::removeparam;
use crate::limits::MAX_TRANSFORM_URL;
use crate::rule::action::ResponseAction;

pub(super) enum Transformed {
    Forward { url: String, uri: Uri },
    Redirect(HeaderValue),
}

/// The matcher selects all URL changes from the original request. Apply
/// native parameter removal first, then the selected transforms in source order.
pub(super) fn apply(
    original: &str,
    target: &Uri,
    method: &Method,
    rewritten: Option<&str>,
    transforms: &[Arc<ResponseAction>],
) -> Option<Transformed> {
    if (rewritten.is_none() && transforms.is_empty()) || original.len() > MAX_TRANSFORM_URL {
        return None;
    }
    let previous = validated_url(original)?;
    let parameters = remove_parameters(original, &previous, rewritten, transforms);
    let redirects = matches!(*method, Method::GET | Method::HEAD);
    let url = transform_url(&parameters, redirects, transforms)
        .unwrap_or_else(|| Cow::Borrowed(parameters.as_ref()));
    if url == original {
        return None;
    }
    let parsed = validated_url(&url)?;

    if !same_origin(&previous, &parsed) {
        return HeaderValue::from_str(&url).ok().map(Transformed::Redirect);
    }
    let (origin, rest) = split_url(&url)?;
    let path = request_path(rest);
    let mut parts = target.clone().into_parts();
    let path = path_and_query(&path)?;
    if parts.path_and_query.as_ref() == Some(&path) {
        return None;
    }
    let url = format!("{origin}{path}");
    if url.len() > MAX_TRANSFORM_URL {
        return None;
    }
    parts.path_and_query = Some(path);
    Some(Transformed::Forward {
        url,
        uri: Uri::from_parts(parts).ok()?,
    })
}

fn remove_parameters<'a>(
    original: &'a str,
    previous: &Uri,
    rewritten: Option<&'a str>,
    actions: &[Arc<ResponseAction>],
) -> Cow<'a, str> {
    let Some(rewritten) =
        rewritten.filter(|url| validated_url(url).is_some_and(|uri| same_origin(previous, &uri)))
    else {
        return Cow::Borrowed(original);
    };
    let exempt: HashSet<_> = actions
        .iter()
        .filter_map(|action| match action.as_ref() {
            ResponseAction::RemoveParamException(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    if exempt.is_empty() {
        Cow::Borrowed(rewritten)
    } else {
        removeparam::preserve(original, rewritten, &exempt)
            .map_or(Cow::Borrowed(original), Cow::Owned)
    }
}

fn transform_url<'a>(
    original: &'a str,
    redirects: bool,
    transforms: &[Arc<ResponseAction>],
) -> Option<Cow<'a, str>> {
    let mut url = Cow::Borrowed(original);
    for action in transforms {
        let ResponseAction::UrlTransform(action) = action.as_ref() else {
            continue;
        };
        if action.whole_url {
            if !redirects {
                continue;
            }
            if let Cow::Owned(changed) = action.apply(&url)? {
                validated_url(&changed)?;
                url = Cow::Owned(changed);
            }
        } else {
            let (origin, rest) = split_url(&url)?;
            let fragment = rest.split_once('#').map(|(_, fragment)| fragment);
            let path = request_path(rest);
            if let Cow::Owned(changed) = action.apply(&path)? {
                path_and_query(&changed)?;
                let mut changed = format!("{origin}{changed}");
                if let Some(fragment) = fragment {
                    changed.push('#');
                    changed.push_str(fragment);
                }
                validated_url(&changed)?;
                url = Cow::Owned(changed);
            }
        }
    }
    Some(url)
}

fn split_url(url: &str) -> Option<(&str, &str)> {
    let host = url.find("://")? + 3;
    let end = url[host..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |end| host + end);
    Some(url.split_at(end))
}

/// An absolute URL can omit its slash, but the forwarded target needs one.
fn request_path(rest: &str) -> Cow<'_, str> {
    let path = rest.split_once('#').map_or(rest, |(path, _)| path);
    if path.starts_with('/') {
        Cow::Borrowed(path)
    } else {
        Cow::Owned(format!("/{path}"))
    }
}

fn path_and_query(path: &str) -> Option<PathAndQuery> {
    if !path.starts_with('/') || path.contains('#') {
        return None;
    }
    path.parse().ok()
}

fn validated_url(url: &str) -> Option<Uri> {
    if url.len() > MAX_TRANSFORM_URL
        || url.contains('\\')
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let (origin, _) = split_url(url)?;
    let uri: Uri = url.parse().ok()?;
    let authority = uri.authority()?;
    let host = authority.host();
    let host_valid = if let Some(ip) = host.strip_prefix('[').and_then(|ip| ip.strip_suffix(']')) {
        ip.parse::<Ipv6Addr>().is_ok()
    } else {
        crate::host::is_host(host)
    };
    (matches!(uri.scheme_str(), Some("http" | "https"))
        && host_valid
        && !origin.contains('@')
        && (authority.as_str() == host || authority.port_u16().is_some()))
    .then_some(uri)
}

fn same_origin(first: &Uri, second: &Uri) -> bool {
    fn port(uri: &Uri) -> u16 {
        uri.port_u16()
            .unwrap_or(if uri.scheme_str() == Some("https") {
                443
            } else {
                80
            })
    }
    first.scheme() == second.scheme()
        && first
            .host()
            .zip(second.host())
            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
        && port(first) == port(second)
}
