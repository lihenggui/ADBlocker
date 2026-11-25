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
use hyper::header::{
    ACCEPT_ENCODING, ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_LENGTH,
    CONTENT_SECURITY_POLICY, CONTENT_TYPE, HOST, HeaderName, HeaderValue, ORIGIN, RANGE, REFERER,
    UPGRADE,
};
use hyper::http::uri::{Authority, PathAndQuery};
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;

use super::upstream::{LINGER_TIME, Upstream};
use super::{Ctx, trace};
use crate::filter::{AppFilters, Filters, Hiding, Page, Redirect};
use crate::relay;
use crate::rewrite::RewritePlan;
use crate::rule::action::{Place, html};
use crate::stats::Counter;
use crate::tls::Tls;

type Body = BoxBody<Bytes, hyper::Error>;

/// Whether the request identified its resource type or was judged as `other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    Known(&'static str),
    Assumed,
}

impl RequestKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Known(kind) => kind,
            Self::Assumed => "other",
        }
    }
}

pub(super) fn kind<B>(req: &Request<B>) -> RequestKind {
    let h = |n: &str| {
        req.headers()
            .get(n)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    };
    RequestKind::Known(match h("sec-fetch-dest") {
        "document" => "document",
        "iframe" | "frame" => "subdocument",
        "script" | "worker" | "sharedworker" | "serviceworker" => SCRIPT,
        "style" => STYLESHEET,
        "image" => IMAGE,
        "font" => FONT,
        "audio" | "video" | "track" => MEDIA,
        "object" | "embed" => "object",
        "empty" => XHR,
        _ => {
            let path = req.uri().path();
            let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
            match ext {
                "js" | "mjs" => SCRIPT,
                "css" => STYLESHEET,
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "avif" => IMAGE,
                "woff" | "woff2" | "ttf" | "otf" => FONT,
                "mp4" | "webm" | "m3u8" | "ts" | "mp3" | "m4a" => MEDIA,
                _ => return RequestKind::Assumed,
            }
        }
    })
}

const IMAGE: &str = "image";
const STYLESHEET: &str = "stylesheet";
const SCRIPT: &str = "script";
const MEDIA: &str = "media";
const FONT: &str = "font";
const XHR: &str = "xmlhttprequest";
const RESPONSE_KINDS: &[&str] = &[IMAGE, STYLESHEET, SCRIPT, MEDIA, FONT, XHR];

pub(super) fn response_kind(headers: &HeaderMap) -> Option<RequestKind> {
    let mime = crate::media_type::read(headers)?.to_ascii_lowercase();
    let (group, subtype) = mime.split_once('/')?;
    let kind = match (group, subtype) {
        ("image", _) => IMAGE,
        ("text", "css") => STYLESHEET,
        ("text" | "application", "javascript" | "ecmascript") | ("application", "x-javascript") => {
            SCRIPT
        }
        ("video" | "audio", _)
        | ("application", "vnd.apple.mpegurl" | "x-mpegurl" | "dash+xml") => MEDIA,
        ("font", _) | ("application", "vnd.ms-fontobject") => FONT,
        ("application", subtype)
            if subtype.starts_with("font-") || subtype.starts_with("x-font-") =>
        {
            FONT
        }
        ("text" | "application", "json" | "xml") | ("application", "x-protobuf") => XHR,
        ("application", subtype) if subtype.starts_with("grpc") => XHR,
        (_, subtype) if subtype.ends_with("+json") => XHR,
        _ => return None,
    };
    Some(RequestKind::Known(kind))
}

fn header<'a, B>(req: &'a Request<B>, name: &str) -> Option<&'a [u8]> {
    req.headers().get(name).map(|value| value.as_bytes())
}

/// Whether the request is for a page of its own. A browser says so with
/// `Sec-Fetch-Dest`, which it sends over HTTPS only: over HTTP it asks for
/// an upgrade when it goes to a page, and names no page that led there
/// when the user did.
fn is_document<B>(req: &Request<B>, has_told_page: bool) -> bool {
    match header(req, "sec-fetch-dest") {
        Some(dest) => dest == b"document",
        None => header(req, "upgrade-insecure-requests") == Some(b"1") && !has_told_page,
    }
}

/// What the request says of the page that made it.
fn page_of<'a, B>(req: &Request<B>, told: Option<&'a adblock::url_parser::RequestUrl>) -> Page<'a> {
    if is_document(req, told.is_some()) {
        return Page::Itself;
    }
    match told {
        Some(page) => Page::Named(page),
        // A browser that withholds the page still says whose it is.
        None => Page::Unnamed {
            cross_site: header(req, "sec-fetch-site") == Some(b"cross-site"),
        },
    }
}

/// Whether the request is one of an app with pages: it names its page, is
/// one, or carries what only a browser's engine sends. An exception for
/// named pages can apply to such an app, and to no other.
fn shows_page<B>(req: &Request<B>, page: &Page<'_>) -> bool {
    !matches!(page, Page::Unnamed { .. })
        || ["sec-fetch-site", "sec-fetch-mode", "sec-fetch-dest"]
            .iter()
            .any(|name| req.headers().contains_key(*name))
}

/// The page a request says it was made by, if it says.
fn told_page<B>(req: &Request<B>) -> Option<(&HeaderValue, adblock::url_parser::RequestUrl)> {
    [REFERER, ORIGIN].into_iter().find_map(|name| {
        let value = req.headers().get(name)?;
        Some((value, crate::filter::parse_http_page(value.to_str().ok()?)?))
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
    filters: &AppFilters<'_>,
    req: &'a Request<B>,
    url: &str,
) -> RequestDecision<'a> {
    let told = told_page(req);
    let metadata = RequestMetadata::new(req, req.method(), told.as_ref().map(|(v, p)| (*v, p)));
    metadata.evaluate(filters, url, kind(req)).0
}

/// The policy inputs that must survive moving the request into the transport.
struct RequestMetadata<'a, 't> {
    page: Page<'a>,
    told: Option<&'t str>,
    shows_page: bool,
    method: &'a str,
    permits_actions: bool,
    permits_body_rewrite: bool,
}

impl<'a, 't> RequestMetadata<'a, 't> {
    fn new<B>(
        req: &Request<B>,
        method: &'a Method,
        told: Option<(&'t HeaderValue, &'a adblock::url_parser::RequestUrl)>,
    ) -> Self {
        let page = page_of(req, told.map(|(_, page)| page));
        Self {
            page,
            told: told.and_then(|(value, _)| value.to_str().ok()),
            shows_page: shows_page(req, &page),
            method: method.as_str(),
            permits_actions: permits_actions(req),
            permits_body_rewrite: permits_body_rewrite(req),
        }
    }

    fn evaluate(
        &self,
        filters: &AppFilters<'_>,
        url: &str,
        kind: RequestKind,
    ) -> (RequestDecision<'t>, RewritePlan) {
        let matched = self
            .permits_actions
            .then(|| filters.response_matches(url, self.page, self.method))
            .flatten();
        let actions = matched
            .as_ref()
            .map_or_else(Vec::new, |m| m.actions(kind.as_str()));
        let mut plan = RewritePlan::select(actions, self.permits_body_rewrite);
        // Only replacement rules take precedence after the connection passes host filtering.
        let check = if plan
            .body
            .actions
            .iter()
            .any(|action| action.overrides_block())
        {
            Default::default()
        } else {
            filters.check_with_page(url, self.page, kind.as_str(), self.method)
        };
        let blocked = check.blocked;
        plan.ask_for_identity = !plan.body.actions.is_empty()
            || (!blocked
                && self.permits_body_rewrite
                && kind == RequestKind::Assumed
                && matched.is_some_and(|matches| {
                    RESPONSE_KINDS.iter().any(|kind| {
                        matches
                            .actions(kind)
                            .iter()
                            .any(|action| matches!(action.place(), Place::ResponseBody))
                    })
                }));
        (
            RequestDecision {
                blocked,
                redirect: check.redirect,
                kind,
                page: self.told,
            },
            plan,
        )
    }
}

#[derive(Clone)]
pub struct RequestDecision<'a> {
    pub blocked: bool,
    pub redirect: Option<Redirect>,
    pub kind: RequestKind,
    pub page: Option<&'a str>,
}

struct MatchedRequest<'a> {
    shared: &'a Shared,
    filters: &'a AppFilters<'a>,
    url: &'a str,
    host: &'a str,
    metadata: RequestMetadata<'a, 'a>,
}

impl MatchedRequest<'_> {
    fn evaluate(&self, kind: RequestKind) -> (RequestDecision<'_>, RewritePlan) {
        self.metadata.evaluate(self.filters, self.url, kind)
    }

    fn select_html(
        &self,
        filters: &Filters,
        kind: RequestKind,
        mut plan: RewritePlan,
    ) -> RewritePlan {
        if self.metadata.permits_body_rewrite
            && matches!(
                kind,
                RequestKind::Assumed | RequestKind::Known("document" | "subdocument")
            )
            && self.shared.ctx.selection.cosmetic.load(Relaxed)
        {
            plan.body.html = filters.html_rules(self.host, self.url, self.metadata.method);
            plan.ask_for_identity |= !plan.body.html.is_empty();
        }
        plan
    }

    fn record(&self, decision: &RequestDecision<'_>) {
        let Shared { ctx, package, .. } = self.shared;
        let Self { host, url, .. } = self;
        let RequestDecision {
            blocked,
            kind,
            page,
            ..
        } = *decision;
        let kind = kind.as_str();
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

    fn blocked(&self, decision: RequestDecision<'_>) -> Response<Body> {
        self.record(&decision);
        let Some(redirect) = decision.redirect else {
            return empty(StatusCode::FORBIDDEN);
        };
        let html = redirect.content_type == "text/html";
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, redirect.content_type)
            .header(CONTENT_LENGTH, redirect.body.len())
            .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(CACHE_CONTROL, "no-store")
            .body(Full::new(redirect.body).map_err(|e| match e {}).boxed())
            .expect("fixed redirect response headers");
        if html {
            // Extension HTML can contain relative URLs. At the request's
            // origin those would load the website's files.
            response.headers_mut().insert(
                CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'; sandbox"),
            );
        }
        response
    }
}

/// If forwarding ends without a response head, keep the request-side verdict.
struct PendingVerdict<'a> {
    request: &'a MatchedRequest<'a>,
    decision: Option<RequestDecision<'a>>,
}

impl PendingVerdict<'_> {
    fn finish(
        mut self,
        inferred: RequestKind,
        rewrite: &mut RewritePlan,
    ) -> Option<Response<Body>> {
        let request = self.request;
        // As in AdGuard, the request has already been sent before its response reveals the type.
        let (decision, selected) = request.evaluate(inferred);
        self.decision = None;
        if decision.blocked {
            return Some(request.blocked(decision));
        }
        rewrite.body.actions = selected.body.actions;
        rewrite.response_headers = selected.response_headers;
        request.record(&decision);
        None
    }
}

impl Drop for PendingVerdict<'_> {
    fn drop(&mut self) {
        if let Some(decision) = &self.decision {
            self.request.record(decision);
        }
    }
}

pub(super) async fn serve<'a>(
    shared: &Shared,
    req: Request<Incoming>,
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
    let for_app = ctx.app_filters(&filters, package);
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
    let filtered = target.as_ref().filter(|(_, host)| {
        selected
        // `guard_site` found a decrypted request's site untrusted, and it
        // can name no other (`address`).
        && (https.is_some() || !ctx.lists.allow.read().unwrap().covers(host))
    });
    if !url_filtering
        && let Some((_, host)) = filtered
        && for_app.blocks_domain(host)
    {
        ctx.services
            .stats
            .count(Some(package), Counter::HostsBlocked);
        ctx.services.live.host_blocked(package, host);
        return Ok(empty(StatusCode::FORBIDDEN));
    }
    let filtered = filtered.filter(|_| url_filtering);
    let told = filtered
        .and_then(|_| told_page(&req))
        .map(|(value, page)| (value.clone(), page));
    let method = req.method().clone();
    let matched = filtered.map(|(url, host)| MatchedRequest {
        shared,
        filters: &for_app,
        url,
        host,
        metadata: RequestMetadata::new(&req, &method, told.as_ref().map(|(v, p)| (v, p))),
    });
    let (judged, mut rewrite) = if let Some(request) = &matched {
        let (decision, rewrite) = request.evaluate(kind);
        if request.metadata.shows_page {
            ctx.services.policy.note_page(package);
        }
        if decision.blocked {
            return Ok(request.blocked(decision));
        }
        let rewrite = request.select_html(&filters, kind, rewrite);
        (Some((request, decision)), rewrite)
    } else {
        (None, RewritePlan::default())
    };
    // An error opening the upstream ends the connection before any HTTP response.
    let upstream = upstream.await?;
    let pending = judged.and_then(|(request, decision)| match decision.kind {
        RequestKind::Known(_) => {
            request.record(&decision);
            None
        }
        RequestKind::Assumed => Some(PendingVerdict {
            request,
            decision: Some(decision),
        }),
    });
    let hiding = hiding_for(shared, &filters, &req, url, kind.as_str());
    rewrite.ask_for_identity |= hiding.is_some();
    let response = forward_response(shared, upstream, req, url, rewrite, pending).await;
    Ok(hide(&filters, response, hiding).await)
}

async fn forward_response(
    shared: &Shared,
    upstream: &Upstream,
    mut req: Request<Incoming>,
    url: &str,
    mut rewrite: RewritePlan,
    pending: Option<PendingVerdict<'_>>,
) -> Response<Body> {
    if rewrite.ask_for_identity {
        req.headers_mut()
            .insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    }
    // A WebSocket starts as a request asking to switch protocols; once the
    // server agrees the two connections are joined and no longer read.
    let connect_tunnel = shared.https.is_none() && req.method() == Method::CONNECT;
    let app_upgrade = (connect_tunnel || req.headers().contains_key(UPGRADE))
        .then(|| hyper::upgrade::on(&mut req));
    let mut resp = match forward(upstream, req, rewrite.request_headers.drain(..)).await {
        Ok(response) => response,
        Err(e) => {
            let Shared { ctx, package, .. } = shared;
            trace!(ctx, "error   {package} {url}: {e}");
            return empty(StatusCode::BAD_GATEWAY);
        }
    };
    let html_document = pending.is_none()
        || crate::media_type::read(resp.headers()).is_some_and(|mime| {
            mime.eq_ignore_ascii_case("text/html")
                || mime.eq_ignore_ascii_case("application/xhtml+xml")
        });
    // A protocol switch has no response representation to classify.
    if let Some(pending) = pending
        && resp.status() != StatusCode::SWITCHING_PROTOCOLS
        && let Some(kind) = response_kind(resp.headers())
        && let Some(blocked) = pending.finish(kind, &mut rewrite)
    {
        return blocked;
    }
    join_upgrade(shared, &mut resp, app_upgrade, connect_tunnel);
    let html = (html_document && !rewrite.body.html.is_empty()).then(|| html::Action {
        rules: rewrite.body.html,
        charset: crate::html::transport_charset(resp.headers()).map(str::to_owned),
    });
    // The verdict, upgrade handling and HTML charset use the original head.
    // Remove headers before the body rewrite and cosmetic code use the head
    // for the app. Content-Type, framing, encoding and CSP are protected.
    if resp.status() != StatusCode::SWITCHING_PROTOCOLS {
        for name in rewrite.response_headers {
            resp.headers_mut().remove(name);
        }
    }
    crate::rewrite::wrap(resp, rewrite.body.actions, html)
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
    req: &Request<Incoming>,
    url: &str,
    kind: &str,
) -> Option<Hiding> {
    // With element hiding enabled, ask for eligible documents unpacked
    // so that injection can stream the page without decompressing it.
    let whole_page = matches!(kind, "document" | "subdocument")
        && req.method() == hyper::Method::GET
        && permits_body_rewrite(req);
    (shared.https.is_some() && whole_page && shared.ctx.selection.cosmetic.load(Relaxed))
        .then(|| filters.hiding(url))
        .filter(|hiding| hiding.any())
}

fn permits_actions<B>(req: &Request<B>) -> bool {
    req.method() != Method::CONNECT && !req.headers().contains_key(UPGRADE)
}

fn permits_body_rewrite<B>(req: &Request<B>) -> bool {
    permits_actions(req) && req.method() != Method::HEAD && !req.headers().contains_key(RANGE)
}

async fn forward(
    upstream: &Upstream,
    mut req: Request<Incoming>,
    remove_headers: impl Iterator<Item = HeaderName>,
) -> Result<Response<Incoming>, hyper::Error> {
    for name in remove_headers {
        req.headers_mut().remove(name);
    }
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
    resp: Response<Body>,
    hiding: Option<Hiding>,
) -> Response<Body> {
    use crate::cosmetic::{ByName, Inject, Inline, Prefix, Preparation};

    let Some(mut hiding) = hiding.filter(|_| crate::cosmetic::takes(&resp)) else {
        return resp;
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
