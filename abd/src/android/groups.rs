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

use std::collections::BTreeMap;

use abd_engine::apps::{Identity, PER_USER_RANGE};

use super::package_inventory::{PackageUids, SharedUsers, package_owners, package_uids};
use super::package_settings::read_shared_users;
use super::packages::{package_list, package_snapshot};

/// Revalidate local membership as soon as Android publishes a scope
/// change, without affecting other users' selected groups.
pub fn verified_groups(user: u32) -> BTreeMap<u32, String> {
    let snapshot = || {
        let (reported, known, shared) = package_snapshot(user).ok()?;
        Some(retained_groups(
            user,
            &reported,
            &package_owners(&known),
            &shared?,
        ))
    };
    snapshot().unwrap_or_default()
}

pub(super) fn retained_groups(
    user: u32,
    list: &str,
    known: &PackageUids<'_>,
    shared: &SharedUsers,
) -> BTreeMap<u32, String> {
    let reported = package_uids(list, user);
    reported
        .owners
        .iter()
        .filter_map(|(&uid, packages)| {
            let id = uid % PER_USER_RANGE;
            if !packages.iter().all(|&package| {
                known
                    .packages
                    .get(package)
                    .is_none_or(|reported| *reported == Some(id))
                    && reported.packages.get(package) == Some(&Some(uid))
            }) {
                return None;
            }
            let name = shared.still_owns(id, known, |package| {
                reported.packages.get(package) == Some(&Some(uid))
            })?;
            Some((uid, format!("@{name}")))
        })
        .collect()
}

/// The personal packages owning this UID, only while Android still
/// identifies it by the group we were asked to mount into.
pub fn personal_group_members(uid: u32, token: &str) -> Option<Vec<String>> {
    let name = Identity::parse(token)?.group_id()?;
    if uid >= PER_USER_RANGE {
        return None;
    }
    let known = package_list().ok()?;
    let owners = package_owners(&known);
    let shared = read_shared_users().ok()?;
    (shared.still_owns(uid, &owners, |_| true)? == name).then(|| {
        owners.owners[&uid]
            .iter()
            .map(|&package| package.to_owned())
            .collect()
    })
}
