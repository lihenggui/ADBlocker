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

//! Resolve only queries addressed to the VPN's virtual DNS endpoints.

mod upstream;

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use abd_engine::dns::{self, HEADER, Question};
use abd_engine::mitm::{Ctx, Origin};
use tokio::net::TcpStream;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::datagram;
use crate::platform::{Connect, Platform, Protect, lookup_owner};
use upstream::UdpResolver;

const VIRTUAL_V4: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 120, 38, 2));
const VIRTUAL_V6: IpAddr = IpAddr::V6(Ipv6Addr::new(0xfd8b, 0x5f3a, 0x9c21, 0, 0, 0, 0, 2));

const DNS: u16 = 53;
const DOT: u16 = 853;
pub(crate) const MAX_PENDING: usize = 64;
const RESOLVER_TIME: Duration = Duration::from_secs(1);
const TCP_LIFETIME: Duration = Duration::from_secs(30);
const MAX_TCP_CLIENTS: usize = 32;
const BLOCKED_TTL: u32 = 0;
const IPV4_UDP_HEADERS: usize = 20 + 8;
const IPV6_UDP_HEADERS: usize = 40 + 8;

pub(crate) struct Resolver {
    upstream: Vec<UdpResolver>,
    ctx: Arc<Ctx>,
    platform: Arc<dyn Platform>,
    connect: Connect,
    protect: Protect,
}

impl Resolver {
    pub(crate) fn new(
        upstream: Vec<SocketAddr>,
        ctx: Arc<Ctx>,
        platform: Arc<dyn Platform>,
        connect: Connect,
        protect: Protect,
    ) -> Option<Arc<Self>> {
        let mut servers = Vec::new();
        for addr in upstream {
            if addr.port() == DNS
                && !Self::virtual_address(addr)
                && !addr.ip().is_unspecified()
                && !addr.ip().is_multicast()
                && !servers.contains(&addr)
            {
                servers.push(addr);
            }
        }
        (!servers.is_empty()).then(|| {
            Arc::new(Self {
                upstream: servers.into_iter().map(UdpResolver::new).collect(),
                ctx,
                platform,
                connect,
                protect,
            })
        })
    }

    fn virtual_address(addr: SocketAddr) -> bool {
        matches!(addr.ip(), VIRTUAL_V4 | VIRTUAL_V6)
    }

    pub(crate) fn answers(addr: SocketAddr) -> bool {
        addr.port() == DNS && Self::virtual_address(addr)
    }

    pub(crate) fn refuses(addr: SocketAddr) -> bool {
        addr.port() == DOT && Self::virtual_address(addr)
    }

    async fn owner(&self, protocol: i32, app: SocketAddr, server: SocketAddr) -> Option<u32> {
        static LOOKUPS: Semaphore = Semaphore::const_new(8);
        let platform = self.platform.clone();
        let uid = lookup_owner(&LOOKUPS, move || platform.owner(protocol, app, server)).await;
        Origin::of(server, 0, uid).map(|origin| origin.uid())
    }

    async fn upstream_tcp(
        &self,
        target: SocketAddr,
        request: &[u8],
        matches: impl Fn(&[u8]) -> bool,
    ) -> Option<Vec<u8>> {
        let mut socket = (self.connect)(target).await.ok()?;
        dns::write_frame(&mut socket, request).await.ok()?;
        let answer = frame(&mut socket).await.ok()?;
        matches(&answer).then_some(answer)
    }

    /// Tries each resolver in turn with the same request.
    async fn forward(
        &self,
        request: &[u8],
        udp_mtu: Option<usize>,
        question: &Question<'_>,
    ) -> Option<Vec<u8>> {
        for target in &self.upstream {
            let forwarded = timeout(RESOLVER_TIME, async {
                if let Some(mtu) = udp_mtu {
                    target
                        .query(self.protect.clone(), request, question, mtu)
                        .await
                } else {
                    self.upstream_tcp(target.address, request, |answer| question.matches(answer))
                        .await
                }
            })
            .await;
            if let Ok(Some(answer)) = forwarded {
                return Some(answer);
            }
        }
        None
    }

    async fn answer(
        &self,
        app: SocketAddr,
        server: SocketAddr,
        request: &[u8],
        udp_payload: Option<usize>,
    ) -> Option<Vec<u8>> {
        let Some(question) = Question::parse(request, false) else {
            // What cannot be judged is refused at once: forwarding it would
            // let a crafted message carry a blocked name past the rules,
            // and silence would only make the app wait for its timeout.
            // Android's own resolver never sends such a message.
            return dns::refusal(request);
        };
        let protocol = if udp_payload.is_some() {
            libc::IPPROTO_UDP
        } else {
            libc::IPPROTO_TCP
        };
        let filtered_uid = self
            .owner(protocol, app, server)
            .await
            .filter(|uid| self.ctx.selection.apps.read().unwrap().contains_key(uid));
        if let Some(uid) = filtered_uid
            && let Some(answer) = dns::blocked_answer(&self.ctx, uid, &question, BLOCKED_TTL)
        {
            return Some(answer);
        }
        let headers = if app.is_ipv4() {
            IPV4_UDP_HEADERS
        } else {
            IPV6_UDP_HEADERS
        };
        let answer = self
            .forward(
                request,
                udp_payload.map(|payload| payload + headers),
                &question,
            )
            .await?;
        // A resolver can advertise a larger datagram than the TUN can
        // carry. Signal truncation so the client can retry using TCP.
        Some(if udp_payload.is_some_and(|max| answer.len() > max) {
            question.local_answer(BLOCKED_TTL, true)
        } else {
            answer
        })
    }

    pub(crate) async fn udp(
        self: Arc<Self>,
        mut queries: mpsc::Receiver<Vec<u8>>,
        replies: mpsc::Sender<Vec<u8>>,
        mtu: usize,
    ) {
        let mut pending = JoinSet::new();
        loop {
            tokio::select! {
                Some(_) = pending.join_next(), if !pending.is_empty() => {}
                next = queries.recv(), if pending.len() < MAX_PENDING => {
                    let Some(packet) = next else { return };
                    let (resolver, replies) = (self.clone(), replies.clone());
                    pending.spawn(async move {
                        let Some(query) = datagram::parse(&packet) else { return };
                        let headers = if query.app.is_ipv4() { IPV4_UDP_HEADERS } else { IPV6_UDP_HEADERS };
                        if let Some(answer) = resolver.answer(
                            query.app, query.server, query.payload, Some(mtu.saturating_sub(headers)),
                        ).await
                            && let Some(reply) = datagram::packet(query.server, query.app, &answer, mtu)
                        {
                            let _ = replies.try_send(reply);
                        }
                    });
                }
            }
        }
    }

    pub(crate) async fn tcp(&self, mut stream: TcpStream, app: SocketAddr, server: SocketAddr) {
        static TCP_CLIENTS: Semaphore = Semaphore::const_new(MAX_TCP_CLIENTS);
        let Ok(_slot) = TCP_CLIENTS.try_acquire() else {
            return;
        };
        let _ = timeout(TCP_LIFETIME, async {
            while let Ok(request) = frame(&mut stream).await {
                let Some(answer) = self.answer(app, server, &request, None).await else {
                    break;
                };
                if dns::write_frame(&mut stream, &answer).await.is_err() {
                    break;
                }
            }
        })
        .await;
    }
}

/// A stream message of at least a header: anything shorter ends the
/// connection.
async fn frame(socket: &mut TcpStream) -> io::Result<Vec<u8>> {
    let bytes = dns::read_frame(socket).await?;
    if bytes.len() < HEADER {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_shared_dns_socket_matches_both_id_and_question_and_keeps_the_client_ids() {
        use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
        use tokio::net::UdpSocket;

        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let upstream = UdpResolver::new(server.local_addr().unwrap());
        let protects = Arc::new(AtomicUsize::new(0));
        let calls = protects.clone();
        let protect: Protect = Arc::new(move |_| {
            calls.fetch_add(1, Relaxed);
            true
        });
        let first = query(1);
        let second = query(28);
        let first_question = Question::parse(&first, false).unwrap();
        let second_question = Question::parse(&second, false).unwrap();
        let serve = async {
            let mut buffer = [0; 1500];
            let (n, first_peer) = server.recv_from(&mut buffer).await.unwrap();
            let mut one = buffer[..n].to_vec();
            let (n, second_peer) = server.recv_from(&mut buffer).await.unwrap();
            let mut two = buffer[..n].to_vec();
            assert_eq!(first_peer, second_peer);
            assert_ne!(one[..2], two[..2]);
            one[2] |= 0x80;
            two[2] |= 0x80;

            // Matching a transaction number alone could misdeliver this
            // reply and consume the slot for the real answer.
            let mut wrong = one.clone();
            wrong[HEADER + 1] = b'x';
            server.send_to(&wrong, first_peer).await.unwrap();
            server.send_to(&two, first_peer).await.unwrap();
            server.send_to(&one, first_peer).await.unwrap();
        };
        let (one, two, ()) = timeout(RESOLVER_TIME, async {
            tokio::join!(
                upstream.query(protect.clone(), &first, &first_question, 1500),
                upstream.query(protect.clone(), &second, &second_question, 1500),
                serve,
            )
        })
        .await
        .unwrap();
        assert!(first_question.matches(&one.unwrap()));
        assert!(second_question.matches(&two.unwrap()));
        assert_eq!(protects.load(Relaxed), 1);
    }

    #[tokio::test]
    async fn virtual_dns_uses_host_rules_even_when_an_address_rule_blocks_its_endpoint() {
        use std::os::fd::RawFd;
        use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
        use tokio::net::UdpSocket;

        struct Device(AtomicUsize);
        impl Platform for Device {
            fn owner(&self, _: i32, _: SocketAddr, _: SocketAddr) -> Option<u32> {
                Some(10001)
            }
            fn uid_of(&self, _: &str) -> Option<u32> {
                Some(10001)
            }
            fn ca_installed(&self) -> bool {
                false
            }
            fn protect(&self, _: RawFd) -> bool {
                self.0.fetch_add(1, Relaxed);
                false
            }
        }

        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let paths = abd_engine::paths::Paths::new(dir.path());
        std::fs::create_dir_all(paths.rules()).unwrap();
        std::fs::write(
            paths.rules().join("list.txt"),
            "||ads.example^\n10.120.38.2:53$network\n",
        )
        .unwrap();
        std::fs::write(paths.apps(), "com.app\n").unwrap();
        let device = Arc::new(Device(AtomicUsize::new(0)));
        let engine =
            crate::engine::Engine::open(dir.path(), rustls::RootCertStore::empty(), device.clone())
                .unwrap();
        let mut resolver = engine.dns(vec!["192.0.2.53:53".parse().unwrap()]).unwrap();
        Arc::get_mut(&mut resolver).unwrap().upstream =
            vec![UdpResolver::new(server.local_addr().unwrap())];

        let (mut client, tun) = crate::fake::app_and_channels();
        let mtu = crate::fake::MTU;
        let tunnel = tokio::spawn(crate::tunnel::run(
            tun,
            crate::tunnel::Config {
                mtu,
                handler: engine.handler(),
                reach: Some(engine.reach()),
                protect: engine.protect(),
                policy: engine.udp_policy(),
                dns: Some(resolver),
            },
        ));
        let request = query(1);
        let app = "10.120.38.1:30000".parse().unwrap();
        let destination = SocketAddr::new(VIRTUAL_V4, DNS);
        client
            .send(datagram::packet(app, destination, &request, mtu).unwrap())
            .await;
        let reply = client.given().await.unwrap();
        let reply = datagram::parse(&reply).expect("DNS answer over UDP");
        assert_eq!(reply.app, destination);
        assert_eq!(reply.server, app);
        let local = Question::parse(&request, false)
            .unwrap()
            .local_answer(BLOCKED_TTL, false);
        assert_eq!(reply.payload, local);
        assert_eq!(device.0.load(Relaxed), 0);
        tunnel.abort();
    }

    fn query(kind: u16) -> Vec<u8> {
        let mut bytes = b"\x12\x34\x01\0\0\x01\0\0\0\0\0\0\x03AdS\x07example\0".to_vec();
        bytes.extend_from_slice(&kind.to_be_bytes());
        bytes.extend_from_slice(&[0, 1]);
        bytes
    }

    #[tokio::test]
    async fn only_an_active_attributed_query_is_blocked_and_upstream_failure_tries_the_next_server()
    {
        use std::os::fd::RawFd;
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

        struct Device(AtomicU32);
        impl Platform for Device {
            fn owner(&self, _: i32, _: SocketAddr, _: SocketAddr) -> Option<u32> {
                Some(self.0.load(Relaxed))
            }
            fn uid_of(&self, package: &str) -> Option<u32> {
                (package == "com.app").then_some(10001)
            }
            fn ca_installed(&self) -> bool {
                false
            }
            fn protect(&self, _: RawFd) -> bool {
                true
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let paths = abd_engine::paths::Paths::new(dir.path());
        std::fs::create_dir_all(paths.rules()).unwrap();
        std::fs::write(paths.rules().join("list.txt"), "||ads.example^\n").unwrap();
        std::fs::write(paths.apps(), "com.app\n").unwrap();
        let lookups = Arc::new(Mutex::new(Vec::new()));
        let connect: Connect = {
            let lookups = lookups.clone();
            Arc::new(move |addr| {
                lookups.lock().unwrap().push(addr);
                Box::pin(async { Err(io::ErrorKind::ConnectionRefused.into()) })
            })
        };
        let device = Arc::new(Device(AtomicU32::new(10001)));
        let engine = crate::engine::Engine::open_with(
            dir.path(),
            rustls::RootCertStore::empty(),
            device.clone(),
            connect,
        )
        .unwrap();
        let upstream: Vec<SocketAddr> = ["192.0.2.53:53", "[2001:db8::53]:53"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        let dns = engine.dns(upstream.clone()).unwrap();
        let app = "10.120.38.1:30000".parse().unwrap();
        let server = SocketAddr::new(VIRTUAL_V4, DNS);
        let query = query(1);

        let blocked = dns.answer(app, server, &query, None).await.unwrap();
        assert_eq!(blocked[7], 1);
        assert_eq!(&blocked[blocked.len() - 10..blocked.len() - 6], &[0; 4]);
        assert_eq!(&blocked[blocked.len() - 4..], &[0; 4]);
        assert!(lookups.lock().unwrap().is_empty());

        for (uid, trust) in [(10002, false), (10001, true)] {
            device.0.store(uid, Relaxed);
            if trust {
                abd_engine::allow::Allow::edit(&paths.allow(), "ads.example", true).unwrap();
                engine.reload();
            }
            assert!(dns.answer(app, server, &query, None).await.is_none());
            assert_eq!(std::mem::take(&mut *lookups.lock().unwrap()), upstream);
        }

        // A message that cannot be judged is refused at once, not forwarded:
        // a second question must not carry a blocked name upstream.
        let mut two = query.clone();
        two[5] = 2;
        let refused = dns.answer(app, server, &two, None).await.unwrap();
        assert_eq!(refused.len(), HEADER);
        assert_eq!(&refused[..2], &two[..2]);
        assert_eq!(refused[2] & 0x80, 0x80);
        assert_eq!(refused[3] & 0xf, 1);
        let mut notify = query.clone();
        notify[2] |= 0x20;
        let refused = dns.answer(app, server, &notify, None).await.unwrap();
        assert_eq!(refused[3] & 0xf, 4);
        assert!(lookups.lock().unwrap().is_empty());
        assert!(
            dns.answer(app, server, &two[..HEADER - 1], None)
                .await
                .is_none()
        );
        assert!(lookups.lock().unwrap().is_empty());
    }
}
