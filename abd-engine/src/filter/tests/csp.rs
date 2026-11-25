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

const RULES: &str = "||example.org^$csp=frame-src 'none'
    ||example.org^$csp=script-src 'self' 'unsafe-eval' http: https:
    @@||example.org/page/*$csp=frame-src 'none'
    @@||example.org/all/*$csp
    @@||example.org/doc/*$document
    @@||example.org/url/*$urlblock
    @@||example.org/body/*$content
    @@||example.org/sub/*$csp,subdocument
    ||example.org^$app=com.reader,csp=style-src 'none'
    @@||example.org/app/*$app=com.reader,csp";

#[test]
fn csp_policies_and_response_actions_stay_in_their_own_matchers_after_cache_reload() {
    let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(dir.path().join("list.txt"), RULES).unwrap();
    let rules = dir.path().join("rules");
    std::fs::create_dir(&rules).unwrap();
    std::fs::write(rules.join("user.txt"), "||example.org^$removeheader=x-test").unwrap();
    let built = Filters::load_cached(&[dir.path(), &rules], cache.path()).unwrap();
    let cached = Filters::read_cache(cache.path(), built.generation.unwrap()).unwrap();
    let issues = crate::rulecheck::check(RULES).issues;
    assert_eq!(
        issues
            .iter()
            .map(|issue| issue.text.as_str())
            .collect::<Vec<_>>(),
        [
            "@@||example.org/url/*$urlblock",
            "@@||example.org/body/*$content"
        ],
    );
    for filters in [built, cached] {
        let web = filters.for_app("com.other");
        let app = filters.for_app("com.reader");
        let csp = |path: &str, kind, app: &AppFilters<'_>| {
            let mut policies: Vec<_> = app
                .csp(
                    &format!("https://example.org/{path}"),
                    Page::Itself,
                    kind,
                    "GET",
                )
                .into_iter()
                .flat_map(|policies| policies.split(',').map(str::to_owned).collect::<Vec<_>>())
                .collect();
            policies.sort_unstable();
            policies
        };
        let both = [
            "frame-src 'none'",
            "script-src 'self' 'unsafe-eval' http: https:",
        ];
        assert_eq!(csp("", "document", &web), both);
        assert_eq!(csp("body/a", "document", &web), both);
        assert_eq!(csp("page/a", "document", &web), [both[1]]);
        for path in ["all/a", "doc/a", "url/a"] {
            assert!(csp(path, "document", &web).is_empty(), "{path}");
        }
        assert_eq!(csp("sub/a", "document", &web), both);
        assert!(csp("sub/a", "subdocument", &web).is_empty());
        assert_eq!(
            csp("", "document", &app),
            [both[0], both[1], "style-src 'none'"]
        );
        assert_eq!(csp("app/a", "document", &app), both);
        let actions = web
            .response_matches("https://example.org/", Page::Itself, "GET")
            .unwrap()
            .actions("document", false);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0].as_ref(),
            crate::rule::action::ResponseAction::RemoveHeader(_)
        ));
    }
}

#[test]
fn invalid_csp_rules_do_not_become_network_or_response_rules() {
    let rules = "||example.org^$csp
        ||example.org^$csp=
        ||example.org^$~csp=frame-src 'none'
        ||example.org^$csp=frame-src 'none'\\,script-src 'none'
        ||example.org^$csp=frame-src 'none'$script-src 'none'
        ||example.org^$csp=default-src 'none'; REPORT-URI https://example.org/log
        ||example.org^$csp=report-to reports
        ||example.org^$csp=frame-src 'none',method=GET
        ||example.org^$csp=frame-src 'none',image";
    assert_eq!(
        crate::rulecheck::check(rules).issues.len(),
        rules.lines().count()
    );
    let f = filters(rules);
    let f = f.for_app("com.reader");
    assert!(
        f.csp("https://example.org/", Page::Itself, "document", "GET")
            .is_none()
    );
    assert!(!f.blocks_host("example.org"));
}
