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
use adblock::lists::{FilterSet, ParseOptions, RuleTypes};
use adblock::request::Request;

use super::{Page, RequestRules, is_network_exception};
use crate::rule::action::{PageExceptions as ResponseExceptions, html};
use crate::rule::{self, Elements, PageException, Rule};

/// adblock-rust handles generic-hide and cosmetic exceptions itself. It
/// treats document exceptions as network exceptions and does not parse
/// element-hide or specific-hide, so compile those as page matchers here.
#[derive(Default)]
pub(super) struct PageExceptions {
    pub(super) all: Option<Engine>,
    pub(super) content: Option<Engine>,
    pub(super) urlblock: Option<Engine>,
    pub(super) element: Option<Engine>,
    pub(super) script: Option<Engine>,
    pub(super) specific: Option<(Engine, Engine)>,
}

#[derive(Default)]
pub(super) struct PageProjections {
    all: String,
    content: String,
    urlblock: String,
    element: String,
    specific: String,
    script: String,
}

fn projection(source: &Rule<'_>, page: &PageException<'_>) -> Option<String> {
    let projected = page.project(source.literal_options());
    let source = rule::read(&projected);
    if !source
        .network()
        .is_some_and(|rule| rule.is_badfilter() || is_network_exception(&rule))
    {
        return None;
    }
    if source.denyallow {
        Some(source.normalized().into_owned())
    } else {
        Some(projected)
    }
}

pub(crate) fn valid_script_exception(source: &Rule<'_>) -> bool {
    source
        .page_exception()
        .is_some_and(|page| page.script && projection(source, page).is_some())
}

impl PageProjections {
    pub(super) fn add(&mut self, source: &rule::Rule<'_>) {
        let Some(page) = source.page_exception() else {
            return;
        };
        let Some(projected) = projection(source, page) else {
            return;
        };
        let target = if page.document {
            Some(&mut self.all)
        } else {
            page.elements.map(|elements| match elements {
                Elements::All => &mut self.element,
                Elements::Specific => &mut self.specific,
            })
        };
        for target in target
            .into_iter()
            .chain(page.urlblock.then_some(&mut self.urlblock))
            .chain(page.content.then_some(&mut self.content))
            .chain(page.script.then_some(&mut self.script))
        {
            target.push_str(&projected);
            target.push('\n');
        }
    }

    pub(super) fn compile<'a>(self, lists: impl Iterator<Item = &'a str>) -> PageExceptions {
        let Self {
            all,
            content,
            urlblock,
            element,
            specific,
            script,
        } = self;
        let specific = (!specific.is_empty()).then(|| {
            let options = ParseOptions {
                rule_types: RuleTypes::CosmeticOnly,
                ..ParseOptions::default()
            };
            let generic = generic_rules(lists);
            (
                compile(specific, ParseOptions::default()),
                compile(generic, options),
            )
        });
        PageExceptions {
            all: (!all.is_empty()).then(|| compile(all, ParseOptions::default())),
            content: (!content.is_empty()).then(|| compile(content, ParseOptions::default())),
            urlblock: (!urlblock.is_empty()).then(|| compile(urlblock, ParseOptions::default())),
            element: (!element.is_empty()).then(|| compile(element, ParseOptions::default())),
            script: (!script.is_empty()).then(|| compile(script, ParseOptions::default())),
            specific,
        }
    }
}

impl PageExceptions {
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
                document: Self::matches(self.all.as_ref(), &page),
                content: Self::matches(self.content.as_ref(), &page),
                urlblock: Self::matches(self.urlblock.as_ref(), &page),
            },
        )
    }

    pub(super) fn url_rules_disabled(&self, url: &str, page: Page<'_>, method: &str) -> bool {
        let disables = |page| {
            let exceptions = self.response(url, page, method);
            exceptions.document || exceptions.urlblock
        };
        disables(Page::Itself) || matches!(page, Page::Named(_)) && disables(page)
    }

    pub(super) fn skip_html(&self, url: &str, method: &str) -> bool {
        html::disabled_by(self.response(url, Page::Itself, method))
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

fn generic_rules<'a>(lists: impl Iterator<Item = &'a str>) -> String {
    let mut generic = String::new();
    for line in lists
        .flat_map(rule::preprocess::lines)
        .map(str::trim)
        .filter(|line| rule::generic_cosmetic(line))
    {
        generic.push_str(line);
        generic.push('\n');
    }
    generic
}
