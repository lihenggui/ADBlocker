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

//! Reads what a TLS ClientHello says in the clear, without consuming it, so
//! that a connection can still be handed on untouched.

use std::time::Duration;

use tokio::net::TcpStream;

#[cfg(test)]
pub(crate) mod ech_fixture;

#[derive(Debug, Default, Clone)]
pub struct Hello {
    pub sni: Option<String>,
    pub alpn: Vec<Vec<u8>>,
    /// The client will speak nothing older than TLS 1.3.
    pub tls13_only: bool,
    /// The visible hello offers real or GREASE Encrypted Client Hello.
    pub ech: bool,
}

/// The longest wait between two looks at a hello that is arriving.
const MAX_WAIT: Duration = Duration::from_millis(320);

const TLS_1_3: usize = 0x0304;

/// What is looked at first, and the most a TLS record holds.
const FIRST_LOOK: usize = 2048;
const MAX_RECORD: usize = 16 * 1024;

pub enum Peeked {
    Tls(Hello),
    /// The first bytes are not a TLS handshake record.
    NotTls,
    /// The client sent nothing in time (a protocol where the server speaks first).
    Silent,
}

/// Looks at the start of the stream until a whole ClientHello is there.
pub async fn peek(stream: &TcpStream) -> std::io::Result<Peeked> {
    // A hello is a few hundred bytes, two thousand with the keys now sent
    // in it: room for that, and for the most a record can be only when one
    // says it is longer.
    let mut buf = vec![0u8; FIRST_LOOK];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut last = 0;
    let mut wait = Duration::from_millis(1);
    loop {
        let n = match tokio::time::timeout_at(deadline, stream.peek(&mut buf)).await {
            Ok(r) => r?,
            Err(_) => return Ok(Peeked::Silent),
        };
        if n == 0 {
            return Ok(Peeked::NotTls);
        }
        if buf[0] != 0x16 || (n >= 2 && buf[1] != 0x03) {
            return Ok(Peeked::NotTls);
        }
        if n >= 5 {
            let len = (u16::from_be_bytes([buf[3], buf[4]]) as usize).min(MAX_RECORD);
            if 5 + len > buf.len() {
                buf.resize(5 + len, 0);
                continue;
            }
            if n >= 5 + len {
                return Ok(match parse(&buf[5..n.min(5 + len)]) {
                    Some(h) => Peeked::Tls(h),
                    None => Peeked::NotTls,
                });
            }
        }
        // peek() returns at once while the same bytes are there, and there
        // is no asking to be woken for more of them: look again after a
        // wait that doubles, so that a client dribbling its hello costs a
        // few wakeups and not hundreds.
        if n == last {
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(MAX_WAIT);
            if tokio::time::Instant::now() >= deadline {
                return Ok(Peeked::Silent);
            }
        }
        last = n;
    }
}

struct Cur<'a>(&'a [u8]);

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<usize> {
        self.take(1).map(|b| b[0] as usize)
    }
    fn u16(&mut self) -> Option<usize> {
        self.take(2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
    }
}

pub(crate) fn parse(record: &[u8]) -> Option<Hello> {
    let mut c = Cur(record);
    if c.u8()? != 1 {
        return None;
    }
    // Decide only from a complete ClientHello.
    let length = c.take(3)?;
    let len = u32::from_be_bytes([0, length[0], length[1], length[2]]) as usize;
    let mut c = Cur(c.take(len)?);
    c.take(2 + 32)?;
    let n = c.u8()?;
    c.take(n)?;
    let n = c.u16()?;
    c.take(n)?;
    let n = c.u8()?;
    c.take(n)?;
    if c.0.is_empty() {
        return Some(Hello::default());
    }
    let n = c.u16()?;
    let extensions = Cur(c.take(n)?);
    if !c.0.is_empty() {
        return None;
    }
    parse_extensions(extensions)
}

fn parse_extensions(mut ext: Cur<'_>) -> Option<Hello> {
    let mut hello = Hello::default();
    let mut named = false;
    while !ext.0.is_empty() {
        let ty = ext.u16()?;
        let len = ext.u16()?;
        let body = ext.take(len)?;
        let mut b = Cur(body);
        match ty {
            0 => {
                // A server refuses a hello that names it twice. Nothing is
                // decided from one: which name would the server go by?
                if std::mem::replace(&mut named, true) {
                    return None;
                }
                hello.sni = parse_server_name(b)?;
            }
            16 => {
                b.u16()?;
                while let Some(n) = b.u8() {
                    hello.alpn.push(b.take(n)?.to_vec());
                }
            }
            0xfe0d => {
                // Only the outer form can appear in the clear. The rest is
                // opaque here, and GREASE looks like a real offer.
                if b.u8()? != 0 {
                    return None;
                }
                hello.ech = true;
            }
            43 => hello.tls13_only = offers_only_tls13(b)?,
            _ => {}
        }
    }
    Some(hello)
}

fn parse_server_name(mut b: Cur<'_>) -> Option<Option<String>> {
    b.u16()?;
    let mut name = None;
    while let Some(kind) = b.u8() {
        let n = b.u16()?;
        let bytes = b.take(n)?;
        if kind == 0 {
            name = std::str::from_utf8(bytes)
                .ok()
                .map(|s| s.to_ascii_lowercase());
        }
    }
    Some(name)
}

fn offers_only_tls13(mut b: Cur<'_>) -> Option<bool> {
    let n = b.u8()?;
    let mut versions = Cur(b.take(n)?);
    let mut oldest = None;
    while let Some(v) = versions.u16() {
        // The values clients send to keep servers tolerant of
        // new ones (0x?a?a) are not versions.
        if v & 0x0f0f != 0x0a0a {
            oldest = Some(oldest.map_or(v, |o: usize| o.min(v)));
        }
    }
    Some(oldest.is_some_and(|v| v >= TLS_1_3))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    /// The first flight of a real client: the record with its ClientHello.
    fn real_hello(host: &str, alpn: &[&str]) -> Vec<u8> {
        client_hello(host, alpn, None)
    }

    fn client_hello(host: &str, alpn: &[&str], ech: Option<rustls::client::EchMode>) -> Vec<u8> {
        let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ));
        let mut config = match ech {
            Some(mode) => builder.with_ech(mode),
            None => builder.with_safe_default_protocol_versions(),
        }
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
        let name = rustls::pki_types::ServerName::try_from(host.to_owned()).unwrap();
        let mut conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
        let mut out = Vec::new();
        conn.write_tls(&mut out).unwrap();
        out
    }

    /// A ClientHello with nothing in it but the given extensions.
    fn hello_with(extensions: &[(u16, &[u8])]) -> Vec<u8> {
        let mut ext = Vec::new();
        for (kind, body) in extensions {
            ext.extend_from_slice(&kind.to_be_bytes());
            ext.extend_from_slice(&(body.len() as u16).to_be_bytes());
            ext.extend_from_slice(body);
        }
        let mut hello = vec![3, 3];
        hello.extend_from_slice(&[0; 32]); // random
        hello.push(0); // session id
        hello.extend_from_slice(&[0, 2, 0x13, 0x01]); // cipher suites
        hello.extend_from_slice(&[1, 0]); // compression methods
        hello.extend_from_slice(&(ext.len() as u16).to_be_bytes());
        hello.extend_from_slice(&ext);
        let mut handshake = vec![1, 0];
        handshake.extend_from_slice(&(hello.len() as u16).to_be_bytes());
        handshake.extend_from_slice(&hello);
        handshake
    }

    /// A connection to this process: the end to write on, and the end read.
    async fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        (client, server)
    }

    #[test]
    fn reads_the_name_and_protocols_a_real_client_sends() {
        let record = real_hello("www.Example.com", &["h2", "http/1.1"]);
        let hello = parse(&record[5..]).unwrap();
        // Names are compared as the rules write them, in lower case.
        assert_eq!(hello.sni.as_deref(), Some("www.example.com"));
        assert_eq!(hello.alpn, [b"h2".to_vec(), b"http/1.1".to_vec()]);
    }

    #[test]
    fn a_hello_naming_the_server_twice_decides_nothing() {
        let name: &[u8] = &[0, 4, 0, 0, 1, b'a'];
        assert!(parse(&hello_with(&[(0, name)])).is_some());
        assert!(parse(&hello_with(&[(0, name), (0, name)])).is_none());
    }

    #[test]
    fn knows_a_client_that_speaks_tls_1_3_only() {
        let versions = |list: &[u8]| {
            let mut body = vec![list.len() as u8];
            body.extend_from_slice(list);
            parse(&hello_with(&[(43, &body)])).unwrap().tls13_only
        };
        assert!(versions(&[3, 4]));
        assert!(!versions(&[3, 4, 3, 3]));
        // With a value that is no version among them.
        assert!(versions(&[0x1a, 0x1a, 3, 4]));
        assert!(!versions(&[]));
        // A real client offers 1.2 as well.
        let record = real_hello("a.example", &[]);
        assert!(!parse(&record[5..]).unwrap().tls13_only);
    }

    #[test]
    fn a_hello_without_a_name_has_none() {
        let hello = parse(&hello_with(&[])).unwrap();
        assert_eq!(hello.sni, None);
        assert!(hello.alpn.is_empty());
    }

    #[test]
    fn a_grease_ech_hello_keeps_its_site_name() {
        let record = client_hello("site.test", &["h2"], Some(ech_fixture::grease()));
        let hello = parse(&record[5..]).unwrap();
        assert_eq!(hello.sni.as_deref(), Some("site.test"));
        assert_eq!(hello.alpn, [b"h2".to_vec()]);
        assert!(hello.ech);
    }

    #[test]
    fn a_real_ech_offer_exposes_the_public_name() {
        let record = client_hello("site.test", &[], Some(ech_fixture::offer("public.test")));
        let hello = parse(&record[5..]).unwrap();
        assert_eq!(hello.sni.as_deref(), Some("public.test"));
        assert!(hello.ech);
    }

    #[test]
    fn an_ech_extension_is_checked_only_for_its_outer_type() {
        let public_name = [0, 4, 0, 0, 1, b'a'];
        for body in [&[1][..], &[0xff], &[]] {
            let hello = hello_with(&[(0, &public_name), (0xfe0d, body)]);
            assert!(parse(&hello).is_none());
        }
        let hello = hello_with(&[(0, &public_name), (0xfe0d, &[0])]);
        assert!(parse(&hello).unwrap().ech);
        assert!(parse(&hello[..hello.len() - 5]).is_none());
    }

    #[test]
    fn partial_hellos_are_handed_on_without_a_name() {
        let record = real_hello("www.example.com", &["h2"]);
        // What matters is that none of these panics or reads past the end.
        for len in 0..record.len() - 5 {
            assert!(parse(&record[5..5 + len]).is_none());
        }
        assert!(parse(&[]).is_none());
        // Not a ClientHello at all.
        assert!(parse(&[2, 0, 0, 0]).is_none());
    }

    #[test]
    fn bytes_after_the_handshake_do_not_extend_its_extensions() {
        let name: &[u8] = &[0, 4, 0, 0, 1, b'a'];
        let mut handshake = hello_with(&[(0, name)]);
        handshake.push(0);
        assert_eq!(parse(&handshake).unwrap().sni.as_deref(), Some("a"));

        // Now include the same byte in the declared ClientHello, outside
        // the extensions block. There is no valid name to decide from.
        handshake[3] += 1;
        assert!(parse(&handshake).is_none());
    }

    #[test]
    fn lengths_that_lie_do_not_reach_past_the_record() {
        let mut record = hello_with(&[(0, &[0, 5, 0, 0, 2, b'a', b'b'])]);
        assert_eq!(parse(&record).unwrap().sni.as_deref(), Some("ab"));
        // The name claims to be longer than what follows.
        let at = record.len() - 3;
        record[at] = 200;
        assert_eq!(parse(&record).and_then(|h| h.sni), None);
    }

    #[tokio::test]
    async fn peeking_leaves_the_bytes_to_be_read_again() {
        let (mut client, server) = pair().await;
        let record = real_hello("a.example", &[]);
        client.write_all(&record).await.unwrap();
        let Peeked::Tls(hello) = peek(&server).await.unwrap() else {
            panic!("not taken for TLS");
        };
        assert_eq!(hello.sni.as_deref(), Some("a.example"));
        let mut again = vec![0; record.len()];
        tokio::io::AsyncReadExt::read_exact(&mut { server }, &mut again)
            .await
            .unwrap();
        assert_eq!(again, record);
    }

    #[tokio::test]
    #[ignore = "a measurement, run by hand"]
    async fn measures_the_delay_for_a_split_client_hello() {
        let hello = real_hello("example.com", &["h2"]);
        let mut total = Duration::ZERO;
        for _ in 0..50 {
            let (mut sender, receiver) = pair().await;
            sender.write_all(&hello[..10]).await.unwrap();
            let client = tokio::spawn(async move { peek(&receiver).await.unwrap() });
            tokio::task::yield_now().await;
            let ready = std::time::Instant::now();
            sender.write_all(&hello[10..]).await.unwrap();
            assert!(matches!(client.await.unwrap(), Peeked::Tls(_)));
            total += ready.elapsed();
        }
        println!(
            "split ClientHello: {:?} after the second segment",
            total / 50
        );
    }

    #[tokio::test]
    async fn waits_for_a_hello_that_arrives_in_pieces() {
        let (mut client, server) = pair().await;
        let record = real_hello("pieces.example", &["h2"]);
        let (first, rest) = record.split_at(20);
        client.write_all(first).await.unwrap();
        let rest = rest.to_vec();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            client.write_all(&rest).await.unwrap();
            // Held open until the other end has looked.
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let Peeked::Tls(hello) = peek(&server).await.unwrap() else {
            panic!("not taken for TLS");
        };
        assert_eq!(hello.sni.as_deref(), Some("pieces.example"));
    }

    #[tokio::test]
    async fn a_hello_larger_than_the_first_look_is_read_whole() {
        let (mut client, server) = pair().await;
        // A name at the end, behind an extension of padding.
        let padding = vec![0u8; 3 * FIRST_LOOK];
        let name: &[u8] = &[0, 9, 0, 0, 6, b'f', b'a', b'r', b'.', b'e', b'x'];
        let handshake = hello_with(&[(21, &padding), (0, name)]);
        let mut record = vec![0x16, 3, 1];
        record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
        record.extend_from_slice(&handshake);
        client.write_all(&record).await.unwrap();
        let Peeked::Tls(hello) = peek(&server).await.unwrap() else {
            panic!("not taken for TLS");
        };
        assert_eq!(hello.sni.as_deref(), Some("far.ex"));
    }

    #[tokio::test]
    async fn what_does_not_start_as_tls_is_not_taken_for_it() {
        let (mut client, server) = pair().await;
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: a.example\r\n\r\n")
            .await
            .unwrap();
        assert!(matches!(peek(&server).await.unwrap(), Peeked::NotTls));
    }

    #[tokio::test]
    async fn a_closed_connection_is_not_tls() {
        let (client, server) = pair().await;
        drop(client);
        assert!(matches!(peek(&server).await.unwrap(), Peeked::NotTls));
    }

    #[tokio::test(start_paused = true)]
    async fn a_client_that_says_nothing_is_given_up_on() {
        let (_client, server) = pair().await;
        assert!(matches!(peek(&server).await.unwrap(), Peeked::Silent));
    }
}
