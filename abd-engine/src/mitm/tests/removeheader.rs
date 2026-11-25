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
use tokio::io::{AsyncBufReadExt, BufReader};

pub(super) fn fixture(req: &Request<Incoming>) -> Option<Response<Full<Bytes>>> {
    let path = req.uri().path().strip_prefix("/headers/")?;
    let seen = |name: &str| {
        req.headers()
            .get(name)
            .map_or("absent", |value| value.to_str().unwrap())
    };
    let status = if path == "redirect" {
        StatusCode::FOUND
    } else {
        StatusCode::OK
    };
    Some(
        Response::builder()
            .status(status)
            .header("location", "/next")
            .header("content-type", "application/json")
            .header("x-seen-client-data", seen("x-client-data"))
            .header("x-seen-control", seen("x-control"))
            .header("x-request-encoding", seen("accept-encoding"))
            .body(Full::new(Bytes::from_static(br#"{"original":true}"#)))
            .unwrap(),
    )
}

async fn read_head(stream: &mut BufReader<TcpStream>) -> String {
    let mut head = String::new();
    loop {
        let start = head.len();
        assert_ne!(stream.read_line(&mut head).await.unwrap(), 0);
        if &head[start..] == "\r\n" {
            return head.to_ascii_lowercase();
        }
    }
}

#[tokio::test]
async fn header_only_removals_forward_the_head_before_the_unchanged_body() {
    let (rig, server) = plain_rig(Setup {
        rules: "||ads.example/asset$removeheader=X-Trace\n\
                ||ads.example/asset$removeheader=request:X-Client-Data",
        ..Setup::default()
    })
    .await;
    let (release, wait) = tokio::sync::oneshot::channel();
    let payload = b"\0original\xff\r\n";
    let upstream = tokio::spawn(async move {
        let mut tcp = BufReader::new(server.accept().await.unwrap().0);
        let head = read_head(&mut tcp).await;
        assert!(!head.contains("x-client-data:"), "{head}");
        assert!(head.contains("x-control: keep\r\n"), "{head}");
        assert!(head.contains("accept-encoding: gzip, br\r\n"), "{head}");
        tcp.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/plain\r\n\
             Content-Length: {}\r\nX-Trace: one\r\nX-Trace: two\r\nX-Control: keep\r\n\r\n",
                payload.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        wait.await.unwrap();
        tcp.write_all(payload).await.unwrap();
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut app = BufReader::new(TcpStream::connect(rig.proxy).await.unwrap());
        app.write_all(
            b"GET /asset HTTP/1.1\r\nHost: ads.example\r\nConnection: close\r\n\
              Accept-Encoding: gzip, br\r\nX-Client-Data: one\r\nX-Client-Data: two\r\nX-Control: keep\r\n\r\n",
        ).await.unwrap();
        let head = read_head(&mut app).await;
        assert!(head.starts_with("http/1.1 200 "), "{head}");
        assert!(!head.contains("x-trace:"), "{head}");
        assert!(head.contains("x-control: keep\r\n"), "{head}");
        assert!(head.contains(&format!("content-length: {}\r\n", payload.len())), "{head}");
        release.send(()).unwrap();
        let mut received = Vec::new();
        app.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, payload);
        upstream.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn a_redirect_loses_location_and_head_and_range_can_use_header_rules() {
    let rig = rig(Setup {
        rules: "||site.test/headers/$removeheader=location",
        ..Setup::default()
    })
    .await;
    for (method, range, status, body) in [
        ("GET", false, StatusCode::FOUND, r#"{"original":true}"#),
        ("HEAD", false, StatusCode::OK, ""),
        ("GET", true, StatusCode::OK, r#"{"original":true}"#),
    ] {
        let path = if status == StatusCode::FOUND {
            "/headers/redirect"
        } else {
            "/headers/asset"
        };
        let (headers, actual, received, real) = rig
            .request_response(Trust::System, H1, path, None, |req| {
                let req = req.method(method);
                if range {
                    req.header("range", "bytes=0-2")
                } else {
                    req
                }
            })
            .await
            .unwrap();
        assert!(!real);
        assert_eq!(actual, status);
        assert_eq!(received, body);
        assert!(!headers.contains_key("location"));
        assert_eq!(headers["content-type"], "application/json");
    }
}

#[tokio::test]
async fn removal_rules_leave_blocked_requests_at_the_proxy() {
    let rig = rig(Setup {
        rules: "||site.test/headers/blocked^\n\
                ||site.test/headers/$removeheader=location\n\
                ||site.test/headers/$removeheader=request:x-client-data",
        ..Setup::default()
    })
    .await;
    let (status, body, _) = rig
        .get(Trust::System, H1, "/headers/blocked", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.is_empty());
    assert_eq!(rig.hits(), 0);
}

#[tokio::test]
async fn request_and_response_headers_use_the_type_known_at_their_own_stage() {
    let rig = rig(Setup {
        alpn: BOTH,
        rules: "||site.test/headers/$removeheader=location,xmlhttprequest\n\
                ||site.test/headers/$removeheader=request:x-client-data,xmlhttprequest\n\
                ||site.test/headers/$removeheader=request:x-control,other\n\
                ||site.test/headers/$removeheader=x-seen-control,other",
        ..Setup::default()
    })
    .await;
    let (headers, status, body, _) = rig
        .request_response(Trust::System, BOTH, "/headers/asset", None, |req| {
            req.header("accept-encoding", "gzip, br")
                .header("x-client-data", "kept")
                .header("x-control", "removed")
        })
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, r#"{"original":true}"#);
    assert_eq!(headers["x-seen-client-data"], "kept");
    assert_eq!(headers["x-seen-control"], "absent");
    assert_eq!(headers["x-request-encoding"], "gzip, br");
    assert!(!headers.contains_key("location"));
}

#[tokio::test]
async fn a_response_can_lose_a_header_and_have_its_body_replaced() {
    let rig = rig(Setup {
        rules: "||site.test/headers/$removeheader=location,xmlhttprequest\n\
                ||site.test/headers/$replace=/original/changed/,xmlhttprequest",
        ..Setup::default()
    })
    .await;
    let (headers, status, body, _) = rig
        .get_response(Trust::System, H1, "/headers/asset", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key("location"));
    assert_eq!(headers["x-request-encoding"], "identity");
    assert_eq!(body, r#"{"changed":true}"#);
}
