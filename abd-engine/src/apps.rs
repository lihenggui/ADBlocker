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

//! The apps whose traffic is filtered, as the user chose them.
//!
//! The file names one app per line. Which UID a package has on the device
//! is for whoever brings its connections to find out.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Result, bail};

/// The largest saved selection, including both chosen apps and exclusions.
pub const MAX_FILE: usize = 16 << 20;

/// Android's normal application UID range within a user.
pub const PERSONAL_APP_UIDS: std::ops::RangeInclusive<u32> = 10_000..=19_999;
/// A full UID is the Android user ID times this range plus the app ID.
pub const PER_USER_RANGE: u32 = 100_000;

/// The valid app identities in file order.
pub fn named(text: &str) -> Vec<&str> {
    let mut seen = BTreeSet::new();
    text.lines()
        .filter_map(|line| {
            let package = line.trim_ascii();
            (Identity::parse(package).is_some() && seen.insert(package)).then_some(package)
        })
        .collect()
}

/// Resolve saved choices in scope, adding the host's personal apps in All apps.
pub fn in_scope<'a, T>(
    text: &'a str,
    scope: crate::settings::Scope,
    resolve_saved: impl FnOnce(Vec<&'a str>) -> T,
    add_personal: impl FnOnce(&mut T, &BTreeSet<&'a str>),
) -> T {
    let mut saved = named(text);
    if scope == crate::settings::Scope::All {
        saved.retain(|name| Identity::parse(name).is_some_and(|id| id.profile_serial.is_some()));
    }
    let mut selected = resolve_saved(saved);
    if scope == crate::settings::Scope::All {
        add_personal(&mut selected, &excluded(text));
    }
    selected
}

/// Personal exclusions, by the package or named group that owns the UID.
pub fn excluded(text: &str) -> BTreeSet<&str> {
    text.lines()
        .filter_map(|line| line.trim_ascii().strip_prefix('!'))
        .filter(|name| Identity::parse(name).is_some_and(|id| id.profile_serial.is_none()))
        .collect()
}

/// What Android allows in a package name, and so nothing that could start
/// a comment or a second line in the file.
pub fn valid_package(package: &str) -> bool {
    !package.is_empty()
        && package.len() <= 255
        && package
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
}

/// A package or a named shared UID in the personal user or a managed profile.
/// The serial outlives a reboot and differs even when Android reuses a user ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity<'a> {
    /// Bare Android package name or the `@sharedUserId` group key.
    pub package: &'a str,
    pub profile_serial: Option<u32>,
}

impl<'a> Identity<'a> {
    pub fn parse(token: &'a str) -> Option<Self> {
        let (package, profile_serial) = if let Some((serial, package)) = token.split_once(':') {
            if serial.starts_with('0') || !serial.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let serial = serial.parse::<i32>().ok()?;
            (package, Some(u32::try_from(serial).ok()?))
        } else {
            (token, None)
        };
        valid_package(package.strip_prefix('@').unwrap_or(package)).then_some(Self {
            package,
            profile_serial,
        })
    }

    /// Android's sharedUserId name, independent of its current numeric UID or members.
    pub fn group_id(self) -> Option<&'a str> {
        self.package.strip_prefix('@')
    }

    pub fn is_group(self) -> bool {
        self.group_id().is_some()
    }
}

pub enum Configure {
    Replace,
    KeepWork,
}

/// Changes the selection in one write, validating new
/// identities against the state held by the same lock as the write.
pub fn configure(
    file: &Path,
    selection: &str,
    change: Configure,
    validate: impl FnOnce(&[&str], &[&str]) -> Result<()>,
) -> Result<()> {
    let mut packages = selection_packages(selection)?;
    let _lock = crate::files::lock(file)?;
    let previous = crate::files::read_text(file, MAX_FILE)?;
    let existing: BTreeSet<_> = named(&previous).into_iter().collect();
    let added: Vec<_> = packages
        .iter()
        .copied()
        .filter(|name| !existing.contains(name))
        .collect();
    validate(&packages.iter().copied().collect::<Vec<_>>(), &added)?;
    if matches!(change, Configure::KeepWork) {
        packages.extend(
            existing.iter().copied().filter(|name| {
                Identity::parse(name).is_some_and(|app| app.profile_serial.is_some())
            }),
        );
    }
    write_selection(file, packages, &excluded(&previous), MAX_FILE)
}

fn selection_packages(selection: &str) -> Result<BTreeSet<&str>> {
    let packages = selection.lines().collect::<BTreeSet<_>>();
    if let Some(package) = packages.iter().find(|name| Identity::parse(name).is_none()) {
        bail!(crate::malformed::Malformed(format!(
            "not an app identity: {package:?}"
        )));
    }
    Ok(packages)
}

pub fn configure_exclusions(file: &Path, exclusion_list: &str) -> Result<()> {
    let exclusions = selection_packages(exclusion_list)?;
    for name in &exclusions {
        validate_personal_exclusion(name)?;
    }
    let _lock = crate::files::lock(file)?;
    let text = crate::files::read_text(file, MAX_FILE)?;
    write_selection(
        file,
        named(&text).into_iter().collect(),
        &exclusions,
        MAX_FILE,
    )
}

pub fn set_excluded(file: &Path, package: &str, exclude: bool) -> Result<()> {
    validate_personal_exclusion(package)?;
    let _lock = crate::files::lock(file)?;
    let text = crate::files::read_text(file, MAX_FILE)?;
    let mut exclusions = excluded(&text);
    if exclude {
        exclusions.insert(package);
    } else {
        exclusions.remove(package);
    }
    write_selection(
        file,
        named(&text).into_iter().collect(),
        &exclusions,
        MAX_FILE,
    )
}

fn validate_personal_exclusion(name: &str) -> Result<()> {
    if !Identity::parse(name).is_some_and(|id| id.profile_serial.is_none()) {
        bail!(crate::malformed::Malformed(
            "only personal apps can be excluded".to_owned()
        ));
    }
    Ok(())
}

fn write_selection(
    file: &Path,
    packages: BTreeSet<&str>,
    exclusions: &BTreeSet<&str>,
    max: usize,
) -> Result<()> {
    let mut text = String::new();
    for (package, prefix) in packages
        .into_iter()
        .map(|app| (app, ""))
        .chain(exclusions.iter().map(|&app| (app, "!")))
    {
        if text.len() + prefix.len() + package.len() + 1 > max {
            bail!(crate::malformed::Malformed(
                "app selection exceeds its size limit".to_owned()
            ));
        }
        text.push_str(prefix);
        text.push_str(package);
        text.push('\n');
    }
    crate::files::replace_durable(file, text.as_bytes())?;
    Ok(())
}

/// What to change of an app in the file.
pub enum Change {
    Add,
    Remove,
}

/// Changes the file for `package`. Returns whether the package was named
/// in it before. An app added keeps its place if it was there.
pub fn edit(file: &Path, package: &str, change: Change) -> Result<bool> {
    edit_checked(file, package, change, |_, _| Ok(()))
}

pub fn add(
    file: &Path,
    package: &str,
    validate: impl FnOnce(&[&str], &[&str]) -> Result<()>,
) -> Result<bool> {
    edit_checked(file, package, Change::Add, validate)
}

fn edit_checked(
    file: &Path,
    package: &str,
    change: Change,
    validate: impl FnOnce(&[&str], &[&str]) -> Result<()>,
) -> Result<bool> {
    if Identity::parse(package).is_none() {
        bail!(crate::malformed::Malformed(format!(
            "not an app identity: {package:?}"
        )));
    }
    let _lock = crate::files::lock(file)?;
    let text = crate::files::read_text(file, MAX_FILE)?;
    let is = |line: &str| line.trim_ascii() == package;
    let was = text.lines().any(is);
    validate(
        &[package],
        if was {
            &[]
        } else {
            std::slice::from_ref(&package)
        },
    )?;
    let add = matches!(change, Change::Add);
    let mut lines = Vec::new();
    for line in text.lines() {
        if !is(line) {
            lines.push(line);
            continue;
        }
        if add {
            lines.push(package);
        }
    }
    if add && !was {
        lines.push(package);
    }
    crate::files::replace_durable(file, (lines.join("\n") + "\n").as_bytes())?;
    Ok(was)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_exclusions_and_chosen_work_apps_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("apps");
        std::fs::write(&file, "com.old\n37:com.work\n!@com.group\n").unwrap();
        configure(&file, "com.new", Configure::KeepWork, |_, _| Ok(())).unwrap();
        set_excluded(&file, "com.new", true).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(named(&text), ["37:com.work", "com.new"]);
        assert_eq!(excluded(&text), BTreeSet::from(["@com.group", "com.new"]));
        assert!(set_excluded(&file, "37:com.work", true).is_err());
        set_excluded(&file, "com.new", false).unwrap();
        assert_eq!(
            excluded(&std::fs::read_to_string(&file).unwrap()),
            BTreeSet::from(["@com.group"])
        );
    }

    #[test]
    fn a_named_group_has_a_stable_key_in_exactly_one_profile() {
        for (key, serial) in [("@com.shared", None), ("37:@com.shared", Some(37))] {
            let identity = Identity::parse(key).unwrap();
            assert_eq!(identity.group_id(), Some("com.shared"));
            assert_eq!(identity.profile_serial, serial);
            assert!(!valid_package(key));
        }
        assert!(!Identity::parse("com.shared").unwrap().is_group());
        for bad in [
            "@",
            "37:@@com.shared",
            "@com.a+com.b",
            "@com.a com.b",
            "0:@com.shared",
        ] {
            assert!(Identity::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_merged_selection_at_the_limit_is_written_but_an_oversize_merge_keeps_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("apps");
        let previous = "37:com.work\n";
        let combined = "37:com.work\ncom.new\n";
        std::fs::write(&file, previous).unwrap();
        let packages = BTreeSet::from(["37:com.work", "com.new"]);
        assert!(
            write_selection(
                &file,
                packages.clone(),
                &BTreeSet::new(),
                combined.len() - 1
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), previous);
        write_selection(&file, packages, &BTreeSet::new(), combined.len()).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), combined);
    }

    #[test]
    fn setup_replaces_personal_choices_and_validates_only_new_choices() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("apps");
        std::fs::write(&file, "com.old\ncom.stay\n37:com.work\n38:com.work\n").unwrap();
        configure(
            &file,
            "com.stay\ncom.new\n37:com.work\n39:com.new",
            Configure::KeepWork,
            |_, added| {
                assert_eq!(added, ["39:com.new", "com.new"]);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "37:com.work\n38:com.work\n39:com.new\ncom.new\ncom.stay\n"
        );

        configure(&file, "com.personal", Configure::KeepWork, |_, added| {
            assert_eq!(added, ["com.personal"]);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "37:com.work\n38:com.work\n39:com.new\ncom.personal\n"
        );
    }

    #[test]
    fn rejected_work_choices_leave_the_previous_selection_intact() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("apps");
        let original = "com.old\n37:com.work\n";
        std::fs::write(&file, original).unwrap();
        assert!(
            configure(
                &file,
                "com.new\n39:com.new",
                Configure::KeepWork,
                |_, _| bail!("profile unavailable")
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    }

    #[test]
    fn replacing_the_selection_drops_unchosen_apps() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("apps");
        std::fs::write(&file, "com.keep\ncom.remove\n").unwrap();
        configure(
            &file,
            "com.keep\ncom.new",
            Configure::Replace,
            |_, _| Ok(()),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "com.keep\ncom.new\n"
        );

        assert!(
            configure(
                &file,
                "com.new\ninvalid/name",
                Configure::Replace,
                |_, _| Ok(())
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "com.keep\ncom.new\n"
        );
        configure(&file, "", Configure::Replace, |_, _| Ok(())).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "");
    }

    #[test]
    fn only_lines_with_a_single_identity_are_app_choices() {
        let text = "com.a\ncom.b hosts\ncom.c full\n# com.d\n  37:@com.work  \ncom.a\n";
        assert_eq!(named(text), ["com.a", "37:@com.work"]);
    }

    #[test]
    fn apps_added_at_the_same_moment_are_all_added() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("apps");
        std::thread::scope(|s| {
            for n in 0..16 {
                let file = &file;
                s.spawn(move || edit(file, &format!("com.app{n}"), Change::Add).unwrap());
            }
        });
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(named(&text).len(), 16, "{text}");
    }

    #[test]
    fn an_app_keeps_its_place_when_added_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("apps");
        std::fs::write(&file, "com.a\ncom.b\n").unwrap();
        assert!(edit(&file, "com.a", Change::Add).unwrap());
        assert!(!edit(&file, "com.c", Change::Add).unwrap());
        assert!(edit(&file, "com.b", Change::Remove).unwrap());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "com.a\ncom.c\n");
        assert!(edit(&file, "com.a\nx", Change::Add).is_err());
    }
}
