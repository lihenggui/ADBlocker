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

mod tests {
    use adblock::filters::network::NetworkFilter;

    use super::super::*;
    use crate::list_files::MAX_LISTS;
    use crate::rule;

    fn network(line: &str) -> Option<NetworkFilter<'_>> {
        rule::parse_network(line)
    }

    mod host_rules {
        use super::*;

        pub(super) fn guards_exception(line: &str, parsed: &NetworkFilter<'_>) -> bool {
            crate::filter::host_rules::can_guard(&rule::read(line), parsed)
        }

        pub(super) fn project_guard(line: &str, parsed: &NetworkFilter<'_>, out: &mut String) {
            crate::filter::host_rules::write_guards(&rule::read(line), parsed, out);
        }
    }

    #[test]
    fn early_blocks_use_only_the_guards_the_app_can_need() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for (key, (exception, without_page, with_page, unread)) in [
            (
                "@@|https://$xmlhttprequest,domain=files.example",
                [true, true],
                [true, true],
                [true, true],
            ),
            (
                "@@||ads.example/ok.js$domain=news.example",
                [true, true],
                [false, true],
                [true, true],
            ),
            (
                "@@/consent.$script",
                [true, true],
                [true, true],
                [true, true],
            ),
            (
                "@@||ads.example/ok.js",
                [false, true],
                [false, true],
                [false, true],
            ),
            (
                "@@||ads.example/ok.js$domain=~news.example",
                [false, true],
                [false, true],
                [false, true],
            ),
            (
                "@@|https://ads.example/|",
                [false, true],
                [false, true],
                [false, true],
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let list = format!("||ads.example^\n||other.example^\n{exception}\n");
            std::fs::write(dir.path().join("list.txt"), list).unwrap();
            let built = Filters::load(&[dir.path()], false).unwrap();
            let key = key as u64;
            built.write_cache(cache.path(), key);
            let kept = Filters::read_cache(cache.path(), key).unwrap();
            for filters in [built, kept] {
                let native = filters.for_app_with_pages("com.native", false);
                let browser = filters.for_app_with_pages("com.browser", true);
                for (i, host) in ["ads.example", "other.example"].into_iter().enumerate() {
                    assert_eq!(
                        native.blocks_host(host),
                        without_page[i],
                        "{exception} {host}"
                    );
                    assert_eq!(
                        browser.blocks_host(host),
                        with_page[i],
                        "{exception} {host}"
                    );
                    assert_eq!(
                        filters.blocks_host(host),
                        with_page[i],
                        "{exception} {host}"
                    );
                    assert_eq!(browser.blocks_unread(host), unread[i], "{exception} {host}");
                    assert!(browser.blocks_domain(host), "{exception} {host}");
                }
            }
        }
    }

    const DESTINATION_PROJECTIONS: &[(&str, &str, &str)] = &[
        ("@@||google.*/imgres?", "@@||google.*^", "google.co.uk"),
        ("@@||pornhub.*/signup", "@@||pornhub.*^", "pornhub.com"),
        (
            "@@||ebay.*/makeOffer/$document,subdocument",
            "@@||ebay.*^",
            "www.ebay.de",
        ),
        (
            "@@||cdn*.downdetector.com/*/javascript*/adscript.js",
            "@@||cdn*.downdetector.com^",
            "cdn42.downdetector.com",
        ),
        (
            "@@||animetake*.*^$script,1p",
            "@@||animetake*.*^",
            "animetake42.net",
        ),
        (
            "@@||s*.rea.reastatic.net/rs/rui_advertorial.js",
            "@@||s*.rea.reastatic.net^",
            "s42.rea.reastatic.net",
        ),
        ("@@||i*.gmx.com/image", "@@||i*.gmx.com^", "i42.gmx.com"),
        (
            "@@||notebookcheck*/fileadmin/scripts/pagecall_dfp_async.js",
            "@@||notebookcheck*^",
            "notebookcheck.net",
        ),
        (
            "@@://www.googleadservices.com^|",
            "@@||www.googleadservices.com^",
            "www.googleadservices.com",
        ),
        (
            "@@://www.exoclick.com/ads.js|",
            "@@||www.exoclick.com^",
            "www.exoclick.com",
        ),
        (
            "@@doubleclick.net/ads/preferences/",
            "@@||doubleclick.net^",
            "doubleclick.net",
        ),
        (
            "@@weather.gov/xml/$document",
            "@@||weather.gov^",
            "weather.gov",
        ),
        (
            "@@app.arizonawildcats.com/preferences",
            "@@||app.arizonawildcats.com^",
            "app.arizonawildcats.com",
        ),
        (
            "@@://xhamster.$script,~third-party",
            "@@||xhamster.*^",
            "xhamster.com",
        ),
        (
            "@@|https:///ovh.webshark.pl/adserver/*/main_script.js?advertise_check=1",
            "@@||ovh.webshark.pl^",
            "ovh.webshark.pl",
        ),
        ("@@/wp-content/themes/mts_ad_sense/*$1p", "", "ads.example"),
        (
            "@@/wp-content/plugins/blockalyzer-adblock-counter/*$image,script,~third-party,domain=~gaytube.com|~elsewhere.example",
            "",
            "ads.example",
        ),
        (
            "@@|/wp-content/plugins/*/assets/fuckadblock.js",
            "",
            "ads.example",
        ),
        ("@@||192.168.$~3p", "", "ads.example"),
        ("@@ads.js", "", "ads.js"),
        ("@@main.js?x", "", "main.js"),
        ("@@||*^", "", "ads.example"),
    ];

    #[test]
    fn exceptions_guard_only_the_hosts_they_name() {
        for &(exception, guard, host) in DESTINATION_PROJECTIONS {
            let parsed = network(exception).unwrap();
            assert!(
                host_rules::guards_exception(exception, &parsed),
                "{exception}"
            );
            let mut projected = String::new();
            host_rules::project_guard(exception, &parsed, &mut projected);
            assert_eq!(projected.trim_end(), guard, "{exception}");
            let filters = filters(&format!("||{host}^\n||unrelated.example^\n{exception}\n"));
            for has_shown_page in [false, true] {
                let app = filters.for_app_with_pages("com.app", has_shown_page);
                assert_eq!(
                    app.blocks_host(host),
                    guard.is_empty(),
                    "{exception} on {host}"
                );
                assert!(app.blocks_host("unrelated.example"), "{exception}");
            }
        }
    }

    const WHOLE_HOST_GUARD_CASES: &[(&str, &str, &str, Option<&str>)] = &[
        (
            "ads.example",
            "@@||ads.example/ok.js",
            "ads.example",
            Some("cdn.ads.example"),
        ),
        (
            "janapi.jd.com",
            "@@||jd.com/adclick?$~third-party",
            "jd.com",
            None,
        ),
        (
            "janapi.jd.com",
            "@@||create.jd.com/ad/",
            "create.jd.com",
            None,
        ),
        (
            "janapi.jd.com",
            "@@||media.jd.com/static/*/ad",
            "media.jd.com",
            None,
        ),
        (
            "erebor.douban.com",
            "@@||douban.com/popup/login?",
            "douban.com",
            None,
        ),
        (
            "doubleclick.net",
            "@@doubleclick.net/ads/preferences/",
            "doubleclick.net",
            Some("googleads.g.doubleclick.net"),
        ),
        ("e.qq.com", "@@||qq.com/bossweb/jifen/", "qq.com", None),
        ("e.qq.com", "@@||qq.com/cgi-bin/*?sid=ads", "qq.com", None),
        ("e.qq.com", "@@||qq.com^*?ADTAG=", "qq.com", None),
        (
            "e.qq.com",
            "@@||adnet.qq.com^$~third-party",
            "adnet.qq.com",
            None,
        ),
        (
            "e.qq.com",
            "@@||sdk.e.qq.com/ok",
            "sdk.e.qq.com",
            Some("cdn.sdk.e.qq.com"),
        ),
    ];

    #[test]
    fn a_plain_host_guard_needs_a_whole_host_block_for_its_own_host() {
        for &(blocked, exception, named, guarded_below) in WHOLE_HOST_GUARD_CASES {
            let lists = [
                (Path::new("a-exceptions.txt"), format!("{exception}\n")),
                (
                    Path::new("b-blocks.txt"),
                    format!("||{blocked}^\n||unrelated.example^\n"),
                ),
            ];
            let filters = Filters::build(lists.into_iter().map(Ok), None, true, MAX_LISTS).unwrap();
            let request = Filters::host_request(named).unwrap();
            assert_eq!(
                unguarded(&filters.host_guards, &request),
                guarded_below.is_none(),
                "{exception}"
            );
            for has_shown_page in [false, true] {
                let app = filters.for_app_with_pages("com.app", has_shown_page);
                let refused = blocked != named || guarded_below.is_none();
                assert_eq!(
                    app.blocks_host(blocked),
                    refused,
                    "{exception} on {blocked}"
                );
                assert!(!app.blocks_host(named), "{exception} on {named}");
                assert!(app.blocks_host("unrelated.example"), "{exception}");
                if let Some(host) = guarded_below {
                    assert!(app.blocks_domain(host), "{exception} on {host}");
                    assert!(!app.blocks_host(host), "{exception} on {host}");
                }
            }
        }
    }

    #[test]
    fn a_disabled_parent_block_does_not_keep_a_guard_for_its_subdomains() {
        let filters = filters(
            "||jd.com^\n||janapi.jd.com^\n||unrelated.example^\n\
             @@||jd.com/adclick?\n||jd.com^$badfilter\n",
        );
        for has_shown_page in [false, true] {
            let app = filters.for_app_with_pages("com.app", has_shown_page);
            assert!(app.blocks_host("janapi.jd.com"));
            assert!(!app.blocks_host("jd.com"));
            assert!(app.blocks_host("unrelated.example"));
        }
    }

    #[test]
    fn a_plain_guard_covers_the_named_host_and_its_subdomains() {
        let filters = filters("||qq.com^\n||qq.com.example^\n@@||qq.com^*?ADTAG=\n");
        assert!(!filters.blocks_host("qq.com"));
        assert!(!filters.blocks_host("e.qq.com"));
        assert!(filters.blocks_host("qq.com.example"));
    }

    #[test]
    fn a_web_guard_for_an_app_block_keeps_the_apps_and_page_requirement() {
        let cache = tempfile::tempdir().unwrap();
        for (key, needs_page) in [false, true].into_iter().enumerate() {
            let condition = if needs_page {
                "$domain=files.example"
            } else {
                ""
            };
            let built = filters(&format!(
                "||qq.com^$app=com.reader\n||e.qq.com^\n@@||qq.com/ok{condition}\n"
            ));
            built.write_cache(cache.path(), key as u64);
            let kept = Filters::read_cache(cache.path(), key as u64).unwrap();
            for filters in [built, kept] {
                for has_shown_page in [false, true] {
                    let reader = filters.for_app_with_pages("com.reader", has_shown_page);
                    assert_eq!(
                        reader.blocks_host("e.qq.com"),
                        needs_page && !has_shown_page
                    );
                    assert_eq!(reader.blocks_unread("e.qq.com"), needs_page);
                    let other = filters.for_app_with_pages("com.other", has_shown_page);
                    assert!(other.blocks_host("e.qq.com"));
                    assert!(filters.blocks_host("e.qq.com"));
                }
            }
        }
    }

    #[test]
    fn an_incomplete_guard_budget_leaves_every_host_for_later() {
        let plain = "||ads.example^\n||unrelated.example^\n\
                     @@/image^$to=ads.example\n@@||unblocked.example/ok.js\n";
        let page = "@@||ads.example/ok.js$domain=news.example\n";
        let build = |budget| {
            Filters::build(
                [plain, page]
                    .map(|text| Ok((Path::new("list.txt"), text.to_owned())))
                    .into_iter(),
                None,
                false,
                budget,
            )
            .unwrap()
        };
        let total = 2 * "@@||ads.example^\n".len();
        let complete = build(total);
        assert!(complete.blocks_host("unrelated.example"));
        let incomplete = build(total - 1);
        let cache = tempfile::tempdir().unwrap();
        incomplete.write_cache(cache.path(), 1);
        let kept = Filters::read_cache(cache.path(), 1).unwrap();
        for filters in [incomplete, kept] {
            for has_shown_page in [false, true] {
                let app = filters.for_app_with_pages("com.app", has_shown_page);
                for host in ["ads.example", "unrelated.example"] {
                    assert!(!app.blocks_host(host));
                    assert!(!app.blocks_unread(host));
                    assert!(app.blocks_domain(host));
                }
            }
        }
    }

    #[test]
    fn a_new_generation_removes_stale_optional_engines() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let list = dir.path().join("list.txt");
        std::fs::write(
            &list,
            "||ads.example^\n@@||page.example^$document\n\
             @@||images.example^$ehide\n@@||news.example^$shide\n##div[data-ad]\n\
             @@||ads.example/ok.js$domain=news.example\n\
             ||whole-app.example^$app=com.reader\n@@||whole-app.example/ok\n\
             @@||whole-app.example/ok.js$domain=news.example\n",
        )
        .unwrap();
        let load = || Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        let first = load();
        let optional = [
            "no-cosmetic",
            "no-element-hiding",
            "no-specific-cosmetic",
            "generic-css",
            "page-host-guards",
            "app-host-guards",
            "app-page-host-guards",
        ];
        for name in optional {
            assert!(cache.path().join(name).is_file(), "{name}");
        }

        // What an earlier version kept for requests without a page.
        std::fs::write(cache.path().join("unknown"), "kept").unwrap();
        std::fs::write(&list, "||ads.example^\n").unwrap();
        let second = load();
        assert_ne!(first.generation, second.generation);
        for name in optional.into_iter().chain(["unknown"]) {
            assert!(!cache.path().join(name).exists(), "{name}");
        }
        assert!(Filters::read_cache(cache.path(), first.generation.unwrap()).is_none());
        let kept = Filters::read_cache(cache.path(), second.generation.unwrap()).unwrap();
        assert!(kept.blocks_host("ads.example"));
    }

    #[test]
    fn a_damaged_cache_is_not_used() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(dir.path().join("list.txt"), "||one.example^\n").unwrap();
        let load = || Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        load();
        let full = cache.path().join("full");
        let mut bytes = std::fs::read(&full).unwrap();
        bytes.truncate(bytes.len() / 2);
        std::fs::write(&full, bytes).unwrap();
        // Read from the lists again, and right.
        assert!(load().blocks_host("one.example"));
        std::fs::write(cache.path().join("hosts"), b"not an engine").unwrap();
        assert!(load().blocks_host("one.example"));
    }

    #[test]
    fn what_is_not_an_address_is_let_through() {
        let f = filters("||ads.example^\n");
        assert!(
            !f.check("not an address", "also not", "other", "GET")
                .blocked
        );
        assert!(!f.blocks_host(""));
    }

    #[test]
    fn reusing_the_parsed_page_keeps_request_context_and_canonicalization() {
        let f = filters(
            "||ads.example^$third-party\n@@||ads.example/keep$domain=news.example\n\
             ||news.example/banner$first-party",
        );
        for url in [
            "https://AdS.Example/ad.js",
            "https://ads.example/keep",
            "https://news.example/banner",
        ] {
            for source in [
                "https://news.example/page",
                "https://ads.example",
                "",
                "file:///page",
            ] {
                for document in [false, true] {
                    let page = parse_http_page(source);
                    let told = match (document, page.as_ref()) {
                        (true, _) => Page::Itself,
                        (false, Some(page)) => Page::Named(page),
                        (false, None) => Page::NONE,
                    };
                    let actual = f.for_any_app().check_with_page(url, told, "script", "GET");
                    let expected =
                        f.check(url, if document { url } else { source }, "script", "GET");
                    assert_eq!(
                        (actual.blocked, actual.rule),
                        (expected.blocked, expected.rule)
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "a measurement, run by hand"]
    fn measures_reusing_request_page_parsing() {
        let f = filters("||ads.example^$third-party");
        let url = "https://ads.example/banner";
        let source = "https://news.example/story";
        let count = 50_000;
        let start = std::time::Instant::now();
        for _ in 0..count {
            // The old request path parsed the header for matching and
            // again for recording, in addition to the matcher's parsing.
            assert!(std::hint::black_box(parse_http_page(source)).is_some());
            assert!(f.check(url, source, "image", "GET").blocked);
            assert!(std::hint::black_box(parse_http_page(source)).is_some());
        }
        println!(
            "separate request page parsing: {:?}/request",
            start.elapsed() / count
        );
        let start = std::time::Instant::now();
        for _ in 0..count {
            let page = std::hint::black_box(parse_http_page(source));
            assert!(
                f.for_any_app()
                    .check_with_page(
                        url,
                        page.as_ref().map_or(Page::NONE, Page::Named),
                        "image",
                        "GET"
                    )
                    .blocked
            );
        }
        println!(
            "shared request page parsing: {:?}/request",
            start.elapsed() / count
        );
    }
}
