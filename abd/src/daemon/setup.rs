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

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use abd_engine::apps::PER_USER_RANGE;
use abd_engine::{ca, exclusions, filter, live, mitm, pins, relay, state, tls};
use anyhow::{Context as _, Result};
use tokio::net::{TcpListener, TcpStream};
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::capture_rules::CaptureRules;
use crate::paths::Paths;
use crate::{capture, dns, mounts, rules, sockdiag, system_roots, trust, watch};

pub(super) struct Signals {
    pub(super) term: Signal,
    pub(super) int: Signal,
    pub(super) hup: Signal,
}

pub(super) struct Setup {
    pub(super) ctx: Arc<mitm::Ctx>,
    pub(super) mounts: Arc<mounts::Mounts>,
    pub(super) rules: CaptureRules,
    pub(super) accepting: JoinHandle<Result<()>>,
    pub(super) event: mpsc::UnboundedReceiver<mitm::Event>,
    pub(super) signals: Signals,
}

pub(super) fn open(paths: &Paths) -> Result<Setup> {
    state::started();
    let verbose = std::env::var_os("ABD_VERBOSE").is_some();
    // Made here, with the mode of everything else, rather than by whoever
    // first puts a file in them.
    for dir in [paths.lists(), paths.rules()] {
        std::fs::create_dir_all(dir)?;
    }
    // Before anything that takes time: the pid file is there already, and
    // a signal with nobody to take it would end us.
    let term = signal(SignalKind::terminate())?;
    let int = signal(SignalKind::interrupt())?;
    let hup = signal(SignalKind::hangup())?;
    let (events, event) = mpsc::unbounded_channel();
    let (ctx, mounts) = context(paths, verbose, events)?;

    // Found out now, not by the first connection handed on.
    relay::kernel_splices();
    let (v4, v6, port) = capture::listeners()?;

    let rules = CaptureRules::new(rules::Rules::start(port)?, dns::Dns::start(&ctx));

    // Packet filter tools can wait for xtables. Keep accepting captures
    // while the serialized configuration task waits for them.
    let connection_ctx = ctx.clone();
    let accepting = tokio::spawn(accept_connections(v4, v6, move |stream| {
        let ctx = connection_ctx.clone();
        tokio::spawn(async move {
            let origin = delivered(&stream);
            let verbose = ctx.hooks.verbose;
            if let Err(error) = mitm::handle(ctx, stream, origin).await
                && verbose
            {
                eprintln!("error   {error:#}");
            }
        });
    }));
    Ok(Setup {
        ctx,
        mounts,
        rules,
        accepting,
        event,
        signals: Signals { term, int, hup },
    })
}

fn context(
    paths: &Paths,
    verbose: bool,
    events: mpsc::UnboundedSender<mitm::Event>,
) -> Result<(Arc<mitm::Ctx>, Arc<mounts::Mounts>)> {
    let records = mitm::Records::open(paths);
    // Asked while no daemon ran.
    if let Err(error) = pins::take_requests(&records.policy, &paths.forget()) {
        eprintln!("pins: pending removals could not be applied: {error}");
    }
    if let Err(e) = live::listen(&paths.watch(), records.live.clone()) {
        eprintln!("watch: not listening ({e}); `abd watch` has nothing to show");
    }
    // The copies of the stores, for the apps' processes to be given.
    let authority = ca::Authority::open(&paths.ca())?;
    tokio::task::block_in_place(|| trust::prepare(paths))?;
    // A process given the mounts may try once more the hosts its app pins.
    let policy = records.policy.clone();
    let mounts =
        Arc::new(mounts::Mounts::new()?.telling(move |package| policy.process_started(package)));
    let tls = tls::Tls::new(authority, system_roots::read(paths)?);
    let lists = mitm::Lists::open(load_filters(paths, verbose)?, load_exclusions(paths));
    let ctx = Arc::new(mitm::Ctx::open(
        lists,
        tls,
        records,
        mitm::Hooks {
            verbose,
            dial: Box::new(|origin| {
                Box::pin(capture::connect_as(
                    origin.dst(),
                    origin.mark(),
                    origin.uid(),
                ))
            }),
            trusted: {
                let mounts = mounts.clone();
                Box::new(move |uid| {
                    let mounts = mounts.clone();
                    Box::pin(async move {
                        if uid < PER_USER_RANGE {
                            mounts.ensure(uid).await
                        } else {
                            // Managed apps are never mounted into. The engine
                            // checks that this profile has provisioned trust.
                            mitm::Trusting::Yes
                        }
                    })
                })
            },
        },
        mitm::InitialTrust::Available,
        mitm::Ipv6Health::new(events),
    ));
    ctx.keep_written(paths);

    Ok((ctx, mounts))
}

pub(super) fn watch_starts() -> Option<watch::Starts> {
    // Where the system keeps a directory for every process it starts.
    // Of the places it may be, the one that has the apps' directories.
    let groups = mounts::GROUPS.map(Path::new);
    let holds_apps = |dir: &&&Path| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().as_encoded_bytes().starts_with(b"uid_"))
        })
    };
    let groups = groups.iter().find(holds_apps).unwrap_or(&groups[0]);
    let starts = watch::Starts::watch(groups).ok();
    if starts.is_none() {
        eprintln!("processes: not watched; an app is made to trust us at its first connection");
    }
    starts
}

pub(super) fn load_filters(paths: &Paths, verbose: bool) -> Result<filter::Filters> {
    let started = Instant::now();
    let dirs = [paths.lists(), paths.rules()];
    let dirs = [dirs[0].as_path(), dirs[1].as_path()];
    // The engine that names the rule that matched holds the text of every
    // rule: not the one that is kept.
    let f = if verbose {
        filter::Filters::load(&dirs, true)?
    } else {
        filter::Filters::load_cached(&dirs, &paths.cache())?
    };
    eprintln!(
        "lists: {} lines, {} of them whole-host rules, read in {:?}",
        f.rules,
        f.host_rules,
        started.elapsed()
    );
    if f.rules == 0 {
        eprintln!("lists: none yet; run `abd update`");
    }
    Ok(f)
}

fn load_exclusions(paths: &Paths) -> exclusions::Exclusions {
    let e = exclusions::Exclusions::load(&paths.exclusions());
    eprintln!("exclusions: {} hosts never decrypted", e.len());
    e
}

/// What a connection TPROXY delivered is: its local address is the server
/// the app was connecting to. None for one that came any other way.
fn delivered(app: &tokio::net::TcpStream) -> Option<mitm::Origin> {
    let dst = app.local_addr().ok()?;
    let uid = sockdiag::uid_of(app.peer_addr().ok()?, dst);
    capture::admitted(dst, capture::mark_of(app), uid)
}

pub(super) async fn accept_connections(
    v4: TcpListener,
    v6: TcpListener,
    dispatch: impl Fn(TcpStream),
) -> Result<()> {
    loop {
        let accepted = tokio::select! {
            result = v4.accept() => result,
            result = v6.accept() => result,
        };
        match accepted {
            Ok((stream, _)) => dispatch(stream),
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::EBADF | libc::EINVAL | libc::ENOTSOCK)
                ) =>
            {
                return Err(error).context("accepting captured connections");
            }
            // Out of descriptors, most likely: let some connections end.
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}
