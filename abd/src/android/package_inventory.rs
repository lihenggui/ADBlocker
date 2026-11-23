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

//! Package and shared-user evidence before scope or selection policy.

use std::collections::{BTreeMap, BTreeSet};

use abd_engine::apps::{PER_USER_RANGE, PERSONAL_APP_UIDS, valid_package};

#[derive(Default)]
pub(super) struct PackageUids<'a> {
    pub(super) packages: BTreeMap<&'a str, Option<u32>>,
    pub(super) owners: BTreeMap<u32, BTreeSet<&'a str>>,
}

impl<'a> PackageUids<'a> {
    fn add(&mut self, package: &'a str, uid: Option<u32>) {
        self.packages
            .entry(package)
            .and_modify(|old| {
                if *old != uid {
                    *old = None;
                }
            })
            .or_insert(uid);
        if let Some(uid) = uid {
            self.owners.entry(uid).or_default().insert(package);
        }
    }
}

pub(super) fn package_uids(output: &str, user: u32) -> PackageUids<'_> {
    let mut packages = PackageUids::default();
    for line in output.lines() {
        let Some((package, uid)) = line
            .strip_prefix("package:")
            .and_then(|line| line.split_once(" uid:"))
        else {
            continue;
        };
        if valid_package(package) {
            let uid = uid.parse::<u32>().ok().filter(|uid| {
                uid / PER_USER_RANGE == user && PERSONAL_APP_UIDS.contains(&(uid % PER_USER_RANGE))
            });
            packages.add(package, uid);
        }
    }
    packages
}

/// A package hidden from a user can still share the UID of a visible app.
/// Confirm each app ID against Android's package database across users.
pub(super) fn package_owners(packages: &str) -> PackageUids<'_> {
    let mut ids = PackageUids::default();
    for line in packages.lines() {
        let mut fields = line.split_ascii_whitespace();
        let (Some(name), Some(id)) = (fields.next(), fields.next().and_then(|id| id.parse().ok()))
        else {
            continue;
        };
        if valid_package(name) {
            ids.add(name, Some(id));
        }
    }
    ids
}

#[derive(Default)]
pub(super) struct SharedUsers {
    names: BTreeMap<u32, String>,
    members: BTreeMap<u32, BTreeSet<String>>,
}

impl SharedUsers {
    fn add(&mut self, tag: &[u8], name: Option<&str>, id: Option<&str>) -> Option<()> {
        let id: u32 = match (tag, id) {
            (b"shared-user", id) => id?.parse().ok()?,
            (b"package", Some(id)) => id.parse().ok()?,
            _ => return Some(()),
        };
        if !PERSONAL_APP_UIDS.contains(&id) {
            return Some(());
        }
        let name = name.filter(|name| valid_package(name))?;
        if tag == b"shared-user" {
            if self.names.insert(id, name.to_owned()).is_some() {
                return None;
            }
        } else if !self.members.entry(id).or_default().insert(name.to_owned()) {
            return None;
        }
        Some(())
    }

    pub(super) fn confirmed<'a>(&'a self, id: u32, known: &PackageUids<'_>) -> Option<&'a str> {
        let name = self.names.get(&id)?;
        let members = self.members.get(&id)?;
        let reported = known.owners.get(&id)?;
        (reported.len() == members.len()
            && reported.iter().all(|&package| {
                members.contains(package) && known.packages.get(package) == Some(&Some(id))
            }))
        .then_some(name)
    }

    // Android cannot reassign an app ID while this registered member
    // still owns it. Use this evidence only to keep an existing capture
    // through publication of another member, never to arm a new one.
    pub(super) fn still_owns(
        &self,
        id: u32,
        known: &PackageUids<'_>,
        mut in_scope: impl FnMut(&str) -> bool,
    ) -> Option<&str> {
        let name = self.names.get(&id)?;
        self.members
            .get(&id)?
            .iter()
            .any(|package| {
                known.packages.get(package.as_str()) == Some(&Some(id)) && in_scope(package)
            })
            .then_some(name)
    }

    pub(super) fn declared(&self, id: u32) -> bool {
        self.names.contains_key(&id) || self.members.contains_key(&id)
    }
}

pub(super) fn shared_users(bytes: &[u8]) -> Option<SharedUsers> {
    let mut users = SharedUsers::default();
    if bytes.starts_with(crate::abx::MAGIC) {
        let tags = crate::abx::package_elements(bytes)?;
        tags.first().filter(|root| root.name == b"packages")?;
        for element in tags.into_iter().filter(|element| element.depth == 1) {
            let id = if element.name == b"shared-user" {
                &b"userId"[..]
            } else {
                &b"sharedUserId"[..]
            };
            users.add(
                element.name,
                element.attrs.get(b"name".as_slice()).map(String::as_str),
                element.attrs.get(id).map(String::as_str),
            )?;
        }
    } else {
        let doc = roxmltree::Document::parse(std::str::from_utf8(bytes).ok()?).ok()?;
        let root = doc.root_element();
        root.has_tag_name("packages").then_some(())?;
        for element in root.children().filter(roxmltree::Node::is_element) {
            let tag = element.tag_name().name();
            let id = if tag == "shared-user" {
                "userId"
            } else {
                "sharedUserId"
            };
            users.add(
                tag.as_bytes(),
                element.attribute("name"),
                element.attribute(id),
            )?;
        }
    }
    let names: BTreeSet<_> = users.names.values().collect();
    let packages: BTreeSet<_> = users.members.values().flatten().collect();
    (names.len() == users.names.len()
        && packages.len() == users.members.values().map(BTreeSet::len).sum::<usize>())
    .then_some(users)
}
