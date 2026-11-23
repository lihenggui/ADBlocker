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

//! Device check for an H3-only origin and client. Only the fixture certificate
//! is trusted. The client sends concurrent requests over the same QUIC connection.

use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result, ensure};
use bytes::{Buf, Bytes};
use quinn::{
    Endpoint,
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};

const HOST: &str = "fixture.example";
const BANNER: &str = "/ads/banner";
const ARTICLE: &str = "/articles/today";
const EXCEPTED: &str = "/ads/banner?slot=fixture-allowed";
const PATHS: &[&str] = &[BANNER, ARTICLE];
const FULL_PATHS: &[&str] = &[BANNER, ARTICLE, EXCEPTED];

fn cert(path: &Path) -> Result<CertificateDer<'static>> {
    Ok(CertificateDer::from(std::fs::read(path)?))
}

fn server(addr: SocketAddr, chain: Vec<CertificateDer<'static>>, key: &Path) -> Result<Endpoint> {
    let mut tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(chain, PrivatePkcs8KeyDer::from(std::fs::read(key)?).into())?;
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let config = quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
    Ok(Endpoint::server(config, addr)?)
}

async fn origin(endpoint: Endpoint, host: &str, expected: &[&str]) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    println!("listening {}", endpoint.local_addr()?);
    let conn = endpoint.accept().await.context("endpoint closed")?.await?;
    assert_h3(&conn)?;
    println!("h3 origin connection");
    let mut h3 = h3::server::Connection::new(h3_quinn::Connection::new(conn)).await?;
    loop {
        let stream = match h3.accept().await {
            Ok(Some(stream)) => stream,
            Ok(None) => break,
            Err(err) if err.is_h3_no_error() => break,
            Err(err) => return Err(err.into()),
        };
        let (req, mut stream) = stream.resolve_request().await?;
        let path = req
            .uri()
            .path_and_query()
            .context("no fixture path")?
            .as_str();
        ensure!(
            req.method() == http::Method::GET
                && req.uri().scheme_str() == Some("https")
                && req
                    .uri()
                    .authority()
                    .is_some_and(|a| a.host() == host && a.port_u16().is_none_or(|p| p == 443))
                && expected.contains(&path),
            "unrecognized fixture request"
        );
        println!("origin received {path}");
        ensure!(paths.len() < expected.len(), "too many fixture requests");
        paths.push(path.to_owned());
        stream.send_response(http::Response::new(())).await?;
        stream.send_data(Bytes::from_static(b"origin")).await?;
        stream.finish().await?;
    }
    Ok(paths)
}

async fn connect(
    addr: SocketAddr,
    host: &str,
    roots: rustls::RootCertStore,
) -> Result<(Endpoint, quinn::Connection)> {
    let mut tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let any = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let mut endpoint = Endpoint::client(any.parse()?)?;
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(tls)?,
    )));
    let conn = endpoint.connect(addr, host)?.await?;
    assert_h3(&conn)?;
    Ok((endpoint, conn))
}

fn assert_h3(conn: &quinn::Connection) -> Result<()> {
    let info = conn
        .handshake_data()
        .context("no TLS handshake")?
        .downcast::<quinn::crypto::rustls::HandshakeData>()
        .map_err(|_| anyhow::anyhow!("unexpected crypto provider"))?;
    ensure!(info.protocol.as_deref() == Some(b"h3"), "h3 not negotiated");
    Ok(())
}

fn roots(certificates: Vec<CertificateDer<'static>>) -> Result<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(certificates);
    ensure!(!roots.is_empty(), "no usable trust anchors");
    Ok(roots)
}

async fn fetch(
    addr: SocketAddr,
    host: &str,
    roots: rustls::RootCertStore,
    full: bool,
) -> Result<http::StatusCode> {
    let (endpoint, conn) = connect(addr, host, roots).await?;
    if full {
        println!("h3 client connection");
    }
    let paths = if full { FULL_PATHS } else { PATHS };
    let (mut driver, send) = h3::client::new(h3_quinn::Connection::new(conn)).await?;
    let drive = tokio::spawn(async move { driver.wait_idle().await });
    let request = |path: &'static str| {
        let mut send = send.clone();
        let url = format!("https://{host}{path}");
        async move {
            let mut stream = send.send_request(http::Request::get(url).body(())?).await?;
            stream.finish().await?;
            let status = stream.recv_response().await?.status();
            let mut received = Vec::new();
            while let Some(mut chunk) = stream.recv_data().await? {
                ensure!(
                    received.len() + chunk.remaining() <= 64,
                    "fixture body too large"
                );
                received.extend_from_slice(&chunk.copy_to_bytes(chunk.remaining()));
            }
            if status == http::StatusCode::OK {
                ensure!(received == b"origin", "unexpected fixture response body");
            }
            println!("h3 {path} {}", status.as_u16());
            Ok::<_, anyhow::Error>((status, received))
        }
    };
    let mut streams = tokio::task::JoinSet::new();
    for &path in paths {
        let response = request(path);
        streams.spawn(async move { Ok::<_, anyhow::Error>((path, response.await?)) });
    }
    let mut banner = http::StatusCode::FORBIDDEN;
    while let Some(result) = streams.join_next().await {
        let (path, (status, body)) = result??;
        if path == BANNER {
            banner = status;
        }
        if path == EXCEPTED || path == ARTICLE || status != http::StatusCode::FORBIDDEN {
            ensure!(
                status == http::StatusCode::OK && body == b"origin",
                "unexpected origin response"
            );
        }
    }
    endpoint.close(0x100u32.into(), b"done");
    drive.abort();
    endpoint.wait_idle().await;
    Ok(banner)
}

// Both endpoints belong to the fixture and the upstream peer is fixed before
// the client connects. No request header can supply a destination.
async fn filter_fixture(
    endpoint: Endpoint,
    upstream: SocketAddr,
    cert_path: &Path,
    filters: &abd_engine::filter::Filters,
    streams: usize,
) -> Result<()> {
    let (out, conn) = connect(upstream, HOST, roots(vec![cert(cert_path)?])?).await?;
    let (mut driver, send) = h3::client::new(h3_quinn::Connection::new(conn)).await?;
    let driving = tokio::spawn(async move { driver.wait_idle().await });
    let incoming = endpoint.accept().await.context("endpoint closed")?.await?;
    let mut client = h3::server::Connection::new(h3_quinn::Connection::new(incoming)).await?;
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..streams {
        let next = client.accept().await?.context("missing fixture stream")?;
        let (req, mut stream) = next.resolve_request().await?;
        ensure!(
            req.method() == http::Method::GET
                && req.uri().scheme_str() == Some("https")
                && req.uri().authority().map(|a| a.as_str()) == Some(HOST),
            "unexpected fixture origin"
        );
        let blocked =
            abd_engine::mitm::evaluate_request(filters, &req, &req.uri().to_string()).blocked;
        let mut send = send.clone();
        workers.spawn(async move {
            if blocked {
                stream
                    .send_response(http::Response::builder().status(403).body(())?)
                    .await?;
            } else {
                let mut origin = send.send_request(req).await?;
                origin.finish().await?;
                stream.send_response(origin.recv_response().await?).await?;
                let mut received = 0;
                while let Some(mut chunk) = origin.recv_data().await? {
                    received += chunk.remaining();
                    ensure!(received <= 64, "fixture body too large");
                    stream
                        .send_data(chunk.copy_to_bytes(chunk.remaining()))
                        .await?;
                }
            }
            stream.finish().await?;
            Ok::<_, anyhow::Error>(())
        });
    }
    while let Some(worker) = workers.join_next().await {
        worker??;
    }
    match client.accept().await {
        Ok(None) => {}
        Err(err) if err.is_h3_no_error() => {}
        _ => anyhow::bail!("unexpected extra fixture stream"),
    }
    out.close(0x100u32.into(), b"done");
    out.wait_idle().await;
    driving.abort();
    Ok(())
}

async fn check_rules(crt: &Path, key: &Path, full: bool) -> Result<()> {
    let rules = tempfile::tempdir()?;
    let mut rules_text = format!("||{HOST}/ads/\n");
    let mut expected = vec![ARTICLE];
    let paths = if full {
        rules_text.push_str(&format!("@@|https://{HOST}{EXCEPTED}|\n"));
        expected.insert(0, EXCEPTED);
        FULL_PATHS
    } else {
        PATHS
    };
    std::fs::write(rules.path().join("fixture.txt"), rules_text)?;
    let filters = abd_engine::filter::Filters::load(&[rules.path()], false)?;
    let local = "127.0.0.1:0".parse()?;
    let backend = server(local, vec![cert(crt)?], key)?;
    let upstream = backend.local_addr()?;
    let frontend = server(local, vec![cert(crt)?], key)?;
    let addr = frontend.local_addr()?;
    let (mut received, (), banner) = tokio::try_join!(
        origin(backend, HOST, paths),
        filter_fixture(frontend, upstream, crt, &filters, paths.len()),
        fetch(addr, HOST, roots(vec![cert(crt)?])?, full)
    )?;
    ensure!(
        banner == http::StatusCode::FORBIDDEN,
        "banner was not blocked"
    );
    received.sort_unstable();
    ensure!(received == expected, "unexpected fixture origin requests");
    Ok(())
}

async fn check_refusal(addr: SocketAddr) -> Result<()> {
    let any = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = tokio::net::UdpSocket::bind(any).await?;
    socket.connect(addr).await?;
    let started = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(1), async {
        let initial = include_bytes!("../../testdata/quic/aioquic-v2-initial.bin");
        // An OUTPUT REJECT can fail the send with EPERM and still deliver
        // ICMP. Read the socket to verify the port-unreachable error itself.
        if let Err(error) = socket.send(initial).await
            && error.raw_os_error() != Some(libc::EPERM)
        {
            return Err(error.into());
        }
        let mut reply = [0; 1500];
        match socket.recv(&mut reply).await {
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => Ok(()),
            result => anyhow::bail!("expected UDP port unreachable, got {result:?}"),
        }
    })
    .await
    .context("UDP probe timed out instead of receiving port unreachable")??;
    println!("refused in {} ms", started.elapsed().as_millis());
    Ok(())
}

async fn cosmetic_origin(addr: SocketAddr, crt: &Path, key: &Path) -> Result<()> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert(crt)?],
            PrivatePkcs8KeyDer::from(std::fs::read(key)?).into(),
        )?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    let first = "window.fixtureAtStart = window.fixtureConstant ?? null;";
    let hash = "GZdAjEm/ya7tGFi1zr8pCzmJ/DmbklY3s5GYT2mz/Xc=";
    let policy =
        format!("default-src 'none'; script-src 'self' 'sha256-{hash}'; object-src 'none'");
    let body = format!(
        "<!doctype html><html><head><meta http-equiv=Content-Security-Policy content=\"{policy}\">\
         <script>{first}</script><script>window.fixtureForbidden = true;</script></head>\
         <body><div id=advert>Sponsored fixture</div><div id=control>Article</div></body></html>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Security-Policy: {policy}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    println!("listening {addr}");
    loop {
        let (socket, _) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let response = response.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(5), async {
                let mut tls = acceptor.accept(socket).await?;
                let mut request = Vec::new();
                while request.len() < 8192 {
                    request.push(tls.read_u8().await?);
                    if request.ends_with(b"\r\n\r\n") {
                        tls.write_all(response.as_bytes()).await?;
                        tls.shutdown().await?;
                        break;
                    }
                }
                Ok::<_, std::io::Error>(())
            })
            .await;
        });
    }
}

async fn tcp_origin(addr: SocketAddr) -> Result<()> {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("listening {addr}");
    loop {
        let (mut socket, _) = listener.accept().await?;
        // The proxy may connect before seeing the request. Count only the
        // HTTP paths actually received by the origin.
        let mut request = String::new();
        let read = tokio::time::timeout(
            Duration::from_secs(2),
            BufReader::new(&mut socket).read_line(&mut request),
        )
        .await;
        if let Ok(Ok(n)) = read
            && n > 0
        {
            let path = request.split_ascii_whitespace().nth(1).context("no path")?;
            println!("origin received {path}");
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\norigin",
                )
                .await?;
            if path == ARTICLE {
                return Ok(());
            }
        }
    }
}

async fn dual_fetch(mut addr: SocketAddr) -> Result<()> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    check_refusal(addr).await?;
    addr.set_port(80);
    for (path, expected) in [(BANNER, "403"), (ARTICLE, "200")] {
        let mut socket = tokio::net::TcpStream::connect(addr).await?;
        // The banner belongs to the article. Supply its source so a
        // subscribed exception for an unknown page does not mask the rule.
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {HOST}\r\nReferer: http://{HOST}{ARTICLE}\r\nConnection: close\r\n\r\n"
        );
        socket.write_all(request.as_bytes()).await?;
        let mut response = Vec::new();
        socket.take(4096).read_to_end(&mut response).await?;
        let status = std::str::from_utf8(&response)?
            .lines()
            .next()
            .and_then(|line| line.split_ascii_whitespace().nth(1));
        ensure!(
            status == Some(expected),
            "unexpected TCP status for {path}: {status:?}"
        );
        println!("tcp {path} {expected}");
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    tokio::time::timeout(Duration::from_secs(60), async {
        match args.as_slice() {
            [cmd, addr] if cmd == "dual-fetch" => dual_fetch(addr.parse()?).await,
            [cmd, addr] if cmd == "tcp-origin" => tcp_origin(addr.parse()?).await,
            [cmd, addr, crt, key] if cmd == "cosmetic-origin" => {
                cosmetic_origin(addr.parse()?, Path::new(crt), Path::new(key)).await
            }
            [cmd, addr, crt, key] if cmd == "origin" => origin(
                server(addr.parse()?, vec![cert(Path::new(crt))?], Path::new(key))?,
                HOST,
                PATHS,
            )
            .await
            .map(drop),
            [cmd, addr, crt] if cmd == "fetch" => fetch(
                addr.parse()?,
                HOST,
                roots(vec![cert(Path::new(crt))?])?,
                false,
            )
            .await
            .map(drop),
            [cmd, crt, key] if cmd == "check-rules" => {
                check_rules(Path::new(crt), Path::new(key), false).await
            }
            _ => anyhow::bail!(
                "expected origin, fetch, tcp-origin, cosmetic-origin, dual-fetch, or check-rules"
            ),
        }
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_the_allowed_h3_stream_reaches_the_fixture_origin() -> Result<()> {
        check_rules_with_test_identity(false).await
    }

    #[tokio::test]
    async fn an_exact_url_exception_and_the_article_reach_the_same_h3_origin() -> Result<()> {
        check_rules_with_test_identity(true).await
    }

    async fn check_rules_with_test_identity(full: bool) -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cert = dir.path().join("cert.der");
        let key = dir.path().join("key.der");
        let identity = rcgen::generate_simple_self_signed(vec![HOST.into()])?;
        std::fs::write(&cert, identity.cert.der())?;
        std::fs::write(&key, identity.key_pair.serialize_der())?;
        tokio::time::timeout(Duration::from_secs(10), check_rules(&cert, &key, full)).await?
    }
}
