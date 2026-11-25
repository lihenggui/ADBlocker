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

fn active(text: &str) -> Vec<&str> {
    lines(text)
        .filter(|line| !crate::rule::is_comment(line))
        .collect()
}

fn issues(text: &str) -> Vec<(usize, DirectiveIssue)> {
    let mut issues = scan(text)
        .filter_map(|((number, _), result)| result.err().map(|issue| (number, issue)))
        .collect::<Vec<_>>();
    issues.sort_unstable_by_key(|&(number, _)| number);
    issues
}

#[test]
fn the_adguard_examples_keep_the_android_rules_and_report_each_active_include() {
    let rules = "||example.org^$third-party\ndomain.com##div.ad\n";
    let adguard = format!("!#if (adguard && !adguard_ext_safari)\n{rules}!#endif");
    let safari = format!(
        "!#if (adguard && !adguard_ext_safari)\n{rules}!#else\n||subdomain.example.org^$third-party\n!#endif"
    );
    for text in [adguard, safari] {
        assert_eq!(active(&text), rules.lines().collect::<Vec<_>>());
        assert!(issues(&text).is_empty());
    }

    let android = "!#if (adguard_app_android)\n!#include /androidspecific.txt\n!#endif";
    assert_eq!(issues(android), [(2, DirectiveIssue::Include)]);
    let includes = "\
!#include https://example.org/path/includedfile.txt
!#include /includedfile.txt
!#include includedfile.txt
!#include ../path2/includedfile.txt
!#include https://domain.com/path/includedfile.txt";
    assert!(active(includes).is_empty());
    assert_eq!(
        issues(includes),
        (1..=5)
            .map(|line| (line, DirectiveIssue::Include))
            .collect::<Vec<_>>()
    );
}

#[test]
fn ublock_html_capability_selects_the_html_rule() {
    let text = "!#if cap_html_filtering\nexample.com##^script:has-text(fakeAd)\n\
                !#else\nexample.com##+js(rmnt, script, fakeAd)\n!#endif";
    assert_eq!(active(text), ["example.com##^script:has-text(fakeAd)"]);
}

#[test]
fn outer_conditions_gate_both_branches_of_nested_blocks() {
    let text = "before\n!#if false\n!#if adguard\nhidden\n!#else\nalso-hidden\n!#endif\n\
                !#include dormant.txt\n!#else\nfallback\n\
                !#if env_mobile\nmobile\n!#endif\n!#endif\nafter";
    assert_eq!(active(text), ["before", "fallback", "mobile", "after"]);
    assert!(issues(text).is_empty());
}

#[test]
fn malformed_directives_recover_at_their_own_block_boundaries() {
    let text = "!#else\n!#endif\n\
                !#if adguard &&\nhidden\n!#else trailing\nstill-hidden\n!#else\n\
                fallback\n!#else\nstill-fallback\n!#endif trailing\n\
                same-branch\n!#endif\nroot\n!#if adguard\nouter\n!#if false\nhidden-at-eof";
    assert_eq!(
        active(text),
        ["fallback", "still-fallback", "same-branch", "root", "outer"]
    );
    assert_eq!(
        issues(text),
        [1, 2, 3, 5, 9, 11, 15, 17].map(|line| (line, DirectiveIssue::Malformed))
    );
    assert_eq!(issues("!#if\n"), [(1, DirectiveIssue::Malformed)]);
    assert_eq!(
        issues("!#if false\n!#if adguard &\n!#endif\n!#endif"),
        [(2, DirectiveIssue::Malformed)]
    );
}

#[test]
fn excess_nesting_drops_its_entire_subtree_and_resumes_at_the_parent() {
    let text = format!(
        "{}!#if adguard\n!#else\nhidden\n!#if adguard\nalso-hidden\n!#endif\n!#endif\n\
         inside-limit\n{}outside",
        "!#if adguard\n".repeat(MAX_NESTING),
        "!#endif\n".repeat(MAX_NESTING),
    );
    assert_eq!(active(&text), ["inside-limit", "outside"]);
    assert_eq!(
        issues(&text),
        [MAX_NESTING + 1, MAX_NESTING + 4].map(|line| (line, DirectiveIssue::Malformed))
    );
}

#[test]
fn whitespace_between_the_prefix_and_keyword_leaves_a_comment() {
    let text = "!# if false\n!#safari_cb_affinity(general)\nkept\n!#ifSomething false\nalso-kept\n\
                \t!#if\tfalse\ndropped\n!#endif";
    assert_eq!(active(text), ["kept", "also-kept"]);
    assert!(issues(text).is_empty());
}
