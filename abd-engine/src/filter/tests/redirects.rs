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
use base64::{Engine as _, prelude::BASE64_STANDARD};

const TARGET: &str = "https://ads.example/ad.js";
const SCRIPT: &[u8] = include_bytes!("../../../resources/ublock/web_accessible_resources/noop.js");
static GIF: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
    BASE64_STANDARD
        .decode(
            include_str!("../../../resources/ublock/web_accessible_resources/1x1.gif.base64")
                .replace('\n', ""),
        )
        .unwrap()
});

fn assert_redirect(check: Check, content_type: &str, body: &[u8]) {
    assert!(check.blocked);
    let redirect = check.redirect.expect("request has a substitute");
    assert_eq!(redirect.content_type, content_type);
    assert_eq!(redirect.body.as_ref(), body);
}

#[test]
fn every_bundled_mime_type_round_trips_from_the_matcher_to_http() {
    use adblock::resources::Resource;
    use std::collections::BTreeSet;

    let bundle: Vec<Resource> =
        serde_json::from_str(include_str!("../../../resources/redirects.json")).unwrap();
    assert_eq!(bundle.len(), 43);
    let mut mime_types = BTreeSet::new();
    for resource in bundle {
        let adblock::resources::ResourceType::Mime(mime) = resource.kind else {
            panic!("redirect resource needs a MIME type");
        };
        let mime = mime.to_string();
        mime_types.insert(mime.clone());
        let expected = BASE64_STANDARD.decode(resource.content).unwrap();
        let name = resource.name;
        for name in std::iter::once(name).chain(resource.aliases) {
            let f = filters(&format!("||ads.example/ad.js$script,redirect={name}"));
            assert_redirect(f.check(TARGET, "", "script", "GET"), &mime, &expected);
        }
    }
    assert_eq!(
        mime_types.into_iter().collect::<Vec<_>>(),
        [
            "application/javascript",
            "application/json",
            "audio/mp3",
            "image/gif",
            "image/png",
            "text/css",
            "text/html",
            "text/plain",
            "text/xml",
            "video/mp4",
        ]
    );
}

#[test]
fn the_generator_requires_each_adguard_mapping_and_the_bundle_resolves_it() {
    let generator = include_str!("../../../resources/generate.mjs");
    let table = generator
        .split_once("const adguardRedirects = new Map(Object.entries({\n")
        .unwrap()
        .1
        .split_once("}));")
        .unwrap()
        .0;
    let resources: Vec<adblock::resources::Resource> =
        serde_json::from_str(include_str!("../../../resources/redirects.json")).unwrap();
    for line in table.lines() {
        let (alias, name) = line.trim().trim_end_matches(',').split_once(": ").unwrap();
        let alias = alias.trim_matches('\'');
        let name = name.trim_matches('\'');
        let resource = resources.iter().find(|r| r.name == name).unwrap();
        assert!(
            alias == name || resource.aliases.iter().any(|n| n == alias),
            "{alias}: {name}"
        );
    }
    assert_eq!(table.lines().count(), 32);
}

#[test]
fn a_redirect_rule_only_supplies_a_resource_when_a_block_stands() {
    let redirect = "||ads.example/ad.js$script,redirect-rule=noopjs\n";
    let block = "||ads.example/ad.js$script\n";
    let allow = "@@||ads.example/ad.js$script\n";
    for rules in [redirect.to_owned(), format!("{block}{redirect}{allow}")] {
        let check = filters(&rules).check(TARGET, "", "script", "GET");
        assert!(!check.blocked);
        assert!(check.redirect.is_none());
    }
    assert_redirect(
        filters(&format!("{block}{redirect}")).check(TARGET, "", "script", "GET"),
        "application/javascript",
        SCRIPT,
    );
}

#[test]
fn both_valueless_exceptions_suppress_both_kinds_of_redirects() {
    let rules = "||ads.example/ad.js$script,redirect=noopjs\n\
                 ||ads.example/ad.js$script,redirect-rule=1x1.gif:9\n";
    for option in ["redirect", "redirect-rule"] {
        let exception = format!("@@||ads.example/ad.js${option},script,domain=page.test");
        let f = filters(&format!("{rules}{exception}"));
        let spared = f.check(TARGET, "https://page.test/", "script", "GET");
        assert!(spared.blocked && spared.redirect.is_none(), "{exception}");
        assert_redirect(
            f.check(TARGET, "https://other.test/", "script", "GET"),
            "image/gif",
            &GIF,
        );
    }
}

#[test]
fn redirect_priority_and_exceptions_follow_the_matcher() {
    let choices = "||ads.example/ad.js$script,redirect=noopjs:1\n\
                   ||ads.example/ad.js$script,redirect=1x1.gif:9\n";
    let check = |rules: &str| filters(rules).check(TARGET, "", "script", "GET");
    assert_redirect(check(choices), "image/gif", &GIF);

    let important = "||ads.example/ad.js$script,important\n";
    let named = "@@||ads.example/ad.js$script,redirect=1x1.gif:9\n";
    let bare = "@@||ads.example/ad.js$script,redirect\n";
    let allow = "@@||ads.example/ad.js$script\n";
    assert_redirect(
        check(&format!("{choices}{important}{named}")),
        "application/javascript",
        SCRIPT,
    );
    assert!(!check(&format!("{choices}{named}")).blocked);
    assert_redirect(
        check(&format!("{choices}{important}{allow}")),
        "image/gif",
        &GIF,
    );
    for rules in [
        format!("{choices}{bare}"),
        format!("{choices}{important}{bare}"),
        "||ads.example/ad.js$script,redirect=noopjs\n\
         ||ads.example/ad.js$script,redirect=ati-smarttag:9\n"
            .to_owned(),
    ] {
        let result = check(&rules);
        assert!(result.blocked);
        assert!(result.redirect.is_none(), "{rules}");
    }
    assert_redirect(
        check(&format!(
            "{choices}{bare}@@||ads.example/ad.js$script,redirect,badfilter"
        )),
        "image/gif",
        &GIF,
    );
}

#[test]
fn legacy_empty_and_mp4_supply_the_same_resources_as_their_redirect_forms() {
    let mp4 = BASE64_STANDARD
        .decode(
            include_str!("../../../resources/ublock/web_accessible_resources/noop-1s.mp4.base64")
                .replace('\n', ""),
        )
        .unwrap();
    for (legacy, alias, mime, body) in [
        (
            "empty",
            "nooptext",
            "text/plain",
            include_bytes!("../../../resources/ublock/web_accessible_resources/noop.txt")
                .as_slice(),
        ),
        ("mp4", "noopmp4-1s", "video/mp4", mp4.as_slice()),
    ] {
        for option in [legacy.to_owned(), format!("redirect={alias}")] {
            let f = filters(&format!("||ads.example/ad.js$script,{option}"));
            assert_redirect(f.check(TARGET, "", "script", "GET"), mime, body);
        }
    }
}

#[test]
fn redirect_rules_merge_app_and_page_conditions_and_priorities() {
    let f = filters(
        "||ads.example/ad.js$script,redirect=noopjs:2,domain=page.test\n\
         ||ads.example/ad.js$script,redirect-rule=1x1.gif:5,app=com.reader*\n\
         ||ads.example/ad.js$script,redirect-rule=empty:1,app=~com.reader*\n\
         @@||ads.example/ad.js$script,redirect-rule,app=com.disabled\n\
         ||ads.example/own$redirect=noopjs,app=com.reader*\n",
    );
    let page = parse_http_page("https://page.test/").unwrap();
    for (app, mime, body) in [
        ("com.reader.news", "image/gif", GIF.as_slice()),
        ("com.other", "application/javascript", SCRIPT),
    ] {
        let v = f
            .for_app(app)
            .check_with_page(TARGET, Page::Named(&page), "script", "GET");
        assert_redirect(v, mime, body);
    }
    let no_page = f
        .for_app("com.reader.news")
        .check_with_page(TARGET, Page::NONE, "script", "GET");
    assert!(!no_page.blocked);
    assert!(no_page.redirect.is_none());
    let disabled =
        f.for_app("com.disabled")
            .check_with_page(TARGET, Page::Named(&page), "script", "GET");
    assert!(disabled.blocked && disabled.redirect.is_none());
    for app in ["com.reader.news", "com.other"] {
        let check =
            f.for_app(app)
                .check_with_page("https://ads.example/own", Page::NONE, "script", "GET");
        if app == "com.reader.news" {
            assert_redirect(check, "application/javascript", SCRIPT);
        } else {
            assert!(!check.blocked && check.redirect.is_none());
        }
    }
}

#[test]
fn redirect_rules_do_not_block_a_whole_host() {
    for option in [
        "redirect=noopjs",
        "redirect-rule=noopjs",
        "empty",
        "mp4",
        "redirect=noopjs,app=com.reader",
    ] {
        let f = filters(&format!("||ads.example^${option}"));
        let app = f.for_app("com.reader");
        assert!(!app.blocks_host("ads.example"), "{option}");
        assert!(!app.blocks_domain("ads.example"), "{option}");
        assert!(!app.blocks_unread("ads.example"), "{option}");
    }
}

#[test]
fn a_redirect_resource_survives_a_cache_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    std::fs::write(
        dir.path().join("list.txt"),
        "||ads.example/ad.js$script,redirect=noopjs\n\
         ||ads.example/ad.js$script,redirect-rule=1x1.gif:1,app=com.reader\n",
    )
    .unwrap();
    let built = Filters::load_cached(&[dir.path()], &cache).unwrap();
    let kept = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
    for filters in [built, kept] {
        assert_redirect(
            filters.check(TARGET, "", "script", "GET"),
            "application/javascript",
            SCRIPT,
        );
        assert_redirect(
            filters
                .for_app("com.reader")
                .check_with_page(TARGET, Page::NONE, "script", "GET"),
            "image/gif",
            &GIF,
        );
    }
}
