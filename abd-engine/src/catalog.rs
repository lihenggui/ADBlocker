/*
 * Copyright (C) 2026 Mercury Li
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License version 3 as
 * published by the Free Software Foundation.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program. If not, see <https://www.gnu.org/licenses/>.
 */

use serde::Deserialize;
use std::sync::{LazyLock, OnceLock};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Catalog<'a> {
    #[serde(borrow)]
    lists: Vec<List<'a>>,
    #[serde(borrow)]
    https_exclusions: Vec<&'a str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct List<'a> {
    url: &'a str,
    #[serde(default)]
    default_enabled: bool,
    /// The primary language subtags, lowercase, of the devices the list is
    /// a default for.
    #[serde(default, borrow)]
    default_languages: Vec<&'a str>,
    #[serde(default)]
    trusted_scriptlets: bool,
    #[serde(default)]
    trusted_response_rules: bool,
}

const DATA: &str = include_str!("../../config/filter-catalog.json");

fn catalog() -> Catalog<'static> {
    serde_json::from_str(DATA).expect("the bundled filter catalog is valid")
}

fn source_text(urls: impl IntoIterator<Item = &'static str>) -> String {
    urls.into_iter().map(|url| format!("{url}\n")).collect()
}

/// The primary language subtag of a language tag, such as `zh` of
/// `zh-Hans-CN`, or none for a tag that names no language.
fn primary_language(tag: &str) -> Option<String> {
    let primary = tag.trim().split(['-', '_']).next()?.to_ascii_lowercase();
    let letters =
        (2..=3).contains(&primary.len()) && primary.bytes().all(|b| b.is_ascii_lowercase());
    (letters && primary != "und").then_some(primary)
}

fn defaults_for(languages: &[String]) -> String {
    source_text(
        catalog()
            .lists
            .into_iter()
            .filter(|entry| {
                entry.default_enabled
                    || entry
                        .default_languages
                        .iter()
                        .any(|language| languages.iter().any(|l| l == language))
            })
            .map(|entry| entry.url),
    )
}

// The defaults are read deep under every command that lists, changes or
// downloads the sources, and the device's languages, like the catalog,
// stay the same for the life of a process: set once when it starts, they
// are one value for all of these without a parameter through each.
static SOURCES: OnceLock<String> = OnceLock::new();
static EXCLUSIONS: LazyLock<String> = LazyLock::new(|| source_text(catalog().https_exclusions));

pub(crate) enum Trust {
    Scriptlets,
    ResponseRules,
}

static SCRIPTLETS: LazyLock<Vec<&str>> =
    LazyLock::new(|| trusted_urls(|list| list.trusted_scriptlets));
static RESPONSES: LazyLock<Vec<&str>> =
    LazyLock::new(|| trusted_urls(|list| list.trusted_response_rules));

fn trusted_urls(takes: impl Fn(&List<'_>) -> bool) -> Vec<&'static str> {
    catalog()
        .lists
        .into_iter()
        .filter(takes)
        .map(|list| list.url)
        .collect()
}

/// Makes the lists for `tags`, the device's language tags, defaults
/// besides the general ones. The host calls it once, before anything reads
/// the defaults; a later call changes nothing and returns false. A tag
/// that names no language adds nothing.
pub fn set_device_languages<'a>(tags: impl IntoIterator<Item = &'a str>) -> bool {
    let languages: Vec<String> = tags.into_iter().filter_map(primary_language).collect();
    SOURCES.set(defaults_for(&languages)).is_ok()
}

impl Trust {
    pub(crate) fn urls(self) -> &'static [&'static str] {
        match self {
            Self::Scriptlets => &SCRIPTLETS,
            Self::ResponseRules => &RESPONSES,
        }
    }
}

/// What compiled lists depend on here: the catalog, and the defaults it
/// gave for the device's languages.
pub(crate) fn cache_identity() -> (&'static str, &'static str) {
    (DATA, default_sources())
}

/// Standalone defaults when the user has not saved subscriptions: the
/// general lists, and those for the device's languages once the host has
/// told them (`set_device_languages`).
pub fn default_sources() -> &'static str {
    SOURCES.get_or_init(|| defaults_for(&[]))
}

pub(crate) fn exclusion_sources() -> &'static str {
    &EXCLUSIONS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_shared_catalog_contains_distinct_https_sources_and_language_free_general_defaults() {
        let data = catalog();
        let mut urls = HashSet::new();
        for url in data
            .lists
            .iter()
            .map(|entry| entry.url)
            .chain(data.https_exclusions)
        {
            assert!(crate::sources::valid(url), "invalid catalog URL: {url}");
            assert!(urls.insert(url), "duplicate catalog URL: {url}");
        }
        let full: serde_json::Value = serde_json::from_str(DATA).unwrap();
        for entry in full["lists"].as_array().unwrap() {
            if entry["defaultEnabled"] == true {
                assert!(entry.get("defaultLanguages").is_none());
                assert_ne!(entry["category"], "Regional");
            }
            if entry["category"] == "Regional" {
                assert!(entry.get("defaultLanguages").is_some());
            }
        }
        for entry in data.lists {
            for language in entry.default_languages {
                assert_eq!(primary_language(language).as_deref(), Some(language));
            }
        }
        assert!(!default_sources().is_empty());
        assert!(!exclusion_sources().is_empty());
    }

    fn url_of(id: &str) -> String {
        let full: serde_json::Value = serde_json::from_str(DATA).unwrap();
        let entry = full["lists"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == id)
            .unwrap();
        entry["url"].as_str().unwrap().to_owned()
    }

    fn languages(tags: &[&str]) -> Vec<String> {
        tags.iter().copied().filter_map(primary_language).collect()
    }

    #[test]
    fn the_lists_of_a_device_language_are_defaults_besides_the_general_ones() {
        let general = defaults_for(&[]);
        let chinese = defaults_for(&languages(&["zh-Hans-CN"]));
        for line in general.lines() {
            assert!(chinese.lines().any(|l| l == line), "{line}");
        }
        for id in ["easylist_china", "adguard_chinese", "awavenue"] {
            assert!(chinese.contains(&url_of(id)), "{id}");
            assert!(!general.contains(&url_of(id)), "{id}");
        }
        assert!(!chinese.contains(&url_of("japanese_filters")));
        let both = defaults_for(&languages(&["ja-JP", "zh-TW"]));
        assert!(both.contains(&url_of("japanese_filters")));
        assert!(both.contains(&url_of("awavenue")));
    }

    #[test]
    fn only_the_primary_language_subtag_is_matched() {
        assert_eq!(
            languages(&["pt-BR", "de_AT", "ES", "fr"]),
            ["pt", "de", "es", "fr"]
        );
        // Nothing, an undetermined language, or a script names no language.
        assert!(languages(&["", " ", "und", "Hans", "-zh", "zh1"]).is_empty());
        assert_eq!(defaults_for(&languages(&["en-US", ""])), defaults_for(&[]));
    }
}
