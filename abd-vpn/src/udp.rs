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

//! UDP relay for the apps Android routes into the VPN. Refuse recognized
//! QUIC Initials when their owner is an active selection.
//! Replies must come from the original peer.

use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use abd_engine::quic::is_initial;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    Icmpv4Packet, Icmpv6Packet, IpProtocol, Ipv4Packet, Ipv4Repr, Ipv6Packet, Ipv6Repr,
};
use tokio::net::UdpSocket;
use tokio::sync::{Semaphore, mpsc};

pub use crate::datagram::{Datagram, packet, parse};
use crate::log::log;
pub use crate::platform::Protect;
use crate::platform::{lookup_owner, protect_socket};

/// Separate relays for ordinary UDP and UDP 443 share the 256-socket budget.
const MAX_PORTS: usize = 128;
const MAX_PEERS_PER_PORT: usize = 64;
const MAX_ADMISSIONS: usize = 512;
const ADMISSION_TTL: Duration = Duration::from_secs(2);

/// How long a socket is kept with nothing passing either way.
const IDLE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Admission {
    pub uid: u32,
    pub epoch: u64,
    pub generation: u64,
    pub filter: bool,
}

type Owner = (u32, u64);

impl Admission {
    fn owner(self) -> Owner {
        (self.uid, self.epoch)
    }
}

type Lookup = Arc<dyn Fn(SocketAddr, SocketAddr) -> Option<Admission> + Send + Sync>;

#[derive(Clone)]
pub struct Policy {
    pub lookup: Lookup,
    pub current: Arc<dyn Fn(Admission) -> bool + Send + Sync>,
}

#[cfg(test)]
impl Policy {
    pub fn test(uid: Option<u32>) -> Self {
        Self {
            lookup: Arc::new(move |_, _| {
                uid.map(|uid| Admission {
                    uid,
                    epoch: 1,
                    generation: 0,
                    filter: true,
                })
            }),
            current: Arc::new(|_| true),
        }
    }
}

impl Policy {
    async fn admission(&self, app: SocketAddr, peer: SocketAddr) -> Option<Admission> {
        static LOOKUPS: Semaphore = Semaphore::const_new(4);
        let lookup = self.lookup.clone();
        lookup_owner(&LOOKUPS, move || lookup(app, peer))
            .await
            .filter(|&admission| (self.current)(admission))
    }
}

/// Quote the invoking packet as received, including IPv4 options if present.
fn port_unreachable(original: &[u8], mtu: usize) -> Option<Vec<u8>> {
    const ICMP_HEADER: usize = 8;
    match original.first()? >> 4 {
        4 => {
            let ip = Ipv4Packet::new_checked(original).ok()?;
            let quote = original.get(..usize::from(ip.header_len()) + 8)?;
            let outer = Ipv4Repr {
                src_addr: ip.dst_addr(),
                dst_addr: ip.src_addr(),
                next_header: IpProtocol::Icmp,
                payload_len: ICMP_HEADER + quote.len(),
                hop_limit: 64,
            };
            let header = outer.buffer_len();
            let len = header + outer.payload_len;
            (len <= mtu).then_some(())?;
            let mut bytes = vec![0; len];
            outer.emit(
                &mut Ipv4Packet::new_unchecked(&mut bytes),
                &ChecksumCapabilities::default(),
            );
            let icmp = &mut bytes[header..];
            icmp[..2].copy_from_slice(&[3, 3]);
            icmp[ICMP_HEADER..].copy_from_slice(quote);
            Icmpv4Packet::new_unchecked(icmp).fill_checksum();
            Some(bytes)
        }
        6 => {
            let ip = Ipv6Packet::new_checked(original).ok()?;
            let mut outer = Ipv6Repr {
                src_addr: ip.dst_addr(),
                dst_addr: ip.src_addr(),
                next_header: IpProtocol::Icmpv6,
                payload_len: 0,
                hop_limit: 64,
            };
            let header = outer.buffer_len();
            // An ICMPv6 error must fit in the IPv6 minimum MTU.
            let max_quote = mtu.min(1280).checked_sub(header + ICMP_HEADER)?;
            let quote = original.get(..original.len().min(max_quote))?;
            (quote.len() >= header + 8).then_some(())?;
            outer.payload_len = ICMP_HEADER + quote.len();
            let mut bytes = vec![0; header + outer.payload_len];
            outer.emit(&mut Ipv6Packet::new_unchecked(&mut bytes));
            let icmp = &mut bytes[header..];
            icmp[..2].copy_from_slice(&[1, 4]);
            icmp[ICMP_HEADER..].copy_from_slice(quote);
            Icmpv6Packet::new_unchecked(icmp).fill_checksum(&outer.src_addr, &outer.dst_addr);
            Some(bytes)
        }
        _ => None,
    }
}

/// A socket of ours for one port of an app, and for one kind of address.
struct Port {
    /// Recent destinations for a shared, unconnected UDP socket.
    peers: VecDeque<SocketAddr>,
    /// To send on by: the same socket its task takes the answers from.
    socket: std::net::UdpSocket,
    /// When something last passed, in seconds since `Forwarder::started`.
    used: Arc<AtomicU64>,
    task: tokio::task::AbortHandle,
    /// Whether its task still takes the answers.
    open: Arc<AtomicBool>,
}

impl Drop for Port {
    fn drop(&mut self) {
        self.open.store(false, Relaxed);
        self.task.abort();
    }
}

pub struct Reply {
    open: Arc<AtomicBool>,
    packet: Vec<u8>,
    owner: Option<Owner>,
}

struct CachedAdmission {
    admission: Admission,
    expires: Instant,
}

pub struct Forwarder {
    admissions: HashMap<(SocketAddr, SocketAddr), CachedAdmission>,
    ports: HashMap<(SocketAddr, Option<(SocketAddr, Owner)>), Port>,
    /// The packets for the apps, for whoever has the VPN's descriptor.
    back: mpsc::Sender<Reply>,
    protect: Protect,
    policy: Policy,
    mtu: usize,
    started: Instant,
    // One bucket per relay, shared by all sockets in that relay.
    refusal_tokens: u32,
    last_refill: Instant,
    /// Whether a socket that could not be opened has been told of.
    told_open_failure: bool,
}

impl Forwarder {
    pub fn new(back: mpsc::Sender<Reply>, protect: Protect, policy: Policy, mtu: usize) -> Self {
        Self {
            admissions: HashMap::new(),
            ports: HashMap::new(),
            back,
            protect,
            policy,
            mtu,
            started: Instant::now(),
            refusal_tokens: 32,
            last_refill: Instant::now(),
            told_open_failure: false,
        }
    }

    async fn open(
        &mut self,
        datagram: &Datagram<'_>,
        owner: Option<Owner>,
    ) -> std::io::Result<&mut Port> {
        let key = (datagram.app, owner.map(|owner| (datagram.server, owner)));
        if self.ports.get(&key).is_some_and(|p| !p.open.load(Relaxed)) {
            self.ports.remove(&key);
        }
        if !self.ports.contains_key(&key) {
            if self.ports.len() >= MAX_PORTS {
                self.ports.retain(|_, port| port.open.load(Relaxed));
            }
            // Still full: the one unused for longest makes room. Its
            // task ends when it finds itself closed.
            if self.ports.len() >= MAX_PORTS {
                let oldest = self
                    .ports
                    .iter()
                    .min_by_key(|(_, port)| port.used.load(Relaxed))
                    .map(|(key, _)| *key);
                if let Some(port) = oldest.and_then(|key| self.ports.remove(&key)) {
                    port.open.store(false, Relaxed);
                }
            }
            let any: SocketAddr = if datagram.server.is_ipv4() {
                (Ipv4Addr::UNSPECIFIED, 0).into()
            } else {
                (Ipv6Addr::UNSPECIFIED, 0).into()
            };
            let socket = std::net::UdpSocket::bind(any)?;
            socket.set_nonblocking(true)?;
            let socket = protect_socket(self.protect.clone(), socket).await?;
            if owner.is_some() {
                socket.connect(datagram.server)?;
            }
            let upstream = socket.try_clone()?;
            let used = Arc::new(AtomicU64::new(self.started.elapsed().as_secs()));
            let open = Arc::new(AtomicBool::new(true));
            let task = tokio::spawn(answers(
                Arc::new(UdpSocket::from_std(socket)?),
                used.clone(),
                open.clone(),
                (datagram.app, owner),
                self.back.clone(),
                self.mtu,
                self.started,
            ));
            let port = Port {
                peers: VecDeque::new(),
                socket: upstream,
                used,
                open,
                task: task.abort_handle(),
            };
            self.ports.insert(key, port);
        }
        Ok(self.ports.get_mut(&key).expect("open UDP port"))
    }

    fn cached(&self, app: SocketAddr, peer: SocketAddr, now: Instant) -> Option<Admission> {
        let entry = self.admissions.get(&(app, peer))?;
        (entry.expires > now && (self.policy.current)(entry.admission)).then_some(entry.admission)
    }

    async fn admission(&mut self, app: SocketAddr, peer: SocketAddr) -> Option<Admission> {
        if let Some(current) = self.cached(app, peer, Instant::now()) {
            return Some(current);
        }
        let key = (app, peer);
        self.admissions.remove(&key);
        let admission = self.policy.admission(app, peer).await?;
        if self.admissions.len() >= MAX_ADMISSIONS {
            let oldest = self
                .admissions
                .iter()
                .min_by_key(|(_, entry)| entry.expires)
                .map(|(key, _)| *key);
            if let Some(oldest) = oldest {
                self.admissions.remove(&oldest);
            }
        }
        self.admissions.insert(
            key,
            CachedAdmission {
                admission,
                expires: Instant::now() + ADMISSION_TTL,
            },
        );
        Some(admission)
    }

    // Refill at 64 errors per second with a burst of 32. Above the
    // limit, let traffic continue without synthesizing an error.
    fn can_refuse(&mut self) -> bool {
        const TICK: Duration = Duration::from_millis(16);
        let now = Instant::now();
        let refill =
            (now.duration_since(self.last_refill).as_nanos() / TICK.as_nanos()).min(32) as u32;
        if refill != 0 {
            self.last_refill = now;
            self.refusal_tokens = (self.refusal_tokens + refill).min(32);
        }
        if let Some(left) = self.refusal_tokens.checked_sub(1) {
            self.refusal_tokens = left;
            true
        } else {
            false
        }
    }

    /// Returns true when an Initial needs an immediate error in the TUN.
    pub async fn send(&mut self, datagram: &Datagram<'_>) -> bool {
        let web = datagram.server.port() == 443;
        let initial = is_initial(datagram.payload);
        if !web && !initial {
            self.forward(datagram, None).await;
            return false;
        }
        let Some(admission) = self.admission(datagram.app, datagram.server).await else {
            self.forward(datagram, None).await;
            return false;
        };
        if initial && admission.filter && self.can_refuse() {
            return true;
        }
        self.forward(datagram, web.then(|| admission.owner())).await;
        false
    }

    async fn forward(&mut self, datagram: &Datagram<'_>, owner: Option<Owner>) {
        let now = self.started.elapsed().as_secs();
        let port = match self.open(datagram, owner).await {
            Ok(port) => port,
            // The datagram is dropped, as the network may drop it: the app
            // sends again, and the next one tries a socket anew. Told of
            // once, since every datagram may fail alike.
            Err(error) => {
                if !std::mem::replace(&mut self.told_open_failure, true) {
                    log(&format!("udp: a socket could not be opened: {error}"));
                }
                return;
            }
        };
        port.used.store(now, Relaxed);
        // Not waited for: a socket that cannot take it now drops it.
        if owner.is_some() {
            let _ = port.socket.send(datagram.payload);
        } else if port
            .socket
            .send_to(datagram.payload, datagram.server)
            .is_ok()
        {
            // Ordinary UDP can use one local port with several peers, as
            // in ICE. Keep its external port stable and admit only replies
            // from destinations the app has actually contacted.
            if let Some(i) = port.peers.iter().position(|p| *p == datagram.server) {
                port.peers.remove(i);
            } else if port.peers.len() == MAX_PEERS_PER_PORT {
                port.peers.pop_front();
            }
            port.peers.push_back(datagram.server);
        }
    }

    /// The app port may have changed hands while its reply waited for TUN.
    pub async fn deliver(&mut self, reply: Reply) -> Option<Vec<u8>> {
        if !reply.open.load(Relaxed) {
            return None;
        }
        let packet = parse(&reply.packet)?;
        if let Some(owner) = reply.owner {
            // A server may answer after the app has been quiet for longer
            // than the cache keeps the binding: ask Android again then,
            // rather than lose a reply the app is still waiting for.
            if self.admission(packet.server, packet.app).await?.owner() != owner {
                return None;
            }
        } else if !self
            .ports
            .get(&(packet.server, None))?
            .peers
            .contains(&packet.app)
        {
            return None;
        }
        Some(reply.packet)
    }

    /// How many sockets are held.
    pub fn len(&self) -> usize {
        self.ports.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ports.is_empty()
    }
}

pub type Outbound = Vec<u8>;

/// Android owner lookups run outside the TUN reader. Bound both queues so a
/// busy UDP app or a stalled system service cannot stall TCP or grow memory.
pub async fn relay(
    mut outbound: mpsc::Receiver<Outbound>,
    to_tun: mpsc::Sender<Vec<u8>>,
    policy: Policy,
    protect: Protect,
    mtu: usize,
) {
    let (to_relay, mut replies) = mpsc::channel(256);
    let mut forwarder = Forwarder::new(to_relay, protect, policy, mtu);
    loop {
        tokio::select! {
            sent = outbound.recv() => {
                let Some(packet) = sent else { return };
                if let Some(datagram) = parse(&packet)
                    && forwarder.send(&datagram).await
                    && let Some(error) = port_unreachable(&packet, mtu)
                {
                    let _ = to_tun.try_send(error);
                }
            }
            Some(reply) = replies.recv() => {
                if let Some(packet) = forwarder.deliver(reply).await {
                    let _ = to_tun.try_send(packet);
                }
            }
        }
    }
}

/// Gives the answers that come to a socket back to the app, until the
/// socket has been idle for long enough or was closed to make room.
async fn answers(
    socket: Arc<UdpSocket>,
    used: Arc<AtomicU64>,
    open: Arc<AtomicBool>,
    (app, owner): (SocketAddr, Option<Owner>),
    back: mpsc::Sender<Reply>,
    mtu: usize,
    started: Instant,
) {
    let mut buf = vec![0u8; mtu];
    while open.load(Relaxed) {
        match tokio::time::timeout(IDLE, socket.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => {
                used.store(started.elapsed().as_secs(), Relaxed);
                // An answer longer than a packet may be is dropped: the
                // app asked with one that was not.
                if let Some(packet) = packet(from, app, &buf[..n], mtu) {
                    // Dropped if the apps are not taking what they are
                    // given: nothing waits on a datagram.
                    let _ = back.try_send(Reply {
                        packet,
                        owner,
                        open: open.clone(),
                    });
                }
            }
            Ok(Err(_)) => break,
            Err(_) => {
                let idle = started.elapsed().as_secs() - used.load(Relaxed);
                if idle >= IDLE.as_secs() {
                    break;
                }
            }
        }
    }
    open.store(false, Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    const MTU: usize = 1500;

    #[test]
    fn port_unreachable_quotes_the_original_udp_header_and_has_valid_checksums() {
        let payload = [0; 1200];
        let original = packet(
            "10.0.0.2:60000".parse().unwrap(),
            "192.0.2.1:443".parse().unwrap(),
            &payload,
            1500,
        )
        .unwrap();
        let response = port_unreachable(&original, 1500).unwrap();
        let ip = Ipv4Packet::new_checked(&response).unwrap();
        assert!(ip.verify_checksum());
        assert_eq!(ip.next_header(), IpProtocol::Icmp);
        assert_eq!(ip.src_addr().octets(), [192, 0, 2, 1]);
        assert_eq!(ip.dst_addr().octets(), [10, 0, 0, 2]);
        let icmp = Icmpv4Packet::new_checked(ip.payload()).unwrap();
        assert!(icmp.verify_checksum());
        assert_eq!(&ip.payload()[..2], &[3, 3]);
        assert_eq!(&ip.payload()[8..], &original[..28]);

        let original = packet(
            "[fd00::2]:60000".parse().unwrap(),
            "[2001:db8::1]:14443".parse().unwrap(),
            &payload,
            1500,
        )
        .unwrap();
        let response = port_unreachable(&original, 1500).unwrap();
        let ip = Ipv6Packet::new_checked(&response).unwrap();
        assert_eq!(ip.next_header(), IpProtocol::Icmpv6);
        assert_eq!(ip.src_addr(), "2001:db8::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(ip.dst_addr(), "fd00::2".parse::<Ipv6Addr>().unwrap());
        let icmp = Icmpv6Packet::new_checked(ip.payload()).unwrap();
        assert!(icmp.verify_checksum(&ip.src_addr(), &ip.dst_addr()));
        assert_eq!(&ip.payload()[..2], &[1, 4]);
        assert_eq!(&ip.payload()[8..], &original[..1232]);
        assert_eq!(response.len(), 1280);
    }

    #[tokio::test]
    async fn only_selected_initials_get_refused_on_web_and_alternative_ports() {
        let initial = include_bytes!("../../testdata/quic/aioquic-v2-initial.bin");
        for port in [443, 14443] {
            let (back, _) = mpsc::channel(4);
            let policy = Policy::test(Some(10001));
            let mut forwarder = Forwarder::new(
                back,
                Arc::new(|_| panic!("unexpected upstream")),
                policy,
                1500,
            );
            assert!(
                forwarder
                    .send(&Datagram {
                        app: "10.0.0.2:60000".parse().unwrap(),
                        server: SocketAddr::from(([192, 0, 2, 1], port)),
                        payload: initial,
                    })
                    .await
            );
            assert!(forwarder.is_empty());
        }
    }

    #[tokio::test]
    async fn a_bound_reply_needs_no_lookup_and_only_its_current_owner_receives_it() {
        use std::sync::atomic::AtomicUsize;
        let calls = Arc::new(AtomicUsize::new(0));
        let generation = Arc::new(AtomicU64::new(1));
        let epoch = Arc::new(AtomicU64::new(1));
        let mut policy = Policy::test(None);
        policy.lookup = {
            let (calls, generation, epoch) = (calls.clone(), generation.clone(), epoch.clone());
            Arc::new(move |_, _| {
                calls.fetch_add(1, Relaxed);
                Some(Admission {
                    uid: 10001,
                    epoch: epoch.load(Relaxed),
                    generation: generation.load(Relaxed),
                    filter: true,
                })
            })
        };
        policy.current = {
            let (generation, epoch) = (generation.clone(), epoch.clone());
            Arc::new(move |admission| {
                admission.generation == generation.load(Relaxed)
                    && admission.epoch == epoch.load(Relaxed)
            })
        };
        let (back, _) = mpsc::channel(1);
        let mut forwarder = Forwarder::new(back, Arc::new(|_| false), policy, 1500);
        let app = "10.0.0.2:60000".parse().unwrap();
        let peer = "192.0.2.1:443".parse().unwrap();
        let reply = |owner| Reply {
            packet: packet(peer, app, b"reply", 1500).unwrap(),
            open: Arc::new(AtomicBool::new(true)),
            owner: Some(owner),
        };
        let first = forwarder.admission(app, peer).await.unwrap();
        assert_eq!(forwarder.admission(app, peer).await, Some(first));
        assert!(forwarder.deliver(reply(first.owner())).await.is_some());
        assert_eq!(calls.load(Relaxed), 1);

        // A new selection asks Android again; the same app keeps its replies.
        generation.store(2, Relaxed);
        assert!(forwarder.deliver(reply(first.owner())).await.is_some());
        assert_eq!(calls.load(Relaxed), 2);

        // A recycled UID is a different owner and gets nothing.
        epoch.store(2, Relaxed);
        assert!(forwarder.deliver(reply(first.owner())).await.is_none());
        assert_eq!(calls.load(Relaxed), 3);

        // An expired binding is looked up again instead of losing the reply.
        let current = forwarder.admission(app, peer).await.unwrap();
        forwarder.admissions.get_mut(&(app, peer)).unwrap().expires = Instant::now();
        assert!(forwarder.deliver(reply(current.owner())).await.is_some());
        assert_eq!(calls.load(Relaxed), 4);
    }

    #[tokio::test]
    async fn a_reply_for_an_app_that_lost_its_port_is_discarded() {
        let (back, _) = mpsc::channel(1);
        let mut f = Forwarder::new(back, Arc::new(|_| true), Policy::test(Some(10002)), MTU);
        let packet = packet(
            "192.0.2.1:443".parse().unwrap(),
            "10.0.0.2:1000".parse().unwrap(),
            b"old reply",
            MTU,
        )
        .unwrap();
        assert!(
            f.deliver(Reply {
                packet,
                open: Arc::new(AtomicBool::new(true)),
                owner: Some((10001, 1))
            })
            .await
            .is_none()
        );
    }

    #[tokio::test]
    async fn a_stalled_owner_lookup_expires_while_the_runtime_can_still_make_progress() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let wait = std::sync::Mutex::new(wait);
        let policy = Policy {
            lookup: Arc::new(move |_, _| {
                let _ = wait.lock().unwrap().recv_timeout(Duration::from_secs(3));
                Some(Admission {
                    uid: 10001,
                    epoch: 1,
                    generation: 1,
                    filter: true,
                })
            }),
            current: Arc::new(|_| true),
        };
        let owner = policy
            .admission(
                "10.0.0.2:50000".parse().unwrap(),
                "192.0.2.1:443".parse().unwrap(),
            )
            .await;
        release.send(()).unwrap();
        assert!(owner.is_none());
    }

    /// A forwarder, a server that answers in capitals, and where the
    /// packets for the apps come out.
    async fn forwarder() -> (Forwarder, SocketAddr, mpsc::Receiver<Reply>) {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let at = server.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((n, from)) = server.recv_from(&mut buf).await {
                let _ = server.send_to(&buf[..n].to_ascii_uppercase(), from).await;
            }
        });
        let (back, packets) = mpsc::channel(64);
        (
            Forwarder::new(back, Arc::new(|_| true), Policy::test(Some(10001)), MTU),
            at,
            packets,
        )
    }

    #[tokio::test]
    async fn a_datagram_without_a_socket_is_dropped_and_the_next_tries_again() {
        let (_, server, _) = forwarder().await;
        let protects = Arc::new(AtomicBool::new(false));
        let protected = protects.clone();
        let (back, mut packets) = mpsc::channel(64);
        let protect: Protect = Arc::new(move |_| protected.load(Relaxed));
        let mut forwarder = Forwarder::new(back, protect, Policy::test(Some(10001)), MTU);
        let app: SocketAddr = "10.0.0.2:5353".parse().unwrap();
        let datagram = Datagram {
            app,
            server,
            payload: b"a question",
        };
        for _ in 0..2 {
            assert!(!forwarder.send(&datagram).await);
            assert!(forwarder.is_empty());
            assert!(forwarder.told_open_failure);
        }
        protects.store(true, Relaxed);
        forwarder.send(&datagram).await;
        let bytes = forwarder
            .deliver(packets.recv().await.unwrap())
            .await
            .unwrap();
        assert_eq!(parse(&bytes).unwrap().payload, b"A QUESTION");
    }

    #[tokio::test]
    async fn an_answer_comes_back_to_the_app_as_from_where_it_sent_to() {
        let (mut forwarder, server, mut packets) = forwarder().await;
        let app: SocketAddr = "10.0.0.2:5353".parse().unwrap();
        forwarder
            .send(&Datagram {
                app,
                server,
                payload: b"a question",
            })
            .await;
        let bytes = forwarder
            .deliver(packets.recv().await.unwrap())
            .await
            .unwrap();
        let answer = parse(&bytes).unwrap();
        // As a packet the app would have sent, the ends the other way.
        assert_eq!(answer.app, server);
        assert_eq!(answer.server, app);
        assert_eq!(answer.payload, b"A QUESTION");

        // The same socket for the same port of the app.
        forwarder
            .send(&Datagram {
                app,
                server,
                payload: b"again",
            })
            .await;
        assert_eq!(
            parse(
                &forwarder
                    .deliver(packets.recv().await.unwrap())
                    .await
                    .unwrap()
            )
            .unwrap()
            .payload,
            b"AGAIN"
        );
        assert_eq!(forwarder.len(), 1);
    }

    #[tokio::test]
    async fn ordinary_udp_keeps_its_source_port_and_accepts_only_contacted_peers() {
        let (back, mut replies) = mpsc::channel(4);
        let mut relay = Forwarder::new(back, Arc::new(|_| true), Policy::test(None), MTU);
        let app = "10.0.0.2:45321".parse().unwrap();
        let first = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let second = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut observed = Vec::new();
        for peer in [&first, &second] {
            relay
                .send(&Datagram {
                    app,
                    server: peer.local_addr().unwrap(),
                    payload: b"q",
                })
                .await;
            let (_, from) =
                tokio::time::timeout(Duration::from_secs(1), peer.recv_from(&mut [0; 8]))
                    .await
                    .unwrap()
                    .unwrap();
            observed.push(from);
        }
        assert_eq!(observed[0], observed[1]);
        for (peer, allowed) in [(&stranger, false), (&first, true), (&second, true)] {
            peer.send_to(b"r", observed[0]).await.unwrap();
            let reply = tokio::time::timeout(Duration::from_secs(1), replies.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(relay.deliver(reply).await.is_some(), allowed);
        }
    }

    #[tokio::test]
    async fn no_more_sockets_are_held_than_the_bound() {
        let (mut forwarder, server, mut packets) = forwarder().await;
        for port in 0..MAX_PORTS as u16 + 20 {
            forwarder
                .send(&Datagram {
                    app: SocketAddr::new("10.0.0.2".parse().unwrap(), 10_000 + port),
                    server,
                    payload: b"x",
                })
                .await;
            assert!(forwarder.len() <= MAX_PORTS);
        }
        // More answers came than are held for the apps at a time, and
        // those are dropped. With them out of the way, the newest port
        // still has its socket, and the answer to what it sends next.
        while tokio::time::timeout(Duration::from_millis(200), packets.recv())
            .await
            .is_ok()
        {}
        let newest = SocketAddr::new("10.0.0.2".parse().unwrap(), 10_000 + MAX_PORTS as u16 + 19);
        forwarder
            .send(&Datagram {
                app: newest,
                server,
                payload: b"last",
            })
            .await;
        let bytes = forwarder
            .deliver(packets.recv().await.unwrap())
            .await
            .unwrap();
        let answer = parse(&bytes).unwrap();
        assert_eq!(
            (answer.server, answer.payload),
            (newest, b"LAST".as_slice())
        );
        assert!(forwarder.len() <= MAX_PORTS);
    }

    #[tokio::test]
    async fn a_socket_that_cannot_be_kept_out_of_the_vpn_is_not_used() {
        let (back, mut packets) = mpsc::channel(4);
        let mut forwarder =
            Forwarder::new(back, Arc::new(|_| false), Policy::test(Some(10001)), MTU);
        forwarder
            .send(&Datagram {
                app: "10.0.0.2:5353".parse().unwrap(),
                server: "127.0.0.1:9".parse().unwrap(),
                payload: b"x",
            })
            .await;
        assert!(forwarder.is_empty());
        assert!(packets.try_recv().is_err());
    }
}
