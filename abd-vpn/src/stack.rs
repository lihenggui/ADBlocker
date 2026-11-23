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

//! The apps' packets, made into connections.
//!
//! Without root there is no kernel to do this for us: what the system
//! hands over is each packet the chosen apps send, and what it takes back
//! is packets for them. Here every server an app connects to is answered
//! in its place, so that the app's connection is one of ours to pass on,
//! as the module's are when the packet filter delivers them.
//!
//! Nothing here reads or writes anything: packets are given and taken, and
//! the bytes of a connection read and written, by whoever drives it
//! (`tunnel/`). That is also what lets it be tested with no device.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::time::Duration;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{
    HardwareAddress, IpAddress, IpCidr, IpListenEndpoint, IpProtocol, Ipv4Address, Ipv4Packet,
    Ipv6Address, Ipv6Packet, TcpPacket,
};
use tokio::time::Instant as TokioInstant;

/// The app's address and the server's, which name a connection.
pub(crate) type Ends = (SocketAddr, SocketAddr);

/// The most connections held at a time, for all the apps together: each
/// has its two buffers, and this many are 32 MiB. An app is held to far
/// fewer by the engine (`limits.rs`), once its connection is known to be
/// its own; this is what cannot be exceeded before that.
const MAX_FLOWS: usize = 512;

/// What is held of a connection in each direction until it is passed on.
const BUFFER: usize = 32 << 10;

/// The most packets waiting to be taken back by the system. A connection
/// sends no more than the other end has room for, so this is reached only
/// by answers to packets that begin nothing; those are then dropped, and
/// asked for again by whoever wants them.
const MAX_QUEUE: usize = 1024;

/// How long a connection may give no sign of its other end before it is
/// taken for gone, and how often it is asked for one while it says
/// nothing.
// The other end is local. Probe half as often while keeping the same
// three-interval limit and the separate hard bound on connections.
const ASK_EVERY: Duration = Duration::from_secs(120);
const SILENCE: Duration = Duration::from_secs(360);
const PACKET_SPARES: usize = 32;

/// The packets in and out, as the interface below sees a device.
#[derive(Default)]
struct Queues {
    from_apps: Packets,
    to_apps: Packets,
    mtu: usize,
}

#[derive(Default)]
struct Packets {
    ready: VecDeque<Vec<u8>>,
    spare: Vec<Vec<u8>>,
}

impl Packets {
    fn buffer(&mut self) -> Vec<u8> {
        self.spare.pop().unwrap_or_default()
    }

    fn recycle(&mut self, mut packet: Vec<u8>) {
        if self.spare.len() < PACKET_SPARES {
            packet.clear();
            self.spare.push(packet);
        }
    }

    fn copy(&mut self, packet: &[u8]) {
        if self.ready.len() < MAX_QUEUE {
            let mut buffer = self.buffer();
            buffer.extend_from_slice(packet);
            self.ready.push_back(buffer);
        }
    }
}

struct Rx<'a>(Vec<u8>, &'a mut Packets);
struct Tx<'a>(&'a mut Packets);

impl phy::RxToken for Rx<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        let result = f(&self.0);
        self.1.recycle(self.0);
        result
    }
}

impl phy::TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut packet = self.0.buffer();
        packet.resize(len, 0);
        let r = f(&mut packet);
        if self.0.ready.len() < MAX_QUEUE {
            self.0.ready.push_back(packet);
        } else {
            self.0.recycle(packet);
        }
        r
    }
}

impl phy::Device for Queues {
    type RxToken<'a> = Rx<'a>;
    type TxToken<'a> = Tx<'a>;

    fn receive(&mut self, _: Instant) -> Option<(Rx<'_>, Tx<'_>)> {
        let packet = self.from_apps.ready.pop_front()?;
        Some((Rx(packet, &mut self.from_apps), Tx(&mut self.to_apps)))
    }

    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        Some(Tx(&mut self.to_apps))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps
    }
}

/// A connection of an app, by a number that is never given twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Id(u64);

/// A connection an app has made: from where, and to which server.
#[derive(Debug, PartialEq, Eq)]
pub struct Opened {
    pub id: Id,
    pub app: SocketAddr,
    pub server: SocketAddr,
}

/// What reading from a connection gives.
#[derive(Debug, PartialEq, Eq)]
pub enum Read {
    /// This many bytes.
    Data(usize),
    /// Nothing now; more may come.
    Wait,
    /// The app has sent all it will, or the connection is no more.
    End,
}

struct Flow {
    handle: SocketHandle,
    ends: Ends,
    /// Whether it has been told of as opened.
    told: bool,
}

pub struct Stack {
    // TCP timers and tunnel wakeups use the same monotonic clock.
    started: TokioInstant,
    iface: Interface,
    queues: Queues,
    sockets: SocketSet<'static>,
    flows: HashMap<Id, Flow>,
    by_ends: HashMap<Ends, Id>,
    opened: VecDeque<Opened>,
    /// At most MAX_FLOWS live IDs, plus removals until the next pump.
    changed: HashSet<Id>,
    all: bool,
    next: u64,
}

/// The two ends of the connection a TCP packet is of, and whether it
/// begins one.
fn tcp_ends(packet: &[u8]) -> Option<(Ends, bool)> {
    let (src, dst, payload): (IpAddress, IpAddress, &[u8]) = match packet.first()? >> 4 {
        4 => {
            let ip = Ipv4Packet::new_checked(packet).ok()?;
            (ip.next_header() == IpProtocol::Tcp).then_some(())?;
            (ip.src_addr().into(), ip.dst_addr().into(), ip.payload())
        }
        6 => {
            let ip = Ipv6Packet::new_checked(packet).ok()?;
            (ip.next_header() == IpProtocol::Tcp).then_some(())?;
            (ip.src_addr().into(), ip.dst_addr().into(), ip.payload())
        }
        _ => return None,
    };
    let tcp = TcpPacket::new_checked(payload).ok()?;
    Some((
        (
            SocketAddr::new(src.into(), tcp.src_port()),
            SocketAddr::new(dst.into(), tcp.dst_port()),
        ),
        tcp.syn() && !tcp.ack(),
    ))
}

impl Stack {
    /// For packets no longer than `mtu`, which is what the system is told
    /// the apps may send.
    pub fn new(mtu: usize) -> Self {
        let mut queues = Queues {
            mtu,
            ..Queues::default()
        };
        let config = Config::new(HardwareAddress::Ip);
        let mut iface = Interface::new(config, &mut queues, Instant::ZERO);
        // Every address is ours to answer for: that of whatever server an
        // app connects to. The addresses given here are never seen by an
        // app; they only say that everything is reached through here.
        iface.set_any_ip(true);
        let (v4, v6) = (Ipv4Address::new(10, 215, 0, 1), Ipv6Address::LOCALHOST);
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(v4.into(), 32));
            let _ = addrs.push(IpCidr::new(v6.into(), 128));
        });
        let _ = iface.routes_mut().add_default_ipv4_route(v4);
        let _ = iface.routes_mut().add_default_ipv6_route(v6);
        Self {
            started: TokioInstant::now(),
            iface,
            queues,
            sockets: SocketSet::new(Vec::new()),
            flows: HashMap::new(),
            by_ends: HashMap::new(),
            opened: VecDeque::new(),
            changed: HashSet::new(),
            all: false,
            next: 0,
        }
    }

    /// Takes a packet an app sent. One that begins a connection is given
    /// somebody to answer it, in the place of the server it is for; with
    /// too many connections already it is answered with a reset, and the
    /// app fails at once instead of waiting.
    pub fn receive(&mut self, packet: &[u8]) {
        let tcp = tcp_ends(packet);
        if let Some((ends, true)) = tcp
            && !self.by_ends.contains_key(&ends)
            && self.flows.len() < MAX_FLOWS
        {
            let buffer = || tcp::SocketBuffer::new(vec![0; BUFFER]);
            let mut socket = tcp::Socket::new(buffer(), buffer());
            socket.set_timeout(Some(SILENCE.into()));
            socket.set_keep_alive(Some(ASK_EVERY.into()));
            // The app is on this device: nothing is gained by waiting to
            // fill a packet or to acknowledge two at once.
            socket.set_nagle_enabled(false);
            socket.set_ack_delay(None);
            let server = IpListenEndpoint {
                addr: Some(ends.1.ip().into()),
                port: ends.1.port(),
            };
            if socket.listen(server).is_ok() {
                let id = Id(self.next);
                self.next += 1;
                let handle = self.sockets.add(socket);
                self.by_ends.insert(ends, id);
                self.flows.insert(
                    id,
                    Flow {
                        handle,
                        ends,
                        told: false,
                    },
                );
            }
        }
        self.queues.from_apps.copy(packet);
    }

    /// The app's end and the server's of the connection a packet begins,
    /// if it begins one that is not held: for whoever would first see
    /// that the server is there before the app is answered in its place.
    pub fn begins(&self, packet: &[u8]) -> Option<Ends> {
        let (ends, syn) = tcp_ends(packet)?;
        (syn && !self.by_ends.contains_key(&ends)).then_some(ends)
    }

    /// Takes the packet that begins a connection without anybody to
    /// answer it: the app is told at once that nothing is there, as it is
    /// by a server that refuses it, and goes on to wherever else it may.
    pub fn refuse(&mut self, packet: &[u8]) {
        self.queues.from_apps.copy(packet);
    }

    /// A packet for an app, if there is one.
    pub fn transmit(&mut self) -> Option<Vec<u8>> {
        self.queues.to_apps.ready.pop_front()
    }

    pub fn recycle(&mut self, packet: Vec<u8>) {
        self.queues.to_apps.recycle(packet);
    }

    /// Does what the packets taken and the bytes written call for, and
    /// says how long it can be until this is called again if nothing
    /// happens before: `None` is for ever.
    pub fn poll(&mut self) -> Option<Duration> {
        let now = Instant::from_micros(self.started.elapsed().as_micros() as i64);
        while let Some(packet) = self.queues.from_apps.ready.front() {
            let ends = tcp_ends(packet).map(|(ends, _)| ends);
            let id = ends.and_then(|ends| self.by_ends.get(&ends).copied());
            let was = id.map(|id| self.state(id));
            self.iface
                .poll_ingress_single(now, &mut self.queues, &mut self.sockets);
            if let Some(id) = id {
                // A reset before the connection opened takes it back to
                // listening, for the next beginning to this server from
                // any port, which may be the next packet: it would then
                // be told of with this one's app.
                if was != Some(tcp::State::Listen) && self.state(id) == tcp::State::Listen {
                    self.forget(id);
                    continue;
                }
                self.changed.insert(id);
                // Deliver an opening even if a following packet in
                // this batch closes the same connection.
                let flow = self.flows.get_mut(&id).expect("flow in endpoint index");
                Self::opened_if_ready(id, flow, &self.sockets, &mut self.opened);
            } else if ends.is_none() {
                self.all = true;
            }
        }
        self.iface.poll(now, &mut self.queues, &mut self.sockets);
        if std::mem::take(&mut self.all) {
            self.changed.extend(self.flows.keys().copied());
        }
        let mut gone = Vec::new();
        for &id in &self.changed {
            let Some(flow) = self.flows.get_mut(&id) else {
                continue;
            };
            let socket = self.sockets.get::<tcp::Socket>(flow.handle);
            // Every packet taken has been seen: one still listening never
            // had its beginning, which was lost, and the app sends it
            // again. A listener has no timeout and would hold its place.
            if matches!(socket.state(), tcp::State::Closed | tcp::State::Listen) {
                gone.push(id);
            } else {
                Self::opened_if_ready(id, flow, &self.sockets, &mut self.opened);
            }
        }
        for id in gone {
            self.forget(id);
        }
        self.iface
            .poll_delay(now, &self.sockets)
            .map(Duration::from)
    }

    fn opened_if_ready(
        id: Id,
        flow: &mut Flow,
        sockets: &SocketSet<'_>,
        opened: &mut VecDeque<Opened>,
    ) {
        if !flow.told
            && !matches!(
                sockets.get::<tcp::Socket>(flow.handle).state(),
                tcp::State::Closed | tcp::State::Listen | tcp::State::SynReceived
            )
        {
            flow.told = true;
            opened.push_back(Opened {
                id,
                app: flow.ends.0,
                server: flow.ends.1,
            });
        }
    }

    pub fn refresh_all(&mut self) {
        self.all = true;
    }

    pub fn take_changes(&mut self, changes: &mut HashSet<Id>) {
        std::mem::swap(&mut self.changed, changes);
    }

    pub fn changed(&mut self, id: Id) {
        if self.has(id) {
            self.changed.insert(id);
        }
    }

    fn forget(&mut self, id: Id) {
        if let Some(flow) = self.flows.remove(&id) {
            self.by_ends.remove(&flow.ends);
            self.sockets.remove(flow.handle);
        }
    }

    /// A connection an app has made since this was last asked.
    pub fn opened(&mut self) -> Option<Opened> {
        self.opened.pop_front()
    }

    fn state(&self, id: Id) -> tcp::State {
        self.sockets
            .get::<tcp::Socket>(self.flows[&id].handle)
            .state()
    }

    fn socket(&mut self, id: Id) -> Option<&mut tcp::Socket<'static>> {
        let handle = self.flows.get(&id)?.handle;
        Some(self.sockets.get_mut::<tcp::Socket>(handle))
    }

    /// Reads what the app sent on a connection.
    pub fn read(&mut self, id: Id, into: &mut [u8]) -> Read {
        let Some(socket) = self.socket(id) else {
            return Read::End;
        };
        if socket.can_recv() {
            match socket.recv_slice(into) {
                Ok(n) if n > 0 => Read::Data(n),
                Ok(_) => Read::Wait,
                Err(_) => Read::End,
            }
        } else if socket.may_recv() {
            Read::Wait
        } else {
            Read::End
        }
    }

    /// Writes to the app as much of `bytes` as there is room for, and says
    /// how much that was. None is written to a connection that is no more.
    pub fn write(&mut self, id: Id, bytes: &[u8]) -> usize {
        self.socket(id)
            .and_then(|socket| socket.send_slice(bytes).ok())
            .unwrap_or(0)
    }

    /// Whether the app can still be written to.
    pub fn open(&mut self, id: Id) -> bool {
        self.socket(id).is_some_and(|socket| socket.may_send())
    }

    /// Whether everything written has been taken by the app.
    #[cfg(test)]
    pub fn flushed(&mut self, id: Id) -> bool {
        self.socket(id)
            .is_none_or(|socket| socket.send_queue() == 0)
    }

    /// Ends our side of a connection: the app has been sent everything.
    pub fn finish(&mut self, id: Id) {
        if let Some(socket) = self.socket(id) {
            socket.close();
            self.changed.insert(id);
        }
    }

    /// Ends a connection with a reset, as a server that refuses it would.
    pub fn reset(&mut self, id: Id) {
        if let Some(socket) = self.socket(id) {
            socket.abort();
            self.changed.insert(id);
        }
    }

    /// Whether the connection is still held: not once both sides have
    /// ended it, or either has reset it.
    pub fn has(&self, id: Id) -> bool {
        self.flows.contains_key(&id)
    }

    /// How many connections are held.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.flows.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.flows.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use smoltcp::iface::SocketStorage;
    use smoltcp::phy::ChecksumCapabilities;
    use smoltcp::wire::{IpEndpoint, Ipv4Repr, TcpControl, TcpRepr, TcpSeqNumber};

    use super::*;

    const MTU: usize = 1500;

    /// An app: the other end of the packets, with a stack of its own, as
    /// the system has one for every app.
    struct App {
        iface: Interface,
        queues: Queues,
        sockets: SocketSet<'static>,
        port: u16,
    }

    impl App {
        fn new() -> Self {
            let mut queues = Queues {
                mtu: MTU,
                ..Queues::default()
            };
            let mut iface = Interface::new(
                Config::new(HardwareAddress::Ip),
                &mut queues,
                Instant::now(),
            );
            let (v4, v6) = (
                Ipv4Address::new(10, 0, 0, 2),
                Ipv6Address::new(0xfd00, 0, 0, 0, 0, 0, 0, 2),
            );
            iface.update_ip_addrs(|a| {
                a.push(IpCidr::new(v4.into(), 24)).unwrap();
                a.push(IpCidr::new(v6.into(), 64)).unwrap();
            });
            iface
                .routes_mut()
                .add_default_ipv4_route(Ipv4Address::new(10, 0, 0, 1))
                .unwrap();
            iface
                .routes_mut()
                .add_default_ipv6_route(Ipv6Address::new(0xfd00, 0, 0, 0, 0, 0, 0, 1))
                .unwrap();
            let storage: Vec<SocketStorage<'static>> = Vec::new();
            Self {
                iface,
                queues,
                sockets: SocketSet::new(storage),
                port: 40000,
            }
        }

        fn connect(&mut self, server: &str) -> SocketHandle {
            let buffer = || tcp::SocketBuffer::new(vec![0; BUFFER]);
            let mut socket = tcp::Socket::new(buffer(), buffer());
            // Answered at once, so that a test need not wait out the delay.
            socket.set_ack_delay(None);
            let server: SocketAddr = server.parse().unwrap();
            self.port += 1;
            socket
                .connect(self.iface.context(), IpEndpoint::from(server), self.port)
                .unwrap();
            self.sockets.add(socket)
        }

        fn socket(&mut self, handle: SocketHandle) -> &mut tcp::Socket<'static> {
            self.sockets.get_mut::<tcp::Socket>(handle)
        }
    }

    /// Passes packets between the app and the stack until neither has any
    /// more to say.
    fn settle(app: &mut App, stack: &mut Stack) {
        for _ in 0..200 {
            app.iface
                .poll(Instant::now(), &mut app.queues, &mut app.sockets);
            let mut passed = false;
            while let Some(packet) = app.queues.to_apps.ready.pop_front() {
                stack.receive(&packet);
                app.queues.to_apps.recycle(packet);
                passed = true;
            }
            stack.poll();
            while let Some(packet) = stack.transmit() {
                app.queues.from_apps.copy(&packet);
                stack.recycle(packet);
                passed = true;
            }
            if !passed {
                return;
            }
        }
        panic!("the two never fell silent");
    }

    #[test]
    fn a_connection_an_app_makes_to_any_server_is_answered_here() {
        for server in ["93.184.216.34:443", "[2606:2800:220:1::1]:8443"] {
            let (mut app, mut stack) = (App::new(), Stack::new(MTU));
            let handle = app.connect(server);
            settle(&mut app, &mut stack);
            assert_eq!(app.socket(handle).state(), tcp::State::Established);
            let opened = stack.opened().unwrap();
            assert_eq!(opened.server, server.parse().unwrap());
            assert_eq!(opened.app.port(), 40001);
            // Told of once.
            assert_eq!(stack.opened(), None);
            assert_eq!(stack.len(), 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_flow_is_probed_after_two_minutes_and_kept_until_three_intervals() {
        let (mut app, mut stack) = (App::new(), Stack::new(MTU));
        app.connect("93.184.216.34:443");
        settle(&mut app, &mut stack);
        let id = stack.opened().unwrap().id;
        tokio::time::advance(Duration::from_secs(119)).await;
        stack.poll();
        assert!(stack.transmit().is_none());
        tokio::time::advance(Duration::from_secs(1)).await;
        stack.poll();
        assert!(stack.transmit().is_some());
        assert!(stack.has(id));
        tokio::time::advance(Duration::from_secs(241)).await;
        stack.refresh_all();
        stack.poll();
        assert!(!stack.has(id));
    }

    #[test]
    fn packet_storage_is_reused_and_capped() {
        let mut packets = Packets::default();
        for _ in 0..(PACKET_SPARES + 1) {
            packets.recycle(Vec::with_capacity(MTU));
        }
        assert_eq!(packets.spare.len(), PACKET_SPARES);
        let reused = packets.spare.last().unwrap().as_ptr();
        packets.copy(b"new packet");
        assert_eq!(packets.ready[0], b"new packet");
        assert_eq!(packets.ready[0].as_ptr(), reused);
    }

    #[test]
    fn what_the_app_sends_is_read_and_what_is_written_reaches_it() {
        let (mut app, mut stack) = (App::new(), Stack::new(MTU));
        let handle = app.connect("93.184.216.34:443");
        settle(&mut app, &mut stack);
        let id = stack.opened().unwrap().id;

        let mut buf = [0; 64];
        assert_eq!(stack.read(id, &mut buf), Read::Wait);
        app.socket(handle).send_slice(b"hello").unwrap();
        settle(&mut app, &mut stack);
        assert_eq!(stack.read(id, &mut buf), Read::Data(5));
        assert_eq!(&buf[..5], b"hello");

        assert_eq!(stack.write(id, b"welcome"), 7);
        assert!(!stack.flushed(id));
        settle(&mut app, &mut stack);
        assert!(stack.flushed(id));
        let n = app.socket(handle).recv_slice(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"welcome");
    }

    #[test]
    fn more_than_there_is_room_for_is_not_taken_until_the_app_has_read() {
        let (mut app, mut stack) = (App::new(), Stack::new(MTU));
        let handle = app.connect("93.184.216.34:443");
        settle(&mut app, &mut stack);
        let id = stack.opened().unwrap().id;
        let much = vec![7u8; 4 * BUFFER];
        let mut written = 0;
        let mut read = Vec::new();
        for _ in 0..64 {
            written += stack.write(id, &much[written..]);
            settle(&mut app, &mut stack);
            let mut buf = [0; 4096];
            while let Ok(n) = app.socket(handle).recv_slice(&mut buf) {
                if n == 0 {
                    break;
                }
                read.extend_from_slice(&buf[..n]);
            }
            settle(&mut app, &mut stack);
        }
        assert_eq!(written, much.len());
        assert_eq!(read, much);
    }

    #[test]
    fn each_side_ends_its_half_and_the_connection_is_then_forgotten() {
        let (mut app, mut stack) = (App::new(), Stack::new(MTU));
        let handle = app.connect("93.184.216.34:443");
        settle(&mut app, &mut stack);
        let id = stack.opened().unwrap().id;
        let mut buf = [0; 8];

        app.socket(handle).close();
        settle(&mut app, &mut stack);
        assert_eq!(stack.read(id, &mut buf), Read::End);
        // The app sent all it will, and can still be answered.
        assert!(stack.open(id));
        assert_eq!(stack.write(id, b"bye"), 3);
        stack.finish(id);
        settle(&mut app, &mut stack);
        let n = app.socket(handle).recv_slice(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"bye");
        assert!(!app.socket(handle).may_recv());
        assert!(stack.is_empty());
        assert_eq!(stack.read(id, &mut buf), Read::End);
        assert_eq!(stack.write(id, b"late"), 0);
    }

    #[test]
    fn a_connection_reset_here_is_reset_for_the_app() {
        let (mut app, mut stack) = (App::new(), Stack::new(MTU));
        let handle = app.connect("93.184.216.34:443");
        settle(&mut app, &mut stack);
        let id = stack.opened().unwrap().id;
        stack.reset(id);
        settle(&mut app, &mut stack);
        assert_eq!(app.socket(handle).state(), tcp::State::Closed);
        assert!(stack.is_empty());
    }

    #[test]
    fn with_too_many_connections_a_new_one_is_refused_at_once() {
        let (mut app, mut stack) = (App::new(), Stack::new(MTU));
        let handles: Vec<SocketHandle> = (0..MAX_FLOWS)
            .map(|_| app.connect("93.184.216.34:443"))
            .collect();
        settle(&mut app, &mut stack);
        assert_eq!(stack.len(), MAX_FLOWS);
        assert_eq!(app.socket(handles[0]).state(), tcp::State::Established);
        let over = app.connect("93.184.216.34:443");
        settle(&mut app, &mut stack);
        assert_eq!(app.socket(over).state(), tcp::State::Closed);
        assert_eq!(stack.len(), MAX_FLOWS);
    }

    /// A segment from an app at 10.0.0.2 to 93.184.216.34:443.
    fn segment(port: u16, control: TcpControl, seq: u32, ack: Option<u32>) -> Vec<u8> {
        let (app, server) = (Ipv4Address::new(10, 0, 0, 2), SERVER);
        let tcp = TcpRepr {
            src_port: port,
            dst_port: 443,
            control,
            seq_number: TcpSeqNumber(seq as i32),
            ack_number: ack.map(|ack| TcpSeqNumber(ack as i32)),
            window_len: u16::MAX,
            window_scale: None,
            max_seg_size: None,
            sack_permitted: false,
            sack_ranges: [None; 3],
            timestamp: None,
            payload: &[],
        };
        let ip = Ipv4Repr {
            src_addr: app,
            dst_addr: server,
            next_header: IpProtocol::Tcp,
            payload_len: tcp.buffer_len(),
            hop_limit: 64,
        };
        let mut packet = vec![0; ip.buffer_len() + tcp.buffer_len()];
        let checksums = ChecksumCapabilities::default();
        let mut v4 = Ipv4Packet::new_unchecked(&mut packet);
        ip.emit(&mut v4, &checksums);
        let mut segment = TcpPacket::new_unchecked(v4.payload_mut());
        tcp.emit(&mut segment, &app.into(), &server.into(), &checksums);
        packet
    }

    const SERVER: Ipv4Address = Ipv4Address::new(93, 184, 216, 34);

    /// The sequence number the stack answered a SYN with.
    fn syn_ack(stack: &mut Stack) -> u32 {
        let packet = stack.transmit().unwrap();
        let ip = Ipv4Packet::new_checked(packet.as_slice()).unwrap();
        let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
        assert!(tcp.syn() && tcp.ack());
        tcp.seq_number().0 as u32
    }

    #[tokio::test(start_paused = true)]
    async fn a_connection_reset_before_it_opened_is_forgotten() {
        let mut stack = Stack::new(MTU);
        stack.receive(&segment(40001, TcpControl::Syn, 100, None));
        stack.poll();
        syn_ack(&mut stack);
        assert_eq!(stack.len(), 1);
        // The app gave up on it while its server was being reached.
        stack.receive(&segment(40001, TcpControl::Rst, 101, None));
        stack.poll();
        tokio::time::advance(Duration::from_secs(3600)).await;
        stack.refresh_all();
        stack.poll();
        assert!(stack.is_empty());
        assert_eq!(stack.opened(), None);
    }

    #[test]
    fn a_connection_after_one_reset_before_it_opened_is_told_with_its_own_address() {
        // The reset and the next connection may come in one batch.
        for polled_between in [true, false] {
            let mut stack = Stack::new(MTU);
            stack.receive(&segment(40001, TcpControl::Syn, 100, None));
            stack.poll();
            syn_ack(&mut stack);
            stack.receive(&segment(40001, TcpControl::Rst, 101, None));
            if polled_between {
                stack.poll();
            }
            stack.receive(&segment(40002, TcpControl::Syn, 500, None));
            stack.poll();
            let theirs = syn_ack(&mut stack);
            stack.receive(&segment(40002, TcpControl::None, 501, Some(theirs + 1)));
            stack.poll();
            let opened = stack.opened().unwrap();
            assert_eq!(opened.app, "10.0.0.2:40002".parse().unwrap());
            assert_eq!(stack.opened(), None);
            assert_eq!(stack.len(), 1);
        }
    }

    #[test]
    fn a_beginning_that_was_lost_holds_no_place_and_is_taken_when_sent_again() {
        let mut stack = Stack::new(MTU);
        for _ in 0..MAX_QUEUE {
            stack.receive(&[0x45]);
        }
        let syn = segment(40001, TcpControl::Syn, 100, None);
        stack.receive(&syn);
        assert_eq!(stack.len(), 1);
        stack.poll();
        assert!(stack.is_empty());
        stack.receive(&syn);
        stack.poll();
        let theirs = syn_ack(&mut stack);
        stack.receive(&segment(40001, TcpControl::None, 101, Some(theirs + 1)));
        stack.poll();
        assert_eq!(stack.opened().unwrap().app.port(), 40001);
    }

    #[test]
    fn a_beginning_sent_again_is_the_same_connection() {
        let mut stack = Stack::new(MTU);
        let syn = segment(40001, TcpControl::Syn, 100, None);
        stack.receive(&syn);
        stack.poll();
        let theirs = syn_ack(&mut stack);
        stack.receive(&syn);
        stack.refresh_all();
        stack.poll();
        assert_eq!(stack.len(), 1);
        stack.receive(&segment(40001, TcpControl::None, 101, Some(theirs + 1)));
        stack.poll();
        assert_eq!(stack.opened().unwrap().app.port(), 40001);
    }

    #[test]
    fn what_is_no_beginning_of_a_connection_begins_none() {
        let mut stack = Stack::new(MTU);
        for packet in [&[][..], &[0x45], &[0x60, 0, 0, 0], &[0xff; 40]] {
            stack.receive(packet);
            stack.poll();
        }
        assert!(stack.is_empty());
        assert_eq!(stack.opened(), None);
    }
}
