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

use std::collections::{HashMap, hash_map::Entry};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ring::rand::{SecureRandom as _, SystemRandom};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use tokio::task::JoinHandle;

use super::{MAX_PENDING, Question};
use crate::platform::{Protect, protect_socket};

type Pending = Mutex<HashMap<[u8; 2], Reply>>;

/// A tunnel owns its resolver sockets. Changing the Android network
/// replaces the tunnel and drops the sockets and their pending queries.
pub(super) struct UdpResolver {
    pub(super) address: SocketAddr,
    connection: AsyncMutex<Option<Arc<Connected>>>,
}

struct Connected {
    socket: Arc<UdpSocket>,
    pending: Arc<Pending>,
    receiver: JoinHandle<()>,
}

impl Drop for Connected {
    fn drop(&mut self) {
        self.receiver.abort();
    }
}

struct Reply {
    name: Box<[u8]>,
    kind: u16,
    class: u16,
    original_id: [u8; 2],
    deliver: Option<oneshot::Sender<Vec<u8>>>,
}

struct Outstanding<'a> {
    table: &'a Pending,
    id: [u8; 2],
}

impl Drop for Outstanding<'_> {
    fn drop(&mut self) {
        self.table.lock().unwrap().remove(&self.id);
    }
}

impl UdpResolver {
    pub(super) fn new(address: SocketAddr) -> Self {
        Self {
            address,
            connection: AsyncMutex::default(),
        }
    }

    /// The caller bounds this operation, including waiting for a socket and
    /// for its answer. A cancelled attempt releases its transaction ID.
    pub(super) async fn query(
        &self,
        protect: Protect,
        request: &[u8],
        question: &Question<'_>,
        mtu: usize,
    ) -> Option<Vec<u8>> {
        let connection = self.connection(protect, mtu).await?;
        let (transaction, answer) = connection.reserve(question)?;
        let mut bytes = request.to_vec();
        bytes[..2].copy_from_slice(&transaction.id);
        connection.socket.send(&bytes).await.ok()?;
        let answer = answer.await.ok();
        drop(transaction);
        answer
    }

    async fn connection(&self, protect: Protect, mtu: usize) -> Option<Arc<Connected>> {
        let mut current = self.connection.lock().await;
        if let Some(connection) = current.as_ref().filter(|conn| !conn.receiver.is_finished()) {
            return Some(connection.clone());
        }
        let bind = if self.address.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = protect_socket(protect, UdpSocket::bind(bind).await.ok()?)
            .await
            .ok()?;
        socket.connect(self.address).await.ok()?;
        let socket = Arc::new(socket);
        let pending = Arc::new(Pending::default());
        let receiver = tokio::spawn(receive(socket.clone(), pending.clone(), mtu));
        let connected = Arc::new(Connected {
            socket,
            pending,
            receiver,
        });
        *current = Some(connected.clone());
        Some(connected)
    }
}

impl Connected {
    fn reserve(
        &self,
        question: &Question<'_>,
    ) -> Option<(Outstanding<'_>, oneshot::Receiver<Vec<u8>>)> {
        // A reused port must still have unpredictable transaction IDs,
        // including for clients that send the same ID at the same time.
        let random = SystemRandom::new();
        let mut table = self.pending.lock().unwrap();
        if table.len() >= MAX_PENDING {
            return None;
        }
        for _ in 0..MAX_PENDING {
            let mut id = [0; 2];
            random.fill(&mut id).ok()?;
            if let Entry::Vacant(entry) = table.entry(id) {
                let (deliver, receiver) = oneshot::channel();
                entry.insert(Reply {
                    name: question.name().into(),
                    kind: question.kind(),
                    class: question.class(),
                    original_id: question.id(),
                    deliver: Some(deliver),
                });
                return Some((
                    Outstanding {
                        table: &self.pending,
                        id,
                    },
                    receiver,
                ));
            }
        }
        None
    }
}

async fn receive(socket: Arc<UdpSocket>, pending: Arc<Pending>, mtu: usize) {
    // Keep one extra byte to distinguish a datagram exceeding the TUN MTU.
    let mut buffer = vec![0; mtu + 1];
    const IDLE: Duration = Duration::from_secs(300);
    while let Ok(Ok(len)) = tokio::time::timeout(IDLE, socket.recv(&mut buffer)).await {
        let raw = &buffer[..len];
        let Some(question) = Question::parse(raw, true) else {
            continue;
        };
        let response = {
            let mut pending = pending.lock().unwrap();
            pending.get_mut(&question.id()).and_then(|reply| {
                (question.kind() == reply.kind
                    && question.class() == reply.class
                    && question.name().eq_ignore_ascii_case(&reply.name))
                .then_some(())?;
                Some((reply.original_id, reply.deliver.take()?))
            })
        };
        if let Some((id, deliver)) = response {
            let mut bytes = raw.to_vec();
            bytes[..2].copy_from_slice(&id);
            let _ = deliver.send(bytes);
        }
    }
    // Let current queries retry the next resolver without waiting out
    // their timeout when the socket reports a failure.
    for reply in pending.lock().unwrap().values_mut() {
        reply.deliver = None;
    }
}
