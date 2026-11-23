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

use abd_engine::apps::{PER_USER_RANGE, named};
use anyhow::{Result, bail};

use super::classification::{Installed, Resolution, ResolvedPackages, resolved_packages};
use super::eligibility::{personal_eligibility, report_eligibility};
use super::groups::retained_groups;
use super::package_inventory::{SharedUsers, package_owners, shared_users};
use super::profiles::{Profile, profile_inventory, user, user_list};
use super::selection::{all_personal, prune_orphans_in, resolve};
use super::validate::{CanonicalMode, canonicalize, validate_new_apps};
use crate::paths::Paths;

fn profile(id: u32, bytes: &[u8]) -> Option<Profile> {
    user(id, bytes)?.1
}

#[test]
fn reads_only_complete_managed_profiles_of_user_zero() {
    let managed = r#"<user id="10" serialNumber="31" profileGroupId="0" flags="160" type="android.os.usertype.profile.MANAGED"><name>Work</name></user>"#;
    assert_eq!(
        profile(10, managed.as_bytes()),
        Some(Profile {
            id: 10,
            serial: 31,
            quiet: true
        })
    );
    for state in [
        managed.replace("MANAGED", "CLONE"),
        managed.replace("MANAGED", "PRIVATE"),
        managed.replace("profileGroupId=\"0\"", "profileGroupId=\"11\""),
        managed.replace("flags=\"160\"", "flags=\"96\""),
        managed.replace("id=\"10\"", "id=\"10\" partial=\"true\""),
        managed.replace("id=\"10\"", "id=\"10\" preCreated=\"true\""),
    ] {
        assert!(profile(10, state.as_bytes()).is_none(), "{state}");
    }
}

#[test]
fn reads_typed_binary_user_records_and_rejects_truncation() {
    let user = b"ABX\0\x10\x32\xff\xff\x00\x04user\
        \x6f\xff\xff\x00\x02id\x00\x00\x00\x0a\
        \x6f\xff\xff\x00\x0cserialNumber\x00\x00\x00\x1f\
        \x6f\xff\xff\x00\x05flags\x00\x00\x00\xa0\
        \x6f\xff\xff\x00\x0eprofileGroupId\x00\x00\x00\x00\
        \x3f\xff\xff\x00\x04type\xff\xff\x00\x23android.os.usertype.profile.MANAGED\
        \x33\x00\x00\x11";
    assert_eq!(
        profile(10, user),
        Some(Profile {
            id: 10,
            serial: 31,
            quiet: true
        })
    );
    assert!(profile(10, &user[..user.len() - 1]).is_none());
    let members = b"ABX\0\x10\x32\xff\xff\x00\x05users\
        \x6f\xff\xff\x00\x10nextSerialNumber\x00\x00\x00\x20\
        \x32\xff\xff\x00\x04user\x6f\xff\xff\x00\x02id\x00\x00\x00\x00\x33\x00\x02\
        \x32\x00\x02\x6f\x00\x03\x00\x00\x00\x0a\x33\x00\x02\x33\x00\x00\x11";
    assert_eq!(
        user_list(members),
        Some((BTreeSet::from([0, 10]), Some(32)))
    );
}

#[test]
fn only_a_confirmed_deleted_work_profile_loses_its_selection_and_pauses() {
    let data = tempfile::tempdir().unwrap();
    let paths = Paths::new(data.path());
    let users = tempfile::tempdir().unwrap();
    let userlist = users.path().join("userlist.xml");
    let work = users.path().join("10.xml");
    let apps = "# selections\ncom.same\n31:com.same\n40:com.future\n";
    let pauses = "- *\n960000 com.same\n960000 31:com.same\n960000 40:com.future\n";
    std::fs::write(paths.apps(), apps).unwrap();
    std::fs::write(paths.pauses(), pauses).unwrap();
    std::fs::write(paths.stats(), "retained work statistics\n").unwrap();

    // The next serial is an allocation watermark. Future and current
    // serials survive even when the profile cannot currently filter.
    std::fs::write(
        &userlist,
        r#"<users nextSerialNumber="32"><user id="0"/><user id="10"/></users>"#,
    )
    .unwrap();
    assert!(prune_orphans_in(&paths, users.path()).is_err());
    for (flags, partial) in [(32, false), (160, false), (96, false), (32, true)] {
        std::fs::write(&work, format!(r#"<user id="10" serialNumber="31" profileGroupId="0" flags="{flags}" partial="{partial}" type="android.os.usertype.profile.MANAGED"/>"#)).unwrap();
        assert!(!prune_orphans_in(&paths, users.path()).unwrap());
    }
    assert_eq!(std::fs::read_to_string(paths.apps()).unwrap(), apps);
    assert_eq!(std::fs::read_to_string(paths.pauses()).unwrap(), pauses);

    // A truncated or OEM roster lacking the serial watermark cannot
    // prove that the serial has ever belonged to a deleted user.
    std::fs::write(&userlist, r#"<users nextSerialNumber="32"><user id="0"/>"#).unwrap();
    assert!(prune_orphans_in(&paths, users.path()).is_err());
    std::fs::write(&userlist, r#"<users><user id="0"/></users>"#).unwrap();
    assert!(!prune_orphans_in(&paths, users.path()).unwrap());
    assert_eq!(std::fs::read_to_string(paths.apps()).unwrap(), apps);
    assert_eq!(std::fs::read_to_string(paths.pauses()).unwrap(), pauses);

    // The Android user ID may already have been reused with a new
    // serial. Neither that profile nor the personal app is removed.
    std::fs::write(
        &userlist,
        r#"<users nextSerialNumber="33"><user id="0"/><user id="10"/></users>"#,
    )
    .unwrap();
    std::fs::write(
        &work,
        r#"<user id="10" serialNumber="32" profileGroupId="0" flags="32"/>"#,
    )
    .unwrap();
    assert!(prune_orphans_in(&paths, users.path()).unwrap());
    assert!(!prune_orphans_in(&paths, users.path()).unwrap());
    assert_eq!(
        std::fs::read_to_string(paths.apps()).unwrap(),
        "# selections\ncom.same\n40:com.future\n"
    );
    assert_eq!(
        std::fs::read_to_string(paths.pauses()).unwrap(),
        "- *\n960000 com.same\n960000 40:com.future\n"
    );
    assert_eq!(
        std::fs::read_to_string(paths.stats()).unwrap(),
        "retained work statistics\n"
    );
}

#[test]
fn the_inventory_uses_managed_serials_and_package_names() {
    let profile = Profile {
        id: 10,
        serial: 37,
        quiet: false,
    };
    let inventory = profile_inventory(
        || Ok(vec![profile]),
        |_| Ok(REPORTED_PACKAGES.to_owned()),
        || Ok(KNOWN_PACKAGES.to_owned()),
        || Ok(shared_users(WORK_SHARED_XML).unwrap()),
    )
    .unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../../../testdata/protocol/app-profiles.json")).unwrap();
    assert_eq!(serde_json::to_value(inventory).unwrap(), expected);
}

const REPORTED_PACKAGES: &str =
    "package:com.example.news uid:1010001\npackage:com.example.work uid:1010002\n";
const KNOWN_PACKAGES: &str =
    "com.example.news 10001\ncom.example.work 10002\ncom.example.other_user 10002\n";
const WORK_SHARED_XML: &[u8] = br#"<packages>
    <package name="com.example.work" sharedUserId="10002"/>
    <package name="com.example.other_user" sharedUserId="10002"/>
    <shared-user name="com.example.suite" userId="10002"/>
</packages>"#;

#[test]
fn one_failed_query_leaves_healthy_and_empty_sibling_inventories_visible() {
    let roster = || {
        Ok((10..=12)
            .map(|id| Profile {
                id,
                serial: id + 27,
                quiet: false,
            })
            .collect())
    };
    let inventory = profile_inventory(
        roster,
        |id| match id {
            10 => Ok(REPORTED_PACKAGES.to_owned()),
            11 => Ok(String::new()),
            _ => bail!("the package query timed out"),
        },
        || Ok(KNOWN_PACKAGES.to_owned()),
        || Ok(shared_users(WORK_SHARED_XML).unwrap()),
    )
    .unwrap();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../testdata/protocol/app-profiles-partial.json"
    ))
    .unwrap();
    assert_eq!(serde_json::to_value(inventory).unwrap(), expected);
}

#[test]
fn a_recycled_or_new_profile_remains_unavailable_until_its_inventory_is_queried() {
    let first = Profile {
        id: 10,
        serial: 37,
        quiet: false,
    };
    let recycled = Profile {
        serial: 39,
        ..first
    };
    let sibling = Profile {
        id: 11,
        serial: 38,
        quiet: false,
    };
    let added = Profile {
        id: 12,
        serial: 40,
        quiet: false,
    };
    let mut rosters = [vec![first, sibling], vec![recycled, sibling, added]].into_iter();
    let inventory = profile_inventory(
        || Ok(rosters.next().unwrap()),
        |id| {
            Ok(if id == first.id {
                REPORTED_PACKAGES.to_owned()
            } else {
                String::new()
            })
        },
        || Ok(KNOWN_PACKAGES.to_owned()),
        || Ok(shared_users(WORK_SHARED_XML).unwrap()),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(inventory).unwrap(),
        serde_json::json!({
            "profiles": [
                { "serial": 39, "packages": null, "groups": null },
                { "serial": 38, "packages": [], "groups": [] },
                { "serial": 40, "packages": null, "groups": null }
            ]
        })
    );
}

#[test]
fn an_unavailable_shared_inventory_source_fails_the_command() {
    let profile = Profile {
        id: 10,
        serial: 37,
        quiet: false,
    };
    let query = |_| Ok(REPORTED_PACKAGES.to_owned());
    let mut rosters = [
        Ok(vec![profile]),
        Err(anyhow::anyhow!("roster unavailable")),
    ]
    .into_iter();
    assert!(
        profile_inventory(
            || rosters.next().unwrap(),
            query,
            || Ok(KNOWN_PACKAGES.to_owned()),
            || Ok(shared_users(WORK_SHARED_XML).unwrap()),
        )
        .is_err()
    );
    assert!(
        profile_inventory(
            || Ok(vec![profile]),
            query,
            || bail!("package ownership unavailable"),
            || Ok(SharedUsers::default()),
        )
        .is_err()
    );
}

fn resolve_fixture(
    wanted: &str,
    profiles: &[Profile],
    installed: impl Fn(u32) -> Result<(BTreeMap<String, u32>, bool)>,
) -> (BTreeMap<u32, String>, bool) {
    resolve(named(wanted), profiles, &BTreeMap::new(), |user| {
        let (selectable, registry_available) = installed(user)?;
        Ok(Installed {
            selectable,
            retained: BTreeMap::new(),
            registry_available,
            own_uid: None,
            aliases: BTreeMap::new(),
        })
    })
}

#[test]
fn an_unavailable_inventory_leaves_only_that_users_apps_out() {
    let work = Profile {
        id: 10,
        serial: 31,
        quiet: false,
    };
    let installed = |user| match user {
        0 => Ok((BTreeMap::from([("com.same".to_owned(), 10_001)]), true)),
        _ => bail!("Android package manager timed out"),
    };
    let (chosen, unavailable) = resolve_fixture("com.same\n31:com.same\n", &[work], installed);
    assert_eq!(chosen, BTreeMap::from([(10_001, "com.same".to_owned())]));
    assert!(unavailable);
    assert!(!resolve_fixture("", &[work], installed).1);
}

#[test]
fn package_manager_must_report_an_exact_nonshared_application_uid() {
    let list = "package:com.personal uid:10001\n\
                package:com.work uid:1010001\n\
                package:com.shared_a uid:1010002\n\
                package:com.shared_b uid:1010002\n\
                package:com.system uid:1001000\n\
                package:com.sandbox uid:1020001\n";
    let known = "com.personal 10001\ncom.work 10001\ncom.shared_a 10002\ncom.shared_b 10002\n";
    let work = resolved_fixture(10, list, known);
    assert_eq!(work.apps["com.personal"], Resolution::Unavailable);
    assert_eq!(work.apps["com.work"], Resolution::SharedUid);
    assert_eq!(work.apps["com.shared_a"], Resolution::SharedUid);
    assert_eq!(work.apps["com.shared_b"], Resolution::SharedUid);
    assert_eq!(work.apps["com.system"], Resolution::Unavailable);
    assert_eq!(work.apps["com.sandbox"], Resolution::Unavailable);
    let personal = resolved_fixture(0, list, "com.personal 10001\n");
    assert_eq!(personal.apps["com.personal"], Resolution::Eligible(10001));
    assert_eq!(personal.apps["com.work"], Resolution::Unavailable);
}

const PERSONAL_REPORTED: &str = "package:com.example.eligible uid:10001\npackage:com.example.shared uid:10002\npackage:com.example.unknown uid:10003\n";
const ANDROID_OWNERS: &str = "com.example.eligible 10001\ncom.example.shared 10002\ncom.example.hidden 10002\ncom.example.other 10003\n";

const PERSONAL_SHARED_XML: &[u8] = br#"<packages>
    <package name="com.example.shared" sharedUserId="10002"/>
    <package name="com.example.hidden" sharedUserId="10002"/>
    <shared-user name="com.example.suite" userId="10002"/>
</packages>"#;

fn resolved_fixture(user: u32, list: &str, known: &str) -> ResolvedPackages {
    let shared = shared_users(PERSONAL_SHARED_XML).unwrap();
    resolved_packages(user, list, &package_owners(known), Some(&shared))
}

fn fixture(name: &str) -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testdata/protocol")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn personal_eligibility_proves_sharing_without_equating_unknown_ownership_with_sharing() {
    let resolved = resolved_fixture(0, PERSONAL_REPORTED, ANDROID_OWNERS);
    assert_eq!(resolved.apps["com.example.eligible"].uid(), Some(10001));
    assert_eq!(resolved.apps["com.example.shared"].uid(), None);
    assert_eq!(resolved.apps["com.example.unknown"].uid(), None);
    assert_eq!(
        serde_json::to_value(personal_eligibility(resolved)).unwrap(),
        fixture("app-eligibility.json")
    );

    let duplicate = "package:com.example.eligible uid:10001\n";
    let conflicting = "package:com.example.unknown uid:10004\n";
    let resolved = resolved_fixture(
        0,
        &format!("{PERSONAL_REPORTED}{duplicate}{conflicting}"),
        ANDROID_OWNERS,
    );
    assert_eq!(
        resolved.apps["com.example.eligible"],
        Resolution::Eligible(10001)
    );
    assert_eq!(
        resolved.apps["com.example.unknown"],
        Resolution::Unavailable
    );
}

#[test]
fn a_named_singleton_and_later_members_share_one_identity_in_each_scope() {
    let first = "package:com.a uid:10002\n";
    let both = "package:com.a uid:10002\npackage:com.b uid:10002\n";
    let scoped = "package:com.a uid:1010002\n";
    let xml = |second: &str| {
        format!(
            r#"<packages><shared-user name="com.group" userId="10002"/>
           <package name="com.a" sharedUserId="10002"/>{second}</packages>"#
        )
    };
    let singleton = shared_users(xml("").as_bytes()).unwrap();
    let expanded =
        shared_users(xml(r#"<package name="com.b" sharedUserId="10002"/>"#).as_bytes()).unwrap();
    for (known, shared, listed, user, uid, members) in [
        ("com.a 10002\n", &singleton, first, 0, 10002, &["com.a"][..]),
        (
            "com.a 10002\ncom.b 10002\n",
            &expanded,
            both,
            0,
            10002,
            &["com.a", "com.b"][..],
        ),
        (
            "com.a 10002\ncom.b 10002\n",
            &expanded,
            scoped,
            10,
            1010002,
            &["com.a"][..],
        ),
    ] {
        let packages = resolved_packages(user, listed, &package_owners(known), Some(shared));
        assert_eq!(packages.resolve("com.a"), Resolution::SharedUid);
        assert_eq!(packages.resolve("@com.group"), Resolution::Eligible(uid));
        assert_eq!(packages.groups["com.group"].1, members);
    }
}

#[test]
fn only_previously_active_groups_keep_their_uid_while_a_member_is_published() {
    let shared = shared_users(
        br#"<packages>
        <shared-user name="com.group" userId="10002"/>
        <package name="com.a" sharedUserId="10002"/>
    </packages>"#,
    )
    .unwrap();
    let known = package_owners("com.a 10002\ncom.b 10002\n");
    let listed = "package:com.a uid:1010002\npackage:com.b uid:1010002\n";
    assert_eq!(
        shared.still_owns(10002, &known, |_| true),
        Some("com.group")
    );
    let current = resolved_packages(10, listed, &known, Some(&shared));
    assert_eq!(current.resolve("@com.group"), Resolution::Unavailable);
    let retained = retained_groups(10, listed, &known, &shared);
    assert_eq!(retained[&1010002], "@com.group");
    let lagging = package_owners("com.a 10002\n");
    assert_eq!(retained_groups(10, listed, &lagging, &shared), retained);
    let conflicting = package_owners("com.a 10002\ncom.b 10003\n");
    assert!(retained_groups(10, listed, &conflicting, &shared).is_empty());

    let roster = [Profile {
        id: 10,
        serial: 37,
        quiet: false,
    }];
    let wanted = "37:@com.group\n";
    let installed = |_| {
        Ok(Installed {
            selectable: BTreeMap::new(),
            retained: BTreeMap::from([("@com.group".to_owned(), 1010002)]),
            registry_available: true,
            own_uid: None,
            aliases: BTreeMap::new(),
        })
    };
    let enabled = BTreeMap::from([(1010002, "37:@com.group".to_owned())]);
    assert_eq!(
        resolve(named(wanted), &roster, &enabled, installed).0,
        enabled
    );
    assert!(
        resolve(named(wanted), &roster, &BTreeMap::new(), installed)
            .0
            .is_empty()
    );
    let old_serial = BTreeMap::from([(1010002, "36:@com.group".to_owned())]);
    assert!(
        resolve(named(wanted), &roster, &old_serial, installed)
            .0
            .is_empty()
    );
    assert!(retained_groups(10, "package:com.b uid:1010002\n", &known, &shared).is_empty());
    let reused = package_owners("com.b 10002\n");
    assert_eq!(shared.still_owns(10002, &reused, |_| true), None);
}

#[test]
fn aliases_normalize_to_a_named_group_in_the_requested_profile() {
    let query = |user| {
        let uid = user * PER_USER_RANGE + 10_002;
        Ok(resolved_fixture(
            user,
            &format!(
                "package:com.example.shared uid:{uid}\npackage:com.example.hidden uid:{uid}\n"
            ),
            ANDROID_OWNERS,
        ))
    };
    let roster = || {
        Ok(vec![Profile {
            id: 10,
            serial: 37,
            quiet: false,
        }])
    };
    let choices = [
        "com.example.shared",
        "37:com.example.hidden",
        "@com.example.suite",
    ];
    assert_eq!(
        canonicalize(&choices, CanonicalMode::New, roster, query).unwrap(),
        [
            "@com.example.suite",
            "37:@com.example.suite",
            "@com.example.suite"
        ]
    );
    assert!(
        canonicalize(
            &["38:com.example.hidden"],
            CanonicalMode::New,
            roster,
            query
        )
        .is_err()
    );
    assert_eq!(
        canonicalize(
            &["38:com.example.hidden"],
            CanonicalMode::Existing,
            roster,
            query
        )
        .unwrap(),
        ["38:com.example.hidden"]
    );
}

#[test]
fn all_personal_uses_confirmed_app_uids_and_excludes_the_owner_and_named_groups() {
    let installed = Installed {
        selectable: BTreeMap::from([
            ("android".into(), 1000),
            ("com.example.self".into(), 10_000),
            ("@com.example.shared".into(), 10_001),
            ("com.example.implicit".into(), 19_999),
            ("com.example.otheruser".into(), 1_010_002),
        ]),
        retained: BTreeMap::new(),
        registry_available: true,
        own_uid: Some(10_000),
        aliases: BTreeMap::from([("com.example.member".into(), 10_001)]),
    };
    assert_eq!(
        all_personal(installed, &BTreeSet::from(["com.example.member"])),
        BTreeMap::from([(19_999, "com.example.implicit".to_owned())])
    );
}

#[test]
fn reported_shared_apps_are_inactive_while_retaining_their_choices() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::write(
        paths.apps(),
        "com.example.shared\n37:com.example.shared\ncom.example.unknown\n",
    )
    .unwrap();
    let settings = abd_engine::settings::Settings {
        url_filtering: true,
        scope: Default::default(),
        cosmetic: false,
    };
    let mut listed = abd_engine::protocol::apps(&paths, settings, None, &BTreeSet::new(), 0);
    let work = Profile {
        id: 10,
        serial: 37,
        quiet: false,
    };
    report_eligibility(
        &mut listed,
        || Ok(vec![work]),
        |user| {
            let reported = if user == 0 {
                PERSONAL_REPORTED
            } else {
                "package:com.example.shared uid:1010002\n"
            };
            Ok(resolved_fixture(user, reported, ANDROID_OWNERS))
        },
    );
    assert_eq!(
        serde_json::to_value(listed).unwrap(),
        fixture("apps-unavailable.json")
    );
}

#[test]
fn personal_selection_is_validated_without_accessing_work_and_distinguishes_failure() {
    let roster = || panic!("personal validation does not need the work roster");
    let query = |user| {
        assert_eq!(user, 0);
        Ok(resolved_fixture(user, PERSONAL_REPORTED, ANDROID_OWNERS))
    };
    validate_new_apps(&["com.example.eligible"], roster, query).unwrap();
    assert!(
        validate_new_apps(&["com.example.shared"], roster, query)
            .unwrap_err()
            .to_string()
            .contains("shares an Android UID")
    );
    assert!(
        validate_new_apps(&["com.example.unknown"], roster, query)
            .unwrap_err()
            .to_string()
            .contains("eligibility is unavailable")
    );
    assert!(
        validate_new_apps(&["com.example.eligible"], roster, |_| bail!(
            "no Package Manager"
        ))
        .unwrap_err()
        .to_string()
        .contains("inventory unavailable")
    );
}

#[test]
fn selection_writes_reject_new_shared_apps_and_keep_existing_choices_removable() {
    use abd_engine::apps::{Change, Configure, add, configure, edit};

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("apps");
    let saved = "com.example.eligible\n37:com.example.work\n";
    std::fs::write(&file, saved).unwrap();
    let validate = |_: &[&str], added: &[&str]| {
        validate_new_apps(
            added,
            || panic!("retained work needs no lookup"),
            |user| {
                assert_eq!(user, 0);
                Ok(resolved_fixture(user, PERSONAL_REPORTED, ANDROID_OWNERS))
            },
        )
    };
    assert!(add(&file, "com.example.shared", validate).is_err());
    for change in [Configure::Replace, Configure::KeepWork] {
        assert!(configure(&file, "com.example.shared\n", change, validate).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), saved);
    }

    std::fs::write(&file, "com.example.shared\n37:com.example.work\n").unwrap();
    configure(
        &file,
        "com.example.shared\ncom.example.eligible\n",
        Configure::KeepWork,
        validate,
    )
    .unwrap();
    assert!(edit(&file, "com.example.shared", Change::Remove).unwrap());
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "37:com.example.work\ncom.example.eligible\n"
    );
}

#[test]
fn a_personal_choice_never_selects_the_work_copy() {
    let work = Profile {
        id: 10,
        serial: 31,
        quiet: false,
    };
    let installed = |user| {
        Ok((
            BTreeMap::from([("com.same".to_owned(), user * PER_USER_RANGE + 10001)]),
            true,
        ))
    };
    assert_eq!(
        resolve_fixture("com.same\n", &[work], installed).0,
        BTreeMap::from([(10001, "com.same".to_owned())])
    );
    assert_eq!(
        resolve_fixture("31:com.same\n", &[work], installed).0,
        BTreeMap::from([(1010001, "31:com.same".to_owned())])
    );
    assert_eq!(
        resolve_fixture("com.same\n31:com.same\n", &[work], installed)
            .0
            .len(),
        2
    );
}

#[test]
fn quiet_and_recreated_profiles_do_not_resolve_a_saved_choice() {
    let work = Profile {
        id: 10,
        serial: 31,
        quiet: true,
    };
    let installed = |user| {
        Ok((
            BTreeMap::from([("com.same".to_owned(), user * PER_USER_RANGE + 10001)]),
            true,
        ))
    };
    assert!(
        resolve_fixture("31:com.same\n", &[work], installed)
            .0
            .is_empty()
    );
    assert!(
        resolve_fixture("31:com.same\n", &[], installed)
            .0
            .is_empty()
    );
    let recreated = Profile {
        serial: 32,
        quiet: false,
        ..work
    };
    assert!(
        resolve_fixture("31:com.same\n", &[recreated], installed)
            .0
            .is_empty()
    );
}
