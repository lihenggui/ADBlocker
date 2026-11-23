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

//! Resolves selected app identities to the full Android UIDs they have now.

mod atomic_file;
mod classification;
mod eligibility;
mod groups;
mod package_inventory;
pub(crate) mod package_settings;
mod packages;
mod profiles;
mod selection;
mod validate;

pub use classification::{companion_installed, known_packages};
pub use eligibility::{print_personal_eligibility, report_app_eligibility};
pub use groups::{personal_group_members, verified_groups};
pub use profiles::{Profile, print_profiles, profiles};
pub use selection::{chosen, personal_app_identities, prune_orphans, selected_profiles, selection};
pub use validate::{canonical_choice, canonical_choices, canonical_existing, validate_selection};

#[cfg(test)]
mod tests;
