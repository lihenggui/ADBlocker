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
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;
use hyper::{Request, Response};

use super::upstream::Upstream;
use super::{Ctx, trace};
use crate::filter::Filters;
use crate::rewrite::RewritePlan;
use crate::tls::Tls;

mod forward;
mod judgement;
mod response;
mod target;
mod transform;

pub(super) use forward::finish_tunnel;
use forward::send;
use judgement::{MatchedRequest, RequestMetadata, kind, told_page};
pub use judgement::{RequestDecision, RequestKind, evaluate_request};
use response::empty;
pub(super) use target::clear_target;
use target::{guard_domain, guard_site, target_of};
use transform::Transformed;

type Body = BoxBody<Bytes, hyper::Error>;

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

pub(super) async fn serve<'a>(
    shared: &Shared,
    mut req: Request<Incoming>,
    upstream: impl Future<Output = anyhow::Result<&'a Upstream>>,
) -> anyhow::Result<Response<Body>> {
    let ctx = &shared.ctx;
    let package = &shared.package;
    let selected = ctx.is_package(shared.uid, package);
    let url_filtering = ctx.selection.url_filtering.load(Relaxed);
    let filters = ctx.filters();
    let for_app = ctx.app_filters(&filters, package);
    if let Some(response) = guard_site(shared, req.version(), selected, url_filtering) {
        return Ok(response);
    }
    let target = match target_of(&req, shared.https.as_ref()) {
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
        && (shared.https.is_some() || !ctx.lists.allow.read().unwrap().covers(host))
    });
    if let Some(response) = guard_domain(
        shared,
        &for_app,
        filtered.map(|(_, host)| host.as_ref()),
        url_filtering,
    ) {
        return Ok(response);
    }
    let filtered = filtered.filter(|_| url_filtering);
    let told = filtered
        .and_then(|_| told_page(&req))
        .map(|(value, page)| (value.clone(), page));
    let method = req.method().clone();
    let mut matched = filtered.map(|(url, host)| MatchedRequest {
        shared,
        filters: &for_app,
        url: Cow::Borrowed(url),
        original_url: url,
        host,
        metadata: RequestMetadata::new(&req, &method, told.as_ref().map(|(v, p)| (v, p))),
    });
    let (judged, rewrite) = match &mut matched {
        Some(request) => match prepare_request(request, &filters, &mut req, kind) {
            RequestPlan::Respond(response) => return Ok(response),
            RequestPlan::Forward(decision, rewrite) => (Some((&*request, decision)), rewrite),
        },
        None => (None, RewritePlan::default()),
    };
    let url = judged
        .as_ref()
        .map_or(url, |(request, _)| request.url.as_ref());
    send(
        shared,
        &filters,
        req,
        kind,
        url,
        (judged, rewrite),
        upstream,
    )
    .await
}

enum RequestPlan<'a> {
    Forward(RequestDecision<'a>, RewritePlan),
    Respond(Response<Body>),
}

fn prepare_request<'a>(
    request: &mut MatchedRequest<'a>,
    filters: &Filters,
    req: &mut Request<Incoming>,
    kind: RequestKind,
) -> RequestPlan<'a> {
    let (mut decision, mut rewrite) = request.evaluate(kind, true);
    if request.metadata.shows_page {
        request
            .shared
            .ctx
            .services
            .policy
            .note_page(&request.shared.package);
    }
    if decision.blocked {
        return RequestPlan::Respond(request.blocked(decision));
    }
    match transform::apply(
        &request.url,
        req.uri(),
        req.method(),
        decision.rewritten_url.take().as_deref(),
        &rewrite.url_transforms,
    ) {
        Some(Transformed::Redirect(location)) => {
            decision.blocked = true;
            request.record(&decision);
            return RequestPlan::Respond(response::redirect(location));
        }
        Some(Transformed::Forward { url, uri }) => {
            request.url = Cow::Owned(url);
            (decision, rewrite) = request.evaluate(kind, false);
            if decision.blocked {
                return RequestPlan::Respond(request.blocked(decision));
            }
            *req.uri_mut() = uri;
        }
        None => rewrite.url_transforms.clear(),
    }
    RequestPlan::Forward(decision, request.select_html(filters, kind, rewrite))
}
