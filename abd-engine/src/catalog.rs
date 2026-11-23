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
use std::sync::LazyLock;

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
    #[serde(default)]
    trusted_scriptlets: bool,
}

const DATA: &str = include_str!("../../config/filter-catalog.json");

fn catalog() -> Catalog<'static> {
    serde_json::from_str(DATA).expect("the bundled filter catalog is valid")
}

fn source_text(urls: impl IntoIterator<Item = &'static str>) -> String {
    urls.into_iter().map(|url| format!("{url}\n")).collect()
}

static SOURCES: LazyLock<String> = LazyLock::new(|| {
    source_text(
        catalog()
            .lists
            .into_iter()
            .filter(|entry| entry.default_enabled)
            .map(|entry| entry.url),
    )
});
static EXCLUSIONS: LazyLock<String> = LazyLock::new(|| source_text(catalog().https_exclusions));

static TRUSTED: LazyLock<Vec<&str>> = LazyLock::new(|| {
    catalog()
        .lists
        .into_iter()
        .filter(|entry| entry.trusted_scriptlets)
        .map(|entry| entry.url)
        .collect()
});

pub(crate) fn trusted_scriptlet_urls() -> &'static [&'static str] {
    &TRUSTED
}

pub(crate) fn cache_identity() -> &'static str {
    DATA
}

/// Standalone defaults when the user has not saved subscriptions.
pub fn default_sources() -> &'static str {
    &SOURCES
}

pub(crate) fn exclusion_sources() -> &'static str {
    &EXCLUSIONS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_shared_catalog_contains_distinct_https_sources_and_uses_only_general_defaults() {
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
                assert!(entry.get("regions").is_none());
                assert_ne!(entry["category"], "Regional");
            }
        }
        assert!(!default_sources().is_empty());
        assert!(!exclusion_sources().is_empty());
    }
}
