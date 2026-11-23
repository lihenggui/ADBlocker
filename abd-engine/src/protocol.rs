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

//! What the commands print with `--json`, for the app: the shapes fixed by
//! the files in `testdata/protocol`, and how each is made from what is on
//! disk. A field with no value is `null`, never left out.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result};
use serde::Serialize;

use crate::allow::Allow;
pub use crate::diagnostics::check;
use crate::paths::Paths;
use crate::pause::Pauses;
pub use crate::protocol_types::{
    Check, Dropped, Event, Issue, IssueKind, ListState, RulesCheck, Seen, Update,
};
use crate::settings::Settings;
use crate::{apps, policy, sources, state, stats};

#[derive(Debug, Serialize)]
pub struct Status {
    pub running: bool,
    pub pid: Option<i32>,
    pub started: Option<u64>,
    pub version: Option<String>,
    /// Number of saved app selections.
    pub apps: usize,
    /// Number of actively filtered apps, when reported by the host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filtering_apps: Option<usize>,
    pub rules: Option<usize>,
    pub host_rules: Option<usize>,
    pub exclusions: Option<usize>,
    pub paused: bool,
    pub resume_at: Option<u64>,
    pub ipv6: Option<bool>,
    pub lists_updated: Option<u64>,
    /// Whether URL filtering is enabled for HTTP and eligible HTTPS.
    pub url_filtering: bool,
    pub scope: crate::settings::Scope,
    /// Since when the program has kept itself turned off, for the user to
    /// turn on again: for the module, since two boots in a row that did
    /// not finish.
    /// Said by the program (`cli::Host`), not found in the files here.
    pub boot_guard: Option<u64>,
    /// Whether what the lists name is hidden on decrypted pages.
    pub cosmetic: bool,
}

/// `abd status --json`. `running` is the daemon's pid if one runs: what
/// its state file says is taken only from that daemon. Snapshot fields
/// are unavailable until that daemon publishes successfully.
pub fn status(
    paths: &Paths,
    running: Option<i32>,
    version: Option<String>,
    settings: Settings,
    now: u64,
    lists_updated: Option<u64>,
) -> Status {
    let state = running
        .and_then(|pid| state::read(&paths.state()).filter(|s| i64::from(s.pid) == i64::from(pid)));
    let named = crate::files::read_text(&paths.apps(), apps::MAX_FILE).unwrap_or_default();
    let pause = Pauses::read(&paths.pauses(), now).all();
    Status {
        running: running.is_some(),
        pid: running,
        started: state.as_ref().map(|s| s.started),
        version,
        apps: apps::named(&named).len(),
        filtering_apps: state.as_ref().and_then(|s| s.filtering_apps),
        rules: state.as_ref().map(|s| s.rules),
        host_rules: state.as_ref().map(|s| s.host_rules),
        exclusions: state.as_ref().map(|s| s.exclusions),
        paused: pause.is_some(),
        resume_at: pause.flatten(),
        ipv6: state.as_ref().map(|s| s.ipv6),
        lists_updated,
        url_filtering: settings.url_filtering,
        scope: settings.scope,
        cosmetic: settings.cosmetic,
        boot_guard: None,
    }
}

/// The module's version, from the `module.prop` next to the program or
/// in the directory above it: `bin/abd` in the module.
pub fn version(program: &Path) -> Option<String> {
    let dir = program.parent()?;
    [dir.join("module.prop"), dir.join("../module.prop")]
        .iter()
        .find_map(|file| {
            let text = std::fs::read_to_string(file).ok()?;
            let version = text.lines().find_map(|l| l.strip_prefix("version="))?;
            Some(version.trim().to_owned())
        })
}

#[derive(Debug, Serialize)]
pub struct App {
    pub package: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_serial: Option<u32>,
    pub effective: EffectiveFiltering,
    pub paused_until: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<AppEligibility>,
}

/// Kernel eligibility reported by the root app resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppEligibility {
    Eligible,
    SharedUid,
    Unavailable,
}

impl App {
    pub fn report_eligibility(&mut self, eligibility: AppEligibility) {
        if eligibility != AppEligibility::Eligible {
            self.unavailable_reason = Some(eligibility);
            self.effective = EffectiveFiltering {
                http: Level {
                    level: "off",
                    reason: None,
                },
                https: Level {
                    level: "off",
                    reason: None,
                },
            };
        }
    }
}

/// The name Android assigns to a shared UID and its installed members in this profile.
#[derive(Debug, Serialize)]
pub struct GroupInventory {
    pub id: String,
    pub packages: Vec<String>,
}

/// Package eligibility from `abd app eligibility --json`.
#[derive(Debug, Serialize)]
pub struct PersonalEligibility {
    pub apps: BTreeMap<String, AppEligibility>,
    pub groups: Vec<GroupInventory>,
}

#[derive(Debug, Serialize)]
pub struct EffectiveFiltering {
    pub http: Level,
    pub https: Level,
}

#[derive(Debug, Serialize)]
pub struct Level {
    pub level: &'static str,
    pub reason: Option<&'static str>,
}

/// Managed profiles and their eligible apps from `abd app profiles --json`.
#[derive(Debug, Serialize)]
pub struct ProfileInventories {
    pub profiles: Vec<ProfileInventory>,
}

#[derive(Debug, Serialize)]
pub struct ProfileInventory {
    pub serial: u32,
    /// `null` when Android could not report this profile's packages.
    pub packages: Option<Vec<String>>,
    pub groups: Option<Vec<GroupInventory>>,
}

fn level(settings: Settings, cannot: Option<&'static str>) -> Level {
    let reason = if settings.url_filtering {
        cannot
    } else {
        Some("url_filtering_off")
    };
    Level {
        level: if reason.is_some() { "hosts" } else { "full" },
        reason,
    }
}

/// The saved preferences remain available even when All apps is active.
#[derive(Debug, Serialize)]
pub struct AppSelections {
    pub scope: crate::settings::Scope,
    pub chosen: Vec<String>,
    pub excluded: BTreeSet<String>,
}

pub fn app_selections(text: &str, scope: crate::settings::Scope) -> AppSelections {
    AppSelections {
        scope,
        chosen: apps::named(text).into_iter().map(str::to_owned).collect(),
        excluded: apps::excluded(text)
            .into_iter()
            .map(str::to_owned)
            .collect(),
    }
}

/// `abd app list --json`.
pub fn apps(
    paths: &Paths,
    settings: Settings,
    cannot: Option<&'static str>,
    trusted_profiles: &BTreeSet<u32>,
    now: u64,
) -> Vec<App> {
    let text = crate::files::read_text(&paths.apps(), apps::MAX_FILE).unwrap_or_default();
    app_levels(
        paths,
        &apps::named(&text),
        settings,
        cannot,
        trusted_profiles,
        now,
    )
}

pub fn app_levels(
    paths: &Paths,
    selected: &[&str],
    settings: Settings,
    cannot: Option<&'static str>,
    trusted_profiles: &BTreeSet<u32>,
    now: u64,
) -> Vec<App> {
    let pauses = Pauses::read(&paths.pauses(), now);
    let mut apps: Vec<App> = selected
        .iter()
        .filter_map(|&token| {
            let identity = apps::Identity::parse(token)?;
            let cannot = if identity
                .profile_serial
                .is_some_and(|s| !trusted_profiles.contains(&s))
            {
                Some("profile_trust_unavailable")
            } else {
                cannot
            };
            Some(App {
                package: identity.package.to_owned(),
                profile_serial: identity.profile_serial,
                effective: EffectiveFiltering {
                    http: level(settings, None),
                    https: level(settings, cannot),
                },
                paused_until: pauses.app(token),
                unavailable_reason: None,
            })
        })
        .collect();
    apps.sort_by(|a, b| (&a.package, a.profile_serial).cmp(&(&b.package, b.profile_serial)));
    apps
}

pub(crate) const STATS_DAYS: usize = 7;

/// Blocked requests and hosts for each day, oldest first.
type Days = [[i64; 2]; STATS_DAYS];

#[derive(Debug, Default, Serialize)]
pub(crate) struct Stats {
    days: Days,
    apps: BTreeMap<String, Days>,
}

/// `abd stats --json` takes the app's ordered local day boundaries, in Unix
/// seconds. Each retained UTC hour belongs to the day its start falls in.
pub(crate) fn stats(paths: &Paths, now: u64, boundaries: [i64; STATS_DAYS + 1]) -> Result<Stats> {
    let [first, .., last] = boundaries;
    let mut result = Stats::default();
    let saved =
        stats::Snapshot::read(&paths.stats(), now).context("reading the saved statistics")?;
    for (package, hours) in saved.apps {
        let mut days: Days = Default::default();
        let mut recorded = false;
        for (hour, [requests, hosts, ..]) in hours {
            let start = i64::from(hour) * 3600;
            if !(first..last).contains(&start) {
                continue;
            }
            let day = boundaries[1..].partition_point(|&end| end <= start);
            for (sum, count) in days[day].iter_mut().zip([requests, hosts]) {
                *sum = sum.saturating_add(i64::try_from(count).unwrap_or(i64::MAX));
            }
            recorded = true;
        }
        // App details also show zero counts for an app with allowed traffic.
        if recorded {
            for (total, app) in result.days.iter_mut().zip(days) {
                for (sum, count) in total.iter_mut().zip(app) {
                    *sum = sum.saturating_add(count);
                }
            }
            result.apps.insert(package, days);
        }
    }
    Ok(result)
}

#[derive(Debug, Serialize)]
pub struct List {
    pub url: String,
    pub enabled: bool,
    pub title: Option<String>,
    pub rules: Option<usize>,
    pub updated: Option<u64>,
    pub error: Option<String>,
}

/// The sections read together for a refresh of the overview.
#[derive(Debug, Serialize)]
pub(crate) struct Overview {
    pub status: Status,
    pub stats: Stats,
    pub lists: Vec<List>,
}

/// `abd lists --json`.
pub fn lists(paths: &Paths) -> Vec<List> {
    let dir = paths.lists();
    let info_dir = paths.list_info();
    sources::read(paths)
        .into_iter()
        .map(|s| {
            let info = sources::list_info(&dir, &info_dir, &s);
            List {
                url: s.url,
                enabled: s.enabled,
                title: info.title,
                rules: info.rules,
                updated: info.updated,
                error: info.error,
            }
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct Pinned {
    pub app: String,
    pub host: String,
    pub since: u64,
}

/// `abd pinned --json`, of every app or of `app`.
pub fn pinned(paths: &Paths, app: Option<&str>, now: u64) -> Vec<Pinned> {
    let mut all: Vec<Pinned> = policy::read_pinned(&paths.pinned(), now)
        .into_iter()
        .filter(|((package, _), _)| app.is_none_or(|a| a == package))
        .map(|((app, host), since)| Pinned { app, host, since })
        .collect();
    all.sort_by(|a, b| (&a.app, &a.host).cmp(&(&b.app, &b.host)));
    all
}

/// `abd allow list --json`.
pub fn allow(paths: &Paths) -> Vec<String> {
    Allow::load(&paths.allow())
        .hosts()
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    use serde_json::Value;

    use super::*;
    use crate::list_url::file_name;
    use crate::policy::Reason;

    const NOW: u64 = 1_763_538_000;
    const ON: Settings = Settings {
        url_filtering: true,
        scope: crate::settings::Scope::Chosen,
        cosmetic: false,
    };
    const EASYLIST: &str = "https://easylist.to/easylist/easylist.txt";
    const CHINA: &str = "https://easylist-downloads.adblockplus.org/easylistchina.txt";
    const ADTIDY: &str = "https://filters.adtidy.org/extension/ublock/filters/224.txt";

    fn sample(name: &str) -> String {
        let file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../testdata/protocol")
            .join(name);
        std::fs::read_to_string(file).unwrap()
    }

    fn json(name: &str) -> Value {
        serde_json::from_str(&sample(name)).unwrap()
    }

    fn value(v: impl Serialize) -> Value {
        serde_json::to_value(v).unwrap()
    }

    fn data() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        (dir, paths)
    }

    fn info(
        paths: &Paths,
        url: &str,
        title: &str,
        rules: usize,
        updated: u64,
        error: Option<&str>,
    ) {
        let info = sources::Info {
            url: url.to_owned(),
            title: Some(title.to_owned()),
            rules: Some(rules),
            updated: Some(updated),
            error: error.map(str::to_owned),
            validator: None,
        };
        sources::write_info(&paths.list_info(), &info).unwrap();
        std::fs::create_dir_all(paths.lists()).unwrap();
        std::fs::write(paths.lists().join(file_name(url)), "||fixture.example^\n").unwrap();
    }

    /// Five apps, two lists updated, the newest at 1763531000.
    fn known(paths: &Paths) {
        std::fs::write(paths.apps(), "com.a\ncom.b\ncom.c\ncom.d\n# none\ncom.e\n").unwrap();
        subscriptions(paths);
    }

    fn running_state(paths: &Paths) {
        let state = state::State {
            pid: 4242,
            started: 1_763_538_000,
            rules: 216_450,
            host_rules: 53211,
            exclusions: 812,
            exclusions_ready: true,
            profile_tls: Default::default(),
            filtering_apps: None,
            ipv6: true,
        };
        state::write(&paths.state(), &state).unwrap();
    }

    #[test]
    fn status_of_a_running_daemon() {
        let (_dir, paths) = data();
        known(&paths);
        running_state(&paths);
        let status = status(
            &paths,
            Some(4242),
            Some("v0.2.0".to_owned()),
            ON,
            NOW,
            sources::lists_updated(&paths),
        );
        assert_eq!(value(status), json("status.json"));
    }

    #[test]
    fn active_filtering_count_comes_only_from_the_current_daemon() {
        let (_dir, paths) = data();
        known(&paths);
        running_state(&paths);
        let mut current = state::read(&paths.state()).unwrap();
        current.filtering_apps = Some(3);
        state::write(&paths.state(), &current).unwrap();
        let read = |pid| {
            status(
                &paths,
                pid,
                Some("v0.2.0".to_owned()),
                ON,
                NOW,
                sources::lists_updated(&paths),
            )
        };
        assert_eq!(value(read(Some(4242))), json("status-filtering-apps.json"));
        assert_eq!(read(Some(6789)).filtering_apps, None);
        assert_eq!(read(None).filtering_apps, None);
    }

    #[test]
    fn a_failed_state_publication_does_not_offer_a_stale_same_process_snapshot() {
        let (_dir, paths) = data();
        running_state(&paths);
        let mut next = state::read(&paths.state()).unwrap();
        next.filtering_apps = Some(1);
        let blocked = paths.state().with_extension("new");
        std::fs::create_dir(&blocked).unwrap();
        assert!(state::write(&paths.state(), &next).is_err());

        let read = || status(&paths, Some(4242), None, ON, NOW, None);
        let unavailable = read();
        assert!(unavailable.running);
        assert_eq!(unavailable.started, None);
        assert_eq!(unavailable.rules, None);

        std::fs::remove_dir(blocked).unwrap();
        state::write(&paths.state(), &next).unwrap();
        assert_eq!(read().filtering_apps, Some(1));
    }

    #[test]
    fn status_with_no_daemon_says_what_the_files_do_and_nothing_of_a_daemon_gone() {
        let (_dir, paths) = data();
        known(&paths);
        // Written by a daemon that is no more.
        let state = state::State {
            pid: 4242,
            started: 1,
            rules: 2,
            host_rules: 3,
            exclusions: 4,
            exclusions_ready: true,
            profile_tls: Default::default(),
            filtering_apps: None,
            ipv6: false,
        };
        state::write(&paths.state(), &state).unwrap();
        std::fs::write(paths.pauses(), "1763541600 *\n").unwrap();
        let status = status(
            &paths,
            None,
            Some("v0.2.0".to_owned()),
            ON,
            NOW,
            sources::lists_updated(&paths),
        );
        assert_eq!(value(status), json("status-stopped.json"));
    }

    #[test]
    fn the_version_is_the_module_s() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        std::fs::write(
            dir.path().join("module.prop"),
            "id=adblocker\nversion=v0.2.0\nversionCode=2\n",
        )
        .unwrap();
        let program = dir.path().join("bin/abd");
        assert_eq!(version(&program).as_deref(), Some("v0.2.0"));
        assert_eq!(version(&dir.path().join("elsewhere/x/abd")), None);
    }

    #[test]
    fn app_selections_reports_saved_choices_and_exclusions_independent_of_scope() {
        let text =
            "com.example.browser\n37:com.example.news\n!@com.example.suite\n!com.example.shop\n";
        assert_eq!(
            value(app_selections(text, crate::settings::Scope::All)),
            json("app-selections.json")
        );
    }

    #[test]
    fn selected_apps_report_the_same_level_and_independent_pauses() {
        let (_dir, paths) = data();
        std::fs::write(
            paths.apps(),
            "com.example.shop\ncom.example.browser\ncom.example.news\n",
        )
        .unwrap();
        std::fs::write(
            paths.pauses(),
            "1763541600 com.example.news\n1763000000 com.example.browser\n",
        )
        .unwrap();
        assert_eq!(
            value(apps(&paths, ON, None, &BTreeSet::new(), NOW)),
            json("apps.json")
        );
    }

    #[test]
    fn a_work_selection_reports_the_profile_serial_and_effective_tls_level() {
        let (_dir, paths) = data();
        std::fs::write(paths.apps(), "com.example.news\n37:com.example.news\n").unwrap();
        std::fs::write(paths.pauses(), "1763541600 37:com.example.news\n").unwrap();
        assert_eq!(
            value(apps(&paths, ON, None, &BTreeSet::new(), NOW)),
            json("apps-work.json")
        );
    }

    #[test]
    fn the_same_package_has_independent_profile_trust_pauses_pins_and_counts() {
        let (_dir, paths) = data();
        std::fs::write(paths.apps(), "com.same\n31:com.same\n").unwrap();
        std::fs::write(paths.pauses(), format!("{} 31:com.same\n", NOW + 60)).unwrap();
        let list = apps(&paths, ON, None, &BTreeSet::new(), NOW);
        assert_eq!(
            (
                list[0].package.as_str(),
                list[0].profile_serial,
                list[0].effective.https.level
            ),
            ("com.same", None, "full")
        );
        assert_eq!(list[0].paused_until, None);
        assert_eq!(
            (
                list[1].package.as_str(),
                list[1].profile_serial,
                list[1].effective.https.level
            ),
            ("com.same", Some(31), "hosts")
        );
        assert_eq!(
            list[1].effective.https.reason,
            Some("profile_trust_unavailable")
        );
        assert_eq!(list[1].paused_until, Some(NOW + 60));
        assert_eq!(
            apps(&paths, ON, None, &BTreeSet::from([31]), NOW)[1]
                .effective
                .https
                .level,
            "full"
        );

        std::fs::write(
            paths.pinned(),
            format!("{NOW} com.same personal.example\n{NOW} 31:com.same work.example\n"),
        )
        .unwrap();
        let work = pinned(&paths, Some("31:com.same"), NOW);
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].host, "work.example");

        let day = (NOW as i64 / 86_400) * 86_400;
        let starts = std::array::from_fn(|i| day - (6_i64 - i as i64) * 86_400);
        let hour = NOW / 3600;
        std::fs::write(
            paths.stats(),
            format!("{hour} com.same 1 2 0 0 0\n{hour} 31:com.same 3 4 0 0 0\n"),
        )
        .unwrap();
        let counts = stats(&paths, NOW, starts).unwrap();
        assert_eq!(counts.apps["com.same"][6], [1, 2]);
        assert_eq!(counts.apps["31:com.same"][6], [3, 4]);
    }

    #[test]
    fn where_nothing_can_be_decrypted_an_app_says_why_it_is_not() {
        let (_dir, paths) = data();
        std::fs::write(paths.apps(), "com.a\ncom.b\n").unwrap();
        let said = |settings| -> Vec<_> {
            apps(
                &paths,
                settings,
                Some("ca_not_installed"),
                &BTreeSet::new(),
                NOW,
            )
            .into_iter()
            .map(|a| (a.effective.https.level, a.effective.https.reason))
            .collect()
        };
        assert_eq!(
            said(ON),
            [
                ("hosts", Some("ca_not_installed")),
                ("hosts", Some("ca_not_installed"))
            ]
        );
        // What the user turned off comes first: it is the nearer to undo.
        let off = Settings {
            url_filtering: false,
            scope: crate::settings::Scope::Chosen,
            cosmetic: false,
        };
        assert_eq!(said(off)[0], ("hosts", Some("url_filtering_off")));
    }

    #[test]
    fn with_url_filtering_off_only_domain_rules_apply() {
        let (_dir, paths) = data();
        std::fs::write(paths.apps(), "com.a\ncom.b\n").unwrap();
        let off = Settings {
            url_filtering: false,
            scope: crate::settings::Scope::Chosen,
            cosmetic: false,
        };
        let said: Vec<_> = apps(&paths, off, None, &BTreeSet::new(), NOW)
            .into_iter()
            .map(|a| {
                (
                    a.package,
                    a.effective.http.level,
                    a.effective.https.level,
                    a.effective.https.reason,
                )
            })
            .collect();
        assert_eq!(
            said,
            [
                (
                    "com.a".to_owned(),
                    "hosts",
                    "hosts",
                    Some("url_filtering_off")
                ),
                (
                    "com.b".to_owned(),
                    "hosts",
                    "hosts",
                    Some("url_filtering_off")
                ),
            ]
        );
        assert!(!status(&paths, None, None, off, NOW, None).url_filtering);
    }

    fn stored_counts(paths: &Paths, last_hour: u64) -> [i64; STATS_DAYS + 1] {
        let mut file = String::from("since 1763000000\n");
        for (name, n) in stats::NAMES.iter().zip([189, 52, 1413, 42, 62, 4, 0]) {
            file += &format!("{name} {n}\n");
        }
        let first = last_hour - 152;
        for (hour, counts) in [
            (first - 1, "com.example.outside 1 0 0 0 0"),
            (first, "com.example.news 4 0 0 0 0"),
            (last_hour, "com.example.news 64 12 511 3 22"),
            (last_hour - 1, "com.example.shop 0 9 0 27 0"),
            (last_hour - 1, "com.example.news 120 31 902 12 40"),
        ] {
            file += &format!("{hour} {counts}\n");
        }
        std::fs::write(paths.stats(), file).unwrap();
        std::array::from_fn(|day| (first as i64 + day as i64 * 24) * 3600)
    }

    #[test]
    fn stats_by_day_and_app_and_in_all() {
        let (_dir, paths) = data();
        let boundaries = stored_counts(&paths, 489_872);
        assert_eq!(
            value(stats(&paths, 489_872 * 3600 + 60, boundaries).unwrap()),
            json("stats.json")
        );
    }

    #[test]
    fn stats_commands_report_an_unreadable_snapshot_without_printing_zero_counts() {
        let (_dir, paths) = data();
        std::fs::write(paths.stats(), [0xff]).unwrap();
        let mut out = Vec::new();
        for args in [
            &["stats"][..],
            &["stats", "--json", "0", "1", "2", "3", "4", "5", "6", "7"],
            &["overview", "--json", "0", "1", "2", "3", "4", "5", "6", "7"],
        ] {
            let result = crate::cli::run(
                &paths,
                &crate::cli::Nobody,
                args,
                &mut std::io::empty(),
                &mut out,
            );
            assert!(result.is_err(), "{args:?}");
            assert!(out.is_empty());
        }
    }

    #[test]
    fn stats_assign_hour_starts_to_local_days_of_different_lengths() {
        let (_dir, paths) = data();
        std::fs::write(
            paths.stats(),
            "400000 com.outside 20 0 0 0 0\n\
             400001 com.a 1 0 0 0 0\n\
             400023 com.a 2 0 0 0 0\n\
             400024 com.a 3 0 0 0 0\n\
             400047 com.a 4 0 0 0 0\n\
             400048 com.a 5 0 0 0 0\n\
             400072 com.allowed 0 0 7 0 0\n\
             400143 com.a 0 6 0 0 0\n\
             400144 com.outside 0 20 0 0 0\n",
        )
        .unwrap();
        let start = 400_000 * 3600 + 1800;
        let skipped = 400_048 * 3600;
        let boundaries = [
            start,
            start + 23 * 3600,
            skipped,
            skipped,
            start + 72 * 3600,
            start + 96 * 3600,
            start + 120 * 3600,
            400_144 * 3600,
        ];
        let report = stats(&paths, 400_144 * 3600 + 60, boundaries).unwrap();
        assert_eq!(
            report.days,
            [[3, 0], [7, 0], [0, 0], [5, 0], [0, 0], [0, 0], [0, 6]],
        );
        assert_eq!(report.apps["com.a"], report.days);
        assert_eq!(report.apps["com.allowed"], [[0, 0]; STATS_DAYS]);
        assert!(!report.apps.contains_key("com.outside"));
    }

    #[test]
    fn daily_counts_saturate_at_the_maximum_the_app_can_read() {
        let (_dir, paths) = data();
        let max = i64::MAX;
        std::fs::write(
            paths.stats(),
            format!(
                "400000 com.a {} 1 0 0 0\n\
                 400001 com.a 2 {} 0 0 0\n\
                 400001 com.b 2 4 0 0 0\n",
                u64::MAX,
                max - 2
            ),
        )
        .unwrap();
        let boundaries = std::array::from_fn(|day| (400_000 + day as i64 * 24) * 3600);
        let report = stats(&paths, 400_001 * 3600, boundaries).unwrap();
        assert_eq!(report.apps["com.a"][0], [max, max - 1]);
        assert_eq!(report.apps["com.b"][0], [2, 4]);
        assert_eq!(report.days[0], [max, max]);
    }

    fn subscriptions(paths: &Paths) {
        std::fs::write(
            paths.sources(),
            format!("# The filter lists\n{EASYLIST}\n{CHINA}\n#{ADTIDY}\n"),
        )
        .unwrap();
        info(paths, EASYLIST, "EasyList", 72410, 1_763_531_000, None);
        info(
            paths,
            CHINA,
            "EasyList China",
            21544,
            1_763_444_600,
            Some("connection timed out"),
        );
    }

    #[test]
    fn lists_enabled_failed_and_disabled() {
        let (_dir, paths) = data();
        subscriptions(&paths);
        assert_eq!(value(lists(&paths)), json("lists.json"));
    }

    #[test]
    fn one_overview_command_answers_with_all_three_sections() {
        struct Running;
        impl crate::cli::Host for Running {
            fn running(&self) -> Option<i32> {
                Some(4242)
            }
            fn reload(&self) {
                panic!("reading the overview must not reload the daemon")
            }
            fn version(&self) -> Option<String> {
                Some("v0.2.0".to_owned())
            }
            fn boot_guard(&self) -> Option<u64> {
                None
            }
        }
        let (_dir, paths) = data();
        known(&paths);
        running_state(&paths);
        let starts = stored_counts(&paths, crate::clock::now() / 3600).map(|t| t.to_string());
        let mut args = vec!["overview", "--json"];
        args.extend(starts.iter().map(String::as_str));
        let mut out = Vec::new();
        crate::cli::run_with_defaults(&paths, &Running, ON, &args, &mut std::io::empty(), &mut out)
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json("overview.json")
        );
    }

    #[tokio::test]
    async fn update_reports_each_list_as_it_goes() {
        let (_dir, paths) = data();
        std::fs::write(paths.sources(), format!("# Lists\n{EASYLIST}\n{CHINA}\n")).unwrap();
        let mut list = String::from("[Adblock Plus 2.0]\n! Title: EasyList\n! Expires: 4 days\n");
        for n in 0..72410 {
            list += &format!("||ads{n}.example^\n");
        }
        let list = bytes::Bytes::from(list);
        let get = |url: String| {
            let list = list.clone();
            async move {
                match url.as_str() {
                    EASYLIST => Ok(list),
                    _ => Err(anyhow::anyhow!("connection timed out")),
                }
            }
        };
        let mut events = Vec::new();
        let tally = crate::update::fetch_all(
            &paths.sources(),
            "",
            &paths.lists(),
            Some(&paths.list_info()),
            &get,
            &mut |e| events.push(serde_json::to_string(&e).unwrap()),
        )
        .await
        .unwrap();
        events.push(serde_json::to_string(&tally.done(false, 1)).unwrap());
        assert_eq!(events.join("\n") + "\n", sample("update.jsonl"));
        // What `abd lists` shows of them afterwards.
        let lists = lists(&paths);
        assert_eq!(
            (lists[0].title.as_deref(), lists[0].rules),
            (Some("EasyList"), Some(72410))
        );
        assert_eq!(lists[1].error.as_deref(), Some("connection timed out"));
    }

    #[test]
    fn pinned_hosts_of_an_app() {
        let (_dir, paths) = data();
        std::fs::write(
            paths.pinned(),
            "1763401234 com.example.video drm.video.example\n\
             1763400000 com.example.video api.video.example\n\
             1763400000 com.example.bank api.bank.example\n\
             1000 com.example.video too.old.example\n",
        )
        .unwrap();
        assert_eq!(
            value(pinned(&paths, Some("com.example.video"), NOW)),
            json("pinned.json")
        );
        assert_eq!(pinned(&paths, None, NOW).len(), 3);
    }

    #[test]
    fn trusted_hosts() {
        let (_dir, paths) = data();
        std::fs::write(paths.allow(), "intranet.example\nbank.example\n").unwrap();
        assert_eq!(value(allow(&paths)), json("allow.json"));
    }

    #[test]
    fn check_rejects_an_oversized_list_instead_of_diagnosing_a_partial_generation() {
        let (_dir, paths) = data();
        std::fs::create_dir_all(paths.lists()).unwrap();
        std::fs::create_dir_all(paths.rules()).unwrap();
        std::fs::write(paths.sources(), format!("{EASYLIST}\n")).unwrap();
        let downloaded = paths.lists().join(file_name(EASYLIST));
        let rule = "||ads.example^\n";
        std::fs::write(&downloaded, rule).unwrap();
        std::fs::File::options()
            .write(true)
            .open(downloaded)
            .unwrap()
            .set_len(crate::filter::MAX_LIST as u64 + 1)
            .unwrap();
        std::fs::write(paths.user_rules(), rule).unwrap();

        let error = check(
            &paths,
            "https://ads.example/",
            "https://news.example/",
            "image",
        )
        .unwrap_err();
        assert!(
            error
                .downcast_ref::<std::io::Error>()
                .is_some_and(crate::list_files::exceeds_budget)
        );
    }

    #[test]
    fn rules_that_will_not_be_applied() {
        let check = crate::rulecheck::check(&sample("rules.txt"));
        assert_eq!(value(check), json("rules-check.json"));
    }

    #[test]
    fn request_diagnostics_agree_with_the_full_engine_for_mixed_lists() {
        let (_dir, paths) = data();
        std::fs::create_dir_all(paths.rules()).unwrap();
        std::fs::write(
            paths.user_rules(),
            "news.example##.ad\n||ads.example^\n@@||ads.example/allowed\n/banner/*$image\n",
        )
        .unwrap();
        let full = crate::filter::Filters::load(&[&paths.rules()], true).unwrap();
        let page = "https://news.example/";
        for (url, kind) in [
            ("https://ads.example/x", "script"),
            ("https://ads.example/allowed", "script"),
            ("https://news.example/banner/1", "image"),
            ("https://news.example/banner/1", "script"),
            (page, "document"),
        ] {
            let expected = full.check(url, page, kind, "GET");
            let actual = check(&paths, url, page, kind).unwrap();
            assert_eq!(
                (actual.blocked, actual.rule),
                (expected.blocked, expected.rule)
            );
        }
    }

    #[test]
    fn a_cached_check_tracks_source_precedence_edits_and_cache_damage() {
        let (_dir, paths) = data();
        std::fs::create_dir_all(paths.lists()).unwrap();
        std::fs::create_dir_all(paths.rules()).unwrap();
        std::fs::write(paths.sources(), EASYLIST).unwrap();
        let list = paths.lists().join(file_name(EASYLIST));
        let rule = "||tracker.example^";
        std::fs::write(&list, format!("  {rule}  ")).unwrap();
        std::fs::write(paths.user_rules(), rule).unwrap();
        let check = || {
            check(
                &paths,
                "https://tracker.example/",
                "https://news.example/",
                "image",
            )
            .unwrap()
        };
        assert_eq!(check().source.as_deref(), Some(EASYLIST));
        let cache = paths.cache().join("diagnostic-v1");
        assert!(cache.is_file());
        let before = std::fs::read(&cache).unwrap();
        assert_eq!(check().source.as_deref(), Some(EASYLIST));
        let mut corrupt = before.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        std::fs::write(&cache, corrupt).unwrap();
        assert_eq!(check().source.as_deref(), Some(EASYLIST));
        assert_ne!(
            std::fs::read(&cache).unwrap().last(),
            Some(&(before.last().unwrap() ^ 1))
        );
        std::fs::remove_file(list).unwrap();
        assert_eq!(check().source.as_deref(), Some("rules"));
        std::fs::write(paths.user_rules(), "||different.example^").unwrap();
        assert!(!check().blocked);
    }

    #[test]
    fn check_names_the_list_the_rule_is_in() {
        let (_dir, paths) = data();
        std::fs::create_dir_all(paths.lists()).unwrap();
        std::fs::create_dir_all(paths.rules()).unwrap();
        std::fs::write(paths.sources(), format!("{CHINA}\n{EASYLIST}\n")).unwrap();
        std::fs::write(paths.lists().join(file_name(CHINA)), "||other.example^\n").unwrap();
        std::fs::write(
            paths.lists().join(file_name(EASYLIST)),
            "! Title: EasyList\n||adnet.example^\n",
        )
        .unwrap();
        std::fs::write(paths.user_rules(), "||mine.example^\n").unwrap();
        let url = "https://ads.adnet.example/v2/serve?slot=feed_3";
        let c = check(&paths, url, "https://news.example/", "script").unwrap();
        assert_eq!(value(c), json("check.json"));

        let c = check(
            &paths,
            "https://mine.example/",
            "https://news.example/",
            "image",
        )
        .unwrap();
        assert_eq!(c.source.as_deref(), Some("rules"));
        let c = check(
            &paths,
            "https://fine.example/",
            "https://news.example/",
            "image",
        )
        .unwrap();
        assert_eq!((c.blocked, c.rule, c.source), (false, None, None));
    }

    #[tokio::test(start_paused = true)]
    async fn watch_tells_requests_and_connections_and_what_was_left_out() {
        static TIMES: [u64; 4] = [
            1_763_538_401_123,
            1_763_538_401_500,
            1_763_538_402_010,
            1_763_538_402_777,
        ];
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let hub = crate::live::Hub::with(4, || TIMES[NEXT.fetch_add(1, Relaxed).min(3)]);
        let mut watcher = hub.join(None).unwrap();
        let (news, page) = ("com.example.news", Some("https://news.example/"));
        let url = "https://ads.adnet.example/v2/serve?slot=feed_3";
        hub.request(news, "ads.adnet.example", true, url, page, "script");
        let url = "https://img.news.example/cover.webp";
        hub.request(news, "img.news.example", false, url, page, "image");
        hub.host_blocked("com.example.shop", "track.adsdk.example");
        hub.passed("com.example.video", "api.video.example", Reason::Pinned);
        // Nobody reads meanwhile: these are left out.
        for _ in 0..12 {
            hub.passed("com.example.video", "api.video.example", Reason::Pinned);
        }
        let mut out = Vec::new();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            crate::live::feed(&mut watcher, &mut out),
        )
        .await;
        assert_eq!(out, sample("watch.jsonl").into_bytes());
    }
}
