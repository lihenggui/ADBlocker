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

//! One intercepted connection, from the app's SYN to the end.
//!
//! The destination can decide a connection before it is read. Otherwise
//! the ClientHello is read without being consumed, and from the host name
//! in it the connection is refused, handed on untouched, or decrypted
//! (`policy`). To decrypt, the server is connected to first and verified
//! against the system's roots; only then is the app answered, with the
//! protocol the server chose. Until that moment the app's bytes are unread:
//! whatever is wrong with the server, the connection is handed on and the
//! app sees the failure itself. Nothing is signed for a server that was
//! not verified.

mod decrypt;
mod plaintext;
mod request;
mod upstream;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, RwLock};

use anyhow::{Context as _, Result};
use tokio::net::TcpStream;

use crate::allow::Allow;
use crate::apps::{PER_USER_RANGE, PERSONAL_APP_UIDS};
use crate::clienthello::{self, Hello, Peeked};
use crate::exclusions::Exclusions;
use crate::filter::{AddressDecision, AppFilters, Filters};
use crate::limits::{self, Limits};
use crate::live::Hub;
use crate::paths::Paths;
use crate::pause::Pauses;
use crate::policy::{Policy, Reason, Verdict};
use crate::settings::Settings;
use crate::state::{self, State};
use crate::stats::{Counter, Stats};
use crate::tls::Tls;

use decrypt::{Peer, mitm};
use plaintext::{HTTP_LOOK_TIME, plaintext_http, serve_plaintext};
use upstream::{reset, splice};

pub use request::{RequestDecision, RequestKind, evaluate_request};
pub use upstream::{Event, Ipv6Health};

/// Where a connection was going, and for whom. There is none but of an
/// app, and to somewhere else than this device: see [`Origin::of`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    dst: SocketAddr,
    mark: u32,
    uid: u32,
}

impl Origin {
    /// What a connection is, if it is one to carry. Whatever brings the
    /// connections here asks this of each, having seen for itself that
    /// the connection came the way it brings them.
    ///
    /// Only an app's connection is carried. One whose owner cannot be
    /// found, or is the system's, would be made again by us with nothing
    /// of the app's about it: outside its firewall rules, its VPN and its
    /// data count.
    ///
    /// And only one to somewhere else. A connection to this device itself
    /// may be one to where we listen: carried, we would connect to
    /// ourselves, and that connection to ourselves again, until there
    /// were no descriptors left.
    pub fn of(dst: SocketAddr, mark: u32, uid: Option<u32>) -> Option<Self> {
        let elsewhere = !dst.ip().is_loopback() && !dst.ip().is_unspecified();
        let uid = uid.filter(|uid| uid % PER_USER_RANGE >= *PERSONAL_APP_UIDS.start())?;
        elsewhere.then_some(Self { dst, mark, uid })
    }

    /// The server the app was connecting to.
    pub fn dst(&self) -> SocketAddr {
        self.dst
    }

    /// The fwmark of the app's packets, where connections are told apart
    /// by one: the network netd chose for the app. Nothing here reads it;
    /// it is for [`Hooks::dial`] to connect by.
    pub fn mark(&self) -> u32 {
        self.mark
    }

    /// The UID that owns the app's socket: an app's, always.
    pub fn uid(&self) -> u32 {
        self.uid
    }
}

/// Connects to the server of a connection as the app would have: by the
/// network, the firewall rules and the data count that are the app's.
pub type Dial = Box<
    dyn Fn(Origin) -> Pin<Box<dyn Future<Output = std::io::Result<TcpStream>> + Send>>
        + Send
        + Sync,
>;

/// Whether the processes of an app trust the authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trusting {
    /// Not, or not yet: its connections are not decrypted.
    No,
    Yes,
    /// Yes, but one of them only just: if the app refuses a certificate of
    /// ours now, that may be for roots it read before and has not read
    /// again, and says nothing of the host being pinned.
    Newly,
}

/// See [`Hooks::trusted`].
pub type Trusted = Box<dyn Fn(u32) -> Pin<Box<dyn Future<Output = Trusting> + Send>> + Send + Sync>;

pub struct Ctx {
    pub lists: Lists,
    pub selection: Selection,
    pub services: Services,
    pub hooks: Hooks,
    pub ipv6: Ipv6Health,
}

pub struct Lists {
    /// Replaced as a whole when the lists are read again.
    pub filters: RwLock<Arc<Filters>>,
    /// The hosts never decrypted. Replaced with the lists.
    pub exclusions: RwLock<Arc<Exclusions>>,
    /// The hosts the user trusts. Replaced with the lists.
    pub allow: RwLock<Arc<Allow>>,
}

impl Lists {
    pub fn open(filters: Filters, exclusions: Exclusions) -> Self {
        Self {
            filters: RwLock::new(Arc::new(filters)),
            exclusions: RwLock::new(Arc::new(exclusions)),
            allow: RwLock::default(),
        }
    }
}

#[derive(Default)]
pub struct Selection {
    /// The filtered apps, as the package each UID belongs to.
    pub apps: RwLock<Arc<BTreeMap<u32, String>>>,
    /// A managed profile gets its own system roots and TLS sessions.
    pub profile_tls: RwLock<BTreeMap<u32, Arc<Tls>>>,
    /// Whether request-level rules apply, including on cleartext HTTP.
    pub url_filtering: AtomicBool,
    /// Whether the backend can open HTTPS when URL filtering is enabled.
    pub https_inspection_available: AtomicBool,
    /// Whether what the lists name is hidden on decrypted pages (`cosmetic.rs`).
    pub cosmetic: AtomicBool,
}

impl Selection {
    fn profile_tls_is_current(&self, serial: u32, expected: &Arc<Tls>) -> bool {
        self.profile_tls
            .read()
            .unwrap()
            .get(&serial)
            .is_some_and(|current| Arc::ptr_eq(current, expected))
    }
}

pub struct Services {
    pub tls: Tls,
    pub policy: Arc<Policy>,
    pub stats: Arc<Stats>,
    /// Told what happens, for `abd watch`, while anyone watches.
    pub live: Arc<Hub>,
    pub limits: Arc<Limits>,
}

pub struct Records {
    pub stats: Arc<Stats>,
    pub policy: Arc<Policy>,
    pub live: Arc<Hub>,
}

impl Records {
    pub fn open(paths: &Paths) -> Self {
        Self {
            stats: Arc::new(Stats::open(&paths.stats())),
            policy: Arc::new(Policy::open(paths.pinned())),
            live: Hub::new(),
        }
    }
}

pub enum InitialTrust {
    Available,
    AwaitingCertificate,
}

pub enum SelectionUpdate {
    Root(BTreeMap<u32, Arc<Tls>>),
    Vpn,
}

pub enum SnapshotSelection {
    Root,
    Vpn(usize),
}

pub struct Hooks {
    /// Log each connection and request. For development only: the log then
    /// holds what the user's apps talk to.
    pub verbose: bool,
    pub dial: Dial,
    /// Whether every process of an app trusts the authority, having seen
    /// to it that they do where that can be seen to.
    pub trusted: Trusted,
}

macro_rules! trace {
    ($ctx:expr, $($arg:tt)*) => {
        if $ctx.hooks.verbose {
            eprintln!($($arg)*);
        }
    };
}
use trace;

impl Ctx {
    pub fn open(
        lists: Lists,
        tls: Tls,
        records: Records,
        hooks: Hooks,
        trust: InitialTrust,
        ipv6: Ipv6Health,
    ) -> Self {
        let Records {
            stats,
            policy,
            live,
        } = records;
        Self {
            lists,
            selection: Selection {
                https_inspection_available: matches!(trust, InitialTrust::Available).into(),
                ..Selection::default()
            },
            services: Services {
                tls,
                policy,
                stats,
                live,
                limits: Limits::new(limits::raise_descriptors()),
            },
            hooks,
            ipv6,
        }
    }

    pub fn snapshot(&self, ipv6: bool, selection: SnapshotSelection) -> State {
        let filters = self.lists.filters.read().unwrap().clone();
        let exclusions = self.lists.exclusions.read().unwrap().clone();
        let pid = std::process::id();
        let started = state::started();
        let (profile_tls, filtering_apps) = match selection {
            SnapshotSelection::Root => (
                self.selection
                    .profile_tls
                    .read()
                    .unwrap()
                    .keys()
                    .copied()
                    .collect(),
                self.selection.apps.read().unwrap().len(),
            ),
            SnapshotSelection::Vpn(filtering_apps) => (BTreeSet::new(), filtering_apps),
        };
        State {
            pid,
            started,
            rules: filters.rules,
            host_rules: filters.host_rules,
            exclusions: exclusions.len(),
            exclusions_ready: exclusions.ready(),
            profile_tls,
            filtering_apps: Some(filtering_apps),
            ipv6,
        }
    }

    pub fn keep_written(&self, paths: &Paths) {
        self.services.stats.clone().keep_written(paths.stats());
        self.services.policy.clone().keep_written();
    }

    /// Reload sources that can be published without compiling filter rules.
    pub fn reload_sources(
        &self,
        paths: &Paths,
        forget: impl FnOnce(&Policy, &std::path::Path) -> std::io::Result<()>,
    ) {
        let current = self.lists.exclusions.read().unwrap().clone();
        match current.changed(&paths.exclusions()) {
            Ok(Some(new)) => *self.lists.exclusions.write().unwrap() = Arc::new(new),
            Ok(None) => {}
            Err(e) => eprintln!("exclusions: kept as they were: {e}"),
        }
        if let Err(error) = forget(&self.services.policy, &paths.forget()) {
            eprintln!("pins: pending removals could not be applied: {error}");
        }
    }

    /// Before filtering any app, verify the choices that can require its
    /// traffic to pass unread. Keep the last trust list for in-flight work
    /// when a reload fails and let the caller suspend new filtering.
    pub fn reload_user_policy(&self, paths: &Paths, now: u64) -> Result<(Settings, Pauses)> {
        let settings =
            Settings::try_read(&paths.settings(), Settings::UNSAID).context("reading settings")?;
        let pauses = Pauses::load(&paths.pauses(), now).context("reading pauses")?;
        let allow = Allow::try_load(&paths.allow()).context("reading trusted sites")?;
        *self.lists.allow.write().unwrap() = Arc::new(allow);
        Ok((settings, pauses))
    }

    pub fn publish_selection(
        &self,
        settings: Settings,
        chosen: Option<&std::collections::BTreeSet<&str>>,
        active: BTreeMap<u32, String>,
        update: SelectionUpdate,
    ) -> usize {
        if let Some(chosen) = chosen {
            self.services.policy.keep_only(chosen);
        }
        let selection = &self.selection;
        let url_filtering = || {
            selection
                .url_filtering
                .store(settings.url_filtering, Relaxed)
        };
        let cosmetic = || selection.cosmetic.store(settings.cosmetic, Relaxed);
        // Preserve the publication order of each host. Root replaces the
        // profile verifiers before publishing the active map.
        match update {
            SelectionUpdate::Root(profiles) => {
                url_filtering();
                cosmetic();
                *selection.profile_tls.write().unwrap() = profiles;
            }
            SelectionUpdate::Vpn => {
                cosmetic();
                url_filtering();
            }
        }
        let active_count = active.len();
        *selection.apps.write().unwrap() = Arc::new(active);
        active_count
    }

    fn can_inspect_https(&self, url_filtering: bool) -> bool {
        url_filtering && self.selection.https_inspection_available.load(Relaxed)
    }

    fn filters(&self) -> Arc<Filters> {
        self.lists.filters.read().unwrap().clone()
    }

    fn app_filters<'a>(&self, filters: &'a Filters, package: &'a str) -> AppFilters<'a> {
        filters.for_app_with_pages(package, self.services.policy.has_shown_page(package))
    }

    fn package(&self, uid: u32) -> Option<String> {
        self.selection.apps.read().unwrap().get(&uid).cloned()
    }

    fn is_package(&self, uid: u32, expected: &str) -> bool {
        self.selection
            .apps
            .read()
            .unwrap()
            .get(&uid)
            .is_some_and(|package| package == expected)
    }

    pub fn has_address_rules(&self) -> bool {
        self.lists.filters.read().unwrap().has_address_rules()
    }

    fn address_decision_for(&self, package: &str, dst: SocketAddr) -> AddressDecision {
        let filters = self.filters();
        if filters.has_address_rules() {
            filters.for_app(package).address_decision(dst)
        } else {
            AddressDecision::NoMatch
        }
    }

    /// Evaluate the endpoint of an attributed, active app.
    pub fn address_decision(&self, origin: Origin) -> AddressDecision {
        self.package(origin.uid)
            .map_or(AddressDecision::NoMatch, |package| {
                self.address_decision_for(&package, origin.dst())
            })
    }

    /// The VPN normally dials before accepting TCP. Refuse a known block
    /// before that dial, since `conduct` will never see this connection.
    pub fn refuse_blocked_address(&self, origin: Origin) -> bool {
        let Some(package) = self.package(origin.uid) else {
            return false;
        };
        if self.address_decision_for(&package, origin.dst()) != AddressDecision::Block {
            return false;
        }
        self.address_blocked(&package, origin.dst());
        true
    }

    fn address_blocked(&self, package: &str, dst: SocketAddr) {
        self.services
            .stats
            .count(Some(package), Counter::HostsBlocked);
        self.services.live.host_blocked(package, dst);
    }

    /// Evaluate a visible name only after the ingress has admitted the app.
    pub fn blocks_udp_host(&self, uid: u32, host: &str) -> bool {
        let Some(host) = crate::host::canonical_authority(host, "https", 443)
            .filter(|name| !name.explicit_port)
            .map(|name| name.host)
        else {
            return false;
        };
        let Some(package) = self.package(uid) else {
            return false;
        };
        let exclusions = self.lists.exclusions.read().unwrap().clone();
        let allow = self.lists.allow.read().unwrap().clone();
        let blocked = self.services.policy.decide_named(
            &package,
            &host,
            self.app_filters(&self.filters(), &package),
            &exclusions,
            &allow,
            !self.selection.url_filtering.load(Relaxed),
        ) == Verdict::Block;
        if blocked {
            self.services
                .stats
                .count(Some(&package), Counter::HostsBlocked);
            self.services.live.host_blocked(&package, &host);
        }
        blocked
    }

    /// A connection of `package` to `host` handed on unread.
    fn passed(&self, package: &str, host: &str, reason: Reason) {
        self.services.stats.count(Some(package), Counter::Passed);
        self.services.live.passed(package, host, reason);
    }

    /// What is done with a connection that was to be decrypted and stays
    /// encrypted after all, for `reason`: it has no requests to judge, so
    /// its host is. Returns whether it is handed on; one that is not has
    /// been reset.
    ///
    /// Where the app does not trust us or the server cannot be verified,
    /// an exception that names the host leaves the host to the app. Any
    /// other connection is judged by the domain rules, as
    /// `Policy::decide_named` judges one known beforehand to stay
    /// encrypted for the same reason.
    fn hands_on(&self, app: &TcpStream, peer: (u32, &str, &str), reason: Reason) -> bool {
        let (uid, package, host) = peer;
        let filters = self.filters();
        let filters = self.app_filters(&filters, package);
        // The user may have trusted the host, or taken the app out, while
        // the server was asked: neither is blocked any more.
        let judged =
            self.is_package(uid, package) && !self.lists.allow.read().unwrap().covers(host);
        let blocked = judged
            && match reason {
                Reason::Untrusted | Reason::Unverified => filters.blocks_unread(host),
                _ => filters.blocks_domain(host),
            };
        if blocked {
            self.services
                .stats
                .count(Some(package), Counter::HostsBlocked);
            self.services.live.host_blocked(package, host);
            reset(app);
            return false;
        }
        self.passed(package, host, reason);
        true
    }
}

/// Ends a connection that is not carried, with a reset: the app fails at
/// once instead of waiting.
fn refuse(ctx: &Ctx, app: &TcpStream) {
    ctx.services.stats.count(None, Counter::Refused);
    reset(app);
}

/// Takes an accepted connection, with what it is if it is one to carry
/// ([`Origin::of`]). Any other is reset, and so is one more than its app
/// may have.
pub async fn handle(ctx: Arc<Ctx>, app: TcpStream, origin: Option<Origin>) -> Result<()> {
    let Some(origin) = origin else {
        refuse(&ctx, &app);
        return Ok(());
    };
    // Held until the connection ends.
    let Some(_slot) = ctx.services.limits.take(origin.uid) else {
        refuse(&ctx, &app);
        return Ok(());
    };
    conduct(ctx, app, origin).await
}

pub async fn conduct(ctx: Arc<Ctx>, app: TcpStream, origin: Origin) -> Result<()> {
    // An app, but not one of the filtered: its uid left the list while
    // this connection was on its way. Not ours to read.
    let Some(package) = ctx.package(origin.uid) else {
        ctx.services.stats.count(None, Counter::Passed);
        return splice(&ctx, app, origin).await;
    };
    match ctx.address_decision_for(&package, origin.dst()) {
        AddressDecision::Block => {
            ctx.address_blocked(&package, origin.dst());
            reset(&app);
            return Ok(());
        }
        AddressDecision::Pass => {
            ctx.services.stats.count(Some(&package), Counter::Passed);
            ctx.services.live.address_passed(&package, origin.dst());
            return splice(&ctx, app, origin).await;
        }
        AddressDecision::NoMatch => {}
    }
    let url_filtering = ctx.selection.url_filtering.load(Relaxed);
    // Port 80 can also carry TLS or another protocol. Classify the
    // connection without consuming it, and leave opaque streams alone.
    let first = if origin.dst.port() == 80 {
        let deadline = tokio::time::Instant::now() + HTTP_LOOK_TIME;
        let peeked = clienthello::peek(&app).await?;
        if matches!(peeked, Peeked::NotTls) && plaintext_http(&app, deadline).await? {
            return serve_plaintext(ctx, app, origin, package).await;
        }
        Some(peeked)
    } else {
        None
    };
    let exclusions_ready = ctx.lists.exclusions.read().unwrap().ready();
    // Before anything is sent on, in either direction: the app's processes
    // are made to trust the authority. An app reads the roots it trusts
    // once, at the first certificate it is shown, and that may be one of
    // a connection that is handed on. An app that cannot be made to trust
    // us is not decrypted: it would refuse us, and every host it asked for
    // be taken for pinned. An app whose hosts only are judged is never
    // decrypted, and so never asked.
    let opaque = first.as_ref().is_some_and(|p| !matches!(p, Peeked::Tls(_)));
    let trusting = if !ctx.can_inspect_https(url_filtering) || !exclusions_ready || opaque {
        Trusting::No
    } else {
        (ctx.hooks.trusted)(origin.uid).await
    };
    let peeked = match first {
        Some(peeked) => peeked,
        None => clienthello::peek(&app).await?,
    };
    let hello = match peeked {
        Peeked::Tls(h) => h,
        Peeked::NotTls | Peeked::Silent => {
            trace!(ctx, "pass    {package} {} (not TLS)", origin.dst);
            ctx.services.stats.count(Some(&package), Counter::Passed);
            return splice(&ctx, app, origin).await;
        }
    };
    inspect_tls(ctx, app, origin, package, trusting, hello, url_filtering).await
}

async fn inspect_tls(
    ctx: Arc<Ctx>,
    app: TcpStream,
    origin: Origin,
    package: String,
    trusting: Trusting,
    hello: Hello,
    url_filtering: bool,
) -> Result<()> {
    let host = hello
        .sni
        .clone()
        .unwrap_or_else(|| origin.dst.ip().to_string());
    let exclusions = ctx.lists.exclusions.read().unwrap().clone();
    let allow = ctx.lists.allow.read().unwrap().clone();
    let filters = ctx.filters();
    match ctx.services.policy.decide(
        &package,
        &hello,
        ctx.app_filters(&filters, &package),
        &exclusions,
        &allow,
        !url_filtering,
    ) {
        Verdict::Block => {
            trace!(ctx, "block   {package} {host} (host rule)");
            ctx.services
                .stats
                .count(Some(&package), Counter::HostsBlocked);
            ctx.services.live.host_blocked(&package, &host);
            reset(&app);
            Ok(())
        }
        // A pinned host is decrypted only as its one try, given at its app's
        // start, and only by a connection that can be.
        Verdict::Passthrough(Reason::Pinned) if trusting != Trusting::No => {
            let Some(ticket) = ctx.services.policy.take_try(&package, &host) else {
                trace!(ctx, "pass    {package} {host} (Pinned)");
                ctx.passed(&package, &host, Reason::Pinned);
                return splice(&ctx, app, origin).await;
            };
            trace!(ctx, "retry   {package} {host}");
            let peer = Peer {
                origin,
                package: package.clone(),
                host: host.clone(),
                retry: Some(ticket),
            };
            let result = mitm(ctx.clone(), app, peer, hello, trusting).await;
            ctx.services.policy.try_over(&package, &host, ticket);
            result
        }
        Verdict::Passthrough(reason) => {
            trace!(ctx, "pass    {package} {host} ({reason:?})");
            ctx.passed(&package, &host, reason);
            splice(&ctx, app, origin).await
        }
        Verdict::Mitm if trusting == Trusting::No => {
            trace!(ctx, "pass    {package} {host} (not made to trust us)");
            if !ctx.hands_on(&app, (origin.uid, &package, &host), Reason::Untrusted) {
                return Ok(());
            }
            splice(&ctx, app, origin).await
        }
        Verdict::Mitm => {
            let peer = Peer {
                origin,
                package,
                host,
                retry: None,
            };
            mitm(ctx, app, peer, hello, trusting).await
        }
    }
}

#[cfg(test)]
mod tests;
