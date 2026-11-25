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

mod time_zones;

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
    #[serde(default, borrow)]
    default_regions: Vec<&'a str>,
    #[serde(default)]
    trusted_scriptlets: bool,
    #[serde(default)]
    trusted_scripts: bool,
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

/// Find the region in a BCP 47 tag, after its language, extlangs and script.
fn locale_region(tag: &str) -> Option<String> {
    let mut subtags = tag.trim().split(['-', '_']).peekable();
    let language = subtags.next()?;
    let letters = |s: &str| s.bytes().all(|b| b.is_ascii_alphabetic());
    if !(2..=8).contains(&language.len()) || !letters(language) {
        return None;
    }
    if language.len() <= 3 {
        for _ in 0..3 {
            if subtags.next_if(|s| s.len() == 3 && letters(s)).is_none() {
                break;
            }
        }
    }
    subtags.next_if(|s| s.len() == 4 && letters(s));
    let region = subtags.next()?;
    (region.len() == 2 && letters(region)).then(|| region.to_ascii_uppercase())
}

fn defaults_for<'a>(locale_tags: impl IntoIterator<Item = &'a str>, time_zone: &str) -> String {
    let mut languages = Vec::new();
    let mut regions = Vec::new();
    for tag in locale_tags {
        languages.extend(primary_language(tag));
        regions.extend(locale_region(tag));
    }
    regions.extend(time_zones::region(time_zone).map(str::to_owned));
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
                    || entry
                        .default_regions
                        .iter()
                        .any(|region| regions.iter().any(|r| r == region))
            })
            .map(|entry| entry.url),
    )
}

// The defaults are read deep under every command that lists, changes or
// downloads the sources. Capture the defaults once per process so all
// of these commands use the same selection without passing it through each.
static SOURCES: OnceLock<String> = OnceLock::new();
static EXCLUSIONS: LazyLock<String> = LazyLock::new(|| source_text(catalog().https_exclusions));

pub(crate) enum Trust {
    Scriptlets,
    Scripts,
    ResponseRules,
}

static SCRIPTLETS: LazyLock<Vec<&str>> =
    LazyLock::new(|| trusted_urls(|list| list.trusted_scriptlets));
static SCRIPTS: LazyLock<Vec<&str>> = LazyLock::new(|| trusted_urls(|list| list.trusted_scripts));
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

/// Sets the general defaults and those for the device's locale tags and
/// time zone. The host calls it before anything reads the defaults.
/// A later call changes nothing and returns false.
pub fn set_device_locale<'a>(
    locale_tags: impl IntoIterator<Item = &'a str>,
    time_zone: &str,
) -> bool {
    SOURCES.set(defaults_for(locale_tags, time_zone)).is_ok()
}

impl Trust {
    pub(crate) fn urls(self) -> &'static [&'static str] {
        match self {
            Self::Scriptlets => &SCRIPTLETS,
            Self::Scripts => &SCRIPTS,
            Self::ResponseRules => &RESPONSES,
        }
    }
}

/// What compiled lists depend on here: the catalog, and the defaults it
/// gave for the device.
pub(crate) fn cache_identity() -> (&'static str, &'static str) {
    (DATA, default_sources())
}

/// Standalone defaults when the user has not saved subscriptions: the
/// general lists, and the regional defaults set by the host.
pub fn default_sources() -> &'static str {
    SOURCES.get_or_init(|| defaults_for([], ""))
}

pub(crate) fn exclusion_sources() -> &'static str {
    &EXCLUSIONS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_shared_catalog_contains_distinct_https_sources_and_only_regional_lists_have_locale_defaults()
     {
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
            let regional = entry["category"] == "Regional";
            assert_eq!(entry.get("defaultLanguages").is_some(), regional);
            assert_eq!(entry.get("defaultRegions").is_some(), regional);
            if entry["defaultEnabled"] == true {
                assert!(!regional);
            }
            if regional {
                assert!(!entry["defaultLanguages"].as_array().unwrap().is_empty());
                assert!(!entry["defaultRegions"].as_array().unwrap().is_empty());
            }
        }
        for entry in data.lists {
            for language in entry.default_languages {
                assert_eq!(primary_language(language).as_deref(), Some(language));
            }
            for region in entry.default_regions {
                assert_eq!(region.len(), 2);
                assert!(region.bytes().all(|b| b.is_ascii_uppercase()));
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

    const CHINESE: &[&str] = &["easylist_china", "adguard_chinese", "awavenue"];

    #[test]
    fn a_language_locale_region_or_time_zone_can_each_enable_a_regional_default() {
        let general = defaults_for(["en-US"], "Etc/UTC");
        assert_eq!(general, defaults_for([], ""));
        for (locale, zone) in [
            ("zh", "Etc/UTC"),
            ("en-CN", "Etc/UTC"),
            ("en-HK", "Etc/UTC"),
            ("en-MO", "Etc/UTC"),
            ("en-TW", "Etc/UTC"),
            ("en-US", "Asia/Shanghai"),
            ("en-US", "Asia/Hong_Kong"),
            ("en-US", "Asia/Macau"),
            ("en-US", "Asia/Taipei"),
        ] {
            let defaults = defaults_for([locale], zone);
            for line in general.lines() {
                assert!(defaults.lines().any(|l| l == line), "{line}");
            }
            for id in CHINESE {
                assert!(defaults.contains(&url_of(id)), "{locale} {zone}: {id}");
                assert!(!general.contains(&url_of(id)), "{id}");
            }
            assert!(!defaults.contains(&url_of("japanese_filters")));
        }
    }

    #[test]
    fn all_locale_tags_and_the_time_zone_contribute_defaults() {
        let both = defaults_for(["ja", "en-BE"], "Asia/Shanghai");
        for id in ["japanese_filters", "easylist_dutch", "liste_fr", "awavenue"] {
            assert!(both.contains(&url_of(id)), "{id}");
        }
    }

    #[test]
    fn only_the_primary_language_and_region_subtags_are_matched() {
        assert_eq!(
            ["pt-BR", "de_AT", "ES", "fr"].map(|tag| primary_language(tag).unwrap()),
            ["pt", "de", "es", "fr"]
        );
        for tag in ["", " ", "und", "Hans", "-zh", "zh1"] {
            assert_eq!(primary_language(tag), None, "{tag}");
        }
        assert_eq!(locale_region("en_hans_cn").as_deref(), Some("CN"));
        assert_eq!(locale_region("zh-yue-Hant-HK").as_deref(), Some("HK"));
        assert_eq!(locale_region("und-CN").as_deref(), Some("CN"));
        for tag in ["en-419", "en-u-rg-cnzzzz", "en-x-CN", "x-en-CN"] {
            assert_eq!(locale_region(tag), None, "{tag}");
        }
    }

    #[test]
    fn new_regional_defaults_respect_both_kinds_of_saved_choice() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("sources");
        let disabled = url_of("easylist_china");
        let enabled = url_of("adguard_chinese");
        let saved = format!("#{disabled}\n{enabled}\n");
        std::fs::write(&file, &saved).unwrap();
        let defaults = defaults_for(["en-US"], "Asia/Shanghai");
        let selected = crate::sources::selection(&file, &defaults).unwrap();
        for id in CHINESE {
            let url = url_of(id);
            let enabled = crate::sources::subscribed(&selected).any(|source| source == url);
            assert_eq!(enabled, url != disabled, "{id}");
            let copies = selected
                .lines()
                .filter(|line| line.trim_start_matches('#') == url)
                .count();
            assert_eq!(copies, 1, "{id}");
        }
        assert_eq!(std::fs::read_to_string(file).unwrap(), saved);
    }
}
