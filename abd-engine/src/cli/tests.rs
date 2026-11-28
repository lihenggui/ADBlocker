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

use crate::{filter, sources};

use super::*;

fn ask(paths: &Paths, args: &[&str], input: &str) -> Result<String> {
    let mut out = Vec::new();
    run(paths, &Nobody, args, &mut input.as_bytes(), &mut out)?;
    Ok(String::from_utf8(out).unwrap())
}

#[test]
fn rules_check_reports_include_and_malformed_directives_on_their_original_lines() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let text = "!#include child.txt\n!#if adguard &&\n||inactive.example^$unsupported\n!#else\n\
                ||active.example^\n!#endif\n!#endif\n";
    assert_eq!(
        ask(&paths, &["rules", "check"], text).unwrap(),
        "1: Unsupported (include-not-followed): !#include child.txt\n\
         2: Invalid (malformed-directive): !#if adguard &&\n\
         7: Invalid (malformed-directive): !#endif\n"
    );
    assert_eq!(crate::rulecheck::count(text), 1);
    assert_eq!(
        crate::rulecheck::check(text, crate::rulecheck::SourcePermissions::USER).rules,
        1
    );
}

#[test]
fn setup_commands_apply_choices_from_standard_input() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let regional = "https://regional.example/list.txt";
    assert_eq!(ask(&paths, &["list", "configure"], regional).unwrap(), "");
    assert!(
        sources::read(&paths)
            .iter()
            .any(|list| list.url == regional && list.enabled)
    );
    assert_eq!(
        ask(&paths, &["app", "configure"], "com.example.app").unwrap(),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(paths.apps()).unwrap(),
        "com.example.app\n"
    );
    assert_eq!(ask(&paths, &["app", "configure"], "").unwrap(), "");
    assert_eq!(ask(&paths, &["app", "list"], "").unwrap(), "");
    assert!(
        ask(&paths, &["app", "configure"], "invalid/name")
            .unwrap_err()
            .is::<Malformed>()
    );
    assert!(
        ask(&paths, &["list", "configure"], "invalid")
            .unwrap_err()
            .is::<Malformed>()
    );
}

#[test]
fn a_command_answers_where_it_is_told_to_and_reads_what_it_is_given() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    assert_eq!(ask(&paths, &["app", "add", "com.a"], "").unwrap(), "");
    assert_eq!(ask(&paths, &["app", "list"], "").unwrap(), "com.a\n");
    let json = ask(&paths, &["app", "list", "--json"], "").unwrap();
    assert!(json.contains("\"com.a\"") && json.ends_with('\n'));

    assert!(json.contains("\"level\":\"full\",\"reason\":null"));
    ask(&paths, &["url-filtering", "off"], "").unwrap();
    let json = ask(&paths, &["app", "list", "--json"], "").unwrap();
    assert!(json.contains("\"level\":\"hosts\",\"reason\":\"url_filtering_off\""));
    ask(&paths, &["cosmetic", "off"], "").unwrap();
    let status = ask(&paths, &["status", "--json"], "").unwrap();
    assert!(status.contains("\"url_filtering\":false") && status.contains("\"cosmetic\":false"));
    ask(&paths, &["url-filtering", "on"], "").unwrap();
    let json = ask(&paths, &["app", "list", "--json"], "").unwrap();
    assert!(json.contains("\"level\":\"full\",\"reason\":null"));
    assert!(ask(&paths, &["app", "mode", "com.a", "hosts"], "").is_err());
    assert!(ask(&paths, &["url-filtering", "maybe"], "").is_err());

    ask(&paths, &["rules", "set"], "||ads.example^\n").unwrap();
    assert_eq!(
        ask(&paths, &["rules", "get"], "").unwrap(),
        "||ads.example^\n"
    );
}

#[test]
fn all_apps_can_pause_a_selected_work_app_independently_of_the_global_pause() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let work = "37:com.work";
    std::fs::write(paths.apps(), "37:com.work\ncom.personal\n").unwrap();
    ask(&paths, &["scope", "all"], "").unwrap();
    let pauses = || crate::pause::Pauses::read(&paths.pauses(), crate::clock::now());

    let missing = ask(
        &paths,
        &["app", "pause", "com.personal", "--minutes", "1"],
        "",
    )
    .unwrap_err();
    assert_eq!(missing.to_string(), "not a filtered app: com.personal");
    assert!(!missing.is::<Malformed>());
    let malformed = ask(&paths, &["app", "resume", work, "extra"], "").unwrap_err();
    assert!(malformed.is::<Malformed>());
    assert!(!paths.pauses().exists());

    assert_eq!(
        ask(&paths, &["app", "pause", work, "--minutes", "1"], "").unwrap(),
        ""
    );
    assert_eq!(ask(&paths, &["pause"], "").unwrap(), "");
    assert!(pauses().app(work).is_some());
    assert_eq!(pauses().all(), Some(None));

    assert_eq!(ask(&paths, &["app", "resume", work], "").unwrap(), "");
    assert!(pauses().app(work).is_none());
    assert_eq!(pauses().all(), Some(None));
    assert_eq!(ask(&paths, &["resume"], "").unwrap(), "");
    assert!(!paths.pauses().exists());
}

#[test]
fn stats_json_requires_eight_nondecreasing_epoch_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let args = [
        "stats", "--json", "-3600", "0", "3600", "7200", "7200", "10800", "14400", "18000",
    ];
    assert_eq!(
        ask(&paths, &args, "").unwrap(),
        "{\"days\":[[0,0],[0,0],[0,0],[0,0],[0,0],[0,0],[0,0]],\"apps\":{}}\n"
    );

    let mut extra = args.to_vec();
    extra.push("21600");
    let mut wrong_order = args;
    wrong_order.swap(3, 4);
    let mut not_seconds = args;
    not_seconds[2] = "local-midnight";
    let mut overflow = args;
    overflow[9] = "9223372036854775808";
    for args in [
        &args[..2],
        &args[..9],
        &extra,
        &wrong_order,
        &not_seconds,
        &overflow,
    ] {
        for command in ["stats", "overview"] {
            let mut args = args.to_vec();
            args[0] = command;
            assert!(ask(&paths, &args, "").unwrap_err().is::<Malformed>());
        }
    }
}

#[test]
fn stats_text_keeps_the_lifetime_counters_for_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::write(
        paths.stats(),
        "since 1\nrequests_blocked 3\nhosts_blocked 5\npassed 7\npinned 9\nrefused 11\n",
    )
    .unwrap();
    assert_eq!(
        ask(&paths, &["stats"], "").unwrap(),
        "since 1970-01-01 00:00 UTC\n\
         hosts_blocked 5\n\
         passed 7\n\
         decrypted 0\n\
         requests_allowed 0\n\
         requests_blocked 3\n\
         pinned 9\n\
         refused 11\n"
    );
}

#[test]
fn concurrent_rule_appends_keep_each_addition_and_import() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    ask(&paths, &["rules", "set"], "! existing").unwrap();
    let start = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        for rule in ["||first.example^", "||second.example^", "||first.example^"] {
            let (paths, start) = (&paths, &start);
            scope.spawn(move || {
                start.wait();
                ask(paths, &["rules", "append", "--unique"], rule).unwrap();
            });
        }
        start.wait();
        ask(&paths, &["rules", "append"], "! import\n||third.example^").unwrap();
    });
    let text = ask(&paths, &["rules", "get"], "").unwrap();
    let mut lines: Vec<_> = text.lines().collect();
    lines.sort_unstable();
    assert_eq!(
        lines,
        [
            "! existing",
            "! import",
            "||first.example^",
            "||second.example^",
            "||third.example^"
        ]
    );
}

#[test]
fn an_append_over_the_combined_size_limit_preserves_the_existing_rules() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    ask(&paths, &["rules", "set"], "keep").unwrap();
    assert!(ask(&paths, &["rules", "append"], &"x".repeat(filter::MAX_LIST)).is_err());
    assert_eq!(ask(&paths, &["rules", "get"], "").unwrap(), "keep");
}

#[test]
fn removing_exact_rules_preserves_other_bytes_and_reloads_only_after_a_change() {
    #[derive(Default)]
    struct Owner(std::cell::Cell<usize>);
    impl Host for Owner {
        fn running(&self) -> Option<i32> {
            None
        }
        fn reload(&self) {
            self.0.set(self.0.get() + 1);
        }
        fn version(&self) -> Option<String> {
            None
        }
        fn boot_guard(&self) -> Option<u64> {
            None
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let host = Owner::default();
    let current = "! comment\r\n  ||remove.example^ \n||remove.example^$app=com.keep\n\n\t||remove.example^\r\n@@||second.example^\n||keep.example^";
    let remaining = "! comment\r\n||remove.example^$app=com.keep\n\n||keep.example^";
    ask(&paths, &["rules", "set"], current).unwrap();
    // The blank line in the input leaves the user's own blank line alone.
    let remove = " \t||remove.example^\r\n\n@@||second.example^ \n||absent.example^\n";
    let mut out = Vec::new();
    run(
        &paths,
        &host,
        &["rules", "remove"],
        &mut remove.as_bytes(),
        &mut out,
    )
    .unwrap();
    assert!(out.is_empty());
    assert_eq!(ask(&paths, &["rules", "get"], "").unwrap(), remaining);
    assert_eq!(host.0.get(), 1);

    run(
        &paths,
        &host,
        &["rules", "remove"],
        &mut remove.as_bytes(),
        &mut out,
    )
    .unwrap();
    assert!(out.is_empty());
    assert_eq!(ask(&paths, &["rules", "get"], "").unwrap(), remaining);
    assert_eq!(host.0.get(), 1);
    assert!(
        ask(&paths, &["rules", "remove", "extra"], "")
            .unwrap_err()
            .is::<Malformed>()
    );
}

#[test]
fn rule_edit_reports_whether_the_locked_edit_changed_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let added: serde_json::Value =
        serde_json::from_str(include_str!("../../../testdata/protocol/rules-edit.json")).unwrap();
    let rule = "||mine.example^";
    for args in [
        &["rules", "append", "--unique", "--json"][..],
        &["rules", "remove", "--json"],
    ] {
        let edit = |args| {
            serde_json::from_str::<serde_json::Value>(&ask(&paths, args, rule).unwrap()).unwrap()
        };
        assert_eq!(edit(args), added);
        assert_eq!(edit(args), serde_json::json!({ "changed": false }));
    }
}

#[test]
fn what_only_one_program_does_is_not_known_here() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    for args in [
        &["update"][..],
        &["watch"],
        &["run"],
        &["handoff", "begin", "92883ed4-48f5-434d-bafe-db87c3aa1832"],
        &[],
    ] {
        let e = ask(&paths, args, "").unwrap_err();
        assert!(e.downcast_ref::<Malformed>().is_some());
    }
}
