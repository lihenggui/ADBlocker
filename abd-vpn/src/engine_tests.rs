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

//! The engine behind the VPN, from an app's packets to a server and back.

use std::os::fd::RawFd;
use std::sync::Mutex;

use smoltcp::socket::tcp;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

use super::*;
use crate::fake::{App, tunnel, tunnel_reaching};
use crate::platform::CALLBACK_TIME;

const SERVER: &str = "93.184.216.34:443";
const UID: u32 = 10001;

struct SlowCallbacks {
    owner_started: tokio::sync::Notify,
    protect_delay: Duration,
}

impl Platform for SlowCallbacks {
    fn owner(&self, _: i32, app: SocketAddr, _: SocketAddr) -> Option<u32> {
        if app.port() == 40001 {
            self.owner_started.notify_one();
            std::thread::sleep(Duration::from_millis(500));
        }
        Some(UID)
    }

    fn uid_of(&self, _: &str) -> Option<u32> {
        Some(UID)
    }

    fn ca_installed(&self) -> bool {
        false
    }

    fn protect(&self, _: RawFd) -> bool {
        std::thread::sleep(self.protect_delay);
        true
    }
}

async fn while_an_owner_is_slow() -> (Duration, std::io::Result<()>) {
    let platform = Arc::new(SlowCallbacks {
        owner_started: tokio::sync::Notify::new(),
        protect_delay: Duration::ZERO,
    });
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dest = server.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open_with(
        dir.path(),
        rustls::RootCertStore::empty(),
        platform.clone(),
        Arc::new(move |_| Box::pin(TcpStream::connect(dest))),
    )
    .unwrap();
    let dest = SERVER.parse().unwrap();
    let reach = engine.reach();
    let started = Instant::now();
    let slow = tokio::spawn(reach("10.0.0.2:40001".parse().unwrap(), dest));
    tokio::time::timeout(Duration::from_secs(2), platform.owner_started.notified())
        .await
        .unwrap();
    reach("10.0.0.2:40002".parse().unwrap(), dest)
        .await
        .unwrap();
    let elapsed = started.elapsed();
    (elapsed, slow.await.unwrap())
}

#[tokio::test]
#[ignore = "a measurement, run by hand"]
async fn measures_tcp_reach_during_a_slow_owner_lookup() {
    let (elapsed, result) = while_an_owner_is_slow().await;
    println!("unrelated TCP reach: {elapsed:?}, slow result: {result:?}");
}

#[tokio::test]
async fn a_slow_owner_lookup_leaves_other_tcp_connections_running_and_is_refused() {
    let (elapsed, result) = while_an_owner_is_slow().await;
    assert!(elapsed < CALLBACK_TIME, "unrelated reach took {elapsed:?}");
    assert_eq!(
        result.unwrap_err().kind(),
        std::io::ErrorKind::ConnectionRefused,
    );
}

#[tokio::test]
async fn a_protect_timeout_does_not_connect_even_if_binder_later_succeeds() {
    let platform = Arc::new(SlowCallbacks {
        owner_started: tokio::sync::Notify::new(),
        protect_delay: Duration::from_millis(500),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let result = connect(protector(platform))(listener.local_addr().unwrap()).await;
    assert_eq!(
        result.unwrap_err().to_string(),
        "the socket could not be kept out of the VPN"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(350), listener.accept())
            .await
            .is_err()
    );
}

/// The app's answers, as a test has them.
struct Device {
    /// Whose every connection is said to be.
    owner: Option<u32>,
    ca_installed: bool,
}

impl Platform for Device {
    fn owner(&self, _: i32, _: SocketAddr, _: SocketAddr) -> Option<u32> {
        self.owner
    }
    fn uid_of(&self, package: &str) -> Option<u32> {
        (package == "com.app").then_some(UID)
    }
    fn personal_app_identities(&self, exclusions: &BTreeSet<&str>) -> BTreeMap<u32, String> {
        [(UID, "com.app"), (UID + 1, "@com.group")]
            .into_iter()
            .filter(|(_, name)| !exclusions.contains(name))
            .map(|(uid, name)| (uid, name.to_owned()))
            .collect()
    }
    fn ca_installed(&self) -> bool {
        self.ca_installed
    }
    fn protect(&self, _: RawFd) -> bool {
        true
    }
}

/// An engine with the app `com.app` chosen and one host blocked, a server
/// that every connection handed on reaches, and an app before the VPN.
struct Rig {
    app: App,
    engine: Arc<Engine>,
    /// What the server was sent, a connection each.
    received: Arc<Mutex<Vec<Vec<u8>>>>,
    /// How often a server was connected to.
    connects: Arc<std::sync::atomic::AtomicUsize>,
    _dir: tempfile::TempDir,
}

async fn rig(device: Device) -> Rig {
    rig_with(device, Way::Answered).await
}

/// How an app's connection is taken.
#[derive(Clone, Copy, PartialEq)]
enum Way {
    /// Answered at once, and the server connected to after.
    Answered,
    /// The server connected to first, and it is there.
    Reached,
    /// The server connected to first, and it refuses.
    Refused,
}

async fn rig_with(device: Device, way: Way) -> Rig {
    rig_with_config(device, way, |_| {}).await
}

async fn rig_with_config(device: Device, way: Way, configure: impl FnOnce(&Paths)) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.rules()).unwrap();
    std::fs::write(paths.rules().join("list.txt"), "||blocked.test^\n").unwrap();
    std::fs::write(paths.apps(), "com.app\n").unwrap();
    configure(&paths);

    // Answers what it is sent with "pong", and keeps what that was.
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = server.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let kept = received.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = server.accept().await.unwrap();
            let kept = kept.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                kept.lock().unwrap().push(buf[..n].to_vec());
                let _ = socket.write_all(b"pong").await;
            });
        }
    });
    let connects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = connects.clone();
    let connect: Connect = Arc::new(move |_| {
        counted.fetch_add(1, Relaxed);
        Box::pin(async move {
            if way == Way::Refused {
                return Err(std::io::ErrorKind::ConnectionRefused.into());
            }
            TcpStream::connect(at).await
        })
    });
    let roots = rustls::RootCertStore::empty();
    let engine = Engine::open_with(dir.path(), roots, Arc::new(device), connect).unwrap();
    Rig {
        app: match way {
            Way::Answered => tunnel(engine.handler()),
            _ => tunnel_reaching(engine.handler(), Some(engine.reach())),
        },
        connects,
        engine,
        received,
        _dir: dir,
    }
}

/// What a client sends first to the server named `host`.
fn hello(host: &str) -> Vec<u8> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_owned()).unwrap();
    let mut client = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
    let mut bytes = Vec::new();
    client.write_tls(&mut bytes).unwrap();
    bytes
}

const CHOSEN: Device = Device {
    owner: Some(UID),
    ca_installed: false,
};

/// Connects, sends `hello`, and waits for the server's four bytes.
async fn answered(rig: &mut Rig, hello: &[u8]) -> Vec<u8> {
    let handle = rig.app.connect(SERVER);
    rig.app.until(|app| app.socket(handle).may_send()).await;
    rig.app.socket(handle).send_slice(hello).unwrap();
    let mut got = Vec::new();
    rig.app
        .until(|app| {
            got.extend(app.read(handle));
            got.len() == 4
        })
        .await;
    got
}

#[tokio::test]
async fn a_connection_to_a_host_the_lists_block_is_reset() {
    let mut rig = rig(CHOSEN).await;
    let handle = rig.app.connect(SERVER);
    rig.app.until(|app| app.socket(handle).may_send()).await;
    rig.app
        .socket(handle)
        .send_slice(&hello("blocked.test"))
        .unwrap();
    rig.app
        .until(|app| app.socket(handle).state() == tcp::State::Closed)
        .await;
    // Never reached a server.
    assert!(rig.received.lock().unwrap().is_empty());
    assert!(rig.app.read(handle).is_empty());
}

#[tokio::test]
async fn any_other_connection_is_handed_on_as_it_came_and_never_decrypted() {
    let mut rig = rig(CHOSEN).await;
    let sent = hello("allowed.test");
    // The server's own answer, to the bytes the app sent.
    assert_eq!(answered(&mut rig, &sent).await, b"pong");
    assert_eq!(*rig.received.lock().unwrap(), [sent]);
}

#[tokio::test]
async fn an_app_is_not_decrypted_for_asking_while_the_authority_is_not_installed() {
    let mut rig = rig(CHOSEN).await;
    Settings::set(&rig.engine.paths().settings(), "url-filtering", "on").unwrap();
    rig.engine.reload();
    let sent = hello("allowed.test");
    assert_eq!(answered(&mut rig, &sent).await, b"pong");
    assert_eq!(*rig.received.lock().unwrap(), [sent]);
}

#[tokio::test]
async fn a_connection_that_is_nobody_s_or_the_system_s_is_reset() {
    for owner in [None, Some(1000)] {
        let mut rig = rig(Device {
            owner,
            ca_installed: false,
        })
        .await;
        let handle = rig.app.connect(SERVER);
        rig.app
            .until(|app| app.socket(handle).state() == tcp::State::Closed)
            .await;
        assert!(rig.received.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn a_paused_app_has_its_connections_handed_on_unread() {
    let mut rig = rig(CHOSEN).await;
    let until = clock::now() + 600;
    pause::Pauses::change(&rig.engine.paths().pauses(), clock::now(), |p| {
        p.pause_app("com.app", until);
    })
    .unwrap();
    rig.engine.apply();
    assert_eq!(*rig.engine.apply_at().borrow(), Some(until));
    // Blocked while the app is filtered; not looked at now.
    assert_eq!(answered(&mut rig, &hello("blocked.test")).await, b"pong");
    assert_eq!(rig.received.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn all_apps_filters_nothing_until_unreadable_exclusions_can_be_read_again() {
    let mut rig = rig(CHOSEN).await;
    let engine = rig.engine.clone();
    let paths = engine.paths();
    Settings::set(&paths.settings(), "scope", "all").unwrap();
    let saved = "!com.app\n";
    std::fs::write(paths.apps(), saved).unwrap();
    engine.reload();
    let group = BTreeMap::from([(UID + 1, Arc::<str>::from("@com.group"))]);
    assert_eq!(**engine.ctx.selection.apps.read().unwrap(), group);

    std::fs::write(paths.apps(), [0xff]).unwrap();
    engine.reload();
    assert!(engine.ctx.selection.apps.read().unwrap().is_empty());
    assert!(engine.selected.read().unwrap().owners.is_empty());
    assert!(engine.apply_at().borrow().is_some());
    assert_eq!(answered(&mut rig, &hello("blocked.test")).await, b"pong");

    std::fs::write(paths.apps(), saved).unwrap();
    engine.apply();
    assert_eq!(**engine.ctx.selection.apps.read().unwrap(), group);
    assert_eq!(*engine.apply_at().borrow(), None);

    std::fs::write(paths.apps(), "").unwrap();
    engine.reload();
    assert_eq!(engine.ctx.selection.apps.read().unwrap().len(), 2);
}

#[tokio::test]
async fn unreadable_user_policy_leaves_cold_start_unfiltered() {
    for file in ["settings", "pauses", "allow"] {
        let mut rig = rig_with_config(CHOSEN, Way::Answered, |paths| {
            std::fs::write(paths.dir().join(file), [0xff]).unwrap();
        })
        .await;
        assert!(
            rig.engine.ctx.selection.apps.read().unwrap().is_empty(),
            "{file}"
        );
        assert!(rig.engine.apply_at().borrow().is_some(), "{file}");
        assert_eq!(answered(&mut rig, &hello("blocked.test")).await, b"pong");
    }
}

#[tokio::test]
async fn unreadable_user_policy_suspends_filtering_until_reloaded() {
    let mut rig = rig(CHOSEN).await;
    let engine = rig.engine.clone();
    let paths = engine.paths();
    let allow = paths.allow();
    std::fs::write(&allow, "blocked.test\n").unwrap();
    engine.reload();
    assert_eq!(engine.ctx.selection.apps.read().unwrap().len(), 1);
    assert!(!engine.ctx.blocks_udp_host(UID, "blocked.test"));

    std::fs::write(&allow, [0xff]).unwrap();
    engine.reload();
    assert!(engine.ctx.selection.apps.read().unwrap().is_empty());
    assert_eq!(answered(&mut rig, &hello("blocked.test")).await, b"pong");

    std::fs::write(&allow, "blocked.test\n").unwrap();
    engine.apply();
    assert_eq!(engine.ctx.selection.apps.read().unwrap().len(), 1);
    assert!(!engine.ctx.blocks_udp_host(UID, "blocked.test"));

    std::fs::write(&allow, "").unwrap();
    let pauses = paths.pauses();
    let until = clock::now() + 600;
    std::fs::write(&pauses, format!("{until} com.app\n")).unwrap();
    engine.apply();
    assert_eq!(*engine.apply_at().borrow(), Some(until));
    assert!(engine.ctx.selection.apps.read().unwrap().is_empty());
    std::fs::write(&pauses, [0xff]).unwrap();
    engine.reload();
    assert!(engine.ctx.selection.apps.read().unwrap().is_empty());
    assert_eq!(answered(&mut rig, &hello("blocked.test")).await, b"pong");

    std::fs::write(&pauses, format!("{until} com.app\n")).unwrap();
    engine.apply();
    assert_eq!(*engine.apply_at().borrow(), Some(until));
    assert!(engine.ctx.selection.apps.read().unwrap().is_empty());
    std::fs::write(&pauses, "").unwrap();
    engine.apply();
    assert!(engine.ctx.blocks_udp_host(UID, "blocked.test"));
    assert_eq!(*engine.apply_at().borrow(), None);
}

#[tokio::test]
async fn the_connection_made_to_find_the_server_there_is_the_one_handed_on() {
    let mut rig = rig_with(CHOSEN, Way::Reached).await;
    let sent = hello("allowed.test");
    assert_eq!(answered(&mut rig, &sent).await, b"pong");
    assert_eq!(*rig.received.lock().unwrap(), [sent]);
    // Connected to once: not once to find out and once more to carry.
    assert_eq!(rig.connects.load(Relaxed), 1);
    assert!(rig.engine.made.lock().unwrap().is_empty());
    assert!(rig.engine.ready.lock().unwrap().is_empty());
}

#[tokio::test]
async fn no_server_is_connected_to_for_more_connections_than_are_kept() {
    let rig = rig_with(CHOSEN, Way::Reached).await;
    let reach = rig.engine.reach();
    let server: SocketAddr = SERVER.parse().unwrap();
    let app = |port| SocketAddr::from(([10, 0, 0, 2], port));
    for port in 0..MAX_MADE as u16 {
        reach(app(port), server).await.unwrap();
    }
    let over = reach(app(u16::MAX), server).await.unwrap_err();
    assert_eq!(over.kind(), std::io::ErrorKind::ConnectionRefused);
    assert_eq!(rig.connects.load(Relaxed), MAX_MADE);
    // Those left too long make room again.
    for made in rig.engine.made.lock().unwrap().values_mut() {
        made.at -= MADE_TIME;
    }
    reach(app(u16::MAX), server).await.unwrap();
    assert_eq!(rig.engine.made.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_connection_made_too_long_before_its_app_was_answered_is_not_handed_on() {
    let mut rig = rig(CHOSEN).await;
    let stale = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let made = Made {
        server: TcpStream::connect(stale.local_addr().unwrap())
            .await
            .unwrap(),
        uid: Some(UID),
        at: Instant::now() - MADE_TIME,
    };
    // The address the app's next connection has.
    let ends = (
        SocketAddr::from(([10, 0, 0, 2], 40001)),
        SERVER.parse().unwrap(),
    );
    rig.engine.made.lock().unwrap().insert(ends, made);
    let sent = hello("allowed.test");
    assert_eq!(answered(&mut rig, &sent).await, b"pong");
    assert_eq!(*rig.received.lock().unwrap(), [sent]);
    assert_eq!(rig.connects.load(Relaxed), 1);
}

#[tokio::test]
async fn a_server_that_is_not_there_is_not_there_for_the_app_either() {
    let mut rig = rig_with(CHOSEN, Way::Refused).await;
    let handle = rig.app.connect(SERVER);
    let mut opened = false;
    rig.app
        .until(|app| {
            // Never made, to be broken after: refused while connecting,
            // as the server itself would have.
            opened |= app.socket(handle).may_send();
            app.socket(handle).state() == tcp::State::Closed
        })
        .await;
    assert!(!opened);
}

#[tokio::test]
async fn a_blocked_host_s_connection_is_closed_with_nothing_sent_on_it() {
    let mut rig = rig_with(CHOSEN, Way::Reached).await;
    let handle = rig.app.connect(SERVER);
    rig.app.until(|app| app.socket(handle).may_send()).await;
    rig.app
        .socket(handle)
        .send_slice(&hello("blocked.test"))
        .unwrap();
    rig.app
        .until(|app| app.socket(handle).state() == tcp::State::Closed)
        .await;
    // The server was connected to, the host not being known until the
    // app says it; it was sent nothing, and nothing is kept of it.
    rig.app
        .until(|_| rig.received.lock().unwrap().len() == 1)
        .await;
    assert_eq!(*rig.received.lock().unwrap(), [Vec::<u8>::new()]);
    assert!(rig.engine.ready.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_blocked_address_is_refused_before_the_vpn_connects_to_it() {
    let mut rig = rig_with_config(CHOSEN, Way::Reached, |paths| {
        std::fs::write(paths.rules().join("list.txt"), format!("{SERVER}$network")).unwrap();
    })
    .await;
    let handle = rig.app.connect(SERVER);
    let mut opened = false;
    rig.app
        .until(|app| {
            opened |= app.socket(handle).may_send();
            app.socket(handle).state() == tcp::State::Closed
        })
        .await;
    assert!(!opened);
    assert_eq!(rig.connects.load(Relaxed), 0);
}

#[tokio::test]
async fn udp_address_rules_drop_only_the_named_port_and_exceptions_bypass_quic_refusal() {
    let rig = rig_with_config(CHOSEN, Way::Reached, |paths| {
        std::fs::write(
            paths.rules().join("list.txt"),
            "192.0.2.1:3478^$network,app=com.app\n@@192.0.2.1:443$network\n",
        )
        .unwrap();
    })
    .await;
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = attempts.clone();
    let protect: Protect = Arc::new(move |_| {
        counted.fetch_add(1, Relaxed);
        false
    });
    let (back, _) = tokio::sync::mpsc::channel(1);
    let mut forwarder = crate::udp::Forwarder::new(back, protect, rig.engine.udp_policy(), 1500);
    let datagram = |port, payload| crate::datagram::Datagram {
        app: "10.0.0.2:60000".parse().unwrap(),
        server: ([192, 0, 2, 1], port).into(),
        payload,
    };
    let blocked = datagram(3478, b"STUN");
    assert!(forwarder.send(&blocked).await);
    assert_eq!(attempts.load(Relaxed), 0);
    let initial = include_bytes!("../../testdata/quic/aioquic-v2-initial.bin");
    assert!(!forwarder.send(&datagram(443, initial)).await);
    assert_eq!(attempts.load(Relaxed), 1);
    for _ in 0..40 {
        forwarder.send(&blocked).await;
    }
    assert_eq!(attempts.load(Relaxed), 1);
    assert!(!forwarder.send(&datagram(3479, b"STUN")).await);
    assert_eq!(attempts.load(Relaxed), 2);

    let paths = rig.engine.paths();
    let until = clock::now() + 600;
    std::fs::write(paths.pauses(), format!("{until} com.app\n")).unwrap();
    rig.engine.apply();
    assert!(!forwarder.send(&blocked).await);
    assert_eq!(attempts.load(Relaxed), 3);
}

#[tokio::test]
async fn a_connection_that_is_nobody_s_is_refused_before_any_server_is_connected_to() {
    let device = Device {
        owner: None,
        ca_installed: false,
    };
    let mut rig = rig_with(device, Way::Reached).await;
    let handle = rig.app.connect(SERVER);
    let mut opened = false;
    rig.app
        .until(|app| {
            opened |= app.socket(handle).may_send();
            app.socket(handle).state() == tcp::State::Closed
        })
        .await;
    assert!(!opened);
    assert_eq!(rig.connects.load(Relaxed), 0);
}

#[tokio::test]
async fn settings_reload_reuses_only_lists_that_are_still_current() {
    let rig = rig(CHOSEN).await;
    let engine = &rig.engine;
    let filters = engine.ctx.lists.filters.read().unwrap().clone();
    let exclusions = engine.ctx.lists.exclusions.read().unwrap().clone();
    engine.reload();
    assert!(Arc::ptr_eq(
        &filters,
        &engine.ctx.lists.filters.read().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &exclusions,
        &engine.ctx.lists.exclusions.read().unwrap()
    ));

    std::fs::create_dir_all(engine.paths.exclusions()).unwrap();
    std::fs::write(engine.paths.exclusions().join("banks.txt"), "bank.example").unwrap();
    engine.reload();
    assert!(Arc::ptr_eq(
        &filters,
        &engine.ctx.lists.filters.read().unwrap()
    ));
    let updated = engine.ctx.lists.exclusions.read().unwrap().clone();
    assert!(updated.covers("com.app", "bank.example"));

    std::fs::write(engine.paths.user_rules(), "||changed.example^").unwrap();
    engine.reload();
    assert!(
        engine
            .ctx
            .lists
            .filters
            .read()
            .unwrap()
            .blocks_host("changed.example")
    );
    assert!(Arc::ptr_eq(
        &updated,
        &engine.ctx.lists.exclusions.read().unwrap()
    ));
}

#[test]
fn a_reused_uid_cannot_receive_packets_from_an_earlier_selection() {
    let mut selection = UdpSelection::default();
    let uid = 10001;
    let app = [(uid, "com.example.first")];
    selection.replace(&app);
    let original = selection.owners[&uid].1;
    selection.replace(&[(uid, "com.example.first")]);
    assert_eq!(selection.owners[&uid].1, original);
    selection.replace(&[(uid, "com.example.new")]);
    assert_ne!(selection.owners[&uid].1, original);
    let second = selection.owners[&uid].1;
    selection.replace(&[]);
    selection.replace(&[(uid, "com.example.new")]);
    assert_ne!(selection.owners[&uid].1, second);
}

/// Says whether the selection was being changed when it was asked about
/// the authority.
#[derive(Default)]
struct Asked {
    engine: std::sync::OnceLock<std::sync::Weak<Engine>>,
    while_selection_held: std::sync::atomic::AtomicBool,
}

impl Platform for Asked {
    fn owner(&self, _: i32, _: SocketAddr, _: SocketAddr) -> Option<u32> {
        None
    }
    fn uid_of(&self, package: &str) -> Option<u32> {
        (package == "com.app").then_some(UID)
    }
    fn ca_installed(&self) -> bool {
        if let Some(engine) = self.engine.get().and_then(std::sync::Weak::upgrade)
            && engine.selected.try_read().is_err()
        {
            self.while_selection_held.store(true, Relaxed);
        }
        true
    }
    fn protect(&self, _: RawFd) -> bool {
        true
    }
}

#[tokio::test]
async fn android_is_not_asked_about_the_authority_while_the_selection_is_held() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(Paths::new(dir.path()).apps(), "com.app\n").unwrap();
    let platform = Arc::new(Asked::default());
    let engine = Engine::open_with(
        dir.path(),
        rustls::RootCertStore::empty(),
        platform.clone(),
        Arc::new(|_| Box::pin(async { Err(std::io::ErrorKind::Other.into()) })),
    )
    .unwrap();
    platform.engine.set(Arc::downgrade(&engine)).unwrap();
    engine.apply();
    assert!(
        engine
            .ctx
            .selection
            .https_inspection_available
            .load(Relaxed)
    );
    assert!(!platform.while_selection_held.load(Relaxed));
}
