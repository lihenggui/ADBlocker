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

//! The engine as the app holds it, asked as the app asks.

use std::cell::Cell;
use std::net::SocketAddr;
use std::os::fd::RawFd;
use std::os::unix::net::UnixDatagram;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::Duration;

use super::*;

struct Device {
    ca_installed: AtomicBool,
}

impl Platform for Device {
    fn owner(&self, _: i32, _: SocketAddr, _: SocketAddr) -> Option<u32> {
        Some(10001)
    }
    fn uid_of(&self, package: &str) -> Option<u32> {
        (package == "com.app").then_some(10001)
    }
    fn ca_installed(&self) -> bool {
        self.ca_installed.load(Relaxed)
    }
    fn protect(&self, _: RawFd) -> bool {
        true
    }
}

/// A root to verify servers against: any certificate will do.
fn a_root() -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    abd_engine::ca::Authority::open(dir.path()).unwrap();
    let pem = std::fs::read(dir.path().join("system.pem")).unwrap();
    CertificateDer::from_pem_slice(&pem).unwrap().to_vec()
}

fn session() -> (Session, Arc<Device>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let device = Arc::new(Device {
        ca_installed: AtomicBool::new(false),
    });
    let session = Session::open(dir.path(), &[a_root()], device.clone()).unwrap();
    (session, device, dir)
}

fn said(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).unwrap()
}

fn close_with_a_delayed_callback() -> Duration {
    let (session, _, _dir) = session();
    let (started, ready) = mpsc::sync_channel(1);
    let (done, finished) = mpsc::sync_channel(1);
    session.runtime().spawn_blocking(move || {
        started.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(750));
        done.send(()).unwrap();
    });
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let began = std::time::Instant::now();
    drop(session);
    let elapsed = began.elapsed();
    finished.recv_timeout(Duration::from_secs(2)).unwrap();
    elapsed
}

#[test]
fn closing_a_session_does_not_wait_for_a_stuck_blocking_callback() {
    assert!(close_with_a_delayed_callback() < Duration::from_millis(500));
}

#[test]
#[ignore = "a measurement, run by hand"]
fn measures_closing_with_a_delayed_blocking_callback() {
    for _ in 0..5 {
        println!(
            "session close with a delayed callback: {:?}",
            close_with_a_delayed_callback()
        );
    }
}

#[test]
fn a_command_is_answered_as_the_daemon_answers_it() {
    let (session, _, _dir) = session();
    assert_eq!(
        session.command(&["app", "add", "com.app"], b""),
        Output::default()
    );
    assert_eq!(said(&session.command(&["app", "list"], b"")), "com.app\n");
    for key in ["@com.shared", "37:@com.shared"] {
        assert_eq!(session.command(&["app", "add", key], b"").exit, 2);
    }
    assert_eq!(said(&session.command(&["app", "list"], b"")), "com.app\n");
    let status = session.command(&["status", "--json"], b"");
    assert_eq!(status.exit, 0);
    // No VPN yet. URL filtering is on, as it is for the module.
    assert!(said(&status).contains("\"running\":false"));
    assert!(said(&status).contains("\"url_filtering\":true"));

    // What nobody knows, and what only the module has.
    for unknown in [&["nonsense"][..], &["run"], &["unhook"], &[]] {
        let output = session.command(unknown, b"");
        assert_eq!(output.exit, 2, "{unknown:?}");
        assert!(!output.stderr.is_empty());
    }
    // Known, and failing.
    assert_eq!(
        session
            .command(&["app", "pause", "com.other", "--minutes", "1"], b"")
            .exit,
        1
    );
    // Read from what it is given.
    session.command(&["rules", "set"], b"||ads.example^\n");
    assert_eq!(
        said(&session.command(&["rules", "get"], b"")),
        "||ads.example^\n"
    );
}

#[test]
fn unreadable_user_policy_scope_cannot_be_used_to_rebuild_vpn_routes() {
    let (session, _, _dir) = session();
    let file = session.engine.paths().settings();
    std::fs::write(&file, [0xff]).unwrap();
    for args in [["app", "selections", "--json"], ["app", "list", "--json"]] {
        let output = session.command(&args, b"");
        assert_eq!(output.exit, 1, "{args:?}");
        assert!(output.stdout.is_empty());
    }
    std::fs::write(&file, "scope all\n").unwrap();
    let selections = session.command(&["app", "selections", "--json"], b"");
    assert_eq!(selections.exit, 0);
    assert!(said(&selections).contains(r#""scope":"all""#));
}

#[test]
fn an_app_is_filtered_by_host_while_the_authority_or_exclusions_are_missing() {
    let (session, device, _dir) = session();
    session.command(&["app", "add", "com.app"], b"");
    let level = |session: &Session| {
        let list = session.command(&["app", "list", "--json"], b"");
        let list = said(&list).to_owned();
        let (_, https) = list.split_once("\"https\":").unwrap();
        let (_, reason) = https.split_once("\"reason\":").unwrap();
        reason.split([',', '}']).next().unwrap().to_owned()
    };
    // On from the start, and nothing decrypted without the certificate.
    assert_eq!(level(&session), "\"ca_not_installed\"");
    session.command(&["url-filtering", "off"], b"");
    assert_eq!(level(&session), "\"url_filtering_off\"");
    session.command(&["url-filtering", "on"], b"");
    assert_eq!(level(&session), "\"ca_not_installed\"");
    device.ca_installed.store(true, Relaxed);
    session.command(&["reload"], b"");
    assert_eq!(level(&session), "\"exclusions_pending\"");
}

#[test]
fn pin_removal_uses_the_vpn_table_even_while_the_tunnel_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let paths = abd_engine::paths::Paths::new(dir.path());
    std::fs::write(paths.apps(), "com.app\n").unwrap();
    std::fs::write(
        paths.pinned(),
        format!("{} com.app pinned.example\n", clock::now()),
    )
    .unwrap();
    let device = Arc::new(Device {
        ca_installed: AtomicBool::new(false),
    });
    let session = Session::open(dir.path(), &[a_root()], device).unwrap();

    assert_eq!(
        session.command(&["pinned", "forget", "com.app", "pinned.example"], b""),
        Output::default()
    );
    assert!(!paths.forget().exists());
    assert_eq!(said(&session.command(&["pinned", "--json"], b"")), "[]\n");
}

#[test]
fn the_authority_is_given_out_for_the_user_to_install() {
    let (session, _, _dir) = session();
    let pem = session.command(&["ca", "export"], b"");
    assert!(said(&pem).starts_with("-----BEGIN CERTIFICATE-----"));
    let der = session.command(&["ca", "export", "--der"], b"");
    assert_eq!(der.exit, 0);
    assert_eq!(
        CertificateDer::from_pem_slice(&pem.stdout)
            .unwrap()
            .as_ref(),
        &der.stdout[..]
    );
    // The certificate only: never the key.
    assert!(!said(&pem).contains("PRIVATE KEY"));
    assert_eq!(session.command(&["ca", "export", "--key"], b"").exit, 2);
}

#[test]
fn without_roots_the_engine_does_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let device = Arc::new(Device {
        ca_installed: AtomicBool::new(false),
    });
    assert!(Session::open(dir.path(), &[], device.clone()).is_err());
    assert!(Session::open(dir.path(), &[b"not a certificate".to_vec()], device).is_err());
}

/// Says what it is told to a channel.
struct Told(mpsc::Sender<std::result::Result<String, i32>>);

impl Sink for Told {
    fn line(&mut self, text: &str) {
        let _ = self.0.send(Ok(text.to_owned()));
    }
    fn end(&mut self, exit: i32) {
        let _ = self.0.send(Err(exit));
    }
}

#[test]
fn watching_ends_when_it_is_cancelled_and_says_so_once() {
    let (session, _, _dir) = session();
    let (tell, told) = mpsc::channel();
    let id = session.stream(&["watch", "--json"], Box::new(Told(tell)));
    assert_ne!(id, 0);
    session.cancel(id);
    let soon = Duration::from_secs(5);
    assert_eq!(told.recv_timeout(soon).unwrap(), Err(0));
    // Once: the sink is gone with it.
    assert!(told.recv_timeout(soon).is_err());
    session.cancel(id);
}

/// Like a JNI line callback waiting for room in the app's channel.
struct Stalled {
    told: Told,
    release: Option<mpsc::Receiver<()>>,
}

impl Sink for Stalled {
    fn line(&mut self, text: &str) {
        self.told.line(text);
        if let Some(release) = self.release.take() {
            let _ = release.recv();
        }
    }

    fn end(&mut self, exit: i32) {
        self.told.end(exit);
    }
}

#[test]
fn stalled_watch_callbacks_leave_packet_processing_running_and_cancel_frees_their_places() {
    let (session, _, _dir) = session();
    let (system, ours) = UnixDatagram::pair().unwrap();
    let soon = Duration::from_secs(5);
    system.set_read_timeout(Some(soon)).unwrap();
    session
        .start(ours.into(), false, Vec::new(), Box::new(|_| {}))
        .unwrap();

    let watch = || {
        let (tell, told) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let id = session.stream(
            &["watch", "--json"],
            Box::new(Stalled {
                told: Told(tell),
                release: Some(wait),
            }),
        );
        assert_ne!(id, 0);
        (id, told, release)
    };
    let watchers = [watch(), watch()];
    let hub = session.engine.live();
    hub.host_blocked("com.app", "ads.example");
    for (_, told, _) in &watchers {
        assert!(
            told.recv_timeout(soon)
                .unwrap()
                .unwrap()
                .contains("ads.example")
        );
    }

    // Packet processing continues while both observer callbacks are waiting.
    check_udp_round_trip(&system);

    for (id, _, _) in &watchers {
        session.cancel(*id);
    }
    // A cancellation cannot free a writer still inside a callback.
    let (tell, told) = mpsc::channel();
    assert_eq!(session.stream(&["watch"], Box::new(Told(tell))), 0);
    assert_eq!(told.recv_timeout(soon).unwrap(), Err(1));
    for (_, told, release) in watchers {
        release.send(()).unwrap();
        assert_eq!(told.recv_timeout(soon).unwrap(), Err(0));
        assert_eq!(
            told.recv_timeout(soon),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    let (tell, told) = mpsc::channel();
    let id = session.stream(&["watch"], Box::new(Told(tell)));
    assert_ne!(id, 0);
    hub.host_blocked("com.app", "again.example");
    assert!(
        told.recv_timeout(soon)
            .unwrap()
            .unwrap()
            .contains("again.example")
    );
    session.cancel(id);
    assert_eq!(told.recv_timeout(soon).unwrap(), Err(0));
}

#[test]
fn updating_delivers_progress_and_the_summary_before_ending() {
    let (session, _, _dir) = session();
    let paths = session.engine.paths();
    // A malformed source fails before any network access, but still
    // gives the app each stage of an update.
    std::fs::write(paths.sources(), "not-an-https-address\n").unwrap();
    std::fs::write(paths.exclusion_sources(), "").unwrap();
    let (tell, told) = mpsc::channel();
    assert_ne!(
        session.stream(&["update", "--json"], Box::new(Told(tell))),
        0
    );
    let soon = Duration::from_secs(5);
    for part in [
        r#""event":"start""#,
        r#""state":"fetching""#,
        r#""state":"failed""#,
        r#""event":"done""#,
    ] {
        assert!(told.recv_timeout(soon).unwrap().unwrap().contains(part));
    }
    assert_eq!(told.recv_timeout(soon).unwrap(), Err(0));
    assert_eq!(
        told.recv_timeout(soon),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}

#[test]
fn what_does_not_go_on_is_not_started_as_if_it_did() {
    let (session, _, _dir) = session();
    for args in [
        &["status"][..],
        &["watch", "--app", "not a package"],
        &["update", "--now"],
    ] {
        let (tell, told) = mpsc::channel();
        assert_eq!(session.stream(args, Box::new(Told(tell))), 0, "{args:?}");
        assert_eq!(told.recv().unwrap(), Err(2));
    }
}

fn send_udp_reply(tun: &UnixDatagram) -> (SocketAddr, SocketAddr) {
    let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let client = "10.0.0.2:40000".parse().unwrap();
    let dst = server.local_addr().unwrap();
    tun.send(&crate::datagram::packet(client, dst, b"hello", MTU).unwrap())
        .unwrap();
    let mut packet = [0u8; MTU];
    let (n, peer) = server.recv_from(&mut packet).unwrap();
    assert_eq!(&packet[..n], b"hello");
    server.send_to(b"reply", peer).unwrap();
    (client, dst)
}

fn check_udp_round_trip(tun: &UnixDatagram) {
    let (client, dst) = send_udp_reply(tun);
    let mut packet = [0u8; MTU];
    let n = tun.recv(&mut packet).unwrap();
    let reply = crate::datagram::parse(&packet[..n]).unwrap();
    assert_eq!(
        (reply.app, reply.server, reply.payload),
        (dst, client, b"reply".as_slice())
    );
}

#[test]
fn a_fatal_udp_write_tells_the_app_that_the_vpn_failed() {
    let (session, _, _dir) = session();
    let (system, ours) = UnixDatagram::pair().unwrap();
    ours.shutdown(std::net::Shutdown::Write).unwrap();
    let (tell, told) = mpsc::channel();
    session
        .start(
            ours.into(),
            false,
            Vec::new(),
            Box::new(move |code| tell.send(code).unwrap()),
        )
        .unwrap();
    send_udp_reply(&system);
    assert_eq!(told.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
    assert!(system.send(b"anyone?").is_err());
}

#[test]
fn the_vpn_is_filtered_from_start_to_stop_and_can_be_started_again() {
    let (session, _, _dir) = session();
    session.command(&["app", "add", "com.app"], b"");
    for _ in 0..2 {
        let (system, ours) = UnixDatagram::pair().unwrap();
        system
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        session
            .start(ours.into(), false, Vec::new(), Box::new(|_| {}))
            .unwrap();
        assert!(said(&session.command(&["status", "--json"], b"")).contains("\"running\":true"));

        check_udp_round_trip(&system);

        session.stop();
        assert!(said(&session.command(&["status", "--json"], b"")).contains("\"running\":false"));
        // The descriptor is closed: nothing is there to send to.
        assert!(system.send(b"anyone?").is_err());
    }
}

#[test]
fn a_pause_made_while_the_vpn_runs_arms_its_idle_timer() {
    let (session, _, _dir) = session();
    assert_eq!(
        session.command(&["app", "add", "com.app"], b""),
        Output::default()
    );
    let (system, ours) = UnixDatagram::pair().unwrap();
    session
        .start(ours.into(), false, Vec::new(), Box::new(|_| {}))
        .unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(async {
        let now = Cell::new(clock::now());
        let mut ends = session.engine.apply_at();
        let waiting = wait_for_pause_end(&mut ends, || now.get());
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), &mut waiting)
                .await
                .is_err()
        );

        assert_eq!(
            session.command(&["app", "pause", "com.app", "--minutes", "1"], b""),
            Output::default()
        );
        let until = pause::Pauses::read(&session.engine.paths().pauses(), now.get())
            .next_end()
            .unwrap();
        now.set(until - 60);
        assert!(
            tokio::time::timeout(Duration::from_secs(59), &mut waiting)
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(2), &mut waiting)
                .await
                .unwrap()
        );
    });
    drop(system);
}

/// Android, slow to answer once while the end of a pause is applied.
struct Stalling {
    stall: Mutex<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>>,
}

impl Platform for Stalling {
    fn owner(&self, _: i32, _: SocketAddr, _: SocketAddr) -> Option<u32> {
        Some(10001)
    }
    fn uid_of(&self, package: &str) -> Option<u32> {
        let stall = self.stall.lock().unwrap().take();
        if let Some((entered, release)) = stall {
            entered.send(()).unwrap();
            let _ = release.recv();
        }
        (package == "com.app").then_some(10001)
    }
    fn ca_installed(&self) -> bool {
        false
    }
    fn protect(&self, _: RawFd) -> bool {
        true
    }
}

#[test]
fn packets_keep_moving_while_the_end_of_a_pause_is_applied() {
    let dir = tempfile::tempdir().unwrap();
    let device = Arc::new(Stalling {
        stall: Mutex::new(None),
    });
    let session = Session::open(dir.path(), &[a_root()], device.clone()).unwrap();
    session.command(&["app", "add", "com.app"], b"");
    let (system, ours) = UnixDatagram::pair().unwrap();
    let soon = Duration::from_secs(5);
    system.set_read_timeout(Some(soon)).unwrap();
    session
        .start(ours.into(), false, Vec::new(), Box::new(|_| {}))
        .unwrap();

    // A pause that ends in a second, read before Android is slow.
    let pauses = session.engine.paths().pauses();
    std::fs::write(pauses, format!("{} com.app\n", clock::now() + 1)).unwrap();
    session.command(&["reload"], b"");
    let (entered, inside) = mpsc::sync_channel(1);
    let (release, released) = mpsc::channel();
    *device.stall.lock().unwrap() = Some((entered, released));
    inside.recv_timeout(soon).unwrap();

    check_udp_round_trip(&system);
    release.send(()).unwrap();
}

#[test]
fn the_app_is_told_when_the_vpn_ends_by_itself() {
    let (session, _, _dir) = session();
    // A kind of socket that, as the descriptor of a VPN taken down, says
    // so to whoever reads it.
    let (system, ours) =
        socket2::Socket::pair(socket2::Domain::UNIX, socket2::Type::SEQPACKET, None).unwrap();
    let ours = std::os::fd::OwnedFd::from(ours);
    let (tell, told) = mpsc::channel();
    let stopped = Box::new(move |code| {
        let _ = tell.send(code);
    });
    session.start(ours, true, Vec::new(), stopped).unwrap();
    assert_eq!(InApp(&session).vpn_ipv6(), Some(true));
    // The system takes the VPN down: no more packets, ever.
    drop(system);
    assert_eq!(told.recv_timeout(Duration::from_secs(5)).unwrap(), 0);
    // Not said to run once it has ended, before the app stops it.
    let status = || said(&session.command(&["status", "--json"], b"")).to_owned();
    for _ in 0..500 {
        if status().contains("\"running\":false") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(status().contains("\"running\":false"));
    assert_eq!(InApp(&session).vpn_ipv6(), None);
}
