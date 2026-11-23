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

//! The engine, run by the app: what the module's daemon is to it under
//! root.
//!
//! What only the system can say, the app is asked ([`Platform`]): whose a
//! connection is, which UID a package has, and whether the user had the
//! authority put among the certificates the device trusts. Without root
//! nothing else can make an app trust it, so an app is decrypted only
//! where the user did that and asked for decryption as well; until then
//! every app has its hosts judged and nothing more.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use abd_engine::mitm::{
    self, Ctx, Dial, Hooks, InitialTrust, Ipv6Health, Lists, Origin, Records, SelectionUpdate,
    SnapshotSelection, Trusting,
};
use abd_engine::paths::Paths;
use abd_engine::settings::Settings;
use abd_engine::{apps, ca, clock, exclusions, filter, live, pause, pins, state};
use abd_engine::{relay, tls};
use anyhow::{Context as _, Result};
use tokio::net::TcpStream;
use tokio::sync::watch;

use crate::log::log;
pub use crate::platform::{Connect, Platform};
use crate::platform::{Protect, connect, protector, tcp_owner};
use crate::tunnel::Handler;

const CONFIG_RETRY_SECS: u64 = 5;

#[derive(Default)]
struct UdpSelection {
    generation: u64,
    // A recycled UID must not receive datagrams retained for the previous app.
    owners: BTreeMap<u32, (String, u64)>,
}

impl UdpSelection {
    fn replace(&mut self, apps: &[(u32, &str)]) {
        self.generation += 1;
        self.owners = apps
            .iter()
            .map(|&(uid, package)| {
                let epoch = self
                    .owners
                    .get(&uid)
                    .filter(|(previous, _)| previous == package)
                    .map_or(self.generation, |(_, epoch)| *epoch);
                (uid, (package.to_owned(), epoch))
            })
            .collect();
    }
}

/// A connection made to a server before its app was answered, to find out
/// that the server is there (`Engine::reach`).
struct Made {
    server: TcpStream,
    /// Whose the app's connection is, as the system said then.
    uid: Option<u32>,
    at: Instant,
}

/// The connections made so, for the engine to take when it connects for
/// the app: each by the number its app's connection was given.
type Ready = Arc<Mutex<HashMap<u32, TcpStream>>>;

/// The most connections made and not yet taken by a connection of an app,
/// and how long one is kept for it: an app that began to connect and went
/// away leaves one behind.
const MAX_MADE: usize = 256;
const MADE_TIME: Duration = Duration::from_secs(30);

pub struct Engine {
    /// Includes paused apps whose traffic still arrives through the VPN.
    selected: RwLock<UdpSelection>,
    ctx: Arc<Ctx>,
    paths: Paths,
    platform: Arc<dyn Platform>,
    connect: Connect,
    /// By the app's address and the server's.
    made: Mutex<HashMap<(SocketAddr, SocketAddr), Made>>,
    ready: Ready,
    /// The number given to the last connection that had one made for it.
    last: AtomicU32,
    /// A reload and a pause ending both apply the config. Keep the
    /// filtering state and its next deadline in the same order.
    loading: Mutex<()>,
    apply_at: watch::Sender<Option<u64>>,
}

/// Connects for the engine: with the connection made before the app was
/// answered, the first time, and anew after that.
fn dial(connect: Connect, ready: Ready) -> Dial {
    Box::new(move |origin: Origin| {
        let made = ready.lock().unwrap().remove(&origin.mark());
        let connect = connect.clone();
        Box::pin(async move {
            match made {
                Some(server) => Ok(server),
                None => connect(origin.dst()).await,
            }
        })
    })
}

impl Engine {
    /// Opens the engine on the data in `dir`, verifying servers against
    /// `roots`.
    pub fn open(
        dir: &Path,
        roots: rustls::RootCertStore,
        platform: Arc<dyn Platform>,
    ) -> Result<Arc<Self>> {
        let connect = connect(protector(platform.clone()));
        Self::open_with(dir, roots, platform, connect)
    }

    /// As [`Engine::open`], connecting to servers by `connect`: for the
    /// tests, which have no network.
    pub fn open_with(
        dir: &Path,
        roots: rustls::RootCertStore,
        platform: Arc<dyn Platform>,
        connect: Connect,
    ) -> Result<Arc<Self>> {
        let ready = Ready::default();
        let dial = dial(connect.clone(), ready.clone());
        let paths = Paths::new(dir);
        for dir in [paths.lists(), paths.rules()] {
            std::fs::create_dir_all(dir)?;
        }
        let authority = ca::Authority::open(&paths.ca()).context("the authority")?;
        let dirs = [paths.lists(), paths.rules()];
        let filters = filter::Filters::load_cached(&[&dirs[0], &dirs[1]], &paths.cache())?;
        let records = Records::open(&paths);
        if let Err(error) = pins::take_requests(&records.policy, &paths.forget()) {
            log(&format!(
                "pins: pending removals could not be applied: {error}"
            ));
        }
        let tls = tls::Tls::new(authority, roots);
        let lists = Lists::open(filters, exclusions::Exclusions::load(&paths.exclusions()));
        let ctx = Arc::new(Ctx::open(
            lists,
            tls,
            records,
            Hooks {
                verbose: false,
                dial,
                // VPN certificate availability is published in `apply`.
                trusted: Box::new(|_| Box::pin(async { Trusting::Yes })),
            },
            InitialTrust::AwaitingCertificate,
            Ipv6Health::default(),
        ));
        ctx.keep_written(&paths);
        relay::kernel_splices();
        let engine = Arc::new(Self {
            selected: RwLock::default(),
            ctx,
            paths,
            platform,
            connect,
            made: Mutex::default(),
            ready,
            last: AtomicU32::new(0),
            loading: Mutex::default(),
            apply_at: watch::Sender::new(None),
        });
        engine.apply();
        Ok(engine)
    }

    /// Reads again what the commands change, the lists among it.
    pub fn reload(&self) {
        let _loading = self.loading.lock().unwrap();
        let dirs = [self.paths.lists(), self.paths.rules()];
        let reload = || -> Result<()> {
            let dirs = [&*dirs[0], &*dirs[1]];
            let mut files = filter::ListFiles::read(&dirs)?;
            for _ in 0..2 {
                if Some(files.generation) == self.ctx.lists.filters.read().unwrap().generation {
                    return Ok(());
                }
                let filters = files.filters(Some(&self.paths.cache()), false)?;
                let current = filter::ListFiles::read(&dirs)?;
                if files.generation == current.generation {
                    *self.ctx.lists.filters.write().unwrap() = Arc::new(filters);
                    return Ok(());
                }
                files = current;
            }
            anyhow::bail!("the lists are changing")
        };
        if let Err(e) = reload() {
            log(&format!("lists: kept as they were: {e}"));
        }
        self.ctx.reload_sources(&self.paths, pins::take_requests);
        self.apply_locked();
    }

    pub(crate) fn personal_app_identities(
        &self,
        exclusions: &BTreeSet<&str>,
    ) -> BTreeMap<u32, String> {
        self.platform.personal_app_identities(exclusions)
    }

    /// Reads the chosen apps, the pauses and the settings.
    pub fn apply(&self) -> Option<u64> {
        let _loading = self.loading.lock().unwrap();
        self.apply_locked()
    }

    fn apply_locked(&self) -> Option<u64> {
        let now = clock::now();
        let text = abd_engine::files::read_text(&self.paths.apps(), apps::MAX_FILE)
            .inspect_err(|error| log(&format!("app selection: {error}")))
            .ok();
        let (text, settings, pauses) = match self.ctx.reload_user_policy(&self.paths, now) {
            Ok((settings, pauses)) => (text, settings, pauses),
            Err(error) => {
                log(&format!("user policy: {error:#}"));
                (None, Settings::UNSAID, pause::Pauses::default())
            }
        };
        let named = text.as_deref().map(apps::named).unwrap_or_default();
        let personal;
        // Paused apps stay routed, but the filtered map excludes them.
        let selected: Vec<_> = if let Some(text) = &text
            && settings.scope == abd_engine::settings::Scope::All
        {
            personal = self.personal_app_identities(&apps::excluded(text));
            personal
                .iter()
                .map(|(&uid, name)| (uid, name.as_str()))
                .collect()
        } else {
            named
                .iter()
                .filter(|name| apps::valid_package(name))
                .filter_map(|&package| Some((self.platform.uid_of(package)?, package)))
                .collect()
        };
        // Asked for or not, nothing is decrypted for an app that would
        // not have our certificate: it would refuse us for every host.
        // Android is asked before the selection is held, which the UDP
        // relays wait for on the runtime's workers.
        let inspectable = settings.url_filtering && self.platform.ca_installed();
        let mut admission = self.selected.write().unwrap();
        admission.replace(&selected);
        let filtered: BTreeMap<u32, String> = selected
            .iter()
            .filter(|(_, package)| !pauses.is_paused(package))
            .map(|&(uid, package)| (uid, package.to_owned()))
            .collect();
        self.ctx
            .selection
            .https_inspection_available
            .store(inspectable, Relaxed);
        let chosen: BTreeSet<_> = if settings.scope == abd_engine::settings::Scope::All {
            selected.iter().map(|(_, package)| *package).collect()
        } else {
            named.into_iter().collect()
        };
        let filtering_apps = self.ctx.publish_selection(
            settings,
            text.is_some().then_some(&chosen),
            filtered,
            SelectionUpdate::Vpn,
        );
        drop(admission);
        // What the engine holds, for `status`: as the daemon says it.
        let state = self
            .ctx
            .snapshot(true, SnapshotSelection::Vpn(filtering_apps));
        if let Err(error) = state::write(&self.paths.state(), &state) {
            log(&format!("state: could not publish snapshot: {error:#}"));
        }
        let ends = pauses.next_end();
        let apply_at = if text.is_some() {
            ends
        } else {
            Some(clock::now() + CONFIG_RETRY_SECS)
        };
        self.apply_at.send_if_modified(|old| {
            let changed = *old != apply_at;
            *old = apply_at;
            changed
        });
        ends
    }

    /// The next pause expiry or retry, including changes while the VPN runs.
    pub fn apply_at(&self) -> watch::Receiver<Option<u64>> {
        self.apply_at.subscribe()
    }

    /// Whether all bundled HTTPS exclusion lists have usable copies.
    pub fn exclusions_ready(&self) -> bool {
        self.ctx.lists.exclusions.read().unwrap().ready()
    }

    /// Whether the user has the authority among the certificates the
    /// device trusts.
    pub fn ca_installed(&self) -> bool {
        self.platform.ca_installed()
    }

    /// Takes the connections of the VPN (`tunnel::run`).
    pub fn handler(self: &Arc<Self>) -> Handler {
        let engine = self.clone();
        Arc::new(move |socket: TcpStream, app, server| {
            // What was found out before the app was answered, if that was
            // done: whose the connection is, and a connection to its
            // server, for the engine to have when it connects.
            let made = engine.made().remove(&(app, server));
            let engine = engine.clone();
            tokio::spawn(async move {
                let (uid, number) = match made {
                    Some(made) => {
                        // Never 0, which is the number of a connection that
                        // has none made for it.
                        let number = engine.last.fetch_add(1, Relaxed).wrapping_add(1).max(1);
                        engine.ready.lock().unwrap().insert(number, made.server);
                        (made.uid, number)
                    }
                    None => (tcp_owner(&engine.platform, app, server).await, 0),
                };
                // No fwmark tells the connections apart here: its place holds
                // the number instead, which `dial` goes by.
                let origin = Origin::of(server, number, uid);
                let _ = mitm::handle(engine.ctx.clone(), socket, origin).await;
                // Not taken, where the engine did not connect: closed.
                engine.ready.lock().unwrap().remove(&number);
            });
        })
    }

    /// Finds out whether a server is there before its app is answered
    /// (`tunnel::run`), by connecting to it; the connection is kept for
    /// the engine. A connection that is nobody's or the system's is
    /// refused here, where the app is still connecting.
    ///
    /// What the app asks for is not known yet, the server it connects to
    /// only by its address: a server the lists block is connected to as
    /// well, and that connection closed with nothing sent on it.
    pub fn reach(self: &Arc<Self>) -> crate::tunnel::Reach {
        let engine = self.clone();
        Arc::new(move |app, server| {
            let engine = engine.clone();
            Box::pin(async move {
                let refused = || Err(std::io::ErrorKind::ConnectionRefused.into());
                let uid = tcp_owner(&engine.platform, app, server).await;
                if Origin::of(server, 0, uid).is_none() || engine.made().len() >= MAX_MADE {
                    return refused();
                }
                let connected = (engine.connect)(server).await?;
                // Others may have made theirs meanwhile.
                let mut made = engine.made();
                if made.len() >= MAX_MADE {
                    return refused();
                }
                made.insert(
                    (app, server),
                    Made {
                        server: connected,
                        uid,
                        at: Instant::now(),
                    },
                );
                Ok(())
            })
        })
    }

    /// The connections made, without those kept longer than `MADE_TIME`:
    /// one that old was left by an app that went away, and the same two
    /// ends may now be another connection's, or another app's.
    fn made(&self) -> std::sync::MutexGuard<'_, HashMap<(SocketAddr, SocketAddr), Made>> {
        let mut made = self.made.lock().unwrap();
        made.retain(|_, made| made.at.elapsed() < MADE_TIME);
        made
    }

    pub fn udp_policy(self: &Arc<Self>) -> crate::udp::Policy {
        let engine = self.clone();
        let lookup = Arc::new(move |app, server| {
            let uid = engine.platform.owner(libc::IPPROTO_UDP, app, server)?;
            let origin = Origin::of(server, 0, Some(uid))?;
            let state = engine.selected.read().unwrap();
            let epoch = state.owners.get(&uid)?.1;
            Some(crate::udp::Admission {
                uid: origin.uid(),
                epoch,
                generation: state.generation,
                filter: engine.ctx.selection.apps.read().unwrap().contains_key(&uid),
            })
        });
        let engine = self.clone();
        let current = Arc::new(move |admission: crate::udp::Admission| {
            let state = engine.selected.read().unwrap();
            state.generation == admission.generation
                && state
                    .owners
                    .get(&admission.uid)
                    .is_some_and(|(_, epoch)| *epoch == admission.epoch)
        });
        crate::udp::Policy { lookup, current }
    }

    pub(crate) fn dns(&self, upstream: Vec<SocketAddr>) -> Option<Arc<crate::dns::Resolver>> {
        crate::dns::Resolver::new(
            upstream,
            self.ctx.clone(),
            self.platform.clone(),
            self.connect.clone(),
            self.protect(),
        )
    }

    /// Keeps a socket of ours out of the VPN (`tunnel::run`).
    pub fn protect(&self) -> Protect {
        protector(self.platform.clone())
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub(crate) fn reload_pins(&self) -> std::io::Result<()> {
        pins::take_requests(&self.ctx.services.policy, &self.paths.forget())
    }

    /// What happens, as it happens, for whoever watches.
    pub fn live(&self) -> &Arc<live::Hub> {
        &self.ctx.services.live
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
