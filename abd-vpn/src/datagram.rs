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

//! Whole UDP datagrams shared by DNS, ingress and the UDP relay.

use std::net::SocketAddr;

use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    IpAddress, IpProtocol, Ipv4Packet, Ipv4Repr, Ipv6Packet, Ipv6Repr, UdpPacket, UdpRepr,
};

/// A datagram, with the app's end and the other.
#[derive(Debug, PartialEq, Eq)]
pub struct Datagram<'a> {
    pub app: SocketAddr,
    pub server: SocketAddr,
    pub payload: &'a [u8],
}

/// The datagram in a packet an app sent, if it is one, whole.
pub fn parse(packet: &[u8]) -> Option<Datagram<'_>> {
    let (src, dst, payload): (IpAddress, IpAddress, &[u8]) = match packet.first()? >> 4 {
        4 => {
            let ip = Ipv4Packet::new_checked(packet).ok()?;
            let whole = !ip.more_frags() && ip.frag_offset() == 0;
            (whole && ip.next_header() == IpProtocol::Udp).then_some(())?;
            (ip.src_addr().into(), ip.dst_addr().into(), ip.payload())
        }
        6 => {
            let ip = Ipv6Packet::new_checked(packet).ok()?;
            (ip.next_header() == IpProtocol::Udp).then_some(())?;
            (ip.src_addr().into(), ip.dst_addr().into(), ip.payload())
        }
        _ => return None,
    };
    let udp = UdpPacket::new_checked(payload).ok()?;
    Some(Datagram {
        app: SocketAddr::new(src.into(), udp.src_port()),
        server: SocketAddr::new(dst.into(), udp.dst_port()),
        payload: udp.payload(),
    })
}

/// The packet that carries `payload` from one address to another, if the
/// two are of one kind and it fits in `mtu`.
pub fn packet(from: SocketAddr, to: SocketAddr, payload: &[u8], mtu: usize) -> Option<Vec<u8>> {
    let udp = UdpRepr {
        src_port: from.port(),
        dst_port: to.port(),
    };
    let len = udp.header_len() + payload.len();
    let caps = ChecksumCapabilities::default();
    let (src, dst): (IpAddress, IpAddress) = (from.ip().into(), to.ip().into());
    let emit = |into: &mut [u8]| {
        let mut datagram = UdpPacket::new_unchecked(into);
        let fill = |room: &mut [u8]| room.copy_from_slice(payload);
        udp.emit(&mut datagram, &src, &dst, payload.len(), fill, &caps);
    };
    match (from, to) {
        (SocketAddr::V4(from), SocketAddr::V4(to)) => {
            let ip = Ipv4Repr {
                src_addr: *from.ip(),
                dst_addr: *to.ip(),
                next_header: IpProtocol::Udp,
                payload_len: len,
                hop_limit: 64,
            };
            let mut bytes = vec![0; ip.buffer_len() + len];
            (bytes.len() <= mtu).then_some(())?;
            ip.emit(&mut Ipv4Packet::new_unchecked(&mut bytes[..]), &caps);
            emit(&mut bytes[ip.buffer_len()..]);
            Some(bytes)
        }
        (SocketAddr::V6(from), SocketAddr::V6(to)) => {
            let ip = Ipv6Repr {
                src_addr: *from.ip(),
                dst_addr: *to.ip(),
                next_header: IpProtocol::Udp,
                payload_len: len,
                hop_limit: 64,
            };
            let mut bytes = vec![0; ip.buffer_len() + len];
            (bytes.len() <= mtu).then_some(())?;
            ip.emit(&mut Ipv6Packet::new_unchecked(&mut bytes[..]));
            emit(&mut bytes[ip.buffer_len()..]);
            Some(bytes)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MTU: usize = 1500;

    #[test]
    fn a_datagram_is_read_from_the_packet_that_carries_it() {
        for (app, server) in [
            ("10.0.0.2:5353", "8.8.8.8:53"),
            ("[fd00::2]:5353", "[2001:4860:4860::8888]:53"),
        ] {
            let (app, server): (SocketAddr, SocketAddr) =
                (app.parse().unwrap(), server.parse().unwrap());
            let bytes = packet(app, server, b"a question", MTU).unwrap();
            let datagram = parse(&bytes).unwrap();
            assert!(std::ptr::eq(
                datagram.payload,
                &bytes[bytes.len() - b"a question".len()..]
            ));
            assert_eq!(
                datagram,
                Datagram {
                    app,
                    server,
                    payload: b"a question"
                }
            );
        }
    }

    #[test]
    fn what_is_no_whole_datagram_is_none() {
        let (app, server) = (
            "10.0.0.2:5353".parse().unwrap(),
            "8.8.8.8:53".parse().unwrap(),
        );
        let whole = packet(app, server, b"a question", MTU).unwrap();
        assert!(parse(&whole[..whole.len() - 1]).is_none());
        assert!(parse(&[]).is_none());
        assert!(parse(&[0x45; 12]).is_none());
        // A connection's packet.
        let mut tcp = whole.clone();
        tcp[9] = 6;
        assert!(parse(&tcp).is_none());
        // One piece of a datagram sent in several.
        let mut piece = whole;
        piece[6] |= 0x20;
        assert!(parse(&piece).is_none());
    }

    #[test]
    fn a_packet_is_made_only_if_it_can_be_one() {
        let (v4, v6): (SocketAddr, SocketAddr) = (
            "10.0.0.2:1".parse().unwrap(),
            "[fd00::2]:1".parse().unwrap(),
        );
        assert!(packet(v4, v6, b"x", MTU).is_none());
        assert!(packet(v4, v4, &[0; 1472], MTU).is_some());
        assert!(packet(v4, v4, &[0; 1473], MTU).is_none());
    }
}
