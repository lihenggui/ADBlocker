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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;

use anyhow::{Result, anyhow};
use hyper::Request;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use super::request::{InspectedSite, Shared, finish_tunnel, serve};
use super::upstream::{HANDSHAKE_TIME, Upstream, carry, connect, splice};
use super::{Ctx, Origin, Trusting, trace};
use crate::clienthello::Hello;
use crate::policy::Reason;
use crate::stats::Counter;
use crate::tls::Tls;

/// A client that checks a pin after the handshake closes at once when the
/// check fails. A connection a browser opened ahead of need and never used
/// also ends without a request, but only after sitting idle for a while.
const PIN_CHECK_TIME: Duration = Duration::from_secs(1);

pub(super) struct Peer {
    pub(super) origin: Origin,
    pub(super) package: String,
    pub(super) host: String,
    /// The ticket of the try of a pinned host this connection is
    /// (`Policy::take_try`), if it is one.
    pub(super) retry: Option<u64>,
}

struct Verified {
    stream: tokio_rustls::client::TlsStream<TcpStream>,
    profile: Option<(u32, Arc<Tls>)>,
}

fn profile_current(ctx: &Ctx, peer: &Peer, verified: &Verified) -> bool {
    verified.profile.as_ref().is_none_or(|(serial, tls)| {
        let still_current = ctx.selection.profile_tls_is_current(*serial, tls);
        still_current && ctx.is_package(peer.origin.uid, &peer.package)
    })
}

pub(super) async fn mitm(
    ctx: Arc<Ctx>,
    app: TcpStream,
    peer: Peer,
    hello: Hello,
    trusting: Trusting,
) -> Result<()> {
    // On the loopback interface nothing is gained by waiting to fill a
    // packet, and a handshake waits out a delayed acknowledgement for it.
    app.set_nodelay(true)?;
    let Some(verified) = verify_upstream(&ctx, &app, &peer, &hello).await? else {
        return splice(&ctx, app, peer.origin).await;
    };
    if let Some(reason) = still_inspectable(&ctx, &peer) {
        ctx.passed(&peer.package, &peer.host, reason);
        drop(verified);
        return splice(&ctx, app, peer.origin).await;
    }
    if !profile_current(&ctx, &peer, &verified) {
        drop(verified);
        return splice(&ctx, app, peer.origin).await;
    }
    let alpn = verified
        .stream
        .get_ref()
        .1
        .alpn_protocol()
        .map(<[u8]>::to_vec);
    let tls = verified
        .profile
        .as_ref()
        .map_or(&ctx.services.tls, |(_, tls)| tls.as_ref());
    let Some(app) = accept_client(&ctx, app, &peer, tls, alpn.as_deref(), trusting).await? else {
        return Ok(());
    };
    ctx.services
        .stats
        .count(Some(&peer.package), Counter::Decrypted);
    serve_decrypted(ctx, app, verified, peer, alpn, trusting).await
}

async fn verify_upstream(
    ctx: &Ctx,
    app: &TcpStream,
    peer: &Peer,
    hello: &Hello,
) -> Result<Option<Verified>> {
    let Peer {
        origin,
        package,
        host,
        ..
    } = peer;
    let Ok(name) = ServerName::try_from(host.clone()) else {
        // Not a name a certificate could be checked against, so not one
        // to answer for: the app is given whatever is there.
        trace!(ctx, "pass    {package} {host} (not a name)");
        ctx.passed(package, host, Reason::Unverified);
        return Ok(None);
    };
    let serial = crate::apps::Identity::parse(package).and_then(|app| app.profile_serial);
    let profile_tls = serial.and_then(|serial| {
        ctx.selection
            .profile_tls
            .read()
            .unwrap()
            .get(&serial)
            .cloned()
    });
    if serial.is_some() && profile_tls.is_none() {
        ctx.passed(package, host, Reason::Untrusted);
        return Ok(None);
    }
    let tls = profile_tls.as_deref().unwrap_or(&ctx.services.tls);
    let up_tcp = connect(ctx, app, *origin).await?;
    let handshake = TlsConnector::from(tls.client(hello)?).connect(name, up_tcp);
    let up = match tokio::time::timeout(HANDSHAKE_TIME, handshake).await {
        Ok(Ok(s)) => s,
        // Not a server we can vouch for, or none that answers: the app's
        // bytes are unread, and it is given the server itself to judge.
        failed => {
            let why = failed.map_or("timed out".to_owned(), |r| {
                r.err().map_or_else(String::new, |e| e.to_string())
            });
            trace!(ctx, "pass    {package} {host} (server: {why})");
            ctx.passed(package, host, Reason::Unverified);
            return Ok(None);
        }
    };
    // A server that proves who is behind it, as banks and payment
    // services do, is not for us to read: the connection made to find out
    // is dropped, and the app is given one of its own.
    let certificate = up.get_ref().1.peer_certificates().and_then(|c| c.first());
    if certificate.is_some_and(|c| tls.extended_validation(host, c)) {
        trace!(ctx, "pass    {package} {host} (extended validation)");
        ctx.services.policy.found_sensitive(host);
        ctx.passed(package, host, Reason::Sensitive);
        drop(up);
        return Ok(None);
    }
    Ok(Some(Verified {
        stream: up,
        profile: serial.zip(profile_tls),
    }))
}

// A reload can add exclusions or lose readiness while we await the
// upstream handshake. The app still has its original ClientHello.
fn still_inspectable(ctx: &Ctx, peer: &Peer) -> Option<Reason> {
    let Peer {
        origin,
        package,
        host,
        ..
    } = peer;
    let exclusions = ctx.lists.exclusions.read().unwrap();
    if !ctx.can_inspect_https(ctx.selection.url_filtering.load(Relaxed))
        || !ctx.is_package(origin.uid, package)
    {
        Some(Reason::DomainOnly)
    } else if ctx.lists.allow.read().unwrap().covers(host) {
        Some(Reason::Trusted)
    } else if exclusions.covers(package, host) {
        Some(Reason::Excluded)
    } else if !exclusions.ready() {
        Some(Reason::ExclusionsPending)
    } else {
        None
    }
}

async fn accept_client(
    ctx: &Ctx,
    app: TcpStream,
    peer: &Peer,
    tls: &Tls,
    alpn: Option<&[u8]>,
    trusting: Trusting,
) -> Result<Option<tokio_rustls::server::TlsStream<TcpStream>>> {
    let Peer { package, host, .. } = peer;
    let accept = TlsAcceptor::from(tls.server(alpn)?).accept(app);
    match tokio::time::timeout(HANDSHAKE_TIME, accept).await {
        Ok(Ok(s)) => Ok(Some(s)),
        Ok(Err(e)) => {
            trace!(ctx, "reject  {package} {host} ({e})");
            use rustls::AlertDescription as Alert;
            let error = e
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<rustls::Error>());
            let pinned = if let Some(ticket) = peer.retry {
                // The try is spent; the host was pinned before.
                ctx.services.policy.try_refused(package, host, ticket);
                false
            } else if trusting == Trusting::Newly {
                false
            } else {
                match error {
                    Some(rustls::Error::AlertReceived(
                        Alert::BadCertificate
                        | Alert::UnknownCA
                        | Alert::CertificateUnknown
                        | Alert::UnsupportedCertificate
                        | Alert::CertificateRevoked
                        | Alert::CertificateExpired,
                    )) => ctx.services.policy.rejected(package, host),
                    Some(rustls::Error::AlertReceived(Alert::EncryptedClientHelloRequired)) => {
                        false
                    }
                    // A silent close counts even from a client that offered
                    // ECH: Chrome and Cronet offer it on every connection, and
                    // an app that pins without an alert must not stay broken.
                    // A wrong record only leaves that name encrypted.
                    _ => ctx.services.policy.closed_silent(package, host),
                }
            };
            if pinned {
                ctx.services.stats.count(Some(package), Counter::Pinned);
            }
            Ok(None)
        }
        Err(_) => {
            // It was shown our certificate, and did not go on.
            if let Some(ticket) = peer.retry {
                ctx.services.policy.try_refused(package, host, ticket);
            }
            Err(anyhow!("client handshake timed out"))
        }
    }
}

async fn serve_decrypted(
    ctx: Arc<Ctx>,
    app: tokio_rustls::server::TlsStream<TcpStream>,
    verified: Verified,
    peer: Peer,
    alpn: Option<Vec<u8>>,
    trusting: Trusting,
) -> Result<()> {
    let Verified {
        stream: up,
        profile,
    } = verified;
    let h2 = alpn.as_deref() == Some(b"h2");
    let (upstream, up_task) = start_upstream(up, h2).await?;
    let spoke = Arc::new(AtomicBool::new(false));
    // What every request of the connection needs, shared rather than
    // copied for each. Each request uses the current list snapshot.
    let shared = Shared {
        ctx: ctx.clone(),
        https: Some(InspectedSite {
            name: peer.host.clone(),
            port: peer.origin.dst.port(),
            profile,
            stop: up_task.abort_handle(),
        }),
        uid: peer.origin.uid,
        package: peer.package.clone(),
        tunnel: std::sync::Mutex::new(None),
    };
    let connection = Arc::new((shared, upstream));
    let retry = peer.retry;
    let svc = {
        let (connection, spoke) = (connection.clone(), spoke.clone());
        service_fn(move |req: Request<Incoming>| {
            // Said at the first request, not when the connection ends:
            // one that goes on for hours accepts us all the while.
            if !spoke.swap(true, Relaxed) {
                connection.0.ctx.services.policy.spoke(
                    &connection.0.package,
                    &connection.0.https.as_ref().unwrap().name,
                    retry,
                );
            }
            let connection = connection.clone();
            async move {
                let (shared, upstream) = &*connection;
                serve(shared, req, std::future::ready(Ok(upstream))).await
            }
        })
    };
    let app_io = TokioIo::new(app);
    let since = std::time::Instant::now();
    let app_closed = if h2 {
        let conn = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
            .serve_connection(app_io, svc);
        carry(conn, up_task, |conn| conn.graceful_shutdown()).await
    } else {
        let conn = hyper::server::conn::http1::Builder::new()
            .serve_connection(app_io, svc)
            .with_upgrades();
        carry(conn, up_task, |conn| conn.graceful_shutdown()).await
    };
    record_close(&ctx, &peer, trusting, &spoke, app_closed, since);
    // A connection that became a tunnel is still one carried: its place
    // among them is kept until it ends.
    finish_tunnel(&connection.0).await;
    Ok(())
}

async fn start_upstream(
    up: tokio_rustls::client::TlsStream<TcpStream>,
    h2: bool,
) -> Result<(Upstream, JoinHandle<()>)> {
    let up_io = TokioIo::new(up);
    if h2 {
        let (tx, conn) = hyper::client::conn::http2::handshake(TokioExecutor::new(), up_io).await?;
        let task = tokio::spawn(async move {
            conn.await.ok();
        });
        Ok((Upstream::H2(tx), task))
    } else {
        let (tx, conn) = hyper::client::conn::http1::handshake(up_io).await?;
        let task = tokio::spawn(async move {
            conn.with_upgrades().await.ok();
        });
        Ok((Upstream::H1(Mutex::new(tx)), task))
    }
}

fn record_close(
    ctx: &Ctx,
    peer: &Peer,
    trusting: Trusting,
    spoke: &AtomicBool,
    app_closed: bool,
    since: std::time::Instant,
) {
    let Peer { package, host, .. } = peer;
    if let Some(ticket) = peer.retry {
        // However and whenever it ended, a try without a request is one
        // the app refused.
        if !spoke.load(Relaxed) {
            ctx.services.policy.try_refused(package, host, ticket);
        }
        return;
    }
    if !spoke.load(Relaxed)
        && app_closed
        && trusting != Trusting::Newly
        && since.elapsed() < PIN_CHECK_TIME
    {
        trace!(ctx, "silent  {package} {host}");
        if ctx.services.policy.closed_silent(package, host) {
            ctx.services.stats.count(Some(package), Counter::Pinned);
        }
    }
}
