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

//! On-demand request diagnostics. Only this engine needs original rule text.

use std::path::{Path, PathBuf};

use adblock::{
    Engine,
    lists::{ParseOptions, RuleTypes},
};
use anyhow::Result;
use ring::digest::{SHA256, digest};

use crate::{
    files,
    filter::{AppProjections, AppRules, MatcherLists, Patterns, RequestRules, app_rule},
    list_files::{ListFiles, MAX_COMPILED, read_lists},
    paths::Paths,
    protocol_types::Check,
    rule::{self, Kind, Rule},
    sources,
};

// A fingerprint and a big-endian list number. Sorting puts the earliest
// source first when several lists contain the same rule.
type Entry = [u8; 20];
const MAX_INDEX: usize = 512 * 1024;

struct Diagnostics {
    rules: RequestRules,
    apps: Option<AppRules>,
    patterns: Patterns,
    index: Vec<Entry>,
}

fn fingerprint(rule: &str) -> [u8; 16] {
    digest(&SHA256, rule.as_bytes()).as_ref()[..16]
        .try_into()
        .unwrap()
}

fn app_name(rule: &Rule<'_>) -> Option<String> {
    match &rule.kind {
        Kind::App(app) => app_rule(app),
        _ => None,
    }
}

fn index_rule(source: &Rule<'_>, number: usize, index: &mut Vec<Entry>) -> bool {
    let normalized = source.normalized();
    if normalized.is_empty() {
        return true;
    }
    // A rule for apps takes two entries.
    if index.len() + 2 > MAX_INDEX {
        *index = Vec::new();
        return false;
    }
    // A rule for apps is named as the engine has it.
    let named = app_name(source);
    for rule in [Some(normalized.as_ref()), named.as_deref()]
        .into_iter()
        .flatten()
    {
        let mut entry = [0; 20];
        entry[..16].copy_from_slice(&fingerprint(rule));
        entry[16..].copy_from_slice(&(number as u32).to_be_bytes());
        index.push(entry);
    }
    true
}

impl Diagnostics {
    fn build<'a>(
        paths: &[PathBuf],
        entries: impl Iterator<Item = std::io::Result<(&'a Path, String)>>,
    ) -> Result<Self> {
        let options = ParseOptions {
            rule_types: RuleTypes::NetworkOnly,
            ..ParseOptions::default()
        };
        let mut index = Vec::new();
        let mut indexing = true;
        let lists = entries.collect::<std::io::Result<Vec<_>>>()?;
        let mut matcher = MatcherLists::new(true);
        let mut apps = AppProjections::default();
        let mut patterns = Patterns::default();
        for (file, text) in &lists {
            let mut normalized = String::with_capacity(text.len());
            let number = paths.binary_search_by(|p| p.as_path().cmp(file)).unwrap();
            for line in text.lines() {
                let source = rule::read(line);
                MatcherLists::add(&mut normalized, &source);
                if let Kind::App(app) = &source.kind {
                    apps.add(app, &mut patterns);
                }
                if indexing {
                    indexing = index_rule(&source, number, &mut index);
                }
            }
            matcher.finish_list(normalized, options);
        }
        index.sort_unstable();
        index.dedup_by(|next, first| next[..16] == first[..16]);
        let apps = apps.compile(true, |_, _| {});
        patterns.report_dropped();
        Ok(Self {
            apps,
            patterns,
            rules: matcher.compile(),
            index,
        })
    }

    fn source<'a>(&self, files: &'a [PathBuf], rule: &str) -> Option<&'a Path> {
        // Very dense lists still get exact attribution within the text budget.
        if self.index.is_empty() {
            for (file, text) in read_lists(files).flatten() {
                if text.lines().any(|line| {
                    let source = rule::read(line);
                    source.normalized() == rule || app_name(&source).as_deref() == Some(rule)
                }) {
                    return Some(file);
                }
            }
            return None;
        }
        let key = fingerprint(rule);
        let n = self.index.binary_search_by(|e| e[..16].cmp(&key)).ok()?;
        let number = u32::from_be_bytes(self.index[n][16..].try_into().ok()?);
        files.get(number as usize).map(PathBuf::as_path)
    }

    fn read_part(input: &[u8]) -> Option<(&[u8], &[u8])> {
        let (size, rest) = input.split_at_checked(4)?;
        rest.split_at_checked(u32::from_le_bytes(size.try_into().ok()?) as usize)
    }

    fn read(path: &Path, generation: u64) -> Option<Self> {
        let data = files::read_bytes(path, MAX_COMPILED).ok()?;
        let (checksum, payload) = data.split_at_checked(32)?;
        let (key, rest) = payload.split_at_checked(8)?;
        if key != generation.to_le_bytes() || digest(&SHA256, payload).as_ref() != checksum {
            return None;
        }
        fn engine(input: &[u8]) -> Option<(Option<Engine>, &[u8])> {
            let (compiled, tail) = Diagnostics::read_part(input)?;
            let decoded = if compiled.is_empty() {
                None
            } else {
                let mut engine = Engine::default();
                engine.deserialize(compiled).ok()?;
                Some(engine)
            };
            Some((decoded, tail))
        }
        let (full, rest) = engine(rest)?;
        let mut rest = rest;
        let mut apps = [&[][..]; 4];
        for kept in &mut apps {
            (*kept, rest) = Self::read_part(rest)?;
        }
        let apps = if apps[0].is_empty() {
            None
        } else {
            Some(AppRules::compiled(apps)?)
        };
        let (patterns, entries) = Self::read_part(rest)?;
        let patterns = Patterns::cached(patterns)?;
        let (entries, tail) = entries.as_chunks::<20>();
        if !tail.is_empty() || entries.len() > MAX_INDEX {
            return None;
        }
        Some(Self {
            rules: RequestRules { full: full? },
            apps,
            patterns,
            index: entries.to_vec(),
        })
    }

    fn write(&self, path: &Path, generation: u64) -> std::io::Result<()> {
        let full = self.rules.full.serialize();
        // Empty, as no compiled engine is, where no list has such a rule.
        let apps = self
            .apps
            .as_ref()
            .map(AppRules::serialized)
            .unwrap_or_default();
        let compiled = [full]
            .into_iter()
            .chain(apps)
            .chain([self.patterns.serialized()]);
        let size = 40
            + compiled.clone().map(|part| 4 + part.len()).sum::<usize>()
            + self.index.len() * size_of::<Entry>();
        if size > MAX_COMPILED {
            return Ok(());
        }
        let mut data = Vec::with_capacity(size);
        data.resize(32, 0);
        data.extend(generation.to_le_bytes());
        for compiled in compiled {
            data.extend((compiled.len() as u32).to_le_bytes());
            data.extend(compiled);
        }
        data.extend_from_slice(self.index.as_flattened());
        let checksum = digest(&SHA256, &data[32..]);
        data[..32].copy_from_slice(checksum.as_ref());
        std::fs::create_dir_all(path.parent().unwrap())?;
        let _lock = files::lock(path)?;
        files::replace(path, &data)
    }
}

fn load(dirs: &[&Path], cache: &Path) -> Result<(Vec<PathBuf>, Diagnostics)> {
    let mut files = ListFiles::read(dirs)?;
    if let Some(hit) = Diagnostics::read(cache, files.generation) {
        return Ok((files.paths, hit));
    }
    for _ in 0..2 {
        let new = Diagnostics::build(&files.paths, read_lists(&files.paths))?;
        let current = ListFiles::read(dirs)?;
        if current.generation == files.generation {
            // The cache is optional: this check has its whole result,
            // and the next one builds the cache again.
            let _ = new.write(cache, files.generation);
            return Ok((files.paths, new));
        }
        files = current;
    }
    anyhow::bail!("the lists are changing")
}

/// `abd check --json` keeps the original rule text for attribution.
///
/// `app` is the app that asks, as the engine names it, for the rules that
/// are for apps; without one only the rules for every app are looked at.
pub fn check(paths: &Paths, app: Option<&str>, url: &str, page: &str, kind: &str) -> Result<Check> {
    crate::public_suffix::install();
    let rules = paths.rules();
    let lists = paths.lists();
    let (files, diagnostics) = load(&[&lists, &rules], &paths.cache().join("diagnostic-v1"))?;
    let apps = diagnostics
        .apps
        .as_ref()
        .zip(app.and_then(|app| diagnostics.patterns.scope(app)));
    let result = diagnostics.rules.check_for(apps, url, page, kind, "GET");
    let source = result.rule.as_deref().and_then(|rule| {
        let file = diagnostics.source(&files, rule)?;
        if file.parent() == Some(rules.as_path()) {
            return Some("rules".to_owned());
        }
        sources::read(paths)
            .into_iter()
            .find(|s| s.enabled && sources::active_copy(&lists, &s.url).1 == *file)
            .map(|s| s.url)
    });
    Ok(Check {
        blocked: result.blocked,
        rule: result.rule,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_fail_when_a_list_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(paths.rules()).unwrap();
        std::fs::create_dir_all(paths.lists()).unwrap();
        std::fs::write(paths.user_rules(), "||ads.example^\n").unwrap();
        let diagnostic = || check(&paths, None, "https://ads.example/", "", "image");
        assert!(diagnostic().unwrap().blocked);
        let cache = paths.cache().join("diagnostic-v1");
        let cached = std::fs::read(&cache).unwrap();
        let list = paths.lists().join("missing.txt");
        std::os::unix::fs::symlink(paths.lists().join("missing"), &list).unwrap();

        let error = diagnostic().unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(std::fs::read(cache).unwrap(), cached);

        std::fs::remove_file(list).unwrap();
        assert!(diagnostic().unwrap().blocked);
    }

    #[test]
    fn the_index_of_rules_stays_within_its_limit_where_a_rule_for_an_app_fills_it() {
        let dir = tempfile::tempdir().unwrap();
        let list = dir.path().join("list.txt");
        let mut text = String::new();
        for n in 0..MAX_INDEX - 1 {
            text.push_str(&format!("||{n}.example^\n"));
        }
        text.push_str("||last.example^$app=com.reader\n||more.example^\n");
        let paths = [list.clone()];
        let built =
            Diagnostics::build(&paths, std::iter::once(Ok((list.as_path(), text)))).unwrap();
        assert!(built.index.len() <= MAX_INDEX);
    }

    #[test]
    fn a_rule_for_an_app_is_found_for_that_app_and_named_with_its_list() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(paths.rules()).unwrap();
        std::fs::write(
            paths.user_rules(),
            "||shop.example^$app=com.read*\n||ads.example^\n@@||ads.example^$app=com.reader\n",
        )
        .unwrap();
        let shop = |app| check(&paths, app, "https://shop.example/a", "", "other").unwrap();
        let ads = |app| check(&paths, app, "https://ads.example/a", "", "other").unwrap();
        // Built, and then read back as it was kept.
        for _ in 0..2 {
            let found = shop(Some("com.reader"));
            assert!(found.blocked);
            assert_eq!(found.rule.as_deref(), Some("||shop.example^$app=com.read*"));
            assert_eq!(found.source.as_deref(), Some("rules"));
            assert!(!shop(Some("com.other")).blocked);
            assert!(!shop(None).blocked);
            assert!(!ads(Some("com.reader")).blocked);
            assert!(ads(Some("com.other")).blocked && ads(None).blocked);
        }
    }

    #[test]
    fn diagnostics_without_a_source_apply_no_rule_for_a_named_page_across_cache_loads() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(paths.rules()).unwrap();
        std::fs::write(
            paths.user_rules(),
            "||private.example^$3p\n||ads.example^\n@@||ads.example/allowed^$from=reader.example,image\n",
        ).unwrap();
        let no_source =
            || check(&paths, None, "https://private.example/p.gif", "", "image").unwrap();
        let normal = || {
            check(
                &paths,
                None,
                "https://ads.example/blocked/p.gif",
                "",
                "image",
            )
            .unwrap()
        };
        let exception_for_a_page = |page| {
            check(
                &paths,
                None,
                "https://ads.example/allowed/p.gif",
                page,
                "image",
            )
            .unwrap()
        };
        for _ in 0..2 {
            let unknown = no_source();
            assert!(!unknown.blocked);
            assert!(unknown.rule.is_none());
            let neutral = normal();
            assert!(neutral.blocked);
            assert_eq!(neutral.rule.as_deref(), Some("||ads.example^"));
            assert_eq!(neutral.source.as_deref(), Some("rules"));
            assert!(exception_for_a_page("").blocked);
            assert!(!exception_for_a_page("https://reader.example/").blocked);
            assert!(
                check(
                    &paths,
                    None,
                    "https://private.example/p.gif",
                    "https://reader.example/",
                    "image"
                )
                .unwrap()
                .blocked
            );
            assert!(paths.cache().join("diagnostic-v1").is_file());
        }
    }
}
