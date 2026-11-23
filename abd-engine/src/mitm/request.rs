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
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::header::{ACCEPT_ENCODING, CONTENT_LENGTH, HOST, ORIGIN, RANGE, REFERER, UPGRADE};
use hyper::http::uri::{Authority, PathAndQuery};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;

use super::upstream::{LINGER_TIME, Upstream};
use super::{Ctx, trace};
use crate::filter::{Filters, Hiding};
use crate::relay;
use crate::stats::Counter;
use crate::tls::Tls;

type Body = BoxBody<Bytes, hyper::Error>;

/// What kind of resource a request is for, as the filter options name them.
pub(super) fn kind<B>(req: &Request<B>) -> &'static str {
    let h = |n: &str| {
        req.headers()
            .get(n)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    };
    match h("sec-fetch-dest") {
        "document" => "document",
        "iframe" | "frame" => "subdocument",
        "script" | "worker" | "sharedworker" | "serviceworker" => "script",
        "style" => "stylesheet",
        "image" => "image",
        "font" => "font",
        "audio" | "video" | "track" => "media",
        "object" | "embed" => "object",
        "empty" => "xmlhttprequest",
        _ => {
            let path = req.uri().path();
            let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
            match ext {
                "js" | "mjs" => "script",
                "css" => "stylesheet",
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "avif" => "image",
                "woff" | "woff2" | "ttf" | "otf" => "font",
                "mp4" | "webm" | "m3u8" | "ts" | "mp3" | "m4a" => "media",
                _ => "other",
            }
        }
    }
}

fn is_document<B>(req: &Request<B>) -> bool {
    req.headers().get("sec-fetch-dest").map(|v| v.as_bytes()) == Some(b"document")
}

/// The page a request says it was made by, if it says.
fn told_page<B>(req: &Request<B>) -> Option<(&str, adblock::url_parser::RequestUrl)> {
    [REFERER, ORIGIN].into_iter().find_map(|name| {
        let value = req.headers().get(name)?.to_str().ok()?;
        Some((value, crate::filter::parse_http_page(value)?))
    })
}

pub(super) fn clear_target(method: &Method, target: &str, host: &str) -> Option<(String, String)> {
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
pub(super) fn address<B>(req: &Request<B>, sni: &str, port: u16) -> Option<String> {
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

pub(super) struct InspectedSite {
    pub(super) name: String,
    pub(super) port: u16,
    pub(super) profile: Option<(u32, Arc<Tls>)>,
    pub(super) stop: tokio::task::AbortHandle,
}

/// What the requests on one parsed connection have in common.
pub(super) struct Shared {
    pub(super) ctx: Arc<Ctx>,
    /// The TLS server's name, where the request was decrypted.
    pub(super) https: Option<InspectedSite>,
    pub(super) uid: u32,
    pub(super) package: String,
    /// The joining of the two sides once the connection has switched
    /// protocols, for `mitm` to wait on.
    pub(super) tunnel: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Evaluate the metadata of an HTTP request using the URL validated by its transport.
pub fn evaluate_request<'a, B>(
    filters: &Filters,
    req: &'a Request<B>,
    url: &str,
) -> RequestDecision<'a> {
    evaluate(filters, req, url, kind(req))
}

fn evaluate<'a, B>(
    filters: &Filters,
    req: &'a Request<B>,
    url: &str,
    kind: &'static str,
) -> RequestDecision<'a> {
    let page = told_page(req);
    let blocked = filters
        .check_with_page(
            url,
            page.as_ref().map(|(_, page)| page),
            is_document(req),
            kind,
            req.method().as_str(),
        )
        .blocked;
    RequestDecision {
        blocked,
        kind,
        page: page.map(|(value, _)| value),
    }
}

pub struct RequestDecision<'a> {
    pub blocked: bool,
    pub kind: &'static str,
    pub page: Option<&'a str>,
}

fn record_request(ctx: &Ctx, package: &str, host: &str, url: &str, decision: RequestDecision<'_>) {
    let RequestDecision {
        blocked,
        kind,
        page,
    } = decision;
    ctx.services
        .live
        .request(package, host, blocked, url, page, kind);
    let count = if blocked {
        Counter::RequestsBlocked
    } else {
        Counter::RequestsAllowed
    };
    ctx.services.stats.count(Some(package), count);
    trace!(ctx, "request {package} [{kind}] {url} blocked={blocked}");
}

pub(super) async fn serve<'a>(
    shared: &Shared,
    mut req: Request<Incoming>,
    upstream: impl Future<Output = anyhow::Result<&'a Upstream>>,
) -> anyhow::Result<Response<Body>> {
    let Shared {
        ctx,
        https,
        uid,
        package,
        ..
    } = shared;
    let selected = ctx.is_package(*uid, package);
    let url_filtering = ctx.selection.url_filtering.load(Relaxed);
    let filters = ctx.filters();
    if let Some(response) = guard_site(shared, req.version(), selected, url_filtering) {
        return Ok(response);
    }
    let target = match target_of(&req, https.as_ref()) {
        Ok(target) => target,
        Err(status) => return Ok(empty(status)),
    };
    let kind = kind(&req);
    let url = if !url_filtering || !selected {
        ""
    } else {
        target.as_ref().map_or("", |(url, _)| url.as_str())
    };
    let decision = if let Some((url, host)) = &target
        && selected
        // `guard_site` found a decrypted request's site untrusted, and it
        // can name no other (`address`).
        && (https.is_some() || !ctx.lists.allow.read().unwrap().covers(host))
    {
        if url_filtering {
            let decision = evaluate(&filters, &req, url, kind);
            if decision.blocked {
                record_request(ctx, package, host, url, decision);
                return Ok(empty(StatusCode::FORBIDDEN));
            }
            Some((url, host, decision))
        } else {
            if filters.blocks_domain(host) {
                ctx.services
                    .stats
                    .count(Some(package), Counter::HostsBlocked);
                ctx.services.live.host_blocked(package, host);
                return Ok(empty(StatusCode::FORBIDDEN));
            }
            None
        }
    } else {
        None
    };
    // An error opening the upstream ends the connection before any HTTP response.
    let upstream = upstream.await?;
    if let Some((url, host, decision)) = decision {
        record_request(ctx, package, host, url, decision);
    }
    let hiding = hiding_for(shared, &filters, &mut req, url, kind);

    // A WebSocket starts as a request asking to switch protocols; once the
    // server agrees the two connections are joined and no longer read.
    let connect_tunnel = https.is_none() && req.method() == Method::CONNECT;
    let app_upgrade = (connect_tunnel || req.headers().contains_key(UPGRADE))
        .then(|| hyper::upgrade::on(&mut req));
    let mut resp = match forward(upstream, req).await {
        Ok(r) => r,
        Err(e) => {
            trace!(ctx, "error   {package} {url}: {e}");
            return Ok(empty(StatusCode::BAD_GATEWAY));
        }
    };
    join_upgrade(shared, &mut resp, app_upgrade, connect_tunnel);
    Ok(hide(&filters, resp, hiding).await)
}

fn guard_site(
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

/// The address a request is for and its host: for a decrypted one, the
/// site's, borrowed from it.
fn target_of<'s>(
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

fn hiding_for(
    shared: &Shared,
    filters: &Filters,
    req: &mut Request<Incoming>,
    url: &str,
    kind: &str,
) -> Option<Hiding> {
    // The style that hides what the lists name on this page, where the
    // user has pages changed for it and this is the whole of a page. It
    // is put into a page that is not packed: asked for so, one request in
    // very many, rather than unpacked here from whatever a server sends.
    let whole_page = matches!(kind, "document" | "subdocument")
        && req.method() == hyper::Method::GET
        && !req.headers().contains_key(RANGE);
    let hiding =
        (shared.https.is_some() && whole_page && shared.ctx.selection.cosmetic.load(Relaxed))
            .then(|| filters.hiding(url))
            .filter(|hiding| hiding.any());
    if hiding.is_some() {
        let plain = hyper::header::HeaderValue::from_static("identity");
        req.headers_mut().insert(ACCEPT_ENCODING, plain);
    }

    hiding
}

async fn forward(
    upstream: &Upstream,
    req: Request<Incoming>,
) -> Result<Response<Incoming>, hyper::Error> {
    match upstream {
        Upstream::H2(tx) => tx.clone().send_request(req).await,
        Upstream::H1(tx) => {
            let mut tx = tx.lock().await;
            // Until the previous response has been read to its end.
            match tx.ready().await {
                Ok(()) => tx.send_request(req).await,
                Err(e) => Err(e),
            }
        }
    }
}

fn join_upgrade(
    shared: &Shared,
    resp: &mut Response<Incoming>,
    app_upgrade: Option<hyper::upgrade::OnUpgrade>,
    connect_tunnel: bool,
) {
    if (resp.status() == StatusCode::SWITCHING_PROTOCOLS
        || (connect_tunnel && resp.status().is_success()))
        && let Some(app_upgrade) = app_upgrade
    {
        let server_upgrade = hyper::upgrade::on(resp);
        let joined = tokio::spawn(async move {
            if let (Ok(app), Ok(server)) = (app_upgrade.await, server_upgrade.await) {
                relay::join_streams(TokioIo::new(app), TokioIo::new(server), LINGER_TIME).await;
            }
        });
        *shared.tunnel.lock().unwrap() = Some(joined);
    }
}

async fn hide(
    filters: &Arc<Filters>,
    resp: Response<Incoming>,
    hiding: Option<Hiding>,
) -> Response<Body> {
    use crate::cosmetic::{ByName, Inject, Inline, Prefix, Preparation};

    let Some(mut hiding) = hiding.filter(|_| crate::cosmetic::takes(&resp)) else {
        return resp.map(BodyExt::boxed);
    };
    let (mut parts, body) = resp.into_parts();
    // The generic style is decided using the original policy, before
    // any head allowance for a specific style is added to it.
    let (looked_through, nonce) = match crate::cosmetic::inline(&parts.headers) {
        Inline::Any => (hiding.by_name(), None),
        Inline::Nonce(nonce) => (hiding.by_name(), Some(nonce)),
        Inline::No => (false, None),
    };
    let mut body = Prefix::new(body);
    let css = hiding.css.take();
    let mut head_css = css.as_deref();
    if let Some(script) = hiding.script.take() {
        match body.prepare(&mut parts.headers, &script, head_css).await {
            Preparation::Injected => head_css = None,
            Preparation::CssOnly if css.is_none() && !looked_through => {
                return Response::from_parts(parts, body.boxed());
            }
            Preparation::NoHead => return Response::from_parts(parts, body.boxed()),
            Preparation::CssOnly => {}
        }
    }
    parts.headers.remove(CONTENT_LENGTH);
    if let Some(css) = &css {
        crate::cosmetic::permit(&mut parts.headers, css);
    }
    let by_name: Option<ByName> = looked_through.then(|| {
        let filters = filters.clone();
        let by_name: ByName =
            Box::new(move |names| filters.hiding_by_name(&hiding, &names.classes, &names.ids));
        by_name
    });
    Response::from_parts(parts, Inject::new(body, head_css, by_name, nonce).boxed())
}

fn empty(status: StatusCode) -> Response<Body> {
    let mut resp = Response::new(Full::new(Bytes::new()).map_err(|e| match e {}).boxed());
    *resp.status_mut() = status;
    resp
}

pub(super) async fn finish_tunnel(shared: &Shared) {
    let tunnel = shared.tunnel.lock().unwrap().take();
    if let Some(tunnel) = tunnel {
        let _ = tunnel.await;
    }
}
