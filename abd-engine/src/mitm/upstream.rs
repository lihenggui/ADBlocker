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

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::time::Duration;

use anyhow::{Context as _, Result};
use hyper::body::Incoming;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};

use super::{Ctx, Origin};
use crate::relay;

/// What a connection can tell the main loop.
pub enum Event {
    /// Several IPv6 connections in a row found no way to their server.
    /// That is how a network without IPv6 looks, and also an app
    /// connecting to addresses it knows lead nowhere: for the main loop
    /// to find out which.
    V6Suspect,
}

/// Connections in a row that found no way, with no success between,
/// before IPv6 is doubted.
const V6_FAILURES: u32 = 3;

#[derive(Default)]
pub struct Ipv6Health {
    /// IPv6 connections in a row that found no way to the server.
    failures: AtomicU32,
    /// An IPv6 server that was reached, to try again when it seems none can.
    reached: std::sync::Mutex<Option<Origin>>,
    events: Option<mpsc::UnboundedSender<Event>>,
}

impl Ipv6Health {
    pub fn new(events: mpsc::UnboundedSender<Event>) -> Self {
        Self {
            events: Some(events),
            ..Self::default()
        }
    }

    pub fn last_reached(&self) -> Option<Origin> {
        *self.reached.lock().unwrap()
    }

    pub fn reset(&self) {
        self.failures.store(0, Relaxed);
    }

    fn observed(&self, origin: Origin, result: &std::io::Result<TcpStream>) {
        let Some(events) = &self.events else { return };
        if !origin.dst.is_ipv6() {
            return;
        }
        match result {
            Ok(_) => {
                self.reset();
                *self.reached.lock().unwrap() = Some(origin);
            }
            // A server that refuses, or does not answer, says nothing
            // about the network.
            Err(e) if no_way(e) => {
                if self.failures.fetch_add(1, Relaxed) + 1 == V6_FAILURES {
                    let _ = events.send(Event::V6Suspect);
                }
            }
            Err(_) => {}
        }
    }
}

/// How long either side may take over its handshake with us.
pub(super) const HANDSHAKE_TIME: Duration = Duration::from_secs(15);

/// How long one direction of a connection handed on may go on after the
/// other has ended. Without an end to it, a server that never closes
/// would keep the connection of an app long gone.
pub(super) const LINGER_TIME: Duration = Duration::from_secs(120);

/// Whether a connection failed for want of any way to its destination,
/// rather than at the destination.
pub(super) fn no_way(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENETUNREACH | libc::EHOSTUNREACH)
    )
}

/// Connects to the server; if that fails, resets the app's connection, the
/// nearest thing left to the error its own connect() would have returned.
///
/// An accepted connection cannot be un-accepted. Where the network has an
/// IPv6 route that leads nowhere, the app sees its connection made and then
/// reset, and does not fall back to IPv4 as it does when connect() itself
/// fails: the main loop is told when it looks like that.
pub(super) async fn connect(ctx: &Ctx, app: &TcpStream, origin: Origin) -> Result<TcpStream> {
    let r = dial(ctx, origin).await;
    if r.is_err() {
        socket2::SockRef::from(app)
            .set_linger(Some(Duration::ZERO))
            .ok();
    }
    r.context("connect")
}

pub(super) async fn dial(ctx: &Ctx, origin: Origin) -> std::io::Result<TcpStream> {
    let result = (ctx.hooks.dial)(origin).await;
    ctx.ipv6.observed(origin, &result);
    result
}

/// Hands a connection on untouched.
pub(super) async fn splice(ctx: &Ctx, app: TcpStream, origin: Origin) -> Result<()> {
    let up = connect(ctx, &app, origin).await?;
    relay::join(app, up, LINGER_TIME).await;
    Ok(())
}

/// How long the app is given to take what was already on its way to it
/// when the server's side ended.
const DRAIN_TIME: Duration = Duration::from_secs(30);

/// Serves the app's side of a decrypted connection until it ends, and
/// says whether it ended of itself.
///
/// When the server's side ends first, so does the app's, which must not be
/// left sending on a connection whose other half is gone; but not before
/// it has what the server sent last. A server that ends the connection
/// with its answer is seen to end before that answer has been passed on.
pub(super) async fn carry<C: Future>(
    conn: C,
    upstream: impl Future,
    finish: impl FnOnce(Pin<&mut C>),
) -> bool {
    tokio::pin!(conn);
    tokio::select! {
        biased;
        _ = conn.as_mut() => return true,
        _ = upstream => {}
    }
    finish(conn.as_mut());
    let _ = tokio::time::timeout(DRAIN_TIME, conn).await;
    false
}

pub(super) enum Upstream {
    H1(Mutex<hyper::client::conn::http1::SendRequest<Incoming>>),
    H2(hyper::client::conn::http2::SendRequest<Incoming>),
}
