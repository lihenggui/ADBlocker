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

//! The sockets on both sides of an intercepted connection.
//!
//! Inbound, TPROXY hands over the app's connection with its destination as
//! our local address. Outbound, the connection is the app's own as far as
//! the kernel can tell: it carries the app's fwmark (the network netd chose
//! for it), and its socket is created with the app's credentials.
//!
//! Everything that asks whose a socket is gets the app for an answer: the
//! routing rules, netd's per-app firewall and data counters, a VPN's choice
//! of apps, and the rules of any other module. A transparent proxy the user
//! runs treats the connection as it would have treated the app's, without
//! this module knowing anything about it.

#![allow(unsafe_code, reason = "socket options the standard library lacks")]

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};

use std::time::Duration;

use anyhow::{Context, Result};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::{TcpListener, TcpSocket, TcpStream, UdpSocket};

use abd_engine::mitm::Origin;

/// Set by the mangle OUTPUT rule on packets to intercept; routes them to lo.
///
/// Both marks are bits netd leaves alone, and not the ones other root
/// modules are known to use for the same purpose (the transparent proxy
/// modules mark with 0x1000000): a packet marked by one of them must not
/// be taken for one of ours.
pub const MARK_CAPTURE: u32 = 0x0400_0000;
/// Set on our own outbound sockets so the OUTPUT rule leaves them alone,
/// and so that they are never delivered back to us, whoever marks them.
pub const MARK_BYPASS: u32 = 0x0800_0000;
/// The bits netd owns: netId, explicit, protect, permission, billing.
const MARK_NETD: u32 = 0x001f_ffff;

/// The two listeners TPROXY delivers to, on a port of the kernel's
/// choosing: a fixed one could be taken, by any app, while no daemon runs.
pub fn listeners() -> Result<(TcpListener, TcpListener, u16)> {
    let mut last = None;
    // The port free for IPv4 may be taken for IPv6: then another.
    for _ in 0..8 {
        let v4 = listener((Ipv4Addr::LOCALHOST, 0).into())?;
        let port = v4.local_addr()?.port();
        match listener((Ipv6Addr::LOCALHOST, port).into()) {
            Ok(v6) => return Ok((v4, v6, port)),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("no port")))
}

/// A transparent listener at `addr`, for TPROXY to deliver to.
pub fn listener(addr: SocketAddr) -> Result<TcpListener> {
    let s = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    s.set_reuse_address(true)?;
    match addr {
        SocketAddr::V4(_) => s.set_ip_transparent_v4(true)?,
        SocketAddr::V6(_) => {
            s.set_only_v6(true)?;
            setsockopt(&s, libc::SOL_IPV6, libc::IPV6_TRANSPARENT, 1)?;
        }
    }
    s.set_nonblocking(true)?;
    s.bind(&addr.into())
        .with_context(|| format!("bind {addr}"))?;
    s.listen(1024)?;
    Ok(TcpListener::from_std(s.into())?)
}

pub(crate) fn setsockopt(s: &Socket, level: i32, name: i32, val: i32) -> std::io::Result<()> {
    // SAFETY: the value is an i32 that lives through the call, passed
    // with its size.
    let r = unsafe {
        libc::setsockopt(
            s.as_raw_fd(),
            level,
            name,
            (&raw const val).cast(),
            size_of::<i32>() as u32,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// The fwmark the accepted socket took from the app's SYN
/// (net.ipv4.tcp_fwmark_accept, which Android turns on).
pub fn mark_of(stream: &TcpStream) -> u32 {
    socket2::SockRef::from(stream).mark().unwrap_or(0)
}

/// What an accepted connection is, if it is one to carry.
///
/// The listener is on the loopback interface, where every app can reach
/// it. Only the rules give a packet the mark, which no app can set, and a
/// destination that is somewhere else; and of what they delivered, only
/// an app's connection is one (`Origin::of`).
pub fn admitted(dst: SocketAddr, mark: u32, uid: Option<u32>) -> Option<Origin> {
    // Marked for us, and not a connection of our own come back.
    let by_the_rules = mark & (MARK_CAPTURE | MARK_BYPASS) == MARK_CAPTURE;
    by_the_rules.then(|| Origin::of(dst, mark, uid)).flatten()
}

/// Connects to `dst` as the app would have.
///
/// The error is returned as the system gave it, so that the caller can
/// tell "no way there" from the rest.
pub async fn connect_as(dst: SocketAddr, app_mark: u32, uid: u32) -> std::io::Result<TcpStream> {
    dial(dst, app_mark, Some(uid), CONNECT_TIME).await
}

/// Connects to `dst` to learn whether it can be reached at all: as the
/// app that reached it before, since no connection to an address an app
/// chose is made as anybody else, or by a way that is not the app's.
pub async fn probe(dst: SocketAddr, app_mark: u32, uid: u32) -> std::io::Result<()> {
    dial(dst, app_mark, Some(uid), PROBE_TIME).await.map(drop)
}

/// A UDP socket of the app's own, connected to `dst`: what `connect_as`
/// is for a datagram the app was sending there.
pub fn udp_as(dst: SocketAddr, app_mark: u32, uid: u32) -> std::io::Result<UdpSocket> {
    let s = as_app(uid, || {
        Socket::new(Domain::for_address(dst), Type::DGRAM, Some(Protocol::UDP))
    })?;
    s.set_nonblocking(true)?;
    s.set_mark((app_mark & MARK_NETD) | MARK_BYPASS)?;
    s.connect(&dst.into())?;
    UdpSocket::from_std(s.into())
}

/// How long a server may take to accept a connection.
const CONNECT_TIME: Duration = Duration::from_secs(10);
const PROBE_TIME: Duration = Duration::from_secs(5);

/// Runs `make` with the calling thread's file-system identity set to
/// `uid`, which is the identity a new socket takes for good: its owner as
/// the routing rules see it, and its creator as the packet filter does.
///
/// Only this thread is changed, and only until `make` returns; nothing
/// else runs on it meanwhile, as there is no await in between. If the
/// identity cannot be put back the process ends: a thread left as an app
/// would do everything after as that app.
fn as_app<T>(uid: u32, make: impl FnOnce() -> T) -> T {
    // The raw calls: the C library's may be made for every thread. Each
    // returns the identity there was before it.
    // SAFETY: system calls on integers, which touch no memory.
    let (was_gid, was_uid) = unsafe {
        (
            libc::syscall(libc::SYS_setfsgid, uid),
            libc::syscall(libc::SYS_setfsuid, uid),
        )
    };
    let made = make();
    // SAFETY: as above. Set a second time, each says whether the first
    // took: these calls report no failure any other way.
    let restored = unsafe {
        libc::syscall(libc::SYS_setfsuid, was_uid);
        libc::syscall(libc::SYS_setfsgid, was_gid);
        libc::syscall(libc::SYS_setfsuid, was_uid) == was_uid
            && libc::syscall(libc::SYS_setfsgid, was_gid) == was_gid
    };
    if !restored {
        eprintln!("could not return to our own identity after making a socket; ending");
        std::process::abort();
    }
    made
}

async fn dial(
    dst: SocketAddr,
    mark: u32,
    uid: Option<u32>,
    wait: Duration,
) -> std::io::Result<TcpStream> {
    let new = || Socket::new(Domain::for_address(dst), Type::STREAM, Some(Protocol::TCP));
    let s = match uid {
        Some(uid) => as_app(uid, new)?,
        None => new()?,
    };
    s.set_nonblocking(true)?;
    s.set_mark((mark & MARK_NETD) | MARK_BYPASS)?;
    s.set_tcp_nodelay(true)?;
    // SAFETY: the descriptor was just released by its only owner, and is
    // a TCP socket set non-blocking above.
    let sock = unsafe { TcpSocket::from_raw_fd(s.into_raw_fd()) };
    match tokio::time::timeout(wait, sock.connect(dst)).await {
        Ok(result) => result,
        Err(_) => Err(std::io::ErrorKind::TimedOut.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marks_are_bits_of_nobody_else() {
        // netd's, and the bit the transparent proxy modules mark with.
        for taken in [MARK_NETD, 0x0100_0000, 0x8000_0000] {
            assert_eq!(MARK_CAPTURE & taken, 0);
            assert_eq!(MARK_BYPASS & taken, 0);
        }
        assert_eq!(MARK_CAPTURE & MARK_BYPASS, 0);
    }

    #[test]
    fn only_what_the_rules_delivered_for_an_app_is_carried() {
        let server: SocketAddr = "93.184.216.34:443".parse().unwrap();
        let by_rules = MARK_CAPTURE | 0x64;
        let origin = admitted(server, by_rules, Some(10145)).unwrap();
        assert_eq!(
            (origin.dst(), origin.mark(), origin.uid()),
            (server, by_rules, 10145)
        );
        // An app of a second user.
        assert!(admitted(server, by_rules, Some(1_010_145)).is_some());

        // Connected to the listener directly: no mark, and the listener
        // itself for a destination.
        let listener: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        assert_eq!(admitted(listener, 0x64, Some(10145)), None);
        assert_eq!(admitted(listener, by_rules, Some(10145)), None);
        assert_eq!(
            admitted("[::1]:40000".parse().unwrap(), by_rules, Some(10145)),
            None
        );
        assert_eq!(admitted(server, 0x64, Some(10145)), None);
        // One of the daemon's own connections, marked by somebody else.
        let own = by_rules | MARK_BYPASS;
        assert_eq!(admitted(server, own, Some(10145)), None);

        // Nobody's, root's, the system's.
        assert_eq!(admitted(server, by_rules, None), None);
        assert_eq!(admitted(server, by_rules, Some(0)), None);
        assert_eq!(admitted(server, by_rules, Some(1000)), None);
        assert_eq!(admitted(server, by_rules, Some(1_001_000)), None);
    }

    #[test]
    fn the_thread_is_itself_again_after_making_something_as_an_app() {
        // SAFETY: getuid cannot fail and touches no memory.
        let me = unsafe { libc::getuid() };
        // Without root the identity cannot be changed to another's, and
        // changing it to one's own is what is left to try.
        let made = as_app(me, || 7);
        assert_eq!(made, 7);
        // SAFETY: returns the identity and sets the same.
        assert_eq!(unsafe { libc::syscall(libc::SYS_setfsuid, me) }, me as i64);
    }
}
