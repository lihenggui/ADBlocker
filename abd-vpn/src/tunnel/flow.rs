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
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc, oneshot};

use super::Handler;
use crate::stack::{Id, Opened, Read, Stack};

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
    /// What is for the app, and whom to tell once the stack has it all.
    Data(Id, Bytes, oneshot::Sender<()>),
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
    pending: Option<(Bytes, oneshot::Sender<()>)>,
    /// Nothing more is for the app once `pending` is written.
    ending: bool,
    /// The app was told so.
    finished: bool,
    /// The socket was told the app has sent all it will.
    app_ended: bool,
}

/// Two ends of a connection on the loopback interface.
///
/// A listener of its own for each, on a port of the kernel's choosing and
/// gone at once: what is accepted is taken only if it is the end made
/// here, since any app on the device can connect to the loopback.
async fn pair() -> io::Result<(TcpStream, TcpStream)> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let ours = TcpStream::connect(listener.local_addr()?).await?;
    let from = ours.local_addr()?;
    loop {
        let (theirs, peer) = listener.accept().await?;
        if peer == from {
            // Nothing is gained on the loopback by waiting to fill a packet.
            ours.set_nodelay(true)?;
            theirs.set_nodelay(true)?;
            return Ok((ours, theirs));
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
) {
    let id = opened.id;
    let Ok(Ok((ours, theirs))) = tokio::time::timeout(PAIR_TIME, pair()).await else {
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
                    let (taken, took) = oneshot::channel();
                    let piece = Bytes::copy_from_slice(&buf[..n]);
                    if to_stack.send(FromSocket::Data(id, piece, taken)).await.is_err() {
                        return;
                    }
                    // No more is read until the stack has this: the app
                    // may be slower than the server.
                    if took.await.is_err() {
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
        if let Some((bytes, taken)) = self.pending.take() {
            let wrote = stack.write(id, &bytes);
            *moved |= wrote > 0;
            if wrote < bytes.len() && stack.open(id) {
                self.pending = Some((bytes.slice(wrote..), taken));
            } else {
                let _ = taken.send(());
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
    handler: Handler,
    to_stack: mpsc::Sender<FromSocket>,
    pub(super) room: Arc<Notify>,
    stalled: HashSet<Id>,
    changed: HashSet<Id>,
    piece: Vec<u8>,
}

impl Flows {
    pub(super) fn new(handler: Handler) -> (Self, mpsc::Receiver<FromSocket>) {
        let (to_stack, from_sockets) = mpsc::channel(64);
        let flows = Self {
            active: HashMap::new(),
            handler,
            to_stack,
            room: Arc::new(Notify::new()),
            stalled: HashSet::new(),
            changed: HashSet::new(),
            piece: vec![0u8; PIECE],
        };
        (flows, from_sockets)
    }

    pub(super) fn on_socket(&mut self, stack: &mut Stack, said: FromSocket) {
        match said {
            FromSocket::Data(id, bytes, taken) => {
                // Dropped if the connection is no more, which tells
                // its task so.
                if let Some(flow) = self.active.get_mut(&id) {
                    flow.pending = Some((bytes, taken));
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
        self.active.insert(
            opened.id,
            Flow {
                to_socket,
                pending: None,
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
        tokio::spawn(carry(opened, handler, from_stack, to_stack, room));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{MTU, app_and_channels};
    use crate::tunnel::Tun;
    use smoltcp::wire::{Ipv4Packet, TcpPacket};

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

        let (mut flows, _) = Flows::new(Arc::new(|_, _, _| unreachable!()));
        let (to_socket, mut receiver) = mpsc::channel(1);
        to_socket
            .try_send(ToSocket::Data(Bytes::from_static(b"queued")))
            .unwrap();
        receiver.close();
        assert_eq!(to_socket.capacity(), 0);
        flows.active.insert(
            id,
            Flow {
                to_socket,
                pending: None,
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
