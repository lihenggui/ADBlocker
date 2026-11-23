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

//! The engine as the app holds it: opened once, asked with the commands
//! the daemon is asked with, and given the VPN's descriptor to filter by.
//!
//! This is everything `jni.rs` exports, in plain Rust, so that it is
//! tested here without a device. The contract is docs/engine-jni.md.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use abd_engine::cli::{self, Host};
use abd_engine::malformed::Malformed;
use abd_engine::{apps, clock, live, pause};
use anyhow::{Context as _, Result, bail};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject as _;
use tokio::io::{AsyncBufReadExt as _, BufReader, DuplexStream};
use tokio::sync::{Semaphore, oneshot, watch};

use crate::engine::Engine;
use crate::fd::Fd;
use crate::log::log;
use crate::platform::Platform;
use crate::tunnel;

/// The longest packet the apps may send: what the VPN is set up with.
/// Their connections end here, not on the path beyond, so a packet is
/// as large as moves the most for each one passed.
pub const MTU: usize = 9000;

/// Bytes of watch output between the async feed and the app's callbacks.
const WATCH_BUFFER: usize = 64 << 10;

const SHUTDOWN_TIME: Duration = Duration::from_millis(250);

/// What a command ended with, as the daemon's would: 0, 2 for one that is
/// malformed or not known, 1 for any other failure.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub exit: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Takes the lines of a command that goes on.
pub trait Sink: Send {
    fn line(&mut self, text: &str);
    /// Called once, last.
    fn end(&mut self, exit: i32);
}

struct RunningTunnel {
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
    ipv6: bool,
}

pub struct Session {
    engine: Arc<Engine>,
    roots: Arc<rustls::RootCertStore>,
    /// The VPN, while it is filtered: ended by sending or dropping this.
    tunnel: Mutex<Option<RunningTunnel>>,
    streams: Arc<Mutex<HashMap<u64, oneshot::Sender<()>>>>,
    /// A callback can still be returning after its watch has been canceled.
    watch_writers: Arc<Semaphore>,
    next: AtomicU64,
    runtime: Option<tokio::runtime::Runtime>,
}

/// The session, as the engine's commands need it.
struct InApp<'a>(&'a Session);

impl Host for InApp<'_> {
    fn running(&self) -> Option<i32> {
        self.0.running(|_| std::process::id() as i32)
    }
    fn reload(&self) {
        self.0.reload();
    }
    fn manages_pins(&self) -> bool {
        true
    }
    fn reload_pins(&self) -> Result<()> {
        self.0.engine.reload_pins()?;
        Ok(())
    }
    fn version(&self) -> Option<String> {
        Some(format!("v{}", env!("CARGO_PKG_VERSION")))
    }
    fn boot_guard(&self) -> Option<u64> {
        None
    }
    fn vpn_ipv6(&self) -> Option<bool> {
        self.0.running(|tunnel| tunnel.ipv6)
    }
    fn personal_app_identities(
        &self,
        exclusions: &std::collections::BTreeSet<&str>,
    ) -> Vec<String> {
        self.0
            .engine
            .personal_app_identities(exclusions)
            .into_values()
            .collect()
    }
    fn reports_filtering_apps(&self) -> bool {
        true
    }
    fn undecryptable(&self) -> Option<&'static str> {
        if !self.0.engine.ca_installed() {
            Some("ca_not_installed")
        } else {
            (!self.0.engine.exclusions_ready()).then_some("exclusions_pending")
        }
    }
}

/// Calls the app in order, on a blocking thread. After cancellation or
/// the feed's write deadline, leave any queued lines behind.
async fn write_watch(
    input: DuplexStream,
    mut finished: oneshot::Receiver<i32>,
    sink: &mut dyn Sink,
) -> i32 {
    let mut lines = BufReader::new(input).lines();
    loop {
        tokio::select! {
            biased;
            exit = &mut finished => return exit.unwrap_or(0),
            line = lines.next_line() => match line {
                Ok(Some(line)) => sink.line(&line),
                Ok(None) => return finished.await.unwrap_or(0),
                Err(_) => return 1,
            },
        }
    }
}

/// Waits for the current deadline, rearming when a reload changes it.
async fn wait_for_pause_end(
    ends: &mut watch::Receiver<Option<u64>>,
    now: impl Fn() -> u64,
) -> bool {
    let mut timer = pause::Timer::default();
    loop {
        timer.arm(*ends.borrow_and_update(), now());
        tokio::select! {
            changed = ends.changed() => {
                if changed.is_err() {
                    return false;
                }
            }
            () = timer.fired() => return true,
        }
    }
}

impl Session {
    /// Opens the engine on the data in `dir`, with the certificates
    /// servers are verified against, as the system gives them.
    pub fn open(dir: &Path, roots: &[Vec<u8>], platform: Arc<dyn Platform>) -> Result<Self> {
        let mut store = rustls::RootCertStore::empty();
        for root in roots {
            // One the library cannot read is left out, as it would be
            // from a store read off the disk.
            let _ = store.add(CertificateDer::from(root.clone()));
        }
        if store.is_empty() {
            bail!("no roots to verify servers against");
        }
        // Keep the bounded Binder workers attached across bursts of traffic.
        let runtime = abd_engine::runtime::builder()
            .thread_keep_alive(Duration::from_secs(120))
            .build()?;
        let engine = {
            let _on = runtime.enter();
            Engine::open(dir, store.clone(), platform)?
        };
        Ok(Self {
            engine,
            roots: Arc::new(store),
            tunnel: Mutex::new(None),
            streams: Arc::default(),
            watch_writers: Arc::new(Semaphore::new(live::MAX_WATCHERS)),
            next: AtomicU64::new(1),
            runtime: Some(runtime),
        })
    }

    fn runtime(&self) -> &tokio::runtime::Runtime {
        self.runtime.as_ref().expect("live session has a runtime")
    }

    /// What `read` says of the VPN while it is filtered. Not after its task
    /// ended by itself, which `stop` has not cleared yet.
    fn running<T>(&self, read: impl FnOnce(&RunningTunnel) -> T) -> Option<T> {
        let tunnel = self.tunnel.lock().unwrap();
        tunnel
            .as_ref()
            .filter(|tunnel| !tunnel.task.is_finished())
            .map(read)
    }

    fn reload(&self) {
        let _on = self.runtime().enter();
        self.engine.reload();
    }

    /// Does one command. Not for the two that go on (`stream`).
    pub fn command(&self, args: &[&str], stdin: &[u8]) -> Output {
        let mut out = Vec::new();
        let done = match args {
            ["reload"] => {
                self.reload();
                Ok(())
            }
            ["ca", "export", rest @ ..] => self.ca(rest, &mut out),
            _ => {
                let mut input = stdin;
                cli::run(
                    self.engine.paths(),
                    &InApp(self),
                    args,
                    &mut input,
                    &mut out,
                )
            }
        };
        match done {
            Ok(()) => Output {
                exit: 0,
                stdout: out,
                stderr: Vec::new(),
            },
            Err(e) => Output {
                exit: if e.downcast_ref::<Malformed>().is_some() {
                    2
                } else {
                    1
                },
                stdout: out,
                stderr: format!("{e:#}\n").into_bytes(),
            },
        }
    }

    /// The authority's certificate, for the user to put among those the
    /// device trusts.
    fn ca(&self, how: &[&str], out: &mut Vec<u8>) -> Result<()> {
        let pem =
            std::fs::read(self.engine.paths().ca().join("user.pem")).context("the authority")?;
        match how {
            [] => out.extend_from_slice(&pem),
            ["--der"] => out.extend_from_slice(&CertificateDer::from_pem_slice(&pem)?),
            _ => bail!(Malformed("ca export takes --der or nothing".to_owned())),
        }
        Ok(())
    }

    /// Starts a command that goes on, `watch` or `update`, saying its
    /// lines to `sink`. Returns what to end it by, or 0 when the
    /// command cannot start and the sink is ended at once.
    pub fn stream(&self, args: &[&str], mut sink: Box<dyn Sink>) -> u64 {
        let id = self.next.fetch_add(1, Relaxed);
        match args {
            ["watch", rest @ ..] => self.stream_watch(id, rest, sink),
            ["update", rest @ ..] => self.stream_update(id, rest, sink),
            _ => {
                sink.end(2);
                0
            }
        }
    }

    fn stream_watch(&self, id: u64, args: &[&str], mut sink: Box<dyn Sink>) -> u64 {
        let Ok(app) = cli::parse_watch_args(args, apps::valid_package) else {
            sink.end(2);
            return 0;
        };
        let app = app.map(str::to_owned);
        let Ok(writer) = self.watch_writers.clone().try_acquire_owned() else {
            sink.end(1);
            return 0;
        };
        let Some(mut joined) = self.engine.live().join(app) else {
            sink.end(1);
            return 0;
        };
        let (cancelled, ended) = self.register_stream(id);
        let (input, mut output) = tokio::io::duplex(WATCH_BUFFER);
        let (finish, finished) = oneshot::channel();
        self.runtime().spawn(async move {
            let exit = tokio::select! {
                biased;
                _ = cancelled => 0,
                fed = live::feed(&mut joined, &mut output) => i32::from(fed.is_err()),
            };
            drop(joined);
            let _ = finish.send(exit);
        });
        self.runtime().spawn_blocking(move || {
            let exit = tokio::runtime::Handle::current().block_on(write_watch(
                input,
                finished,
                sink.as_mut(),
            ));
            ended(sink.as_mut(), exit);
            drop(writer);
        });
        id
    }

    fn stream_update(&self, id: u64, args: &[&str], mut sink: Box<dyn Sink>) -> u64 {
        let Ok(options) = cli::parse_update_args(args) else {
            sink.end(2);
            return 0;
        };
        let (engine, roots) = (self.engine.clone(), self.roots.clone());
        let (cancelled, ended) = self.register_stream(id);
        // On a thread of its own: what it holds while it waits is
        // not to be passed between the runtime's.
        self.runtime().spawn_blocking(move || {
            let mut say = |line: &str| sink.line(line);
            let fetch = cli::update::run_to(engine.paths(), roots, options, &mut say);
            let exit = tokio::runtime::Handle::current().block_on(async {
                tokio::select! {
                    changed = fetch => match changed {
                        Ok(changed) => {
                            if changed > 0 {
                                engine.reload();
                            }
                            0
                        }
                        Err(error) => {
                            // Of the lists and the files here: nothing of
                            // what the apps talk to.
                            log(&format!("update: {error:#}"));
                            1
                        }
                    },
                    _ = cancelled => 0,
                }
            });
            ended(sink.as_mut(), exit);
        });
        id
    }

    fn register_stream(
        &self,
        id: u64,
    ) -> (
        oneshot::Receiver<()>,
        impl FnOnce(&mut dyn Sink, i32) + use<>,
    ) {
        let (cancel, cancelled) = oneshot::channel();
        self.streams.lock().unwrap().insert(id, cancel);
        let streams = self.streams.clone();
        let ended = move |sink: &mut dyn Sink, exit| {
            streams.lock().unwrap().remove(&id);
            sink.end(exit);
        };
        (cancelled, ended)
    }

    /// Ends a command that goes on. Its sink is still told the end.
    pub fn cancel(&self, id: u64) {
        if let Some(cancel) = self.streams.lock().unwrap().remove(&id) {
            let _ = cancel.send(());
        }
    }

    /// Filters what comes by the VPN's descriptor, which is ours from here
    /// on, until `stop`. If it ends by itself, `stopped` is told.
    pub fn start(
        &self,
        tun: OwnedFd,
        ipv6: bool,
        resolvers: Vec<SocketAddr>,
        stopped: Box<dyn FnOnce(i32) + Send>,
    ) -> Result<()> {
        self.stop();
        let _on = self.runtime().enter();
        let tun = Fd::new(tun).context("the VPN's descriptor")?;
        self.reload();
        let engine = self.engine.clone();
        let (stop, asked) = oneshot::channel();
        let dns = engine.dns(resolvers);
        let task = self.runtime().spawn(async move {
            let pauses = async {
                let mut ends = engine.apply_at();
                while wait_for_pause_end(&mut ends, clock::now).await {
                    // Applying reads files, asks Android about the apps and
                    // waits for a reload: on a thread of its own, while this
                    // task goes on with the packets.
                    let engine = engine.clone();
                    match tokio::task::spawn_blocking(move || engine.apply()).await {
                        Ok(_) => {}
                        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
                        // The runtime is shutting down, and this task with it.
                        Err(_) => return,
                    }
                }
            };
            tokio::select! {
                ended = tunnel::run(tun, tunnel::Config {
                    mtu: MTU,
                    handler: engine.handler(),
                    reach: Some(engine.reach()),
                    protect: engine.protect(),
                    policy: engine.udp_policy(),
                    dns,
                }) => {
                    if let Err(error) = &ended {
                        // Its kind only, which can name no host or app.
                        let (kind, code) = (error.kind(), error.raw_os_error());
                        log(&format!("the VPN ended: {kind:?}, os error {code:?}"));
                    }
                    stopped(i32::from(ended.is_err()));
                }
                () = pauses => {}
                _ = asked => {}
            }
        });
        *self.tunnel.lock().unwrap() = Some(RunningTunnel { stop, task, ipv6 });
        Ok(())
    }

    /// Stops filtering and closes the VPN's descriptor. Returns once that
    /// is done.
    pub fn stop(&self) {
        let running = self.tunnel.lock().unwrap().take();
        if let Some(RunningTunnel { stop, task, .. }) = running {
            let _ = stop.send(());
            let _ = self.runtime().block_on(task);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.streams.lock().unwrap().clear();
        self.stop();
        // Blocking Binder calls can outlive their deadlines. Tokio
        // cannot interrupt them, and closing a session must not wait
        // indefinitely for Android. A late callback retains its own
        // resources and permit until it returns.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(SHUTDOWN_TIME);
        }
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
