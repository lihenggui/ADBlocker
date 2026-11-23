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

//! Compiles the page-level exceptions used during cosmetic filtering.

use adblock::Engine;
use adblock::filters::cosmetic::CosmeticFilterMask;
use adblock::lists::{FilterSet, ParseOptions, ParsedLine, RuleTypes, parse_filter};
use adblock::request::Request;

use super::{is_network_exception, network};

/// adblock-rust handles generic-hide and cosmetic exceptions itself. It
/// treats document exceptions as network exceptions and does not parse
/// element-hide or specific-hide, so compile those as page matchers here.
#[derive(Default)]
pub(super) struct PageExceptions {
    pub(super) all: Option<Engine>,
    pub(super) element: Option<Engine>,
    pub(super) specific: Option<(Engine, Engine)>,
}

impl PageExceptions {
    pub(super) fn build<'a>(lists: impl Iterator<Item = &'a str> + Clone) -> Self {
        let mut all = String::new();
        let mut element = String::new();
        let mut specific = String::new();
        for line in lists.clone().flat_map(str::lines).map(str::trim) {
            let Some((rule, options)) = line.rsplit_once('$') else {
                continue;
            };
            if !rule.starts_with("@@") {
                continue;
            }
            let options = options.split(',');
            let any = |names: &[&str]| options.clone().any(|option| names.contains(&option));
            let target = if any(&["document", "doc"]) {
                &mut all
            } else if any(&["elemhide", "ehide"])
                || (any(&["specifichide", "shide"]) && any(&["generichide", "ghide"]))
            {
                &mut element
            } else if any(&["specifichide", "shide"]) {
                &mut specific
            } else {
                continue;
            };
            let mut projected = format!("{rule}$document");
            for option in options {
                if !matches!(
                    option,
                    "document"
                        | "doc"
                        | "elemhide"
                        | "ehide"
                        | "specifichide"
                        | "shide"
                        | "generichide"
                        | "ghide"
                ) {
                    projected.push(',');
                    projected.push_str(option);
                }
            }
            if network(&projected)
                .is_some_and(|rule| rule.is_badfilter() || is_network_exception(&rule))
            {
                target.push_str(&projected);
                target.push('\n');
            }
        }
        let specific = (!specific.is_empty()).then(|| {
            let options = ParseOptions {
                rule_types: RuleTypes::CosmeticOnly,
                ..ParseOptions::default()
            };
            let generic = generic_rules(lists, options);
            (
                compile(specific, ParseOptions::default()),
                compile(generic, options),
            )
        });
        Self {
            all: (!all.is_empty()).then(|| compile(all, ParseOptions::default())),
            element: (!element.is_empty()).then(|| compile(element, ParseOptions::default())),
            specific,
        }
    }

    pub(super) fn matches(engine: Option<&Engine>, page: &Request) -> bool {
        engine.is_some_and(|engine| {
            engine
                .check_network_request_subset(page, false, true)
                .exception
                .is_some()
        })
    }
}

fn compile(text: String, options: ParseOptions) -> Engine {
    let mut set = FilterSet::new(false);
    set.add_filter_list(text, options);
    Engine::new_with_filter_set(set)
}

fn generic_rules<'a>(lists: impl Iterator<Item = &'a str>, options: ParseOptions) -> String {
    let mut generic = String::new();
    for line in lists.flat_map(str::lines).map(str::trim) {
        if let Ok(ParsedLine::Cosmetic(rule)) = parse_filter(line, false, options)
            && (rule.mask.contains(CosmeticFilterMask::UNHIDE)
                || !rule.has_hostname_constraint()
                || rule.hidden_generic_rule().is_some())
        {
            generic.push_str(line);
            generic.push('\n');
        }
    }
    generic
}
