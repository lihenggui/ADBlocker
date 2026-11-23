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

//! Asks the kernel which UID owns a TCP connection or a UDP socket, over
//! sock_diag: one question about one socket, as the system's own
//! `ConnectivityManager.getConnectionOwnerUid` asks it.

#![allow(unsafe_code, reason = "netlink")]

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Mutex;

const SOCK_DIAG_BY_FAMILY: u16 = 20;
const NLM_F_REQUEST: u16 = 1;
const NLMSG_HDRLEN: usize = 16;
/// `struct inet_diag_req_v2`.
const REQUEST_LEN: usize = 56;
/// Where `idiag_uid` is in `struct inet_diag_msg`, and where its
/// attributes begin.
const UID_OFFSET: usize = 64;
const MESSAGE_LEN: usize = 72;
/// The socket's fwmark, given to a caller with CAP_NET_ADMIN.
const INET_DIAG_MARK: u16 = 15;
/// "Any socket with these addresses", in place of a socket's cookie.
const NO_COOKIE: u32 = u32::MAX;

/// The address as sock_diag takes it: 16 bytes, an IPv4 address in the
/// first four.
fn address(ip: IpAddr) -> [u8; 16] {
    let mut out = [0u8; 16];
    match ip {
        IpAddr::V4(v4) => out[..4].copy_from_slice(&v4.octets()),
        IpAddr::V6(v6) => out = v6.octets(),
    }
    out
}

#[derive(Clone, Copy)]
enum Protocol {
    Tcp,
    Udp,
}

/// The question: which socket has `local` as its own address and `remote`
/// as its peer's. `seq` comes back in the answer.
fn request(
    protocol: Protocol,
    local: SocketAddr,
    remote: SocketAddr,
    seq: u32,
) -> [u8; NLMSG_HDRLEN + REQUEST_LEN] {
    let mut m = [0u8; NLMSG_HDRLEN + REQUEST_LEN];
    let len = m.len() as u32;
    m[0..4].copy_from_slice(&len.to_ne_bytes());
    m[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    m[6..8].copy_from_slice(&NLM_F_REQUEST.to_ne_bytes());
    m[8..12].copy_from_slice(&seq.to_ne_bytes());
    let r = &mut m[NLMSG_HDRLEN..];
    r[0] = if local.is_ipv4() {
        libc::AF_INET
    } else {
        libc::AF_INET6
    } as u8;
    // UDP finds a socket as it would deliver a datagram to it: the source
    // is the peer, the destination the socket's own address.
    let (source, destination) = match protocol {
        Protocol::Tcp => (local, remote),
        Protocol::Udp => (remote, local),
    };
    r[1] = match protocol {
        Protocol::Tcp => libc::IPPROTO_TCP,
        Protocol::Udp => libc::IPPROTO_UDP,
    } as u8;
    // The states wanted are not asked for: they matter to a listing only.
    r[8..10].copy_from_slice(&source.port().to_be_bytes());
    r[10..12].copy_from_slice(&destination.port().to_be_bytes());
    r[12..28].copy_from_slice(&address(source.ip()));
    r[28..44].copy_from_slice(&address(destination.ip()));
    // Any interface (zero), any cookie.
    r[48..52].copy_from_slice(&NO_COOKIE.to_ne_bytes());
    r[52..56].copy_from_slice(&NO_COOKIE.to_ne_bytes());
    m
}

/// What the kernel's reply says, if it is the reply to question `seq`.
enum Answer {
    /// The socket, who owns it, and its fwmark where the kernel says.
    Uid(u32, Option<u32>),
    /// There is no such socket.
    None,
    /// The reply to another question, or to none: one that came too late
    /// for whoever asked it. Never taken for this one's: it would name the
    /// owner of somebody else's connection.
    Stale,
}

fn answer(reply: &[u8], seq: u32) -> Answer {
    let field = |at: usize, len: usize| reply.get(at..at + len);
    let (Some(kind), Some(got)) = (field(4, 2), field(8, 4)) else {
        return Answer::Stale;
    };
    if got != seq.to_ne_bytes() {
        return Answer::Stale;
    }
    if kind != SOCK_DIAG_BY_FAMILY.to_ne_bytes() {
        return Answer::None;
    }
    match field(NLMSG_HDRLEN + UID_OFFSET, 4) {
        Some(uid) => Answer::Uid(
            u32::from_ne_bytes([uid[0], uid[1], uid[2], uid[3]]),
            mark(reply),
        ),
        None => Answer::None,
    }
}

/// The `INET_DIAG_MARK` attribute that follows the message, if any.
fn mark(reply: &[u8]) -> Option<u32> {
    let length = u32::from_ne_bytes(reply.get(0..4)?.try_into().ok()?) as usize;
    let attributes = reply.get(NLMSG_HDRLEN + MESSAGE_LEN..length.min(reply.len()))?;
    let mut at = 0;
    while let Some(header) = attributes.get(at..at + 4) {
        let len = usize::from(u16::from_ne_bytes([header[0], header[1]]));
        let kind = u16::from_ne_bytes([header[2], header[3]]);
        if len < 4 {
            return None;
        }
        if kind == INET_DIAG_MARK {
            let value = attributes.get(at + 4..at + 8)?;
            return Some(u32::from_ne_bytes(value.try_into().ok()?));
        }
        at += len.next_multiple_of(4);
    }
    None
}

fn open() -> io::Result<OwnedFd> {
    // SAFETY: a system call with no pointers; the result is checked.
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_SOCK_DIAG,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: just opened, and owned by nothing else.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // The kernel answers before send() returns; the wait is bounded all
    // the same, so that a connection is never held up by this.
    let wait = libc::timeval {
        tv_sec: 1,
        tv_usec: 0,
    };
    // SAFETY: the timeval lives through the call and is passed with its size.
    unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&raw const wait).cast(),
            size_of::<libc::timeval>() as u32,
        );
    }
    Ok(fd)
}

/// The socket the questions are asked on, and the number of the last one.
/// One for the process, opened when first needed: a socket for every
/// connection would be three system calls more for each.
static SOCKET: Mutex<(Option<OwnedFd>, u32)> = Mutex::new((None, 0));

fn ask(
    protocol: Protocol,
    local: SocketAddr,
    remote: SocketAddr,
) -> io::Result<Option<(u32, Option<u32>)>> {
    // Held for the question and its answer: the kernel answers at once.
    let mut socket = SOCKET
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (fd, last) = &mut *socket;
    *last = last.wrapping_add(1);
    let seq = *last;
    let asked = (|| {
        let fd = match fd {
            Some(fd) => fd.as_raw_fd(),
            None => fd.insert(open()?).as_raw_fd(),
        };
        let m = request(protocol, local, remote, seq);
        // SAFETY: the request lives through the call and is passed with
        // its length.
        if unsafe { libc::send(fd, m.as_ptr().cast(), m.len(), 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // Replies that came too late for an earlier question are passed
        // over; there are few, and the wait for each is bounded.
        for _ in 0..8 {
            let mut reply = [0u8; 1024];
            // SAFETY: reads at most the length of `reply` into it.
            let n = unsafe { libc::recv(fd, reply.as_mut_ptr().cast(), reply.len(), 0) };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            match answer(&reply[..n as usize], seq) {
                Answer::Uid(uid, mark) => return Ok(Some((uid, mark))),
                Answer::None => return Ok(None),
                Answer::Stale => {}
            }
        }
        Ok(None)
    })();
    // A socket something went wrong with is not asked again.
    if asked.is_err() {
        *fd = None;
    }
    asked
}

/// The UID that owns the app's end of a connection: the socket whose own
/// address is `app` and whose peer is `server`.
pub fn uid_of(app: SocketAddr, server: SocketAddr) -> Option<u32> {
    ask(Protocol::Tcp, app, server)
        .ok()
        .flatten()
        .map(|(uid, _)| uid)
}

/// The UID that owns the UDP socket at `app` that sends to `server`, and
/// the socket's fwmark: the network netd chose for it, or none where the
/// kernel does not say.
pub fn udp_owner(app: SocketAddr, server: SocketAddr) -> Option<(u32, Option<u32>)> {
    ask(Protocol::Udp, app, server).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the kernel answers with when there is no such socket.
    const NLMSG_ERROR: u16 = 2;

    #[test]
    fn asks_about_one_tcp_socket_by_its_two_addresses() {
        let m = request(
            Protocol::Tcp,
            "10.0.2.16:40000".parse().unwrap(),
            "93.184.216.34:443".parse().unwrap(),
            7,
        );
        assert_eq!(u32::from_ne_bytes(m[0..4].try_into().unwrap()), 72);
        assert_eq!(u32::from_ne_bytes(m[8..12].try_into().unwrap()), 7);
        let r = &m[NLMSG_HDRLEN..];
        assert_eq!((r[0], r[1]), (libc::AF_INET as u8, libc::IPPROTO_TCP as u8));
        assert_eq!(&r[8..12], &[0x9c, 0x40, 0x01, 0xbb]);
        assert_eq!(&r[12..16], &[10, 0, 2, 16]);
        assert_eq!(&r[28..32], &[93, 184, 216, 34]);
        assert!(r[48..56].iter().all(|b| *b == 0xff));
    }

    #[test]
    fn asks_about_a_udp_socket_as_a_datagram_from_its_peer_would_find_it() {
        let m = request(
            Protocol::Udp,
            "127.0.0.1:40000".parse().unwrap(),
            "10.0.2.3:53".parse().unwrap(),
            9,
        );
        let r = &m[NLMSG_HDRLEN..];
        assert_eq!((r[0], r[1]), (libc::AF_INET as u8, libc::IPPROTO_UDP as u8));
        assert_eq!(&r[8..12], &[0x00, 0x35, 0x9c, 0x40]);
        assert_eq!(&r[12..16], &[10, 0, 2, 3]);
        assert_eq!(&r[28..32], &[127, 0, 0, 1]);
    }

    fn reply(kind: u16, seq: u32, uid: u32) -> [u8; NLMSG_HDRLEN + 72] {
        let mut reply = [0u8; NLMSG_HDRLEN + 72];
        reply[0..4].copy_from_slice(&((NLMSG_HDRLEN + 72) as u32).to_ne_bytes());
        reply[4..6].copy_from_slice(&kind.to_ne_bytes());
        reply[8..12].copy_from_slice(&seq.to_ne_bytes());
        reply[NLMSG_HDRLEN + UID_OFFSET..][..4].copy_from_slice(&uid.to_ne_bytes());
        reply
    }

    #[test]
    fn reads_the_uid_of_a_socket_and_nothing_of_an_error() {
        let found = reply(SOCK_DIAG_BY_FAMILY, 7, 10145);
        assert!(matches!(answer(&found, 7), Answer::Uid(10145, None)));
        assert!(matches!(
            answer(&reply(NLMSG_ERROR, 7, 10145), 7),
            Answer::None
        ));
        // Cut short of the uid.
        assert!(matches!(answer(&found[..40], 7), Answer::None));
        assert!(matches!(answer(&found[..6], 7), Answer::Stale));
    }

    #[test]
    fn reads_the_mark_among_the_attributes_that_follow() {
        let mut found = reply(SOCK_DIAG_BY_FAMILY, 7, 10145).to_vec();
        // An attribute of another kind, then the mark.
        found.extend_from_slice(&[8, 0, 1, 0, 0, 0, 0, 0]);
        found.extend_from_slice(&8u16.to_ne_bytes());
        found.extend_from_slice(&INET_DIAG_MARK.to_ne_bytes());
        found.extend_from_slice(&0xf0065u32.to_ne_bytes());
        let length = found.len() as u32;
        found[0..4].copy_from_slice(&length.to_ne_bytes());
        assert!(matches!(
            answer(&found, 7),
            Answer::Uid(10145, Some(0xf0065))
        ));
        // Cut short of the mark's value.
        found[0..4].copy_from_slice(&(length - 2).to_ne_bytes());
        assert!(matches!(answer(&found, 7), Answer::Uid(10145, None)));
    }

    #[test]
    fn the_answer_to_another_question_is_never_taken_for_this_ones() {
        let late = reply(SOCK_DIAG_BY_FAMILY, 6, 10999);
        assert!(matches!(answer(&late, 7), Answer::Stale));
    }

    #[test]
    fn finds_the_owner_of_a_connection_of_this_process_time_after_time() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        // SAFETY: cannot fail and touches no memory.
        let me = unsafe { libc::getuid() };
        for _ in 0..50 {
            let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let uid = uid_of(client.local_addr().unwrap(), client.peer_addr().unwrap());
            assert_eq!(uid, Some(me));
        }
        // And of one that is not there.
        let gone = "192.0.2.1:9".parse().unwrap();
        assert_eq!(uid_of(gone, listener.local_addr().unwrap()), None);
    }

    #[test]
    fn finds_the_owner_of_a_udp_socket_of_this_process() {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        client.connect(server.local_addr().unwrap()).unwrap();
        // SAFETY: cannot fail and touches no memory.
        let me = unsafe { libc::getuid() };
        let owner = udp_owner(client.local_addr().unwrap(), server.local_addr().unwrap());
        assert_eq!(owner.map(|(uid, _)| uid), Some(me));
        let gone = "127.0.0.1:9".parse().unwrap();
        assert_eq!(udp_owner(gone, server.local_addr().unwrap()), None);
    }
}
