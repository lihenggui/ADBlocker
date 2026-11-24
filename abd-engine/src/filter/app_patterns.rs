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

//! Interns app patterns so network and response rules can share package matches.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use adblock::utils::fast_hash;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use super::{
    app_package,
    rule_options::{delimiter, unescape, unescaped},
};
use crate::apps::valid_package;
use crate::limits::{
    MAX_APP_EXPRESSION, MAX_APP_PATTERN_PACKAGES, MAX_APP_PATTERNS, MAX_APP_REGEX,
};

// '@' is forbidden inside a package name and denotes userinfo in a URL,
// so no package or hostname can be one of these source names.
const SYNTHETIC: &str = "@abd-app-";
/// A negative pattern condition needs every pattern's result before it can apply.
const INCOMPLETE: &str = "@abd-app-incomplete";

enum Matcher {
    Prefix,
    Regex(Regex),
}

struct Pattern {
    text: String,
    matcher: Matcher,
    hash: u64,
}

impl Pattern {
    fn new(text: String, index: usize) -> Option<Self> {
        let matcher = if let Some(prefix) = text.strip_suffix('*') {
            (valid_package(prefix) && prefix == prefix.to_ascii_lowercase())
                .then_some(Matcher::Prefix)?
        } else {
            let body = text.strip_prefix('/')?.strip_suffix('/')?;
            if body.len() > MAX_APP_EXPRESSION
                || delimiter(body).is_some()
                || (*b"$,|")
                    .into_iter()
                    .any(|b| unescaped(body, b).next().is_some())
                || text.as_bytes().iter().any(u8::is_ascii_control)
            {
                return None;
            }
            let expr = unescape(body, &['/', '$', ',', '|']);
            Matcher::Regex(
                RegexBuilder::new(&expr)
                    .size_limit(MAX_APP_REGEX)
                    .dfa_size_limit(MAX_APP_REGEX)
                    .build()
                    .ok()?,
            )
        };
        let hash = fast_hash(&format!("{SYNTHETIC}{index}"));
        Some(Self {
            text,
            matcher,
            hash,
        })
    }

    fn matches(&self, package: &str, lowercase: &str) -> bool {
        match &self.matcher {
            Matcher::Prefix => lowercase.starts_with(&self.text[..self.text.len() - 1]),
            Matcher::Regex(regex) => regex.is_match(package),
        }
    }
}

#[derive(Default)]
pub(crate) struct Patterns {
    entries: Vec<Pattern>,
    packages: Mutex<HashMap<String, Arc<[u64]>>>,
    dropped: usize,
}

/// A rule can claim new slots only after all its conditions are valid.
pub(super) struct Prepared {
    pub(super) encoded: String,
    pending: Vec<Pattern>,
}

/// One app's source hashes, shared across the engines of a generation.
pub(crate) struct Scope<'a> {
    pub(super) hashes: Arc<[u64]>,
    pub(super) patterns: &'a Patterns,
}

/// App qualifiers for response rules, which can also have page conditions.
#[derive(Serialize, Deserialize)]
pub(super) struct Condition {
    negated: bool,
    hashes: Vec<u64>,
}

impl Condition {
    pub(super) fn new(encoded: &str) -> Self {
        let negated = encoded.starts_with('~');
        Self {
            negated,
            hashes: encoded
                .split('|')
                .map(|name| fast_hash(name.trim_start_matches('~')))
                .collect(),
        }
    }

    pub(super) fn matches(&self, scope: &Scope<'_>) -> bool {
        self.hashes.iter().any(|hash| scope.hashes.contains(hash)) != self.negated
    }
}

/// Read a single entry up to a pipe outside its regex delimiters.
fn entry(text: &str) -> Option<(&str, &str)> {
    let end = if let Some(body) = text.strip_prefix('/') {
        delimiter(body)? + 2
    } else {
        text.find('|').unwrap_or(text.len())
    };
    Some(text.split_at(end))
}

impl Patterns {
    /// Deduplicate entries before compiling regexes or charging the budget.
    pub(super) fn prepare(&mut self, mut names: &str) -> Option<Prepared> {
        let mut pending = Vec::new();
        let mut encoded = String::new();
        let negated = names.starts_with('~');
        let mut has_pattern = false;
        loop {
            let text = names.strip_prefix('~').unwrap_or(names);
            if negated != (text.len() != names.len()) {
                return None;
            }
            let (name, rest) = entry(text)?;
            if negated {
                encoded.push('~');
            }
            if name.starts_with('/') || name.ends_with('*') {
                has_pattern = true;
                let key = if name.starts_with('/') {
                    name.to_owned()
                } else {
                    name.to_ascii_lowercase()
                };
                let index = match self
                    .entries
                    .iter()
                    .chain(&pending)
                    .position(|p: &Pattern| p.text == key)
                {
                    Some(index) => index,
                    None => {
                        let index = self.entries.len() + pending.len();
                        if index == MAX_APP_PATTERNS {
                            self.dropped += 1;
                            return None;
                        }
                        pending.push(Pattern::new(key, index)?);
                        index
                    }
                };
                use std::fmt::Write;
                write!(encoded, "{SYNTHETIC}{index}").expect("writing to a string");
            } else if valid_package(name) {
                encoded.push_str(&name.to_ascii_lowercase());
            } else {
                return None;
            }
            if rest.is_empty() {
                break;
            }
            names = rest.strip_prefix('|')?;
            encoded.push('|');
        }
        if negated && has_pattern {
            encoded.push_str("|~");
            encoded.push_str(INCOMPLETE);
        }
        Some(Prepared { encoded, pending })
    }

    pub(super) fn keep(&mut self, prepared: Prepared) {
        self.entries.extend(prepared.pending);
    }

    pub(crate) fn report_dropped(&self) {
        if self.dropped != 0 {
            eprintln!(
                "lists: skipped {} app rules at the pattern limit",
                self.dropped
            );
        }
    }

    pub(crate) fn scope(&self, app: &str) -> Option<Scope<'_>> {
        let package = app_package(app)?;
        let plain = || fast_hash(&package.to_ascii_lowercase());
        let hashes = if self.entries.is_empty() {
            [plain()].into()
        } else {
            let mut packages = self.packages.lock().unwrap();
            if let Some(hashes) = packages.get(package) {
                Arc::clone(hashes)
            } else if packages.len() == MAX_APP_PATTERN_PACKAGES {
                [plain(), fast_hash(INCOMPLETE)].into()
            } else {
                let lower = package.to_ascii_lowercase();
                let hashes: Arc<[u64]> = std::iter::once(fast_hash(&lower))
                    .chain(
                        self.entries
                            .iter()
                            .filter(|p| p.matches(package, &lower))
                            .map(|p| p.hash),
                    )
                    .collect();
                packages.insert(package.to_owned(), Arc::clone(&hashes));
                hashes
            }
        };
        Some(Scope {
            hashes,
            patterns: self,
        })
    }

    /// Restore the author-facing spelling after an engine reports a rule.
    pub(super) fn show(&self, encoded: &str) -> String {
        encoded
            .split('|')
            .filter(|name| name.strip_prefix('~') != Some(INCOMPLETE))
            .map(|name| {
                let (sign, name) = name
                    .strip_prefix('~')
                    .map_or(("", name), |name| ("~", name));
                let original = name
                    .strip_prefix(SYNTHETIC)
                    .and_then(|index| index.parse::<usize>().ok())
                    .and_then(|index| self.entries.get(index))
                    .map_or(name, |pattern| pattern.text.as_str());
                format!("{sign}{original}")
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    pub(crate) fn serialized(&self) -> Vec<u8> {
        let keys: Vec<_> = self.entries.iter().map(|pattern| &pattern.text).collect();
        serde_json::to_vec(&keys).expect("serializing strings")
    }

    pub(crate) fn cached(bytes: &[u8]) -> Option<Self> {
        let keys: Vec<String> = serde_json::from_slice(bytes).ok()?;
        if keys.len() > MAX_APP_PATTERNS {
            return None;
        }
        let entries = keys
            .into_iter()
            .enumerate()
            .map(|(i, key)| Pattern::new(key, i))
            .collect::<Option<_>>()?;
        Some(Self {
            entries,
            ..Self::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_package_cache_keeps_old_matches_and_uses_only_the_new_packages_name() {
        let mut patterns = Patterns::default();
        let prepared = patterns.prepare("com.reader*").unwrap();
        let condition = Condition::new(&prepared.encoded);
        patterns.keep(prepared);
        let negated = patterns.prepare("~com.reader*").unwrap();
        let excluded = Condition::new(&negated.encoded);
        assert_eq!(patterns.show(&negated.encoded), "~com.reader*");
        let plain = Condition::new("com.reader.overflow");
        for n in 0..MAX_APP_PATTERN_PACKAGES {
            let package = format!("com.reader{n}");
            assert!(condition.matches(&patterns.scope(&package).unwrap()));
        }
        let app = "com.reader.overflow";
        let scope = patterns.scope(app).unwrap();
        assert!(plain.matches(&scope));
        assert!(!condition.matches(&scope));
        assert!(!excluded.matches(&scope));
        assert!(condition.matches(&patterns.scope("com.reader0").unwrap()));
        assert!(patterns.scope("@com.reader0").is_none());
    }
}
