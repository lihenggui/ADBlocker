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

//! Compiles the page-level exceptions for cosmetic filtering and response rules.

use adblock::Engine;
use adblock::filters::cosmetic::CosmeticFilterMask;
use adblock::lists::{FilterSet, ParseOptions, ParsedLine, RuleTypes, parse_filter};
use adblock::request::Request;

use super::{Page, RequestRules, is_network_exception, network, response_rules};

/// adblock-rust handles generic-hide and cosmetic exceptions itself. It
/// treats document exceptions as network exceptions and does not parse
/// element-hide or specific-hide, so compile those as page matchers here.
#[derive(Default)]
pub(super) struct PageExceptions {
    pub(super) all: Option<Engine>,
    pub(super) content: Option<Engine>,
    pub(super) urlblock: Option<Engine>,
    pub(super) element: Option<Engine>,
    pub(super) specific: Option<(Engine, Engine)>,
}

#[derive(Default)]
pub(super) struct ResponseExceptions {
    pub(super) all: bool,
    pub(super) prune: bool,
}

impl PageExceptions {
    pub(super) fn build<'a>(lists: impl Iterator<Item = &'a str> + Clone) -> Self {
        let mut all = String::new();
        let mut content = String::new();
        let mut urlblock = String::new();
        let mut element = String::new();
        let mut specific = String::new();
        for line in lists.clone().flat_map(str::lines).map(str::trim) {
            let Some((rule, options)) = line.rsplit_once('$') else {
                continue;
            };
            if !rule.starts_with("@@") || response_rules::is_response_rule(line) {
                continue;
            }
            let options = options.split(',');
            let any = |names: &[&str]| options.clone().any(|option| names.contains(&option));
            let also_content = any(&["content"]) && !any(&["document", "doc"]);
            let also_urlblock = any(&["urlblock"]);
            let target = if any(&["document", "doc"]) {
                Some(&mut all)
            } else if any(&["elemhide", "ehide"])
                || (any(&["specifichide", "shide"]) && any(&["generichide", "ghide"]))
            {
                Some(&mut element)
            } else if any(&["specifichide", "shide"]) {
                Some(&mut specific)
            } else {
                None
            };
            if target.is_none() && !also_content && !also_urlblock {
                continue;
            }
            let projected = document_exception(rule, options);
            if network(&projected)
                .is_some_and(|rule| rule.is_badfilter() || is_network_exception(&rule))
            {
                if let Some(target) = target {
                    target.push_str(&projected);
                    target.push('\n');
                }
                if also_urlblock {
                    urlblock.push_str(&projected);
                    urlblock.push('\n');
                }
                if also_content {
                    content.push_str(&projected);
                    content.push('\n');
                }
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
            content: (!content.is_empty()).then(|| compile(content, ParseOptions::default())),
            urlblock: (!urlblock.is_empty()).then(|| compile(urlblock, ParseOptions::default())),
            element: (!element.is_empty()).then(|| compile(element, ParseOptions::default())),
            specific,
        }
    }

    pub(super) fn response(&self, url: &str, page: Page<'_>, method: &str) -> ResponseExceptions {
        if self.all.is_none() && self.content.is_none() && self.urlblock.is_none() {
            return ResponseExceptions::default();
        }
        let (url, method) = match page {
            Page::Named(page) => (page.url.as_str(), "GET"),
            Page::Itself => (url, method),
            Page::Unnamed { .. } => return ResponseExceptions::default(),
        };
        RequestRules::request_with_page(url, Page::Itself, "document", method).map_or_else(
            ResponseExceptions::default,
            |page| ResponseExceptions {
                all: Self::matches(self.all.as_ref(), &page)
                    || Self::matches(self.content.as_ref(), &page),
                prune: Self::matches(self.urlblock.as_ref(), &page),
            },
        )
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

/// Match page-specific modifiers through the same network options as a document exception.
fn document_exception<'a>(rule: &str, options: impl Iterator<Item = &'a str>) -> String {
    let mut projected = format!("{rule}$document");
    for option in options {
        if !matches!(
            option,
            "document"
                | "doc"
                | "content"
                | "urlblock"
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
    projected
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
