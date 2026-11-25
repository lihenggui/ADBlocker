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

//! The compiled engines kept for one generation of the source lists.

use std::path::Path;

use adblock::Engine;

use super::html_rules::HtmlRules;
use super::response_rules::ResponseRules;
use super::{AppRules, Filters, PageExceptions, Patterns, RequestRules, load::use_resources};
use crate::files;
use crate::list_files::MAX_COMPILED;
use crate::rule::javascript::{MAX_CACHED_SCRIPTS, ScriptTable};

/// The files of the rules for apps, in the order they are kept in.
const APPS: [&str; 4] = ["app-blocks", "app-exceptions", "app-hosts", "app-guards"];

fn read_apps(cache: &Path, remaining: &mut usize) -> Option<AppRules> {
    let [blocks, exceptions, hosts, guards] = APPS.map(|name| {
        let kept = files::read_bytes(&cache.join(name), *remaining).ok()?;
        *remaining -= kept.len();
        Some(kept)
    });
    AppRules::compiled([&blocks?, &exceptions?, &hosts?, &guards?])
}

fn optional_engine(present: bool, read: impl FnOnce() -> Option<Engine>) -> Option<Option<Engine>> {
    if present {
        Some(Some(read()?))
    } else {
        Some(None)
    }
}

fn read_engine(cache: &Path, name: &str, remaining: &mut usize) -> Option<Engine> {
    let compiled = files::read_bytes(&cache.join(name), *remaining).ok()?;
    *remaining -= compiled.len();
    let mut engine = Engine::default();
    // The engine's checksum rejects a truncated or damaged file.
    engine.deserialize(&compiled).ok()?;
    Some(engine)
}

fn read_html(cache: &Path, present: bool, remaining: &mut usize) -> Option<HtmlRules> {
    if !present {
        return Some(HtmlRules::default());
    }
    let specs = files::read_bytes(&cache.join("html-rules"), *remaining).ok()?;
    *remaining -= specs.len();
    HtmlRules::cached(&specs)
}

fn read_requests(
    mut read: impl FnMut(&str) -> Option<Engine>,
    has_redirects: bool,
    scripts: &ScriptTable,
) -> Option<RequestRules> {
    let mut requests = RequestRules {
        full: read("full")?,
        redirects: optional_engine(has_redirects, || read("redirects"))?,
        originals: Default::default(),
    };
    use_resources(&mut requests, scripts);
    Some(requests)
}

impl Filters {
    /// The engines kept for the lists that `key` stands for, if they are.
    pub(super) fn read_cache(cache: &Path, key: u64) -> Option<Self> {
        crate::public_suffix::install();
        let key_file = cache.join("key");
        let _lock = files::lock(&key_file).ok()?;
        let meta = files::read_text(&key_file, 160).ok()?;
        let mut f = meta.split_ascii_whitespace();
        if f.next()?.parse::<u64>().ok()? != key {
            return None;
        }
        let (rules, host_rules) = (f.next()?.parse().ok()?, f.next()?.parse().ok()?);
        let guards_incomplete = f.next()?.parse().ok()?;
        let has_page_guards: bool = f.next()?.parse().ok()?;
        let has_app_guards: bool = f.next()?.parse().ok()?;
        let has_app_page_guards: bool = f.next()?.parse().ok()?;
        let has_no_cosmetic: bool = f.next()?.parse().ok()?;
        let has_no_elements: bool = f.next()?.parse().ok()?;
        let has_specific: bool = f.next()?.parse().ok()?;
        let has_apps: bool = f.next()?.parse().ok()?;
        let has_no_content: bool = f.next()?.parse().ok()?;
        let has_responses: bool = f.next()?.parse().ok()?;
        let has_urlblock: bool = f.next()?.parse().ok()?;
        let has_no_scripts: bool = f.next()?.parse().ok()?;
        let mut remaining = MAX_COMPILED;
        let patterns = files::read_bytes(&cache.join("app-patterns"), remaining).ok()?;
        remaining -= patterns.len();
        let patterns = Patterns::cached(&patterns)?;
        let actions = if has_responses {
            let kept = files::read_bytes(&cache.join("response-actions"), remaining).ok()?;
            remaining -= kept.len();
            Some(kept)
        } else {
            None
        };
        let apps = if has_apps {
            Some(read_apps(cache, &mut remaining)?)
        } else {
            None
        };
        let script_data =
            files::read_bytes(&cache.join("scripts"), remaining.min(MAX_CACHED_SCRIPTS)).ok()?;
        remaining -= script_data.len();
        let scripts = ScriptTable::cached(&script_data)?;
        let html = read_html(cache, f.next()?.parse().ok()?, &mut remaining)?;
        let has_redirects = f.next()?.parse().ok()?;
        let mut engine = |name: &str| read_engine(cache, name, &mut remaining);
        let responses = match actions {
            Some(actions) => Some(ResponseRules::cached(engine("response-rules")?, &actions)?),
            None => None,
        };
        let requests = read_requests(&mut engine, has_redirects, &scripts)?;
        Some(Self {
            generation: Some(key),
            requests,
            scripts,
            page_exceptions: PageExceptions {
                all: optional_engine(has_no_cosmetic, || engine("no-cosmetic"))?,
                content: optional_engine(has_no_content, || engine("no-content"))?,
                urlblock: optional_engine(has_urlblock, || engine("no-urlblock"))?,
                script: optional_engine(has_no_scripts, || engine("no-scripts"))?,
                element: optional_engine(has_no_elements, || engine("no-element-hiding"))?,
                specific: if has_specific {
                    Some((engine("no-specific-cosmetic")?, engine("generic-css")?))
                } else {
                    None
                },
            },
            hosts: engine("hosts")?,
            host_guards: engine("host-guards")?,
            page_host_guards: optional_engine(has_page_guards, || engine("page-host-guards"))?,
            app_host_guards: optional_engine(has_app_guards, || engine("app-host-guards"))?,
            app_page_host_guards: optional_engine(has_app_page_guards, || {
                engine("app-page-host-guards")
            })?,
            guards_incomplete,
            apps,
            patterns,
            responses,
            html,
            rules,
            host_rules,
        })
    }

    fn compiled_files(&self) -> impl Iterator<Item = (&'static str, Option<Vec<u8>>)> {
        let exceptions = &self.page_exceptions;
        let specific = exceptions.specific.as_ref();
        let kept = self.apps.as_ref().map(|rules| rules.serialized().map(Some));
        APPS.into_iter().zip(kept.unwrap_or_default()).chain([
            ("app-patterns", Some(self.patterns.serialized())),
            ("html-rules", self.html.serialized()),
            ("full", Some(self.requests.full.serialize())),
            (
                "redirects",
                self.requests.redirects.as_ref().map(Engine::serialize),
            ),
            ("scripts", Some(self.scripts.serialized())),
            (
                "no-scripts",
                exceptions.script.as_ref().map(Engine::serialize),
            ),
            (
                "no-content",
                exceptions.content.as_ref().map(Engine::serialize),
            ),
            (
                "no-urlblock",
                exceptions.urlblock.as_ref().map(Engine::serialize),
            ),
            (
                "response-rules",
                self.responses.as_ref().map(|r| r.engine.serialize()),
            ),
            (
                "response-actions",
                self.responses.as_ref().and_then(ResponseRules::serialized),
            ),
            // Kept by versions that judged a request without a page apart.
            ("unknown", None),
            ("hosts", Some(self.hosts.serialize())),
            ("host-guards", Some(self.host_guards.serialize())),
            (
                "page-host-guards",
                self.page_host_guards.as_ref().map(Engine::serialize),
            ),
            (
                "app-host-guards",
                self.app_host_guards.as_ref().map(Engine::serialize),
            ),
            (
                "app-page-host-guards",
                self.app_page_host_guards.as_ref().map(Engine::serialize),
            ),
            (
                "no-cosmetic",
                exceptions.all.as_ref().map(Engine::serialize),
            ),
            (
                "no-element-hiding",
                exceptions.element.as_ref().map(Engine::serialize),
            ),
            (
                "no-specific-cosmetic",
                specific.map(|(engine, _)| engine.serialize()),
            ),
            (
                "generic-css",
                specific.map(|(_, engine)| engine.serialize()),
            ),
        ])
    }

    /// Keeps the engines for next time. Not being able to is no error.
    pub(super) fn write_cache(&self, cache: &Path, key: u64) {
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(cache)?;
            let exceptions = &self.page_exceptions;
            let compiled: Vec<_> = self.compiled_files().collect();
            let total: usize = compiled
                .iter()
                .filter_map(|(_, data)| data.as_ref())
                .map(Vec::len)
                .sum();
            if total > MAX_COMPILED {
                return Ok(());
            }
            let _lock = files::lock(&cache.join("key"))?;
            // The key last, and removed first: it says the compiled files
            // are whole and belong together.
            files::remove_if_present(&cache.join("key"))?;
            for (name, data) in compiled {
                if let Some(data) = data {
                    std::fs::write(cache.join(name), data)?;
                } else {
                    files::remove_if_present(&cache.join(name))?;
                }
            }
            let meta = format!(
                "{key} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}\n",
                self.rules,
                self.host_rules,
                self.guards_incomplete,
                self.page_host_guards.is_some(),
                self.app_host_guards.is_some(),
                self.app_page_host_guards.is_some(),
                exceptions.all.is_some(),
                exceptions.element.is_some(),
                exceptions.specific.is_some(),
                self.apps.is_some(),
                exceptions.content.is_some(),
                self.responses.is_some(),
                exceptions.urlblock.is_some(),
                exceptions.script.is_some(),
                !self.html.is_empty(),
                self.requests.redirects.is_some(),
            );
            files::replace(&cache.join("key"), meta.as_bytes())
        };
        let _ = write();
    }
}
