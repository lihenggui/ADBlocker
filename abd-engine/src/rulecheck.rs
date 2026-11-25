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

//! Rules as the engine reads them: which it applies, and which not.
//!
//! The check uses the permissions of the user's own rules.

use crate::filter::{Patterns, valid_app_rule, valid_response_rule, valid_script_exception};
use crate::rule::javascript::Scripts;
use crate::rule::{self, Kind, MatcherCheck};

use crate::protocol_types::{Issue, IssueKind, RulesCheck};
pub use crate::rule::{count, is_comment};

/// The rules of `text` that will not be applied, by line.
pub fn check(text: &str) -> RulesCheck {
    crate::public_suffix::install();
    let mut patterns = Patterns::default();
    let mut scripts = Scripts::default();
    let mut rules = 0;
    let mut issues = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let rule = rule::read_with_scripts(line, Some(&mut scripts));
        if matches!(rule.kind, Kind::Comment) {
            continue;
        }
        rules += 1;
        let kind = match &rule.kind {
            Kind::Comment => unreachable!(),
            Kind::Html(_) => continue,
            Kind::Response(response) if valid_response_rule(response, &mut patterns) => continue,
            Kind::App(app) if valid_app_rule(app, &mut patterns) => continue,
            Kind::PageException(_) if valid_script_exception(&rule) => continue,
            Kind::App(_) | Kind::Response(_) => IssueKind::Invalid,
            Kind::Matcher | Kind::Cosmetic | Kind::PageException(_) | Kind::NotApplied(_) => {
                match rule.matcher_check() {
                    MatcherCheck::Network | MatcherCheck::AppliedCosmetic => continue,
                    MatcherCheck::Cosmetic => IssueKind::Cosmetic,
                    MatcherCheck::Invalid => IssueKind::Invalid,
                }
            }
        };
        issues.push(Issue {
            line: n + 1,
            text: rule.line.to_owned(),
            kind,
        });
    }
    RulesCheck { rules, issues }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(usize, IssueKind)> {
        check(text)
            .issues
            .into_iter()
            .map(|i| (i.line, i.kind))
            .collect()
    }

    #[test]
    fn rules_that_need_a_page_are_told_from_rules_that_are_none() {
        let text = "! comment\n\n||ads.example^\n\
                    example.org##.ad\n\
                    ##+js(nobab)\n\
                    example.org#%#//scriptlet('abort')\n\
                    example.org$$script[data-ad]\n\
                    ||ads.example^$frobnicate\n\
                    x\n\
                    # a hosts comment\n\
                    0.0.0.0 tracker.example\n\
                    ||ads.example^$app=com.reader\n\
                    ||ads.example^$app=not/a/package\n\
                    ||ads.example^$app=com.reader,domain=site.example\n";
        assert_eq!(
            kinds(text),
            [
                (4, IssueKind::Cosmetic),
                (5, IssueKind::Cosmetic),
                (6, IssueKind::Cosmetic),
                (8, IssueKind::Invalid),
                (9, IssueKind::Invalid),
                (13, IssueKind::Invalid),
                (14, IssueKind::Invalid),
            ]
        );
        assert_eq!(check(text).rules, 11);
    }

    #[test]
    fn supported_adguard_cosmetics_drop_out_of_the_issue_count() {
        let text = "example.org#%#//scriptlet('set-constant', 'x', 'true')\n\
                    example.org#$#.ad { display: none; }\n\
                    example.org#%#//scriptlet('trusted-set-constant', 'x', 'true')\n\
                    example.org#%#window.close()\n\
                    example.org$$script";
        assert_eq!(check(text).rules, 5);
        assert_eq!(kinds(text), [(3, IssueKind::Cosmetic)]);
    }

    #[test]
    fn denyallow_rules_are_reported_when_the_matcher_cannot_apply_them() {
        let text = "/ad[s]?/$denyallow=x.com\n\
                    *$denyallow=x.com\n\
                    *$script,domain=a.*,denyallow=x.com\n\
                    *$script,domain=a.*|b.com,denyallow=x.com\n\
                    /assets/ad.js$denyallow=x.com\n\
                    *$app=~com.reader,denyallow=x.com";
        let checked = check(text);
        assert_eq!(checked.rules, 6);
        assert_eq!(
            kinds(text),
            [
                (1, IssueKind::Invalid),
                (2, IssueKind::Invalid),
                (3, IssueKind::Invalid),
                (6, IssueKind::Invalid)
            ]
        );
        for issue in checked.issues {
            assert_eq!(Some(issue.text.as_str()), text.lines().nth(issue.line - 1));
        }
    }

    #[test]
    fn javascript_and_page_script_exceptions_are_checked_with_user_permissions() {
        let text = "site.test#%#window.fixture = 1;\n\
                    except.site.test#@%#window.fixture = 1;\n\
                    @@||site.test^$jsinject,elemhide\n\
                    @@||site.test^$jsinject,content\n\
                    @@||site.test^$jsinject,extension\n\
                    @@||site.test^$extension\n\
                    site.test#%#window.fixture = '</script>';\n";
        assert_eq!(
            kinds(text),
            [(6, IssueKind::Invalid), (7, IssueKind::Cosmetic)]
        );
    }

    #[test]
    fn app_patterns_are_checked_as_a_whole_for_network_and_response_rules() {
        let invalid = [
            "com.*.music",
            "com.app|~com.other",
            "com.*|~/other/",
            "/[ab/",
            r"/\w{100000}/",
            "/a/b/",
            "/a,b/",
            "/a|b/",
            "/a$script",
            "/missing",
            "com.reader||other",
            "com.reader|",
            "*",
        ];
        for value in invalid {
            for rule in [
                format!("||example.org^$app={value}"),
                format!("||example.org^$replace=/X/Y/,app={value}"),
            ] {
                assert_eq!(kinds(&rule), [(1, IssueKind::Invalid)], "{rule}");
            }
        }
        for rule in [
            "||graph.facebook.com^$app=com.vitastudio.*",
            r"||example.org^$app=/^com\.reader\$\|^org\.example\./,script",
            r"/url$/$app=/^com\.reader\$/",
            r"||example.org^$app=~/a\,b\/c\$\|reader/,replace=/X/Y/",
        ] {
            assert!(kinds(rule).is_empty(), "{rule}");
        }
    }

    #[test]
    fn the_header_and_comments_of_a_list_are_not_rules() {
        assert_eq!(
            count("[Adblock Plus 2.0]\n! Title: X\n# hosts\n\n||a.example^\n#@#.ad\n"),
            2
        );
    }
}
