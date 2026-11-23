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

//! For the tests: an app and the system below it, a stack of its own
//! whose packets go to a tunnel running here and come from it.

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, IpCidr, IpEndpoint, Ipv4Address};
use tokio::sync::mpsc;

use crate::tunnel::{Config as TunnelConfig, Handler, Reach, Tun, run};

pub const MTU: usize = 1500;

/// The VPN's descriptor, as two channels.
pub struct Channels {
    from_apps: mpsc::Receiver<Vec<u8>>,
    to_apps: mpsc::UnboundedSender<Vec<u8>>,
}

impl Tun for Channels {
    async fn recv(&mut self, into: &mut [u8]) -> io::Result<usize> {
        match self.from_apps.recv().await {
            Some(packet) => {
                into[..packet.len()].copy_from_slice(&packet);
                Ok(packet.len())
            }
            None => Ok(0),
        }
    }

    fn send(&mut self, packet: &[u8]) -> io::Result<()> {
        let _ = self.to_apps.send(packet.to_vec());
        Ok(())
    }
}

#[derive(Default)]
pub struct Wire {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
}
pub struct Rx(Vec<u8>);
pub struct Tx<'a>(&'a mut VecDeque<Vec<u8>>);
impl phy::RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}
impl phy::TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut packet = vec![0; len];
        let r = f(&mut packet);
        self.0.push_back(packet);
        r
    }
}
impl phy::Device for Wire {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;
    fn receive(&mut self, _: Instant) -> Option<(Rx, Tx<'_>)> {
        Some((Rx(self.rx.pop_front()?), Tx(&mut self.tx)))
    }
    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        Some(Tx(&mut self.tx))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = MTU;
        caps
    }
}

/// An app and the system below it: a stack of its own, whose packets
/// go to the tunnel and come from it.
pub struct App {
    iface: Interface,
    wire: Wire,
    sockets: SocketSet<'static>,
    to_tunnel: mpsc::Sender<Vec<u8>>,
    from_tunnel: mpsc::UnboundedReceiver<Vec<u8>>,
    port: u16,
}

impl App {
    /// Leaves the VPN's half idle, without walking these sockets in the test client.
    pub fn leave_idle(&mut self, handles: Vec<SocketHandle>) {
        for handle in handles {
            self.sockets.remove(handle);
        }
    }

    /// Exchanges queued packets without adding artificial delay while a
    /// bulk transfer is making progress.
    pub async fn transfer(&mut self, handle: SocketHandle, size: usize, byte: u8) {
        let mut total = 0;
        let mut buf = [0u8; 16 << 10];
        while total < size {
            self.poll();
            while let Some(packet) = self.wire.tx.pop_front() {
                self.to_tunnel.send(packet).await.unwrap();
            }
            while let Ok(n) = self.socket(handle).recv_slice(&mut buf) {
                if n == 0 {
                    break;
                }
                assert!(buf[..n].iter().all(|&b| b == byte));
                total += n;
            }
            if total == size {
                return;
            }
            if let Ok(Some(packet)) =
                tokio::time::timeout(Duration::from_millis(1), self.from_tunnel.recv()).await
            {
                self.wire.rx.push_back(packet);
            }
            while let Ok(packet) = self.from_tunnel.try_recv() {
                self.wire.rx.push_back(packet);
            }
        }
        assert_eq!(total, size);
    }

    pub fn connect(&mut self, server: &str) -> SocketHandle {
        let buffer = || tcp::SocketBuffer::new(vec![0; 64 << 10]);
        let mut socket = tcp::Socket::new(buffer(), buffer());
        socket.set_ack_delay(None);
        let server: SocketAddr = server.parse().unwrap();
        self.port += 1;
        socket
            .connect(self.iface.context(), IpEndpoint::from(server), self.port)
            .unwrap();
        self.sockets.add(socket)
    }

    pub fn socket(&mut self, handle: SocketHandle) -> &mut tcp::Socket<'static> {
        self.sockets.get_mut::<tcp::Socket>(handle)
    }

    fn poll(&mut self) {
        self.iface
            .poll(Instant::now(), &mut self.wire, &mut self.sockets);
    }

    /// Lets packets pass both ways until `done` says so.
    pub async fn until(&mut self, mut done: impl FnMut(&mut Self) -> bool) {
        for _ in 0..2000 {
            self.poll();
            while let Some(packet) = self.wire.tx.pop_front() {
                self.to_tunnel.send(packet).await.unwrap();
            }
            if done(self) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
            while let Ok(packet) = self.from_tunnel.try_recv() {
                self.wire.rx.push_back(packet);
            }
        }
        panic!("it never came to that");
    }

    /// Sends a packet as it is, past the app's own stack.
    pub async fn send(&mut self, packet: Vec<u8>) {
        self.to_tunnel.send(packet).await.unwrap();
    }

    pub fn packet_sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.to_tunnel.clone()
    }

    /// The next packet given back for the app, if one comes soon.
    pub async fn given(&mut self) -> Option<Vec<u8>> {
        let soon = Duration::from_secs(2);
        tokio::time::timeout(soon, self.from_tunnel.recv())
            .await
            .ok()
            .flatten()
    }

    /// Reads what has come for the app on a connection.
    pub fn read(&mut self, handle: SocketHandle) -> Vec<u8> {
        let mut all = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = self.socket(handle).recv_slice(&mut buf) {
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf[..n]);
        }
        all
    }
}

/// A tunnel running, with an app before it; connections are given to
/// `handler`.
pub fn tunnel(handler: Handler) -> App {
    tunnel_reaching(handler, None)
}

/// As [`tunnel`], with what finds out whether a server is there.
pub fn tunnel_reaching(handler: Handler, reach: Option<Reach>) -> App {
    let (app, channels) = app_and_channels();
    let protect = std::sync::Arc::new(|_| true);
    tokio::spawn(run(
        channels,
        TunnelConfig {
            mtu: MTU,
            handler,
            reach,
            protect,
            policy: crate::udp::Policy::test(Some(10001)),
            dns: None,
        },
    ));
    app
}

pub fn first_syn(server: &str) -> Vec<u8> {
    let (mut app, _) = app_and_channels();
    app.connect(server);
    app.poll();
    app.wire.tx.pop_front().unwrap()
}

pub fn app_and_channels() -> (App, Channels) {
    let (to_tunnel, from_apps) = mpsc::channel(256);
    let (to_apps, from_tunnel) = mpsc::unbounded_channel();
    let mut wire = Wire::default();
    let mut iface = Interface::new(Config::new(HardwareAddress::Ip), &mut wire, Instant::now());
    iface.update_ip_addrs(|a| {
        a.push(IpCidr::new(Ipv4Address::new(10, 0, 0, 2).into(), 24))
            .unwrap();
    });
    iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 0, 0, 1))
        .unwrap();
    let app = App {
        iface,
        wire,
        sockets: SocketSet::new(Vec::new()),
        to_tunnel,
        from_tunnel,
        port: 40000,
    };
    (app, Channels { from_apps, to_apps })
}
