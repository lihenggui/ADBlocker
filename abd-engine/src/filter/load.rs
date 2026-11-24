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
use adblock::lists::{FilterSet, ParseOptions};
use adblock::resources::{PermissionMask, Resource};
use anyhow::Result;

use super::host_rules::{AppHostBlocks, HostProjections};
use super::page_exceptions::PageProjections;
use super::response_rules::ResponseProjections;
use super::{AppProjections, Filters, ListFiles, Patterns, RequestRules, app_rules};
use crate::list_files::{MAX_LISTS, exceeds_budget, read_lists, readable_paths};
use crate::list_url::{Trust, trusted_list_file};
use crate::rule::javascript::{self, ScriptTable, Scripts};
use crate::rule::{self, Kind, Rule};

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
        .flat_map(str::lines)
        .filter(|line| rule::badfilter_candidate(line))
        .filter_map(|line| rule::read(line).network())
        .filter(NetworkFilter::is_badfilter)
        .map(|rule| rule.get_id())
        .collect()
}

pub(crate) struct MatcherLists(FilterSet);

impl MatcherLists {
    pub(crate) fn new(debug: bool) -> Self {
        Self(FilterSet::new(debug))
    }

    pub(crate) fn add(list: &mut String, source: &Rule<'_>) {
        let Some(line) = source.matcher_input() else {
            return;
        };
        // Reject unsafe calls before their helpers can consume the page's
        // budget. The raw text still includes controls adblock may discard.
        if source.is_scriptlet() && !crate::cosmetic::safe_raw_text(&line) {
            return;
        }
        list.push_str(&line);
        list.push('\n');
    }

    pub(crate) fn finish_list(&mut self, list: String, options: ParseOptions) {
        self.0.add_filter_list(list, options);
    }

    pub(crate) fn compile(self) -> RequestRules {
        RequestRules {
            full: Engine::new_with_filter_set(self.0),
        }
    }
}

pub(super) fn use_resources(engine: &mut Engine, scripts: &ScriptTable) {
    static RESOURCES: std::sync::LazyLock<Vec<Resource>> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../resources/scriptlets.json"))
            .expect("bundled scriptlet resources are valid")
    });
    engine.use_resources(RESOURCES.iter().cloned().chain(scripts.resources()));
}

fn user_rules(path: &Path) -> bool {
    path.parent().and_then(Path::file_name) == Some("rules".as_ref())
}

fn source_permissions(path: &Path) -> (bool, PermissionMask) {
    let scripts = user_rules(path) || trusted_list_file(path, Trust::Scripts);
    let mut permissions =
        PermissionMask::from_bits(u8::from(trusted_list_file(path, Trust::Scriptlets)));
    if scripts {
        permissions |= javascript::PERMISSION;
    }
    (scripts, permissions)
}

impl Filters {
    /// Loads every `*.txt` list in `dirs`. With `debug` the engine keeps the
    /// text of each rule, to name the one that matched; it costs memory.
    pub fn load(dirs: &[&Path], debug: bool) -> Result<Self> {
        Self::open(dirs, None, debug)
    }

    /// As [`Filters::load`], but from what was kept in `cache` when these
    /// same lists were loaded before, and keeping it there otherwise.
    ///
    /// Reading the lists is a tenth of a second of one core at full work
    /// and tens of megabytes passed through memory, at every start and
    /// every reload, for lists that change once a day at most.
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

    pub(super) fn build<'a>(
        entries: impl Iterator<Item = std::io::Result<(&'a Path, String)>>,
        generation: Option<u64>,
        debug: bool,
        guard_budget: usize,
    ) -> Result<Self> {
        crate::public_suffix::install();
        let lists = entries.collect::<std::io::Result<Vec<_>>>()?;
        let rules_count = lists.iter().map(|(_, text)| text.lines().count()).sum();
        let texts = || lists.iter().map(|(_, text)| text.as_str());
        let disabled = badfilters(texts());
        let mut matcher = MatcherLists::new(debug);
        let mut hosts = HostProjections::new();
        let mut pages = PageProjections::default();
        let mut apps = AppProjections::default();
        let mut responses = ResponseProjections::default();
        let mut patterns = Patterns::default();
        let mut scripts = Scripts::default();
        for (path, text) in &lists {
            let trust_responses = user_rules(path) || trusted_list_file(path, Trust::ResponseRules);
            let (trust_scripts, permissions) = source_permissions(path);
            let mut normalized = String::with_capacity(text.len());
            for line in text.lines() {
                let source = rule::read_with_scripts(line, trust_scripts.then_some(&mut scripts));
                MatcherLists::add(&mut normalized, &source);
                hosts.add(&source, &disabled);
                pages.add(&source);
                match source.kind {
                    Kind::App(app) => apps.add(&app, &mut patterns),
                    Kind::Response(response) if trust_responses => {
                        responses.add(source.line, response)
                    }
                    _ => (),
                }
            }
            hosts.finish_list();
            matcher.finish_list(
                normalized,
                ParseOptions {
                    permissions,
                    ..ParseOptions::default()
                },
            );
        }
        let mut app_blocks = AppHostBlocks::new();
        let apps = apps.compile(debug, |host, apps| {
            app_blocks
                .entry(host.to_ascii_lowercase())
                .or_default()
                .push(format!("^${}{}", app_rules::SOURCE, apps));
        });
        let host_rules = hosts.finish(&app_blocks, guard_budget);
        let mut requests = matcher.compile();
        let scripts = scripts.finish();
        use_resources(&mut requests.full, &scripts);
        let responses = responses.compile(&mut patterns);
        patterns.report_dropped();
        Ok(Self {
            generation,
            requests,
            scripts,
            page_exceptions: pages.compile(texts()),
            hosts: Engine::new_with_filter_set(host_rules.whole),
            host_guards: Engine::new_with_filter_set(host_rules.guards),
            page_host_guards: host_rules.page_guards.map(Engine::new_with_filter_set),
            app_host_guards: host_rules.app_guards.map(Engine::new_with_filter_set),
            app_page_host_guards: host_rules.app_page_guards.map(Engine::new_with_filter_set),
            guards_incomplete: host_rules.guards_incomplete,
            apps,
            patterns,
            responses,
            rules: rules_count,
            host_rules: host_rules.count,
        })
    }
}
