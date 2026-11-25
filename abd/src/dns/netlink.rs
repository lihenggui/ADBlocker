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

//! The routing side of lookup capture, said to the kernel over rtnetlink:
//! Android's `ip` predates the rule attributes that match a port and a
//! protocol, and a process for each change would be slower besides.

use std::io::{self, Read as _, Write as _};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

use super::{DEVICE, DNS_PORT, PREF, SOURCE_V4, SOURCE_V6, TABLE};
use crate::capture::MARK_BYPASS;

const NLMSG_HDRLEN: usize = 16;
const NLM_F_REQUEST: u16 = 0x1;
const NLM_F_MULTI: u16 = 0x2;
const NLM_F_ACK: u16 = 0x4;
const NLM_F_REPLACE: u16 = 0x100;
const NLM_F_EXCL: u16 = 0x200;
const NLM_F_CREATE: u16 = 0x400;
const NLM_F_DUMP: u16 = 0x300;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;

const RTM_NEWLINK: u16 = 16;
const RTM_DELLINK: u16 = 17;
const RTM_GETLINK: u16 = 18;
const RTM_NEWADDR: u16 = 20;
const RTM_NEWROUTE: u16 = 24;
const RTM_DELROUTE: u16 = 25;
const RTM_GETROUTE: u16 = 26;
const RTM_NEWRULE: u16 = 32;
const RTM_DELRULE: u16 = 33;
const RTM_GETRULE: u16 = 34;

const FRA_SRC: u16 = 2;
const FRA_PRIORITY: u16 = 6;
const FRA_FWMARK: u16 = 10;
const FRA_TABLE: u16 = 15;
const FRA_FWMASK: u16 = 16;
const FRA_UID_RANGE: u16 = 20;
const FRA_IP_PROTO: u16 = 22;
const FRA_DPORT_RANGE: u16 = 24;
const FR_ACT_TO_TBL: u8 = 1;

const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_PREFSRC: u16 = 7;
const RTA_TABLE: u16 = 15;
const RTPROT_STATIC: u8 = 4;
const RT_SCOPE_HOST: u8 = 254;
const RT_SCOPE_NOWHERE: u8 = 255;
const RTN_LOCAL: u8 = 2;
const RTN_MULTICAST: u8 = 5;

const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_CACHEINFO: u16 = 6;
const IFA_F_NODAD: u8 = 0x02;
/// A lifetime that does not end.
const FOREVER: u32 = u32::MAX;
const IFLA_IFNAME: u16 = 3;
const IFF_UP: u32 = 0x1;

/// How long the kernel may take with one answer. It answers at once; this
/// bounds a wait that should never happen.
const ANSWER_TIME: Duration = Duration::from_secs(1);
/// The most messages read for one request: our rules can number four for
/// each of 4096 ranges, and the system has its own.
const MAX_MESSAGES: usize = 32 * 1024;

pub(super) fn family(v6: bool) -> u8 {
    (if v6 { libc::AF_INET6 } else { libc::AF_INET }) as u8
}

/// One of our policy routing rules: the lookups of the UIDs `uids` over
/// `proto`, sent with no bypass mark from `source`, go to our table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Rule {
    pub(super) v6: bool,
    pub(super) uids: (u32, u32),
    pub(super) proto: u8,
    pub(super) source: Source,
}

/// What source a packet has when a rule is asked about it.
///
/// A socket with no address of its own is routed without a source, and
/// our route gives it the anchor's, with which it is routed again. A
/// socket that already has another address keeps it: brought here, its
/// packets would match no TPROXY rule and be delivered to nothing, so it
/// is left to its own route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Source {
    Unset,
    Anchor,
}

impl Source {
    fn address(self, v6: bool) -> IpAddr {
        match (self, v6) {
            (Source::Unset, false) => Ipv4Addr::UNSPECIFIED.into(),
            (Source::Unset, true) => Ipv6Addr::UNSPECIFIED.into(),
            (Source::Anchor, false) => SOURCE_V4.into(),
            (Source::Anchor, true) => SOURCE_V6.into(),
        }
    }
}

/// A request being put together: its header, its fixed part, attributes.
pub(super) struct Request(Vec<u8>);

impl Request {
    fn new(kind: u16, flags: u16, fixed: &[u8]) -> Self {
        let mut bytes = vec![0; NLMSG_HDRLEN];
        bytes[4..6].copy_from_slice(&kind.to_ne_bytes());
        bytes[6..8].copy_from_slice(&(NLM_F_REQUEST | flags).to_ne_bytes());
        bytes.extend_from_slice(fixed);
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        Self(bytes)
    }

    fn attr(mut self, kind: u16, data: &[u8]) -> Self {
        let len = 4 + data.len();
        self.0.extend_from_slice(&(len as u16).to_ne_bytes());
        self.0.extend_from_slice(&kind.to_ne_bytes());
        self.0.extend_from_slice(data);
        self.0.resize(self.0.len().next_multiple_of(4), 0);
        self
    }

    fn finish(mut self, seq: u32) -> Vec<u8> {
        let len = self.0.len() as u32;
        self.0[0..4].copy_from_slice(&len.to_ne_bytes());
        self.0[8..12].copy_from_slice(&seq.to_ne_bytes());
        self.0
    }

    #[cfg(test)]
    pub(super) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

fn address(ip: IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(v4) => v4.octets().to_vec(),
        IpAddr::V6(v6) => v6.octets().to_vec(),
    }
}

fn rule_header(v6: bool) -> [u8; 12] {
    let mut header = [0; 12];
    header[0] = family(v6);
    header[4] = TABLE as u8;
    header[7] = FR_ACT_TO_TBL;
    header
}

/// Adds `rule`, or deletes it.
pub(super) fn rule(add: bool, rule: &Rule) -> Request {
    let (kind, flags) = if add {
        (RTM_NEWRULE, NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL)
    } else {
        (RTM_DELRULE, NLM_F_ACK)
    };
    let mut uids = rule.uids.0.to_ne_bytes().to_vec();
    uids.extend_from_slice(&rule.uids.1.to_ne_bytes());
    let mut ports = DNS_PORT.to_ne_bytes().to_vec();
    ports.extend_from_slice(&DNS_PORT.to_ne_bytes());
    let source = address(rule.source.address(rule.v6));
    let mut header = rule_header(rule.v6);
    header[2] = (source.len() * 8) as u8;
    Request::new(kind, flags, &header)
        .attr(FRA_SRC, &source)
        .attr(FRA_PRIORITY, &PREF.to_ne_bytes())
        .attr(FRA_TABLE, &TABLE.to_ne_bytes())
        .attr(FRA_FWMARK, &0u32.to_ne_bytes())
        .attr(FRA_FWMASK, &MARK_BYPASS.to_ne_bytes())
        .attr(FRA_UID_RANGE, &uids)
        .attr(FRA_IP_PROTO, &[rule.proto])
        .attr(FRA_DPORT_RANGE, &ports)
}

/// Deletes one rule of ours, whichever: the kernel takes the first with
/// our priority and table.
pub(super) fn any_rule(v6: bool) -> Request {
    Request::new(RTM_DELRULE, NLM_F_ACK, &rule_header(v6))
        .attr(FRA_PRIORITY, &PREF.to_ne_bytes())
        .attr(FRA_TABLE, &TABLE.to_ne_bytes())
}

pub(super) fn rules(v6: bool) -> Request {
    Request::new(RTM_GETRULE, NLM_F_DUMP, &rule_header(v6))
}

/// Our table's one route: everything is local, through `index`, from
/// `source`. Without a source the kernel would take the destination for
/// one, and no connection could be made from it.
pub(super) fn route(index: u32, source: IpAddr) -> Request {
    let fixed = route_header(source.is_ipv6(), RTPROT_STATIC, RT_SCOPE_HOST);
    Request::new(
        RTM_NEWROUTE,
        NLM_F_ACK | NLM_F_CREATE | NLM_F_REPLACE,
        &fixed,
    )
    .attr(RTA_TABLE, &TABLE.to_ne_bytes())
    .attr(RTA_OIF, &index.to_ne_bytes())
    .attr(RTA_PREFSRC, &address(source))
}

/// Deletes our table's route, through whichever device.
pub(super) fn no_route(v6: bool) -> Request {
    let fixed = route_header(v6, 0, RT_SCOPE_NOWHERE);
    Request::new(RTM_DELROUTE, NLM_F_ACK, &fixed).attr(RTA_TABLE, &TABLE.to_ne_bytes())
}

fn route_header(v6: bool, protocol: u8, scope: u8) -> [u8; 12] {
    let mut fixed = [0; 12];
    fixed[0] = family(v6);
    fixed[4] = TABLE as u8;
    fixed[5] = protocol;
    fixed[6] = scope;
    fixed[7] = RTN_LOCAL;
    fixed
}

/// The route the kernel would take to `ip`, for its kind.
pub(super) fn route_to(ip: IpAddr) -> Request {
    let mut fixed = [0; 12];
    fixed[0] = family(ip.is_ipv6());
    fixed[1] = if ip.is_ipv6() { 128 } else { 32 };
    Request::new(RTM_GETROUTE, 0, &fixed).attr(RTA_DST, &address(ip))
}

/// An address only the device has, with no duplicate detection: it is
/// usable at once, and nothing else is on the device to collide with.
/// It is deprecated from the start, preferred for no time and valid
/// forever, so that the kernel never picks it as the source of anything
/// else: only our route names it.
pub(super) fn address_v6(index: u32, ip: IpAddr) -> Request {
    let mut fixed = [0; 8];
    fixed[0] = family(true);
    fixed[1] = 128;
    fixed[2] = IFA_F_NODAD;
    fixed[4..8].copy_from_slice(&index.to_ne_bytes());
    Request::new(
        RTM_NEWADDR,
        NLM_F_ACK | NLM_F_CREATE | NLM_F_REPLACE,
        &fixed,
    )
    .attr(IFA_LOCAL, &address(ip))
    .attr(IFA_ADDRESS, &address(ip))
    .attr(IFA_CACHEINFO, &lifetimes(0, FOREVER))
}

/// `struct ifa_cacheinfo`: the preferred and valid lifetimes, in seconds,
/// and two timestamps the kernel sets.
fn lifetimes(preferred: u32, valid: u32) -> [u8; 16] {
    let mut info = [0; 16];
    info[0..4].copy_from_slice(&preferred.to_ne_bytes());
    info[4..8].copy_from_slice(&valid.to_ne_bytes());
    info
}

fn link_header(index: u32, up: bool) -> [u8; 16] {
    let mut fixed = [0; 16];
    fixed[4..8].copy_from_slice(&index.to_ne_bytes());
    if up {
        fixed[8..12].copy_from_slice(&IFF_UP.to_ne_bytes());
        fixed[12..16].copy_from_slice(&IFF_UP.to_ne_bytes());
    }
    fixed
}

pub(super) fn link_up(index: u32) -> Request {
    Request::new(RTM_NEWLINK, NLM_F_ACK, &link_header(index, true))
}

fn device_name() -> Vec<u8> {
    let mut name = DEVICE.as_bytes().to_vec();
    name.push(0);
    name
}

/// Asks for the device's index.
pub(super) fn link_index() -> Request {
    Request::new(RTM_GETLINK, 0, &link_header(0, false)).attr(IFLA_IFNAME, &device_name())
}

/// Deletes the device, whoever holds it.
pub(super) fn link_delete() -> Request {
    Request::new(RTM_DELLINK, NLM_F_ACK, &link_header(0, false)).attr(IFLA_IFNAME, &device_name())
}

/// What one of the kernel's messages is about.
pub(super) struct Message<'a> {
    kind: u16,
    flags: u16,
    seq: u32,
    payload: &'a [u8],
}

/// The messages of one read, as far as they are whole.
fn messages(mut bytes: &[u8]) -> impl Iterator<Item = Message<'_>> {
    std::iter::from_fn(move || {
        let header = bytes.get(..NLMSG_HDRLEN)?;
        let len = u32::from_ne_bytes(header[0..4].try_into().ok()?) as usize;
        let message = Message {
            kind: u16::from_ne_bytes([header[4], header[5]]),
            flags: u16::from_ne_bytes([header[6], header[7]]),
            seq: u32::from_ne_bytes(header[8..12].try_into().ok()?),
            payload: bytes.get(NLMSG_HDRLEN..len)?,
        };
        bytes = bytes
            .get(len.checked_next_multiple_of(4)?..)
            .unwrap_or_default();
        Some(message)
    })
}

/// The error an acknowledgement carries, none for success.
fn acknowledged(payload: &[u8]) -> io::Result<()> {
    let code = payload
        .get(0..4)
        .map(|code| i32::from_ne_bytes([code[0], code[1], code[2], code[3]]))
        .ok_or(io::ErrorKind::InvalidData)?;
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(-code))
    }
}

/// The attributes after a message's fixed part of `fixed` bytes.
fn attributes(payload: &[u8], fixed: usize) -> impl Iterator<Item = (u16, &[u8])> {
    let mut rest = payload.get(fixed..).unwrap_or_default();
    std::iter::from_fn(move || {
        let header = rest.get(..4)?;
        let len = usize::from(u16::from_ne_bytes([header[0], header[1]]));
        let kind = u16::from_ne_bytes([header[2], header[3]]);
        let data = rest.get(4..len)?;
        rest = rest
            .get(len.checked_next_multiple_of(4)?..)
            .unwrap_or_default();
        Some((kind, data))
    })
}

/// A rule as the kernel lists it, of what matters here.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Listed {
    pub(super) ours: bool,
    pub(super) uids: Option<(u32, u32)>,
    pub(super) proto: Option<u8>,
    pub(super) ports: Option<(u16, u16)>,
    pub(super) mask: Option<u32>,
}

fn listed(payload: &[u8]) -> Listed {
    let (mut pref, mut table) = (None, payload.get(4).map(|t| u32::from(*t)));
    let mut rule = Listed::default();
    let u32_of = |data: &[u8]| {
        data.get(..4)
            .map(|d| u32::from_ne_bytes([d[0], d[1], d[2], d[3]]))
    };
    for (kind, data) in attributes(payload, 12) {
        match kind {
            FRA_PRIORITY => pref = u32_of(data),
            FRA_TABLE => table = u32_of(data),
            FRA_FWMASK => rule.mask = u32_of(data),
            FRA_UID_RANGE => rule.uids = u32_of(data).zip(data.get(4..).and_then(u32_of)),
            FRA_IP_PROTO => rule.proto = data.first().copied(),
            FRA_DPORT_RANGE if data.len() >= 4 => {
                rule.ports = Some((
                    u16::from_ne_bytes([data[0], data[1]]),
                    u16::from_ne_bytes([data[2], data[3]]),
                ));
            }
            _ => {}
        }
    }
    rule.ours = pref == Some(PREF) && table == Some(TABLE);
    rule
}

/// A socket to the kernel's routing, for requests made one at a time.
pub(super) struct Netlink {
    socket: Socket,
    seq: u32,
    buffer: Vec<u8>,
}

impl Netlink {
    pub(super) fn open() -> io::Result<Self> {
        Self::with_mode(false)
    }

    pub(super) fn open_lookup() -> io::Result<Self> {
        Self::with_mode(true)
    }

    fn with_mode(nonblocking: bool) -> io::Result<Self> {
        let socket = Socket::new(
            Domain::from(libc::AF_NETLINK),
            Type::RAW,
            Some(Protocol::from(libc::NETLINK_ROUTE)),
        )?;
        if nonblocking {
            socket.set_nonblocking(true)?;
        } else {
            socket.set_read_timeout(Some(ANSWER_TIME))?;
        }
        Ok(Self {
            socket,
            seq: 0,
            buffer: vec![0; 32 * 1024],
        })
    }

    /// Sends `request` and hands each message of the answer to `take`,
    /// until the answer ends or `take` has what it wanted.
    fn exchange(
        &mut self,
        request: Request,
        mut take: impl FnMut(&Message<'_>) -> Option<io::Result<()>>,
    ) -> io::Result<()> {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        (&self.socket).write_all(&request.finish(seq))?;
        let mut read = 0;
        for _ in 0..MAX_MESSAGES {
            if read >= MAX_MESSAGES {
                break;
            }
            let n = (&self.socket).read(&mut self.buffer)?;
            if n < NLMSG_HDRLEN {
                return Err(io::ErrorKind::InvalidData.into());
            }
            for message in messages(&self.buffer[..n]) {
                read += 1;
                // An answer to an earlier request that came too late.
                if message.seq != seq {
                    continue;
                }
                if message.kind == NLMSG_ERROR {
                    return acknowledged(message.payload);
                }
                if let Some(done) = take(&message) {
                    return done;
                }
                if message.kind == NLMSG_DONE || message.flags & NLM_F_MULTI == 0 {
                    return Ok(());
                }
            }
        }
        Err(io::ErrorKind::InvalidData.into())
    }

    /// A change, done once the kernel says it is.
    pub(super) fn change(&mut self, request: Request) -> io::Result<()> {
        self.exchange(request, |_| Some(Err(io::ErrorKind::InvalidData.into())))
    }

    /// Our rules of the family, as the kernel lists them.
    pub(super) fn our_rules(&mut self, v6: bool) -> io::Result<Vec<Listed>> {
        let mut found = Vec::new();
        self.exchange(rules(v6), |message| {
            if message.kind == RTM_NEWRULE {
                let rule = listed(message.payload);
                if rule.ours {
                    found.push(rule);
                }
            }
            None
        })?;
        Ok(found)
    }

    /// The device's index.
    pub(super) fn index(&mut self) -> io::Result<u32> {
        let mut index = None;
        self.exchange(link_index(), |message| {
            index = message
                .payload
                .get(4..8)
                .map(|i| u32::from_ne_bytes([i[0], i[1], i[2], i[3]]));
            Some(Ok(()))
        })?;
        index.ok_or_else(|| io::ErrorKind::NotFound.into())
    }

    /// Whether `ip` is this device's own, or one to many: never a server
    /// a lookup can be sent on to.
    pub(super) fn is_local(&mut self, ip: IpAddr) -> io::Result<bool> {
        let mut kind = None;
        let asked = self.exchange(route_to(ip), |message| {
            kind = message.payload.get(7).copied();
            Some(Ok(()))
        });
        match asked {
            Ok(()) => Ok(kind.is_some_and(|kind| (RTN_LOCAL..=RTN_MULTICAST).contains(&kind))),
            // No route at all: nothing here either.
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::ENETUNREACH | libc::EHOSTUNREACH | libc::ESRCH)
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(request: &Request, fixed: usize) -> Vec<(u16, Vec<u8>)> {
        attributes(&request.bytes()[NLMSG_HDRLEN..], fixed)
            .map(|(kind, data)| (kind, data.to_vec()))
            .collect()
    }

    fn u32s(a: u32, b: u32) -> Vec<u8> {
        [a.to_ne_bytes(), b.to_ne_bytes()].concat()
    }

    #[test]
    fn a_rule_matches_the_uids_the_protocol_and_port_53_without_the_bypass_mark() {
        let wanted = Rule {
            v6: true,
            uids: (10001, 10003),
            proto: 17,
            source: Source::Anchor,
        };
        let added = rule(true, &wanted);
        let bytes = added.bytes();
        assert_eq!(u16::from_ne_bytes([bytes[4], bytes[5]]), RTM_NEWRULE);
        let flags = u16::from_ne_bytes([bytes[6], bytes[7]]);
        assert_eq!(flags, NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL);
        let header = &bytes[NLMSG_HDRLEN..NLMSG_HDRLEN + 12];
        assert_eq!((header[0], header[2]), (libc::AF_INET6 as u8, 128));
        assert_eq!((header[4], header[7]), (TABLE as u8, FR_ACT_TO_TBL));
        let ports = [DNS_PORT.to_ne_bytes(), DNS_PORT.to_ne_bytes()].concat();
        assert_eq!(
            attrs(&added, 12),
            [
                (FRA_SRC, SOURCE_V6.octets().to_vec()),
                (FRA_PRIORITY, PREF.to_ne_bytes().to_vec()),
                (FRA_TABLE, TABLE.to_ne_bytes().to_vec()),
                (FRA_FWMARK, 0u32.to_ne_bytes().to_vec()),
                (FRA_FWMASK, MARK_BYPASS.to_ne_bytes().to_vec()),
                (FRA_UID_RANGE, u32s(10001, 10003)),
                (FRA_IP_PROTO, vec![17]),
                (FRA_DPORT_RANGE, ports),
            ]
        );
        let deleted = rule(false, &wanted);
        assert_eq!(&deleted.bytes()[4..6], &RTM_DELRULE.to_ne_bytes());
        assert_eq!(attrs(&deleted, 12), attrs(&added, 12));
    }

    #[test]
    fn a_rule_takes_only_a_socket_with_no_source_yet_or_the_anchors() {
        let unset = Rule {
            v6: false,
            uids: (10001, 10001),
            proto: 6,
            source: Source::Unset,
        };
        let request = rule(true, &unset);
        assert_eq!(request.bytes()[NLMSG_HDRLEN + 2], 32);
        assert_eq!(attrs(&request, 12)[0], (FRA_SRC, vec![0; 4]));
        let anchor = rule(
            true,
            &Rule {
                source: Source::Anchor,
                ..unset
            },
        );
        assert_eq!(attrs(&anchor, 12)[0], (FRA_SRC, vec![127, 0, 0, 1]));
        let v6 = rule(true, &Rule { v6: true, ..unset });
        assert_eq!(attrs(&v6, 12)[0], (FRA_SRC, vec![0; 16]));
    }

    #[test]
    fn any_rule_of_ours_is_named_by_priority_and_table_alone() {
        let any = any_rule(false);
        assert_eq!(&any.bytes()[4..6], &RTM_DELRULE.to_ne_bytes());
        assert_eq!(any.bytes()[NLMSG_HDRLEN], libc::AF_INET as u8);
        assert_eq!(
            attrs(&any, 12),
            [
                (FRA_PRIORITY, PREF.to_ne_bytes().to_vec()),
                (FRA_TABLE, TABLE.to_ne_bytes().to_vec()),
            ]
        );
    }

    #[test]
    fn the_route_is_local_through_the_device_from_the_anchor_source() {
        let source: IpAddr = "127.0.0.1".parse().unwrap();
        let added = route(7, source);
        let fixed = &added.bytes()[NLMSG_HDRLEN..NLMSG_HDRLEN + 12];
        assert_eq!(fixed[0], libc::AF_INET as u8);
        assert_eq!((fixed[1], fixed[2]), (0, 0));
        assert_eq!(
            (fixed[4], fixed[5], fixed[6], fixed[7]),
            (TABLE as u8, RTPROT_STATIC, RT_SCOPE_HOST, RTN_LOCAL)
        );
        assert_eq!(
            attrs(&added, 12),
            [
                (RTA_TABLE, TABLE.to_ne_bytes().to_vec()),
                (RTA_OIF, 7u32.to_ne_bytes().to_vec()),
                (RTA_PREFSRC, vec![127, 0, 0, 1]),
            ]
        );
        let gone = no_route(true);
        assert_eq!(&gone.bytes()[4..6], &RTM_DELROUTE.to_ne_bytes());
        assert_eq!(gone.bytes()[NLMSG_HDRLEN + 6], RT_SCOPE_NOWHERE);
        assert_eq!(
            attrs(&gone, 12),
            [(RTA_TABLE, TABLE.to_ne_bytes().to_vec())]
        );
    }

    #[test]
    fn the_ipv6_source_is_usable_at_once_never_preferred_and_the_device_comes_up() {
        let ip: IpAddr = "fd00::1".parse().unwrap();
        let added = address_v6(7, ip);
        let fixed = &added.bytes()[NLMSG_HDRLEN..NLMSG_HDRLEN + 8];
        assert_eq!(fixed[..4], [libc::AF_INET6 as u8, 128, IFA_F_NODAD, 0]);
        assert_eq!(fixed[4..8], 7u32.to_ne_bytes());
        let octets = address(ip);
        let mut forever = 0u32.to_ne_bytes().to_vec();
        forever.extend_from_slice(&u32::MAX.to_ne_bytes());
        forever.resize(16, 0);
        assert_eq!(
            attrs(&added, 8),
            [
                (IFA_LOCAL, octets.clone()),
                (IFA_ADDRESS, octets),
                (IFA_CACHEINFO, forever)
            ]
        );
        let up = link_up(7);
        let fixed = &up.bytes()[NLMSG_HDRLEN..NLMSG_HDRLEN + 16];
        assert_eq!(fixed[4..8], 7u32.to_ne_bytes());
        assert_eq!(fixed[8..16], u32s(IFF_UP, IFF_UP)[..]);
        let mut name = DEVICE.as_bytes().to_vec();
        name.push(0);
        assert_eq!(attrs(&link_delete(), 16), [(IFLA_IFNAME, name)]);
        assert_eq!(&link_index().bytes()[4..6], &RTM_GETLINK.to_ne_bytes());
    }

    #[test]
    fn requests_carry_their_length_and_sequence_and_answers_are_read_whole() {
        let finished = route_to("10.0.2.3".parse().unwrap()).finish(9);
        assert_eq!(u32::from_ne_bytes(finished[0..4].try_into().unwrap()), 36);
        assert_eq!(u32::from_ne_bytes(finished[8..12].try_into().unwrap()), 9);
        assert_eq!(finished[NLMSG_HDRLEN + 1], 32);

        let mut error = vec![0; NLMSG_HDRLEN + 4];
        error[0..4].copy_from_slice(&20u32.to_ne_bytes());
        error[4..6].copy_from_slice(&NLMSG_ERROR.to_ne_bytes());
        error[16..20].copy_from_slice(&(-libc::ENOENT).to_ne_bytes());
        let mut two = error.clone();
        two.extend_from_slice(&error[..NLMSG_HDRLEN]);
        let read: Vec<_> = messages(&two).collect();
        assert_eq!(read.len(), 1);
        assert_eq!(
            acknowledged(read[0].payload).unwrap_err().raw_os_error(),
            Some(libc::ENOENT)
        );
        assert!(acknowledged(&[0; 4]).is_ok());
        error[..4].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(messages(&error).next().is_none());
        assert!(attributes(&[0xff, 0xff, 0, 0], 0).next().is_none());
    }

    #[test]
    fn a_listed_rule_is_ours_only_with_our_priority_and_table() {
        let wanted = Rule {
            v6: false,
            uids: (10001, 10001),
            proto: 6,
            source: Source::Unset,
        };
        let request = rule(true, &wanted);
        let found = listed(&request.bytes()[NLMSG_HDRLEN..]);
        assert_eq!(
            found,
            Listed {
                ours: true,
                uids: Some((10001, 10001)),
                proto: Some(6),
                ports: Some((DNS_PORT, DNS_PORT)),
                mask: Some(MARK_BYPASS),
            }
        );
        // What a kernel that does not know the port and protocol keeps.
        let old = Request::new(RTM_NEWRULE, 0, &rule_header(false))
            .attr(FRA_PRIORITY, &PREF.to_ne_bytes())
            .attr(FRA_TABLE, &TABLE.to_ne_bytes())
            .attr(FRA_UID_RANGE, &u32s(1, 1));
        let found = listed(&old.bytes()[NLMSG_HDRLEN..]);
        assert!(found.ours && found.proto.is_none() && found.ports.is_none());
        let other = Request::new(RTM_NEWRULE, 0, &rule_header(false))
            .attr(FRA_PRIORITY, &(PREF + 1).to_ne_bytes())
            .attr(FRA_TABLE, &TABLE.to_ne_bytes());
        assert!(!listed(&other.bytes()[NLMSG_HDRLEN..]).ours);
    }
}
