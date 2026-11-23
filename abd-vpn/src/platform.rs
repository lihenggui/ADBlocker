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

//! Bounded system callbacks and sockets kept out of the VPN.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::net::SocketAddr;
use std::os::fd::{AsRawFd, RawFd};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::{Semaphore, SemaphorePermit};

/// How long a server may take to accept a connection.
const CONNECT_TIME: Duration = Duration::from_secs(10);
pub(crate) const CALLBACK_TIME: Duration = Duration::from_millis(250);

/// Keeps a socket of ours out of the VPN.
pub type Protect = Arc<dyn Fn(RawFd) -> bool + Send + Sync>;

/// What the app answers for the system.
pub trait Platform: Send + Sync {
    /// The UID that owns the connection from the first address, an app's,
    /// to the second, a server's.
    fn owner(&self, protocol: i32, app: SocketAddr, server: SocketAddr) -> Option<u32>;
    /// The UID of a package, if it is installed.
    fn uid_of(&self, package: &str) -> Option<u32>;
    /// Confirmed personal UIDs and their current stable selection identities.
    fn personal_app_identities(&self, _exclusions: &BTreeSet<&str>) -> BTreeMap<u32, String> {
        BTreeMap::new()
    }
    /// Whether the user has the authority among the certificates the
    /// device trusts.
    fn ca_installed(&self) -> bool;
    /// Keeps a socket of ours out of the VPN, so that what it sends is
    /// not brought back to us.
    fn protect(&self, socket: RawFd) -> bool;
}

/// Bound owner lookups without releasing a slot while a timed-out Binder call still runs.
pub(crate) async fn lookup_owner<T: Send + 'static>(
    slots: &'static Semaphore,
    query: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    let slot = slots.try_acquire().ok()?;
    tokio::time::timeout(CALLBACK_TIME, call(slot, query))
        .await
        .ok()?
}

async fn call<T: Send + 'static>(
    slot: SemaphorePermit<'static>,
    callback: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    tokio::task::spawn_blocking(move || {
        let _slot = slot;
        callback()
    })
    .await
    .ok()?
}

async fn platform_call<T: Send + 'static>(
    slots: &'static Semaphore,
    callback: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    tokio::time::timeout(CALLBACK_TIME, async {
        let slot = slots.acquire().await.ok()?;
        call(slot, callback).await
    })
    .await
    .ok()?
}

pub(crate) async fn protect_socket<T: AsRawFd + Send + 'static>(
    protect: Protect,
    socket: T,
) -> std::io::Result<T> {
    static PROTECTS: Semaphore = Semaphore::const_new(8);
    // A timed-out Binder call can still be using this descriptor. Keep
    // ownership with the call so its number cannot be reused underneath it.
    platform_call(&PROTECTS, move || {
        protect(socket.as_raw_fd()).then_some(socket)
    })
    .await
    .ok_or_else(|| std::io::Error::other("the socket could not be kept out of the VPN"))
}

pub(crate) fn protector(platform: Arc<dyn Platform>) -> Protect {
    Arc::new(move |socket| platform.protect(socket))
}

/// Connects to a server.
pub type Connect = Arc<
    dyn Fn(SocketAddr) -> Pin<Box<dyn Future<Output = std::io::Result<TcpStream>> + Send>>
        + Send
        + Sync,
>;

pub(crate) async fn tcp_owner(
    platform: &Arc<dyn Platform>,
    app: SocketAddr,
    server: SocketAddr,
) -> Option<u32> {
    static OWNERS: Semaphore = Semaphore::const_new(8);
    let platform = platform.clone();
    platform_call(&OWNERS, move || {
        platform.owner(libc::IPPROTO_TCP, app, server)
    })
    .await
}

/// Connects to a server from outside the VPN. The connection is the
/// app's own to nobody but us: the system counts it as ours.
pub(crate) fn connect(protect: Protect) -> Connect {
    Arc::new(move |dst: SocketAddr| {
        let protect = protect.clone();
        Box::pin(async move {
            let socket = if dst.is_ipv4() {
                TcpSocket::new_v4()?
            } else {
                TcpSocket::new_v6()?
            };
            let socket = protect_socket(protect, socket).await?;
            socket.set_nodelay(true)?;
            match tokio::time::timeout(CONNECT_TIME, socket.connect(dst)).await {
                Ok(connected) => connected,
                Err(_) => Err(std::io::ErrorKind::TimedOut.into()),
            }
        })
    })
}
