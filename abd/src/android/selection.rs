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
use std::path::Path;

use abd_engine::apps::{Identity, PERSONAL_APP_UIDS, named};
use abd_engine::files;
use anyhow::Result;

use super::classification::{Installed, installed};
use super::profiles::{Profile, Roster, USERS};
use crate::paths::Paths;

pub fn selection(file: &Path) -> std::io::Result<String> {
    files::read_text(file, abd_engine::apps::MAX_FILE)
}

pub fn prune_orphans(paths: &Paths) -> Result<bool> {
    prune_orphans_in(paths, Path::new(USERS))
}

pub(super) fn prune_orphans_in(paths: &Paths, users: &Path) -> Result<bool> {
    fn serial(line: &str, field: usize) -> Option<u32> {
        if line.trim_start().starts_with('#') {
            return None;
        }
        Identity::parse(line.split_ascii_whitespace().nth(field)?)?.profile_serial
    }

    let apps = paths.apps();
    let pauses = paths.pauses();
    // Write pauses first so an interrupted cleanup can retry from the
    // selection still on disk. Both files use the same directory lock.
    let inputs = [
        (&pauses, abd_engine::pause::MAX_FILE, 1),
        (&apps, abd_engine::apps::MAX_FILE, 0),
    ];
    let mut has_work = false;
    for (file, max, field) in inputs {
        if files::read_text(file, max)?
            .lines()
            .any(|line| serial(line, field).is_some())
        {
            has_work = true;
            break;
        }
    }
    if !has_work {
        return Ok(false);
    }

    let _lock = files::lock(&apps)?;
    let roster = Roster::read(users)?;
    let mut changed = false;
    for (file, max, field) in inputs {
        let text = files::read_text(file, max)?;
        let retained: String = text
            .split_inclusive('\n')
            .filter(|line| !serial(line, field).is_some_and(|serial| roster.deleted(serial)))
            .collect();
        if retained.len() != text.len() {
            if retained.is_empty() && *file == pauses {
                files::remove_if_present(file)?;
            } else {
                files::replace(file, retained.as_bytes())?;
            }
            changed = true;
        }
    }
    Ok(changed)
}

pub fn selected_profiles(wanted: &str) -> BTreeSet<u32> {
    named(wanted)
        .into_iter()
        .filter_map(|token| Identity::parse(token)?.profile_serial)
        .collect()
}

pub fn personal_app_identities(exclusions: &BTreeSet<&str>) -> Result<BTreeMap<u32, String>> {
    Ok(all_personal(installed(0)?, exclusions))
}

pub(super) fn all_personal(
    installed: Installed,
    exclusions: &BTreeSet<&str>,
) -> BTreeMap<u32, String> {
    let excluded_uids: BTreeSet<_> = exclusions
        .iter()
        .filter_map(|name| {
            installed
                .selectable
                .get(*name)
                .or_else(|| installed.aliases.get(*name))
                .copied()
        })
        .collect();
    installed
        .selectable
        .into_iter()
        .filter(|(_, uid)| {
            PERSONAL_APP_UIDS.contains(uid)
                && Some(*uid) != installed.own_uid
                && !excluded_uids.contains(uid)
        })
        .map(|(name, uid)| (uid, name))
        .collect()
}

pub fn chosen(
    wanted: &str,
    profiles: &[Profile],
    previous: &BTreeMap<u32, String>,
    scope: abd_engine::settings::Scope,
) -> (BTreeMap<u32, String>, bool) {
    abd_engine::apps::in_scope(
        wanted,
        scope,
        |saved| resolve(saved, profiles, previous, installed),
        |(apps, unavailable), exclusions| match personal_app_identities(exclusions) {
            Ok(personal) => apps.extend(personal),
            Err(_) => *unavailable = true,
        },
    )
}

pub(super) fn resolve(
    wanted: Vec<&str>,
    profiles: &[Profile],
    previous: &BTreeMap<u32, String>,
    installed: impl Fn(u32) -> Result<Installed>,
) -> (BTreeMap<u32, String>, bool) {
    let mut by_user: BTreeMap<u32, Vec<(&str, &str)>> = BTreeMap::new();
    for token in wanted {
        let Some(app) = Identity::parse(token) else {
            continue;
        };
        let id = match app.profile_serial {
            None => 0,
            Some(serial) => {
                let Some(profile) = profiles.iter().find(|p| p.serial == serial && !p.quiet) else {
                    continue;
                };
                profile.id
            }
        };
        by_user.entry(id).or_default().push((token, app.package));
    }
    let mut found = BTreeMap::new();
    let mut unavailable = false;
    for (id, wanted) in by_user {
        let Ok(packages) = installed(id) else {
            unavailable = true;
            continue;
        };
        unavailable |=
            !packages.registry_available && wanted.iter().any(|(_, name)| name.starts_with('@'));
        found.extend(wanted.into_iter().filter_map(|(token, package)| {
            let uid = packages.selectable.get(package).copied().or_else(|| {
                let uid = *packages.retained.get(package)?;
                (previous.get(&uid).is_some_and(|active| active == token)).then_some(uid)
            })?;
            Some((uid, token.to_owned()))
        }));
    }
    (found, unavailable)
}
