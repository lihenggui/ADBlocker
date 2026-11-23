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

//! The two system calls lookup capture needs that no library makes: the
//! device that anchors the routes, and a datagram with the address it was
//! sent to before TPROXY took it.

#![allow(unsafe_code, reason = "an ioctl and recvmsg")]

use std::fs::{File, OpenOptions};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};

/// A TUN device of this process's own, `IFF_TUN | IFF_NO_PI`, held by the
/// descriptor returned: it is not persistent, and when the descriptor is
/// closed, by us or by the end of the process, the device is gone and so
/// is every route through it.
pub(super) fn open_tun(name: &str) -> io::Result<OwnedFd> {
    // Android's node first; std opens it close-on-exec, so that no process
    // we start keeps the device after us.
    let file = ["/dev/tun", "/dev/net/tun"]
        .iter()
        .map(|path| OpenOptions::new().read(true).write(true).open(path))
        .reduce(|first, next| first.or(next))
        .unwrap_or_else(|| Err(io::ErrorKind::NotFound.into()))?;
    set_iff(&file, name)?;
    Ok(file.into())
}

fn set_iff(file: &File, name: &str) -> io::Result<()> {
    // `struct ifreq`: the name, then the flags. 40 bytes is its size on
    // 64-bit systems and more than it on 32-bit ones.
    let mut request = [0u8; 40];
    let name = name.as_bytes();
    if name.len() >= libc::IFNAMSIZ {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    request[..name.len()].copy_from_slice(name);
    let flags = (libc::IFF_TUN | libc::IFF_NO_PI) as libc::c_short;
    request[libc::IFNAMSIZ..libc::IFNAMSIZ + 2].copy_from_slice(&flags.to_ne_bytes());
    // SAFETY: TUNSETIFF reads and writes one `struct ifreq`, which fits in
    // the buffer; the buffer lives through the call.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), libc::TUNSETIFF, request.as_mut_ptr()) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// One datagram as it came: how much of the buffer it filled, who sent
/// it, and where it was going.
pub(super) struct Received {
    pub(super) len: usize,
    pub(super) from: SocketAddr,
    pub(super) to: Option<SocketAddr>,
}

/// Reads one datagram into `buffer` from a socket with
/// `IP(V6)_RECVORIGDSTADDR` on. One that does not fit is an error.
pub(super) fn receive(fd: BorrowedFd<'_>, buffer: &mut [u8]) -> io::Result<Received> {
    let mut name = [0u8; 128];
    let mut control = [0u8; 256];
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    // SAFETY: an all-zero msghdr is a valid empty one.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_name = name.as_mut_ptr().cast();
    msg.msg_namelen = name.len() as _;
    msg.msg_iov = &raw mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = control.len() as _;
    // SAFETY: every pointer in `msg` is to a buffer of ours that lives
    // through the call, with its length; the kernel writes within them.
    let n = unsafe { libc::recvmsg(fd.as_raw_fd(), &raw mut msg, 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    if msg.msg_flags & libc::MSG_TRUNC != 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let name_len = (msg.msg_namelen as usize).min(name.len());
    let control_len = (msg.msg_controllen as usize).min(control.len());
    Ok(Received {
        len: n as usize,
        from: socket_address(&name[..name_len]).ok_or(io::ErrorKind::InvalidData)?,
        to: original_destination(&control[..control_len]),
    })
}

/// A `sockaddr_in` or `sockaddr_in6` as the kernel wrote it.
fn socket_address(raw: &[u8]) -> Option<SocketAddr> {
    let family = i32::from(u16::from_ne_bytes(raw.get(0..2)?.try_into().ok()?));
    let port = u16::from_be_bytes(raw.get(2..4)?.try_into().ok()?);
    match family {
        libc::AF_INET => {
            let ip: [u8; 4] = raw.get(4..8)?.try_into().ok()?;
            Some(SocketAddrV4::new(Ipv4Addr::from(ip), port).into())
        }
        libc::AF_INET6 => {
            let flow = u32::from_ne_bytes(raw.get(4..8)?.try_into().ok()?);
            let ip: [u8; 16] = raw.get(8..24)?.try_into().ok()?;
            let scope = u32::from_ne_bytes(raw.get(24..28)?.try_into().ok()?);
            Some(SocketAddrV6::new(Ipv6Addr::from(ip), port, flow, scope).into())
        }
        _ => None,
    }
}

/// The original destination among the control messages, if one is there.
fn original_destination(control: &[u8]) -> Option<SocketAddr> {
    const WORD: usize = size_of::<usize>();
    let header = size_of::<libc::cmsghdr>().next_multiple_of(WORD);
    let mut at = 0;
    while let Some(fields) = control.get(at..at + header) {
        let len = usize::from_ne_bytes(fields[..WORD].try_into().ok()?);
        let level = i32::from_ne_bytes(fields[WORD..WORD + 4].try_into().ok()?);
        let kind = i32::from_ne_bytes(fields[WORD + 4..WORD + 8].try_into().ok()?);
        if len < header {
            return None;
        }
        if matches!(
            (level, kind),
            (libc::SOL_IP, libc::IP_ORIGDSTADDR) | (libc::SOL_IPV6, libc::IPV6_ORIGDSTADDR)
        ) {
            return socket_address(control.get(at + header..at + len)?);
        }
        at += len.next_multiple_of(WORD);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd as _;

    use super::*;

    fn sockaddr_in(ip: [u8; 4], port: u16) -> Vec<u8> {
        let mut raw = (libc::AF_INET as u16).to_ne_bytes().to_vec();
        raw.extend_from_slice(&port.to_be_bytes());
        raw.extend_from_slice(&ip);
        raw.extend_from_slice(&[0; 8]);
        raw
    }

    fn cmsg(level: i32, kind: i32, data: &[u8]) -> Vec<u8> {
        const WORD: usize = size_of::<usize>();
        let header = size_of::<libc::cmsghdr>().next_multiple_of(WORD);
        let mut raw = (header + data.len()).to_ne_bytes().to_vec();
        raw.extend_from_slice(&level.to_ne_bytes());
        raw.extend_from_slice(&kind.to_ne_bytes());
        raw.resize(header, 0);
        raw.extend_from_slice(data);
        raw.resize(raw.len().next_multiple_of(WORD), 0);
        raw
    }

    #[test]
    fn the_original_destination_is_found_among_other_control_messages() {
        let orig = sockaddr_in([10, 0, 2, 3], 53);
        let mut control = cmsg(libc::SOL_IP, libc::IP_PKTINFO, &[0; 12]);
        control.extend(cmsg(libc::SOL_IP, libc::IP_ORIGDSTADDR, &orig));
        assert_eq!(
            original_destination(&control),
            Some("10.0.2.3:53".parse().unwrap())
        );
        assert_eq!(original_destination(&control[..control.len() - 20]), None);
        assert_eq!(original_destination(&[]), None);
    }

    #[test]
    fn an_ipv6_destination_keeps_its_port_and_scope() {
        let mut raw = (libc::AF_INET6 as u16).to_ne_bytes().to_vec();
        raw.extend_from_slice(&53u16.to_be_bytes());
        raw.extend_from_slice(&0u32.to_ne_bytes());
        raw.extend_from_slice(&"2001:db8::53".parse::<Ipv6Addr>().unwrap().octets());
        raw.extend_from_slice(&3u32.to_ne_bytes());
        let control = cmsg(libc::SOL_IPV6, libc::IPV6_ORIGDSTADDR, &raw);
        let SocketAddr::V6(found) = original_destination(&control).unwrap() else {
            panic!("not IPv6");
        };
        assert_eq!(found.to_string(), "[2001:db8::53%3]:53");
        assert_eq!(socket_address(&raw[..20]), None);
        let mut unknown = raw.clone();
        unknown[0..2].copy_from_slice(&(libc::AF_UNIX as u16).to_ne_bytes());
        assert_eq!(socket_address(&unknown), None);
    }

    #[test]
    fn a_datagram_is_read_with_its_sender_and_without_a_destination_when_not_asked() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .send_to(b"query", socket.local_addr().unwrap())
            .unwrap();
        let mut buffer = [0; 16];
        let got = receive(socket.as_fd(), &mut buffer).unwrap();
        assert_eq!(&buffer[..got.len], b"query");
        assert_eq!(got.from, sender.local_addr().unwrap());
        assert_eq!(got.to, None);
        sender
            .send_to(&[1; 17], socket.local_addr().unwrap())
            .unwrap();
        assert!(receive(socket.as_fd(), &mut buffer).is_err());
    }
}
