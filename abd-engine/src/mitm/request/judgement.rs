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

use hyper::header::{HeaderValue, ORIGIN, RANGE, REFERER, UPGRADE};
use hyper::{HeaderMap, Method, Request};

use super::{Shared, trace};
use crate::filter::{AppFilters, Page, Redirect};
use crate::rewrite::RewritePlan;
use crate::rule::action::Place;
use crate::stats::Counter;

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

pub(in crate::mitm) fn kind<B>(req: &Request<B>) -> RequestKind {
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
pub(super) fn told_page<B>(
    req: &Request<B>,
) -> Option<(&HeaderValue, adblock::url_parser::RequestUrl)> {
    [REFERER, ORIGIN].into_iter().find_map(|name| {
        let value = req.headers().get(name)?;
        Some((value, crate::filter::parse_http_page(value.to_str().ok()?)?))
    })
}

/// Evaluate the metadata of an HTTP request using the URL validated by its transport.
pub fn evaluate_request<'a, B>(
    filters: &AppFilters<'_>,
    req: &'a Request<B>,
    url: &str,
) -> RequestDecision<'a> {
    let told = told_page(req);
    let metadata = RequestMetadata::new(req, req.method(), told.as_ref().map(|(v, p)| (*v, p)));
    metadata.evaluate(filters, url, kind(req), false).0
}

/// The policy inputs that must survive moving the request into the transport.
pub(super) struct RequestMetadata<'a, 't> {
    page: Page<'a>,
    told: Option<&'t str>,
    pub(super) shows_page: bool,
    pub(super) method: &'a str,
    permits_actions: bool,
    pub(super) permits_body_rewrite: bool,
}

impl<'a, 't> RequestMetadata<'a, 't> {
    pub(super) fn new<B>(
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
        include_transforms: bool,
    ) -> (RequestDecision<'t>, RewritePlan) {
        let matched = self
            .permits_actions
            .then(|| filters.response_matches(url, self.page, self.method))
            .flatten();
        let actions = matched
            .as_ref()
            .map_or_else(Vec::new, |m| m.actions(kind.as_str(), include_transforms));
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
                            .actions(kind, false)
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

pub(super) struct MatchedRequest<'a> {
    pub(super) shared: &'a Shared,
    pub(super) filters: &'a AppFilters<'a>,
    pub(super) url: Cow<'a, str>,
    pub(super) original_url: &'a str,
    pub(super) host: &'a str,
    pub(super) metadata: RequestMetadata<'a, 'a>,
}

impl<'a> MatchedRequest<'a> {
    pub(super) fn evaluate(
        &self,
        kind: RequestKind,
        include_transforms: bool,
    ) -> (RequestDecision<'a>, RewritePlan) {
        self.metadata
            .evaluate(self.filters, &self.url, kind, include_transforms)
    }

    pub(super) fn record(&self, decision: &RequestDecision<'_>) {
        let Shared { ctx, package, .. } = self.shared;
        let Self {
            host,
            original_url: url,
            ..
        } = self;
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
}

fn permits_actions<B>(req: &Request<B>) -> bool {
    req.method() != Method::CONNECT && !req.headers().contains_key(UPGRADE)
}

pub(super) fn permits_body_rewrite<B>(req: &Request<B>) -> bool {
    permits_actions(req) && req.method() != Method::HEAD && !req.headers().contains_key(RANGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Filters;

    #[test]
    fn a_request_is_of_the_kind_the_browser_says_or_its_name_suggests() {
        let with = |dest: Option<&str>, path: &str| {
            let mut req = Request::get(path);
            if let Some(dest) = dest {
                req = req.header("sec-fetch-dest", dest);
            }
            kind(&req.body(()).unwrap())
        };
        use RequestKind::{Assumed, Known};
        assert_eq!(with(Some("script"), "/a"), Known("script"));
        assert_eq!(with(Some("empty"), "/api.png"), Known("xmlhttprequest"));
        assert_eq!(with(Some("iframe"), "/frame"), Known("subdocument"));
        assert_eq!(with(None, "/static/app.js"), Known("script"));
        assert_eq!(with(None, "/img/logo.png"), Known("image"));
        assert_eq!(with(None, "/v1/feed"), Assumed);
    }

    #[test]
    fn a_single_recognized_response_media_type_identifies_the_resource() {
        use hyper::header::{CONTENT_TYPE, HeaderValue};

        for (mime, resource) in [
            ("Image/PNG; Q=1", "image"),
            ("text/css", "stylesheet"),
            ("text/javascript", "script"),
            ("application/javascript", "script"),
            ("application/x-javascript", "script"),
            ("application/ecmascript", "script"),
            ("text/ecmascript", "script"),
            ("video/mp4", "media"),
            ("audio/mpeg", "media"),
            ("application/vnd.apple.mpegurl", "media"),
            ("application/x-mpegurl", "media"),
            ("application/dash+xml", "media"),
            ("font/woff2", "font"),
            ("application/font-woff", "font"),
            ("application/x-font-ttf", "font"),
            ("application/vnd.ms-fontobject", "font"),
            ("application/json", "xmlhttprequest"),
            ("text/json", "xmlhttprequest"),
            ("application/problem+json", "xmlhttprequest"),
            ("application/xml", "xmlhttprequest"),
            ("text/xml", "xmlhttprequest"),
            ("application/x-protobuf", "xmlhttprequest"),
            ("application/grpc", "xmlhttprequest"),
            ("application/grpc+proto", "xmlhttprequest"),
        ] {
            let mut headers = hyper::HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
            assert_eq!(
                response_kind(&headers),
                Some(RequestKind::Known(resource)),
                "{mime}"
            );
        }
        let mut headers = hyper::HeaderMap::new();
        assert_eq!(response_kind(&headers), None);
        for mime in [
            "application/octet-stream",
            "text/html",
            "image/",
            "image/png, text/html",
        ] {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
            assert_eq!(response_kind(&headers), None, "{mime}");
        }
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("image/png"));
        headers.append(CONTENT_TYPE, HeaderValue::from_static("text/html"));
        assert_eq!(response_kind(&headers), None);
    }

    #[test]
    fn a_page_gone_to_is_its_own_first_party_whatever_led_there() {
        let dir = tempfile::tempdir().unwrap();
        let rules = "||ads.example/own$document,script,first-party\n\
                     ||ads.example/other$document,script,third-party\n";
        std::fs::write(dir.path().join("list.txt"), rules).unwrap();
        let filters = Filters::load(&[dir.path()], false).unwrap();
        let blocked = |path: &str, dest: Option<&str>, referer: Option<&str>| {
            let mut req = Request::get(path);
            if let Some(dest) = dest {
                req = req.header("sec-fetch-dest", dest);
            }
            if let Some(referer) = referer {
                req = req.header(REFERER, referer);
            }
            let url = format!("https://ads.example{path}");
            evaluate_request(&filters.for_app("com.app"), &req.body(()).unwrap(), &url).blocked
        };
        let news = Some("https://news.example/");
        for referer in [None, news] {
            assert!(blocked("/own", Some("document"), referer), "{referer:?}");
            assert!(!blocked("/other", Some("document"), referer), "{referer:?}");
        }
        assert!(blocked("/other", Some("script"), news));
        assert!(!blocked("/own", Some("script"), news));
        // Made by no page it names: there is no other party to be the third.
        assert!(blocked("/own", Some("script"), None));
        assert!(!blocked("/other", Some("script"), None));
    }

    #[test]
    fn a_browser_that_names_no_page_still_says_whose_request_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let rules = "||ads.example/own$script,first-party\n\
                     ||ads.example/other$script,third-party\n\
                     ||ads.example/page$domain=~ads.example\n";
        std::fs::write(dir.path().join("list.txt"), rules).unwrap();
        let filters = Filters::load(&[dir.path()], false).unwrap();
        let blocked = |path: &str, headers: &[(&str, &str)]| {
            let mut req = Request::get(path);
            for (name, value) in headers {
                req = req.header(*name, *value);
            }
            let url = format!("https://ads.example{path}");
            evaluate_request(&filters.for_app("com.app"), &req.body(()).unwrap(), &url).blocked
        };
        let script = ("sec-fetch-dest", "script");
        let cross_site = [script, ("sec-fetch-site", "cross-site")];
        assert!(blocked("/other", &cross_site));
        assert!(!blocked("/own", &cross_site));
        for site in ["same-origin", "same-site"] {
            let same = [script, ("sec-fetch-site", site)];
            assert!(blocked("/own", &same), "{site}");
            assert!(!blocked("/other", &same), "{site}");
        }
        // Over HTTP a browser sends no Sec-Fetch-Dest: going to a page, it
        // asks for an upgrade, and the page is its own. An app's request for
        // the same is of no page, which is not the one page the rule spares.
        assert!(!blocked("/page", &[("upgrade-insecure-requests", "1")]));
        assert!(blocked("/page", &[]));
        // Led there from another page, it is not the user's own going there.
        let led = [
            ("upgrade-insecure-requests", "1"),
            ("referer", "https://news.example/"),
        ];
        assert!(blocked("/page", &led));
    }
}
