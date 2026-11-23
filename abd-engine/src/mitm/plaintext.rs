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

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::task::Poll;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, OnceCell};

use super::request::{Shared, clear_target, finish_tunnel, serve};
use super::upstream::{HANDSHAKE_TIME, Upstream, carry, dial};
use super::{Ctx, Origin};
use crate::clienthello;

/// Only a complete, ordinary HTTP/1 request can switch port 80 away from
/// byte forwarding. The same bounds apply even if the sender dribbles
/// bytes; the upstream receives every byte unchanged on an opaque stream.
pub(super) const HTTP_LOOK: usize = 8192;
pub(super) const HTTP_LOOK_TIME: Duration = Duration::from_secs(5);

pub(super) async fn plaintext_http(
    app: &TcpStream,
    deadline: tokio::time::Instant,
) -> std::io::Result<bool> {
    let mut buf = vec![0; HTTP_LOOK];
    let mut previous = 0;
    let mut wait = Duration::from_millis(5);
    loop {
        let n = match tokio::time::timeout_at(deadline, app.peek(&mut buf)).await {
            Ok(result) => result?,
            Err(_) => return Ok(false),
        };
        if n == 0 {
            return Ok(false);
        }
        let mut headers = [httparse::EMPTY_HEADER; 100];
        let mut request = httparse::Request::new(&mut headers);
        match request.parse(&buf[..n]) {
            Ok(httparse::Status::Complete(_)) => {
                let mut hosts = request
                    .headers
                    .iter()
                    .filter(|h| h.name.eq_ignore_ascii_case("host"));
                let host = hosts.next().and_then(|h| std::str::from_utf8(h.value).ok());
                let method = request.method.and_then(|s| s.parse().ok());
                return Ok(hosts.next().is_none()
                    && reliable_body_framing(request.version, request.headers)
                    && method.zip(request.path).zip(host).is_some_and(
                        |((method, path), host)| clear_target(&method, path, host).is_some(),
                    ));
            }
            Ok(httparse::Status::Partial) if n < buf.len() => {}
            _ => return Ok(false),
        }
        // A non-consuming peek cannot wait for a larger read. Limit the
        // wakeups while the app is still writing its headers.
        if n == previous {
            tokio::time::sleep_until((tokio::time::Instant::now() + wait).min(deadline)).await;
            wait = (wait * 2).min(clienthello::MAX_WAIT);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        previous = n;
    }
}

fn reliable_body_framing(version: Option<u8>, headers: &[httparse::Header<'_>]) -> bool {
    let mut length = None;
    let mut transfer = None;
    for header in headers {
        if header.name.eq_ignore_ascii_case("content-length") {
            let len = std::str::from_utf8(header.value).ok().and_then(|value| {
                let value = value.trim();
                value
                    .bytes()
                    .all(|b| b.is_ascii_digit())
                    .then(|| value.parse::<u64>().ok())
                    .flatten()
            });
            if len.is_none() || length.is_some_and(|previous| Some(previous) != len) {
                return false;
            }
            length = len;
        } else if header.name.eq_ignore_ascii_case("transfer-encoding") {
            transfer = Some(std::str::from_utf8(header.value).is_ok_and(|value| {
                value
                    .rsplit(',')
                    .next()
                    .is_some_and(|name| name.trim().eq_ignore_ascii_case("chunked"))
            }));
        }
    }
    transfer.is_none() || (transfer == Some(true) && version == Some(1) && length.is_none())
}

pub(super) async fn serve_plaintext(
    ctx: Arc<Ctx>,
    app: TcpStream,
    origin: Origin,
    package: String,
) -> Result<()> {
    let shared = Shared {
        ctx,
        https: None,
        uid: origin.uid,
        package,
        tunnel: std::sync::Mutex::new(None),
    };
    let upstream = OnceCell::<Result<Upstream>>::new();
    let driver = std::sync::Mutex::new(None);
    let reset = AtomicBool::new(false);
    let service = service_fn(|request| {
        serve(&shared, request, async {
            upstream
                .get_or_init(|| async {
                    // Origin already holds the accepted socket's identity,
                    // mark and destination. Host is only for rule matching.
                    let socket = dial(&shared.ctx, origin)
                        .await
                        .inspect_err(|_| reset.store(true, Relaxed))
                        .context("connect")?;
                    let (tx, conn) = tokio::time::timeout(
                        HANDSHAKE_TIME,
                        hyper::client::conn::http1::handshake(TokioIo::new(socket)),
                    )
                    .await??;
                    *driver.lock().unwrap() = Some(conn.with_upgrades());
                    Ok(Upstream::H1(Mutex::new(tx)))
                })
                .await
                .as_ref()
                .map_err(|_| anyhow!("could not open cleartext HTTP upstream"))
        })
    });
    let mut conn = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(app), service)
        .with_upgrades();
    // The service installs the driver while carry polls the app first.
    // Polling both here lets the driver drain or drop with this connection.
    let mut started = None;
    let upstream_driver = poll_fn(|cx| {
        if started.is_none() {
            started = driver.lock().unwrap().take();
        }
        match &mut started {
            Some(driver) => Pin::new(driver).poll(cx),
            None => Poll::Pending,
        }
    });
    carry(&mut conn, upstream_driver, |conn| {
        Pin::new(&mut **conn.get_mut()).graceful_shutdown();
    })
    .await;
    if reset.load(Relaxed) {
        if let Some(parts) = conn.into_parts() {
            super::upstream::reset(parts.io.inner());
        }
    } else {
        drop(conn);
    }
    drop(started);
    finish_tunnel(&shared).await;
    if let Some(Err(error)) = upstream.into_inner() {
        return Err(error);
    }
    Ok(())
}
