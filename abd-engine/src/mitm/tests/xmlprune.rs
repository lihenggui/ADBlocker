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

const RULES: &str = r#".mpd$xmlprune=//*[name()="Period"][.//*[@value="Ad"]] | //*[name()="Period"]/@start[boolean(//*[@value="Ad"])],xmlhttprequest,domain=amazon.com|amazon.co.jp|amazon.de|amazon.co.uk|primevideo.com
/manifest.mpd$xmlprune=/MPD/Period[contains(@id\,'ad-')],domain=hulu.com"#;
const NS: &str = " xmlns='urn:mpeg:dash:schema:mpd:2011'";
const AD: &str = "<Period id='ad-one' start='PT0S'><Event value='Ad'/></Period>";
const MAIN: &str = "<Period id='content' start='PT15S'><Event value='Content'/></Period>";
const MAIN_NO_START: &str = "<Period id='content'><Event value='Content'/></Period>";

fn manifest(namespace: bool, ad: &str, main: &str) -> String {
    let ns = if namespace { NS } else { "" };
    format!("<MPD{ns}>{ad}{main}</MPD>")
}

pub(super) fn fixture(req: &Request<Incoming>) -> Option<Response<Full<Bytes>>> {
    let mime = match req.uri().path() {
        "/dash/manifest.mpd" => "Application/Dash+Xml; charset=utf-8",
        "/dash/plain/manifest.mpd" => "text/plain",
        "/dash/text/manifest.mpd" => "text/xml",
        "/dash/xml/manifest.mpd" => "application/xml",
        _ => return None,
    };
    let text = manifest(req.uri().query() != Some("no-namespace"), AD, MAIN);
    Some(
        Response::builder()
            .header("etag", "\"dash\"")
            .header("content-type", mime)
            .header("content-length", text.len())
            .body(Full::new(Bytes::from(text)))
            .unwrap(),
    )
}

#[tokio::test]
async fn real_dash_rules_use_the_request_type_and_xpath_namespace_rules() {
    let checked = crate::rulecheck::check(RULES);
    assert_eq!(checked.rules, 2);
    assert!(checked.issues.is_empty());
    let rig = rig(Setup {
        rules: RULES,
        ..Setup::default()
    })
    .await;
    for namespace in [false, true] {
        let path = if namespace {
            "/dash/manifest.mpd"
        } else {
            "/dash/manifest.mpd?no-namespace"
        };
        for (page, dest, ad, main) in [
            (
                "https://www.amazon.com/watch",
                Some("empty"),
                "",
                MAIN_NO_START,
            ),
            ("https://www.amazon.com/watch", None, AD, MAIN),
            ("https://www.amazon.com/watch", Some("video"), AD, MAIN),
            (
                "https://www.hulu.com/watch",
                Some("empty"),
                if namespace { AD } else { "" },
                MAIN,
            ),
            (
                "https://www.hulu.com/watch",
                None,
                if namespace { AD } else { "" },
                MAIN,
            ),
            (
                "https://www.hulu.com/watch",
                Some("video"),
                if namespace { AD } else { "" },
                MAIN,
            ),
        ] {
            let (_, status, body, real) = rig
                .request_response(Trust::System, H1, path, Some(page), |req| {
                    if let Some(dest) = dest {
                        req.header("sec-fetch-dest", dest)
                    } else {
                        req
                    }
                })
                .await
                .unwrap();
            assert_eq!(status, StatusCode::OK, "{path} {page} {dest:?}");
            assert!(!real);
            assert_eq!(
                body,
                manifest(namespace, ad, main),
                "{path} {page} {dest:?}"
            );
        }
    }
}

#[tokio::test]
async fn xmlprune_edits_only_xml_media_types_and_precedes_replace() {
    let rig = rig(Setup {
        rules: r"||site.test/dash/$xmlprune=//*[name()='Period'][contains(@id\,'ad-')]
||site.test/dash/$replace=/ad-one/would-keep/",
        ..Setup::default()
    })
    .await;
    for path in [
        "/dash/manifest.mpd",
        "/dash/text/manifest.mpd",
        "/dash/xml/manifest.mpd",
    ] {
        let (_, status, body, _) = rig
            .get_response(Trust::System, H1, path, None)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, manifest(true, "", MAIN), "{path}");
    }
    let rig = self::rig(Setup {
        rules: "||site.test/dash/$xmlprune=//*[name()='Period']",
        ..Setup::default()
    })
    .await;
    let (headers, status, body, _) = rig
        .get_response(Trust::System, H1, "/dash/plain/manifest.mpd", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, manifest(true, AD, MAIN));
    assert_eq!(headers["etag"], "\"dash\"");
    assert!(headers.contains_key("content-length"));
}
