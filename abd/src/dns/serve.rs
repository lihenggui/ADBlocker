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

//! A lookup brought here: whose it is, and its answer. One the rules
//! block for an active app is answered here; any other is sent on as the
//! app, unchanged, and its answer relayed unchanged. Only what cannot be
//! sent on as the app it came from is dropped, and nothing is ever
//! blocked that was not judged.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use abd_engine::dns;
use abd_engine::mitm::Origin;
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::timeout;

use super::{DNS_PORT, SOURCE_V4, SOURCE_V6};

/// The longest query taken. A query asks one question; one this long is
/// not a lookup an app makes.
pub(super) const MAX_QUERY: usize = 4096;
/// The longest datagram there is: an answer can be as long.
pub(super) const MAX_DATAGRAM: usize = 65535;
/// Lookups over UDP waiting for their answer, overall and for one UID.
const MAX_PENDING: usize = 256;
const MAX_PENDING_PER_UID: usize = 64;
/// Connections over TCP, overall and for one UID.
const MAX_STREAMS: usize = 32;
const MAX_STREAMS_PER_UID: usize = 8;
/// How long a server may take: to accept a connection, to answer, or to
/// take an answer of ours. Android's resolver waits no longer itself.
const UPSTREAM_TIME: Duration = Duration::from_secs(5);
/// How long a connection may wait for its next query, and last at most.
const IDLE_TIME: Duration = Duration::from_secs(10);
const STREAM_TIME: Duration = Duration::from_secs(60);
/// Datagrams from the server that answer something else, before giving up.
const MAX_STRAYS: usize = 8;

/// What serving a lookup needs from the system: the engine's judgement,
/// and the kernel's sockets and answers. The tests have one of their own.
pub(super) trait Host: Send + Sync + 'static {
    /// The answer to `query` of the app of `uid` if the rules block it.
    fn blocked(&self, uid: u32, query: &[u8]) -> Option<Vec<u8>>;
    /// The UID and fwmark of the UDP socket at `client` sending to `server`.
    fn udp_owner(&self, client: SocketAddr, server: SocketAddr) -> Option<(u32, Option<u32>)>;
    fn tcp_owner(&self, client: SocketAddr, server: SocketAddr) -> Option<u32>;
    /// Whether `ip` is this device's own: never a server that is sent to.
    fn is_local(&self, ip: IpAddr) -> bool;
    fn udp(&self, origin: &Origin) -> io::Result<UdpSocket>;
    fn tcp(&self, origin: Origin) -> Pin<Box<dyn Future<Output = io::Result<TcpStream>> + Send>>;
    /// Sends `answer` to `client` from `server`, as the server.
    fn reply(&self, server: SocketAddr, client: SocketAddr, answer: &[u8]) -> Result<(), Unsent>;
    /// Stops bringing lookups here: none can be answered.
    fn give_up(&self, why: &io::Error);
    fn verbose(&self) -> bool;
}

/// Why an answer did not reach the app.
pub(super) enum Unsent {
    /// Nothing can send as the server: no answer can reach an app.
    Never(io::Error),
    /// This answer was lost.
    Lost(io::Error),
}

/// Places taken by what is in progress, overall and by UID: one app
/// cannot take what is another's.
pub(super) struct Slots {
    total: usize,
    per_uid: usize,
    taken: Mutex<(usize, HashMap<u32, usize>)>,
}

/// A place, given back when dropped.
pub(super) struct Slot {
    slots: Arc<Slots>,
    uid: Option<u32>,
}

impl Slots {
    pub(super) fn new(total: usize, per_uid: usize) -> Arc<Self> {
        Arc::new(Self {
            total,
            per_uid,
            taken: Mutex::default(),
        })
    }

    pub(super) fn pending() -> Arc<Self> {
        Self::new(MAX_PENDING, MAX_PENDING_PER_UID)
    }

    pub(super) fn streams() -> Arc<Self> {
        Self::new(MAX_STREAMS, MAX_STREAMS_PER_UID)
    }

    /// One of all the places.
    pub(super) fn take(self: &Arc<Self>) -> Option<Slot> {
        let mut taken = self.taken.lock().unwrap();
        (taken.0 < self.total).then(|| {
            taken.0 += 1;
            Slot {
                slots: self.clone(),
                uid: None,
            }
        })
    }

    /// One of the places of `uid`, as well.
    pub(super) fn take_for(self: &Arc<Self>, uid: u32) -> Option<Slot> {
        let mut taken = self.taken.lock().unwrap();
        let count = taken.1.entry(uid).or_default();
        (*count < self.per_uid).then(|| {
            *count += 1;
            Slot {
                slots: self.clone(),
                uid: Some(uid),
            }
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut taken = self.slots.taken.lock().unwrap();
        match self.uid {
            None => taken.0 -= 1,
            Some(uid) => {
                if let Some(count) = taken.1.get_mut(&uid) {
                    *count -= 1;
                    if *count == 0 {
                        taken.1.remove(&uid);
                    }
                }
            }
        }
    }
}

/// Whether a lookup from `client` to `server` came by our route: from the
/// source the route gives, to port 53 of somewhere that is not this
/// device. Anything can send to the listener; only this is served.
pub(super) fn by_our_route(client: SocketAddr, server: SocketAddr) -> bool {
    let source = match client.ip() {
        IpAddr::V4(ip) => ip == SOURCE_V4,
        IpAddr::V6(ip) => ip == SOURCE_V6,
    };
    let elsewhere = match server.ip() {
        IpAddr::V4(ip) => {
            !(ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast())
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || ip == SOURCE_V6)
        }
    };
    source && elsewhere && server.port() == DNS_PORT
}

/// The app a lookup is for, if it can be sent on as the app's: a real
/// app's, to somewhere that is not this device.
fn origin_of(host: &dyn Host, server: SocketAddr, mark: u32, uid: Option<u32>) -> Option<Origin> {
    let origin = Origin::of(server, mark, uid)?;
    (!host.is_local(server.ip())).then_some(origin)
}

/// One lookup over UDP, from `client` to `server`: answered, or dropped.
pub(super) async fn datagram(
    host: Arc<dyn Host>,
    pending: Arc<Slots>,
    query: Vec<u8>,
    client: SocketAddr,
    server: SocketAddr,
) {
    let Some((uid, mark)) = host.udp_owner(client, server) else {
        return;
    };
    let Some(origin) = origin_of(&*host, server, mark.unwrap_or(0), Some(uid)) else {
        return;
    };
    let Some(_slot) = pending.take_for(uid) else {
        return;
    };
    let answer = match host.blocked(uid, &query) {
        Some(answer) => answer,
        None => match forward(&*host, &origin, &query).await {
            Ok(answer) => answer,
            Err(error) => {
                if host.verbose() {
                    eprintln!("dns     {uid} {server}: {error}");
                }
                return;
            }
        },
    };
    match host.reply(server, client, &answer) {
        Ok(()) => {}
        Err(Unsent::Never(error)) => host.give_up(&error),
        Err(Unsent::Lost(error)) => {
            if host.verbose() {
                eprintln!("dns     reply to {client}: {error}");
            }
        }
    }
}

async fn forward(host: &dyn Host, origin: &Origin, query: &[u8]) -> io::Result<Vec<u8>> {
    let socket = host.udp(origin)?;
    timeout(UPSTREAM_TIME, exchange(&socket, query))
        .await
        .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
}

/// Sends `query` and waits for its answer, passing over any datagram
/// that does not answer it.
pub(super) async fn exchange(socket: &UdpSocket, query: &[u8]) -> io::Result<Vec<u8>> {
    socket.send(query).await?;
    let mut buffer = Vec::with_capacity(MAX_DATAGRAM);
    for _ in 0..MAX_STRAYS {
        socket.recv_buf(&mut buffer).await?;
        if dns::is_answer_to(query, &buffer) {
            return Ok(buffer);
        }
        buffer.clear();
    }
    Err(io::ErrorKind::InvalidData.into())
}

/// A connection over TCP, its queries answered in turn until it ends,
/// stays idle, or has lasted long enough.
pub(super) async fn stream(host: Arc<dyn Host>, streams: Arc<Slots>, app: TcpStream, mark: u32) {
    let (Ok(server), Ok(client)) = (app.local_addr(), app.peer_addr()) else {
        return;
    };
    if !by_our_route(client, server) {
        return;
    }
    let uid = host.tcp_owner(client, server);
    let Some(origin) = origin_of(&*host, server, mark, uid) else {
        return;
    };
    let Some(_own) = streams.take_for(origin.uid()) else {
        return;
    };
    let conversed = timeout(STREAM_TIME, converse(&*host, &origin, app)).await;
    if let Ok(Err(error)) = conversed
        && host.verbose()
        && error.kind() != io::ErrorKind::UnexpectedEof
    {
        eprintln!("dns     {} {server} over TCP: {error}", origin.uid());
    }
}

async fn converse(host: &dyn Host, origin: &Origin, mut app: TcpStream) -> io::Result<()> {
    let mut server = None;
    loop {
        let query = timeout(IDLE_TIME, dns::read_frame(&mut app))
            .await
            .map_err(|_| io::ErrorKind::TimedOut)??;
        if query.len() > MAX_QUERY {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let answer = match host.blocked(origin.uid(), &query) {
            Some(answer) => answer,
            None => timeout(UPSTREAM_TIME, ask(host, origin, &mut server, &query))
                .await
                .map_err(|_| io::ErrorKind::TimedOut)??,
        };
        timeout(UPSTREAM_TIME, dns::write_frame(&mut app, &answer))
            .await
            .map_err(|_| io::ErrorKind::TimedOut)??;
    }
}

/// Asks the server over the connection of this one, made when first needed.
async fn ask(
    host: &dyn Host,
    origin: &Origin,
    server: &mut Option<TcpStream>,
    query: &[u8],
) -> io::Result<Vec<u8>> {
    let connection = match server {
        Some(connection) => connection,
        None => server.insert(host.tcp(*origin).await?),
    };
    dns::write_frame(connection, query).await?;
    let answer = dns::read_frame(connection).await?;
    if dns::is_answer_to(query, &answer) {
        Ok(answer)
    } else {
        Err(io::ErrorKind::InvalidData.into())
    }
}

#[cfg(test)]
mod tests;
