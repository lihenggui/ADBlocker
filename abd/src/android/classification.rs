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

//! Classifies package snapshots for selection and installed app reporting.

use std::collections::BTreeMap;

use abd_engine::apps::{PER_USER_RANGE, PERSONAL_APP_UIDS};
use abd_engine::protocol::{AppEligibility, GroupInventory};
use anyhow::Result;

use super::groups::retained_groups;
use super::package_inventory::{PackageUids, SharedUsers, package_owners, package_uids};
use super::package_settings::read_shared_users;
use super::packages::{package_list, package_snapshot};

pub(super) const COMPANION_PACKAGE: &str = "li.mercury.adblocker";

/// A package of Android's own. A list of packages without it is not the
/// package manager's whole list.
pub(super) const ON_EVERY_DEVICE: &str = "com.android.shell";

/// Whether the app is installed for any user, by Android's list of
/// packages. `None` when the list cannot be read or lacks what every device
/// has: such a list says nothing about the app.
pub fn companion_installed() -> Option<bool> {
    companion_listed(&package_list().ok()?)
}

pub(super) fn companion_listed(packages: &str) -> Option<bool> {
    let known = package_owners(packages).packages;
    known
        .contains_key(ON_EVERY_DEVICE)
        .then(|| known.contains_key(COMPANION_PACKAGE))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Resolution {
    Eligible(u32),
    SharedUid,
    Unavailable,
}

impl Resolution {
    pub(super) fn eligibility(self) -> AppEligibility {
        match self {
            Self::Eligible(_) => AppEligibility::Eligible,
            Self::SharedUid => AppEligibility::SharedUid,
            Self::Unavailable => AppEligibility::Unavailable,
        }
    }

    pub(super) fn uid(self) -> Option<u32> {
        match self {
            Self::Eligible(uid) => Some(uid),
            _ => None,
        }
    }
}

/// Stable owners used to retire kernel capture when the Android package
/// database changes. A numeric collision never creates an identity.
pub fn known_packages() -> Result<BTreeMap<u32, String>> {
    let known = package_list()?;
    let owners = package_owners(&known);
    let shared = read_shared_users().ok();
    Ok(owners
        .owners
        .iter()
        .filter_map(|(&id, packages)| {
            if !PERSONAL_APP_UIDS.contains(&id) {
                return None;
            }
            if let Some(name) = shared
                .as_ref()
                .and_then(|shared| shared.still_owns(id, &owners, |_| true))
            {
                return Some((id, format!("@{name}")));
            }
            let package = *packages.first()?;
            (packages.len() == 1
                && owners.packages.get(package) == Some(&Some(id))
                && !shared.as_ref().is_some_and(|shared| shared.declared(id)))
            .then(|| (id, package.to_owned()))
        })
        .collect())
}

#[derive(Default)]
pub(super) struct ResolvedPackages {
    pub(super) apps: BTreeMap<String, Resolution>,
    pub(super) groups: BTreeMap<String, (u32, Vec<String>)>,
    pub(super) shared_registry_available: bool,
}

impl ResolvedPackages {
    pub(super) fn resolve(&self, name: &str) -> Resolution {
        if let Some(name) = name.strip_prefix('@') {
            return self
                .groups
                .get(name)
                .map_or(Resolution::Unavailable, |(uid, _)| {
                    Resolution::Eligible(*uid)
                });
        }
        self.apps
            .get(name)
            .copied()
            .unwrap_or(Resolution::Unavailable)
    }

    pub(super) fn group_of(&self, package: &str) -> Option<&str> {
        self.groups.iter().find_map(|(name, (_, members))| {
            members
                .binary_search_by(|member| member.as_str().cmp(package))
                .is_ok()
                .then_some(name.as_str())
        })
    }

    pub(super) fn inventory_groups(&self) -> Vec<GroupInventory> {
        self.groups
            .iter()
            .map(|(id, (_, packages))| GroupInventory {
                id: id.clone(),
                packages: packages.clone(),
            })
            .collect()
    }
}

pub(super) fn resolved_packages(
    user: u32,
    list: &str,
    known: &PackageUids<'_>,
    shared: Option<&SharedUsers>,
) -> ResolvedPackages {
    let reported = package_uids(list, user);
    let mut resolved = ResolvedPackages {
        shared_registry_available: shared.is_some(),
        ..Default::default()
    };
    for (&package, &uid) in &reported.packages {
        let resolution = uid
            .filter(|uid| known.packages.get(package) == Some(&Some(uid % PER_USER_RANGE)))
            .map_or(Resolution::Unavailable, |uid| {
                let id = uid % PER_USER_RANGE;
                if reported.owners[&uid].len() > 1
                    || known.owners[&id].len() > 1
                    || shared.is_some_and(|shared| shared.declared(id))
                {
                    Resolution::SharedUid
                } else {
                    Resolution::Eligible(uid)
                }
            });
        resolved.apps.insert(package.to_owned(), resolution);
    }
    if let Some(shared) = shared {
        for (uid, members) in reported.owners {
            let id = uid % PER_USER_RANGE;
            let Some(name) = shared.confirmed(id, known) else {
                continue;
            };
            // Every installed member reported for this user must have its
            // exact Android UID and belong to the same global shared user.
            // Other-user installations need not be present in this scope.
            if members.iter().all(|&member| {
                reported.packages.get(member) == Some(&Some(uid))
                    && known.packages.get(member) == Some(&Some(id))
            }) {
                resolved.groups.insert(
                    name.to_owned(),
                    (uid, members.into_iter().map(str::to_owned).collect()),
                );
            }
        }
    }
    resolved
}

pub(super) fn query_resolved_packages(user: u32) -> Result<ResolvedPackages> {
    let (list, known, shared) = package_snapshot(user)?;
    Ok(resolved_packages(
        user,
        &list,
        &package_owners(&known),
        shared.as_ref(),
    ))
}

pub(super) struct Installed {
    pub(super) selectable: BTreeMap<String, u32>,
    pub(super) retained: BTreeMap<String, u32>,
    pub(super) registry_available: bool,
    pub(super) own_uid: Option<u32>,
    pub(super) aliases: BTreeMap<String, u32>,
}

pub(super) fn installed(user: u32) -> Result<Installed> {
    let (list, known, shared) = package_snapshot(user)?;
    let aliases: BTreeMap<_, _> = package_uids(&list, user)
        .packages
        .into_iter()
        .filter_map(|(name, uid)| Some((name.to_owned(), uid?)))
        .collect();
    let own_uid = aliases.get(COMPANION_PACKAGE).copied();
    let known = package_owners(&known);
    let packages = resolved_packages(user, &list, &known, shared.as_ref());
    let mut selectable: BTreeMap<_, _> = packages
        .apps
        .into_iter()
        .filter_map(|(package, resolution)| Some((package, resolution.uid()?)))
        .collect();
    selectable.extend(
        packages
            .groups
            .into_iter()
            .map(|(name, (uid, _))| (format!("@{name}"), uid)),
    );
    let retained = shared
        .map(|shared| retained_groups(user, &list, &known, &shared))
        .into_iter()
        .flatten()
        .map(|(uid, group)| (group, uid))
        .collect();
    Ok(Installed {
        selectable,
        retained,
        registry_available: packages.shared_registry_available,
        own_uid,
        aliases,
    })
}
