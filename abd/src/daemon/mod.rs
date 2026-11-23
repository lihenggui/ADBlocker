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

mod capture_rules;
mod ipv6;
mod owner;
mod reconcile;
mod reload;
mod setup;

use std::ops::ControlFlow;
use std::pin::Pin;
use std::time::Duration;

use abd_engine::mitm::SnapshotSelection;
use abd_engine::{mitm, state};
use anyhow::{Context as _, Result, bail};
use tokio::task::{JoinError, JoinHandle, JoinSet};
use tokio::time::{Instant, Sleep};

use self::ipv6::Ipv6;
use self::owner::Owner;
use self::reconcile::Reconciler;
use self::setup::{Setup, Signals};
use crate::paths::Paths;
use crate::watch;

pub async fn run(paths: Paths) -> Result<()> {
    let Setup {
        ctx,
        mounts,
        rules,
        accepting,
        event,
        signals,
    } = setup::open(&paths)?;
    let mut reconcile = Reconciler::new(&ctx, &paths, rules, mounts);
    reconcile.reapply(true);
    if let Err(e) = &reconcile.packages {
        eprintln!("packages: observer unavailable ({e})");
    }
    let mut daemon = Daemon {
        ctx: &ctx,
        paths: &paths,
        signals,
        accepting,
        listener_finished: false,
        reconcile,
        ipv6: Ipv6::new(event),
        android: AndroidChanges::new(),
        owner: Owner::new(),
        compiling: JoinSet::new(),
    };
    daemon.owner.on_packages();
    let stopped = daemon.drive().await;
    daemon.shutdown(stopped).await
}

struct Daemon<'a> {
    ctx: &'a mitm::Ctx,
    paths: &'a Paths,
    signals: Signals,
    accepting: JoinHandle<Result<()>>,
    listener_finished: bool,
    reconcile: Reconciler<'a>,
    ipv6: Ipv6,
    android: AndroidChanges,
    owner: Owner,
    compiling: JoinSet<reload::Compiled>,
}

impl Daemon<'_> {
    async fn drive(&mut self) -> Result<()> {
        loop {
            if let ControlFlow::Break(stopped) = self.next().await {
                return stopped;
            }
        }
    }

    async fn next(&mut self) -> ControlFlow<Result<()>> {
        let needs_tracking = self
            .reconcile
            .packages
            .as_ref()
            .is_ok_and(watch::Packages::has_profiles);
        self.android.track(needs_tracking);
        let v6 = self.ipv6.on;
        tokio::select! {
            result = &mut self.accepting => return ControlFlow::Break(self.listener_stopped(result)),
            _ = self.signals.term.recv() => return ControlFlow::Break(Ok(())),
            _ = self.signals.int.recv() => return ControlFlow::Break(Ok(())),
            _ = self.signals.hup.recv() => self.on_hup(),
            Some(result) = self.compiling.join_next(), if !self.compiling.is_empty() => {
                if let Err(error) = reload::on_compiled(&mut self.compiling, self.ctx, self.paths, v6, result) {
                    return ControlFlow::Break(Err(error));
                }
            }
            changed = package_changes(&mut self.reconcile.packages) => {
                self.reconcile.on_packages_changed(v6, changed);
                self.android.schedule_apply();
            }
            () = self.android.retry.fired(), if self.android.watch.is_none() => self.android.open_watch(),
            changed = user_states(&mut self.android.watch) => {
                self.android.on_user_state(changed);
                if self.reconcile.on_user_state(v6, changed) {
                    self.android.schedule_apply();
                }
            }
            () = &mut self.android.settle, if self.android.changed => {
                self.android.changed = false;
                self.owner.on_packages();
                self.reconcile.reapply(v6);
            }
            () = self.owner.gone() => {
                eprintln!("the app is uninstalled: nothing is filtered until it starts the module again");
                return ControlFlow::Break(Ok(()));
            }
            () = reconcile::due(&mut self.reconcile.pause_timer, &mut self.reconcile.apps_retry) => self.reconcile.reapply(v6),
            uid = app_start(&mut self.reconcile.starts) => self.reconcile.on_app_started(uid),
            Some(mitm::Event::V6Suspect) = self.ipv6.suspects.recv() => self.ipv6.on_suspect(&self.ctx.ipv6),
            Some(reachable) = self.ipv6.probe.recv() => self.ipv6.on_probe(&self.ctx.ipv6, &mut self.reconcile, reachable),
            () = &mut self.ipv6.retry, if !v6 => self.ipv6.on_retry(&self.ctx.ipv6, &mut self.reconcile),
        }
        ControlFlow::Continue(())
    }

    fn on_hup(&mut self) {
        reload::on_signal(&mut self.compiling, self.ctx, self.paths);
        self.reconcile.reapply(self.ipv6.on);
    }

    fn listener_stopped(&mut self, result: Result<Result<()>, JoinError>) -> Result<()> {
        self.listener_finished = true;
        result
            .context("capture listener failed")
            .and_then(|result| result)
            .and_then(|()| bail!("capture listener stopped unexpectedly"))
    }

    async fn shutdown(self, stopped: Result<()>) -> Result<()> {
        let Self {
            mut reconcile,
            ctx,
            paths,
            listener_finished,
            accepting,
            ..
        } = self;
        // Leave both listeners running until capture has been removed.
        // The supervisor also retries cleanup if the daemon exits unexpectedly.
        let removed = reconcile.remove_capture();
        if let Err(error) = &removed {
            eprintln!("rules: {error:#}");
        }
        let listener = if listener_finished {
            Ok(())
        } else {
            accepting.abort();
            match accepting.await {
                Err(error) if error.is_cancelled() => Ok(()),
                Err(error) => Err(error).context("capture listener failed"),
                Ok(Err(error)) => Err(error),
                Ok(Ok(())) => Err(anyhow::anyhow!("capture listener stopped unexpectedly")),
            }
        };
        if let Err(error) = ctx.services.stats.finish_writing(&paths.stats()) {
            eprintln!("stats: could not save counts on shutdown: {error}");
        }
        if let Err(error) = ctx.services.policy.write() {
            eprintln!("pins: could not save the table on shutdown: {error}");
        }
        // Tidiness only: the next daemon removes a stale socket before it
        // listens, and until then nothing answers on it.
        let _ = std::fs::remove_file(paths.watch());
        eprintln!("stopped");
        stopped.and(listener).and(removed)
    }
}

async fn package_changes(packages: &mut std::io::Result<watch::Packages>) -> watch::Change {
    match packages {
        Ok(packages) => packages.changed().await,
        Err(_) => std::future::pending().await,
    }
}

async fn user_states(watch: &mut Option<watch::UserStates>) -> Option<(u32, u32)> {
    match watch {
        Some(watch) => watch.changed().await,
        None => std::future::pending().await,
    }
}

async fn app_start(starts: &mut Option<watch::Starts>) -> u32 {
    match starts {
        Some(starts) => starts.started().await,
        None => std::future::pending().await,
    }
}

/// How long after the list of packages was replaced it is read: an
/// installation replaces it more than once.
const PACKAGES_SETTLE: Duration = Duration::from_secs(2);

/// Android's users starting and stopping, and the wait after a change of
/// them or of the packages before the apps are read again.
struct AndroidChanges {
    watch: Option<watch::UserStates>,
    retry: watch::Retry,
    tracking: bool,
    settle: Pin<Box<Sleep>>,
    changed: bool,
}

impl AndroidChanges {
    fn new() -> Self {
        Self {
            watch: None,
            retry: watch::Retry::new(),
            tracking: false,
            settle: Box::pin(tokio::time::sleep(PACKAGES_SETTLE)),
            changed: false,
        }
    }

    fn track(&mut self, needs_tracking: bool) {
        if needs_tracking != self.tracking {
            self.tracking = needs_tracking;
            self.watch = needs_tracking
                .then(watch::UserStates::watch)
                .and_then(Result::ok);
            self.retry.arm(false);
        }
        if needs_tracking && self.watch.is_none() {
            self.retry.arm(true);
        }
    }

    fn open_watch(&mut self) {
        self.watch = watch::UserStates::watch().ok();
    }

    fn on_user_state(&mut self, changed: Option<(u32, u32)>) {
        if changed.is_none() {
            self.watch = None;
        } else {
            self.retry.arm(false);
        }
    }

    fn schedule_apply(&mut self) {
        self.changed = true;
        self.settle.as_mut().reset(Instant::now() + PACKAGES_SETTLE);
    }
}

fn write_state(ctx: &mitm::Ctx, paths: &Paths, v6: bool) {
    let state = ctx.snapshot(v6, SnapshotSelection::Root);
    if let Err(error) = state::write(&paths.state(), &state) {
        eprintln!("state: could not publish snapshot: {error:#}");
    }
}

#[cfg(test)]
mod reload_tests;
