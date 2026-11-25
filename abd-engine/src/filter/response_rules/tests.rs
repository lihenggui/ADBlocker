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

//! Rules are checked against the same matcher that uses them at runtime.

use std::path::Path;

use super::*;
use crate::filter::{Filters, parse_http_page};
use crate::list_url::{file_name, untrusted_file_name};
use crate::protocol_types::IssueKind;
use crate::rule::action::value_end;
use crate::rule::options::{
    option_separator, option_texts as options, option_value_end, regex_option_separator,
};

use crate::rule::quoting::delimiter;

impl ResponseRules {
    fn build_with_budget<'a>(
        sources: impl Iterator<Item = &'a str>,
        budget: usize,
        patterns: &mut Patterns,
    ) -> Option<Self> {
        let mut kept = ResponseProjections::default();
        for line in sources.flat_map(str::lines) {
            let source = rule::read(line);
            if let rule::Kind::Response(response) = source.kind {
                kept.add(source.line, response);
            }
        }
        kept.compile_with_budget(budget, patterns)
    }
}

fn split(line: &str) -> Option<(&str, &str)> {
    match rule::read(line).kind {
        rule::Kind::Response(response) => Some(response.parts()),
        _ => None,
    }
}

fn check(line: &str, patterns: &mut Patterns) -> Option<bool> {
    match rule::read(line).kind {
        rule::Kind::Response(response) => Some(valid(&response, patterns)),
        _ => None,
    }
}

fn action(value: &str) -> Option<ActionSpec> {
    ActionKind::Replace.parse(value)
}

impl<'a> Parsed<'a> {
    fn parse(pattern: &'a str, suffix: &'a str) -> Option<Self> {
        Self::from_rule(&ResponseRule::from_parts(pattern, suffix)?)
    }
}

#[test]
fn a_regex_condition_consumes_its_dollar_signs_before_the_options() {
    for condition in [r"/https?:\/\/site\.test\/x$/", r"@@/^foo\$bar$/"] {
        let line = format!("{condition}$replace=/a/b/");
        assert_eq!(regex_option_separator(&line), Some(condition.len()));
    }
    assert_eq!(regex_option_separator("/relative/path$replace=/a/b/"), None);
}

#[test]
fn the_option_separator_ignores_quoted_dollars_and_dollars_inside_values() {
    let condition = r"||site.test/path\$item";
    for suffix in [
        "replace=/x$/y/,app=com.app",
        "app=com.app,replace=/x/$$/",
        "app=com.app,replace=/x/$not-closed",
    ] {
        assert_eq!(
            option_separator(&format!("{condition}${suffix}")),
            Some(condition.len())
        );
    }
    assert_eq!(
        option_separator(r"site.test$token$replace=/a/b/"),
        Some("site.test$token".len())
    );
    assert_eq!(option_separator("site.test/файл"), None);
}

#[test]
fn the_value_scan_stops_after_its_fields_or_at_an_option_comma() {
    let fields = r"x\/y\,z$/value\//";
    assert_eq!(
        option_value_end(&format!("{fields}is,script")),
        fields.len()
    );
    assert_eq!(option_value_end("x/value,script"), "x/value".len());
    assert_eq!(option_value_end("x/$unfinished"), "x/$unfinished".len());
    assert_eq!(delimiter(r"a\//b"), Some(3));
    assert_eq!(delimiter(r"a\\/b"), Some(3));
    assert_eq!(value_end(fields), Some(fields.len()));
}

#[test]
fn options_split_around_the_replacement_and_after_the_url_pattern() {
    let value = r"replace=/foo\,bar|baz\/\$/\$1\,\//is";
    for options_text in [
        format!("{value},xhr,app=com.example"),
        format!("xhr,{value},app=com.example"),
        format!("xhr,app=com.example,{value}"),
    ] {
        let condition = r"/https?:\/\/example\.org\/path$/";
        let line = format!("{condition}${options_text}");
        assert_eq!(split(&line), Some((condition, options_text.as_str())));
        assert_eq!(options(&options_text).count(), 3);
        assert_eq!(check(&line, &mut Patterns::default()), Some(true), "{line}");
    }
    assert_eq!(
        split(r"||example.org/path\$name$replace=/X/Y/"),
        Some((r"||example.org/path\$name", "replace=/X/Y/"))
    );
    assert!(split(r"/replace=foo$/").is_none());
    assert!(split("||example.org^$xhr").is_none());
}

#[test]
fn rules_from_subscriptions_keep_their_conditions_and_actions() {
    let examples = [
        (
            r#"||www.youtube.com/watch?$xhr,1p,replace=/"adPlacements"/"no_ads"/"#,
            "||www.youtube.com/watch?$1p,csp=0",
            Some("xhr"),
            None,
            r#""adPlacements""#,
            r#""no_ads""#,
        ),
        (
            r"||www.msn.com^$replace=/\.feed-ads-wrapper\{display:flex;/\.feed-ads-wrapper\{display:none!important;/",
            "||www.msn.com^$csp=0",
            None,
            None,
            r"\.feed-ads-wrapper\{display:flex;",
            r"\.feed-ads-wrapper\{display:none!important;",
        ),
        (
            r"||a.a-p-i.io/udnnews^$replace=/(photocp4s|adNews)/__\$1/",
            "||a.a-p-i.io/udnnews^$csp=0",
            None,
            None,
            "(photocp4s|adNews)",
            "__$1",
        ),
        (
            r#"||football.ua/hnd/*.ashx$replace=/<item\stype="banner">.*?(<\/item>)//"#,
            "||football.ua/hnd/*.ashx$csp=0",
            None,
            None,
            r#"<item\stype="banner">.*?(<\/item>)"#,
            "",
        ),
        (
            r#"||api.ebay-kleinanzeigen.de/api/ads.json*^q=$app=com.ebay.kleinanzeigen,replace=/"(adsense|ecn|native-ad)"/"x\$1"/"#,
            "||api.ebay-kleinanzeigen.de/api/ads.json*^q=$csp=0",
            None,
            Some("com.ebay.kleinanzeigen"),
            r#""(adsense|ecn|native-ad)""#,
            r#""x$1""#,
        ),
    ];
    for (line, condition, kind, apps, pattern, replacement) in examples {
        let (host, opts) = split(line).unwrap();
        let parsed = Parsed::parse(host, opts).unwrap();
        assert_eq!(parsed.project(0), condition);
        assert_eq!(
            parsed.qualifiers.include,
            kind.and_then(kind_bit).unwrap_or(0)
        );
        assert_eq!(parsed.qualifiers.exclude, 0);
        assert_eq!(parsed.apps.as_deref(), apps);
        assert!(
            parsed.action
                == Some(ActionSpec::Replace {
                    pattern: pattern.to_owned(),
                    replacement: replacement.to_owned(),
                    insensitive: false,
                    single_line: false,
                })
        );
        assert_eq!(check(line, &mut Patterns::default()), Some(true), "{line}");
    }
    let flags = action(r"/(foo\,bar\$)/\$1\//is").unwrap();
    assert!(
        flags
            == ActionSpec::Replace {
                pattern: "(foo,bar$)".to_owned(),
                replacement: "$1/".to_owned(),
                insensitive: true,
                single_line: true,
            }
    );
}

#[test]
fn invalid_response_rules_are_reported_by_line() {
    let rules = [
        r"||example.org^$replace=/X/Y/g",
        r"||example.org^$replace=/X/Y",
        r"||example.org^$replace=/(?=X)/Y/",
        r"||example.org^$replace=/(X)\1/Y/",
        r"||example.org^$replace=/\w{100000}/Y/",
        r"||example.org^$replace=/X/$1/",
        r"||example.org^$replace=/X/Y/,unsupported",
        r"||example.org^$replace",
    ]
    .join("\n");
    let checked = crate::rulecheck::check(&format!(
        "{rules}\n@@||example.org^$replace\n||example.org^$replace=/x/y/"
    ));
    assert_eq!(checked.issues.len(), 8);
    assert!(
        checked
            .issues
            .iter()
            .enumerate()
            .all(|(i, issue)| issue.line == i + 1 && issue.kind == IssueKind::Invalid)
    );
}

fn filters(text: &str) -> Filters {
    let temp = tempfile::tempdir().unwrap();
    let rules = temp.path().join("rules");
    std::fs::create_dir(&rules).unwrap();
    std::fs::write(rules.join("user.txt"), text).unwrap();
    Filters::load(&[&rules], false).unwrap()
}

fn changed(f: &Filters, url: &str, page: Page<'_>, kind: &str, app: &str) -> String {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut bytes = b"X Z".to_vec();
    for action in f.for_app(app).response_actions(url, page, kind, "GET") {
        bytes = action.apply(&bytes, &|| Instant::now() < deadline).unwrap();
    }
    String::from_utf8(bytes).unwrap()
}

#[test]
fn response_rules_ignore_noops_at_every_option_position() {
    let url = "https://example.org/body";
    for (replace, prune) in [
        ("_,replace=/X/Y/", r"jsonprune=\$.ad,_"),
        ("replace=/X/Y/,__,___", r"__,jsonprune=\$.ad,___"),
        (
            "_,replace=/X/Y/,__,xmlhttprequest,___",
            r"_,xmlhttprequest,___,jsonprune=\$.ad,_",
        ),
    ] {
        let rules = format!("||example.org/body${replace}\n||example.org/body${prune}");
        let f = filters(&rules);
        assert!(crate::rulecheck::check(&rules).issues.is_empty());
        assert_eq!(
            changed(&f, url, Page::NONE, "xmlhttprequest", "com.reader"),
            "Y Z"
        );
        let mut data = br#"{"ad":1,"text":"X"}"#.to_vec();
        for action in
            f.for_app("com.reader")
                .response_actions(url, Page::NONE, "xmlhttprequest", "GET")
        {
            data = action.apply(&data, &|| true).unwrap();
        }
        assert_eq!(data, br#"{"text":"Y"}"#);
    }
}

#[test]
fn response_rules_match_the_real_kind_page_party_and_app() {
    let f = filters(
        r"||example.org/qualified$replace=/X/Y/,xhr,~image,app=Com.Read*,domain=page.org,3p,method=GET",
    );
    let url = "https://example.org/qualified";
    let page = parse_http_page("https://page.org/").unwrap();
    for (url, page, kind, app, expected) in [
        (
            url,
            Page::Named(&page),
            "xmlhttprequest",
            "42:com.reader",
            "Y Z",
        ),
        (url, Page::Named(&page), "image", "com.reader", "X Z"),
        (
            url,
            Page::Named(&page),
            "xmlhttprequest",
            "com.other",
            "X Z",
        ),
        (
            url,
            Page::Named(&page),
            "xmlhttprequest",
            "@com.reader",
            "X Z",
        ),
        (url, Page::NONE, "xmlhttprequest", "com.reader", "X Z"),
        (
            "https://example.org/other",
            Page::Named(&page),
            "xmlhttprequest",
            "com.reader",
            "X Z",
        ),
    ] {
        assert_eq!(changed(&f, url, page, kind, app), expected);
    }
    assert!(
        f.for_app("com.reader")
            .response_actions(url, Page::Named(&page), "xhr", "POST")
            .is_empty()
    );
    assert!(!f.check(url, "https://page.org", "xhr", "GET").blocked);
}

#[test]
fn replacement_exceptions_do_not_allow_blocked_requests() {
    let f = filters("||example.org/path\n@@||example.org/path$replace\n");
    assert!(
        f.check("https://example.org/path", "", "other", "GET")
            .blocked
    );
}

#[test]
fn matching_response_rules_are_applied_in_alphabetical_order() {
    let f = filters("||example.org^$replace=/Y/Q/\n||example.org^$replace=/X/Y/\n");
    assert_eq!(
        changed(
            &f,
            "https://example.org/",
            Page::NONE,
            "other",
            "com.reader"
        ),
        "Q Z"
    );
}

#[test]
fn response_exceptions_remove_only_matching_actions_after_qualifiers() {
    let f = filters(
        "||example.org^$replace=/X/Y/\n\
        ||example.org^$replace=/Z/Y/\n\
        @@||example.org/page/*$replace=/Z/Y/\n\
        @@||example.org/all/*$app=com.reader,script,replace\n\
        @@||example.org/all/*$xhr,replace=/Z/Y/\n",
    );
    for (path, kind, app, expected) in [
        ("/", "other", "com.reader", "Y Y"),
        ("/page/story", "other", "com.reader", "Y Z"),
        ("/all/item", "script", "com.reader", "X Z"),
        ("/all/item", "script", "com.other", "Y Y"),
        ("/all/item", "xhr", "com.reader", "Y Z"),
    ] {
        assert_eq!(
            changed(
                &f,
                &format!("https://example.org{path}"),
                Page::NONE,
                kind,
                app
            ),
            expected
        );
    }
}

#[test]
fn page_exceptions_disable_rewriting_on_the_page_and_its_requests() {
    let f = filters("*$replace=/X/Y/\n@@||document.org^$document\n@@||content.org^$content\n");
    for host in ["document.org", "content.org"] {
        let url = format!("https://{host}/page");
        let page = parse_http_page(&url).unwrap();
        assert_eq!(
            changed(&f, &url, Page::Itself, "document", "com.reader"),
            "X Z"
        );
        assert_eq!(
            changed(
                &f,
                "https://example.org/",
                Page::Named(&page),
                "script",
                "com.reader"
            ),
            "X Z"
        );
        assert_eq!(changed(&f, &url, Page::NONE, "script", "com.reader"), "Y Z");
    }
}

#[test]
fn only_trusted_response_rules_are_kept_and_a_new_generation_removes_them() {
    let temp = tempfile::tempdir().unwrap();
    let dirs = ["lists", "rules"].map(|name| temp.path().join(name));
    for dir in &dirs {
        std::fs::create_dir(dir).unwrap();
    }
    let catalog = "https://filters.adtidy.org/android/filters/11.txt";
    let user = dirs[1].join("user.txt");
    let trusted = dirs[0].join(file_name(catalog));
    let cache = temp.path().join("cache");
    for (file, host) in [
        (dirs[0].join("custom.txt"), "custom.org"),
        (
            dirs[0].join(untrusted_file_name(catalog).unwrap()),
            "untrusted-name.org",
        ),
        (trusted.clone(), "stock.org"),
        (user.clone(), "user.org"),
    ] {
        std::fs::write(
            file,
            format!("||{host}^$replace=/X/Y/\n@@||{host}/off$replace\n@@||page.org^$content"),
        )
        .unwrap();
    }
    let dirs = dirs.each_ref().map(Path::new);
    let built = Filters::load_cached(&dirs, &cache).unwrap();
    let cached = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
    for f in [built, cached] {
        let page = parse_http_page("https://page.org/").unwrap();
        for (host, expected) in [
            ("custom.org", "X Z"),
            ("untrusted-name.org", "X Z"),
            ("stock.org", "Y Z"),
            ("user.org", "Y Z"),
        ] {
            let url = format!("https://{host}/");
            assert_eq!(
                changed(&f, &url, Page::NONE, "other", "com.reader"),
                expected
            );
            assert_eq!(
                changed(&f, &url, Page::Named(&page), "other", "com.reader"),
                "X Z"
            );
            assert_eq!(
                changed(&f, &format!("{url}off"), Page::NONE, "other", "com.reader"),
                "X Z"
            );
        }
    }
    std::fs::remove_file(trusted).unwrap();
    std::fs::remove_file(user).unwrap();
    for path in ["custom.txt", &untrusted_file_name(catalog).unwrap()] {
        std::fs::write(dirs[0].join(path), "||ads.org^\n").unwrap();
    }
    let rebuilt = Filters::load_cached(&dirs, &cache).unwrap();
    assert!(rebuilt.responses.is_none());
    for file in ["response-actions", "response-rules", "no-content"] {
        assert!(!cache.join(file).exists(), "{file}");
    }
}

#[test]
fn the_rule_budget_keeps_the_same_alphabetical_prefix() {
    let mut text = String::new();
    for i in (0..=MAX_RESPONSE_RULES).rev() {
        text.push_str(&format!("||example.org/{i:04}|$replace=/X/Y/\n"));
    }
    let f = filters(&text);
    assert_eq!(
        changed(
            &f,
            "https://example.org/0000",
            Page::NONE,
            "other",
            "com.reader"
        ),
        "Y Z"
    );
    let url = format!("https://example.org/{MAX_RESPONSE_RULES:04}");
    assert_eq!(changed(&f, &url, Page::NONE, "other", "com.reader"), "X Z");
}

#[test]
fn the_regex_budget_keeps_actions_in_rule_order_and_charges_each_once() {
    let first = r"||example.org/a$replace=/(X+)/Y/";
    let second = r"||example.org/b$replace=/(Z+)/W/";
    let also_first = r"||example.org/c$replace=/(X+)/Y/";
    let first_spec = action("/(X+)/Y/").unwrap();
    let budget = first_spec.compile().unwrap().regex_bytes();
    assert!(budget > 0);
    for lines in [[first, second, also_first], [also_first, second, first]] {
        let f =
            ResponseRules::build_with_budget(lines.into_iter(), budget, &mut Patterns::default())
                .unwrap();
        assert!(f.table.actions == [first_spec.clone()]);
        assert_eq!(f.table.rules.len(), 2);
    }
}

#[test]
fn blocks_stand_only_when_the_request_has_no_applicable_response_action() {
    use crate::mitm::evaluate_request;
    use hyper::Request;

    let f = filters(
        "||example.org/ads/\n\
        ||example.org/ads/$app=com.reader,xhr,replace=/X/Y/\n\
        @@||example.org/ads/except$replace\n\
        @@||page.org^$content\n",
    );
    for (path, dest, app, referer, blocked) in [
        ("start", "empty", "com.reader", "", false),
        ("start", "script", "com.reader", "", true),
        ("start", "empty", "com.other", "", true),
        ("except", "empty", "com.reader", "", true),
        ("start", "empty", "com.reader", "https://page.org/", true),
    ] {
        let url = format!("https://example.org/ads/{path}");
        let request = Request::get(&url)
            .header("sec-fetch-dest", dest)
            .header("referer", referer)
            .body(())
            .unwrap();
        assert_eq!(
            evaluate_request(&f.for_app(app), &request, &url).blocked,
            blocked
        );
    }
}

#[test]
fn list_patterns_with_literal_braces_repetitions_and_commas_compile() {
    for rule in [
        r"||a.example^$replace=/a{b/x/",
        r"||a.example^$replace=/\(\){return/x/",
        r"||a.example^$replace=/({adv:).(\,q:!0)/\$1!1\$2/",
        r"||a.example^$replace=/.{280\,400}.*/x/",
        r#"||a.example^$replace=/(^.{1\,150}\(\{).*?"bunch".*(}\);)/\$1\$2/,script"#,
    ] {
        assert_eq!(check(rule, &mut Patterns::default()), Some(true), "{rule}");
    }
}

#[test]
fn unsupported_regex_constructs_are_never_approximated() {
    for pattern in [
        r"a(?=b)", r"a(?!b)", r"(?<=b)a", r"(a)\1", "(?>a)", "a++a", "a*+a", "a?+a", "a{2,3}+a",
    ] {
        let rule = format!("||example.org^$replace=/{pattern}/x/").replace("{2,3}", r"{2\,3}");
        assert_eq!(
            check(&rule, &mut Patterns::default()),
            Some(false),
            "{pattern}"
        );
    }
}

#[test]
fn subscription_rules_compile_and_rewrite_the_intended_bytes() {
    let cases = [
        (
            r##"||football.ua/hnd/Android/News.ashx$replace=/<item type=\"banner\">.{280\,400}.*<\/background><\/item>//"##,
            Some((
                format!(
                    r#"start<item type="banner">{}</background></item>end"#,
                    "é".repeat(280)
                ),
                "startend",
            )),
        ),
        (
            r##"||static.xx.fbcdn.net/rsrc.php/v*.js$script,replace=/var n=s\(t\.call\(JSON\.parse\)\),/return false;var n=s(t.call(JSON.parse)),/"##,
            Some((
                r"var n=s(t.call(JSON.parse)),".to_owned(),
                "return false;var n=s(t.call(JSON.parse)),",
            )),
        ),
        (
            r##"||cdn.greenvideo.io/adapter/player/$script,replace=/window\.__gv_org_tfa.*?templateData\:\{embedId\:[a-z0-9]{1,3}\}\}\)\}\)\}\}//"##,
            None,
        ),
        (
            r##"||botrix.live/web_files/main-es2015.*.js$replace=/isUsingAdblock\(\){return window\.usingAdblock/isUsingAdblock(){return;/"##,
            Some((
                r"isUsingAdblock(){return window.usingAdblock".to_owned(),
                "isUsingAdblock(){return;",
            )),
        ),
        (
            r##"||tamrieltradecentre.com/bundles/View?v=$script,replace=/LoadFirstPartyAsync\(\) {/LoadFirstPartyAsync() {return;/"##,
            None,
        ),
        (
            r##"||filecr.com/_next/static/chunks/pages/*.js$replace=/(return new Promise\(.=>{.\().\|\|.(\)}\))/\$1!1\$2/"##,
            None,
        ),
        (
            r##"/bundle_js.ashx$replace=/function\(\){try{if\(\(eval\(atob[\s\S]*?}}}\,/function(){}\,/,domain=mobilmania.cz"##,
            None,
        ),
        (
            r##"||wp.pl^$replace=/(^.{1\,150}\(\{).*?"bunch".*?"bidRequestId".*(}\);)/\$1\$2/,script"##,
            Some((
                r#"x({"bunch":1,"bidRequestId":2}); more"#.to_owned(),
                "x({}); more",
            )),
        ),
        (
            r##"||wp-pilot-gatsby.wpcdn.pl/*.js$replace=/({adv:).(\,qoeEnabled:!0\,advVastDuration:)/\$1!1\$2/"##,
            None,
        ),
        (
            r##"||thestar.co.uk/_build/main.*.js$replace=/\,a\w\whield:\w+//"##,
            None,
        ),
        (
            r##"||pluto.tv/*/dash/*/*.mpd$replace=/<Period.{1\,100}?><BaseURL>.{1\,100}\.plutotv\.net\/.{1\,50}_ad\/.*?<\/BaseURL>[\s\S]*?<\/Period>//"##,
            None,
        ),
        (
            r##"||today.line.me/dist/80980ed1/app.80980ed1.js$replace=/Vt={isLoaded:\!1/Vt={isLoaded:1/"##,
            None,
        ),
    ];
    for (rule, sample) in cases {
        let (pattern, opts) = split(rule).unwrap();
        let parsed = Parsed::parse(pattern, opts).unwrap();
        let action = parsed
            .action
            .unwrap()
            .compile()
            .unwrap_or_else(|| panic!("{rule}"));
        if let Some((input, expected)) = sample {
            assert_eq!(
                action.apply(input.as_bytes(), &|| true).unwrap(),
                expected.as_bytes(),
                "{rule}"
            );
        }
    }
}

mod cookie;
mod hls;
mod prune;
mod removeheader;
mod urltransform;

#[test]
fn response_patterns_and_their_exceptions_survive_the_cache_without_network_app_rules() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("rules");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("user.txt"),
        r"||example.org^$app=/^com\.reader[12]\$/,replace=/X/Y/
@@||example.org/skip$app=COM.READER1*,replace",
    )
    .unwrap();
    let cache = temp.path().join("cache");
    let built = Filters::load_cached(&[&dir], &cache).unwrap();
    let cached = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
    for f in [built, cached] {
        for (app, path, expected) in [
            ("com.reader1", "/", "Y Z"),
            ("com.reader1", "/skip", "X Z"),
            ("com.reader2", "/skip", "Y Z"),
            ("com.reader3", "/", "X Z"),
            ("@com.reader1", "/", "X Z"),
        ] {
            let url = format!("https://example.org{path}");
            assert_eq!(changed(&f, &url, Page::NONE, "other", app), expected);
        }
    }
}
