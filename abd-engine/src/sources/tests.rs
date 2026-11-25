// SPDX-License-Identifier: GPL-3.0-only
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

use super::*;
use crate::list_url::MAX_URL;
use crate::list_url::{file_name, untrusted_file_name};

/// The lines that turn off each default but those of `named`, which
/// the file has lines of its own for.
pub(crate) fn defaults_off(named: &[&str]) -> String {
    subscribed(crate::catalog::default_sources())
        .filter(|url| !named.contains(url))
        .map(|url| format!("#{url}\n"))
        .collect()
}

const A: &str = "https://a.example/list.txt";
const B: &str = "https://b.example/other.txt";

#[test]
fn a_default_that_a_saved_selection_does_not_name_is_on_until_it_is_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let defaults: Vec<_> = subscribed(crate::catalog::default_sources()).collect();
    let (first, rest) = defaults.split_first().unwrap();
    std::fs::write(paths.sources(), format!("{A}\n#{first}")).unwrap();
    let on = |url: &str| read(&paths).iter().any(|s| s.url == url && s.enabled);
    assert!(on(A) && !on(first));
    assert!(rest.iter().all(|url| on(url)));
    assert_eq!(read(&paths).len(), defaults.len() + 1);

    // Removed, it is not a default the file does not name.
    let removed = rest[0];
    edit(&paths, removed, Change::Remove).unwrap();
    assert!(!on(removed));
    assert!(
        std::fs::read_to_string(paths.sources())
            .unwrap()
            .contains(&format!("#{removed}\n"))
    );
    edit(&paths, A, Change::Remove).unwrap();
    assert!(read(&paths).iter().all(|s| s.url != A));
}

#[test]
fn a_confirmed_selection_disables_all_its_lists_without_resetting_other_subscriptions() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let off = defaults_off(&[]);
    std::fs::write(paths.sources(), format!("# Lists\n{A}\n{B}\n{off}")).unwrap();
    configure(&paths, &format!("#{A}")).unwrap();
    assert_eq!(
        std::fs::read_to_string(paths.sources()).unwrap(),
        format!("# Lists\n#{A}\n{B}\n{off}")
    );
    let saved = std::fs::read_to_string(paths.sources()).unwrap();
    assert!(configure(&paths, &format!("#{B}\ninvalid")).is_err());
    assert_eq!(std::fs::read_to_string(paths.sources()).unwrap(), saved);
    configure(&paths, &format!("#{B}")).unwrap();
    assert!(read(&paths).iter().all(|source| !source.enabled));
}

#[test]
fn a_sources_file_that_cannot_be_looked_at_is_not_replaced_by_the_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    // A link to itself: whether a file is there cannot be told.
    std::os::unix::fs::symlink(paths.sources(), paths.sources()).unwrap();
    assert!(configure(&paths, &format!("#{A}")).is_err());
    assert!(edit(&paths, A, Change::Add).is_err());
    let still = std::fs::symlink_metadata(paths.sources()).unwrap();
    assert!(still.file_type().is_symlink());
}

#[test]
fn a_source_is_disabled_by_a_hash_against_its_address_only() {
    let sources = parse(
        "# The filter lists, one address a line.\n#https://a.example/x\nhttps://b.example/y\n#not an address\n\n",
    );
    assert_eq!(
        sources,
        [
            Source {
                url: "https://a.example/x".to_owned(),
                enabled: false
            },
            Source {
                url: "https://b.example/y".to_owned(),
                enabled: true
            },
        ]
    );
}

#[test]
fn failing_to_save_a_disable_keeps_the_active_copy() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.lists()).unwrap();
    std::fs::write(paths.sources(), format!("{A}\n")).unwrap();
    std::fs::create_dir(paths.sources().with_extension("new")).unwrap();
    let list = paths.lists().join(file_name(A));
    std::fs::write(&list, "||ads.example^\n").unwrap();

    assert!(edit(&paths, A, Change::Disable).is_err());
    assert!(read(&paths)[0].enabled);
    assert_eq!(std::fs::read_to_string(list).unwrap(), "||ads.example^\n");
    assert!(!has_disabled_copy(&paths.lists(), A));
}

#[test]
fn list_status_prefers_the_local_copy_when_download_metadata_is_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.lists()).unwrap();
    std::fs::write(paths.sources(), format!("{A}\n{B}\n")).unwrap();
    std::fs::write(
        paths.lists().join(file_name(A)),
        "! Title: First\n||ads.example^\n",
    )
    .unwrap();
    write_info(
        &paths.list_info(),
        &Info {
            url: B.to_owned(),
            rules: Some(2),
            updated: Some(5),
            error: Some("timed out".to_owned()),
            ..Info::default()
        },
    )
    .unwrap();

    let status = crate::protocol::lists(&paths);
    assert_eq!(status[0].rules, Some(1));
    assert_eq!(status[0].title.as_deref(), Some("First"));
    assert_eq!(status[1].rules, None);
    assert_eq!(status[1].updated, None);
    assert_eq!(status[1].error.as_deref(), Some("timed out"));
}

#[test]
fn list_status_recounts_legacy_metadata_by_active_lines() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.lists()).unwrap();
    std::fs::create_dir_all(paths.list_info()).unwrap();
    std::fs::write(
        paths.lists().join(file_name(A)),
        "! Title: Conditional\n!#if false\n||hidden.example^\n!#else\n||active.example^\n!#endif\n",
    )
    .unwrap();
    std::fs::write(
        info_file(&paths.list_info(), A),
        format!(r#"{{"url":"{A}","rules":2,"updated":5}}"#),
    )
    .unwrap();
    let source = Source {
        url: A.to_owned(),
        enabled: true,
    };
    let (known, previous) = list_info(&paths.lists(), &paths.list_info(), &source);
    assert!(!previous);
    assert_eq!(known.rules, Some(1));
    assert_eq!(known.updated, Some(5));
    assert_eq!(known.title.as_deref(), Some("Conditional"));
}

#[test]
fn a_disabled_source_stops_filtering_and_can_be_enabled_without_a_download() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let off = defaults_off(&[]);
    std::fs::create_dir_all(paths.lists()).unwrap();
    std::fs::write(paths.sources(), format!("# Lists\n{A}\n{B}\n{off}")).unwrap();
    let list = paths.lists().join(file_name(A));
    std::fs::write(&list, "||ads.example^\n").unwrap();
    write_info(
        &paths.list_info(),
        &Info {
            url: A.to_owned(),
            updated: Some(5),
            ..Info::default()
        },
    )
    .unwrap();

    assert!(edit(&paths, A, Change::Disable).unwrap());
    assert_eq!(
        std::fs::read_to_string(paths.sources()).unwrap(),
        format!("# Lists\n#{A}\n{B}\n{off}")
    );
    assert!(!list.exists() && info(&paths.list_info(), A).is_some());
    assert!(
        !crate::filter::Filters::load(&[&paths.lists()], false)
            .unwrap()
            .blocks_host("ads.example")
    );
    let reported = crate::protocol::lists(&paths);
    assert!(!reported[0].enabled);
    assert_eq!(reported[0].updated, Some(5));
    assert_eq!(
        read(&paths)[0],
        Source {
            url: A.to_owned(),
            enabled: false
        }
    );

    assert!(edit(&paths, A, Change::Enable).unwrap());
    assert!(
        crate::filter::Filters::load(&[&paths.lists()], false)
            .unwrap()
            .blocks_host("ads.example")
    );
    assert_eq!(
        std::fs::read_to_string(paths.sources()).unwrap(),
        format!("# Lists\n{A}\n{B}\n{off}")
    );
    // Twice is once.
    edit(&paths, A, Change::Enable).unwrap();
    edit(&paths, A, Change::Add).unwrap();
    assert_eq!(read(&paths).iter().filter(|s| s.enabled).count(), 2);

    edit(&paths, A, Change::Disable).unwrap();
    edit(&paths, A, Change::Remove).unwrap();
    assert!(!has_disabled_copy(&paths.lists(), A));
    assert!(info(&paths.list_info(), A).is_none());
}

#[test]
fn a_copy_under_its_untrusted_name_keeps_filtering_until_refresh_and_both_copies_are_disabled_together()
 {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let url = "https://filters.adtidy.org/android/filters/11.txt";
    std::fs::create_dir(paths.lists()).unwrap();
    std::fs::write(paths.sources(), format!("{url}\n")).unwrap();
    let old = paths.lists().join(untrusted_file_name(url).unwrap());
    let new = paths.lists().join(file_name(url));
    std::fs::write(&old, "||old.example^\n").unwrap();
    {
        let lock = files::lock(&paths.sources()).unwrap();
        prune_unsubscribed(url, &paths.lists(), None, &lock).unwrap();
    }
    assert!(old.is_file());
    assert_eq!(copy_path(&paths.lists(), url, "txt"), old);
    std::fs::write(&new, "||fresh.example^\n").unwrap();
    edit(&paths, url, Change::Disable).unwrap();
    assert!(!old.exists() && !new.exists());
    edit(&paths, url, Change::Enable).unwrap();
    assert!(new.is_file() && !old.exists());
    edit(&paths, url, Change::Remove).unwrap();
    assert!(!old.with_extension("disabled").exists());
    assert!(!new.with_extension("disabled").exists());
}

#[test]
fn sources_are_added_and_removed_and_only_https_addresses_are_taken() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    // No file yet: the defaults, and the new one after them.
    edit(&paths, A, Change::Add).unwrap();
    let sources = read(&paths);
    assert!(sources.len() > 1 && sources.last().unwrap().url == A);
    edit(&paths, A, Change::Disable).unwrap();
    edit(&paths, A, Change::Remove).unwrap();
    assert!(read(&paths).iter().all(|s| s.url != A));
    assert!(edit(&paths, A, Change::Enable).is_err());
    let long = format!("https://a.example/{}", "x".repeat(MAX_URL));
    for bad in [
        "http://a.example/l.txt",
        "https://",
        "https://a.example/a b",
        "https://a.example/\nx",
        &long,
    ] {
        assert!(edit(&paths, bad, Change::Add).is_err(), "{bad:?}");
    }
}

#[test]
fn a_saved_choice_of_a_moved_list_is_read_as_its_new_address() {
    for &(old, new) in MOVED {
        moved_selection(old, new);
    }
}

fn moved_selection(old: &str, new: &str) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let off = defaults_off(&[new]);
    std::fs::write(paths.sources(), format!("# Lists\n{A}\n#{old}\n{off}")).unwrap();
    assert_eq!(
        read(&paths)[..2],
        [
            Source {
                url: A.to_owned(),
                enabled: true
            },
            Source {
                url: new.to_owned(),
                enabled: false
            },
        ]
    );
    edit(&paths, new, Change::Enable).unwrap();
    assert_eq!(
        std::fs::read_to_string(paths.sources()).unwrap(),
        format!("# Lists\n{A}\n{new}\n{off}")
    );

    std::fs::write(paths.sources(), format!("{old}\n#{new}\n{off}")).unwrap();
    assert_eq!(
        read(&paths)[..1],
        [Source {
            url: new.to_owned(),
            enabled: false
        }]
    );
}

#[test]
fn a_moved_copy_and_its_metadata_are_kept_only_while_selected_and_without_a_replacement() {
    for &(old, new) in MOVED {
        moved_copy(old, new);
    }
}

fn moved_copy(old: &str, new: &str) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.lists()).unwrap();
    std::fs::write(paths.sources(), format!("{old}\n")).unwrap();
    let copy = paths.lists().join(file_name(old));
    assert_ne!(file_name(new), file_name(old));
    std::fs::write(&copy, "||ads.example^\n").unwrap();
    write_info(
        &paths.list_info(),
        &Info {
            url: old.to_owned(),
            ..Info::default()
        },
    )
    .unwrap();

    let lock = files::lock(&paths.sources()).unwrap();
    let selection = configured(&paths).unwrap();
    prune_unsubscribed(&selection, &paths.lists(), Some(&paths.list_info()), &lock).unwrap();
    assert!(copy.exists());
    assert!(info_file(&paths.list_info(), old).exists());
    assert_eq!(read(&paths)[0].url, new);

    let replacement = paths.lists().join(file_name(new));
    std::fs::write(&replacement, "||fresh.example^\n").unwrap();
    prune_unsubscribed(&selection, &paths.lists(), Some(&paths.list_info()), &lock).unwrap();
    assert!(replacement.exists());
    assert!(!copy.exists());
    assert!(!info_file(&paths.list_info(), old).exists());
}

#[test]
fn a_disabled_moved_list_resumes_filtering_with_its_previous_copy() {
    for &(old, new) in MOVED {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::write(paths.sources(), format!("{old}\n")).unwrap();
        std::fs::create_dir(paths.lists()).unwrap();
        let copy = paths.lists().join(file_name(old));
        std::fs::write(&copy, "||ads.example^\n").unwrap();
        write_info(
            &paths.list_info(),
            &Info {
                url: old.to_owned(),
                updated: Some(123),
                ..Info::default()
            },
        )
        .unwrap();

        assert!(edit(&paths, new, Change::Disable).unwrap());
        assert!(!copy.exists() && copy.with_extension("disabled").exists());
        {
            let lock = files::lock(&paths.sources()).unwrap();
            let selection = configured(&paths).unwrap();
            prune_unsubscribed(&selection, &paths.lists(), Some(&paths.list_info()), &lock)
                .unwrap();
        }
        assert!(
            !crate::filter::Filters::load(&[&paths.lists()], false)
                .unwrap()
                .blocks_host("ads.example")
        );
        assert!(edit(&paths, new, Change::Enable).unwrap());
        assert!(copy.exists() && !copy.with_extension("disabled").exists());
        assert!(
            crate::filter::Filters::load(&[&paths.lists()], false)
                .unwrap()
                .blocks_host("ads.example")
        );
        let (known, previous) = list_info(
            &paths.lists(),
            &paths.list_info(),
            &Source {
                url: new.to_owned(),
                enabled: true,
            },
        );
        assert!(previous);
        assert_eq!(known.updated, Some(123));

        edit(&paths, new, Change::Disable).unwrap();
        edit(&paths, new, Change::Remove).unwrap();
        assert!(!copy.exists() && !copy.with_extension("disabled").exists());
        assert!(!info_file(&paths.list_info(), old).exists());
    }
}

#[test]
fn a_restored_previous_copy_keeps_the_trust_of_its_original_file() {
    use crate::list_url::{Trust, trusted_list_file};
    let &(old, new) = MOVED
        .iter()
        .find(|(old, _)| copy_names(old).count() == 2)
        .unwrap();
    for (position, name) in copy_names(old).enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::write(paths.sources(), format!("{old}\n")).unwrap();
        std::fs::create_dir(paths.lists()).unwrap();
        let original = paths.lists().join(name);
        std::fs::write(&original, "||ads.example^\n").unwrap();
        edit(&paths, new, Change::Disable).unwrap();
        edit(&paths, new, Change::Enable).unwrap();
        let (address, restored) = active_copy(&paths.lists(), new);
        assert_eq!(address, old);
        assert_eq!(restored, original);
        assert!(restored.is_file());
        assert_eq!(
            trusted_list_file(&restored, Trust::ResponseRules),
            position == 0
        );
    }
}

#[test]
fn a_list_names_itself_in_its_header() {
    assert_eq!(
        title("[Adblock Plus 2.0]\n! Title: EasyList\n||a^\n").as_deref(),
        Some("EasyList")
    );
    assert_eq!(title("||a^\n! Title: Not the header\n"), None);
    assert_eq!(title("! Title:\n"), None);
}
