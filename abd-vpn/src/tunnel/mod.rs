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

//! The packets of the VPN, taken and given back, and each connection made
//! of them handed on as a socket.
//!
//! The engine works on sockets of the kernel: it looks at the beginning of
//! a connection without taking it, has the kernel move what it does not
//! read, and refuses with a reset. A connection made of packets here is
//! therefore given to it through a socket on the loopback interface, whose
//! other end is passed to and from the stack by the one task that owns it
//! (`run`). The engine is then the same for a connection that came this
//! way as for one the packet filter delivered.
//!
//! Nothing is held without a bound: what an app sent is taken from the
//! stack only while the socket has room, and what is for the app is taken
//! from the socket only once the stack took what came before.

mod flow;
mod ingress;

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Instant, Sleep};

use self::flow::Flows;
use self::ingress::{Ingress, PACKET_BATCH, Turn};
use crate::dns::Resolver;
use crate::platform::Protect;
use crate::stack::Stack;
use crate::udp::Policy;

const FULL_PASS: Duration = Duration::from_millis(5);

/// Where the packets come from and go to: the VPN's descriptor, or what
/// stands for it in a test.
pub trait Tun: Send {
    /// The next packet the apps sent.
    fn recv(&mut self, into: &mut [u8]) -> impl Future<Output = io::Result<usize>> + Send;
    /// Attempts to give a whole packet back to an app.
    fn send(&mut self, packet: &[u8]) -> io::Result<()>;
}

/// Takes a connection an app made: the socket it is had through, the
/// app's address and the server it was connecting to. Called on the task
/// that owns the stack, so it starts what it does and returns.
pub type Handler = Arc<dyn Fn(TcpStream, SocketAddr, SocketAddr) + Send + Sync>;

/// Finds out whether the server an app is connecting to is there, given
/// the app's address and the server's, before the app is answered in its
/// place: by connecting to it. An app answered first would hold a
/// connection that then breaks, where its own would never have been made;
/// told that nothing is there while it still connects, it goes on to the
/// server's next address as it would have.
pub type Reach = Arc<
    dyn Fn(SocketAddr, SocketAddr) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send>>
        + Send
        + Sync,
>;

pub(crate) struct Config {
    pub(crate) mtu: usize,
    pub(crate) handler: Handler,
    pub(crate) reach: Option<Reach>,
    pub(crate) protect: Protect,
    pub(crate) policy: Policy,
    pub(crate) dns: Option<Arc<Resolver>>,
}

/// Runs the VPN until the packets end or the descriptor can no longer be used.
pub(crate) async fn run(tun: impl Tun, config: Config) -> io::Result<()> {
    drive(tun, config, Stack::poll).await
}

async fn drive(
    mut tun: impl Tun,
    config: Config,
    mut poll_stack: impl FnMut(&mut Stack) -> Option<Duration>,
) -> io::Result<()> {
    let Config {
        mtu,
        handler,
        reach,
        protect,
        policy,
        dns,
    } = config;
    let mut stack = Stack::new(mtu);
    let (mut ingress, mut answers, mut finds) =
        Ingress::new(mtu, protect, policy, dns.as_ref(), reach);
    let (mut flows, mut from_sockets) = Flows::new(tcp_handler(handler, dns.as_ref()));
    let mut packet = vec![0u8; mtu];
    let mut tcp = TcpSchedule {
        deadline: None,
        full_pass: Instant::now() + FULL_PASS,
    };
    let timer = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(timer);

    loop {
        // A readable descriptor can have another packet ready on every turn.
        tokio::task::consume_budget().await;
        let mut ended = false;
        let turn = tokio::select! {
            got = tun.recv(&mut packet) => match got? {
                0 => return Ok(()),
                n => {
                    let (turn, eof) = ingress.batch(&mut stack, &mut tun, &mut packet, n, tcp.deadline).await?;
                    ended = eof;
                    turn
                }
            },
            Some(found) = finds.recv() => {
                ingress.reach_done(&mut stack, found);
                Turn::Tcp
            }
            Some(said) = from_sockets.recv() => {
                flows.on_socket(&mut stack, said);
                Turn::Tcp
            }
            () = flows.room.notified() => {
                flows.socket_room(&mut stack);
                Turn::Tcp
            }
            Some(packet) = answers.recv() => {
                send_answers(&mut tun, &packet, &mut answers)?;
                Turn::UdpOnly
            }
            () = &mut timer, if tcp.deadline.is_some() => Turn::Tcp,
        };
        tcp.flush_ready(
            turn,
            &mut tun,
            &mut stack,
            &mut flows,
            &mut poll_stack,
            timer.as_mut(),
        )?;
        if ended {
            return Ok(());
        }
    }
}

struct TcpSchedule {
    deadline: Option<Instant>,
    full_pass: Instant,
}

impl TcpSchedule {
    fn flush_ready(
        &mut self,
        turn: Turn,
        tun: &mut impl Tun,
        stack: &mut Stack,
        flows: &mut Flows,
        poll_stack: &mut impl FnMut(&mut Stack) -> Option<Duration>,
        timer: Pin<&mut Sleep>,
    ) -> io::Result<()> {
        // UDP can win the selection even when the TCP timer is ready.
        let now = Instant::now();
        let tcp_due = self.deadline.is_some_and(|at| now >= at);
        if turn == Turn::Tcp || tcp_due {
            // Timers can change a socket without an incoming packet. The
            // backstop also checks every flow during sustained TCP traffic
            // without scheduling an extra periodic wakeup while idle.
            if tcp_due || now >= self.full_pass {
                stack.refresh_all();
                self.full_pass = now + FULL_PASS;
            }
            self.deadline = flush(tun, stack, flows, poll_stack)?;
            if let Some(at) = self.deadline {
                timer.reset(at);
            }
        }
        Ok(())
    }
}

fn send_packet(tun: &mut impl Tun, packet: &[u8]) -> io::Result<()> {
    // Drop the packet on backpressure or interruption. TCP can retry
    // on its existing deadline and UDP remains best effort. A full or
    // memory-starved TUN queue (ENOBUFS, ENOMEM) is backpressure too:
    // ending the tunnel for it would cut every app off.
    tun.send(packet).or_else(|error| {
        if is_transient_write_error(&error) {
            Ok(())
        } else {
            Err(error)
        }
    })
}

/// Sends a UDP answer and those ready after it, a batch at most, so that
/// a burst takes one turn and still leaves TCP its turn.
fn send_answers(
    tun: &mut impl Tun,
    first: &[u8],
    answers: &mut mpsc::Receiver<Vec<u8>>,
) -> io::Result<()> {
    send_packet(tun, first)?;
    for _ in 1..PACKET_BATCH {
        let Ok(packet) = answers.try_recv() else {
            break;
        };
        send_packet(tun, &packet)?;
    }
    Ok(())
}

fn is_transient_write_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    ) || matches!(error.raw_os_error(), Some(libc::ENOBUFS | libc::ENOMEM))
}

fn flush(
    tun: &mut impl Tun,
    stack: &mut Stack,
    flows: &mut Flows,
    poll_stack: &mut impl FnMut(&mut Stack) -> Option<Duration>,
) -> io::Result<Option<Instant>> {
    // Until nothing moves any more: a packet taken may be bytes to
    // pass on, and bytes passed on a packet to give back.
    loop {
        let now = Instant::now();
        let deadline = poll_stack(stack).map(|delay| now + delay);
        let moved = flows.pump(stack);
        while let Some(packet) = stack.transmit() {
            let sent = send_packet(tun, &packet);
            stack.recycle(packet);
            sent?;
        }
        if !moved {
            return Ok(deadline);
        }
    }
}

fn tcp_handler(handler: Handler, dns: Option<&Arc<Resolver>>) -> Handler {
    if let Some(resolver) = dns {
        let resolver = resolver.clone();
        Arc::new(move |socket, app, server| {
            if Resolver::answers(server) {
                let resolver = resolver.clone();
                tokio::spawn(async move { resolver.tcp(socket, app, server).await });
            } else {
                handler(socket, app, server);
            }
        })
    } else {
        handler
    }
}

#[cfg(test)]
mod tests;
