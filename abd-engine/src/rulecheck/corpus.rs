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

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use super::{SourcePermissions, check};
use crate::limits::{MAX_ADDRESS_RULES, MAX_JS_RULE, MAX_JS_SCRIPTS, MAX_JS_TABLE_BYTES};
use crate::list_url::file_name;
use crate::protocol_types::{Issue, IssueKind};
use crate::rule::Code;

const CORPUS: &str = include_str!("../../../testdata/rules/corpus.txt");
const PROTOCOL: &str = include_str!("../../../testdata/protocol/README.md");

struct Entry<'a> {
    line: usize,
    rule: &'a str,
    expected: &'a str,
    trusted: bool,
}

fn entries() -> impl Iterator<Item = Entry<'static>> {
    let mut trusted = false;
    let mut lines = CORPUS.lines().enumerate();
    std::iter::from_fn(move || {
        for (_, line) in lines.by_ref() {
            if line == "! source: trusted" {
                assert!(!trusted, "duplicate source marker");
                trusted = true;
            } else if let Some(expected) = line.strip_prefix("! expect: ") {
                let (line, rule) = lines.next().expect("expected a rule after its verdict");
                assert!(!rule.is_empty() && !rule.starts_with("! "), "{line}");
                return Some(Entry {
                    line: line + 1,
                    rule,
                    expected,
                    trusted: std::mem::take(&mut trusted),
                });
            } else {
                assert!(
                    line.is_empty() || line.starts_with("! "),
                    "missing verdict for {line}"
                );
            }
        }
        assert!(!trusted, "source marker without a rule");
        None
    })
}

fn verdict(issue: &Issue) -> String {
    let kind = match issue.kind {
        IssueKind::Invalid => "invalid",
        IssueKind::Unsupported => "unsupported",
        IssueKind::Inapplicable => "inapplicable",
        IssueKind::Untrusted => "untrusted",
    };
    format!("{kind} {}", issue.reason)
}

fn trusted_list() -> SourcePermissions {
    let name = file_name("https://filters.adtidy.org/android/filters/11.txt");
    SourcePermissions::for_path(&Path::new("lists").join(name))
}

/// Only generation-wide limits need preceding rules. Keep that padding
/// synthetic so the corpus still has one reviewable rule for each verdict.
fn limit_prefix(expected: &str) -> (String, usize) {
    let mut text = String::new();
    let count = match expected {
        "unsupported html-generation-limit" => {
            text = "example.org##^div\n".repeat(crate::limits::MAX_HTML_RULES);
            crate::limits::MAX_HTML_RULES
        }
        "unsupported address-limit" => {
            text = "192.0.2.1$network\n".repeat(MAX_ADDRESS_RULES);
            MAX_ADDRESS_RULES
        }
        "unsupported javascript-count" => {
            for n in 0..MAX_JS_SCRIPTS {
                writeln!(text, "example.org#%#window.fixture = {n};").unwrap();
            }
            MAX_JS_SCRIPTS
        }
        "unsupported javascript-table-size" => {
            let count = MAX_JS_TABLE_BYTES / MAX_JS_RULE;
            for n in 0..count {
                let body = format!("/*{n}*/");
                let padding = "x".repeat(MAX_JS_RULE - body.len() - 4);
                writeln!(text, "example.org#%#{body}/*{padding}*/").unwrap();
            }
            count
        }
        _ => 0,
    };
    (text, count)
}

#[test]
fn the_corpus_matches_the_checker_and_covers_every_reason_code() {
    let mut observed = BTreeSet::new();
    let mut preprocessor = Vec::new();
    let mut wrong = Vec::new();
    for entry in entries() {
        if entry.rule.starts_with("!#") || !preprocessor.is_empty() {
            preprocessor.push(entry);
            continue;
        }
        let permissions = if entry.trusted {
            trusted_list()
        } else {
            SourcePermissions::default()
        };
        let (mut text, prefix) = limit_prefix(entry.expected);
        text.push_str(entry.rule);
        let check = check(&text, permissions);
        assert_eq!(check.rules, prefix + 1, "corpus line {}", entry.line);
        assert!(check.issues.len() <= 1, "corpus line {}", entry.line);
        if let Some(issue) = check.issues.first() {
            assert_eq!(issue.line, prefix + 1);
            assert_eq!(issue.text, entry.rule);
        }
        compare(&entry, check.issues.first(), &mut observed, &mut wrong);
    }
    check_directives(&preprocessor, &mut observed, &mut wrong);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    let missing: Vec<_> = Code::ALL
        .iter()
        .filter(|code| !observed.contains(code.as_str()))
        .collect();
    assert!(missing.is_empty(), "missing corpus codes: {missing:?}");
}

fn compare(
    entry: &Entry<'_>,
    issue: Option<&Issue>,
    observed: &mut BTreeSet<String>,
    wrong: &mut Vec<String>,
) {
    let actual = issue.map_or_else(|| "applied".into(), verdict);
    if let Some(issue) = issue {
        observed.insert(issue.reason.split(':').next().unwrap().into());
    }
    if entry.expected != actual {
        let sample: String = entry.rule.chars().take(150).collect();
        wrong.push(format!(
            "corpus line {}: expected {}, got {actual} for {sample}",
            entry.line, entry.expected
        ));
    }
}

fn check_directives(
    entries: &[Entry<'_>],
    observed: &mut BTreeSet<String>,
    wrong: &mut Vec<String>,
) {
    assert!(entries.iter().all(|entry| !entry.trusted));
    let text = entries
        .iter()
        .map(|entry| entry.rule)
        .collect::<Vec<_>>()
        .join("\n");
    let check = check(&text, SourcePermissions::default());
    assert_eq!(
        check.rules,
        entries
            .iter()
            .filter(|entry| !entry.rule.starts_with("!#"))
            .count()
    );
    for (i, entry) in entries.iter().enumerate() {
        let issue = check.issues.iter().find(|issue| issue.line == i + 1);
        compare(entry, issue, observed, wrong);
    }
}

#[test]
fn every_reason_code_has_the_documented_class_and_spelling() {
    let mut documented = BTreeSet::new();
    for row in PROTOCOL.split("## Rule checks").nth(1).unwrap().lines() {
        let cells: Vec<_> = row.split('|').map(str::trim).collect();
        if let [_, kind, code, _, _] = cells.as_slice()
            && code.starts_with('`')
        {
            let code = code.trim_matches('`').trim_end_matches(":<name>");
            let class = kind.split('`').nth(1).unwrap();
            documented.insert((class, code));
        }
    }
    let produced: BTreeSet<_> = Code::ALL
        .iter()
        .map(|code| {
            let kind = serde_json::to_value(code.kind()).unwrap();
            let kind = match kind.as_str().unwrap() {
                "invalid" => "invalid",
                "unsupported" => "unsupported",
                "inapplicable" => "inapplicable",
                "untrusted" => "untrusted",
                other => panic!("unexpected class {other}"),
            };
            (kind, code.as_str())
        })
        .collect();
    assert_eq!(produced, documented);
}
