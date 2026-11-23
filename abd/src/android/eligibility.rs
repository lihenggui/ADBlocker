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

use abd_engine::protocol::{App, AppEligibility, PersonalEligibility};
use anyhow::{Result, ensure};

use super::classification::{ResolvedPackages, query_resolved_packages};
use super::profiles::{Profile, profiles, queried_unchanged};

pub(super) fn personal_eligibility(resolved: ResolvedPackages) -> PersonalEligibility {
    let groups = resolved.inventory_groups();
    let apps = resolved
        .apps
        .into_iter()
        .map(|(package, resolution)| (package, resolution.eligibility()))
        .collect();
    PersonalEligibility { apps, groups }
}

pub fn print_personal_eligibility(mut out: impl Write) -> Result<()> {
    let resolved = query_resolved_packages(0)?;
    ensure!(
        resolved.shared_registry_available,
        "Android shared-user registry unavailable"
    );
    serde_json::to_writer(&mut out, &personal_eligibility(resolved))?;
    writeln!(out)?;
    Ok(())
}

pub fn report_app_eligibility(apps: &mut [App]) {
    report_eligibility(apps, profiles, query_resolved_packages);
}

pub(super) fn report_eligibility(
    apps: &mut [App],
    mut roster: impl FnMut() -> Result<Vec<Profile>>,
    mut query: impl FnMut(u32) -> Result<ResolvedPackages>,
) {
    let personal = apps
        .iter()
        .any(|app| app.profile_serial.is_none())
        .then(|| query(0).unwrap_or_default());
    let work: BTreeSet<_> = apps.iter().filter_map(|app| app.profile_serial).collect();
    let mut work_apps = BTreeMap::new();
    if !work.is_empty() {
        // An unreadable roster leaves the work apps unavailable.
        let rows = queried_unchanged(|| Ok(roster().unwrap_or_default()), &work, &mut query);
        for (profile, packages) in rows.into_iter().flatten() {
            if let Ok(packages) = packages {
                work_apps.insert(profile.serial, packages);
            }
        }
    }
    for app in apps {
        let packages = match app.profile_serial {
            Some(serial) => work_apps.get(&serial),
            None => personal.as_ref(),
        };
        let eligibility = packages.map_or(AppEligibility::Unavailable, |packages| {
            packages.resolve(&app.package).eligibility()
        });
        app.report_eligibility(eligibility);
    }
}
