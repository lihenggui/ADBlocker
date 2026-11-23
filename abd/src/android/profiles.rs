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
use std::io::Write;
use std::path::Path;

use abd_engine::files;
use abd_engine::protocol::{ProfileInventories, ProfileInventory};
use anyhow::{Context as _, Result, bail, ensure};

use super::atomic_file::{CommittedCopy, open_with_backup, uses_resilient_atomic_file};
use super::classification::resolved_packages;
use super::package_inventory::{SharedUsers, package_owners};
use super::package_settings::read_shared_users;
use super::packages::{android_output, package_list, query_packages};

const DISABLED: u32 = 0x40;
const QUIET_MODE: u32 = 0x80;
const LEGACY_MANAGED_PROFILE: u32 = 0x20;
const MANAGED_PROFILE: &str = "android.os.usertype.profile.MANAGED";
pub(super) const USERS: &str = "/data/system/users";

const MAX_USER_RECORDS: usize = 64;
const MAX_USER_XML: usize = 128 << 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub id: u32,
    pub serial: u32,
    pub quiet: bool,
}

impl Profile {
    pub fn started(self) -> Result<bool> {
        if self.quiet {
            return Ok(false);
        }
        let state = android_output(
            &["activity", "get-started-user-state", &self.id.to_string()],
            64,
        )?;
        match state.trim() {
            "RUNNING_LOCKED" | "RUNNING_UNLOCKING" | "RUNNING_UNLOCKED" => Ok(true),
            "BOOTING" | "STOPPING" | "SHUTDOWN" => Ok(false),
            state if state == format!("User is not started: {}", self.id) => Ok(false),
            _ => bail!("unknown Android user state"),
        }
    }
}

/// A roster contains all allocated serials, even for users whose apps
/// cannot be filtered. Absence from the active profiles is not deletion.
pub(super) struct Roster {
    profiles: Vec<Profile>,
    serials: BTreeSet<u32>,
    next_serial: Option<u32>,
}

impl Roster {
    pub(super) fn read(dir: &Path) -> Result<Self> {
        // Android 12–13 AtomicFile writes to .new, then renames it over the
        // primary. Its legacy recovery file is .bak. Android 14+ writes to
        // the primary while ResilientAtomicFile keeps the old .backup.
        let extension = if uses_resilient_atomic_file() {
            "backup"
        } else {
            "bak"
        };
        Self::read_with_backup(dir, extension)
    }

    fn read_with_backup(dir: &Path, backup_extension: &str) -> Result<Self> {
        let read = |file: &Path| {
            let backup = file.with_added_extension(backup_extension);
            // Publication can delete the backup just after opening the
            // primary failed. Retry the opens once to cover that gap.
            let copy =
                open_with_backup(file, &backup).or_else(|_| open_with_backup(file, &backup))?;
            let (CommittedCopy::Backup(file) | CommittedCopy::Primary(file)) = copy;
            // Android can recover from its reserve by deleting bad copies.
            // Keep this read-only lookup fail-closed on the selected copy.
            files::read_file_bytes(file, MAX_USER_XML)
        };
        let file = dir.join("userlist.xml");
        let list = read(&file)?;
        let (mut ids, next_serial) =
            user_list(&list).context("unknown Android user roster format")?;
        ensure!(ids.remove(&0), "Android user roster is missing the owner");
        ensure!(
            ids.len() <= MAX_USER_RECORDS,
            "Android user roster exceeds its inspection limit"
        );
        let mut profiles = Vec::new();
        let mut serials = BTreeSet::new();
        for id in ids {
            let bytes = read(&dir.join(format!("{id}.xml")))?;
            let (serial, profile) = user(id, &bytes).context("incomplete Android user record")?;
            ensure!(
                serials.insert(serial),
                "Android user roster contains a duplicate serial"
            );
            profiles.extend(profile);
        }
        ensure!(
            list == read(&file)?,
            "Android user roster changed while reading it"
        );
        let next_serial =
            next_serial.filter(|&next| serials.last().is_none_or(|&serial| next > serial));
        Ok(Self {
            profiles,
            serials,
            next_serial,
        })
    }

    pub(super) fn deleted(&self, serial: u32) -> bool {
        self.next_serial.is_some_and(|next| serial < next) && !self.serials.contains(&serial)
    }
}

pub fn profiles() -> Result<Vec<Profile>> {
    Ok(Roster::read(Path::new(USERS))?.profiles)
}

pub(super) fn user_list(bytes: &[u8]) -> Option<(BTreeSet<u32>, Option<u32>)> {
    fn unique(ids: impl Iterator<Item = Option<u32>>) -> Option<BTreeSet<u32>> {
        let mut unique = BTreeSet::new();
        for id in ids {
            if !unique.insert(id?) {
                return None;
            }
        }
        Some(unique)
    }
    if bytes.starts_with(crate::abx::MAGIC) {
        let tags = crate::abx::elements(bytes)?;
        let root = tags.first().filter(|root| root.name == b"users")?;
        let next = root
            .attrs
            .get(b"nextSerialNumber".as_slice())
            .and_then(|s| s.parse::<i32>().ok())
            .and_then(|n| u32::try_from(n).ok());
        let ids = unique(
            tags.iter()
                .filter(|e| e.depth == 1 && e.name == b"user")
                .map(|e| e.attrs.get(b"id".as_slice())?.parse().ok()),
        )?;
        return Some((ids, next));
    }
    let doc = roxmltree::Document::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let root = doc.root_element();
    root.has_tag_name("users").then_some(())?;
    let ids = unique(
        root.children()
            .filter(|node| node.has_tag_name("user"))
            .map(|node| node.attribute("id")?.parse().ok()),
    )?;
    let next = root
        .attribute("nextSerialNumber")
        .and_then(|s| s.parse::<i32>().ok())
        .and_then(|n| u32::try_from(n).ok());
    Some((ids, next))
}

pub(super) fn user(id: u32, bytes: &[u8]) -> Option<(u32, Option<Profile>)> {
    fn parse<'a>(
        id: u32,
        attrs: impl Fn(&str) -> Option<&'a str>,
    ) -> Option<(u32, Option<Profile>)> {
        if attrs("id")?.parse::<u32>().ok()? != id {
            return None;
        }
        let serial = u32::try_from(attrs("serialNumber")?.parse::<i32>().ok()?).ok()?;
        (serial != 0).then(|| (serial, profile_attributes(id, serial, attrs)))
    }
    if bytes.starts_with(crate::abx::MAGIC) {
        let tags = crate::abx::elements(bytes)?;
        let root = tags.first().filter(|root| root.name == b"user")?;
        return parse(id, |name| {
            root.attrs.get(name.as_bytes()).map(String::as_str)
        });
    }
    let doc = roxmltree::Document::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let root = doc.root_element();
    root.has_tag_name("user").then_some(())?;
    parse(id, |name| root.attribute(name))
}

fn profile_attributes<'a>(
    id: u32,
    serial: u32,
    get: impl Fn(&str) -> Option<&'a str>,
) -> Option<Profile> {
    let flags: u32 = get("flags")?.parse().ok()?;
    let managed = match get("type") {
        Some(kind) => kind == MANAGED_PROFILE,
        // Android 10 wrote flags before introducing typed users.
        None => flags & LEGACY_MANAGED_PROFILE != 0,
    };
    if !managed
        || get("profileGroupId") != Some("0")
        || flags & DISABLED != 0
        || get("partial").is_some_and(|v| v != "false")
        || get("preCreated").is_some_and(|v| v != "false")
    {
        return None;
    }
    Some(Profile {
        id,
        serial,
        quiet: flags & QUIET_MODE != 0,
    })
}

/// `abd app profiles --json` exposes only the managed children of user 0.
/// Android intersects these package names with its own launcher visibility.
pub fn print_profiles(mut out: impl Write) -> Result<()> {
    let inventory = profile_inventory(profiles, query_packages, package_list, read_shared_users)?;
    serde_json::to_writer(&mut out, &inventory)?;
    writeln!(out)?;
    Ok(())
}

pub(super) fn profile_inventory(
    mut roster: impl FnMut() -> Result<Vec<Profile>>,
    mut query: impl FnMut(u32) -> Result<String>,
    known: impl FnOnce() -> Result<String>,
    shared: impl FnOnce() -> Result<SharedUsers>,
) -> Result<ProfileInventories> {
    let mut rows = BTreeMap::new();
    for profile in roster()? {
        // An unsuccessful query leaves a hole for this user only.
        rows.insert((profile.id, profile.serial), query(profile.id).ok());
    }
    // Read ownership after the individual queries so no package is offered
    // on the strength of an earlier owner.
    let (known, shared) = if rows.is_empty() {
        (String::new(), SharedUsers::default())
    } else {
        (known()?, shared()?)
    };
    let known = package_owners(&known);
    // A serial seen only in the second roster has no verified inventory.
    // Matching both the ID and serial also excludes recycled users.
    let profiles = roster()?
        .into_iter()
        .map(|profile| {
            let resolved = rows
                .remove(&(profile.id, profile.serial))
                .flatten()
                .map(|list| resolved_packages(profile.id, &list, &known, Some(&shared)));
            let (packages, groups) = resolved.map_or((None, None), |resolved| {
                let mut packages: BTreeSet<_> = resolved
                    .apps
                    .iter()
                    .filter(|(_, resolution)| resolution.uid().is_some())
                    .map(|(name, _)| name.clone())
                    .collect();
                let groups = resolved.inventory_groups();
                packages.extend(
                    groups
                        .iter()
                        .flat_map(|group| group.packages.iter().cloned()),
                );
                (Some(packages.into_iter().collect()), Some(groups))
            });
            ProfileInventory {
                serial: profile.serial,
                packages,
                groups,
            }
        })
        .collect();
    Ok(ProfileInventories { profiles })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roster_and_each_record_use_the_committed_copy() {
        const LIST: &str = r#"<users nextSerialNumber="32"><user id="0"/><user id="10"/></users>"#;
        const USER: &str = r#"<user id="10" serialNumber="31" profileGroupId="0" flags="32"/>"#;
        const PARTIAL: &str = "<unfinished";

        for extension in ["backup", "bak"] {
            for (name, committed) in [("userlist.xml", LIST), ("10.xml", USER)] {
                for (primary, backup, available) in [
                    (Some(committed), None, true),
                    (Some(PARTIAL), Some(committed), true),
                    (None, Some(committed), true),
                    (Some(PARTIAL), Some(PARTIAL), false),
                    (Some(committed), Some(PARTIAL), false),
                    (None, None, false),
                ] {
                    let users = tempfile::tempdir().unwrap();
                    let dir = users.path();
                    std::fs::write(dir.join("userlist.xml"), LIST).unwrap();
                    std::fs::write(dir.join("10.xml"), USER).unwrap();
                    let file = dir.join(name);
                    match primary {
                        Some(bytes) => std::fs::write(&file, bytes).unwrap(),
                        None => std::fs::remove_file(&file).unwrap(),
                    }
                    if let Some(bytes) = backup {
                        std::fs::write(file.with_added_extension(extension), bytes).unwrap();
                    }
                    let roster = Roster::read_with_backup(dir, extension);
                    assert_eq!(
                        roster.is_ok(),
                        available,
                        "{name}.{extension}: primary={primary:?}, backup={backup:?}",
                    );
                    if let Ok(roster) = roster {
                        assert_eq!(
                            roster.profiles,
                            [Profile {
                                id: 10,
                                serial: 31,
                                quiet: false
                            }]
                        );
                        assert!(!roster.deleted(31));
                    }
                }
            }
        }
    }
}
