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
    filter::{self, RequestRules},
    list_files::{ListFiles, MAX_COMPILED, read_lists},
    list_url,
    paths::Paths,
    protocol_types::Check,
    sources,
};

// A fingerprint and a big-endian list number. Sorting puts the earliest
// source first when several lists contain the same rule.
type Entry = [u8; 20];
const MAX_INDEX: usize = 512 * 1024;

struct Diagnostics {
    rules: RequestRules,
    index: Vec<Entry>,
}

fn fingerprint(rule: &str) -> [u8; 16] {
    digest(&SHA256, rule.as_bytes()).as_ref()[..16]
        .try_into()
        .unwrap()
}

impl Diagnostics {
    fn build<'a>(
        paths: &[PathBuf],
        entries: impl Iterator<Item = std::io::Result<(&'a Path, String)>>,
    ) -> Result<Self> {
        let mut lists = Vec::new();
        let options = ParseOptions {
            rule_types: RuleTypes::NetworkOnly,
            ..ParseOptions::default()
        };
        let mut index = Vec::new();
        let mut indexing = true;
        for entry in entries {
            let (file, text) = entry?;
            if indexing {
                let number = paths.binary_search_by(|p| p.as_path().cmp(file)).unwrap();
                for rule in text
                    .lines()
                    .map(filter::network_line)
                    .filter(|s| !s.is_empty())
                {
                    if index.len() == MAX_INDEX {
                        index = Vec::new();
                        indexing = false;
                        break;
                    }
                    let mut entry = [0; 20];
                    entry[..16].copy_from_slice(&fingerprint(&rule));
                    entry[16..].copy_from_slice(&(number as u32).to_be_bytes());
                    index.push(entry);
                }
            }
            lists.push(text);
        }
        index.sort_unstable();
        index.dedup_by(|next, first| next[..16] == first[..16]);
        Ok(Self {
            rules: RequestRules::build(lists, true, options),
            index,
        })
    }

    fn source<'a>(&self, files: &'a [PathBuf], rule: &str) -> Option<&'a Path> {
        // Very dense lists still get exact attribution within the text budget.
        if self.index.is_empty() {
            for (file, text) in read_lists(files).flatten() {
                if text.lines().any(|line| filter::network_line(line) == rule) {
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

    fn read(path: &Path, generation: u64) -> Option<Self> {
        let data = files::read_bytes(path, MAX_COMPILED).ok()?;
        let (checksum, payload) = data.split_at_checked(32)?;
        let (key, rest) = payload.split_at_checked(8)?;
        if key != generation.to_le_bytes() || digest(&SHA256, payload).as_ref() != checksum {
            return None;
        }
        fn engine(input: &[u8]) -> Option<(Option<Engine>, &[u8])> {
            let (size, rest) = input.split_at_checked(4)?;
            let (compiled, tail) =
                rest.split_at_checked(u32::from_le_bytes(size.try_into().ok()?) as usize)?;
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
        let (unknown, entries) = engine(rest)?;
        let (entries, tail) = entries.as_chunks::<20>();
        if !tail.is_empty() || entries.len() > MAX_INDEX {
            return None;
        }
        Some(Self {
            rules: RequestRules {
                full: full?,
                unknown,
            },
            index: entries.to_vec(),
        })
    }

    fn write(&self, path: &Path, generation: u64) -> std::io::Result<()> {
        let full = self.rules.full.serialize();
        let unknown = self
            .rules
            .unknown
            .as_ref()
            .map_or_else(Vec::new, Engine::serialize);
        let size = 48 + full.len() + unknown.len() + self.index.len() * size_of::<Entry>();
        if size > MAX_COMPILED {
            return Ok(());
        }
        let mut data = Vec::with_capacity(size);
        data.resize(32, 0);
        data.extend(generation.to_le_bytes());
        for compiled in [full, unknown] {
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
pub fn check(paths: &Paths, url: &str, page: &str, kind: &str) -> Result<Check> {
    let rules = paths.rules();
    let lists = paths.lists();
    let (files, diagnostics) = load(&[&lists, &rules], &paths.cache().join("diagnostic-v1"))?;
    let result = diagnostics.rules.check(url, page, kind, "GET");
    let source = result.rule.as_deref().and_then(|rule| {
        let file = diagnostics.source(&files, rule)?;
        if file.parent() == Some(rules.as_path()) {
            return Some("rules".to_owned());
        }
        let name = file.file_name()?.to_str()?;
        sources::read(paths)
            .into_iter()
            .find(|s| list_url::file_name(&s.url) == name)
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
        let diagnostic = || check(&paths, "https://ads.example/", "", "image");
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
    fn diagnostics_without_a_source_keep_real_context_unknown_across_cache_loads() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(paths.rules()).unwrap();
        std::fs::write(
            paths.user_rules(),
            "||private.example^$3p\n||ads.example^\n@@||ads.example/allowed^$from=reader.example,image\n",
        ).unwrap();
        let no_source = || check(&paths, "https://private.example/p.gif", "", "image").unwrap();
        let normal = || check(&paths, "https://ads.example/blocked/p.gif", "", "image").unwrap();
        let possible_exception =
            || check(&paths, "https://ads.example/allowed/p.gif", "", "image").unwrap();
        for _ in 0..2 {
            let unknown = no_source();
            assert!(!unknown.blocked);
            assert!(unknown.rule.is_none());
            let neutral = normal();
            assert!(neutral.blocked);
            assert_eq!(neutral.rule.as_deref(), Some("||ads.example^"));
            assert_eq!(neutral.source.as_deref(), Some("rules"));
            assert!(!possible_exception().blocked);
            assert!(
                check(
                    &paths,
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
