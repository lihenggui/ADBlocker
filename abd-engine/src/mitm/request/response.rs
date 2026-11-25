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

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{
    ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_SECURITY_POLICY,
    CONTENT_TYPE, HeaderValue, LOCATION,
};
use hyper::{Request, Response, StatusCode};

use super::judgement::{MatchedRequest, RequestDecision, RequestKind, permits_body_rewrite};
use super::{Body, Shared};
use crate::filter::{Filters, Hiding, Redirect};
use crate::rewrite::RewritePlan;

impl MatchedRequest<'_> {
    pub(super) fn blocked(&self, decision: RequestDecision<'_>) -> Response<Body> {
        self.record(&decision);
        blocked(decision.redirect)
    }

    pub(super) fn select_html(
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
            plan.body.html = filters.html_rules(
                self.host,
                &self.url,
                self.metadata.method,
                kind != RequestKind::Assumed,
            );
            plan.ask_for_identity |= !plan.body.html.is_empty();
        }
        plan
    }
}

fn blocked(redirect: Option<Redirect>) -> Response<Body> {
    let Some(redirect) = redirect else {
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

pub(super) fn redirect(location: HeaderValue) -> Response<Body> {
    // The upstream stays tied to the server the app connected to. If the
    // app follows this redirect, its new request is judged on its own.
    let mut response = empty(StatusCode::TEMPORARY_REDIRECT);
    response.headers_mut().insert(LOCATION, location);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) fn hiding_for(
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

pub(super) async fn hide(
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
        let rewriting = parts
            .extensions
            .remove::<crate::rewrite::HoldsFirstFrame>()
            .is_some();
        match body
            .prepare(&mut parts.headers, &script, head_css, rewriting)
            .await
        {
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

pub(super) fn empty(status: StatusCode) -> Response<Body> {
    let mut resp = Response::new(Full::new(Bytes::new()).map_err(|e| match e {}).boxed());
    *resp.status_mut() = status;
    resp
}
