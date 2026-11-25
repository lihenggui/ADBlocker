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

//! Read and compile a complete generation of filter lists.

use std::collections::HashSet;
use std::path::Path;

use adblock::Engine;
use adblock::filters::network::NetworkFilter;
use adblock::lists::FilterSet;
use anyhow::Result;

use super::host_rules::{AppHostBlocks, HostProjections};
use super::html_rules::HtmlProjections;
use super::matcher::{MatcherList, MatcherLists};
use super::page_exceptions::PageProjections;
use super::response_rules::ResponseProjections;
use super::{
    AddressProjections, AppProjections, AppRules, Filters, ListFiles, Patterns, RequestRules,
    app_rules, redirects,
};
use crate::list_files::{MAX_LISTS, exceeds_budget, read_lists, readable_paths};
use crate::rule::driver::{Sink, drive};
use crate::rule::javascript::{ScriptTable, Scripts};
use crate::rule::{
    self, AppRule, Kind, Reason, ResponseRule, Rule, SourcePermissions, address, html, preprocess,
};

impl ListFiles {
    pub fn filters(&self, cache: Option<&Path>, debug: bool) -> Result<Filters> {
        crate::public_suffix::install();
        let cache = cache.filter(|_| !debug);
        if let Some(kept) = cache.and_then(|dir| Filters::read_cache(dir, self.generation)) {
            return Ok(kept);
        }
        let built = Filters::build(
            read_lists(&self.paths),
            Some(self.generation),
            debug,
            MAX_LISTS,
        )?;
        if let Some(dir) = cache {
            built.write_cache(dir, self.generation);
        }
        Ok(built)
    }
}

fn badfilters<'a>(lists: impl Iterator<Item = &'a str>) -> HashSet<u64> {
    lists
        .flat_map(preprocess::lines)
        .filter(|line| rule::badfilter_candidate(line))
        .filter_map(|line| {
            let source = rule::read(line);
            source
                .network()
                .filter(NetworkFilter::is_badfilter)
                .map(|rule| rule.get_id())
        })
        .collect()
}

pub(super) fn use_resources(requests: &mut RequestRules, scripts: &ScriptTable) {
    requests.full.use_resources(
        rule::scriptlet_resources()
            .iter()
            .cloned()
            .chain(scripts.resources()),
    );
    if let Some(engine) = &mut requests.redirects {
        redirects::use_resources(engine);
    }
}

fn compile_apps(
    apps: AppProjections<'_>,
    debug: bool,
    redirects: &mut redirects::RedirectProjections,
) -> (Option<AppRules>, AppHostBlocks) {
    let mut app_blocks = AppHostBlocks::new();
    let apps = apps.compile(
        debug,
        |host, apps| {
            app_blocks
                .entry(host.to_ascii_lowercase())
                .or_default()
                .push(format!("^${}{}", app_rules::SOURCE, apps));
        },
        Some(redirects),
    );
    (apps, app_blocks)
}

fn add_parameters(list: &mut String, source: &rule::Rule<'_>, disabled: &HashSet<u64>) {
    if source.line.contains("removeparam")
        && source.network().is_some_and(|rule| {
            rule.is_removeparam() && !rule.is_badfilter() && !disabled.contains(&rule.get_id())
        })
    {
        list.push_str(&source.normalized());
        list.push('\n');
    }
}

fn compile_requests(
    matcher: MatcherLists,
    redirects: redirects::RedirectProjections,
) -> RequestRules {
    let (full, originals) = matcher.compile();
    RequestRules {
        full,
        originals,
        redirects: redirects.compile(),
    }
}

/// Reserve bounded app patterns, address entries and JavaScript bodies with
/// exceptions first, without changing the order in which lists reach the matcher.
fn reserve_exceptions<'a>(
    lists: &'a [(&Path, String)],
    patterns: &mut Patterns,
    addresses: &mut AddressProjections,
    scripts: &mut Scripts<'a>,
) {
    for (path, text) in lists {
        let permissions = SourcePermissions::for_path(path);
        for line in preprocess::lines(text).filter(|line| rule::may_reserve_exception(line)) {
            let source =
                rule::read_with_scripts(line, permissions.scripts().then_some(&mut *scripts));
            match source.kind {
                Kind::Address(address) if address.exception => {
                    // The projection counts drops at the address limit.
                    let _ = addresses.add(&address);
                }
                Kind::App(app) if source.line.starts_with("@@") => {
                    // A rejected exception has no app patterns to reserve.
                    let _ = app_rules::validate(&app, patterns);
                }
                Kind::Response(response)
                    if source.line.starts_with("@@") && permissions.allows_response(&response) =>
                {
                    let _ = super::validate_response_rule(&response, patterns);
                }
                _ => {}
            }
        }
    }
}

impl Filters {
    /// Loads every `*.txt` list in `dirs`. With `debug` the engine keeps the
    /// text of each rule, to name the one that matched; it costs memory.
    pub fn load(dirs: &[&Path], debug: bool) -> Result<Self> {
        Self::open(dirs, None, debug)
    }

    /// As [`Filters::load`], but from what was kept in `cache` when these
    /// same lists were loaded before, and keeping it there otherwise.
    pub fn load_cached(dirs: &[&Path], cache: &Path) -> Result<Self> {
        Self::open(dirs, Some(cache), false)
    }

    fn open(dirs: &[&Path], cache: Option<&Path>, debug: bool) -> Result<Self> {
        let load = || ListFiles::read(dirs)?.filters(cache, debug);
        match load() {
            Ok(filters) => Ok(filters),
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(exceeds_budget) =>
            {
                Err(e)
            }
            Err(e) => {
                // No previous engine at startup: use the readable lists.
                // An incomplete generation must never enter the disk cache.
                eprintln!("lists: loading readable lists only: {e}");
                let paths = readable_paths(dirs);
                Self::build(
                    read_lists(&paths)
                        .filter(|entry| entry.as_ref().err().is_none_or(exceeds_budget)),
                    None,
                    debug,
                    MAX_LISTS,
                )
            }
        }
    }

    pub(crate) fn build<'a>(
        entries: impl Iterator<Item = std::io::Result<(&'a Path, String)>>,
        generation: Option<u64>,
        debug: bool,
        guard_budget: usize,
    ) -> Result<Self> {
        crate::public_suffix::install();
        let lists = entries.collect::<std::io::Result<Vec<_>>>()?;
        let disabled = badfilters(lists.iter().map(|(_, text)| text.as_str()));
        let mut compilation = Compilation {
            matcher: MatcherLists::new(debug),
            parameters: FilterSet::new(false),
            redirects: redirects::RedirectProjections::default(),
            hosts: HostProjections::new(),
            pages: PageProjections::default(),
            apps: AppProjections::default(),
            addresses: AddressProjections::default(),
            responses: ResponseProjections::default(),
            html: HtmlProjections::default(),
            patterns: Patterns::default(),
            scripts: Scripts::default(),
        };
        reserve_exceptions(
            &lists,
            &mut compilation.patterns,
            &mut compilation.addresses,
            &mut compilation.scripts,
        );
        let rules = lists
            .iter()
            .map(|(path, text)| {
                compilation.add_list(text, SourcePermissions::for_path(path), &disabled)
            })
            .sum();
        let texts = lists.iter().map(|(_, text)| text.as_str());
        Ok(compilation.finish(texts, generation, rules, debug, guard_budget))
    }
}

struct Compilation<'a> {
    matcher: MatcherLists,
    // Native blocks can suppress parameter removal, even when a replacement lets a request through.
    parameters: FilterSet,
    redirects: redirects::RedirectProjections,
    hosts: HostProjections<'a>,
    pages: PageProjections,
    apps: AppProjections<'a>,
    addresses: AddressProjections,
    responses: ResponseProjections<'a>,
    html: HtmlProjections<'a>,
    patterns: Patterns,
    scripts: Scripts<'a>,
}

impl<'a> Compilation<'a> {
    fn add_list(
        &mut self,
        text: &'a str,
        permissions: SourcePermissions,
        disabled: &HashSet<u64>,
    ) -> usize {
        let mut sink = CompileSink {
            build: self,
            permissions,
            disabled,
            normalized: MatcherList::with_capacity(text.len()),
            parameters: String::new(),
        };
        let count = drive(text, permissions, None, &mut sink);
        let CompileSink {
            normalized,
            parameters,
            ..
        } = sink;
        self.hosts.finish_list();
        self.matcher.finish_list(normalized, permissions.matcher());
        if !parameters.is_empty() {
            self.parameters
                .add_filter_list(parameters, permissions.matcher());
        }
        count
    }

    fn finish(
        self,
        texts: impl Iterator<Item = &'a str>,
        generation: Option<u64>,
        rules: usize,
        debug: bool,
        guard_budget: usize,
    ) -> Filters {
        let Self {
            matcher,
            parameters,
            mut redirects,
            hosts,
            pages,
            apps,
            addresses,
            responses,
            html,
            mut patterns,
            scripts,
        } = self;
        let (apps, app_blocks) = compile_apps(apps, debug, &mut redirects);
        let host_rules = hosts.finish(&app_blocks, guard_budget);
        let mut requests = compile_requests(matcher, redirects);
        let (scripts, responses) = (scripts.finish(), responses.compile(&mut patterns));
        use_resources(&mut requests, &scripts);
        patterns.report_dropped();
        Filters {
            generation,
            requests,
            parameters: Engine::new_with_filter_set(parameters),
            scripts,
            page_exceptions: pages.compile(texts),
            hosts: Engine::new_with_filter_set(host_rules.whole),
            host_guards: Engine::new_with_filter_set(host_rules.guards),
            page_host_guards: host_rules.page_guards.map(Engine::new_with_filter_set),
            app_host_guards: host_rules.app_guards.map(Engine::new_with_filter_set),
            app_page_host_guards: host_rules.app_page_guards.map(Engine::new_with_filter_set),
            guards_incomplete: host_rules.guards_incomplete,
            apps,
            patterns,
            addresses: addresses.finish(),
            responses,
            html: html.finish(),
            rules,
            host_rules: host_rules.count,
        }
    }
}

struct CompileSink<'a, 'build> {
    build: &'build mut Compilation<'a>,
    permissions: SourcePermissions,
    disabled: &'build HashSet<u64>,
    normalized: MatcherList,
    parameters: String,
}

impl CompileSink<'_, '_> {
    fn project_matcher(&mut self, source: &Rule<'_>) {
        self.build.matcher.add(&mut self.normalized, source);
        add_parameters(&mut self.parameters, source, self.disabled);
        self.build.redirects.add_web(source, self.disabled);
    }
}

impl<'a> Sink<'a> for CompileSink<'a, '_> {
    fn script_table(&mut self) -> Option<&mut Scripts<'a>> {
        Some(&mut self.build.scripts)
    }

    fn observe(&mut self, source: &Rule<'a>) {
        if !matches!(source.kind, Kind::Comment) {
            self.build.hosts.add(source, self.disabled);
        }
    }

    fn comment(&mut self, _: &Rule<'a>) {}

    fn matcher(&mut self, _: usize, source: &Rule<'a>) {
        self.project_matcher(source);
    }

    fn app(&mut self, _: usize, source: &Rule<'a>, app: &AppRule<'a>) {
        self.build.pages.add(source);
        self.build.apps.add(app, &mut self.build.patterns);
    }

    fn response(&mut self, _: usize, line: &'a str, rule: ResponseRule<'a>) {
        if self.permissions.allows_response(&rule) {
            self.build.responses.add(line, rule);
        }
    }

    fn page_exception(&mut self, _: usize, source: &Rule<'a>) {
        self.project_matcher(source);
        self.build.pages.add(source);
    }

    fn cosmetic(&mut self, _: usize, source: &Rule<'a>) {
        self.project_matcher(source);
    }

    fn html(&mut self, _: usize, line: &'a str, rule: html::Rule) {
        // Like hiding elements, cutting them is available to any source.
        self.build.html.add(line, rule);
    }

    fn address(&mut self, _: usize, _: &'a str, rule: address::AddressRule<'a>) {
        if !rule.exception {
            // Exceptions already had first use of the bounded table.
            let _ = self.build.addresses.add(&rule);
        }
    }

    fn script(&mut self, _: usize, source: &Rule<'a>) {
        self.project_matcher(source);
    }

    fn redirect(&mut self, _: usize, source: &Rule<'a>) {
        self.project_matcher(source);
    }

    fn not_applied(&mut self, _: usize, source: &Rule<'a>, _: &Reason) {
        self.project_matcher(source);
    }

    fn directive(&mut self, _: usize, _: &'a str, _: preprocess::DirectiveIssue) {}
}
