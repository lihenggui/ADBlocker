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

use std::path::Path;

use super::*;

fn filters(rules: &str) -> Filters {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("list.txt"), rules).unwrap();
    Filters::load(&[dir.path()], true).unwrap()
}

mod guards;
mod javascript;
mod redirects;

#[test]
fn conditional_rules_and_badfilters_survive_a_cache_round_trip() {
    let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        dir.path().join("a.txt"),
        "!#if env_chromium\n||active.example/asset\n||whole.example^\n!#else\n\
         ||inactive.example/asset\n||whole.example^$badfilter\n!#endif\n!#if false\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("b.txt"), "||next.example^\n").unwrap();

    let built = Filters::load_cached(&[dir.path()], cache.path()).unwrap();
    let cached = Filters::read_cache(cache.path(), built.generation.unwrap()).unwrap();
    for filters in [built, cached] {
        assert_eq!(filters.rules, 3);
        assert!(filters.blocks_host("whole.example"));
        assert!(filters.blocks_host("next.example"));
        let check = |host| {
            filters
                .check(&format!("https://{host}/asset"), "", "script", "GET")
                .blocked
        };
        assert!(check("active.example"));
        assert!(!check("inactive.example"));
    }
}

#[test]
fn inactive_generic_cosmetics_stay_out_of_the_specific_hide_exception_engine() {
    let filters = filters(
        "@@||page.example^$specifichide\n\
         !#if env_mobile\n##.active\n!#else\n##.inactive\n!#endif\n",
    );
    let hiding = filters.hiding("https://page.example/");
    assert_eq!(
        filters
            .hiding_by_name(
                &hiding,
                &HashSet::from(["active".to_owned(), "inactive".to_owned()]),
                &HashSet::new(),
            )
            .as_deref(),
        Some(".active{display:none!important}\n")
    );
}

#[test]
fn matcher_negated_to_excludes_destinations_and_their_subdomains() {
    crate::public_suffix::install();
    for (rule, cases) in [
        (
            "*$script,domain=a.com,to=~x.com,important\n@@*$script,domain=a.com",
            &[
                ("y.com", "a.com", "script", true),
                ("sub.y.com", "a.com", "script", true),
                ("x.com", "a.com", "script", false),
                ("sub.x.com", "a.com", "script", false),
                ("a.com", "a.com", "script", true),
                ("y.com", "b.com", "script", false),
                ("y.com", "a.com", "image", false),
            ][..],
        ),
        (
            "*$script,domain=a.com,to=~x.com,first-party",
            &[
                ("sub.a.com", "a.com", "script", true),
                ("y.com", "a.com", "script", false),
            ],
        ),
        (
            "*$script,domain=a.com,to=~x.com,third-party",
            &[
                ("sub.a.com", "a.com", "script", false),
                ("y.com", "a.com", "script", true),
            ],
        ),
    ] {
        let engine = Engine::new_with_list_text(rule);
        for &(target, source, kind, blocked) in cases {
            let request = Request::new(
                &format!("https://{target}/asset"),
                &format!("https://{source}/"),
                kind,
                "GET",
            )
            .unwrap();
            assert_eq!(
                engine.check_network_request(&request).should_block(),
                blocked,
                "{rule}: {source} -> {target} ({kind})"
            );
        }
    }
}

#[test]
fn denyallow_rules_filter_requests_without_refusing_hosts() {
    let script = "*$script,domain=a.com|b.com,denyallow=x.com|y.com";
    let image = "$image,third-party,denyallow=googleapis.com,domain=vipbox1.org";
    let scoped = "/assets/ad.js$script,app=com.reader,denyallow=x.com";
    let rules = format!("{script}\n{image}\n{scoped}\n");
    let f = filters(&rules);
    assert!(crate::rulecheck::check(&rules).issues.is_empty());
    assert_eq!(f.host_rules, 0);
    for host in [
        "a.com",
        "x.com",
        "cdn.x.com",
        "y.com",
        "googleapis.com",
        "z.com",
    ] {
        assert!(!f.blocks_host(host));
        assert!(!f.for_app("com.reader").blocks_host(host));
        assert!(!f.for_app("com.reader").blocks_unread(host));
    }
    for (target, page, kind, matched) in [
        ("z.com", "a.com", "script", Some(script)),
        ("z.com", "b.com", "script", Some(script)),
        ("z.com", "elsewhere.com", "script", None),
        ("x.com", "a.com", "script", None),
        ("sub.y.com", "b.com", "script", None),
        ("z.com", "vipbox1.org", "image", Some(image)),
        ("cdn.googleapis.com", "vipbox1.org", "image", None),
        ("cdn.vipbox1.org", "vipbox1.org", "image", None),
        ("z.com", "vipbox1.org", "script", None),
    ] {
        let check = f.check(
            &format!("https://{target}/asset"),
            &format!("https://{page}/"),
            kind,
            "GET",
        );
        assert_eq!(check.blocked, matched.is_some(), "{target} from {page}");
        assert_eq!(check.rule.as_deref(), matched);
    }
    for (app, target, matched) in [
        ("com.reader", "z.com", Some(scoped)),
        ("com.reader", "x.com", None),
        ("com.reader", "cdn.x.com", None),
        ("com.other", "z.com", None),
    ] {
        let check = f.for_app(app).check_with_page(
            &format!("https://{target}/assets/ad.js"),
            Page::NONE,
            "script",
            "GET",
        );
        assert_eq!(check.blocked, matched.is_some(), "{app} to {target}");
        assert_eq!(check.rule.as_deref(), matched);
    }
}

#[test]
fn noop_rules_keep_host_app_page_and_badfilter_behavior() {
    let f = filters(
        "||ads.com^$_,_____\n\
         ||appads.com^$__,app=com.reader,_\n\
         ||disabled.com^$_\n\
         ||disabled.com^$__,badfilter,_\n\
         ##.ad\n\
         @@||ads.com^$___,elemhide,_",
    );
    assert!(f.blocks_host("ads.com"));
    assert!(!f.blocks_host("disabled.com"));
    assert!(f.for_app("com.reader").blocks_host("appads.com"));
    assert!(!f.for_app("com.other").blocks_host("appads.com"));
    assert!(!f.hiding("https://ads.com/").by_name());
    assert!(f.hiding("https://elsewhere.com/").by_name());
}

#[test]
fn real_noop_rules_only_apply_when_their_other_options_are_supported() {
    let shinden = r"$script,third-party,denyallow=ckeditor.com|fastly.net|statically.io|sharecast.ws|bunnycdn.ru|bootstrapcdn.com|cdn.ampproject.org|cloudflare.com|cdn.staticfile.org|disqus.com|disquscdn.com|dmca.com|ebacdn.com|facebook.net|fbcdn.net|fluidplayer.com|fontawesome.com|github.io|google.com|googleapis.com|googletagmanager.com|gstatic.com|jquery.com|jsdelivr.net|jwpcdn.com|jwplatform.com|recaptcha.net|shrink.pe|twitter.com|ulogin.ru|unpkg.com|userapi.com|vidazoo.com|vk.com|yastatic.net|ytimg.com|zencdn.net|youtube.com|cackle.me|googleoptimize.com|vuukle.com|chatango.com|twimg.com|google-analytics.com|hcaptcha.com|media-imdb.com|blogger.com|hwcdn.net|instagram.com|wp.com|fastcomments.com|plyr.io,_____,domain=shinden.pl";
    let vivicomi = r"*$script,third-party,denyallow=tvableon.me|mega.co.nz|mega.nz|libs.baidu.com|ilfcdn.li|bytecdntp.com|doodcdn.io|intensedebate.com|vixcloud.co|rsc.cdn77.org|linkvertise.com|fastly.net|statically.io|sharecast.ws|b-cdn.net|bunnycdn.ru|bootstrapcdn.com|cdn.ampproject.org|cloudflare.com|cdn.staticfile.org|disqus.com|disquscdn.com|dmca.com|ebacdn.com|facebook.net|fastlylb.net|fbcdn.net|fluidplayer.com|fontawesome.com|github.io|google.com|googleapis.com|googletagmanager.com|gstatic.com|jquery.com|jsdelivr.net|jwpcdn.com|jwplatform.com|jwplayer.com|recaptcha.net|shrink.pe|twitter.com|ulogin.ru|unpkg.com|userapi.com|vidazoo.com|vk.com|yastatic.net|ytimg.com|zencdn.net|youtube.com|cackle.me|googleoptimize.com|vuukle.com|chatango.com|twimg.com|google-analytics.com|hcaptcha.com|media-imdb.com|blogger.com|hwcdn.net|instagram.com|wp.com|fastcomments.com|plyr.io|imgsmail.ru|cloudflare.net|rabbitstream.net|x.com|ko-fi.com|wsimg.com|sharethis.com|utteranc.es|cloudflareinsights.com|tailwindcss.com|capo6play.com|jpg5.su|jpg6.su|tolstoycomments.com|code.iconify.design|alicdn.com|addtoany.com|waust.at|tenor.com|pinterest.com,_____,domain=/^vivicomi\d+\.info$/";
    let rule = crate::rule::read(vivicomi);
    assert!(matches!(
        rule.kind,
        crate::rule::Kind::NotApplied(crate::rule::Reason::RegexSourceDomain)
    ));
    let text = format!("{shinden}\n{vivicomi}");
    let checked = crate::rulecheck::check(&text);
    assert_eq!(checked.issues.len(), 1);
    assert_eq!(checked.issues[0].text, vivicomi);

    let f = filters(&text);
    let blocked = f.check(
        "https://outside.example/ad.js",
        "https://shinden.pl/",
        "script",
        "GET",
    );
    assert!(blocked.blocked);
    assert_eq!(blocked.rule.as_deref(), Some(shinden));
    assert!(
        !f.check(
            "https://cdn.googleapis.com/ad.js",
            "https://shinden.pl/",
            "script",
            "GET"
        )
        .blocked
    );
    assert!(
        !f.check(
            "https://outside.example/ad.js",
            "https://vivicomi12.info/",
            "script",
            "GET"
        )
        .blocked
    );
}

mod lists {
    use super::*;

    fn host_only(line: &str) -> Option<String> {
        crate::rule::read(line)
            .whole_host()
            .map(|(rule, _)| rule.into_owned())
    }

    const HIDING: &str = "site.example##.ad\n\
                          site.example,other.example##a[href^=\"https://ads.example/\"]\n\
                          other.example##.banner\n\
                          ##div[data-ad]\n\
                          ##.by-class\n\
                          excepted.example#@#div[data-ad]\n";

    #[test]
    fn scriptlets_and_procedural_rules_respect_cosmetic_exceptions() {
        let base = "site.test##+js(set-constant, adbFixture, 42)\nsite.test##.ad\nsite.test##div:has-text(Advert)\n##.generic\n";
        let active = filters(base).hiding("https://site.test/");
        let script = active.script.unwrap();
        assert!(
            script.contains(r#"setConstant("adbFixture", "42")"#),
            "{script}"
        );
        assert!(script.contains(r#"{"type":"has-text","arg":"Advert"}"#));
        for (exception, scriptlet, procedural, specific, generic) in [
            ("site.test#@#+js()", false, true, true, true),
            (
                "site.test#@#+js(set-constant, adbFixture, 42)",
                false,
                true,
                true,
                true,
            ),
            ("site.test#@#div:has-text(Advert)", true, false, true, true),
            ("site.test#@#.ad", true, true, false, true),
            ("@@||site.test^$generichide", true, true, true, false),
            ("@@||site.test^$ghide", true, true, true, false),
            ("@@||site.test^$specifichide", true, false, false, true),
            ("@@||site.test^$shide", true, false, false, true),
            ("@@||site.test^$elemhide", true, false, false, false),
            ("@@||site.test^$ehide", true, false, false, false),
            ("@@||site.test^$shide,ghide", true, false, false, false),
            ("@@||site.test^$document", false, false, false, false),
            ("@@||site.test^$ehide,doc", false, false, false, false),
            ("@@||site.test^$shide,ehide", true, false, false, false),
            ("@@||site.test/allowed$document", true, true, true, true),
        ] {
            let filters = filters(&format!("{base}{exception}\n"));
            let found = filters.hiding("https://site.test/");
            assert_eq!(
                found.script.is_some(),
                scriptlet || procedural,
                "{exception}"
            );
            let script = found.script.as_deref().unwrap_or_default();
            assert_eq!(script.contains("adbFixture"), scriptlet, "{exception}");
            assert_eq!(
                script.contains(r#"arg":"Advert""#),
                procedural,
                "{exception}"
            );
            assert_eq!(found.css.is_some(), specific, "{exception}");
            assert_eq!(found.by_name(), generic, "{exception}");
        }
    }

    #[test]
    fn only_a_fresh_catalog_list_can_invoke_trusted_scriptlets_even_after_cache_reload() {
        let dir = tempfile::tempdir().unwrap();
        let lists = dir.path().join("lists");
        let rules = dir.path().join("rules");
        let cache = dir.path().join("cache");
        std::fs::create_dir(&lists).unwrap();
        std::fs::create_dir(&rules).unwrap();
        let stock = "https://ublockorigin.github.io/uAssetsCDN/filters/filters.min.txt";
        let name = crate::list_url::file_name(stock);
        let custom = crate::list_url::file_name(&format!("{stock}?custom"));
        let mobile = "https://filters.adtidy.org/android/filters/11.txt";
        let base = "https://filters.adtidy.org/android/filters/2.txt";
        let chinese = "https://filters.adtidy.org/android/filters/224.txt";
        let tracking = "https://filters.adtidy.org/extension/ublock/filters/3.txt";
        let downloaded = |url| lists.join(crate::list_url::file_name(url));
        for (path, host) in [
            (lists.join(custom), "custom.test"),
            (rules.join(&name), "user.test"),
            (lists.join(name), "catalog.test"),
            (downloaded(mobile), "mobile.test"),
            (downloaded(base), "base.test"),
            (downloaded(chinese), "chinese.test"),
            (downloaded(tracking), "tracking.test"),
            (
                lists.join(crate::list_url::base_file_name(mobile)),
                "old-mobile.test",
            ),
        ] {
            std::fs::write(
                path,
                format!(
                    "{host}##+js(trusted-set, abdTrusted, json:42)\n\
                     {host}#%#//scriptlet('trusted-set-constant', 'adgTrusted', '42')\n"
                ),
            )
            .unwrap();
        }
        let dirs = [&lists as &Path, &rules as &Path];
        let built = Filters::load_cached(&dirs, &cache).unwrap();
        let kept = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
        for filters in [built, kept] {
            for host in [
                "custom.test",
                "user.test",
                "tracking.test",
                "old-mobile.test",
            ] {
                assert!(
                    filters.hiding(&format!("https://{host}/")).script.is_none(),
                    "{host}"
                );
            }
            for host in ["catalog.test", "mobile.test", "base.test", "chinese.test"] {
                let script = filters.hiding(&format!("https://{host}/")).script.unwrap();
                assert!(
                    script.contains(r#"trustedSetConstant("abdTrusted", "json:42")"#),
                    "{host}"
                );
                assert!(
                    script.contains(r#"trustedSetConstant("adgTrusted", "json:42")"#),
                    "{host}"
                );
            }
        }
    }

    #[test]
    fn a_moved_copy_retains_its_trust_and_a_replacement_with_the_same_rules_gets_a_new_cache_key() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(temp.path());
        let lists = paths.lists();
        std::fs::create_dir(&lists).unwrap();
        let (previous, current) = crate::list_url::MOVED[0];
        let old = lists.join(crate::list_url::file_name(previous));
        let new = lists.join(crate::list_url::file_name(current));
        let text = "||blocked.test^\n\
                    ||rewrite.test^$replace=/X/Y/\n\
                    page.test#%#//scriptlet('trusted-set-constant', 'adgTrusted', '42')\n";
        std::fs::write(&old, text).unwrap();
        let load = || Filters::load_cached(&[&lists], &paths.cache()).unwrap();
        let before = load();
        for filters in [before, load()] {
            assert!(filters.blocks_host("blocked.test"));
            assert!(filters.hiding("https://page.test/").script.is_none());
            assert!(filters.responses.is_none());
        }
        let old_generation = load().generation;

        // A reader can see both files while the updater removes the old one.
        std::fs::write(&new, text).unwrap();
        let published = load();
        assert_ne!(published.generation, old_generation);
        assert_eq!(published.rules, text.lines().count());
        assert!(published.hiding("https://page.test/").script.is_some());
        assert!(published.responses.is_some());
        std::fs::remove_file(old).unwrap();
        assert_eq!(load().generation, published.generation);
    }

    #[test]
    fn adguard_cosmetic_exceptions_and_negated_domains_match_in_the_engine() {
        let rules = "example.org,~skip.example.org#%#//scriptlet('set-constant', 'fixture', '42')\n\
                     example.org,~skip.example.org#$#.ad { display: none; }\n\
                     except.example.org#@%#//scriptlet('set-constant', 'fixture', '42')\n\
                     except.example.org#@$#.ad { display: none; }\n";
        let filters = filters(rules);
        let script = filters.hiding("https://example.org/").script.unwrap();
        assert!(script.contains(r#"setConstant("fixture", "42")"#));
        assert!(script.contains(r#"{"type":"style","arg":"display: none;"}"#));
        for host in ["skip.example.org", "except.example.org", "example.net"] {
            assert!(
                filters.hiding(&format!("https://{host}/")).script.is_none(),
                "{host}"
            );
        }
    }

    #[test]
    fn unsafe_scriptlet_arguments_leave_other_lists_scriptlets_active() {
        let dir = tempfile::tempdir().unwrap();
        let safe = "site.test##+js(set-constant, keptFixture, 42)\nsite.test##.ad\n";
        let unsafe_rules = "site.test##+js(set-constant, endTag, '</sCrIpT><script>arbitrary()</script>')\n\
            site.test##+js(set-constant, comment, '<!--')\n\
            site.test##+js(set-constant, nulByte, 'a\0b')\n\
            site.test##+js(set-constant, carriageReturn, 'a\rb')\n";
        std::fs::write(dir.path().join("safe.txt"), safe).unwrap();
        std::fs::write(dir.path().join("unsafe.txt"), unsafe_rules).unwrap();
        let found = Filters::load(&[dir.path()], true)
            .unwrap()
            .hiding("https://site.test/");
        let script = found.script.unwrap();
        assert!(script.contains(r#"setConstant("keptFixture", "42")"#));
        for skipped in [
            "endTag",
            "arbitrary()",
            "comment",
            "nulByte",
            "carriageReturn",
        ] {
            assert!(!script.contains(&format!("\"{skipped}\"")), "{skipped}");
        }
        assert!(!script.to_ascii_lowercase().contains("</script"));
        assert!(!script.contains("<!--"));
        assert!(found.css.is_some());

        let found = filters(unsafe_rules).hiding("https://site.test/");
        assert!(found.script.is_none());
    }

    #[test]
    fn helpers_for_a_rejected_call_do_not_consume_the_script_budget() {
        let url = "https://site.test/";
        let small = "site.test##+js(set-constant, keptFixture, 42";
        let base = filters(&format!("{small})")).hiding(url).script.unwrap();
        let padding = "x".repeat((256 << 10) - base.len());
        let valid = format!("{small}{padding})");
        let expected = filters(&valid).hiding(url).script.unwrap();
        assert_eq!(expected.len(), 256 << 10);
        let found = filters(&format!("{valid}\nsite.test##+js(prevent-fetch, '<!--')")).hiding(url);
        assert_eq!(found.script, Some(expected));
    }

    #[test]
    fn hides_on_a_page_what_the_lists_name_there() {
        let f = filters(HIDING);
        assert_eq!(
            f.hiding("https://www.site.example/news/1").css.unwrap(),
            ".ad{display:none!important}\n\
             a[href^=\"https://ads.example/\"]{display:none!important}\n\
             div[data-ad]{display:none!important}\n"
        );
        // Of the rules for every site, not the ones that go by a class:
        // those wait for the classes of the page to be known.
        assert_eq!(
            f.hiding("https://elsewhere.example/").css.unwrap(),
            "div[data-ad]{display:none!important}\n"
        );
        assert_eq!(f.hiding("https://excepted.example/").css, None);
        assert_eq!(f.hiding("not an address").css, None);
        assert!(!f.hiding("not an address").any());
        let none = filters("||ads.example^\n").hiding("https://site.example/");
        assert_eq!(none.css, None);
    }

    #[test]
    fn hides_by_class_and_id_what_a_page_is_found_to_have() {
        let f = filters(
            "##.by-class\n\
             ###by-id\n\
             ##.not-on-the-page\n\
             excepted.example#@#.by-class\n\
             @@||quiet.example^$generichide\n",
        );
        let names =
            |names: &[&str]| -> HashSet<String> { names.iter().map(|n| (*n).to_owned()).collect() };
        let (classes, ids) = (names(&["by-class", "content"]), names(&["by-id", "main"]));
        let by_name = |url: &str| {
            let hiding = f.hiding(url);
            f.hiding_by_name(&hiding, &classes, &ids)
        };
        assert_eq!(
            by_name("https://site.example/").unwrap(),
            "#by-id{display:none!important}\n.by-class{display:none!important}\n"
        );
        // A rule the lists take back for one site.
        assert_eq!(
            by_name("https://excepted.example/").unwrap(),
            "#by-id{display:none!important}\n"
        );
        // A site excepted from every such rule is not looked through.
        assert!(!f.hiding("https://quiet.example/").by_name());
        assert_eq!(by_name("https://quiet.example/"), None);
        // Nothing the page has is named.
        let hiding = f.hiding("https://site.example/");
        assert_eq!(
            f.hiding_by_name(&hiding, &names(&["content"]), &names(&[])),
            None
        );
    }

    #[test]
    fn what_is_hidden_is_kept_with_the_lists_in_the_cache() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(dir.path().join("list.txt"), HIDING).unwrap();
        let load = || Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        let (built, kept) = (load(), load());
        let url = "https://site.example/";
        assert!(built.hiding(url).css.is_some());
        assert_eq!(built.hiding(url).css, kept.hiding(url).css);
        let (classes, none) = (HashSet::from(["by-class".to_owned()]), HashSet::new());
        let by_name = |f: &Filters| f.hiding_by_name(&f.hiding(url), &classes, &none);
        assert!(by_name(&built).is_some());
        assert_eq!(by_name(&built), by_name(&kept));
    }

    #[test]
    fn element_hiding_exceptions_keep_scriptlets_after_cache_reload() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(
            dir.path().join("list.txt"),
            "site.test##+js(set-constant, adbFixture, 42)\nsite.test##.ad\n\
             site.test##div:has-text(Advert)\n##.generic\n@@||site.test^$ehide\n",
        )
        .unwrap();
        let built = Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        let kept = Filters::read_cache(cache.path(), built.generation.unwrap()).unwrap();
        for filters in [built, kept] {
            let hiding = filters.hiding("https://site.test/");
            assert!(hiding.css.is_none());
            assert!(!hiding.by_name());
            let script = hiding.script.unwrap();
            assert!(script.contains("adbFixture"));
            assert!(!script.contains("Advert"));
        }
    }

    #[test]
    fn keeps_only_rules_that_block_a_whole_host() {
        assert_eq!(
            host_only("||ads.example.com^").as_deref(),
            Some("||ads.example.com^")
        );
        assert_eq!(
            host_only("0.0.0.0 ads.example.com").as_deref(),
            Some("||ads.example.com^")
        );
        assert_eq!(host_only("||example.com^$third-party"), None);
        assert_eq!(host_only("||example.com/ads^"), None);
        assert_eq!(
            host_only("@@||example.com^").as_deref(),
            Some("@@||example.com^")
        );
        assert_eq!(
            host_only("||example.com^$badfilter").as_deref(),
            Some("||example.com^$badfilter")
        );
        assert_eq!(host_only("@@||example.com^$script"), None);
        assert_eq!(host_only("127.0.0.1 localhost"), None);
        assert_eq!(host_only("! a comment"), None);
        // Wider than anything anybody registered.
        assert_eq!(host_only("||com^"), None);
        assert_eq!(host_only("||co.uk^"), None);
        assert_eq!(host_only("0.0.0.0 uk"), None);
        assert_eq!(host_only("||^"), None);
        assert_eq!(
            host_only("||ads.example.co.uk^").as_deref(),
            Some("||ads.example.co.uk^")
        );
        assert_eq!(host_only("example.com##.ad"), None);
    }

    #[test]
    fn a_host_is_blocked_whole_only_by_a_rule_that_says_so() {
        let f = filters("||ads.example^\n||shop.example^$third-party\n||news.example/banner\n");
        assert!(f.blocks_host("ads.example"));
        assert!(f.blocks_host("cdn.ads.example"));
        assert!(!f.blocks_host("shop.example"));
        assert!(!f.blocks_host("news.example"));
        assert!(!f.blocks_host("notads.example"));
    }

    #[test]
    fn a_host_the_lists_take_back_is_not_blocked_whole() {
        let f = filters(
            "||ads.example^\n@@||ads.example^\n||cdn.example^\n||cdn.example^$badfilter\n||left.example^\n",
        );
        assert!(!f.blocks_host("ads.example"));
        assert!(!f.blocks_host("cdn.example"));
        assert!(f.blocks_host("left.example"));
    }

    #[test]
    fn the_host_lookup_matches_a_fully_parsed_request() {
        let f = filters(
            "||ads.example^\n\
             @@||safe.ads.example^\n\
             ||disabled.example^\n\
             ||disabled.example^$badfilter\n\
             0.0.0.0 static.example.co.uk\n\
             ||xn--bcher-kva.example^\n\
             ||shop.example^$third-party\n\
             ||news.example/banner\n",
        );
        let longest = [
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(61),
        ]
        .join(".");
        for (host, plain) in [
            ("ads.example", true),
            ("7.eu-west-1.ads.example", true),
            ("safe.ads.example", true),
            ("child.safe.ads.example", true),
            ("disabled.example", true),
            ("static.example.co.uk", true),
            ("shop.example", true),
            ("news.example", true),
            ("notads.example", true),
            (&format!("{}.ads.example", "a".repeat(63)), true),
            (&longest, true),
            (&format!("{longest}d"), false),
            (&format!("{}.ads.example", "a".repeat(64)), false),
            ("-x.ads.example", false),
            ("x-.ads.example", false),
            ("ads..example", false),
            ("ads.example.", false),
            ("ADS.example", false),
            ("local", false),
            ("", false),
            ("127.0.0.1", false),
            ("0x7f.0x1", false),
            ("ads.123", false),
            ("[2001:db8::1]", false),
            ("bücher.example", false),
            ("xn--bcher-kva.example", false),
            ("ads\u{3002}example", false),
            ("ad%73.example", false),
            ("x_y.ads.example", false),
            ("ads.example:8443", false),
            ("user@ads.example", false),
            ("ads.example/path", false),
            ("ads.example?query", false),
            ("ads.example#fragment", false),
            ("\tads.example", false),
        ] {
            assert_eq!(plain_dns_name(host), plain, "{host:?}");
            let url = format!("https://{host}/");
            let parsed = Request::new(&url, &url, "other", "GET");
            if plain {
                let request = parsed.as_ref().unwrap();
                assert_eq!(request.hostname, host);
                assert_eq!(request.url, url);
                assert!(!request.is_third_party);
            }
            let expected = parsed.is_ok_and(|request| {
                let found = f.hosts.check_network_request(&request);
                found.filter.is_some() && found.exception.is_none()
            });
            assert_eq!(f.blocks_host(host), expected, "{host:?}");
        }
    }

    #[test]
    fn web_exceptions_defer_full_host_checks_across_files_and_cached_generations() {
        let (lists, local, cache) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        std::fs::write(lists.path().join("list.txt"),
            "||ads.example^\n||media.example^\n||scoped.example^\n||unrelated.example^\n0.0.0.0 static.example\n"
        ).unwrap();
        std::fs::write(
            local.path().join("local.txt"),
            "@@||ads.example/allowed^$script,third-party,domain=reader.example\n\
             @@|https://media.example/play$media\n\
             @@/help^$to=scoped.example\n\
             @@||static.example/allowed^\n\
             @@||unrelated.example/test^$script,badfilter\n\
             @@*$generichide\n",
        )
        .unwrap();
        let dirs = [lists.path(), local.path()];
        let _ = Filters::load_cached(&dirs, cache.path()).unwrap();
        let list_files = ListFiles::read(&dirs).unwrap();
        for f in [
            Filters::load(&dirs, true).unwrap(),
            Filters::read_cache(cache.path(), list_files.generation).unwrap(),
        ] {
            for host in [
                "ads.example",
                "media.example",
                "scoped.example",
                "static.example",
            ] {
                assert!(!f.blocks_host(host), "{host}");
                assert!(f.blocks_domain(host), "{host}");
            }
            assert!(f.blocks_host("unrelated.example"));
            let page = "https://reader.example/";
            let check = |url, kind| f.check(url, page, kind, "GET").blocked;
            assert!(!check("https://ads.example/allowed/one.js", "script"));
            assert!(check("https://ads.example/allowed/one.png", "image"));
            assert!(check("https://ads.example/blocked.js", "script"));
            assert!(!check("https://media.example/play", "media"));
            assert!(!check("https://scoped.example/help", "other"));
            assert!(check("https://static.example/blocked.js", "script"));
            assert!(!check("https://static.example/allowed/one.js", "script"));
        }
    }

    #[test]
    fn rules_that_cannot_allow_an_https_request_do_not_defer_early_blocking() {
        let f = filters(
            "||ads.example^\n@@|http://$script\n@@|ws://$script\n\
                         @@*$generichide\n@@*$tag=inactive\n@@*$1p,3p\n",
        );
        assert!(f.blocks_host("ads.example"));
        assert!(
            f.check("https://ads.example/file.js", "", "script", "GET")
                .blocked
        );
        assert!(
            !f.check("http://ads.example/file.js", "", "script", "GET")
                .blocked
        );
    }

    #[test]
    fn an_exception_without_a_host_applies_only_when_the_request_is_read() {
        let f =
            filters("||ads.example^\n||pass.example^\n@@||pass.example^\n@@/downloads/$image\n");
        assert!(f.blocks_host("ads.example"));
        assert!(f.blocks_domain("ads.example"));
        assert!(!f.blocks_domain("pass.example"));
        assert!(
            !f.check("https://ads.example/downloads/a.png", "", "image", "GET")
                .blocked
        );
        assert!(
            f.check("https://ads.example/blocked/a.png", "", "image", "GET")
                .blocked
        );
    }

    #[test]
    fn a_disabled_exception_cannot_mask_a_block_by_host_or_request() {
        let f = filters(
            "||ads.example^\n@@||ads.example/allowed^$3p,script,domain=reader.example\n\
             @@||ads.example/allowed^$script,third-party,from=reader.example,badfilter\n",
        );
        assert!(f.blocks_host("ads.example"));
        for source in ["", "https://reader.example/"] {
            assert!(
                f.check("https://ads.example/allowed/a.js", source, "script", "GET")
                    .blocked
            );
        }
    }

    #[test]
    fn a_request_without_a_page_is_first_party_and_of_no_named_page() {
        let f = filters(
            "||third.example^$3p\n||first.example^$1p\n\
                         ||either.example^$3p\n||either.example^$1p\n\
                         ||context.example^$from=~reader.example\n\
                         ||ads.example^\n@@||ads.example/allowed^$~third-party,script\n\
                         @@||ads.example/reading^$domain=reader.example,script,method=post\n\
                         /promo^$to=neutral.example,image\n",
        );
        let check = |host_path: &str, page, kind, method| {
            f.check(&format!("https://{host_path}"), page, kind, method)
                .blocked
        };
        for no_page in ["", "not a source", "null", "about:blank"] {
            assert!(!check("third.example/ads", no_page, "script", "GET"));
            for url in [
                "first.example/ads",
                "either.example/ads",
                // Every page but one: no page is not that one.
                "context.example/ads",
            ] {
                assert!(check(url, no_page, "script", "GET"), "{url}");
            }
            assert!(check("ads.example/blocked", no_page, "script", "GET"));
            assert!(!check("ads.example/allowed/a.js", no_page, "script", "GET"));
            // An exception for a page is for no request without one.
            for method in ["POST", "GET"] {
                assert!(check("ads.example/reading/a.js", no_page, "script", method));
            }
            assert!(check("ads.example/allowed/a.png", no_page, "image", "GET"));
            assert!(check(
                "neutral.example/promo/a.png",
                no_page,
                "image",
                "GET"
            ));
            assert!(!check("other.example/promo/a.png", no_page, "image", "GET"));
        }
        assert!(check(
            "third.example/ads",
            "https://reader.example/",
            "script",
            "GET"
        ));
        assert!(!check(
            "third.example/ads",
            "https://third.example/",
            "script",
            "GET"
        ));
        assert!(check(
            "first.example/ads",
            "https://first.example/",
            "script",
            "GET"
        ));
        assert!(check(
            "ads.example/allowed/a.js",
            "https://reader.example/",
            "script",
            "GET"
        ));
        assert!(!check(
            "ads.example/reading/a.js",
            "https://reader.example/",
            "script",
            "POST"
        ));
        assert!(check(
            "ads.example/reading/a.js",
            "https://other.example/",
            "script",
            "POST"
        ));
    }

    #[test]
    fn exceptions_for_a_page_keep_their_destinations_and_need_the_page() {
        let f = filters(
            "||ads.example.com^\n||ads.example.net^\n\
             @@*$from=reader.example,to=com,image\n\
             @@*$from=reader.example,to=~ads.example.com,script\n",
        );
        let check = |host, source, kind| {
            f.check(&format!("https://{host}/resource"), source, kind, "GET")
                .blocked
        };
        let source = "https://reader.example/";
        assert!(!check("ads.example.com", source, "image"));
        assert!(check("ads.example.net", source, "image"));
        assert!(check("ads.example.com", source, "script"));
        assert!(!check("ads.example.net", source, "script"));
        for (host, kind) in [
            ("ads.example.com", "image"),
            ("ads.example.net", "image"),
            ("ads.example.com", "script"),
            ("ads.example.net", "script"),
        ] {
            assert!(check(host, "", kind), "{host} {kind}");
        }
        assert!(check(
            "ads.example.com",
            "https://elsewhere.example/",
            "image"
        ));
        assert!(check(
            "ads.example.net",
            "https://elsewhere.example/",
            "script"
        ));
    }

    #[test]
    fn a_request_is_judged_by_its_address_its_page_and_its_kind() {
        let f = filters("/banner/*$image\n||tracker.example^$third-party\n");
        let page = "https://news.example/story";
        assert!(
            f.check("https://news.example/banner/1.png", page, "image", "GET")
                .blocked
        );
        assert!(
            !f.check("https://news.example/banner/1.js", page, "script", "GET")
                .blocked
        );
        assert!(
            f.check("https://tracker.example/t.js", page, "script", "GET")
                .blocked
        );
        // The tracker's own site, visited.
        let own = "https://tracker.example/";
        assert!(
            !f.check("https://tracker.example/t.js", own, "script", "GET")
                .blocked
        );
    }

    #[test]
    fn an_exception_lets_through_what_a_rule_blocks() {
        let f = filters("||ads.example^\n@@||ads.example/allowed^\n");
        let page = "https://news.example/";
        assert!(
            f.check("https://ads.example/x.js", page, "script", "GET")
                .blocked
        );
        assert!(
            !f.check("https://ads.example/allowed/x.js", page, "script", "GET")
                .blocked
        );
    }

    #[test]
    fn names_the_rule_that_matched_when_asked_to_keep_them() {
        let f = filters("||ads.example^\n");
        let check = f.check(
            "https://ads.example/x",
            "https://a.example/",
            "other",
            "GET",
        );
        assert_eq!(check.rule.as_deref(), Some("||ads.example^"));
    }

    #[test]
    fn lists_are_read_from_every_directory_and_only_from_txt_files() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(a.path().join("one.txt"), "||one.example^\n").unwrap();
        std::fs::write(b.path().join("two.txt"), "||two.example^\n").unwrap();
        std::fs::write(b.path().join("two.txt.new"), "||half.example^\n").unwrap();
        let missing = b.path().join("missing");
        let f = Filters::load(&[a.path(), b.path(), &missing], false).unwrap();
        assert!(f.blocks_host("one.example") && f.blocks_host("two.example"));
        assert!(!f.blocks_host("half.example"));
        assert_eq!((f.rules, f.host_rules), (2, 2));
    }

    #[test]
    fn an_incomplete_startup_load_is_used_without_caching_it() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let missing = dir.path().join("later.txt");
        std::os::unix::fs::symlink(dir.path().join("absent"), &missing).unwrap();
        std::fs::write(dir.path().join("ok.txt"), "||one.example^").unwrap();
        let filters = Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        assert!(filters.blocks_host("one.example"));
        assert!(filters.generation.is_none());
        assert!(!cache.path().join("key").exists());
        std::fs::remove_file(missing).unwrap();
        assert!(
            Filters::load_cached(&[dir.path()], cache.path())
                .unwrap()
                .generation
                .is_some()
        );
    }

    #[test]
    fn a_file_that_is_no_list_is_left_out_and_the_rest_still_filter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "||one.example^\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let f = Filters::load(&[dir.path()], false).unwrap();
        assert!(f.blocks_host("one.example"));
        assert_eq!(f.rules, 1);
    }

    #[test]
    fn a_load_that_left_out_a_list_not_in_utf8_is_not_cached() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(dir.path().join("a.txt"), "||one.example^\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let filters = Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        assert!(filters.blocks_host("one.example"));
        assert!(filters.generation.is_none());
        assert!(!cache.path().join("key").exists());
    }

    #[test]
    fn lists_loaded_before_are_taken_from_the_cache_until_one_changes() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let list = dir.path().join("list.txt");
        std::fs::write(&list, "||one.example^\n/banner/*$image\n").unwrap();
        let load = || Filters::load_cached(&[dir.path()], cache.path()).unwrap();

        let first = load();
        assert!(cache.path().join("key").exists());
        // The lists gone from under the cache: only it can answer now.
        let key = ListFiles::read(&[dir.path()]).unwrap().generation;
        let kept = Filters::read_cache(cache.path(), key).unwrap();
        assert!(kept.blocks_host("one.example"));
        let page = "https://news.example/";
        assert!(
            kept.check("https://a.example/banner/1.png", page, "image", "GET")
                .blocked
        );
        assert_eq!(
            (kept.rules, kept.host_rules),
            (first.rules, first.host_rules)
        );

        // A list changed: what was kept is no longer for these lists.
        std::fs::write(&list, "||two.example^\n").unwrap();
        let second = load();
        assert!(second.blocks_host("two.example") && !second.blocks_host("one.example"));
    }

    /// Not a test: prints what reading the lists costs, from the files
    /// and from the cache. `ABD_LISTS=<dir> cargo test --release -- --ignored --nocapture measures`
    #[test]
    #[ignore = "a measurement, run by hand"]
    fn measures_loading_the_lists() {
        let Some(dir) = std::env::var_os("ABD_LISTS") else {
            return;
        };
        let rss = || {
            std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|status| {
                    status
                        .lines()
                        .find_map(|line| line.strip_prefix("VmRSS:").map(|v| v.trim().to_owned()))
                })
        };
        // A supplied cache allows each variant to be loaded in its own
        // process, without counting memory retained by the allocator
        // after an earlier load.
        if let Some(cache) = std::env::var_os("ABD_LIST_CACHE") {
            let started = std::time::Instant::now();
            let f = Filters::load_cached(&[Path::new(&dir)], Path::new(&cache)).unwrap();
            println!(
                "{} lines: {:?}, VmRSS={:?}",
                f.rules,
                started.elapsed(),
                rss()
            );
            return;
        }
        let cache = tempfile::tempdir().unwrap();
        let load = || {
            let started = std::time::Instant::now();
            let f = Filters::load_cached(&[Path::new(&dir)], cache.path()).unwrap();
            (started.elapsed(), f.rules, rss())
        };
        let (from_lists, rules, lists_rss) = load();
        let (from_cache, _, cache_rss) = load();
        println!(
            "{rules} lines: {from_lists:?} from the lists (VmRSS={lists_rss:?}), \
             {from_cache:?} from the cache (VmRSS={cache_rss:?})"
        );
    }
}
