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

//! The DNS messages of a filtered app, whoever brought them: the one
//! question a query asks is read, judged by the host rules and the trusted
//! sites, and answered here when the rules block it. Everything else about
//! a query, and every answer, belongs to the app and its resolver.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::mitm::Ctx;

pub const HEADER: usize = 12;
const QR: u16 = 0x8000;
const OPCODE: u16 = 0x7800;
const TC: u16 = 0x0200;
const RECURSION_DESIRED: u16 = 0x0100;
const RECURSION_AVAILABLE: u16 = 0x0080;
const CHECKING_DISABLED: u16 = 0x0010;
const A: u16 = 1;
const AAAA: u16 = 28;

/// The single question of a standard query or of its response.
pub struct Question<'a> {
    id: [u8; 2],
    flags: u16,
    name: &'a [u8],
    kind: u16,
    class: u16,
    wire: &'a [u8],
}

impl<'a> Question<'a> {
    /// The question of `message`, if it is a standard query (`response`
    /// false) or response (`response` true) with exactly one question whose
    /// name is uncompressed and made of host name characters.
    pub fn parse(message: &'a [u8], response: bool) -> Option<Self> {
        let header = message.get(..HEADER)?;
        let flags = u16::from_be_bytes(header[2..4].try_into().ok()?);
        ((flags & QR != 0) == response && flags & OPCODE == 0 && header[4..6] == [0, 1])
            .then_some(())?;
        let question = &message[HEADER..];
        let mut offset = 0;
        loop {
            let len = usize::from(*question.get(offset)?);
            offset += 1;
            if len == 0 {
                break;
            }
            // A question must fit as an uncompressed name, including its terminator.
            if len > 63 || offset + len >= 255 {
                return None;
            }
            let label = question.get(offset..offset + len)?;
            if !label
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
            {
                return None;
            }
            offset += len;
        }
        let fields = question.get(offset..offset + 4)?;
        Some(Self {
            id: header[..2].try_into().ok()?,
            flags,
            name: &question[..offset],
            kind: u16::from_be_bytes(fields[..2].try_into().ok()?),
            class: u16::from_be_bytes(fields[2..].try_into().ok()?),
            wire: &question[..offset + 4],
        })
    }

    /// The transaction ID.
    pub fn id(&self) -> [u8; 2] {
        self.id
    }

    /// The name as it is on the wire, terminator included.
    pub fn name(&self) -> &'a [u8] {
        self.name
    }

    pub fn kind(&self) -> u16 {
        self.kind
    }

    pub fn class(&self) -> u16 {
        self.class
    }

    /// The name asked about, in lower case, for a question the rules can
    /// judge: of class IN, and not of the root.
    pub fn host(&self) -> Option<String> {
        if self.class != 1 || self.name == [0] {
            return None;
        }
        let mut name = self.name;
        let mut host = String::with_capacity(name.len());
        while name != [0] {
            let len = usize::from(name[0]);
            let label = std::str::from_utf8(&name[1..1 + len]).ok()?;
            if !host.is_empty() {
                host.push('.');
            }
            host.push_str(label);
            name = &name[1 + len..];
        }
        Some(crate::host::canonical_authority(&host, "https", 443)?.host)
    }

    /// Whether `answer` is a response to this question: the same
    /// transaction and the same question, the name in any case.
    pub fn matches(&self, answer: &[u8]) -> bool {
        Question::parse(answer, true).is_some_and(|response| {
            self.id == response.id
                && self.kind == response.kind
                && self.class == response.class
                && self.name.eq_ignore_ascii_case(response.name)
        })
    }

    /// The answer given here: the unspecified address for an A or AAAA
    /// question, and no records for any other, valid for `ttl` seconds.
    /// A truncated one has no records and asks the client to retry over TCP.
    pub fn local_answer(&self, ttl: u32, truncated: bool) -> Vec<u8> {
        let data: &[u8] = match (truncated, self.kind) {
            (false, A) => &[0; 4],
            (false, AAAA) => &[0; 16],
            _ => &[],
        };
        let mut result = Vec::with_capacity(HEADER + self.wire.len() + 12 + data.len());
        let flags = QR
            | RECURSION_AVAILABLE
            | (self.flags & (RECURSION_DESIRED | CHECKING_DISABLED))
            | if truncated { TC } else { 0 };
        result.extend_from_slice(&self.id);
        result.extend_from_slice(&flags.to_be_bytes());
        result.extend_from_slice(&[0, 1, 0, u8::from(!data.is_empty()), 0, 0, 0, 0]);
        result.extend_from_slice(self.wire);
        if !data.is_empty() {
            result.extend_from_slice(&[0xc0, HEADER as u8]);
            result.extend_from_slice(&self.kind.to_be_bytes());
            result.extend_from_slice(&self.class.to_be_bytes());
            result.extend_from_slice(&ttl.to_be_bytes());
            result.extend_from_slice(&(data.len() as u16).to_be_bytes());
            result.extend_from_slice(data);
        }
        result
    }
}

/// The answer to `question` for the filtered app of `uid` when the rules
/// block the host it asks about, valid for `ttl` seconds; none when the
/// app is not filtered, the host is trusted, or nothing blocks it.
pub fn blocked_answer(ctx: &Ctx, uid: u32, question: &Question<'_>, ttl: u32) -> Option<Vec<u8>> {
    let host = question.host()?;
    ctx.blocks_udp_host(uid, &host)
        .then(|| question.local_answer(ttl, false))
}

/// Whether `reply` can be the response to `query`. A query whose question
/// cannot be read is answered by any response with its transaction ID.
pub fn is_answer_to(query: &[u8], reply: &[u8]) -> bool {
    match Question::parse(query, false) {
        Some(question) => question.matches(reply),
        None => {
            query.len() >= 2
                && reply.len() >= HEADER
                && reply[..2] == query[..2]
                && u16::from_be_bytes([reply[2], reply[3]]) & QR != 0
        }
    }
}

/// A header-only error for a query whose question cannot be read: NOTIMP
/// for another opcode, FORMERR otherwise. Nothing for a response or a
/// message too short to carry an ID.
pub fn refusal(request: &[u8]) -> Option<Vec<u8>> {
    const FORMERR: u16 = 1;
    const NOTIMP: u16 = 4;
    let header = request.get(..HEADER)?;
    let flags = u16::from_be_bytes([header[2], header[3]]);
    if flags & QR != 0 {
        return None;
    }
    let code = if flags & OPCODE == 0 { FORMERR } else { NOTIMP };
    let flags = QR | (flags & (OPCODE | RECURSION_DESIRED)) | RECURSION_AVAILABLE | code;
    let mut result = header[..2].to_vec();
    result.extend_from_slice(&flags.to_be_bytes());
    result.extend_from_slice(&[0; 8]);
    Some(result)
}

/// One message of a DNS stream: its length in two bytes, then the
/// message. The length itself bounds it to 65535 bytes.
pub async fn read_frame(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<Vec<u8>> {
    let length = usize::from(stream.read_u16().await?);
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}

pub async fn write_frame(stream: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> io::Result<()> {
    let len = u16::try_from(bytes.len()).map_err(|_| io::ErrorKind::InvalidData)?;
    let mut framed = Vec::with_capacity(2 + bytes.len());
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(bytes);
    stream.write_all(&framed).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: u32 = 30;

    fn query(kind: u16) -> Vec<u8> {
        let mut bytes = b"\x12\x34\x01\0\0\x01\0\0\0\0\0\0\x03AdS\x07example\0".to_vec();
        bytes.extend_from_slice(&kind.to_be_bytes());
        bytes.extend_from_slice(&[0, 1]);
        bytes
    }

    #[test]
    fn a_blocked_address_is_sinkholed_and_other_records_are_empty() {
        for (kind, length) in [(1, 4), (28, 16), (64, 0), (65, 0), (16, 0)] {
            let request = query(kind);
            let question = Question::parse(&request, false).unwrap();
            assert_eq!(question.host().as_deref(), Some("ads.example"));
            let answer = question.local_answer(TTL, false);
            assert!(question.matches(&answer));
            assert_eq!(answer[3] & 0xf, 0);
            assert_eq!(answer[7], u8::from(length > 0));
            if length > 0 {
                assert_eq!(&answer[answer.len() - length..], vec![0; length]);
                assert_eq!(
                    &answer[request.len() + 6..request.len() + 10],
                    &TTL.to_be_bytes()
                );
            } else {
                assert_eq!(answer.len(), request.len());
            }
        }
    }

    #[test]
    fn an_answer_with_no_lifetime_says_zero_seconds() {
        for kind in [1, 28] {
            let request = query(kind);
            let question = Question::parse(&request, false).unwrap();
            let answer = question.local_answer(0, false);
            assert_eq!(&answer[request.len() + 6..request.len() + 10], &[0; 4]);
            assert!(question.matches(&answer));
        }
    }

    #[test]
    fn only_a_bounded_question_and_its_matching_answer_are_accepted() {
        let request = query(65);
        let question = Question::parse(&request, false).unwrap();
        assert!(Question::parse(&request[..request.len() - 1], false).is_none());
        for (at, value) in [(5, 2), (12, 0xc0), (13, b'!')] {
            let mut bad = request.clone();
            bad[at] = value;
            assert!(Question::parse(&bad, false).is_none());
        }
        let mut answer = question.local_answer(TTL, false);
        answer[13] = b'a';
        assert!(question.matches(&answer));
        for index in [1, 14, request.len() - 3, request.len() - 1] {
            let mut other = answer.clone();
            other[index] ^= 1;
            assert!(!question.matches(&other));
        }
        let truncated = question.local_answer(TTL, true);
        assert!(question.matches(&truncated));
        assert_ne!(truncated[2] & 2, 0);
    }

    #[test]
    fn a_reply_must_answer_the_query_and_one_that_cannot_be_read_by_its_id() {
        let request = query(1);
        let answer = Question::parse(&request, false)
            .unwrap()
            .local_answer(0, false);
        assert!(is_answer_to(&request, &answer));
        let mut other_name = answer.clone();
        other_name[14] ^= 1;
        assert!(!is_answer_to(&request, &other_name));
        assert!(!is_answer_to(&request, &request));

        // Two questions: not read, so judged by the transaction alone.
        let mut two = request.clone();
        two[5] = 2;
        let mut reply = two.clone();
        reply[2] |= 0x80;
        assert!(is_answer_to(&two, &reply));
        assert!(!is_answer_to(&two, &two));
        reply[1] ^= 1;
        assert!(!is_answer_to(&two, &reply));
        assert!(!is_answer_to(&two, &reply[..HEADER - 1]));
        assert!(!is_answer_to(&[0x12], &answer));
    }

    #[test]
    fn a_query_that_cannot_be_read_is_refused_by_its_opcode() {
        let mut two = query(1);
        two[5] = 2;
        let refused = refusal(&two).unwrap();
        assert_eq!(refused.len(), HEADER);
        assert_eq!(&refused[..2], &two[..2]);
        assert_eq!(refused[2] & 0x80, 0x80);
        assert_eq!(refused[3] & 0xf, 1);
        let mut notify = query(1);
        notify[2] |= 0x20;
        assert_eq!(refusal(&notify).unwrap()[3] & 0xf, 4);
        assert!(refusal(&two[..HEADER - 1]).is_none());
        let mut response = query(1);
        response[2] |= 0x80;
        assert!(refusal(&response).is_none());
    }

    #[tokio::test]
    async fn stream_messages_carry_their_length_and_several_share_a_connection() {
        let (mut client, mut server) = tokio::io::duplex(1 << 17);
        let large = vec![7; usize::from(u16::MAX)];
        write_frame(&mut client, &query(1)).await.unwrap();
        write_frame(&mut client, &large).await.unwrap();
        write_frame(&mut client, &[]).await.unwrap();
        assert_eq!(read_frame(&mut server).await.unwrap(), query(1));
        assert_eq!(read_frame(&mut server).await.unwrap(), large);
        assert!(read_frame(&mut server).await.unwrap().is_empty());
        let too_long = vec![0; usize::from(u16::MAX) + 1];
        assert_eq!(
            write_frame(&mut client, &too_long)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        drop(client);
        assert!(read_frame(&mut server).await.is_err());
    }
}
