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

mod properties {
    use proptest::prelude::*;

    use super::super::*;
    use crate::rule::{Kind, read};
    use crate::test_support::arbitrary_text;

    fn url() -> impl Strategy<Value = String> {
        let structured = (
            prop::sample::select(vec!["http", "https", "ftp", "HTTPS"]),
            prop::sample::select(vec![
                "a.test",
                "a.test:443",
                "[::1]",
                "user@a.test",
                "a.test:65536",
            ]),
            prop::sample::select(vec!["/old/", "/%6Fld/", "/%0A", "/%2F", "/a%5Cb", "/", ""]),
            "[a-z0-9]{0,20}",
            prop::sample::select(vec!["?ad=1&keep=2", "#fragment", "?x=%FF", "", " "]),
        )
            .prop_map(|(scheme, host, path, leaf, suffix)| {
                format!("{scheme}://{host}{path}{leaf}{suffix}")
            });
        prop_oneof![
            arbitrary_text(),
            structured,
            (MAX_TRANSFORM_URL - 20..MAX_TRANSFORM_URL + 1)
                .prop_map(|size| format!("https://a.test/old/{}", "x".repeat(size))),
        ]
    }

    fn transforms() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec(
            prop_oneof![
                arbitrary_text(),
                prop::sample::select(vec![
                    "pct",
                    "pct|b64",
                    "/old/new/",
                    "/old/longer/g",
                    r"/^https:\/\/a.test/https:\/\/b.test/",
                    r"/^https:\/\/a.test/ftp:\/\/b.test/",
                    r"/^https:\/\/a.test/https:\/\/user@b.test/",
                    r"/^http.*$/javascript:bad/",
                    "/[unterminated/x/",
                    "/a/b/|",
                ])
                .prop_map(str::to_owned),
            ],
            1..4,
        )
    }

    fn compile(value: &str) -> Option<Arc<ResponseAction>> {
        let line = format!("||a.test^$urltransform={value}");
        let Kind::Response(rule) = read(&line).kind else {
            return None;
        };
        rule.action().ok()?.spec?.compile().map(Arc::new)
    }

    proptest! {
        #[test]
        fn a_request_transform_returns_unchanged_or_a_bounded_http_url(
            original in url(), values in transforms(), rewritten in prop::option::of(url()),
            is_get in any::<bool>(),
        ) {
            let actions: Vec<_> = values.iter().filter_map(|value| compile(value)).collect();
            let target = original.parse::<Uri>().unwrap_or_else(|_| Uri::from_static("/"));
            let method = if is_get { Method::GET } else { Method::POST };
            let result = apply(&original, &target, &method, rewritten.as_deref(), &actions);
            let url = match result {
                None => return Ok(()), // The caller forwards the original URL.
                Some(Transformed::Forward { url, .. }) => url,
                Some(Transformed::Redirect(value)) => value.to_str().unwrap().to_owned(),
            };
            prop_assert!(url.len() <= MAX_TRANSFORM_URL);
            let parsed = url.parse::<Uri>().unwrap();
            prop_assert!(matches!(parsed.scheme_str(), Some("http" | "https")));
            prop_assert!(parsed.host().is_some_and(|host| !host.is_empty()));
            prop_assert!(url != original);
        }

        #[test]
        fn a_valid_same_origin_substitution_rewrites_the_request_target(leaf in "[a-z0-9]{1,20}") {
            let original = format!("https://a.test/old/{leaf}");
            let target = format!("/old/{leaf}").parse().unwrap();
            let actions = [compile("/old/new/").unwrap()];
            let result = apply(&original, &target, &Method::GET, None, &actions);
            let Some(Transformed::Forward { url, uri }) = result else {
                return Err(TestCaseError::fail("expected a forwarded request"));
            };
            prop_assert_eq!(url, format!("https://a.test/new/{leaf}"));
            prop_assert_eq!(uri.path(), format!("/new/{leaf}"));
        }
    }
}
