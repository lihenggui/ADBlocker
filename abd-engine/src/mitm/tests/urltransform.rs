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

use super::*;

pub(super) fn fixture(req: &Request<Incoming>) -> Option<Response<Full<Bytes>>> {
    let uri = req.uri();
    uri.path().starts_with("/transformed/").then(|| {
        let body = if req.method() == Method::HEAD {
            Bytes::new()
        } else {
            Bytes::copy_from_slice(uri.path_and_query().unwrap().as_str().as_bytes())
        };
        Response::builder()
            .header("content-type", "text/plain")
            .body(Full::new(body))
            .unwrap()
    })
}

async fn document(
    rig: &Rig,
    alpn: &[&str],
    method: Method,
    path: &str,
) -> (hyper::HeaderMap, StatusCode, String) {
    let (headers, status, body, real) = rig
        .request_response(Trust::System, alpn, path, None, |req| {
            req.method(method).header("sec-fetch-dest", "document")
        })
        .await
        .unwrap();
    assert!(!real);
    (headers, status, body)
}

#[tokio::test]
async fn the_new_path_and_query_reach_the_original_server_without_an_engine_loop() {
    for alpn in [H1, BOTH] {
        let rig = rig(Setup {
            rules: "||site.test/firstpath$all,urltransform=/firstpath/transformed\\/secondpath/\n\
                    ||site.test/transformed/secondpath$all,urltransform=/secondpath/thirdpath/\n\
                    ||site.test/firstpath$all,urltransform=/tracking=1/clean=1/",
            alpn,
            ..Setup::default()
        })
        .await;
        let mut watcher = rig.ctx.services.live.join(None).unwrap();
        let path = "/firstpath?tracking=1";
        let (headers, status, body) = document(&rig, alpn, Method::GET, path).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!headers.contains_key("location"));
        assert_eq!(body, "/transformed/secondpath?clean=1");
        assert_eq!(rig.hits(), 1);
        assert_eq!(rig.count(Counter::RequestsAllowed), 1);
        assert_eq!(rig.count(Counter::RequestsBlocked), 0);
        let event = only_watch_event(&mut watcher).await;
        assert_eq!(event["verdict"], "allowed");
        assert_eq!(event["url"], format!("https://{HOST}{path}"));
    }
}

#[tokio::test]
async fn a_block_on_either_the_original_url_or_its_transformation_stands() {
    // The matcher requires an explicit document option for URL path blocks.
    for rules in [
        "||site.test^$urltransform=/firstpath/secondpath/\n||site.test/firstpath^$document",
        "||site.test^$urltransform=/firstpath/secondpath/\n||site.test/secondpath^$document",
    ] {
        let rig = rig(Setup {
            rules,
            ..Setup::default()
        })
        .await;
        let mut watcher = rig.ctx.services.live.join(None).unwrap();
        let (_, status, body) = document(&rig, H1, Method::GET, "/firstpath").await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{rules}");
        assert!(body.is_empty());
        assert_eq!(rig.hits(), 0);
        assert_eq!(rig.count(Counter::RequestsBlocked), 1);
        assert_eq!(rig.count(Counter::RequestsAllowed), 0);
        let event = only_watch_event(&mut watcher).await;
        assert_eq!(event["verdict"], "blocked");
        assert_eq!(event["url"], format!("https://{HOST}/firstpath"));
    }
}

const OUTSIDE: &str =
    r"||site.test/start$urltransform=/^https:\/\/site\.test\/start/https:\/\/elsewhere.test\/next/";

#[tokio::test]
async fn a_whole_url_transform_redirects_get_and_head_without_sending_a_request() {
    for (method, rules, location) in [
        (Method::GET, OUTSIDE, "https://elsewhere.test/next?q=1"),
        (Method::HEAD, OUTSIDE, "https://elsewhere.test/next?q=1"),
        (
            Method::GET,
            r"||site.test/start$urltransform=/^https:/http:/",
            "http://site.test/start?q=1",
        ),
        (
            Method::GET,
            r"||site.test/start$urltransform=/^https:\/\/site\.test/https:\/\/site.test:8443/",
            "https://site.test:8443/start?q=1",
        ),
    ] {
        let rig = rig(Setup {
            rules,
            ..Setup::default()
        })
        .await;
        let mut watcher = rig.ctx.services.live.join(None).unwrap();
        let (headers, status, body) = document(&rig, H1, method, "/start?q=1").await;
        assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(headers["location"], location);
        assert_eq!(headers["cache-control"], "no-store");
        assert!(body.is_empty());
        assert_eq!(rig.hits(), 0);
        assert_eq!(rig.count(Counter::RequestsBlocked), 1);
        assert_eq!(rig.count(Counter::RequestsAllowed), 0);
        let event = only_watch_event(&mut watcher).await;
        assert_eq!(event["verdict"], "blocked");
        assert_eq!(event["url"], format!("https://{HOST}/start?q=1"));
    }
}

#[tokio::test]
async fn a_whole_url_transform_with_the_same_origin_forwards_only_the_path_and_query() {
    let rig = rig(Setup {
        rules: r"||site.test/start$urltransform=/^https:\/\/site\.test\/start/https:\/\/site.test:443\/transformed\/next?value=1#section/",
        ..Setup::default()
    })
    .await;
    let (headers, status, body) = document(&rig, H1, Method::GET, "/start").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key("location"));
    assert_eq!(body, "/transformed/next?value=1");
    assert_eq!(rig.hits(), 1);
}

#[tokio::test]
async fn post_skips_whole_url_rules_even_if_they_only_change_the_path() {
    for rule in [
        OUTSIDE,
        r"||site.test/start$urltransform=/^https:\/\/site\.test\/start/https:\/\/site.test\/next/",
    ] {
        let rig = rig(Setup {
            rules: rule,
            ..Setup::default()
        })
        .await;
        let (_, status, body) = document(&rig, H1, Method::POST, "/start").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "POST /start");
        assert_eq!(rig.hits(), 1);
    }
}

#[tokio::test]
async fn invalid_or_identical_transformed_urls_leave_the_original_in_place() {
    for rule in [
        r"||site.test/start$urltransform=/^https:\/\/site\.test\/start/not-a-url/",
        r"||site.test/start$urltransform=/^https/httpz/",
        r"||site.test/start$urltransform=/^https:\/\/site\.test\/start/https:\/\/elsewhere.test:99999\//",
        r"||site.test/start$urltransform=/^\/start/relative/",
        r"||site.test/start$urltransform=/start/start/",
    ] {
        let rig = rig(Setup {
            rules: rule,
            ..Setup::default()
        })
        .await;
        let (headers, status, body) = document(&rig, H1, Method::GET, "/start").await;
        assert_eq!(status, StatusCode::OK, "{rule}");
        assert!(!headers.contains_key("location"));
        assert_eq!(body, "GET /start");
        assert_eq!(rig.hits(), 1);
    }
}

#[tokio::test]
async fn response_rules_match_the_forwarded_url_and_assumed_types_never_retry_transforms() {
    let rig = rig(Setup {
        rules: "||site.test/rewrite$other,urltransform=/rewrite/replace\\/text/\n\
                ||site.test/replace/text$replace=/original/changed/\n\
                ||site.test/replace/text$all,urltransform=/text/binary/\n\
                ||site.test/replace/json$xmlhttprequest,urltransform=/json/image/",
        ..Setup::default()
    })
    .await;
    let (_, status, body, _) = rig
        .get_response(Trust::System, H1, "/rewrite", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "changed identity");
    let (_, status, body, _) = rig
        .get_response(Trust::System, H1, "/replace/json", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, r#"{"ad":true,"keep":"original"}"#);
    assert_eq!(rig.hits(), 2);
    assert_eq!(rig.count(Counter::RequestsAllowed), 2);
}
