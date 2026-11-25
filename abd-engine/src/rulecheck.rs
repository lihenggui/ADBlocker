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

//! Rules as the engine reads them, including the reasons it sets lines aside.

use crate::filter::{
    AddressProjections, Patterns, valid_page_exception, validate_app_rule, validate_response_rule,
};
use crate::protocol_types::{Issue, RulesCheck};
use crate::rule::javascript::Scripts;
use crate::rule::preprocess;
use crate::rule::{self, Code, Kind, RuleReason};

pub use crate::rule::{SourcePermissions, count, is_comment};

/// The unapplied rules and active directive issues, in source order.
pub fn check(text: &str, permissions: SourcePermissions) -> RulesCheck {
    crate::public_suffix::install();
    let mut checker = Checker::default();
    let mut rules = 0;
    let mut issues = Vec::new();
    for ((number, line), directive) in preprocess::scan(text) {
        let reason = match directive {
            Err(issue) => issue.into(),
            Ok(()) => {
                let source = rule::read_with_scripts(
                    line,
                    permissions.scripts().then_some(&mut checker.scripts),
                );
                if matches!(source.kind, Kind::Comment) {
                    continue;
                }
                rules += 1;
                let Err(reason) = checker.validate(&source, permissions) else {
                    continue;
                };
                reason
            }
        };
        issues.push(Issue {
            line: number,
            text: line.trim().to_owned(),
            kind: reason.kind(),
            reason: reason.detail(),
        });
    }
    issues.sort_by_key(|issue| issue.line);
    RulesCheck { rules, issues }
}

#[derive(Default)]
struct Checker<'a> {
    patterns: Patterns,
    addresses: AddressProjections,
    scripts: Scripts<'a>,
}

impl Checker<'_> {
    fn validate(
        &mut self,
        source: &rule::Rule<'_>,
        permissions: SourcePermissions,
    ) -> Result<(), RuleReason> {
        match &source.kind {
            Kind::Comment | Kind::Html(_) => Ok(()),
            Kind::Address(address) => self.addresses.add(address),
            Kind::Response(response) => {
                if permissions.allows_response(response) {
                    return validate_response_rule(response, &mut self.patterns);
                }
                // A malformed action still needs a syntax diagnosis. It must
                // not take any of the app-pattern budget of the actual list.
                validate_response_rule(response, &mut Patterns::default())?;
                Err(RuleReason::named(Code::ResponseTrust, response.option()))
            }
            Kind::App(app) => validate_app_rule(app, &mut self.patterns),
            Kind::PageException(_) if valid_page_exception(source) => Ok(()),
            _ => source.check_matcher(permissions),
        }
    }
}

#[cfg(test)]
#[path = "rulecheck/corpus.rs"]
mod corpus;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol_types::IssueKind;

    fn kinds(text: &str) -> Vec<(usize, IssueKind)> {
        check(text, SourcePermissions::USER)
            .issues
            .into_iter()
            .map(|i| (i.line, i.kind))
            .collect()
    }

    #[test]
    fn the_same_text_is_checked_with_user_and_list_permissions() {
        let text = "||ads.example^$frobnicate\n\
                    ||ads.example^$header=x-test\n\
                    ||ads.example^$popup\n\
                    ||ads.example^$replace=/ad/content/\n\
                    site.example#%#window.fixture = 1;\n\
                    site.example##+js(trusted-set, fixture, json:42)\n\
                    site.example##.ad\n";
        use IssueKind::{Inapplicable, Invalid, Unsupported, Untrusted};
        let common = [
            (1, Invalid, "option:frobnicate"),
            (2, Unsupported, "option:header"),
            (3, Inapplicable, "option:popup"),
        ];
        for (permissions, extra) in [
            (
                SourcePermissions::USER,
                vec![(6, Unsupported, "scriptlet-trust:trusted-set")],
            ),
            (
                SourcePermissions::default(),
                vec![
                    (4, Untrusted, "response-trust:replace"),
                    (5, Untrusted, "javascript-untrusted"),
                    (6, Untrusted, "scriptlet-trust:trusted-set"),
                ],
            ),
        ] {
            let result = check(text, permissions);
            assert_eq!(result.rules, 7);
            assert_eq!(
                result
                    .issues
                    .iter()
                    .map(|issue| (issue.line, issue.kind, issue.reason.as_str()))
                    .collect::<Vec<_>>(),
                common.into_iter().chain(extra).collect::<Vec<_>>(),
            );
        }
    }

    #[test]
    fn a_malformed_rule_from_a_list_still_reports_its_syntax() {
        let result = check(
            "example.org#%#\n||ads.example^$replace=bad-value",
            SourcePermissions::default(),
        );
        assert_eq!(
            result
                .issues
                .iter()
                .map(|issue| (issue.kind, issue.reason.as_str()))
                .collect::<Vec<_>>(),
            [
                (IssueKind::Invalid, "javascript-empty"),
                (IssueKind::Invalid, "replace-value")
            ],
        );
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
                (5, IssueKind::Invalid),
                (6, IssueKind::Unsupported),
                (8, IssueKind::Invalid),
                (9, IssueKind::Invalid),
                (13, IssueKind::Invalid),
                (14, IssueKind::Invalid),
            ]
        );
        assert_eq!(check(text, SourcePermissions::USER).rules, 11);
    }

    #[test]
    fn supported_adguard_cosmetics_drop_out_of_the_issue_count() {
        let text = "example.org#%#//scriptlet('set-constant', 'x', 'true')\n\
                    example.org#$#.ad { display: none; }\n\
                    example.org#%#//scriptlet('trusted-set-constant', 'x', 'true')\n\
                    example.org#%#window.close()\n\
                    example.org$$script";
        assert_eq!(check(text, SourcePermissions::USER).rules, 5);
        assert_eq!(kinds(text), [(3, IssueKind::Unsupported)]);
    }

    #[test]
    fn denyallow_rules_are_reported_when_the_matcher_cannot_apply_them() {
        let text = "/ad[s]?/$denyallow=x.com\n\
                    *$denyallow=x.com\n\
                    *$script,domain=a.*,denyallow=x.com\n\
                    *$script,domain=a.*|b.com,denyallow=x.com\n\
                    /assets/ad.js$denyallow=x.com\n\
                    *$app=~com.reader,denyallow=x.com";
        let checked = check(text, SourcePermissions::USER);
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
            [(6, IssueKind::Inapplicable), (7, IssueKind::Invalid)]
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
