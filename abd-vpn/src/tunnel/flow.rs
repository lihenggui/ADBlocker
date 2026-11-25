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

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use ring::rand::{SecureRandom as _, SystemRandom};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::{Notify, mpsc, oneshot, watch};

use super::Handler;
use crate::stack::{Id, MAX_FLOWS, Opened, Read, Stack};

/// How much is passed at a time, and how many such pieces for an app may
/// wait for the socket before no more is taken from the stack.
const PIECE: usize = 16 << 10;
const WAITING: usize = 4;

/// How long the making of the loopback socket for a connection may take.
const PAIR_TIME: Duration = Duration::from_secs(5);

/// What the stack's task tells the task of a connection.
enum ToSocket {
    /// What the app sent.
    Data(Bytes),
    /// The app has sent all it will.
    End,
}

/// What the task of a connection tells the stack's task.
pub(super) enum FromSocket {
    /// What is for the app. The sender waits for the flow to take it.
    Data(Id, Bytes),
    /// Nothing more is for the app.
    End(Id),
    /// The connection was refused or broke: the app is to have a reset.
    Reset(Id),
}

/// A connection, as the stack's task holds it.
struct Flow {
    /// For what the app sent. Dropped to say the app reset the connection.
    to_socket: mpsc::Sender<ToSocket>,
    /// What is for the app and not yet all in the stack.
    pending: Option<Bytes>,
    taken: watch::Sender<()>,
    /// Nothing more is for the app once `pending` is written.
    ending: bool,
    /// The app was told so.
    finished: bool,
    /// The socket was told the app has sent all it will.
    app_ended: bool,
}

type Ticket = [u8; 16];

struct Waiter {
    ticket: Ticket,
    accepted: oneshot::Sender<TcpStream>,
}

struct Pairs {
    address: SocketAddr,
    random: SystemRandom,
    waiting: Mutex<HashMap<SocketAddr, Waiter>>,
}

struct PendingPair<'a> {
    pairs: &'a Pairs,
    peer: SocketAddr,
    ticket: Ticket,
}

impl Drop for PendingPair<'_> {
    fn drop(&mut self) {
        let mut waiting = self.pairs.waiting.lock().unwrap();
        if waiting
            .get(&self.peer)
            .is_some_and(|p| p.ticket == self.ticket)
        {
            waiting.remove(&self.peer);
        }
    }
}

impl Pairs {
    async fn connect(&self) -> io::Result<(TcpStream, TcpStream)> {
        let socket = TcpSocket::new_v4()?;
        socket.bind((Ipv4Addr::LOCALHOST, 0).into())?;
        let peer = socket.local_addr()?;
        let mut ticket = [0; 16];
        self.random
            .fill(&mut ticket)
            .map_err(|_| io::Error::other("loopback ticket"))?;
        let (accepted, incoming) = oneshot::channel();
        {
            let mut waiting = self.waiting.lock().unwrap();
            if waiting.len() >= MAX_FLOWS {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            waiting.insert(peer, Waiter { ticket, accepted });
        }
        let _pending = PendingPair {
            pairs: self,
            peer,
            ticket,
        };
        let mut ours = socket.connect(self.address).await?;
        ours.set_nodelay(true)?;
        ours.write_all(&ticket).await?;
        let mut theirs = incoming
            .await
            .map_err(|_| io::ErrorKind::ConnectionAborted)?;
        // A stale connection may still be in the listener backlog when
        // its peer port is reused. Only our live socket has this ticket.
        let mut received = [0; 16];
        theirs.read_exact(&mut received).await?;
        if received != ticket {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        theirs.set_nodelay(true)?;
        Ok((ours, theirs))
    }

    fn accept(&self, stream: TcpStream, peer: SocketAddr) {
        if let Some(waiter) = self.waiting.lock().unwrap().remove(&peer) {
            // Cancellation drops the recipient, and then this socket too.
            let _ = waiter.accepted.send(stream);
        }
    }
}

async fn accept_pairs(listener: TcpListener, pairs: Arc<Pairs>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => pairs.accept(stream, peer),
            // Resource exhaustion must not stop the established flows.
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

/// Passes one connection between its socket and the stack's task, until
/// both sides have ended it or either has broken it.
async fn carry(
    opened: Opened,
    handler: Handler,
    mut from_stack: mpsc::Receiver<ToSocket>,
    to_stack: mpsc::Sender<FromSocket>,
    room: Arc<Notify>,
    pairs: Arc<Pairs>,
    mut taken: watch::Receiver<()>,
) {
    let id = opened.id;
    let Ok(Ok((ours, theirs))) = tokio::time::timeout(PAIR_TIME, pairs.connect()).await else {
        let _ = to_stack.send(FromSocket::Reset(id)).await;
        return;
    };
    handler(theirs, opened.app, opened.server);
    let (mut read, mut write) = ours.into_split();
    let mut buf = vec![0u8; PIECE];
    // Which halves are still open: what the app sends, and what is for it.
    let (mut app_sends, mut for_app) = (true, true);
    while app_sends || for_app {
        tokio::select! {
            said = from_stack.recv(), if app_sends => match said {
                Some(ToSocket::Data(bytes)) => {
                    let wrote = write.write_all(&bytes).await;
                    // There is room for another piece.
                    room.notify_one();
                    if wrote.is_err() {
                        let _ = to_stack.send(FromSocket::Reset(id)).await;
                        return;
                    }
                }
                Some(ToSocket::End) => {
                    let _ = write.shutdown().await;
                    app_sends = false;
                }
                // The app reset the connection: so is the socket, by
                // being closed with what it was sent unread.
                None => {
                    let whole = read.reunite(write).ok();
                    if let Some(whole) = whole {
                        let _ = socket2::SockRef::from(&whole).set_linger(Some(Duration::ZERO));
                    }
                    return;
                }
            },
            got = read.read(&mut buf), if for_app => match got {
                Ok(0) => {
                    for_app = false;
                    if to_stack.send(FromSocket::End(id)).await.is_err() {
                        return;
                    }
                }
                Ok(n) => {
                    let piece = Bytes::copy_from_slice(&buf[..n]);
                    if to_stack.send(FromSocket::Data(id, piece)).await.is_err() {
                        return;
                    }
                    // No more is read until the stack has this: the app
                    // may be slower than the server.
                    if taken.changed().await.is_err() {
                        // The app is gone, and what was for it with it.
                        for_app = false;
                    }
                }
                Err(_) => {
                    let _ = to_stack.send(FromSocket::Reset(id)).await;
                    return;
                }
            },
        }
    }
}

impl Flow {
    fn pump(&mut self, id: Id, stack: &mut Stack, piece: &mut [u8], moved: &mut bool) -> bool {
        // The receiver can close normally after receiving the app's FIN.
        if !self.app_ended && self.to_socket.is_closed() {
            return Self::receiver_closed(id, stack, moved);
        }
        // What is for the app, as far as the stack has room.
        if let Some(bytes) = self.pending.take() {
            let wrote = stack.write(id, &bytes);
            *moved |= wrote > 0;
            if wrote < bytes.len() && stack.open(id) {
                self.pending = Some(bytes.slice(wrote..));
            } else {
                self.taken.send_replace(());
            }
        }
        if self.ending && self.pending.is_none() && !self.finished {
            stack.finish(id);
            self.finished = true;
            *moved = true;
        }
        // What the app sent, as far as the socket has room.
        while !self.app_ended && self.to_socket.capacity() > 0 {
            let message = match stack.read(id, piece) {
                Read::Data(n) => {
                    *moved = true;
                    ToSocket::Data(Bytes::copy_from_slice(&piece[..n]))
                }
                Read::Wait => break,
                Read::End => {
                    // Not said of a connection the app reset: its
                    // task is told that by the channel closing.
                    self.app_ended = true;
                    if !stack.has(id) && !self.finished {
                        break;
                    }
                    ToSocket::End
                }
            };
            // Only this flow sends, so the room checked above is still
            // available unless the receiver closed while we read.
            if self.to_socket.try_send(message).is_err() {
                return Self::receiver_closed(id, stack, moved);
            }
        }
        // Kept until the stack has let go of it; a task that is
        // still passing on what the app sent last has its own end
        // of the channel.
        stack.has(id)
    }

    fn receiver_closed(id: Id, stack: &mut Stack, moved: &mut bool) -> bool {
        stack.reset(id);
        *moved = true;
        false
    }
}

pub(super) struct Flows {
    active: HashMap<Id, Flow>,
    pairs: Arc<Pairs>,
    accepting: tokio::task::AbortHandle,
    handler: Handler,
    to_stack: mpsc::Sender<FromSocket>,
    pub(super) room: Arc<Notify>,
    stalled: HashSet<Id>,
    changed: HashSet<Id>,
    piece: Vec<u8>,
}

impl Drop for Flows {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

impl Flows {
    pub(super) async fn new(handler: Handler) -> io::Result<(Self, mpsc::Receiver<FromSocket>)> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let pairs = Arc::new(Pairs {
            address: listener.local_addr()?,
            random: SystemRandom::new(),
            waiting: Mutex::default(),
        });
        let accepting = tokio::spawn(accept_pairs(listener, pairs.clone())).abort_handle();
        let (to_stack, from_sockets) = mpsc::channel(64);
        let flows = Self {
            active: HashMap::new(),
            pairs,
            accepting,
            handler,
            to_stack,
            room: Arc::new(Notify::new()),
            stalled: HashSet::new(),
            changed: HashSet::new(),
            piece: vec![0u8; PIECE],
        };
        Ok((flows, from_sockets))
    }

    pub(super) fn on_socket(&mut self, stack: &mut Stack, said: FromSocket) {
        match said {
            FromSocket::Data(id, bytes) => {
                if let Some(flow) = self.active.get_mut(&id) {
                    flow.pending = Some(bytes);
                    stack.changed(id);
                }
            }
            FromSocket::End(id) => {
                if let Some(flow) = self.active.get_mut(&id) {
                    flow.ending = true;
                    stack.changed(id);
                }
            }
            FromSocket::Reset(id) => {
                stack.reset(id);
                self.active.remove(&id);
                self.stalled.remove(&id);
            }
        }
    }

    pub(super) fn socket_room(&mut self, stack: &mut Stack) {
        for id in self.stalled.drain() {
            stack.changed(id);
        }
    }

    pub(super) fn pump(&mut self, stack: &mut Stack) -> bool {
        while let Some(opened) = stack.opened() {
            self.open(opened);
        }
        let mut moved = false;
        stack.take_changes(&mut self.changed);
        for id in self.changed.drain() {
            if let Some(flow) = self.active.get_mut(&id) {
                if !flow.pump(id, stack, &mut self.piece, &mut moved) {
                    self.active.remove(&id);
                    self.stalled.remove(&id);
                } else if !flow.app_ended && flow.to_socket.capacity() == 0 {
                    self.stalled.insert(id);
                } else {
                    self.stalled.remove(&id);
                }
            }
        }
        moved
    }

    fn open(&mut self, opened: Opened) {
        let (to_socket, from_stack) = mpsc::channel(WAITING);
        let (ready, taken) = watch::channel(());
        self.active.insert(
            opened.id,
            Flow {
                to_socket,
                pending: None,
                taken: ready,
                ending: false,
                finished: false,
                app_ended: false,
            },
        );
        let (handler, to_stack, room) = (
            self.handler.clone(),
            self.to_stack.clone(),
            self.room.clone(),
        );
        tokio::spawn(carry(
            opened,
            handler,
            from_stack,
            to_stack,
            room,
            self.pairs.clone(),
            taken,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{MTU, app_and_channels};
    use crate::tunnel::Tun;
    use smoltcp::wire::{Ipv4Packet, TcpPacket};

    #[tokio::test]
    async fn one_listener_routes_live_pairs_and_closes_unregistered_connections() {
        let (flows, _) = Flows::new(Arc::new(|_, _, _| unreachable!()))
            .await
            .unwrap();
        tokio::time::timeout(PAIR_TIME, async {
            let mut stranger = TcpStream::connect(flows.pairs.address).await.unwrap();
            let (first, second) = tokio::join!(flows.pairs.connect(), flows.pairs.connect());
            let (mut first, mut first_peer) = first.unwrap();
            let (mut second, mut second_peer) = second.unwrap();
            assert_eq!(
                first_peer.local_addr().unwrap(),
                second_peer.local_addr().unwrap()
            );
            first.write_all(b"A").await.unwrap();
            second.write_all(b"B").await.unwrap();
            assert_eq!(first_peer.read_u8().await.unwrap(), b'A');
            assert_eq!(second_peer.read_u8().await.unwrap(), b'B');
            assert_eq!(stranger.read(&mut [0]).await.unwrap(), 0);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_loopback_connection_with_the_wrong_ticket_cannot_carry_a_flow() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let pairs = Pairs {
            address: listener.local_addr().unwrap(),
            random: SystemRandom::new(),
            waiting: Mutex::default(),
        };
        let substitute = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut stranger = TcpStream::connect(substitute.local_addr().unwrap())
            .await
            .unwrap();
        let (wrong, _) = substitute.accept().await.unwrap();
        stranger.write_all(&[0; 16]).await.unwrap();
        // Deliver a stream other than the one for this peer, as if a stale
        // connection from the same ephemeral port were still in the backlog.
        let dispatch = async {
            let (actual, peer) = listener.accept().await.unwrap();
            pairs.accept(wrong, peer);
            actual
        };
        let (result, _actual) =
            tokio::time::timeout(PAIR_TIME, async { tokio::join!(pairs.connect(), dispatch) })
                .await
                .unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert!(pairs.waiting.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_closed_full_flow_receiver_resets_the_connection() {
        let (mut app, mut tun) = app_and_channels();
        let handle = app.connect("93.184.216.34:443");
        let mut stack = Stack::new(MTU);
        let handshake = async {
            let mut packet = [0; MTU];
            loop {
                let n = tun.recv(&mut packet).await.unwrap();
                stack.receive(&packet[..n]);
                stack.poll();
                while let Some(reply) = stack.transmit() {
                    tun.send(&reply).unwrap();
                    stack.recycle(reply);
                }
                if let Some(opened) = stack.opened() {
                    return opened.id;
                }
            }
        };
        let (_, id) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(app.until(|app| app.socket(handle).may_send()), handshake)
        })
        .await
        .unwrap();

        let (mut flows, _) = Flows::new(Arc::new(|_, _, _| unreachable!()))
            .await
            .unwrap();
        let (to_socket, mut receiver) = mpsc::channel(1);
        to_socket
            .try_send(ToSocket::Data(Bytes::from_static(b"queued")))
            .unwrap();
        receiver.close();
        assert_eq!(to_socket.capacity(), 0);
        let (taken, _) = watch::channel(());
        flows.active.insert(
            id,
            Flow {
                to_socket,
                pending: None,
                taken,
                ending: false,
                finished: false,
                app_ended: false,
            },
        );
        flows.stalled.insert(id);

        assert!(flows.pump(&mut stack));
        assert!(!flows.active.contains_key(&id));
        assert!(!flows.stalled.contains(&id));
        stack.poll();
        assert!(!stack.has(id));
        let reset = stack.transmit().unwrap();
        let ip = Ipv4Packet::new_checked(reset.as_slice()).unwrap();
        assert!(TcpPacket::new_checked(ip.payload()).unwrap().rst());
    }
}
