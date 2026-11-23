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
//! Network rules are applied. Element hiding is applied only while the
//! user has pages changed for it (`cosmetic.rs`), and scriptlets never:
//! the check names both, whatever the setting is at the moment.

use adblock::lists::{FilterParseError, ParseOptions, ParsedLine, parse_filter};

use crate::protocol_types::{Issue, IssueKind, RulesCheck};
pub use crate::rule_text::{count, is_comment};

/// The rules of `text` that will not be applied, by line.
pub fn check(text: &str) -> RulesCheck {
    crate::public_suffix::install();
    let mut rules = 0;
    let mut issues = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if is_comment(line) {
            continue;
        }
        rules += 1;
        let kind = match parse_filter(line, false, ParseOptions::default()) {
            Ok(ParsedLine::Network(_)) => continue,
            Ok(ParsedLine::Cosmetic(_)) | Err(FilterParseError::Cosmetic(_)) => IssueKind::Cosmetic,
            // What adblock-rust sets aside as AdGuard's HTML filtering.
            Err(FilterParseError::Unsupported) if line.contains("$$") => IssueKind::Cosmetic,
            Err(_) => IssueKind::Invalid,
        };
        issues.push(Issue {
            line: n + 1,
            text: line.trim().to_owned(),
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
                    0.0.0.0 tracker.example\n";
        assert_eq!(
            kinds(text),
            [
                (4, IssueKind::Cosmetic),
                (5, IssueKind::Cosmetic),
                (6, IssueKind::Cosmetic),
                (7, IssueKind::Cosmetic),
                (8, IssueKind::Invalid),
                (9, IssueKind::Invalid),
            ]
        );
        assert_eq!(check(text).rules, 8);
    }

    #[test]
    fn the_header_and_comments_of_a_list_are_not_rules() {
        assert_eq!(
            count("[Adblock Plus 2.0]\n! Title: X\n# hosts\n\n||a.example^\n#@#.ad\n"),
            2
        );
    }
}
