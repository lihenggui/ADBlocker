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

//! Single-country entries for catalog regions from IANA tzdb 2026f,
//! zone1970.tab, plus their aliases from backward. Android setup uses the
//! same table because ICU can assign a country to a shared zone.

const ZONES: &str = include_str!("data/regional-time-zones.tsv");

pub(super) fn region(zone: &str) -> Option<&'static str> {
    entries().find_map(|(id, country)| (zone == id).then_some(country))
}

fn entries() -> impl Iterator<Item = (&'static str, &'static str)> {
    ZONES
        .lines()
        .map(|line| line.split_once('\t').expect("zone and country"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn catalog_regions_are_covered_by_the_table_or_share_all_their_zones() {
        // These countries have no single-country entry in zone1970.tab.
        let shared_only = ["BE", "CH", "DE", "FR", "IT", "JP", "LU", "NL", "PA", "PR"];
        let catalog: BTreeSet<_> = super::super::catalog()
            .lists
            .into_iter()
            .flat_map(|entry| entry.default_regions)
            .collect();
        let covered: BTreeSet<_> = entries().map(|(_, code)| code).collect();
        for code in ["CN", "HK", "MO", "TW"] {
            assert!(covered.contains(code), "{code} has a single-country zone");
        }
        assert!(shared_only.iter().all(|code| !covered.contains(code)));
        assert_eq!(catalog, covered.into_iter().chain(shared_only).collect());
    }

    #[test]
    fn the_table_has_one_country_per_zone_and_matches_the_shared_cases() {
        let mut ids = BTreeSet::new();
        for (id, _) in entries() {
            assert!(ids.insert(id), "duplicate zone: {id}");
        }
        let cases = include_str!("../../../testdata/regional-time-zone-cases.tsv");
        for line in cases.lines() {
            let (id, expected) = line.split_once('\t').unwrap();
            assert_eq!(region(id), (expected != "-").then_some(expected), "{id}");
        }
    }
}
