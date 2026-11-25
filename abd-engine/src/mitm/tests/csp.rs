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

use hyper::header::CONTENT_SECURITY_POLICY;

use super::*;

#[tokio::test]
async fn csp_adds_a_header_to_documents_frames_and_assumed_html() {
    let rig = rig(Setup {
        rules: "||site.test^$csp=frame-src 'none'\n\
                ||site.test^$csp=script-src 'self' 'unsafe-eval' http: https:",
        settings: Some("cosmetic off\n"),
        ..Setup::default()
    })
    .await;
    for (path, dest, expected) in [
        ("/page/script", Some("document"), 2),
        ("/page/script", Some("iframe"), 2),
        ("/page/script", None, 2),
        ("/page/script", Some("script"), 1),
        ("/other", None, 0),
    ] {
        let (headers, status, _, _) = rig
            .request_response(Trust::System, H1, path, None, |mut req| {
                req.headers_mut().unwrap().remove("sec-fetch-dest");
                match dest {
                    Some(dest) => req.header("sec-fetch-dest", dest),
                    None => req,
                }
            })
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        let mut policies = headers.get_all(CONTENT_SECURITY_POLICY).iter();
        assert_eq!(
            headers.get_all(CONTENT_SECURITY_POLICY).iter().count(),
            expected,
            "{path} {dest:?}"
        );
        if expected > 0 {
            assert_eq!(
                policies.next().unwrap(),
                "script-src 'self'; style-src 'self'; object-src 'none'"
            );
        }
        if expected == 2 {
            let mut added = policies
                .next()
                .unwrap()
                .to_str()
                .unwrap()
                .split(',')
                .collect::<Vec<_>>();
            added.sort_unstable();
            assert_eq!(
                added,
                [
                    "frame-src 'none'",
                    "script-src 'self' 'unsafe-eval' http: https:"
                ]
            );
        }
    }
}

#[tokio::test]
async fn cosmetic_injection_permits_its_elements_in_the_server_and_list_policies() {
    let rig = rig(Setup {
        rules: "||site.test^$csp=default-src 'none'\n\
                site.test##.ad\nsite.test##+js(set-constant, fixture, 42)",
        ..Setup::default()
    })
    .await;
    let (headers, status, body, _) = rig
        .get_response(Trust::System, H1, "/page/script", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(".ad{display:none!important}"));
    assert!(body.contains(r#"setConstant("fixture", "42")"#));
    let policies: Vec<_> = headers
        .get_all(CONTENT_SECURITY_POLICY)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    assert_eq!(policies.len(), 2);
    assert!(policies[0].contains("object-src 'none'"));
    assert!(policies[1].contains("default-src 'none'"));
    for policy in policies {
        for directive in ["style-src", "script-src"] {
            assert!(
                policy.split(';').any(|part| {
                    part.trim_start().starts_with(directive) && part.contains("'sha256-")
                }),
                "{policy}"
            );
        }
    }
}

#[tokio::test]
async fn a_blocked_request_and_url_filtering_off_do_not_add_csp() {
    for (domain_only, rules, expected) in [
        (
            false,
            "||site.test/page^$document\n||site.test^$csp=frame-src 'none'",
            StatusCode::FORBIDDEN,
        ),
        (true, "||site.test^$csp=frame-src 'none'", StatusCode::OK),
    ] {
        let rig = rig(Setup {
            rules,
            domain_only,
            ..Setup::default()
        })
        .await;
        let (headers, status, _, _) = rig
            .get_response(
                if domain_only {
                    Trust::ServerOnly
                } else {
                    Trust::System
                },
                H1,
                "/page",
                None,
            )
            .await
            .unwrap();
        assert_eq!(status, expected);
        assert!(!headers.contains_key(CONTENT_SECURITY_POLICY));
        assert_eq!(rig.hits(), usize::from(status == StatusCode::OK));
    }
}
