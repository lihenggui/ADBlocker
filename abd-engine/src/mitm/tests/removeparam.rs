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

async fn request(
    rig: &Rig,
    alpn: &[&str],
    method: Method,
    path: &str,
    dest: Option<&str>,
) -> (hyper::HeaderMap, StatusCode, String) {
    let (headers, status, body, real) = rig
        .request_response(Trust::System, alpn, path, None, |req| {
            let req = req.method(method);
            match dest {
                Some(dest) => req.header("sec-fetch-dest", dest),
                None => req,
            }
        })
        .await
        .unwrap();
    assert!(!real);
    assert!(!headers.contains_key("location"));
    (headers, status, body)
}

#[tokio::test]
async fn supported_methods_remove_named_parameters_in_place() {
    let original = "/transformed/page?utm_source=1&another=2";
    for alpn in [H1, BOTH] {
        let rig = rig(Setup {
            rules: "$removeparam=utm_source\n||site.test/transformed/page?another=2|$csp=frame-src 'none'",
            alpn,
            ..Setup::default()
        })
        .await;
        for method in [
            Method::GET,
            Method::HEAD,
            Method::OPTIONS,
            Method::POST,
            Method::PUT,
        ] {
            let expected = match method {
                Method::HEAD => "",
                Method::PUT => original,
                _ => "/transformed/page?another=2",
            };
            let (headers, status, body) =
                request(&rig, alpn, method.clone(), original, Some("document")).await;
            assert_eq!(
                (status, body.as_str()),
                (StatusCode::OK, expected),
                "{method}"
            );
            assert_eq!(
                headers.contains_key("content-security-policy"),
                method != Method::PUT
            );
        }
        assert_eq!(rig.hits(), 5);
    }
}

#[tokio::test]
async fn the_changed_url_is_judged_once_and_the_original_url_is_reported() {
    let start = "/transformed/page?param=1&another=2";
    for (rules, expected, hits) in [
        (
            "||site.test^$removeparam=param
             ||site.test/transformed/page?param=1*$document",
            StatusCode::FORBIDDEN,
            0,
        ),
        (
            "||site.test^$removeparam=param
             ||site.test/transformed/page?another=2|$document",
            StatusCode::FORBIDDEN,
            0,
        ),
        (
            "||site.test^$removeparam=param
             ||site.test/transformed/page?another=2|$removeparam=another",
            StatusCode::OK,
            1,
        ),
    ] {
        let rig = rig(Setup {
            rules,
            ..Setup::default()
        })
        .await;
        let mut watcher = rig.ctx.services.live.join(None).unwrap();
        let (_, status, body) = request(&rig, H1, Method::GET, start, Some("document")).await;
        assert_eq!(status, expected);
        if hits == 1 {
            assert_eq!(body, "/transformed/page?another=2");
        }
        assert_eq!(rig.hits(), hits);
        assert_eq!(rig.count(Counter::RequestsAllowed), hits as u64);
        assert_eq!(rig.count(Counter::RequestsBlocked), (1 - hits) as u64);
        let event = only_watch_event(&mut watcher).await;
        assert_eq!(
            event["verdict"],
            if hits == 1 { "allowed" } else { "blocked" }
        );
        assert_eq!(event["url"], format!("https://{HOST}{start}"));
    }
}

#[tokio::test]
async fn untyped_rules_wait_for_a_known_document_but_other_rules_can_transform_assumed_requests() {
    let rig = rig(Setup {
        rules: "||site.test^$removeparam=utm_source\n\
                ||site.test^$other,removeparam=also\n\
                ||site.test^$other,urltransform=/page/edited/",
        ..Setup::default()
    })
    .await;
    let (_, status, body) = request(
        &rig,
        H1,
        Method::GET,
        "/transformed/page?utm_source=1&also=2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "/transformed/edited?utm_source=1");
}

#[tokio::test]
async fn the_page_exceptions_and_named_parameter_exceptions_preserve_queries() {
    let rig = rig(Setup {
        rules: "$removeparam=p\n$removeparam=q\n\
                @@||site.test/transformed/one^$removeparam=p\n\
                @@||site.test/transformed/doc^$document\n\
                @@||site.test/transformed/url^$urlblock\n\
                @@||site.test/transformed/basic^",
        ..Setup::default()
    })
    .await;
    for (which, suffix) in [
        ("one", "?p=1"),
        ("doc", "?p=1&q=2"),
        ("url", "?p=1&q=2"),
        ("basic", ""),
    ] {
        let path = format!("/transformed/{which}?p=1&q=2");
        let (_, status, body) = request(&rig, H1, Method::GET, &path, Some("document")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("/transformed/{which}{suffix}"));
    }
}
