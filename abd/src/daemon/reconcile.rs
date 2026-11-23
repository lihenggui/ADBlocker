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
use std::sync::{Arc, RwLock};

use abd_engine::apps::{self, Identity, PER_USER_RANGE};
use abd_engine::pause::{Pauses, Timer};
use abd_engine::settings::{Scope, Settings};
use abd_engine::{clock, mitm, tls};
use anyhow::{Context as _, Result};

use super::capture_rules::CaptureRules;
use super::{setup, write_state};
use crate::paths::Paths;
use crate::{android, mounts, rules, trust, watch};

pub(super) struct Reconciler<'a> {
    ctx: &'a mitm::Ctx,
    paths: &'a Paths,
    rules: CaptureRules,
    mounts: Arc<mounts::Mounts>,
    pub(super) starts: Option<watch::Starts>,
    pub(super) packages: std::io::Result<watch::Packages>,
    pub(super) pause_timer: Timer,
    pub(super) apps_retry: watch::Retry,
}

impl<'a> Reconciler<'a> {
    pub(super) fn new(
        ctx: &'a mitm::Ctx,
        paths: &'a Paths,
        rules: CaptureRules,
        mounts: Arc<mounts::Mounts>,
    ) -> Self {
        Self {
            ctx,
            paths,
            rules,
            mounts,
            starts: setup::watch_starts(),
            pause_timer: Timer::default(),
            apps_retry: watch::Retry::new(),
            packages: watch::Packages::watch(),
        }
    }

    pub(super) fn reapply(&mut self, v6: bool) {
        let (ends, unavailable) = self.apply(v6);
        self.pause_timer.arm(ends, clock::now());
        self.apps_retry.arm(unavailable);
    }

    pub(super) fn on_app_started(&self, uid: u32) {
        // Not waited for: the process is looked at until it is the app's,
        // and connections go on meanwhile.
        let mounts = self.mounts.clone();
        tokio::spawn(async move { mounts.started(uid).await });
    }

    pub(super) fn on_packages_changed(&mut self, v6: bool, changed: watch::Change) {
        // Keep a valid identity when Android only rewrites package state
        // for an ordinary launch or another user. Off the worker: the
        // package files read here can be megabytes.
        tokio::task::block_in_place(|| {
            android_changed(
                self.ctx,
                &mut self.rules,
                &self.mounts,
                self.paths,
                v6,
                changed,
            );
        });
    }

    pub(super) fn on_user_state(&mut self, v6: bool, changed: Option<(u32, u32)>) -> bool {
        let Some((id, state)) = changed.filter(|(id, _)| {
            self.packages
                .as_ref()
                .is_ok_and(|p| p.includes_profile(*id))
        }) else {
            return false;
        };
        if state >= 4 {
            // Off the worker, like the other reads of Android's files.
            tokio::task::block_in_place(|| self.profile_stopped(v6, id));
        }
        true
    }

    fn profile_stopped(&mut self, v6: bool, id: u32) {
        retire_apps(self.ctx, &mut self.rules, v6, |uid, _| {
            uid / PER_USER_RANGE != id
        });
        let profiles = android::profiles().unwrap_or_default();
        self.ctx
            .selection
            .profile_tls
            .write()
            .unwrap()
            .retain(|serial, _| profiles.iter().any(|p| p.id != id && p.serial == *serial));
        write_state(self.ctx, self.paths, v6);
    }

    pub(super) fn remove_capture(&mut self) -> Result<()> {
        tokio::task::block_in_place(|| self.rules.remove()).context("removing capture rules")
    }

    /// Read the latest settings and app identities before publishing or
    /// touching the rules. An unavailable selection gets another attempt.
    fn apply(&mut self, v6: bool) -> (Option<u64>, bool) {
        let ctx = self.ctx;
        let mut selection = resolve_selection(ctx, self.paths, &mut self.packages);
        let owners = if !selection.unavailable
            && same_owners(&ctx.selection.apps.read().unwrap(), &selection.apps)
        {
            rules::Owners::Confirmed
        } else {
            rules::Owners::Unconfirmed
        };
        let (count, paused) = publish(ctx, &self.mounts, &mut self.starts, &mut selection);
        let failed = self.install(v6.into(), owners, count, paused).is_err();
        write_state(ctx, self.paths, v6);
        (selection.pauses.next_end(), selection.unavailable || failed)
    }

    fn install(
        &mut self,
        families: rules::Families,
        owners: rules::Owners,
        count: usize,
        paused: usize,
    ) -> Result<()> {
        let applied = set_capture(&self.ctx.selection.apps, |uids| {
            tokio::task::block_in_place(|| self.rules.set(uids, families, owners))
        });
        match &applied {
            Ok(()) if paused > 0 => eprintln!("filtering {count} apps, {paused} paused"),
            Ok(()) => eprintln!("filtering {count} apps"),
            Err(e) => eprintln!("rules: {e:#}; filtering nothing"),
        }
        applied
    }
}

pub(super) async fn due(pause_timer: &mut Timer, apps_retry: &mut watch::Retry) {
    tokio::select! {
        () = pause_timer.fired() => {},
        () = apps_retry.fired() => {},
    }
}

struct Selection {
    settings: Settings,
    pauses: Pauses,
    wanted: Option<String>,
    apps: BTreeMap<u32, String>,
    profile_tls: BTreeMap<u32, Arc<tls::Tls>>,
    unavailable: bool,
}

struct Resolved {
    apps: BTreeMap<u32, String>,
    profiles: Vec<android::Profile>,
    trust_watched: BTreeSet<u32>,
    unavailable: bool,
}

fn resolve_selection(
    ctx: &mitm::Ctx,
    paths: &Paths,
    packages: &mut std::io::Result<watch::Packages>,
) -> Selection {
    let now = clock::now();
    let cleanup_unavailable =
        tokio::task::block_in_place(|| android::prune_orphans(paths)).is_err();
    let wanted = android::selection(&paths.apps())
        .inspect_err(|error| eprintln!("app selection: {error}"))
        .ok();
    let (wanted, settings, pauses) =
        match tokio::task::block_in_place(|| ctx.reload_user_policy(paths, now)) {
            Ok((settings, pauses)) => (wanted, settings, pauses),
            Err(error) => {
                eprintln!("user policy: {error:#}");
                (None, Settings::UNSAID, Pauses::default())
            }
        };
    let previous = ctx.selection.apps.read().unwrap().clone();
    let (apps, profile_tls, unavailable) = tokio::task::block_in_place(|| {
        let Resolved {
            apps,
            profiles,
            trust_watched,
            mut unavailable,
        } = resolve_android(
            packages,
            wanted.as_deref(),
            &previous,
            settings.scope,
            cleanup_unavailable,
        );
        let profile_tls = if settings.url_filtering {
            profile_tls(
                ctx,
                paths,
                &profiles,
                &apps,
                &trust_watched,
                &mut unavailable,
            )
        } else {
            BTreeMap::new()
        };
        (apps, profile_tls, unavailable)
    });
    Selection {
        settings,
        pauses,
        wanted,
        apps,
        profile_tls,
        unavailable,
    }
}

fn resolve_android(
    packages: &mut std::io::Result<watch::Packages>,
    wanted: Option<&str>,
    previous: &BTreeMap<u32, String>,
    scope: Scope,
    cleanup_unavailable: bool,
) -> Resolved {
    let Some(wanted) = wanted else {
        return Resolved {
            apps: BTreeMap::new(),
            profiles: Vec::new(),
            trust_watched: BTreeSet::new(),
            unavailable: true,
        };
    };
    let all = scope == Scope::All;
    let serials = android::selected_profiles(wanted);
    let chosen = apps::named(wanted);
    let wants_group = chosen
        .iter()
        .any(|token| Identity::parse(token).is_some_and(Identity::is_group));
    let mut unavailable = cleanup_unavailable;
    let mut profiles: Vec<_> = if serials.is_empty() {
        Vec::new()
    } else {
        android::profiles()
            .inspect_err(|_| unavailable = true)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| serials.contains(&p.serial))
            .collect()
    };
    if (!profiles.is_empty() || wants_group || all) && packages.is_err() {
        *packages = watch::Packages::watch();
    }
    let (watched, trust_watched) = packages
        .as_mut()
        .ok()
        .map(|watch| watch.set_profiles(profiles.iter().copied()))
        .unwrap_or_default();
    unavailable |= profiles.iter().any(|p| !watched.contains(&p.id));
    let started: Vec<_> = profiles
        .iter()
        .copied()
        .filter(|p| watched.contains(&p.id))
        .filter(|p| {
            p.started()
                .inspect_err(|_| unavailable = true)
                .unwrap_or(false)
        })
        .collect();
    let (mut found, packages_unavailable) = android::chosen(wanted, &started, previous, scope);
    unavailable |= packages_unavailable;
    found.retain(|&uid, token| {
        let group = Identity::parse(token).is_some_and(Identity::is_group);
        let observed = !(group || (all && uid < PER_USER_RANGE))
            || packages
                .as_ref()
                .is_ok_and(|watch| watch.observes_packages(uid / PER_USER_RANGE));
        unavailable |= !observed;
        observed
    });
    if !profiles.is_empty() {
        let current = android::profiles()
            .inspect_err(|_| unavailable = true)
            .unwrap_or_default();
        // A user number can be reused by Android, unlike its serial.
        profiles.retain(|p| current.contains(p));
        found.retain(|&uid, _| {
            uid < PER_USER_RANGE || profiles.iter().any(|p| p.id == uid / PER_USER_RANGE)
        });
    }
    Resolved {
        apps: found,
        profiles,
        trust_watched,
        unavailable,
    }
}

fn profile_tls(
    ctx: &mitm::Ctx,
    paths: &Paths,
    profiles: &[android::Profile],
    apps: &BTreeMap<u32, String>,
    trust_watched: &BTreeSet<u32>,
    unavailable: &mut bool,
) -> BTreeMap<u32, Arc<tls::Tls>> {
    profiles
        .iter()
        .filter(|p| apps.keys().any(|uid| uid / PER_USER_RANGE == p.id))
        .filter_map(|p| {
            let roots = trust::profile_roots(paths, p.id).ok()?;
            if !trust_watched.contains(&p.id) {
                *unavailable = true;
                return None;
            }
            let current = ctx
                .selection
                .profile_tls
                .read()
                .unwrap()
                .get(&p.serial)
                .filter(|current| current.has_roots(&roots))
                .cloned();
            Some((
                p.serial,
                current.unwrap_or_else(|| Arc::new(ctx.services.tls.with_roots(roots))),
            ))
        })
        .collect()
}

fn publish(
    ctx: &mitm::Ctx,
    mounts: &Arc<mounts::Mounts>,
    starts: &mut Option<watch::Starts>,
    selection: &mut Selection,
) -> (usize, usize) {
    let Selection {
        settings,
        pauses,
        wanted,
        apps,
        ..
    } = selection;
    // Prepare the trust mounts even for paused personal apps so HTTPS
    // inspection can resume when the pause ends.
    let mount_apps: BTreeMap<u32, String> = apps
        .iter()
        .filter(|(uid, _)| settings.url_filtering && **uid < PER_USER_RANGE)
        .map(|(uid, package)| (*uid, package.clone()))
        .collect();

    // Paused for every app: no rules at all.
    let count = apps.len();
    let active: BTreeMap<u32, String> = apps
        .iter()
        .filter(|(_, app)| !pauses.is_paused(app))
        .map(|(&uid, app)| (uid, app.clone()))
        .collect();
    if let Some(starts) = starts {
        starts.set(&mount_apps.keys().copied().collect());
    }
    mounts.set_apps(Arc::new(mount_apps));
    // Not waited for: each process is entered by a process of its own.
    let released = mounts.clone();
    tokio::spawn(async move { released.release().await });
    // A paused app, or an app whose profile is unavailable, is still chosen.
    let still_filtered = wanted.as_deref().map(|wanted| {
        abd_engine::apps::in_scope(
            wanted,
            settings.scope,
            |saved| saved.into_iter().collect::<BTreeSet<_>>(),
            |kept, _| {
                // Excluded or removed personal apps leave no pin history.
                kept.extend(apps.values().map(String::as_str));
            },
        )
    });
    let active_count = ctx.publish_selection(
        *settings,
        still_filtered.as_ref(),
        active,
        mitm::SelectionUpdate::Root(std::mem::take(&mut selection.profile_tls)),
    );
    (active_count, count - active_count)
}

/// Compare complete profile/package identities before removing paused
/// choices from the resolved set. Missing or changed owners force disarm.
pub(super) fn same_owners(
    previous: &BTreeMap<u32, String>,
    resolved: &BTreeMap<u32, String>,
) -> bool {
    previous
        .iter()
        .all(|(uid, owner)| resolved.get(uid) == Some(owner))
}

// The context also controls whether a captured connection can be read.
// If installing capture fails, clear the active identities so any detoured app passes.
pub(super) fn set_capture(
    apps: &RwLock<Arc<BTreeMap<u32, String>>>,
    install: impl FnOnce(&BTreeSet<u32>) -> Result<()>,
) -> Result<()> {
    let uids = apps.read().unwrap().keys().copied().collect();
    let result = install(&uids);
    if result.is_err() {
        *apps.write().unwrap() = Arc::default();
    }
    result
}

/// Immediately retires UIDs that no longer identify the selected apps.
fn retire_apps(
    ctx: &mitm::Ctx,
    rules: &mut CaptureRules,
    v6: bool,
    mut still_selected: impl FnMut(u32, abd_engine::apps::Identity<'_>) -> bool,
) {
    let mut active = ctx.selection.apps.write().unwrap();
    let before = active.len();
    Arc::make_mut(&mut active).retain(|&uid, token| {
        abd_engine::apps::Identity::parse(token)
            .is_some_and(|identity| still_selected(uid, identity))
    });
    if active.len() != before {
        drop(active);
        if let Err(error) = set_capture(&ctx.selection.apps, |uids| {
            tokio::task::block_in_place(|| rules.set(uids, v6.into(), rules::Owners::Unconfirmed))
        }) {
            eprintln!("rules: {error:#}; filtering nothing");
        }
    }
}

fn android_changed(
    ctx: &mitm::Ctx,
    rules: &mut CaptureRules,
    mounts: &Arc<mounts::Mounts>,
    paths: &Paths,
    v6: bool,
    changed: watch::Change,
) {
    if changed.package_owners || changed.profile_users.contains(&0) {
        mounts.invalidate_groups();
    }
    let roster_changed = changed.users
        && (ctx
            .selection
            .apps
            .read()
            .unwrap()
            .keys()
            .any(|&uid| uid >= PER_USER_RANGE)
            || !ctx.selection.profile_tls.read().unwrap().is_empty());
    if roster_changed || changed.package_owners {
        retire_changed_owners(
            ctx,
            rules,
            mounts,
            v6,
            roster_changed,
            changed.package_owners,
        );
    }
    let scope_changed = retire_changed_groups(ctx, rules, mounts, v6, &changed.profile_users);
    if !changed.trust.is_empty() {
        // Rule selection is still valid. Only TLS loses its verifier here.
        ctx.selection
            .profile_tls
            .write()
            .unwrap()
            .retain(|serial, _| !changed.trust.contains(serial));
    }
    if roster_changed || changed.package_owners || scope_changed || !changed.trust.is_empty() {
        write_state(ctx, paths, v6);
    }
}

fn retire_changed_owners(
    ctx: &mitm::Ctx,
    rules: &mut CaptureRules,
    mounts: &Arc<mounts::Mounts>,
    v6: bool,
    roster_changed: bool,
    owners_changed: bool,
) {
    // These snapshots can prove a UID changed owner. Retire it before
    // doing any per-user PackageManager queries.
    let roster = roster_changed.then(android::profiles);
    let owners = owners_changed.then(android::known_packages);
    if let Some(roster) = &roster {
        ctx.selection
            .profile_tls
            .write()
            .unwrap()
            .retain(|serial, _| {
                roster
                    .as_ref()
                    .is_ok_and(|profiles| profiles.iter().any(|p| p.serial == *serial && !p.quiet))
            });
    }
    if let Some(owners) = &owners
        && mounts.retain_apps(|uid, package| owners.get(&uid).is_some_and(|owner| owner == package))
    {
        let released = mounts.clone();
        tokio::spawn(async move { released.release().await });
    }
    retire_apps(ctx, rules, v6, |uid, identity| {
        (identity.profile_serial.is_none()
            || roster.as_ref().is_none_or(|roster| {
                roster.as_ref().is_ok_and(|profiles| {
                    profiles.iter().any(|p| {
                        p.id == uid / PER_USER_RANGE
                            && Some(p.serial) == identity.profile_serial
                            && !p.quiet
                    })
                })
            }))
            && owners.as_ref().is_none_or(|known| {
                known
                    .get(&(uid % PER_USER_RANGE))
                    .is_some_and(|name| name == identity.package)
            })
    });
}

fn retire_changed_groups(
    ctx: &mitm::Ctx,
    rules: &mut CaptureRules,
    mounts: &Arc<mounts::Mounts>,
    v6: bool,
    affected: &BTreeSet<u32>,
) -> bool {
    let mut group_users = changed_group_users(&ctx.selection.apps.read().unwrap(), affected);
    if affected.contains(&0) && mounts.has_groups() {
        group_users.insert(0);
    }
    if group_users.is_empty() {
        return false;
    }

    let by_user: BTreeMap<_, _> = tokio::task::block_in_place(|| {
        group_users
            .into_iter()
            .map(|user| (user, android::verified_groups(user)))
            .collect()
    });
    let still_selected = |uid: u32, package: &str| {
        !package.starts_with('@')
            || by_user
                .get(&(uid / PER_USER_RANGE))
                .is_none_or(|groups| groups.get(&uid).is_some_and(|group| group == package))
    };
    if by_user.contains_key(&0) && mounts.retain_apps(still_selected) {
        let released = mounts.clone();
        tokio::spawn(async move { released.release().await });
    }
    retire_apps(ctx, rules, v6, |uid, identity| {
        still_selected(uid, identity.package)
    });
    true
}
/// Only a surviving group needs the slower scoped membership query.
pub(super) fn changed_group_users(
    active: &BTreeMap<u32, String>,
    affected: &BTreeSet<u32>,
) -> BTreeSet<u32> {
    active
        .iter()
        .filter_map(|(&uid, token)| {
            let user = uid / PER_USER_RANGE;
            (affected.contains(&user)
                && abd_engine::apps::Identity::parse(token)
                    .is_some_and(abd_engine::apps::Identity::is_group))
            .then_some(user)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_choices_cannot_keep_prior_root_uids_or_admit_all_apps() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let previous = BTreeMap::from([
            (10_001, "@com.group".to_owned()),
            (1_010_001, "37:com.work".to_owned()),
        ]);
        std::fs::write(paths.apps(), [0xff]).unwrap();
        let wanted = android::selection(&paths.apps());
        assert!(wanted.is_err());
        let mut packages = Err(std::io::Error::other("no observer"));
        let resolved = resolve_android(
            &mut packages,
            wanted.as_deref().ok(),
            &previous,
            Scope::All,
            false,
        );
        assert!(resolved.apps.is_empty());
        assert!(resolved.unavailable);

        std::fs::write(paths.apps(), "").unwrap();
        let wanted = android::selection(&paths.apps()).unwrap();
        let resolved = resolve_android(
            &mut packages,
            Some(&wanted),
            &previous,
            Scope::Chosen,
            false,
        );
        assert!(resolved.apps.is_empty());
        assert!(!resolved.unavailable);
    }
}
