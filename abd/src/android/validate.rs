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

use std::collections::{BTreeMap, BTreeSet};

use abd_engine::apps::Identity;
use anyhow::{Context as _, Result, bail, ensure};

use super::classification::{Resolution, ResolvedPackages, query_resolved_packages};
use super::profiles::{Profile, profiles, queried_unchanged};

pub fn validate_selection(tokens: &[&str]) -> Result<()> {
    validate_new_apps(tokens, profiles, query_resolved_packages)
}

pub(super) fn validate_new_apps(
    tokens: &[&str],
    mut roster: impl FnMut() -> Result<Vec<Profile>>,
    mut query: impl FnMut(u32) -> Result<ResolvedPackages>,
) -> Result<()> {
    let mut personal = BTreeSet::new();
    let mut work: BTreeMap<u32, BTreeSet<&str>> = BTreeMap::new();
    for &token in tokens {
        let app = Identity::parse(token).ok_or_else(|| {
            abd_engine::malformed::Malformed(format!("not an app identity: {token:?}"))
        })?;
        if let Some(serial) = app.profile_serial {
            work.entry(serial).or_default().insert(token);
        } else {
            personal.insert(token);
        }
    }
    if !personal.is_empty() {
        validate_packages(
            &personal,
            &query(0).context("personal app inventory unavailable")?,
        )?;
    }
    if work.is_empty() {
        return Ok(());
    }
    let before = roster()?;
    for (&serial, names) in &work {
        let Some(profile) = before.iter().find(|p| p.serial == serial) else {
            bail!("managed profile is unavailable");
        };
        let packages =
            query(profile.id).context("managed profile package inventory unavailable")?;
        validate_packages(names, &packages)?;
    }
    let current = roster()?;
    for profile in before.iter().filter(|p| work.contains_key(&p.serial)) {
        ensure!(
            current
                .iter()
                .any(|p| p.id == profile.id && p.serial == profile.serial),
            "managed profile changed during selection"
        );
    }
    Ok(())
}

fn validate_packages(names: &BTreeSet<&str>, packages: &ResolvedPackages) -> Result<()> {
    for &token in names {
        let app = Identity::parse(token).context("invalid app identity")?;
        match packages.resolve(app.package) {
            Resolution::Eligible(_) if packages.shared_registry_available => {}
            Resolution::SharedUid => {
                bail!("app shares an Android UID with another package: {token}")
            }
            _ => bail!("app eligibility is unavailable: {token}"),
        }
    }
    Ok(())
}

/// Resolves bare regular package aliases before writing their selections.
pub fn canonical_choices(tokens: &[&str]) -> Result<Vec<String>> {
    canonicalize(
        tokens,
        CanonicalMode::New,
        profiles,
        query_resolved_packages,
    )
}

pub fn canonical_choice(token: &str) -> Result<String> {
    canonical_one(token, CanonicalMode::New)
}

pub fn canonical_existing(token: &str) -> Result<String> {
    canonical_one(token, CanonicalMode::Existing)
}

fn canonical_one(token: &str, mode: CanonicalMode) -> Result<String> {
    canonicalize(&[token], mode, profiles, query_resolved_packages)?
        .into_iter()
        .next()
        .context("canonical app identity is missing")
}

#[derive(Clone, Copy)]
pub(super) enum CanonicalMode {
    New,
    Existing,
}

impl CanonicalMode {
    fn roster(self, result: Result<Vec<Profile>>) -> Result<Vec<Profile>> {
        match self {
            Self::New => result,
            Self::Existing => Ok(result.unwrap_or_default()),
        }
    }
}

pub(super) fn canonicalize(
    tokens: &[&str],
    mode: CanonicalMode,
    mut roster: impl FnMut() -> Result<Vec<Profile>>,
    mut query: impl FnMut(u32) -> Result<ResolvedPackages>,
) -> Result<Vec<String>> {
    let identities: Vec<_> = tokens
        .iter()
        .map(|token| {
            Identity::parse(token).ok_or_else(|| {
                abd_engine::malformed::Malformed(format!("not an app identity: {token:?}"))
            })
        })
        .collect::<std::result::Result<_, _>>()?;
    let serials: BTreeSet<_> = identities
        .iter()
        .filter(|identity| !identity.is_group())
        .filter_map(|identity| identity.profile_serial)
        .collect();
    let mut by_scope = BTreeMap::new();
    if identities
        .iter()
        .any(|identity| !identity.is_group() && identity.profile_serial.is_none())
    {
        by_scope.insert(None, query(0));
    }
    if !serials.is_empty() {
        for (profile, resolved) in
            queried_unchanged(|| mode.roster(roster()), &serials, &mut query)?
        {
            by_scope.insert(Some(profile.serial), resolved);
        }
    }
    tokens
        .iter()
        .zip(identities)
        .map(|(&token, identity)| {
            if identity.is_group() {
                return Ok(token.to_owned()); // New choices are still checked under the selection lock.
            }
            if let Some(Ok(resolved)) = by_scope.get(&identity.profile_serial) {
                if let Some(name) = resolved.group_of(identity.package) {
                    return Ok(match identity.profile_serial {
                        Some(serial) => format!("{serial}:@{name}"),
                        None => format!("@{name}"),
                    });
                }
                if resolved.shared_registry_available
                    && resolved.resolve(identity.package).uid().is_some()
                {
                    return Ok(token.to_owned());
                }
            }
            if matches!(mode, CanonicalMode::New) {
                bail!("app eligibility is unavailable: {token}");
            }
            // A saved selection or its history can be removed or inspected
            // even if Android no longer reports the app.
            Ok(token.to_owned())
        })
        .collect()
}
