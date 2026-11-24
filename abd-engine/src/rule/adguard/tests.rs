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

use adblock::resources::{
    InMemoryResourceStorage, MimeType, PermissionMask, Resource, ResourceStorage, ResourceType,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

use super::{Unsupported, names, scriptlets};
use crate::rule::{Kind, MatcherCheck, Reason, read};

#[test]
fn the_name_table_points_to_bundled_scriptlets_with_the_same_trust() {
    let resources: Vec<Resource> =
        serde_json::from_str(include_str!("../../../resources/scriptlets.json")).unwrap();
    for &(name, target) in names::NAMES {
        assert_eq!(names::resolve(name), Ok(target));
        let target = format!("{target}.js");
        let resource = resources
            .iter()
            .find(|resource| resource.name == target || resource.aliases.contains(&target))
            .unwrap_or_else(|| panic!("{name} needs {target}"));
        assert!(resource.kind.supports_scriptlet_injection());
        assert_eq!(
            resource
                .permission
                .is_injectable_by(PermissionMask::default()),
            !name.trim_start_matches("ubo-").starts_with("trusted-"),
            "{name}"
        );
    }
}

#[test]
fn quoted_adguard_arguments_round_trip_through_the_matchers_resource_parser() {
    let storage =
        ResourceStorage::from_backend(InMemoryResourceStorage::from_resources([Resource {
            name: "capture.js".into(),
            aliases: vec![],
            kind: ResourceType::Mime(MimeType::ApplicationJavascript),
            content: STANDARD.encode("function capture(...args) {}"),
            dependencies: vec![],
            permission: PermissionMask::default(),
        }]));
    for (input, expected) in [
        (r#"'plain', "two words""#, vec!["plain", "two words"]),
        (
            r#"'comma,inside', 'a\,b', "\"quoted\"", 'can\'t'"#,
            vec!["comma,inside", r"a\,b", "\"quoted\"", "can't"],
        ),
        (
            r#"'a\b', 'c\\d', '', ' ', ''"#,
            vec![r"a\b", r"c\\d", "", " ", ""],
        ),
        (
            r#"'it\'s \"quoted\" `text`, still'"#,
            vec![r#"it's \"quoted\" `text`, still"#],
        ),
    ] {
        let mut call = String::from("capture");
        for arg in scriptlets::read_args(input).unwrap() {
            call.push_str(", ");
            scriptlets::write_arg(&mut call, &arg).unwrap();
        }
        let js_args = expected
            .iter()
            .map(|arg| serde_json::to_string(arg).unwrap())
            .collect::<Vec<_>>()
            .join(", ");
        let result = storage.get_scriptlet_resources([(&*call, PermissionMask::default())]);
        assert!(
            result.contains(&format!("capture({js_args})")),
            "{input}: {call}: {result}"
        );
    }
    let mut call = String::from("capture, ");
    scriptlets::write_arg(&mut call, "end\\").unwrap();
    call.push_str(", ''");
    let result = storage.get_scriptlet_resources([(&*call, PermissionMask::default())]);
    assert!(result.contains(r#"capture("end\\", "")"#), "{result}");
}

fn projects(input: &str, expected: &str) {
    let rule = read(input);
    assert!(
        matches!(rule.kind, Kind::Cosmetic),
        "{input}: {:?}",
        rule.matcher_input()
    );
    assert_eq!(rule.line, input);
    assert_eq!(rule.matcher_input().as_deref(), Some(expected), "{input}");
}

#[test]
fn each_adguard_form_projects_with_its_domains_and_exception_marker() {
    for (input, expected) in [
        (
            "example.org,~skip.example.org#%#//scriptlet('set-constant', 'x', 'true')",
            "example.org,~skip.example.org##+js(set-constant, x, true)",
        ),
        (
            "sub.example.org#@%#//scriptlet('set-constant', 'x', 'true')",
            "sub.example.org#@#+js(set-constant, x, true)",
        ),
        (
            "sub.example.org#@%#//scriptlet()",
            "sub.example.org#@#+js()",
        ),
        (
            "example.org,~skip.example.org#$#.ad { display: none; }",
            "example.org,~skip.example.org##.ad:style(display: none;)",
        ),
        (
            "sub.example.org#@$#.ad { display: none; }",
            "sub.example.org#@#.ad:style(display: none;)",
        ),
        (
            "example.org#$#.ad { remove: true; }",
            "example.org##.ad:remove()",
        ),
        (
            "sub.example.org#@$#.ad { remove: true; }",
            "sub.example.org#@#.ad:remove()",
        ),
        (
            "example.org,~skip.example.org#$?#div:contains(Advert) { opacity: 0; }",
            "example.org,~skip.example.org#?#div:contains(Advert):style(opacity: 0;)",
        ),
        (
            "sub.example.org#@$?#div:contains(Advert) { remove: true; }",
            "sub.example.org#@?#div:contains(Advert):remove()",
        ),
        (
            r#"example.org#$#a[data-token="}"] { color: rgb(10, 20, 30); }"#,
            r#"example.org##a[data-token="}"]:style(color: rgb(10, 20, 30);)"#,
        ),
    ] {
        projects(input, expected);
    }
}

#[test]
fn unsupported_adguard_forms_keep_their_own_reason() {
    use Unsupported::*;
    for (line, expected) in [
        (
            "example.org#%#//scriptlet('json-prune', broken)",
            ScriptletSyntax,
        ),
        ("example.org#%#//scriptlet('json-prune', )", ScriptletSyntax),
        ("example.org#%#//scriptlet()", ScriptletSyntax),
        ("example.org#%#//scriptlet('invented')", ScriptletName),
        (
            "example.org#%#//scriptlet('inject-css-in-shadow-dom')",
            ScriptletName,
        ),
        (
            "example.org#%#//scriptlet('amazon-apstag')",
            ScriptletResource,
        ),
        (
            "example.org#%#//scriptlet('prevent-window-open', '0', 'popup')",
            ScriptletArguments,
        ),
        (
            "example.org#%#//scriptlet('set-constant', 'x', 'false', 'stack')",
            ScriptletArguments,
        ),
        (
            "example.org#%#//scriptlet('prevent-fetch', '*', 'emptyStr', 'status:404')",
            ScriptletArguments,
        ),
        (
            "example.org#%#//scriptlet('prevent-fetch', 'url:ads method:POST')",
            ScriptletArguments,
        ),
        (
            "example.org#%#//scriptlet('abort-on-property-read', '</ScRiPt')",
            UnsafeScriptlet,
        ),
        (
            "example.org#%#//scriptlet('abort-on-property-read', '<!--')",
            UnsafeScriptlet,
        ),
        (
            "example.org#$#@media (width > 60px) { .ad { color: red; } }",
            StyleSyntax,
        ),
        (
            "example.org#$#.one { color: red; } .two { color: blue; }",
            StyleSyntax,
        ),
        ("example.org#$#.ad { remove: false; }", StyleSyntax),
        (
            "example.org#$#.ad { background: url(https://example.net/a); }",
            StyleDeclarations,
        ),
        (
            "example.org#$#.ad { background: image(https://example.net/a); }",
            StyleDeclarations,
        ),
        ("example.org,~sub.example.org#@%#//scriptlet()", Matcher),
        ("~example.org#%#//scriptlet('nowebrtc')", Matcher),
        ("example.org,~/example/#%#//scriptlet('nowebrtc')", Matcher),
        ("#$#.ad { color: red; }", Matcher),
    ] {
        let rule = read(line);
        let Kind::NotApplied(Reason::AdGuard(reason)) = rule.kind else {
            panic!("expected a rejected cosmetic rule: {line}");
        };
        assert_eq!(reason, expected, "{line}");
        assert!(rule.matcher_input().is_none());
    }
    assert!(matches!(
        read("example.org$$script[data-ad]").kind,
        Kind::NotApplied(Reason::HtmlFiltering)
    ));
}

#[test]
fn scriptlet_adapters_keep_match_conditions_and_permission_requirements() {
    for (input, expected) in [
        (
            "example.org#%#//scriptlet('prevent-window-open', '*', '2', 'obj')",
            "example.org##+js(prevent-window-open, '', 2, obj)",
        ),
        (
            "example.org#%#//scriptlet('prevent-fetch', '*', 'emptyArr')",
            "example.org##+js(prevent-fetch, /^/, emptyArr)",
        ),
        (
            "example.org#%#//scriptlet('set-constant', 'fixture', 'false', '', 'asFunction')",
            "example.org##+js(set-constant, fixture, false, as, function)",
        ),
        (
            "example.org#%#//scriptlet('json-prune-fetch-response', 'ads', '', 'ads.example.org', 'fetchAds')",
            "example.org##+js(json-prune-fetch-response, ads, '', propsToMatch, ads.example.org, stackToMatch, fetchAds)",
        ),
        (
            r#"example.org#%#//scriptlet('trusted-set-constant', 'fixture', '"literal"')"#,
            r#"example.org##+js(trusted-set-constant, fixture, json:"literal")"#,
        ),
        (
            "example.org#%#//scriptlet('ubo-acis.js', 'eval', 'split')",
            "example.org##+js(abort-current-script, eval, split)",
        ),
    ] {
        projects(input, expected);
    }
    let trusted = read("example.org#%#//scriptlet('trusted-set-constant', 'fixture', '42')");
    assert!(trusted.is_scriptlet());
    assert!(matches!(trusted.matcher_check(), MatcherCheck::Cosmetic));
    assert!(matches!(
        read("example.org#%#//scriptlet('set-constant', 'fixture', '42')").matcher_check(),
        MatcherCheck::AppliedCosmetic
    ));
}

// Android builds of AdGuard Base 2.5.6.88, Chinese 2.1.70.20
// and Mobile Ads 2.0.73.63. Only the domains are replaced.
const ANDROID_RULES: [(&str, &str); 20] = [
    (
        r#"#%#//scriptlet('set-constant', 'showModal', 'noopFunc')"#,
        "##+js(set-constant, showModal, noopFunc)",
    ),
    (
        r#"#%#//scriptlet('set-constant', 'app_vars.force_disable_adblock', 'undefined')"#,
        "##+js(set-constant, app_vars.force_disable_adblock, undefined)",
    ),
    (
        r#"#%#//scriptlet('abort-current-inline-script', 'eval', 'split')"#,
        "##+js(abort-current-script, eval, split)",
    ),
    (
        r#"#%#//scriptlet("abort-current-inline-script", "decodeURIComponent", "String.fromCharCode")"#,
        "##+js(abort-current-script, decodeURIComponent, String.fromCharCode)",
    ),
    (
        r#"#%#//scriptlet('prevent-window-open', '_blank', '1')"#,
        "##+js(prevent-window-open, _blank, 1)",
    ),
    (
        r#"#%#//scriptlet('prevent-window-open', '?key=', '15')"#,
        "##+js(prevent-window-open, ?key=, 15)",
    ),
    (
        r#"#%#//scriptlet('prevent-fetch', 'pagead2.googlesyndication.com')"#,
        "##+js(prevent-fetch, pagead2.googlesyndication.com, emptyObj)",
    ),
    (
        r#"#%#//scriptlet('prevent-fetch', '/pagead2\.googlesyndication.com|static\.doubleclick\.net/')"#,
        r"##+js(prevent-fetch, /pagead2\.googlesyndication.com|static\.doubleclick\.net/, emptyObj)",
    ),
    (
        r#"#%#//scriptlet('prevent-setTimeout', '/showModal\(\)|chooseAction\(\)/')"#,
        r"##+js(prevent-setTimeout, /showModal\(\)|chooseAction\(\)/)",
    ),
    (
        r#"#%#//scriptlet('prevent-setTimeout', '](getComputedStyle,')"#,
        "##+js(prevent-setTimeout, '](getComputedStyle,')",
    ),
    (
        r#"#%#//scriptlet("prevent-addEventListener", "DOMContentLoaded", "adsBlocked")"#,
        "##+js(addEventListener-defuser, DOMContentLoaded, adsBlocked)",
    ),
    (
        r#"#%#//scriptlet('prevent-addEventListener', 'DOMContentLoaded', 'detectAdBlocker')"#,
        "##+js(addEventListener-defuser, DOMContentLoaded, detectAdBlocker)",
    ),
    (
        r#"#%#//scriptlet("abort-on-property-read", "mdp_deblocker")"#,
        "##+js(abort-on-property-read, mdp_deblocker)",
    ),
    (
        r#"#%#//scriptlet('abort-on-property-read', 'freeProConfig.btnRefresh')"#,
        "##+js(abort-on-property-read, freeProConfig.btnRefresh)",
    ),
    (
        r#"#%#//scriptlet("json-prune", "*", "ad_to_link")"#,
        "##+js(json-prune, *, ad_to_link)",
    ),
    (
        r#"#%#//scriptlet('json-prune', 'ads')"#,
        "##+js(json-prune, ads)",
    ),
    (
        r#"#%#//scriptlet('abort-on-property-write', 'Databdcacb')"#,
        "##+js(abort-on-property-write, Databdcacb)",
    ),
    (
        r#"#%#//scriptlet("abort-on-property-write", "hidden")"#,
        "##+js(abort-on-property-write, hidden)",
    ),
    (
        "#$#.adblock-warning { display: none !important; }",
        "##.adblock-warning:style(display: none !important;)",
    ),
    (
        "#$#html { overflow: auto !important; }",
        "##html:style(overflow: auto !important;)",
    ),
];

#[test]
fn published_android_rules_translate_to_the_expected_matcher_inputs() {
    for (input, expected) in ANDROID_RULES {
        projects(
            &format!("example.org{input}"),
            &format!("example.org{expected}"),
        );
    }
}
