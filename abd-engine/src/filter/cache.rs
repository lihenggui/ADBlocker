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

use super::{AppRules, Filters, PageExceptions, RequestRules, use_scriptlets};
use crate::files;
use crate::list_files::MAX_COMPILED;

/// The files of the rules for apps, in the order they are kept in.
const APPS: [&str; 4] = ["app-blocks", "app-exceptions", "app-hosts", "app-guards"];

impl Filters {
    /// The engines kept for the lists that `key` stands for, if they are.
    pub(super) fn read_cache(cache: &Path, key: u64) -> Option<Self> {
        crate::public_suffix::install();
        let key_file = cache.join("key");
        let _lock = files::lock(&key_file).ok()?;
        let meta = files::read_text(&key_file, 128).ok()?;
        let mut f = meta.split_ascii_whitespace();
        if f.next()?.parse::<u64>().ok()? != key {
            return None;
        }
        let (rules, host_rules) = (f.next()?.parse().ok()?, f.next()?.parse().ok()?);
        let has_host_guards = f.next()?.parse().ok()?;
        let has_no_cosmetic: bool = f.next()?.parse().ok()?;
        let has_no_elements: bool = f.next()?.parse().ok()?;
        let has_specific: bool = f.next()?.parse().ok()?;
        // Absent from what was kept before there were rules for apps:
        // those of its lists were not compiled.
        let has_apps: bool = f.next()?.parse().ok()?;
        let mut remaining = MAX_COMPILED;
        let apps = if has_apps {
            let mut kept = APPS.map(|name| {
                let kept = files::read_bytes(&cache.join(name), remaining).ok()?;
                remaining -= kept.len();
                Some(kept)
            });
            let [blocks, exceptions, hosts, guards] = kept.each_mut().map(Option::take);
            Some(AppRules::compiled([
                &blocks?,
                &exceptions?,
                &hosts?,
                &guards?,
            ])?)
        } else {
            None
        };
        let mut engine = |name: &str| {
            let compiled = files::read_bytes(&cache.join(name), remaining).ok()?;
            remaining -= compiled.len();
            let mut engine = Engine::default();
            // Checked by the engine against its own checksum: a file cut
            // short or damaged is refused, and the lists are read instead.
            engine.deserialize(&compiled).ok()?;
            Some(engine)
        };
        let mut full = engine("full")?;
        use_scriptlets(&mut full);
        Some(Self {
            generation: Some(key),
            requests: RequestRules { full },
            page_exceptions: PageExceptions {
                all: if has_no_cosmetic {
                    Some(engine("no-cosmetic")?)
                } else {
                    None
                },
                element: if has_no_elements {
                    Some(engine("no-element-hiding")?)
                } else {
                    None
                },
                specific: if has_specific {
                    Some((engine("no-specific-cosmetic")?, engine("generic-css")?))
                } else {
                    None
                },
            },
            hosts: engine("hosts")?,
            host_guards: if has_host_guards {
                Some(engine("host-guards")?)
            } else {
                None
            },
            apps,
            rules,
            host_rules,
        })
    }

    /// Keeps the engines for next time. Not being able to is no error.
    pub(super) fn write_cache(&self, cache: &Path, key: u64) {
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(cache)?;
            let full = self.requests.full.serialize();
            let hosts = self.hosts.serialize();
            let host_guards = self.host_guards.as_ref().map(Engine::serialize);
            let exceptions = &self.page_exceptions;
            let apps = self.apps.as_ref();
            let kept = apps.map(|rules| rules.serialized().map(Some));
            let compiled = [
                ("full", Some(full)),
                // Kept by versions that judged a request without a page apart.
                ("unknown", None),
                ("hosts", Some(hosts)),
                ("host-guards", host_guards),
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
                    exceptions
                        .specific
                        .as_ref()
                        .map(|(engine, _)| engine.serialize()),
                ),
                (
                    "generic-css",
                    exceptions
                        .specific
                        .as_ref()
                        .map(|(_, engine)| engine.serialize()),
                ),
            ];
            let compiled: Vec<_> = APPS
                .into_iter()
                .zip(kept.unwrap_or_default())
                .chain(compiled)
                .collect();
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
                "{key} {} {} {} {} {} {} {}\n",
                self.rules,
                self.host_rules,
                self.host_guards.is_some(),
                exceptions.all.is_some(),
                exceptions.element.is_some(),
                exceptions.specific.is_some(),
                apps.is_some(),
            );
            files::replace(&cache.join("key"), meta.as_bytes())
        };
        let _ = write();
    }
}
