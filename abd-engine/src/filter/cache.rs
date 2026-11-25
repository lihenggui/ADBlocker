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

use super::address_rules::AddressRules;
use super::html_rules::HtmlRules;
use super::response_rules::ResponseRules;
use super::{AppRules, Filters, PageExceptions, Patterns, RequestRules, load::use_resources};
use crate::files;
use crate::list_files::MAX_COMPILED;
use crate::rule::javascript::{MAX_CACHED_SCRIPTS, ScriptTable};

/// The files of the rules for apps, in the order they are kept in.
const APPS: [&str; 5] = [
    "app-blocks",
    "app-exceptions",
    "app-hosts",
    "app-guards",
    "app-remove-params",
];

// Declare the positional wire order once. Construction by field name keeps
// changes in the filter tables independent of this cache format.
macro_rules! cache_key {
    ($first:ident: $first_type:ty, $($field:ident: $type:ty),+ $(,)?) => {
        struct CacheKey {
            $first: $first_type,
            $($field: $type),+
        }

        impl CacheKey {
            fn read(line: &str) -> Option<Self> {
                let mut fields = line.split_ascii_whitespace();
                Some(Self {
                    $first: fields.next()?.parse().ok()?,
                    $($field: fields.next()?.parse().ok()?),+
                })
            }

            fn write(&self) -> String {
                format!(
                    concat!("{", stringify!($first), "}", $(" {", stringify!($field), "}",)+ "\n"),
                    $first = self.$first,
                    $($field = self.$field),+
                )
            }
        }
    };
}

cache_key! {
    generation: u64,
    rules: usize,
    host_rules: usize,
    guards_incomplete: bool,
    has_page_guards: bool,
    has_app_guards: bool,
    has_app_page_guards: bool,
    has_no_cosmetic: bool,
    has_no_elements: bool,
    has_specific: bool,
    has_apps: bool,
    has_no_content: bool,
    has_responses: bool,
    has_urlblock: bool,
    has_no_scripts: bool,
    has_html: bool,
    has_redirects: bool,
}

fn read_part(cache: &Path, name: &str, remaining: &mut usize) -> Option<Vec<u8>> {
    let bytes = files::read_bytes(&cache.join(name), *remaining).ok()?;
    *remaining -= bytes.len();
    Some(bytes)
}

fn read_apps(cache: &Path, remaining: &mut usize) -> Option<AppRules> {
    let [blocks, exceptions, hosts, guards, parameters] =
        APPS.map(|name| read_part(cache, name, remaining));
    AppRules::compiled([&blocks?, &exceptions?, &hosts?, &guards?, &parameters?])
}

fn optional_engine(present: bool, read: impl FnOnce() -> Option<Engine>) -> Option<Option<Engine>> {
    if present {
        Some(Some(read()?))
    } else {
        Some(None)
    }
}

fn read_engine(cache: &Path, name: &str, remaining: &mut usize) -> Option<Engine> {
    let compiled = read_part(cache, name, remaining)?;
    let mut engine = Engine::default();
    // The engine's checksum rejects a truncated or damaged file.
    engine.deserialize(&compiled).ok()?;
    Some(engine)
}

fn read_html(cache: &Path, present: bool, remaining: &mut usize) -> Option<HtmlRules> {
    if !present {
        return Some(HtmlRules::default());
    }
    let specs = read_part(cache, "html-rules", remaining)?;
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

fn read_page_exceptions(
    meta: &CacheKey,
    mut engine: impl FnMut(&str) -> Option<Engine>,
) -> Option<PageExceptions> {
    Some(PageExceptions {
        all: optional_engine(meta.has_no_cosmetic, || engine("no-cosmetic"))?,
        content: optional_engine(meta.has_no_content, || engine("no-content"))?,
        urlblock: optional_engine(meta.has_urlblock, || engine("no-urlblock"))?,
        script: optional_engine(meta.has_no_scripts, || engine("no-scripts"))?,
        element: optional_engine(meta.has_no_elements, || engine("no-element-hiding"))?,
        specific: if meta.has_specific {
            Some((engine("no-specific-cosmetic")?, engine("generic-css")?))
        } else {
            None
        },
    })
}

impl Filters {
    /// The engines kept for the lists that `key` stands for, if they are.
    pub(super) fn read_cache(cache: &Path, key: u64) -> Option<Self> {
        crate::public_suffix::install();
        let key_file = cache.join("key");
        let _lock = files::lock(&key_file).ok()?;
        let meta = CacheKey::read(&files::read_text(&key_file, 160).ok()?)?;
        if meta.generation != key {
            return None;
        }
        let mut remaining = MAX_COMPILED;
        let patterns = Patterns::cached(&read_part(cache, "app-patterns", &mut remaining)?)?;
        let addresses = AddressRules::cached(&read_part(cache, "address-rules", &mut remaining)?)?;
        let actions = if meta.has_responses {
            Some(read_part(cache, "response-actions", &mut remaining)?)
        } else {
            None
        };
        let apps = if meta.has_apps {
            Some(read_apps(cache, &mut remaining)?)
        } else {
            None
        };
        let script_data =
            files::read_bytes(&cache.join("scripts"), remaining.min(MAX_CACHED_SCRIPTS)).ok()?;
        remaining -= script_data.len();
        let scripts = ScriptTable::cached(&script_data)?;
        let html = read_html(cache, meta.has_html, &mut remaining)?;
        let mut engine = |name: &str| read_engine(cache, name, &mut remaining);
        let responses = match actions {
            Some(actions) => Some(ResponseRules::cached(engine("response-rules")?, &actions)?),
            None => None,
        };
        let requests = read_requests(&mut engine, meta.has_redirects, &scripts)?;
        Some(Self {
            generation: Some(key),
            requests,
            parameters: engine("remove-params")?,
            scripts,
            page_exceptions: read_page_exceptions(&meta, &mut engine)?,
            hosts: engine("hosts")?,
            host_guards: engine("host-guards")?,
            page_host_guards: optional_engine(meta.has_page_guards, || engine("page-host-guards"))?,
            app_host_guards: optional_engine(meta.has_app_guards, || engine("app-host-guards"))?,
            app_page_host_guards: optional_engine(meta.has_app_page_guards, || {
                engine("app-page-host-guards")
            })?,
            guards_incomplete: meta.guards_incomplete,
            apps,
            patterns,
            addresses,
            responses,
            html,
            rules: meta.rules,
            host_rules: meta.host_rules,
        })
    }

    fn compiled_files(&self) -> impl Iterator<Item = (&'static str, Option<Vec<u8>>)> {
        let exceptions = &self.page_exceptions;
        let specific = exceptions.specific.as_ref();
        let kept = self.apps.as_ref().map(|rules| rules.serialized().map(Some));
        APPS.into_iter().zip(kept.unwrap_or_default()).chain([
            ("app-patterns", Some(self.patterns.serialized())),
            ("address-rules", Some(self.addresses.serialized())),
            ("html-rules", self.html.serialized()),
            ("full", Some(self.requests.full.serialize())),
            ("remove-params", Some(self.parameters.serialize())),
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
            let meta = CacheKey {
                generation: key,
                rules: self.rules,
                host_rules: self.host_rules,
                guards_incomplete: self.guards_incomplete,
                has_page_guards: self.page_host_guards.is_some(),
                has_app_guards: self.app_host_guards.is_some(),
                has_app_page_guards: self.app_page_host_guards.is_some(),
                has_no_cosmetic: exceptions.all.is_some(),
                has_no_elements: exceptions.element.is_some(),
                has_specific: exceptions.specific.is_some(),
                has_apps: self.apps.is_some(),
                has_no_content: exceptions.content.is_some(),
                has_responses: self.responses.is_some(),
                has_urlblock: exceptions.urlblock.is_some(),
                has_no_scripts: exceptions.script.is_some(),
                has_html: !self.html.is_empty(),
                has_redirects: self.requests.redirects.is_some(),
            };
            files::replace(&cache.join("key"), meta.write().as_bytes())
        };
        let _ = write();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_uses_the_existing_wire_format() {
        let cache = tempfile::tempdir().unwrap();
        let text = "||ads.example^\n\
                    @@||ads.example^$domain=reader.test,image\n\
                    ||app.example^$app=com.reader\n\
                    @@||page.example^$elemhide\n\
                    @@||content.example^$content\n\
                    ||ads.example^$redirect=nooptext\n\
                    page.example$$div.ad\n";
        let key = 123456789;
        let filters = Filters::build(
            std::iter::once(Ok((Path::new("rules/user.txt"), text.to_owned()))),
            Some(key),
            false,
            crate::list_files::MAX_LISTS,
        )
        .unwrap();
        filters.write_cache(cache.path(), key);
        let line = std::fs::read_to_string(cache.path().join("key")).unwrap();
        assert_eq!(
            line,
            "123456789 7 1 false true false false false true false true true false false false false true\n"
        );
        assert!(Filters::read_cache(cache.path(), key).is_some());
    }

    #[test]
    fn a_cache_with_an_old_generation_is_rebuilt_from_its_lists() {
        let temp = tempfile::tempdir().unwrap();
        let lists = temp.path().join("lists");
        let cache = temp.path().join("cache");
        std::fs::create_dir(&lists).unwrap();
        std::fs::write(lists.join("user.txt"), "||ads.example^\n").unwrap();
        let key = crate::list_files::ListFiles::read(&[&lists])
            .unwrap()
            .generation;
        Filters::load_cached(&[&lists], &cache).unwrap();

        let key_file = cache.join("key");
        let current = std::fs::read_to_string(&key_file).unwrap();
        let (_, rest) = current.split_once(' ').unwrap();
        std::fs::write(&key_file, format!("{} {rest}", key.wrapping_add(1))).unwrap();
        assert!(Filters::read_cache(&cache, key).is_none());

        let rebuilt = Filters::load_cached(&[&lists], &cache).unwrap();
        assert!(rebuilt.blocks_host("ads.example"));
        assert!(Filters::read_cache(&cache, key).is_some());
    }
}
