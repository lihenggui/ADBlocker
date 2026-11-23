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

use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::wire::{Ipv4Packet, TcpPacket};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;

use super::*;
use crate::fake::{MTU, app_and_channels, first_syn, tunnel};
use crate::udp;

const SERVER: &str = "93.184.216.34:443";

#[test]
fn a_full_tun_queue_drops_the_packet_and_other_errors_end_the_tunnel() {
    let os = |errno| std::io::Error::from_raw_os_error(errno);
    for transient in [libc::EAGAIN, libc::EINTR, libc::ENOBUFS, libc::ENOMEM] {
        assert!(is_transient_write_error(&os(transient)), "{transient}");
    }
    for fatal in [libc::EBADF, libc::EIO, libc::EINVAL] {
        assert!(!is_transient_write_error(&os(fatal)), "{fatal}");
    }
}

#[tokio::test]
async fn the_first_tcp_poll_takes_one_full_batch_of_ready_packets() {
    let (app, tun) = app_and_channels();
    let sender = app.packet_sender();
    for port in 4000..4040 {
        sender
            .try_send(first_syn(&format!("93.184.216.34:{port}")))
            .unwrap();
    }
    let (seen, mut polls) = mpsc::channel(1);
    let tunnel = tokio::spawn(drive(
        tun,
        Config {
            mtu: MTU,
            handler: shouting(),
            reach: None,
            protect: Arc::new(|_| true),
            policy: Policy::test(Some(10001)),
            dns: None,
        },
        move |stack| {
            let _ = seen.try_send(stack.len());
            stack.poll()
        },
    ));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), polls.recv())
            .await
            .unwrap(),
        Some(32)
    );
    tunnel.abort();
    let _ = tunnel.await;
}

/// Run alone in release mode. Both the tunnel and fixture use this process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "a measurement, run by hand"]
async fn measures_bulk_transfer_with_idle_flows() {
    fn cpu_ticks() -> u64 {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        stat.rsplit_once(") ")
            .unwrap()
            .1
            .split_ascii_whitespace()
            .skip(11)
            .take(2)
            .map(|ticks| ticks.parse::<u64>().unwrap())
            .sum()
    }

    let tick_rate = std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .unwrap();
    assert!(tick_rate.status.success());
    let tick_rate: f64 = std::str::from_utf8(&tick_rate.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    const MIB: usize = 64;
    static CHUNK: [u8; 64 << 10] = [b'x'; 64 << 10];

    for idle in [0, 50, 200] {
        let (idle_tx, mut idle_rx) = mpsc::channel(200);
        let mut app = tunnel(Arc::new(move |mut socket, _, server| {
            if server.port() == 443 {
                idle_tx.try_send(socket).unwrap();
            } else {
                tokio::spawn(async move {
                    for _ in 0..(MIB << 20) / CHUNK.len() {
                        socket.write_all(&CHUNK).await.unwrap();
                    }
                });
            }
        }));
        let handles: Vec<_> = (0..idle).map(|_| app.connect(SERVER)).collect();
        let mut sockets = Vec::new();
        app.until(|app| {
            while let Ok(socket) = idle_rx.try_recv() {
                sockets.push(socket);
            }
            sockets.len() == idle && handles.iter().all(|&id| app.socket(id).may_send())
        })
        .await;
        app.leave_idle(handles);

        let start = std::time::Instant::now();
        let cpu = cpu_ticks();
        let bulk = app.connect("93.184.216.34:8443");
        tokio::time::timeout(Duration::from_secs(30), app.transfer(bulk, MIB << 20, b'x'))
            .await
            .unwrap();
        let cpu_ms = (cpu_ticks() - cpu) as f64 * 1000. / tick_rate;
        println!(
            "{idle} idle flows: {:.3} CPU ms/MiB, {:.1} MiB/s",
            cpu_ms / MIB as f64,
            MIB as f64 / start.elapsed().as_secs_f64()
        );
    }
}

/// Answers each connection with what it was sent, in capitals, and
/// ends when the app does.
fn shouting() -> Handler {
    Arc::new(|mut socket, _, _| {
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok(n) = socket.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let loud = buf[..n].to_ascii_uppercase();
                if socket.write_all(&loud).await.is_err() {
                    break;
                }
            }
        });
    })
}

#[tokio::test]
async fn a_connection_an_app_makes_is_handed_on_as_a_socket_with_where_it_was_going() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let told = seen.clone();
    let mut app = tunnel(Arc::new(move |socket, from, to| {
        told.lock().unwrap().push((from, to));
        // Kept open: dropped, it would end the connection.
        tokio::spawn(async move {
            let mut socket = socket;
            let _ = socket.read(&mut [0u8; 1]).await;
        });
    }));
    let handle = app.connect(SERVER);
    app.until(|_| !seen.lock().unwrap().is_empty()).await;
    let (from, to) = seen.lock().unwrap()[0];
    assert_eq!(to, SERVER.parse().unwrap());
    assert_eq!(from, "10.0.0.2:40001".parse().unwrap());
    assert_eq!(app.socket(handle).state(), tcp::State::Established);
}

#[tokio::test]
async fn bytes_pass_both_ways_and_each_side_ends_its_half() {
    let mut app = tunnel(shouting());
    let handle = app.connect(SERVER);
    app.until(|app| app.socket(handle).may_send()).await;
    app.socket(handle).send_slice(b"hello").unwrap();
    let mut got = Vec::new();
    app.until(|app| {
        got.extend(app.read(handle));
        got.len() == 5
    })
    .await;
    assert_eq!(got, b"HELLO");

    // The app has sent all it will: the other side ends too, and the
    // app sees that: ended from both sides, and only waited out.
    app.socket(handle).close();
    app.until(|app| app.socket(handle).state() == tcp::State::TimeWait)
        .await;
}

#[tokio::test]
async fn a_transfer_larger_than_the_buffers_and_its_fin_pass_during_udp_traffic() {
    let server = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let at = server.local_addr().unwrap();
    let datagram = udp::packet("10.0.0.2:5353".parse().unwrap(), at, b"udp", MTU).unwrap();
    let mut app = tunnel(shouting());
    let sender = app.packet_sender();
    let flood = tokio::spawn(async move { while sender.send(datagram.clone()).await.is_ok() {} });
    tokio::time::timeout(Duration::from_secs(2), server.recv_from(&mut [0; 8]))
        .await
        .unwrap()
        .unwrap();

    let handle = app.connect(SERVER);
    app.until(|app| app.socket(handle).may_send()).await;
    let much: Vec<u8> = (0..1_000_000u32).map(|n| b'a' + (n % 26) as u8).collect();
    let (mut sent, mut got) = (0, Vec::new());

    // Stop reading until the window closes, then let the sender resume.
    let mut window_closed = false;
    app.until(|app| {
        let socket = app.socket(handle);
        if sent < much.len() {
            sent += socket.send_slice(&much[sent..]).unwrap_or(0);
        }
        window_closed |= socket.recv_queue() == socket.recv_capacity();
        if window_closed {
            got.extend(app.read(handle));
        }
        got.len() == much.len()
    })
    .await;
    assert!(got == much.to_ascii_uppercase());

    app.socket(handle).close();
    app.until(|app| app.socket(handle).state() == tcp::State::TimeWait)
        .await;
    flood.abort();
    let _ = flood.await;
}

#[tokio::test]
async fn a_connection_refused_with_a_reset_is_reset_for_the_app() {
    // As the engine refuses: closed with nothing lingering.
    let mut app = tunnel(Arc::new(|socket, _, _| {
        let _ = socket2::SockRef::from(&socket).set_linger(Some(Duration::ZERO));
    }));
    let handle = app.connect(SERVER);
    app.until(|app| app.socket(handle).state() == tcp::State::Closed)
        .await;
    // Not an ending the app could take for the server's own.
    assert!(app.read(handle).is_empty());
}

#[tokio::test]
async fn a_connection_the_app_resets_is_reset_for_whoever_has_its_socket() {
    let (said, mut heard) = mpsc::unbounded_channel();
    let mut app = tunnel(Arc::new(move |mut socket, _, _| {
        let said = said.clone();
        tokio::spawn(async move {
            let ended = socket.read(&mut [0u8; 16]).await;
            let _ = said.send(ended.map_err(|e| e.kind()));
        });
    }));
    let handle = app.connect(SERVER);
    app.until(|app| app.socket(handle).may_send()).await;
    app.socket(handle).abort();
    let mut ended = None;
    app.until(|_| {
        ended = heard.try_recv().ok();
        ended.is_some()
    })
    .await;
    assert_eq!(ended.unwrap(), Err(io::ErrorKind::ConnectionReset));
}

#[tokio::test]
async fn many_connections_at_once_each_keep_their_own_bytes() {
    let mut app = tunnel(shouting());
    let handles: Vec<SocketHandle> = (0..40).map(|_| app.connect(SERVER)).collect();
    app.until(|app| handles.iter().all(|h| app.socket(*h).may_send()))
        .await;
    for (n, handle) in handles.iter().enumerate() {
        let said = format!("connection {n}");
        app.socket(*handle).send_slice(said.as_bytes()).unwrap();
    }
    let mut got = vec![Vec::new(); handles.len()];
    app.until(|app| {
        for (n, handle) in handles.iter().enumerate() {
            got[n].extend(app.read(*handle));
        }
        got.iter()
            .enumerate()
            .all(|(n, g)| g.len() == format!("connection {n}").len())
    })
    .await;
    for (n, got) in got.iter().enumerate() {
        assert_eq!(got, format!("CONNECTION {n}").as_bytes());
    }
}

#[tokio::test]
async fn an_app_is_answered_only_once_its_server_is_found_to_be_there() {
    use crate::fake::tunnel_reaching;
    // The server answers after a while: so does the app's connection.
    let slow: Reach = Arc::new(|_, _| {
        Box::pin(async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            Ok(())
        })
    });
    let mut app = tunnel_reaching(shouting(), Some(slow));
    let handle = app.connect(SERVER);
    let began = std::time::Instant::now();
    app.until(|app| app.socket(handle).may_send()).await;
    assert!(began.elapsed() >= Duration::from_millis(150));

    // A server that says nothing: neither is the app told anything,
    // and it waits as it would have.
    let silent: Reach = Arc::new(|_, _| Box::pin(async { Err(io::ErrorKind::TimedOut.into()) }));
    let mut app = tunnel_reaching(shouting(), Some(silent));
    let handle = app.connect(SERVER);
    let mut polls = 0;
    app.until(|_| {
        polls += 1;
        polls > 300
    })
    .await;
    assert_eq!(app.socket(handle).state(), tcp::State::SynSent);
}

#[tokio::test]
async fn datagrams_pass_both_ways_without_polling_tcp() {
    let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let at = server.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = server.recv_from(&mut buf).await {
            let _ = server.send_to(&buf[..n].to_ascii_uppercase(), from).await;
        }
    });
    let (mut app, channels) = app_and_channels();
    let polls = Arc::new(AtomicUsize::new(0));
    let counted = polls.clone();
    tokio::spawn(drive(
        channels,
        Config {
            mtu: MTU,
            handler: shouting(),
            reach: None,
            protect: Arc::new(|_| true),
            policy: Policy::test(Some(10001)),
            dns: None,
        },
        move |stack| {
            counted.fetch_add(1, Relaxed);
            stack.poll()
        },
    ));
    let from: SocketAddr = "10.0.0.2:5353".parse().unwrap();
    app.send(udp::packet(from, at, b"a question", 1500).unwrap())
        .await;
    let bytes = app.given().await.unwrap();
    let answer = udp::parse(&bytes).unwrap();
    assert_eq!(
        (answer.app, answer.server, answer.payload),
        (at, from, b"A QUESTION".as_slice())
    );
    assert_eq!(polls.load(Relaxed), 0);
}

struct BrokenTun(crate::fake::Channels);

impl Tun for BrokenTun {
    async fn recv(&mut self, into: &mut [u8]) -> io::Result<usize> {
        self.0.recv(into).await
    }

    fn send(&mut self, _: &[u8]) -> io::Result<()> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
}

#[tokio::test(start_paused = true)]
async fn a_fatal_tcp_write_ends_the_tunnel() {
    let (mut app, tun) = app_and_channels();
    app.send(first_syn(SERVER)).await;
    let running = run(
        BrokenTun(tun),
        Config {
            mtu: MTU,
            handler: shouting(),
            reach: None,
            protect: Arc::new(|_| true),
            policy: Policy::test(Some(10001)),
            dns: None,
        },
    );
    let error = tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
}

/// Keeps the descriptor readable after the first SYN. The finite
/// supply lets the test fail if the retry never comes during traffic.
struct BusyTun {
    syn: Option<Vec<u8>>,
    datagram: Vec<u8>,
    left: usize,
    sent: mpsc::UnboundedSender<(Instant, Vec<u8>)>,
    block_first_write: bool,
}

impl Tun for BusyTun {
    async fn recv(&mut self, into: &mut [u8]) -> io::Result<usize> {
        let syn = self.syn.take();
        let packet = if let Some(syn) = &syn {
            syn
        } else if self.left > 0 {
            self.left -= 1;
            &self.datagram
        } else {
            return Ok(0);
        };
        into[..packet.len()].copy_from_slice(packet);
        Ok(packet.len())
    }

    fn send(&mut self, packet: &[u8]) -> io::Result<()> {
        let _ = self.sent.send((Instant::now(), packet.to_vec()));
        if std::mem::take(&mut self.block_first_write) {
            Err(io::ErrorKind::WouldBlock.into())
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn eof_inside_an_ingress_batch_still_flushes_a_tcp_reply() {
    let (sent, mut replies) = mpsc::unbounded_channel();
    let tun = BusyTun {
        syn: Some(first_syn(SERVER)),
        datagram: Vec::new(),
        left: 0,
        sent,
        block_first_write: false,
    };
    let running = run(
        tun,
        Config {
            mtu: MTU,
            handler: shouting(),
            reach: None,
            protect: Arc::new(|_| true),
            policy: Policy::test(Some(10001)),
            dns: None,
        },
    );
    tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    let (_, reply) = replies.try_recv().unwrap();
    let ip = Ipv4Packet::new_checked(reply.as_slice()).unwrap();
    let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
    assert!(tcp.syn() && tcp.ack());
}

#[tokio::test(start_paused = true)]
async fn a_syn_ack_dropped_on_backpressure_is_retransmitted_at_its_deadline_during_udp() {
    // Place the TCP deadline between Tokio's millisecond timer ticks.
    tokio::time::advance(Duration::from_micros(125)).await;
    let start = Instant::now();
    let server = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let at = server.local_addr().unwrap();
    let datagram = udp::packet("10.0.0.2:5353".parse().unwrap(), at, b"udp", MTU).unwrap();
    let (sent, mut replies) = mpsc::unbounded_channel();
    let tun = BusyTun {
        syn: Some(first_syn(SERVER)),
        datagram,
        left: 2048,
        sent,
        block_first_write: true,
    };
    let delays = Arc::new(Mutex::new(Vec::new()));
    let timed = delays.clone();
    let running = tokio::spawn(drive(
        tun,
        Config {
            mtu: MTU,
            handler: shouting(),
            reach: None,
            protect: Arc::new(|_| true),
            policy: Policy::test(Some(10001)),
            dns: None,
        },
        move |stack| {
            let delay = stack.poll();
            timed.lock().unwrap().push(delay);
            delay
        },
    ));

    let (sent_at, syn_ack) = replies.recv().await.unwrap();
    assert_eq!(sent_at, start);
    let delay = {
        let delays = delays.lock().unwrap();
        // Only the SYN may cause stack work before the deadline.
        assert!(delays.len() <= 2, "UDP packets triggered TCP polling");
        delays.last().unwrap().unwrap()
    };
    tokio::time::advance(delay).await;
    let (resent_at, retry) = replies.recv().await.expect("SYN-ACK was not retransmitted");
    assert_eq!(resent_at, start + delay);
    let sequence = |packet: &[u8]| {
        let ip = Ipv4Packet::new_checked(packet).unwrap();
        let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
        assert!(tcp.syn() && tcp.ack());
        (tcp.seq_number(), tcp.ack_number())
    };
    assert_eq!(sequence(&syn_ack), sequence(&retry));
    running.abort();
    let _ = running.await;
}
