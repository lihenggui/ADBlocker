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

use std::future::Future;
use std::sync::Arc;

use hyper::body::Incoming;
use hyper::header::{ACCEPT_ENCODING, HeaderName, HeaderValue, UPGRADE};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;

use super::judgement::{MatchedRequest, RequestDecision, RequestKind, response_kind};
use super::response::{empty, hide, hiding_for};
use super::{Body, Shared, Upstream, trace};
use crate::filter::Filters;
use crate::mitm::upstream::LINGER_TIME;
use crate::relay;
use crate::rewrite::RewritePlan;
use crate::rule::action::html;

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
        let (decision, selected) = request.evaluate(inferred, false);
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

pub(super) async fn send<'a>(
    shared: &Shared,
    filters: &Arc<Filters>,
    req: Request<Incoming>,
    kind: RequestKind,
    url: &str,
    (judged, mut rewrite): (
        Option<(&MatchedRequest<'_>, RequestDecision<'_>)>,
        RewritePlan,
    ),
    upstream: impl Future<Output = anyhow::Result<&'a Upstream>>,
) -> anyhow::Result<Response<Body>> {
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
    let hiding = hiding_for(shared, filters, &req, url, kind.as_str());
    rewrite.ask_for_identity |= hiding.is_some();
    let response = forward_response(shared, upstream, req, url, rewrite, pending).await;
    Ok(hide(filters, response, hiding).await)
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
    crate::rewrite::wrap(resp, rewrite.body.actions, html, url)
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

pub(in crate::mitm) async fn finish_tunnel(shared: &Shared) {
    let tunnel = shared.tunnel.lock().unwrap().take();
    if let Some(tunnel) = tunnel {
        let _ = tunnel.await;
    }
}
