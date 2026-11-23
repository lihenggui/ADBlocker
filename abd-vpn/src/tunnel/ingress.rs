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

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::Instant;

use super::{Reach, Tun};
use crate::datagram;
use crate::dns::{self, Resolver};
use crate::platform::Protect;
use crate::stack::{Ends, Stack};
use crate::udp::{self, Policy};

/// The most connections waiting at a time to be found reachable, and how
/// long one may: longer, and the app is left to its own patience.
const MAX_ASKED: usize = 256;
const REACH_TIME: Duration = Duration::from_secs(12);

pub(super) const PACKET_BATCH: usize = 32;

pub(super) type ReachResult = (Ends, io::Result<()>);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Turn {
    UdpOnly,
    Tcp,
}

enum Route {
    Dns,
    Web,
    Udp,
    Refuse,
    Stack,
    Reach(Reach, Ends),
}

fn classify(
    packet: &[u8],
    stack: &Stack,
    dns: Option<&mpsc::Sender<Vec<u8>>>,
    reach: Option<&Reach>,
) -> Route {
    if let Some(datagram) = datagram::parse(packet) {
        return if dns.is_some() && Resolver::answers(datagram.server) {
            Route::Dns
        } else if datagram.server.port() == 443 {
            Route::Web
        } else {
            Route::Udp
        };
    }
    match (reach, stack.begins(packet)) {
        (_, Some((_, server))) if dns.is_some() && Resolver::refuses(server) => Route::Refuse,
        (_, Some((_, server))) if dns.is_some() && Resolver::answers(server) => Route::Stack,
        (Some(reach), Some(ends)) => Route::Reach(reach.clone(), ends),
        _ => Route::Stack,
    }
}

pub(super) struct Ingress {
    // Dropping the join set cancels the relay together with the TUN.
    _relay: JoinSet<()>,
    udp: mpsc::Sender<Vec<u8>>,
    web: mpsc::Sender<Vec<u8>>,
    dns: Option<mpsc::Sender<Vec<u8>>>,
    reach: Option<Reach>,
    // The beginnings of connections whose servers are being checked.
    asked: HashMap<Ends, Vec<u8>>,
    found: mpsc::Sender<ReachResult>,
}

impl Ingress {
    pub(super) fn new(
        mtu: usize,
        protect: Protect,
        policy: Policy,
        dns: Option<&Arc<Resolver>>,
        reach: Option<Reach>,
    ) -> (Self, mpsc::Receiver<Vec<u8>>, mpsc::Receiver<ReachResult>) {
        let (udp_tx, udp_rx) = mpsc::channel(64);
        let (web_tx, web_rx) = mpsc::channel(64);
        let (answered, answers) = mpsc::channel(16);
        let mut relay = tokio::task::JoinSet::new();
        // A slow Android owner lookup on 443 must not hold up DNS or other
        // UDP. A QUIC Initial on another port still waits for its own in
        // the relay of other UDP, which keeps an app port on one socket.
        relay.spawn(udp::relay(
            udp_rx,
            answered.clone(),
            policy.clone(),
            protect.clone(),
            mtu,
        ));
        relay.spawn(udp::relay(web_rx, answered.clone(), policy, protect, mtu));
        let dns_tx = dns.map(|resolver| {
            let (send, queries) = mpsc::channel(dns::MAX_PENDING);
            relay.spawn(resolver.clone().udp(queries, answered, mtu));
            send
        });
        let (found, finds) = mpsc::channel(64);
        let ingress = Self {
            _relay: relay,
            udp: udp_tx,
            web: web_tx,
            dns: dns_tx,
            reach,
            asked: HashMap::new(),
            found,
        };
        (ingress, answers, finds)
    }

    /// Handles the first packet and any ready successors. If EOF arrives
    /// during a batch, the caller still flushes work from the packets taken.
    pub(super) async fn batch(
        &mut self,
        stack: &mut Stack,
        tun: &mut impl Tun,
        packet: &mut [u8],
        n: usize,
        deadline: Option<Instant>,
    ) -> io::Result<(Turn, bool)> {
        let mut ended = false;
        let mut turn = self.packet(stack, &packet[..n]);
        for _ in 1..PACKET_BATCH {
            if deadline.is_some_and(|at| Instant::now() >= at) {
                break;
            }
            let ready = tokio::select! {
                biased;
                got = tun.recv(packet) => Some(got?),
                () = std::future::ready(()) => None,
            };
            match ready {
                None => break,
                Some(0) => {
                    ended = true;
                    break;
                }
                Some(n) => {
                    tokio::task::consume_budget().await;
                    if self.packet(stack, &packet[..n]) == Turn::Tcp {
                        turn = Turn::Tcp;
                    }
                }
            }
        }
        Ok((turn, ended))
    }

    fn packet(&mut self, stack: &mut Stack, packet: &[u8]) -> Turn {
        let sender = match classify(packet, stack, self.dns.as_ref(), self.reach.as_ref()) {
            Route::Dns => self.dns.as_ref(),
            Route::Web => Some(&self.web),
            Route::Udp => Some(&self.udp),
            Route::Refuse => {
                stack.refuse(packet);
                return Turn::Tcp;
            }
            Route::Stack => {
                stack.receive(packet);
                return Turn::Tcp;
            }
            Route::Reach(reach, ends) => {
                self.begin_reach(stack, packet, reach, ends);
                return Turn::Tcp;
            }
        };
        if let Some(sender) = sender
            && let Ok(slot) = sender.try_reserve()
        {
            slot.send(packet.to_vec());
        }
        Turn::UdpOnly
    }

    fn begin_reach(&mut self, stack: &mut Stack, packet: &[u8], reach: Reach, ends: Ends) {
        // Sent again by the app while the server is being checked, it is
        // the same connection.
        if self.asked.contains_key(&ends) {
            return;
        }
        if self.asked.len() >= MAX_ASKED {
            stack.refuse(packet);
            return;
        }
        self.asked.insert(ends, packet.to_vec());
        let found = self.found.clone();
        tokio::spawn(async move {
            let there = tokio::time::timeout(REACH_TIME, reach(ends.0, ends.1));
            let there = match there.await {
                Ok(there) => there,
                Err(_) => Err(io::ErrorKind::TimedOut.into()),
            };
            let _ = found.send((ends, there)).await;
        });
    }

    pub(super) fn reach_done(&mut self, stack: &mut Stack, (ends, there): ReachResult) {
        if let Some(beginning) = self.asked.remove(&ends) {
            match there {
                // Answered in the server's place, as it would
                // have answered.
                Ok(()) => stack.receive(&beginning),
                // No answer came: none is given, and the app
                // waits as long as it would have.
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {}
                // Refused, or no way there: said at once, while
                // the app can still go elsewhere.
                Err(_) => stack.refuse(&beginning),
            }
        }
    }
}
