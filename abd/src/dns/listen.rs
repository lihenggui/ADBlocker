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

//! Where TPROXY delivers the lookups, and the system a lookup is served
//! with.

use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::AsFd as _;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use abd_engine::dns;
use abd_engine::mitm::{Ctx, Origin};
use anyhow::Context as _;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::io::Interest;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Notify;
use tokio::task::JoinSet;

use super::anchor::Anchor;
use super::netlink::Netlink;
use super::serve::{self, Host, MAX_DATAGRAM, MAX_QUERY, Slots, Unsent};
use super::sys;
use crate::{capture, sockdiag};

/// How long a blocked answer is valid: not at all. netd keeps answers for
/// every app of a network in one cache, and keeps none valid for 0
/// seconds: a blocked answer reaches no other app, and a pause takes
/// effect at the app's next lookup.
const TTL: u32 = 0;

pub(super) struct Listeners {
    udp: Vec<UdpSocket>,
    tcp: Vec<TcpListener>,
    /// The port of all of them, of the kernel's choosing: a fixed one
    /// could be taken while no daemon runs.
    pub(super) port: u16,
    /// Whether there are listeners for IPv6 too.
    pub(super) v6: bool,
}

/// Listeners for UDP and TCP on one port, on IPv6 too when `v6` and it
/// can be had: lookups over IPv4 are filtered even if those over IPv6
/// cannot be.
pub(super) fn bind(v6: bool) -> anyhow::Result<Listeners> {
    let mut last = None;
    let mut without_v6 = None;
    // The port free for one may be taken for another: then another.
    for _ in 0..8 {
        let mut listeners = match ipv4() {
            Ok(listeners) => listeners,
            Err(error) => {
                last = Some(error);
                continue;
            }
        };
        if !v6 {
            return Ok(listeners);
        }
        match ipv6(listeners.port) {
            Ok((udp, tcp)) => {
                listeners.udp.push(udp);
                listeners.tcp.push(tcp);
                listeners.v6 = true;
                return Ok(listeners);
            }
            Err(error) => {
                last = Some(error);
                without_v6 = Some(listeners);
            }
        }
    }
    let last = last.unwrap_or_else(|| anyhow::anyhow!("no port"));
    let listeners = without_v6.ok_or(last)?;
    eprintln!("dns: no IPv6 listener; lookups over IPv6 are not filtered");
    Ok(listeners)
}

fn ipv4() -> anyhow::Result<Listeners> {
    let udp = udp((Ipv4Addr::LOCALHOST, 0).into())?;
    let port = udp.local_addr()?.port();
    Ok(Listeners {
        udp: vec![udp],
        tcp: vec![capture::listener((Ipv4Addr::LOCALHOST, port).into())?],
        port,
        v6: false,
    })
}

fn ipv6(port: u16) -> anyhow::Result<(UdpSocket, TcpListener)> {
    let udp = udp((Ipv6Addr::LOCALHOST, port).into())?;
    Ok((udp, capture::listener((Ipv6Addr::LOCALHOST, port).into())?))
}

/// A transparent UDP socket that is told where each datagram was going.
fn udp(addr: SocketAddr) -> anyhow::Result<UdpSocket> {
    let s = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    match addr {
        SocketAddr::V4(_) => {
            s.set_ip_transparent_v4(true)?;
            capture::setsockopt(&s, libc::SOL_IP, libc::IP_RECVORIGDSTADDR, 1)?;
        }
        SocketAddr::V6(_) => {
            s.set_only_v6(true)?;
            s.set_ip_transparent_v6(true)?;
            capture::setsockopt(&s, libc::SOL_IPV6, libc::IPV6_RECVORIGDSTADDR, 1)?;
        }
    }
    s.set_nonblocking(true)?;
    s.bind(&addr.into())
        .with_context(|| format!("bind {addr}"))?;
    Ok(UdpSocket::from_std(s.into())?)
}

/// Serves lookups until a listener fails or `stop` is told that none can
/// be answered. `anchor` goes with this: once nothing listens, no route
/// leads here any more.
pub(super) async fn serve(
    host: Arc<dyn Host>,
    stop: Arc<Notify>,
    listeners: Listeners,
    anchor: Anchor,
) {
    let _anchor = anchor;
    let (pending, streams) = (Slots::pending(), Slots::streams());
    let mut tasks = JoinSet::new();
    for socket in listeners.udp {
        tasks.spawn(receive(host.clone(), pending.clone(), socket));
    }
    for listener in listeners.tcp {
        tasks.spawn(accept(host.clone(), streams.clone(), listener));
    }
    let ended = tokio::select! {
        ended = tasks.join_next() => ended,
        // Said by whoever gave up.
        () = stop.notified() => return,
    };
    let why = match ended {
        Some(Ok(Err(error))) => error.to_string(),
        Some(Err(error)) => error.to_string(),
        Some(Ok(Ok(()))) | None => "ended".to_owned(),
    };
    eprintln!("dns: a listener stopped ({why}); lookups are not filtered");
}

/// A descriptor that cannot be read from any more, rather than a
/// datagram or a connection that went wrong.
fn fatal(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EBADF | libc::EINVAL | libc::ENOTSOCK | libc::EFAULT)
    )
}

async fn receive(host: Arc<dyn Host>, pending: Arc<Slots>, socket: UdpSocket) -> io::Result<()> {
    let mut buffer = vec![0; MAX_DATAGRAM];
    loop {
        socket.readable().await?;
        let received = socket.try_io(Interest::READABLE, || {
            sys::receive(socket.as_fd(), &mut buffer)
        });
        match received {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Ok(got) => {
                let Some(server) = got.to.filter(|&to| serve::by_our_route(got.from, to)) else {
                    continue;
                };
                // Too long to be a lookup, or too many waiting: this one
                // is dropped, and the app's resolver asks again.
                if got.len > MAX_QUERY {
                    continue;
                }
                let Some(slot) = pending.take() else {
                    continue;
                };
                let query = buffer[..got.len].to_vec();
                let (host, pending) = (host.clone(), pending.clone());
                tokio::spawn(async move {
                    let _slot = slot;
                    serve::datagram(host, pending, query, got.from, server).await;
                });
            }
            Err(error) if fatal(&error) => return Err(error),
            // That datagram is lost; the next is read.
            Err(_) => {}
        }
    }
}

async fn accept(host: Arc<dyn Host>, streams: Arc<Slots>, listener: TcpListener) -> io::Result<()> {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                // Too many: closed at once, and the app's resolver goes on.
                let Some(slot) = streams.take() else {
                    continue;
                };
                let mark = capture::mark_of(&stream);
                let (host, streams) = (host.clone(), streams.clone());
                tokio::spawn(async move {
                    let _slot = slot;
                    serve::stream(host, streams, stream, mark).await;
                });
            }
            Err(error) if fatal(&error) => return Err(error),
            // Out of descriptors, most likely: let some connections end.
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

/// The system a lookup is served with.
pub(super) struct System {
    ctx: Arc<Ctx>,
    /// Kept open: a socket for every lookup would be calls more for each.
    routes: Mutex<Option<Netlink>>,
    stop: Arc<Notify>,
    gave_up: AtomicBool,
}

impl System {
    pub(super) fn new(ctx: Arc<Ctx>, stop: Arc<Notify>) -> Self {
        Self {
            ctx,
            routes: Mutex::new(None),
            stop,
            gave_up: AtomicBool::new(false),
        }
    }

    /// A socket that sends as `server`. Binding it fails while another
    /// program holds the server's port on every address without letting
    /// others share it.
    fn as_server(server: SocketAddr) -> io::Result<Socket> {
        let s = Socket::new(
            Domain::for_address(server),
            Type::DGRAM,
            Some(Protocol::UDP),
        )?;
        // Others may be answering for the same server at the same time.
        s.set_reuse_address(true)?;
        match server {
            SocketAddr::V4(_) => s.set_ip_transparent_v4(true)?,
            SocketAddr::V6(_) => {
                s.set_only_v6(true)?;
                s.set_ip_transparent_v6(true)?;
            }
        }
        s.set_nonblocking(true)?;
        s.bind(&server.into())?;
        Ok(s)
    }
}

impl Host for System {
    fn blocked(&self, uid: u32, query: &[u8]) -> Option<Vec<u8>> {
        let question = dns::Question::parse(query, false)?;
        let answer = dns::blocked_answer(&self.ctx, uid, &question, TTL)?;
        if self.ctx.hooks.verbose {
            let host = question.host().unwrap_or_default();
            eprintln!("dns     {uid} blocked {host}");
        }
        Some(answer)
    }

    fn udp_owner(&self, client: SocketAddr, server: SocketAddr) -> Option<(u32, Option<u32>)> {
        sockdiag::udp_owner(client, server)
    }

    fn tcp_owner(&self, client: SocketAddr, server: SocketAddr) -> Option<u32> {
        sockdiag::uid_of(client, server)
    }

    fn is_local(&self, ip: IpAddr) -> bool {
        let mut routes = self
            .routes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let asked = match &mut *routes {
            Some(netlink) => netlink.is_local(ip),
            None => Netlink::open().and_then(|netlink| routes.insert(netlink).is_local(ip)),
        };
        // A socket something went wrong with is not asked again, and what
        // could not be found out is taken for a server elsewhere.
        asked.unwrap_or_else(|_| {
            *routes = None;
            false
        })
    }

    fn udp(&self, origin: &Origin) -> io::Result<UdpSocket> {
        capture::udp_as(origin.dst(), origin.mark(), origin.uid())
    }

    fn tcp(&self, origin: Origin) -> Pin<Box<dyn Future<Output = io::Result<TcpStream>> + Send>> {
        Box::pin(capture::connect_as(
            origin.dst(),
            origin.mark(),
            origin.uid(),
        ))
    }

    fn reply(&self, server: SocketAddr, client: SocketAddr, answer: &[u8]) -> Result<(), Unsent> {
        let s = Self::as_server(server).map_err(Unsent::Never)?;
        s.send_to(answer, &client.into()).map_err(Unsent::Lost)?;
        Ok(())
    }

    fn give_up(&self, why: &io::Error) {
        if !self.gave_up.swap(true, Relaxed) {
            eprintln!("dns: cannot answer as a lookup's server ({why}); lookups are not filtered");
            self.stop.notify_one();
        }
    }

    fn verbose(&self) -> bool {
        self.ctx.hooks.verbose
    }
}
