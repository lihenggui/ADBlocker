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

use hyper::header::{COOKIE, SET_COOKIE};

use super::*;

pub(super) fn fixture(req: &Request<Incoming>) -> Option<Response<Full<Bytes>>> {
    let path = req.uri().path().strip_prefix("/cookies/")?;
    let mut response = Response::builder()
        .header("content-type", "application/json")
        .header(
            "x-request-encoding",
            req.headers()
                .get(ACCEPT_ENCODING)
                .cloned()
                .unwrap_or_else(|| "absent".parse().unwrap()),
        );
    for value in req.headers().get_all(COOKIE) {
        response = response.header("x-seen-cookie", value);
    }
    for cookie in [
        "__cfduid=abc; Secure",
        "__utma=abc; Path=/",
        "c_user=123; HttpOnly",
        "concept=\"x=y\"; Expires=Wed, 09 Jun 2021 10:18:14 GMT",
        "_ga_first=abc; Secure",
        "NAME=one; Path=/; mAX-aGE=24; sAMESITE=Strict; eXPiRES=Wed, 09 Jun 2021 10:18:14 GMT",
        "NAME=two; Secure; HttpOnly",
        "keep=1; Path=/; SameSite=Strict",
        "malformed",
        "NAME=three; Max-Age=oops",
    ] {
        response = response.header(SET_COOKIE, cookie);
    }
    Some(
        response
            .body(Full::new(Bytes::from(format!("{{\"path\":\"{path}\"}}"))))
            .unwrap(),
    )
}

async fn request(
    rig: &Rig,
    path: &str,
    referer: Option<&str>,
    cookies: &[&str],
) -> (hyper::HeaderMap, StatusCode, String) {
    let (headers, status, body, real) = rig
        .request_response(Trust::System, H1, path, referer, |mut req| {
            req = req.header(ACCEPT_ENCODING, "gzip, br");
            for cookie in cookies {
                req = req.header(COOKIE, *cookie);
            }
            req
        })
        .await
        .unwrap();
    assert!(!real);
    (headers, status, body)
}

fn values<'a>(headers: &'a hyper::HeaderMap, name: &str) -> Vec<&'a str> {
    headers
        .get_all(name)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect()
}

#[tokio::test]
async fn matched_cookie_names_are_removed_in_both_directions() {
    let rig = rig(Setup {
        rules: "$cookie=__cfduid\n$cookie=/__utm[a-z]/\n||site.test^$third-party,cookie=c_user",
        ..Setup::default()
    })
    .await;
    let cookie = "keep=1; __cfduid=abc; __utma=xyz; c_user=123";
    for source in [None, Some("https://other.test/page")] {
        let (headers, status, body) =
            request(&rig, "/cookies/echo", source, &[cookie, "__utma=xyz"]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"path":"echo"}"#);
        let seen = if source.is_some() {
            "keep=1"
        } else {
            "keep=1; c_user=123"
        };
        assert_eq!(values(&headers, "x-seen-cookie"), [seen]);
        assert_eq!(headers["x-request-encoding"], "gzip, br");
        let set = values(&headers, "set-cookie");
        assert!(set.iter().all(|value| !value.starts_with("__")));
        assert_eq!(set.contains(&"c_user=123; HttpOnly"), source.is_none());
    }
}

#[tokio::test]
async fn cookie_attributes_only_change_matching_set_cookie_fields() {
    let rig = rig(Setup {
        rules: "||site.test^$cookie=NAME;maxAge=3600;sameSite=lax\n\
                ||site.test^$cookie=NAME;maxAge=60\n\
                ||site.test^$cookie=keep;sameSite=none",
        ..Setup::default()
    })
    .await;
    let (headers, status, _) = request(&rig, "/cookies/echo", None, &["NAME=one; keep=1"]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(values(&headers, "x-seen-cookie"), ["NAME=one; keep=1"]);
    assert_eq!(
        values(&headers, "set-cookie"),
        [
            "__cfduid=abc; Secure",
            "__utma=abc; Path=/",
            "c_user=123; HttpOnly",
            "concept=\"x=y\"; Expires=Wed, 09 Jun 2021 10:18:14 GMT",
            "_ga_first=abc; Secure",
            "NAME=one; Path=/; Max-Age=24; SameSite=Lax",
            "NAME=two; Secure; HttpOnly; Max-Age=60; SameSite=Lax",
            "keep=1; Path=/; SameSite=None",
            "malformed",
            "NAME=three; Max-Age=60; SameSite=Lax",
        ]
    );
}

#[tokio::test]
async fn cookie_exceptions_spare_their_names_or_the_entire_request() {
    let rig = rig(Setup {
        rules: "||site.test^$cookie\n\
                @@||site.test/cookies/named^$cookie=concept\n\
                @@||site.test/cookies/named^$cookie=/^_ga_/\n\
                @@||site.test/cookies/all^$cookie\n\
                @@||site.test/cookies/doc^$document\n\
                @@||site.test/cookies/url^$urlblock",
        ..Setup::default()
    })
    .await;
    let all = "concept=1; _ga_first=2; c_user=3";
    for (path, kept, count) in [
        ("/cookies/suppress", None, 2),
        ("/cookies/named", Some("concept=1; _ga_first=2"), 4),
        ("/cookies/all", Some(all), 10),
        ("/cookies/doc", Some(all), 10),
        ("/cookies/url", Some(all), 10),
    ] {
        let (headers, status, _) = request(&rig, path, None, &[all]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            values(&headers, "x-seen-cookie").first().copied(),
            kept,
            "{path}"
        );
        assert_eq!(values(&headers, "set-cookie").len(), count, "{path}");
    }
}

#[tokio::test]
async fn neither_an_early_nor_an_inferred_block_delivers_cookie_fields() {
    let rig = rig(Setup {
        rules: "||site.test^$cookie=NAME\n\
                ||site.test/cookies/early^\n\
                ||site.test/cookies/inferred^$xmlhttprequest",
        ..Setup::default()
    })
    .await;
    for (path, hits) in [("/cookies/early", 0), ("/cookies/inferred", 1)] {
        let (headers, status, body) = request(&rig, path, None, &["NAME=one; keep=1"]).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.is_empty());
        assert!(!headers.contains_key(SET_COOKIE));
        assert!(!headers.contains_key("x-seen-cookie"));
        assert_eq!(rig.hits(), hits);
    }
}

#[tokio::test]
async fn a_max_age_rule_does_not_revive_an_expired_cookie() {
    let rig = rig(Setup {
        rules: "||site.test^$cookie=concept;maxAge=3600",
        ..Setup::default()
    })
    .await;
    let (headers, status, _) = request(&rig, "/cookies/echo", None, &["concept=1"]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(values(&headers, "x-seen-cookie"), ["concept=1"]);
    assert_eq!(
        values(&headers, "set-cookie")
            .into_iter()
            .find(|value| value.starts_with("concept=")),
        Some("concept=\"x=y\"; Max-Age=0")
    );
}
