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

use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{ACCEPT_ENCODING, REFERER};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::ServerName;
use tokio::sync::mpsc;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use super::plaintext::HTTP_LOOK;
use super::request::{address, kind};
use super::upstream::{HANDSHAKE_TIME, LINGER_TIME, no_way};

use http_body_util::Empty;
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};

use crate::ca::Authority;
use crate::clienthello::ech_fixture;
use rustls::RootCertStore;
use rustls::client::EchMode;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

use super::*;

const HOST: &str = "site.test";
const PACKAGE: &str = "com.app";
const UID: u32 = 10001;

/// A server, a daemon in front of it, and what is needed to be a
/// client of either.
struct Rig {
    ctx: Arc<Ctx>,
    /// Where the app connects: the daemon.
    proxy: SocketAddr,
    /// The requests that reached the server.
    hits: Arc<AtomicUsize>,
    dials: Arc<AtomicUsize>,
    /// The server's own certificate, and the root it is signed by.
    server_leaf: CertificateDer<'static>,
    server_root: CertificateDer<'static>,
    /// How often the app was made to trust us.
    made_to_trust: Arc<AtomicUsize>,
    dir: tempfile::TempDir,
}

struct Setup {
    rules: &'static str,
    /// What the server speaks.
    alpn: &'static [&'static str],
    /// Whether the daemon can verify the server.
    server_trusted: bool,
    /// The UID the connections are said to come from.
    uid: u32,
    /// Whether the server's certificate is an Extended Validation one.
    ev: bool,
    /// Whether the app's processes can be made to trust us.
    trusting: Trusting,
    exclusions: Exclusions,
    before_first_server_handshake: Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
    /// Whether the app is set to have its hosts judged only.
    domain_only: bool,
    /// Whether what the lists name is hidden on pages.
    cosmetic: bool,
    /// The hosts the user trusts, a line each.
    allow: &'static str,
    /// The original destination port and the test socket it maps to.
    plaintext: Option<(u16, SocketAddr)>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            rules: "",
            alpn: &["http/1.1"],
            server_trusted: true,
            uid: UID,
            ev: false,
            trusting: Trusting::Yes,
            exclusions: Exclusions::default().assume_ready(),
            before_first_server_handshake: None,
            domain_only: false,
            cosmetic: false,
            allow: "",
            plaintext: None,
        }
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

#[allow(
    clippy::too_many_lines,
    reason = "keeps the TLS peer, proxy and test hooks in one fixture"
)]
async fn rig(setup: Setup) -> Rig {
    let dir = tempfile::tempdir().unwrap();

    // The server, with a certificate from an authority of its own.
    let root_key = KeyPair::generate().unwrap();
    let mut root = CertificateParams::new(vec![]).unwrap();
    root.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let root_cert = root.self_signed(&root_key).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf = CertificateParams::new(vec![HOST.to_owned()]).unwrap();
    if setup.ev {
        // certificatePolicies naming 2.23.140.1.1, as an Extended
        // Validation certificate does.
        let policies = vec![0x30, 9, 0x30, 7, 0x06, 5, 0x67, 0x81, 0x0c, 0x01, 0x01];
        leaf.custom_extensions
            .push(rcgen::CustomExtension::from_oid_content(
                &[2, 5, 29, 32],
                policies,
            ));
    }
    let leaf = leaf
        .signed_by(&leaf_key, &rcgen::Issuer::from_params(&root, &root_key))
        .unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
    let mut config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf.der().clone()], key)
        .unwrap();
    config.alpn_protocols = setup.alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let mut before_first_server_handshake = setup.before_first_server_handshake;
    tokio::spawn(async move {
        loop {
            let (tcp, _) = server.accept().await.unwrap();
            let (acceptor, hits) = (acceptor.clone(), counted.clone());
            let before_handshake = before_first_server_handshake.take();
            tokio::spawn(async move {
                if let Some((arrived, resume)) = before_handshake {
                    arrived.send(()).unwrap();
                    resume.await.unwrap();
                }
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2");
                let svc = service_fn(move |mut req: Request<Incoming>| {
                    hits.fetch_add(1, Relaxed);
                    // `/upgrade` switches protocols and then never closes.
                    if req.uri().path() == "/upgrade" {
                        let upgraded = hyper::upgrade::on(&mut req);
                        tokio::spawn(async move {
                            let _held = upgraded.await;
                            std::future::pending::<()>().await;
                        });
                        return std::future::ready(Ok(Response::builder()
                            .status(StatusCode::SWITCHING_PROTOCOLS)
                            .header("connection", "upgrade")
                            .header("upgrade", "websocket")
                            .body(Full::new(Bytes::new()))
                            .unwrap()));
                    }
                    // `/big/<n>` is answered with n MiB, for the measurements.
                    let body = match req.uri().path().strip_prefix("/big/") {
                        Some(n) => Bytes::from(vec![0u8; n.parse::<usize>().unwrap() << 20]),
                        None => Bytes::from(format!("{} {}", req.method(), req.uri().path())),
                    };
                    let mut resp = Response::new(Full::new(body));
                    // `/page…` is a page, which says how it was asked for.
                    if req.uri().path().starts_with("/page") {
                        let asked = req
                            .headers()
                            .get(ACCEPT_ENCODING)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("none");
                        // `/page/named` has an element with a class.
                        let named = if req.uri().path().ends_with("/named") {
                            "<div class=\"ad-box\"></div>"
                        } else {
                            ""
                        };
                        let page = match req.uri().path() {
                            "/page/script" | "/page/trusted-types" | "/page/no-head" => {
                                let policy = if req.uri().path().ends_with("/trusted-types") {
                                    "script-src 'self'; style-src 'self'; require-trusted-types-for 'script'"
                                } else {
                                    "script-src 'self'; style-src 'self'; object-src 'none'"
                                };
                                resp.headers_mut().insert(
                                    hyper::header::CONTENT_SECURITY_POLICY,
                                    hyper::header::HeaderValue::from_static(policy),
                                );
                                if req.uri().path().ends_with("/no-head") {
                                    return std::future::ready(Ok::<_, hyper::Error>(
                                        Response::builder()
                                            .header(hyper::header::CONTENT_TYPE, "text/html")
                                            .header(hyper::header::CONTENT_SECURITY_POLICY, policy)
                                            .body(Full::new(Bytes::from_static(b"<body>no head")))
                                            .unwrap(),
                                    ));
                                }
                                format!(
                                    "<!doctype html><html><head><meta http-equiv=Content-Security-Policy content=\"script-src 'self'\"><script src=page.js></script></head><body>{asked}</body></html>"
                                )
                            }
                            _ => format!(
                                "<!doctype html><html><head><title>t</title></head>\
                                 <body>{asked}{named}</body></html>"
                            ),
                        };
                        *resp.body_mut() = Full::new(Bytes::from(page));
                        let html = hyper::header::HeaderValue::from_static("text/html");
                        resp.headers_mut().insert(hyper::header::CONTENT_TYPE, html);
                    }
                    // `/last` ends the connection with its answer.
                    if req.uri().path() == "/last" {
                        let close = hyper::header::HeaderValue::from_static("close");
                        resp.headers_mut().insert(hyper::header::CONNECTION, close);
                    }
                    std::future::ready(Ok::<_, hyper::Error>(resp))
                });
                let io = TokioIo::new(tls);
                if h2 {
                    let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                        .serve_connection(io, svc)
                        .await;
                } else {
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, svc)
                        .with_upgrades()
                        .await;
                }
            });
        }
    });

    let mut roots = RootCertStore::empty();
    if setup.server_trusted {
        roots.add(root_cert.der().clone()).unwrap();
    }
    std::fs::write(dir.path().join("list.txt"), setup.rules).unwrap();
    std::fs::write(dir.path().join("allow"), setup.allow).unwrap();
    let (events, _) = mpsc::unbounded_channel();
    let trusting = setup.trusting;
    let made_to_trust = Arc::new(AtomicUsize::new(0));
    let asked = made_to_trust.clone();
    let plaintext = setup.plaintext;
    let uid = setup.uid;
    let dials = Arc::new(AtomicUsize::new(0));
    let dialed = dials.clone();
    let tls = Tls::new(Authority::open(&dir.path().join("ca")).unwrap(), roots);
    let ctx = Arc::new(Ctx {
        lists: Lists {
            filters: RwLock::new(Arc::new(Filters::load(&[dir.path()], true).unwrap())),
            exclusions: RwLock::new(Arc::new(setup.exclusions)),
            allow: RwLock::new(Arc::new(Allow::load(&dir.path().join("allow")))),
        },
        selection: Selection {
            apps: RwLock::new(Arc::new(BTreeMap::from([(UID, PACKAGE.to_owned())]))),
            profile_tls: RwLock::default(),
            url_filtering: AtomicBool::new(!setup.domain_only),
            https_inspection_available: AtomicBool::new(true),
            cosmetic: AtomicBool::new(setup.cosmetic),
        },
        services: Services {
            tls,
            policy: Arc::new(Policy::open(dir.path().join("pinned"))),
            stats: Arc::new(Stats::default()),
            live: Hub::new(),
            limits: Limits::new(1024),
        },
        hooks: Hooks {
            verbose: false,
            dial: Box::new(move |origin| {
                dialed.fetch_add(1, Relaxed);
                Box::pin(async move {
                    let dst = plaintext.map_or(origin.dst, |(port, socket)| {
                        assert_eq!(origin.dst, SocketAddr::from(([192, 0, 2, 42], port)));
                        assert_eq!(origin.mark, 42);
                        assert_eq!(origin.uid, uid);
                        socket
                    });
                    let server = TcpStream::connect(dst).await?;
                    server.set_nodelay(true)?;
                    Ok(server)
                })
            }),
            trusted: Box::new(move |_| {
                asked.fetch_add(1, Relaxed);
                Box::pin(async move { trusting })
            }),
        },
        ipv6: Ipv6Health::new(events),
    });

    // The daemon, given each connection as TPROXY would deliver it.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = listener.local_addr().unwrap();
    let origin = Origin {
        dst: plaintext.map_or(server_addr, |(port, _)| {
            SocketAddr::from(([192, 0, 2, 42], port))
        }),
        mark: if plaintext.is_some() { 42 } else { 0 },
        uid: setup.uid,
    };
    let conducted = ctx.clone();
    tokio::spawn(async move {
        loop {
            let (app, _) = listener.accept().await.unwrap();
            if plaintext.is_some() {
                let reported = Origin::of(origin.dst, origin.mark, Some(origin.uid));
                tokio::spawn(handle(conducted.clone(), app, reported));
            } else {
                tokio::spawn(conduct(conducted.clone(), app, origin));
            }
        }
    });

    Rig {
        ctx,
        proxy,
        hits,
        dials,
        server_leaf: leaf.der().clone(),
        server_root: root_cert.der().clone(),
        made_to_trust,
        dir,
    }
}

/// What a client trusts.
enum Trust {
    /// The root of ours in the system store, as an app does.
    System,
    /// The root of ours in the user store, as Chromium does.
    User,
    /// The server's own authority and not ours: an app that pins.
    ServerOnly,
}

impl Rig {
    fn roots(&self, trust: &Trust) -> RootCertStore {
        let mut roots = RootCertStore::empty();
        let ours = |file: &str| {
            let pem = std::fs::read(self.dir.path().join("ca").join(file)).unwrap();
            CertificateDer::from_pem_slice(&pem).unwrap()
        };
        roots
            .add(match trust {
                Trust::System => ours("system.pem"),
                Trust::User => ours("user.pem"),
                Trust::ServerOnly => self.server_root.clone(),
            })
            .unwrap();
        roots
    }

    /// The TLS handshake of a client, through the daemon.
    async fn handshake(
        &self,
        trust: Trust,
        alpn: &[&str],
    ) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        self.handshake_with(trust, alpn, None).await
    }

    async fn handshake_with(
        &self,
        trust: Trust,
        alpn: &[&str],
        ech: Option<EchMode>,
    ) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let builder = rustls::ClientConfig::builder_with_provider(provider());
        let mut config = match ech {
            Some(mode) => builder.with_ech(mode),
            None => builder.with_safe_default_protocol_versions(),
        }
        .unwrap()
        .with_root_certificates(self.roots(&trust))
        .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
        let tcp = TcpStream::connect(self.proxy).await?;
        let name = ServerName::try_from(HOST).unwrap();
        TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
    }

    /// One request: the status, the body, and whether the certificate
    /// the client was shown is the server's own.
    async fn get(
        &self,
        trust: Trust,
        alpn: &[&str],
        path: &str,
        referer: Option<&str>,
    ) -> Result<(StatusCode, String, bool)> {
        let (headers, status, body, real) = self.get_response(trust, alpn, path, referer).await?;
        drop(headers);
        Ok((status, body, real))
    }

    async fn get_response(
        &self,
        trust: Trust,
        alpn: &[&str],
        path: &str,
        referer: Option<&str>,
    ) -> Result<(hyper::HeaderMap, StatusCode, String, bool)> {
        let tls = self.handshake(trust, alpn).await?;
        let (_, session) = tls.get_ref();
        let real = session.peer_certificates().unwrap()[0] == self.server_leaf;
        let h2 = session.alpn_protocol() == Some(b"h2");
        let mut req = Request::get(if h2 {
            format!("https://{HOST}{path}")
        } else {
            path.to_owned()
        })
        .header(HOST_HEADER, HOST);
        if let Some(referer) = referer {
            req = req.header(REFERER, referer);
        }
        // A page is asked for as a browser asks for one.
        if path.starts_with("/page") {
            req = req
                .header("sec-fetch-dest", "document")
                .header(ACCEPT_ENCODING, "gzip, br");
        }
        let req = req.body(Empty::<Bytes>::new())?;
        let io = TokioIo::new(tls);
        let resp = if h2 {
            let (mut tx, conn) =
                hyper::client::conn::http2::handshake(TokioExecutor::new(), io).await?;
            tokio::spawn(conn);
            tx.send_request(req).await?
        } else {
            let (mut tx, conn) = hyper::client::conn::http1::handshake(io).await?;
            tokio::spawn(conn);
            tx.send_request(req).await?
        };
        let (parts, body) = resp.into_parts();
        let body = body.collect().await?.to_bytes();
        Ok((
            parts.headers,
            parts.status,
            String::from_utf8(body.to_vec())?,
            real,
        ))
    }

    fn count(&self, counter: Counter) -> u64 {
        self.ctx.services.stats.total(counter)
    }

    fn hits(&self) -> usize {
        self.hits.load(Relaxed)
    }
}

use hyper::header::HOST as HOST_HEADER;

const H1: &[&str] = &["http/1.1"];
const BOTH: &[&str] = &["h2", "http/1.1"];

#[tokio::test]
async fn status_snapshots_use_the_current_root_selection_and_the_supplied_vpn_count() {
    let rig = rig(Setup::default()).await;
    let ctx = &rig.ctx;
    let first = ctx.snapshot(true, SnapshotSelection::Root);
    assert_eq!(first.filtering_apps, Some(1));
    assert!(first.profile_tls.is_empty());

    *ctx.selection.apps.write().unwrap() = Arc::default();
    ctx.selection.profile_tls.write().unwrap().insert(
        31,
        Arc::new(ctx.services.tls.with_roots(RootCertStore::empty())),
    );
    *ctx.lists.exclusions.write().unwrap() = Arc::new(Exclusions::default());

    let root = ctx.snapshot(false, SnapshotSelection::Root);
    assert_eq!(root.filtering_apps, Some(0));
    assert_eq!(root.profile_tls, BTreeSet::from([31]));
    assert!(!root.ipv6);
    assert!(first.exclusions_ready);
    assert!(!root.exclusions_ready);
    assert_eq!((root.pid, root.started), (first.pid, first.started));

    let vpn = ctx.snapshot(true, SnapshotSelection::Vpn(2));
    assert_eq!(vpn.filtering_apps, Some(2));
    assert!(vpn.profile_tls.is_empty());
    assert!(vpn.ipv6);
}

async fn plain_rig(setup: Setup) -> (Rig, TcpListener) {
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rig = rig(Setup {
        plaintext: Some((80, server.local_addr().unwrap())),
        ..setup
    })
    .await;
    (rig, server)
}

async fn plain_request(rig: &Rig, path: &str) -> TcpStream {
    let mut app = TcpStream::connect(rig.proxy).await.unwrap();
    app.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: ads.example\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    app
}

async fn plain_response(mut app: TcpStream) -> String {
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(2), app.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    response
}

#[tokio::test]
async fn a_cleartext_url_block_returns_forbidden_without_dialing() {
    let (rig, _server) = plain_rig(Setup {
        rules: "||ads.example/blocked^\n",
        ..Setup::default()
    })
    .await;
    let response = plain_response(plain_request(&rig, "/blocked").await).await;
    assert!(response.starts_with("HTTP/1.1 403 "), "{response}");
    assert_eq!(rig.count(Counter::RequestsBlocked), 1);
    assert_eq!(rig.dials.load(Relaxed), 0);
}

#[tokio::test]
async fn a_cleartext_host_block_in_domain_mode_returns_forbidden_without_dialing() {
    let (rig, _server) = plain_rig(Setup {
        rules: "||ads.example^\n",
        domain_only: true,
        ..Setup::default()
    })
    .await;
    let response = plain_response(plain_request(&rig, "/").await).await;
    assert!(response.starts_with("HTTP/1.1 403 "), "{response}");
    assert_eq!(rig.count(Counter::HostsBlocked), 1);
    assert_eq!(rig.dials.load(Relaxed), 0);
}

#[tokio::test]
async fn the_first_allowed_cleartext_request_dials_and_later_requests_reuse_that_socket() {
    let (rig, server) = plain_rig(Setup {
        rules: "||ads.example/blocked^\n",
        ..Setup::default()
    })
    .await;
    let upstream = tokio::spawn(async move {
        let (tcp, _) = server.accept().await.unwrap();
        let seen = std::sync::Mutex::new(Vec::new());
        let service = service_fn(|req: Request<Incoming>| {
            seen.lock().unwrap().push(req.uri().path().to_owned());
            async { Ok::<_, hyper::Error>(Response::new(Empty::<Bytes>::new())) }
        });
        hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(tcp), service)
            .await
            .unwrap();
        seen.into_inner().unwrap()
    });
    let tcp = TcpStream::connect(rig.proxy).await.unwrap();
    let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
        .await
        .unwrap();
    let client = tokio::spawn(conn);
    for (path, status, dials) in [
        ("/blocked", StatusCode::FORBIDDEN, 0),
        ("/first", StatusCode::OK, 1),
        ("/second", StatusCode::OK, 1),
    ] {
        let req = Request::get(path)
            .header(HOST_HEADER, "ads.example")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let resp = tokio::time::timeout(Duration::from_secs(2), async {
            tx.ready().await?;
            tx.send_request(req).await
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(resp.status(), status, "{path}");
        resp.into_body().collect().await.unwrap();
        assert_eq!(rig.dials.load(Relaxed), dials, "{path}");
    }
    drop(tx);
    client.await.unwrap().unwrap();
    assert_eq!(upstream.await.unwrap(), ["/first", "/second"]);
}

#[tokio::test]
async fn a_cleartext_dial_failure_resets_the_app_without_an_http_response() {
    let (rig, server) = plain_rig(Setup::default()).await;
    drop(server);
    let mut app = plain_request(&rig, "/allowed").await;
    let error = tokio::time::timeout(Duration::from_secs(2), app.read(&mut [0]))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
    assert_eq!(rig.dials.load(Relaxed), 1);
}

#[tokio::test]
async fn an_allowed_first_cleartext_request_receives_the_origins_final_response() {
    let (rig, server) = plain_rig(Setup::default()).await;
    let upstream = tokio::spawn(async move {
        let (tcp, _) = server.accept().await.unwrap();
        let service = service_fn(|req: Request<Incoming>| async move {
            assert_eq!(req.uri().path(), "/allowed");
            Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from_static(b"origin"))))
        });
        hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(tcp), service)
            .await
            .unwrap();
    });
    let response = plain_response(plain_request(&rig, "/allowed").await).await;
    assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
    assert!(response.ends_with("\r\n\r\norigin"), "{response}");
    assert_eq!(rig.dials.load(Relaxed), 1);
    upstream.await.unwrap();
}

#[tokio::test]
async fn a_cleartext_websocket_upgrade_can_be_the_first_forwarded_request() {
    let (rig, server) = plain_rig(Setup::default()).await;
    let upstream = tokio::spawn(async move {
        let (tcp, _) = server.accept().await.unwrap();
        let (upgrade, upgraded) = tokio::sync::oneshot::channel();
        let upgrade = std::sync::Mutex::new(Some(upgrade));
        let service = service_fn(|mut req: Request<Incoming>| {
            assert_eq!(req.uri().path(), "/socket");
            assert_eq!(req.headers()["upgrade"], "websocket");
            let on_upgrade = hyper::upgrade::on(&mut req);
            assert!(
                upgrade
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(on_upgrade)
                    .is_ok()
            );
            async {
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(StatusCode::SWITCHING_PROTOCOLS)
                        .header("connection", "upgrade")
                        .header("upgrade", "websocket")
                        .body(Empty::<Bytes>::new())
                        .unwrap(),
                )
            }
        });
        hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(tcp), service)
            .with_upgrades()
            .await
            .unwrap();
        let mut io = TokioIo::new(upgraded.await.unwrap().await.unwrap());
        let mut buf = [0; 4];
        io.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        io.write_all(b"pong").await.unwrap();
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        let tcp = TcpStream::connect(rig.proxy).await.unwrap();
        let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
            .await
            .unwrap();
        let client = tokio::spawn(conn.with_upgrades());
        let req = Request::get("/socket")
            .header(HOST_HEADER, "ads.example")
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let mut resp = tx.send_request(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
        let mut io = TokioIo::new(hyper::upgrade::on(&mut resp).await.unwrap());
        io.write_all(b"ping").await.unwrap();
        let mut buf = [0; 4];
        io.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
        drop(io);
        client.await.unwrap().unwrap();
        upstream.await.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(rig.dials.load(Relaxed), 1);
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "exercises request policy across hosts on one connection"
)]
async fn clear_http_uses_the_original_destination_and_filters_each_host_in_full_mode() {
    let (rig, server) = plain_rig(Setup {
        rules: "|http://ads.example^\n@@||ads.example/pass^\n|https://plain.example^\n||trusted.example^\n",
        allow: "trusted.example\n",
        trusting: Trusting::No,
        exclusions: Exclusions::default(),
        ..Setup::default()
    }).await;
    rig.ctx
        .selection
        .https_inspection_available
        .store(false, Relaxed);
    let upstream = tokio::spawn(async move {
        let (tcp, _) = server.accept().await.unwrap();
        let service = service_fn(|req: Request<Incoming>| async move {
            let (parts, body) = req.into_parts();
            let body = body.collect().await?.to_bytes();
            let status = if parts.uri.path() == "/redirect" {
                StatusCode::TEMPORARY_REDIRECT
            } else {
                StatusCode::OK
            };
            let host = parts.headers.get(HOST_HEADER).unwrap().to_str().unwrap();
            let content = format!(
                "{} {} {host} {}",
                parts.method,
                parts.uri,
                String::from_utf8_lossy(&body)
            );
            Ok::<_, hyper::Error>(
                Response::builder()
                    .status(status)
                    .header("location", "http://elsewhere.example/")
                    .body(Full::new(Bytes::from(content)))
                    .unwrap(),
            )
        });
        hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(tcp), service)
            .await
            .unwrap();
    });
    let tcp = TcpStream::connect(rig.proxy).await.unwrap();
    let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
        .await
        .unwrap();
    let client = tokio::spawn(conn);
    for (host, path, status) in [
        ("ads.example", "/blocked", StatusCode::FORBIDDEN),
        ("ads.example", "/pass", StatusCode::OK),
        ("plain.example", "/blocked", StatusCode::OK),
        ("TRUSTED.example:80", "/blocked", StatusCode::OK),
        ("ads.example", "/blocked-again", StatusCode::FORBIDDEN),
        ("plain.example", "/redirect", StatusCode::TEMPORARY_REDIRECT),
    ] {
        tx.ready().await.unwrap();
        let req = Request::post(path)
            .header(HOST_HEADER, host)
            .body(Full::new(Bytes::from_static(b"data")))
            .unwrap();
        let resp = tx.send_request(req).await.unwrap();
        assert_eq!(resp.status(), status, "{host} {path}");
        if status.is_redirection() {
            assert_eq!(
                resp.headers().get("location").unwrap(),
                "http://elsewhere.example/"
            );
        }
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        if status == StatusCode::FORBIDDEN {
            assert!(body.is_empty());
        } else {
            assert_eq!(body, format!("POST {path} {host} data"));
        }
    }
    assert_eq!(rig.count(Counter::RequestsBlocked), 2);
    assert_eq!(rig.count(Counter::RequestsAllowed), 3);
    assert_eq!(rig.count(Counter::Decrypted), 0);
    assert_eq!(rig.made_to_trust.load(Relaxed), 0);
    drop(tx);
    client.await.unwrap().unwrap();
    upstream.await.unwrap();
}

#[tokio::test]
async fn clear_http_domain_mode_checks_only_the_host_and_whole_domain_exceptions() {
    let (rig, server) = plain_rig(Setup {
        rules: "||ads.example^\n@@||ads.example/pass^\n@@||whitelist.ads.example^\n/blocked-path^\n",
        allow: "trusted.ads.example\n",
        domain_only: true,
        ..Setup::default()
    }).await;
    let upstream = tokio::spawn(async move {
        let (tcp, _) = server.accept().await.unwrap();
        let service = service_fn(|_req: Request<Incoming>| async move {
            Ok::<_, hyper::Error>(Response::new(Empty::<Bytes>::new()))
        });
        hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(tcp), service)
            .await
            .unwrap();
    });
    let tcp = TcpStream::connect(rig.proxy).await.unwrap();
    let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
        .await
        .unwrap();
    let client = tokio::spawn(conn);
    for (host, path, status) in [
        ("ads.example", "/pass", StatusCode::FORBIDDEN),
        ("whitelist.ads.example", "/pass", StatusCode::OK),
        ("trusted.ads.example", "/pass", StatusCode::OK),
        ("fine.example", "/blocked-path", StatusCode::OK),
    ] {
        tx.ready().await.unwrap();
        let req = Request::get(path)
            .header(HOST_HEADER, host)
            .body(Empty::<Bytes>::new())
            .unwrap();
        let resp = tx.send_request(req).await.unwrap();
        assert_eq!(resp.status(), status, "{host} {path}");
        resp.into_body().collect().await.unwrap();
    }
    assert_eq!(rig.count(Counter::HostsBlocked), 1);
    assert_eq!(rig.count(Counter::RequestsBlocked), 0);
    assert_eq!(rig.count(Counter::RequestsAllowed), 0);
    assert_eq!(rig.made_to_trust.load(Relaxed), 0);
    drop(tx);
    client.await.unwrap().unwrap();
    upstream.await.unwrap();
}

#[tokio::test]
async fn non_origin_http_targets_and_unsupported_protocols_pass_to_the_same_original_socket() {
    let (rig, server) = plain_rig(Setup::default()).await;
    let oversized = format!(
        "GET / HTTP/1.1\r\nHost: opaque.example\r\nX-Long: {}\r\n\r\n",
        "x".repeat(HTTP_LOOK)
    );
    let cases = [
        "CONNECT attacker.test:443 HTTP/1.1\r\nHost: attacker.test:443\r\n\r\n",
        "GET http://attacker.test/x HTTP/1.1\r\nHost: attacker.test\r\n\r\n",
        "OPTIONS * HTTP/1.1\r\nHost: opaque.example\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: one.example\r\nHost: two.example\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: opaque.example:81\r\n\r\n",
        "POST / HTTP/1.1\r\nHost: opaque.example\r\nContent-Length: unknown\r\n\r\ndata",
        "POST / HTTP/1.0\r\nHost: opaque.example\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n",
        "\u{1}raw data\n",
        oversized.as_str(),
    ];
    for bytes in cases {
        let mut tcp = TcpStream::connect(rig.proxy).await.unwrap();
        tcp.write_all(bytes.as_bytes()).await.unwrap();
        let (mut up, _) = tokio::time::timeout(Duration::from_secs(1), server.accept())
            .await
            .unwrap()
            .unwrap();
        let mut received = vec![0; bytes.len()];
        up.read_exact(&mut received).await.unwrap();
        assert_eq!(received, bytes.as_bytes());
        up.write_all(b"original reply").await.unwrap();
        let mut reply = [0; 14];
        tcp.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"original reply");
    }
    assert_eq!(rig.count(Counter::Passed), cases.len() as u64);
    assert_eq!(rig.made_to_trust.load(Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn incomplete_http_headers_use_the_bounded_raw_fallback() {
    let (rig, server) = plain_rig(Setup::default()).await;
    let mut app = TcpStream::connect(rig.proxy).await.unwrap();
    let prefix = b"GET / HTTP/1.1\r\nHost: opaque.example";
    app.write_all(prefix).await.unwrap();
    let since = tokio::time::Instant::now();
    let (mut up, _) = tokio::time::timeout(HTTP_LOOK_TIME * 2, server.accept())
        .await
        .unwrap()
        .unwrap();
    assert!(since.elapsed() >= HTTP_LOOK_TIME);
    let mut delivered = vec![0; prefix.len()];
    up.read_exact(&mut delivered).await.unwrap();
    assert_eq!(delivered, prefix);
    assert_eq!(rig.count(Counter::Passed), 1);
}

#[tokio::test]
async fn clear_http_on_another_port_remains_opaque() {
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rig = rig(Setup {
        rules: "||ads.example^\n",
        plaintext: Some((8080, server.local_addr().unwrap())),
        ..Setup::default()
    })
    .await;
    let mut app = TcpStream::connect(rig.proxy).await.unwrap();
    let sent = b"GET /blocked HTTP/1.1\r\nHost: ads.example\r\n\r\n";
    app.write_all(sent).await.unwrap();
    let (mut up, _) = server.accept().await.unwrap();
    let mut delivered = vec![0; sent.len()];
    up.read_exact(&mut delivered).await.unwrap();
    assert_eq!(delivered, sent);
    assert_eq!(rig.count(Counter::Passed), 1);
    assert_eq!(rig.count(Counter::RequestsBlocked), 0);
}

#[tokio::test]
async fn connect_after_ordinary_http_tunnels_only_to_the_original_server() {
    let (rig, server) = plain_rig(Setup::default()).await;
    let upstream = tokio::spawn(async move {
        let (tcp, _) = server.accept().await.unwrap();
        let service = service_fn(move |mut req: Request<Incoming>| async move {
            if req.method() == Method::CONNECT {
                assert_eq!(req.uri().authority().unwrap(), "requested.example:443");
                let tunnel = hyper::upgrade::on(&mut req);
                tokio::spawn(async move {
                    let mut io = TokioIo::new(tunnel.await.unwrap());
                    let mut buf = [0; 4];
                    io.read_exact(&mut buf).await.unwrap();
                    assert_eq!(&buf, b"ping");
                    io.write_all(b"pong").await.unwrap();
                });
            }
            Ok::<_, hyper::Error>(Response::new(Empty::<Bytes>::new()))
        });
        hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(tcp), service)
            .with_upgrades()
            .await
            .unwrap();
    });
    let tcp = TcpStream::connect(rig.proxy).await.unwrap();
    let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
        .await
        .unwrap();
    let client = tokio::spawn(conn.with_upgrades());
    let first = Request::get("/")
        .header(HOST_HEADER, "start.example")
        .body(Empty::<Bytes>::new())
        .unwrap();
    tx.send_request(first)
        .await
        .unwrap()
        .into_body()
        .collect()
        .await
        .unwrap();
    tx.ready().await.unwrap();
    let req = Request::builder()
        .method(Method::CONNECT)
        .uri("requested.example:443")
        .header(HOST_HEADER, "requested.example:443")
        .body(Empty::<Bytes>::new())
        .unwrap();
    let mut resp = tx.send_request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let tunnel = hyper::upgrade::on(&mut resp).await.unwrap();
    let mut io = TokioIo::new(tunnel);
    io.write_all(b"ping").await.unwrap();
    let mut echoed = [0; 4];
    io.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"pong");
    drop(io);
    client.await.unwrap().unwrap();
    upstream.await.unwrap();
}

#[tokio::test]
async fn a_request_is_decrypted_and_passed_on_over_http1() {
    let rig = rig(Setup::default()).await;
    let (status, body, real) = rig.get(Trust::System, H1, "/hello", None).await.unwrap();
    assert_eq!((status, body.as_str()), (StatusCode::OK, "GET /hello"));
    // The client was shown a certificate of ours, and accepted it
    // trusting nothing but the root of the system store.
    assert!(!real);
    assert_eq!(rig.hits(), 1);
    assert_eq!(rig.count(Counter::Decrypted), 1);
    assert_eq!(rig.count(Counter::RequestsAllowed), 1);
}

#[tokio::test]
async fn a_request_is_decrypted_and_passed_on_over_http2() {
    let rig = rig(Setup {
        alpn: BOTH,
        ..Setup::default()
    })
    .await;
    let (status, body, real) = rig.get(Trust::User, BOTH, "/hello", None).await.unwrap();
    assert_eq!((status, body.as_str()), (StatusCode::OK, "GET /hello"));
    assert!(!real);
}

#[tokio::test]
async fn the_app_is_offered_the_protocol_the_server_chose() {
    // The app would speak HTTP/2; the server does not.
    let rig = rig(Setup::default()).await;
    let tls = rig.handshake(Trust::System, BOTH).await.unwrap();
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"http/1.1"[..]));
}

#[tokio::test]
async fn a_request_blocked_by_its_path_never_reaches_the_server() {
    let rig = rig(Setup {
        rules: "/ads/banner\n",
        ..Setup::default()
    })
    .await;
    let (status, body, _) = rig
        .get(Trust::System, H1, "/ads/banner.js", None)
        .await
        .unwrap();
    assert_eq!((status, body.as_str()), (StatusCode::FORBIDDEN, ""));
    assert_eq!(rig.hits(), 0);
    assert_eq!(rig.count(Counter::RequestsBlocked), 1);
    // The rest of the site is untouched.
    let (status, _, _) = rig.get(Trust::System, H1, "/story", None).await.unwrap();
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn an_answer_the_server_ends_the_connection_with_reaches_the_app_whole() {
    let rig = rig(Setup::default()).await;
    // Repeat to cover both orders in which the proxy observes shutdown.
    for _ in 0..40 {
        let (status, body, _) = rig.get(Trust::System, H1, "/last", None).await.unwrap();
        assert_eq!((status, body.as_str()), (StatusCode::OK, "GET /last"));
    }
}

#[tokio::test]
async fn a_request_without_a_source_is_neither_first_nor_third_party() {
    let rig = rig(Setup {
        rules: "||site.test^$third-party\n",
        ..Setup::default()
    })
    .await;
    let (status, _, _) = rig.get(Trust::System, H1, "/sdk", None).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    // The site's own page asking for the same.
    let page = Some("https://site.test/index.html");
    let (status, _, _) = rig.get(Trust::System, H1, "/sdk", page).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    // Another site's page.
    let page = Some("https://news.example/");
    let (status, _, _) = rig.get(Trust::System, H1, "/sdk", page).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn full_mode_defers_host_checks_to_honor_request_exceptions() {
    let rig = rig(Setup {
        rules: "||site.test^\n@@||site.test/pass^$image\n",
        ..Setup::default()
    })
    .await;
    for (path, expected) in [
        ("/pass/asset.png", StatusCode::OK),
        ("/pass/asset.js", StatusCode::FORBIDDEN),
        ("/ads/asset.png", StatusCode::FORBIDDEN),
    ] {
        let (status, _, real) = rig.get(Trust::System, BOTH, path, None).await.unwrap();
        assert_eq!(status, expected, "{path}");
        assert!(!real);
    }
    assert_eq!(rig.hits(), 1);
    assert_eq!(rig.count(Counter::HostsBlocked), 0);
    assert_eq!(rig.count(Counter::RequestsBlocked), 2);
}

#[tokio::test]
async fn a_pinned_host_is_judged_by_its_domain_rules_in_full_mode() {
    let rig = rig(Setup {
        rules: "||site.test^\n@@||site.test/pass^$image\n",
        ..Setup::default()
    })
    .await;
    // Its requests will be seen: the web exception may apply to one.
    assert!(!rig.ctx.blocks_udp_host(UID, HOST));
    rig.ctx.services.policy.rejected(PACKAGE, HOST);
    // Pinned, none is seen: the domain rules decide, for the connection
    // and for the lookup alike.
    assert!(rig.handshake(Trust::ServerOnly, H1).await.is_err());
    assert_eq!(rig.hits(), 0);
    assert!(rig.ctx.blocks_udp_host(UID, HOST));
    assert_eq!(rig.count(Counter::Passed), 0);
    assert_eq!(rig.count(Counter::HostsBlocked), 2);
}

#[tokio::test]
async fn a_pinned_host_its_app_now_accepts_is_decrypted_from_its_next_process_on() {
    let rig = rig(Setup::default()).await;
    rig.ctx.services.policy.rejected(PACKAGE, HOST);
    let (_, _, real) = rig.get(Trust::ServerOnly, H1, "/", None).await.unwrap();
    assert!(real, "pinned: passed through");

    rig.ctx.services.policy.process_started(PACKAGE);
    let (status, _, real) = rig.get(Trust::System, H1, "/", None).await.unwrap();
    assert!(!real, "the try is decrypted");
    assert_eq!(status, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(100)).await;
    // No longer pinned, with no further start.
    let exclusions = Exclusions::default().assume_ready();
    let verdict = rig.ctx.services.policy.decide_named(
        PACKAGE,
        HOST,
        &rig.ctx.filters(),
        &exclusions,
        &Allow::default(),
        false,
    );
    assert_eq!(verdict, Verdict::Mitm);
    let (_, _, real) = rig.get(Trust::System, H1, "/", None).await.unwrap();
    assert!(!real);
}

#[tokio::test]
async fn an_app_that_still_pins_loses_one_connection_at_a_new_process() {
    let rig = rig(Setup::default()).await;
    rig.ctx.services.policy.rejected(PACKAGE, HOST);
    rig.ctx.services.policy.process_started(PACKAGE);
    assert!(rig.handshake(Trust::ServerOnly, H1).await.is_err());
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Every connection after it is passed through, also after another
    // start soon after.
    for start in [false, true] {
        if start {
            rig.ctx.services.policy.process_started(PACKAGE);
        }
        let (_, _, real) = rig.get(Trust::ServerOnly, H1, "/", None).await.unwrap();
        assert!(real);
    }
    assert_eq!(rig.count(Counter::Passed), 2);
    assert_eq!(rig.count(Counter::Pinned), 0, "pinned before, not again");
    assert_eq!(rig.ctx.services.policy.take_try(PACKAGE, HOST), None);
}

#[tokio::test]
async fn a_try_whose_server_cannot_be_verified_is_left_for_the_next_connection() {
    let rig = rig(Setup {
        server_trusted: false,
        ..Setup::default()
    })
    .await;
    rig.ctx.services.policy.rejected(PACKAGE, HOST);
    rig.ctx.services.policy.process_started(PACKAGE);
    // The app is given the server itself: it never saw our certificate.
    let (_, _, real) = rig.get(Trust::ServerOnly, H1, "/", None).await.unwrap();
    assert!(real);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rig.ctx.services.policy.take_try(PACKAGE, HOST).is_some());
}

#[tokio::test]
async fn a_pinned_host_its_domain_rules_block_is_refused_with_a_try_pending() {
    for trusting in [Trusting::Yes, Trusting::No] {
        let rig = rig(Setup {
            rules: "||site.test^\n@@||site.test/pass^$image\n",
            trusting,
            ..Setup::default()
        })
        .await;
        rig.ctx.services.policy.rejected(PACKAGE, HOST);
        rig.ctx.services.policy.process_started(PACKAGE);
        assert!(rig.handshake(Trust::System, H1).await.is_err());
        assert_eq!(rig.hits(), 0);
        assert!(rig.ctx.blocks_udp_host(UID, HOST));
        assert_eq!(rig.count(Counter::HostsBlocked), 2);
    }
}

#[tokio::test]
async fn work_tls_uses_only_its_profile_trust_and_respects_opaque_exceptions() {
    let work_uid = 1_010_001;
    let rig = rig(Setup {
        uid: work_uid,
        rules: "||site.test^\n@@||site.test/pass^$image\n",
        ..Setup::default()
    })
    .await;
    *rig.ctx.selection.apps.write().unwrap() =
        Arc::new(BTreeMap::from([(work_uid, "31:com.app".to_owned())]));
    let real = || rig.get(Trust::ServerOnly, H1, "/ads/asset.png", None);
    // The absent local CA and a profile that distrusts the upstream
    // root both leave the ClientHello untouched for the work app.
    assert!(real().await.unwrap().2);
    let without_upstream = Arc::new(rig.ctx.services.tls.with_roots(RootCertStore::empty()));
    rig.ctx
        .selection
        .profile_tls
        .write()
        .unwrap()
        .insert(31, without_upstream);
    assert!(real().await.unwrap().2);
    assert_eq!(rig.count(Counter::HostsBlocked), 0);
    assert_eq!(rig.count(Counter::Decrypted), 0);

    let authorized = Arc::new(
        rig.ctx
            .services
            .tls
            .with_roots(rig.roots(&Trust::ServerOnly)),
    );
    rig.ctx
        .selection
        .profile_tls
        .write()
        .unwrap()
        .insert(31, authorized);
    let (status, _, upstream) = rig
        .get(Trust::User, H1, "/ads/asset.png", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!upstream);
}

#[tokio::test]
async fn replacing_a_work_profile_verifier_retires_its_existing_http2_connection() {
    let uid = 1_010_001;
    let rig = rig(Setup {
        uid,
        alpn: &["h2"],
        ..Setup::default()
    })
    .await;
    *rig.ctx.selection.apps.write().unwrap() =
        Arc::new(BTreeMap::from([(uid, "31:com.app".to_owned())]));
    let publish_verifier = || {
        let verifier = rig
            .ctx
            .services
            .tls
            .with_roots(rig.roots(&Trust::ServerOnly));
        rig.ctx
            .selection
            .profile_tls
            .write()
            .unwrap()
            .insert(31, Arc::new(verifier));
    };
    publish_verifier();
    let tls = rig.handshake(Trust::User, &["h2"]).await.unwrap();
    let (mut tx, conn) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
            .await
            .unwrap();
    let driver = tokio::spawn(conn);
    let request = || {
        Request::get(format!("https://{HOST}/"))
            .body(Empty::<Bytes>::new())
            .unwrap()
    };
    let response = tx.send_request(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body().collect().await.unwrap();

    publish_verifier();
    assert_eq!(
        tx.send_request(request()).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(rig.hits(), 1);
}

#[tokio::test]
async fn disabling_inspection_retires_the_existing_http2_connection() {
    let rig = rig(Setup {
        alpn: &["h2"],
        ..Setup::default()
    })
    .await;
    let tls = rig.handshake(Trust::System, &["h2"]).await.unwrap();
    let (mut tx, conn) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
            .await
            .unwrap();
    let driver = tokio::spawn(conn);
    let request = || {
        Request::get(format!("https://{HOST}/"))
            .body(Empty::<Bytes>::new())
            .unwrap()
    };
    assert_eq!(
        tx.send_request(request()).await.unwrap().status(),
        StatusCode::OK
    );
    let other = Request::get("https://other.test/")
        .body(Empty::<Bytes>::new())
        .unwrap();
    assert_eq!(
        tx.send_request(other).await.unwrap().status(),
        StatusCode::MISDIRECTED_REQUEST
    );
    rig.ctx
        .selection
        .https_inspection_available
        .store(false, Relaxed);
    assert_eq!(
        tx.send_request(request()).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(tx.send_request(request()).await.is_err());
    assert_eq!(rig.hits(), 1);
}

#[tokio::test]
async fn a_host_blocked_whole_is_refused_before_any_handshake() {
    let rig = rig(Setup {
        rules: "||site.test^\n",
        ..Setup::default()
    })
    .await;
    assert!(rig.handshake(Trust::System, H1).await.is_err());
    assert_eq!(rig.hits(), 0);
    assert_eq!(rig.count(Counter::HostsBlocked), 1);
    assert_eq!(rig.count(Counter::Decrypted), 0);
}

#[tokio::test]
async fn a_whole_host_rule_blocks_an_ech_hello_by_its_visible_name() {
    let rig = rig(Setup {
        rules: "||site.test^\n",
        domain_only: true,
        ..Setup::default()
    })
    .await;
    assert!(
        rig.handshake_with(Trust::ServerOnly, H1, Some(ech_fixture::grease()))
            .await
            .is_err()
    );
    assert_eq!(rig.count(Counter::HostsBlocked), 1);
    assert_eq!(rig.dials.load(Relaxed), 0);
}

#[tokio::test]
async fn grease_ech_requests_are_filtered_for_the_verified_name() {
    let rig = rig(Setup {
        rules: "/ads/banner\n",
        ..Setup::default()
    })
    .await;
    let tls = rig
        .handshake_with(Trust::System, H1, Some(ech_fixture::grease()))
        .await
        .unwrap();
    let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(conn);
    for (host, path, expected) in [
        (HOST, "/ads/banner.js", StatusCode::FORBIDDEN),
        ("other.test", "/story", StatusCode::MISDIRECTED_REQUEST),
        (HOST, "/story", StatusCode::OK),
    ] {
        let req = Request::get(path)
            .header(HOST_HEADER, host)
            .body(Empty::<Bytes>::new())
            .unwrap();
        let response = tx.send_request(req).await.unwrap();
        assert_eq!(response.status(), expected);
        response.into_body().collect().await.unwrap();
    }
    assert_eq!(rig.hits(), 1);
    assert_eq!(rig.count(Counter::Decrypted), 1);
}

#[tokio::test]
async fn rejecting_ech_does_not_create_a_pin_for_the_public_name() {
    let rig = rig(Setup::default()).await;
    for _ in 0..4 {
        let error = rig
            .handshake_with(Trust::System, H1, Some(ech_fixture::offer(HOST)))
            .await
            .unwrap_err();
        assert!(matches!(
            error
                .get_ref()
                .and_then(|e| e.downcast_ref::<rustls::Error>()),
            Some(rustls::Error::PeerIncompatible(
                rustls::PeerIncompatible::ServerRejectedEncryptedClientHello(None)
            ))
        ));
    }
    let (status, _, real) = rig.get(Trust::System, H1, "/", None).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(!real);
    assert_eq!(rig.count(Counter::Pinned), 0);
}

/// Chrome and Cronet offer ECH on every connection, so an app that pins
/// and closes without an alert must be recognised whether it offers ECH
/// or not; otherwise it would be decrypted, and fail, every time.
#[tokio::test]
async fn an_ech_client_that_keeps_closing_without_a_request_creates_a_pin() {
    let rig = rig(Setup::default()).await;
    for _ in 0..3 {
        let mut tls = rig
            .handshake_with(Trust::System, H1, Some(ech_fixture::grease()))
            .await
            .unwrap();
        tls.shutdown().await.unwrap();
        assert_eq!(tls.read_to_end(&mut Vec::new()).await.unwrap(), 0);
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while rig.count(Counter::Pinned) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(rig.count(Counter::Decrypted), 3);
    assert_eq!(rig.count(Counter::Pinned), 1);
}

#[tokio::test]
async fn an_ech_client_that_refuses_our_certificate_still_creates_a_pin() {
    let rig = rig(Setup::default()).await;
    assert!(
        rig.handshake_with(Trust::ServerOnly, H1, Some(ech_fixture::grease()))
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        while rig.count(Counter::Pinned) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(rig.count(Counter::Pinned), 1);
}

#[tokio::test]
async fn an_app_that_refuses_our_certificate_gets_the_server_itself_next_time() {
    let rig = rig(Setup::default()).await;
    assert!(rig.handshake(Trust::ServerOnly, H1).await.is_err());
    // The daemon learns of the refusal as its side of the handshake ends.
    for _ in 0..50 {
        if rig.count(Counter::Pinned) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(rig.count(Counter::Pinned), 1);
    let (status, body, real) = rig.get(Trust::ServerOnly, H1, "/api", None).await.unwrap();
    assert_eq!((status, body.as_str()), (StatusCode::OK, "GET /api"));
    assert!(real);
    // Handed on untouched: nothing was read, so nothing was counted.
    assert_eq!(rig.count(Counter::RequestsAllowed), 0);
}

#[tokio::test]
async fn an_app_that_closes_right_after_the_handshake_is_taken_to_pin_after_a_few() {
    for alpn in [H1, BOTH] {
        let rig = rig(Setup {
            alpn,
            ..Setup::default()
        })
        .await;
        for _ in 0..3 {
            assert_eq!(rig.count(Counter::Pinned), 0);
            let mut tls = rig.handshake(Trust::System, alpn).await.unwrap();
            assert_eq!(tls.get_ref().1.alpn_protocol(), Some(alpn[0].as_bytes()));
            tls.shutdown().await.unwrap();
            drop(tls);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(rig.count(Counter::Decrypted), 3);
        assert_eq!(rig.count(Counter::Pinned), 1);
        let (_, _, real) = rig.get(Trust::ServerOnly, alpn, "/", None).await.unwrap();
        assert!(real);
    }
}

#[tokio::test]
async fn nothing_is_signed_for_a_server_that_cannot_be_verified() {
    let rig = rig(Setup {
        server_trusted: false,
        ..Setup::default()
    })
    .await;
    // An app that trusts us is shown the server's certificate, not one
    // of ours vouching for it, and refuses it as it would have.
    assert!(rig.handshake(Trust::System, H1).await.is_err());
    // One that trusts the server reaches it, untouched.
    let (status, _, real) = rig.get(Trust::ServerOnly, H1, "/", None).await.unwrap();
    assert!(real && status == StatusCode::OK);
    assert_eq!(rig.count(Counter::Decrypted), 0);
}

#[tokio::test]
async fn a_connection_of_an_app_that_is_not_filtered_is_handed_on_unread() {
    let rig = rig(Setup {
        rules: "||site.test^\n",
        uid: UID + 1,
        ..Setup::default()
    })
    .await;
    let (status, _, real) = rig.get(Trust::ServerOnly, H1, "/", None).await.unwrap();
    assert!(real && status == StatusCode::OK);
    assert_eq!(rig.count(Counter::HostsBlocked), 0);
}

#[tokio::test]
async fn a_server_with_an_extended_validation_certificate_is_not_decrypted() {
    let rig = rig(Setup {
        ev: true,
        ..Setup::default()
    })
    .await;
    // An app that trusts us is given the server itself all the same.
    assert!(rig.handshake(Trust::System, H1).await.is_err());
    let (status, _, real) = rig.get(Trust::ServerOnly, H1, "/", None).await.unwrap();
    assert!(real && status == StatusCode::OK);
    assert_eq!(rig.count(Counter::Decrypted), 0);
    // Not for having refused us: nothing was found pinned.
    assert_eq!(rig.count(Counter::Pinned), 0);
}

#[tokio::test]
async fn an_app_that_cannot_be_made_to_trust_us_is_not_shown_our_certificate() {
    let rig = rig(Setup {
        trusting: Trusting::No,
        ..Setup::default()
    })
    .await;
    let (status, _, real) = rig.get(Trust::ServerOnly, H1, "/", None).await.unwrap();
    assert!(real && status == StatusCode::OK);
    // Nothing it could have refused, so nothing taken for pinned.
    assert_eq!(rig.count(Counter::Pinned), 0);
    assert_eq!(rig.count(Counter::Decrypted), 0);
}

#[tokio::test]
async fn an_app_only_just_made_to_trust_us_is_not_held_to_what_it_refuses() {
    let rig = rig(Setup {
        trusting: Trusting::Newly,
        ..Setup::default()
    })
    .await;
    // It refuses us: for roots it read before, as far as can be told.
    assert!(rig.handshake(Trust::ServerOnly, H1).await.is_err());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(rig.count(Counter::Pinned), 0);
    // And is decrypted once it has read them again.
    let (_, _, real) = rig.get(Trust::System, H1, "/", None).await.unwrap();
    assert!(!real);
}

#[tokio::test]
async fn connections_stay_encrypted_until_exclusions_are_available() {
    let rig = rig(Setup {
        exclusions: Exclusions::default(),
        rules: "/ads/\n",
        ..Setup::default()
    })
    .await;
    let (status, _, real) = rig
        .get(Trust::ServerOnly, H1, "/ads/x", None)
        .await
        .unwrap();
    assert!(real);
    assert_eq!(status, 200);
}

#[tokio::test]
async fn an_exclusion_reload_during_upstream_verification_keeps_the_connection_encrypted() {
    let lists = tempfile::tempdir().unwrap();
    std::fs::write(lists.path().join("local.txt"), format!("{HOST}\n")).unwrap();
    for exclusions in [
        Exclusions::load(lists.path()).assume_ready(),
        Exclusions::default(),
    ] {
        let (arrived, upstream_started) = tokio::sync::oneshot::channel();
        let (resume, paused) = tokio::sync::oneshot::channel();
        let rig = rig(Setup {
            rules: "/ads/\n",
            before_first_server_handshake: Some((arrived, paused)),
            ..Setup::default()
        })
        .await;
        let request = rig.get(Trust::ServerOnly, H1, "/ads/x", None);
        let reload = async {
            upstream_started.await.unwrap();
            *rig.ctx.lists.exclusions.write().unwrap() = Arc::new(exclusions);
            resume.send(()).unwrap();
        };
        let (result, ()) =
            tokio::time::timeout(HANDSHAKE_TIME, async { tokio::join!(request, reload) })
                .await
                .expect("the test must reach the upstream handshake and finish");

        let (status, _, real) = result.unwrap();
        assert!(real);
        assert_eq!(status, 200);
    }
}

#[tokio::test]
async fn a_host_a_list_excludes_is_handed_on_without_asking_the_server() {
    let lists = tempfile::tempdir().unwrap();
    std::fs::write(lists.path().join("banks.txt"), "site.test\n").unwrap();
    let rig = rig(Setup {
        exclusions: Exclusions::load(lists.path()).assume_ready(),
        rules: "/ads/\n",
        ..Setup::default()
    })
    .await;
    let (status, body, real) = rig
        .get(Trust::ServerOnly, H1, "/ads/x", None)
        .await
        .unwrap();
    // Untouched: the server's own certificate, and nothing filtered.
    assert!(real);
    assert_eq!((status, body.as_str()), (StatusCode::OK, "GET /ads/x"));
    assert_eq!(rig.count(Counter::Decrypted), 0);
}

const HIDING: &str = "site.test##.ad\nother.test##.banner\n";

#[tokio::test]
async fn what_the_lists_name_is_hidden_on_a_page_where_the_user_has_it_so() {
    for alpn in [H1, BOTH] {
        let rig = rig(Setup {
            rules: HIDING,
            alpn,
            cosmetic: true,
            ..Setup::default()
        })
        .await;
        let (status, body, _) = rig.get(Trust::System, alpn, "/page", None).await.unwrap();
        // The style of this site only, where the head begins; and the
        // page asked for unpacked, to have it put in.
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            "<!doctype html><html><head><style>.ad{display:none!important}\n</style>\
             <title>t</title></head><body>identity</body></html>"
        );
        // Not a page: as it came, and asked for as the app asked.
        let (_, body, _) = rig.get(Trust::System, alpn, "/x", None).await.unwrap();
        assert_eq!(body, "GET /x");
    }
}

#[tokio::test]
async fn the_script_runs_first_and_its_hash_only_changes_the_header_policy() {
    use base64::Engine as _;
    use hyper::header::CONTENT_SECURITY_POLICY;
    let rig = rig(Setup {
        rules: "site.test##+js(set-constant, fixture, 42)\nsite.test##div:has-text(Advert)\n",
        alpn: BOTH,
        cosmetic: true,
        ..Setup::default()
    })
    .await;
    for alpn in [H1, BOTH] {
        let (headers, status, body, real) = rig
            .get_response(Trust::System, alpn, "/page/script", None)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(!real);
        let (script, page) = body
            .strip_prefix("<!doctype html><html><head><script>")
            .unwrap()
            .split_once("</script>")
            .unwrap();
        assert!(script.contains(r#"setConstant("fixture", "42")"#));
        assert!(script.contains(r#"{"type":"has-text","arg":"Advert"}"#));
        let hash = base64::engine::general_purpose::STANDARD.encode(ring::digest::digest(
            &ring::digest::SHA256,
            script.as_bytes(),
        ));
        assert_eq!(
            headers[CONTENT_SECURITY_POLICY],
            format!(
                "script-src 'self'; style-src 'self'; object-src 'none'; script-src-elem 'self' 'sha256-{hash}'"
            )
        );
        assert_eq!(
            page,
            "<meta http-equiv=Content-Security-Policy content=\"script-src 'self'\"><script src=page.js></script></head><body>identity</body></html>"
        );
    }
}

#[tokio::test]
async fn a_trusted_types_policy_keeps_css_and_a_missing_head_keeps_the_original_response() {
    use hyper::header::CONTENT_SECURITY_POLICY;
    let rig = rig(Setup {
        rules: "site.test##+js(set-constant, fixture, 42)\nsite.test##.ad\n",
        cosmetic: true,
        ..Setup::default()
    })
    .await;
    let (headers, _, body, _) = rig
        .get_response(Trust::System, H1, "/page/trusted-types", None)
        .await
        .unwrap();
    assert!(body.starts_with("<!doctype html><html><head><style>.ad"));
    assert!(!body.contains("setConstant"));
    let policy = headers[CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(policy.starts_with("script-src 'self'; style-src 'self' 'sha256-"));
    assert!(policy.ends_with("'; require-trusted-types-for 'script'"));
    let (headers, _, body, _) = rig
        .get_response(Trust::System, H1, "/page/no-head", None)
        .await
        .unwrap();
    assert_eq!(
        headers[CONTENT_SECURITY_POLICY],
        "script-src 'self'; style-src 'self'; object-src 'none'"
    );
    assert_eq!(body, "<body>no head");
}

#[tokio::test]
async fn a_page_is_passed_on_as_it_came_unless_the_user_has_it_changed() {
    let rig = rig(Setup {
        rules: "site.test##.ad\nsite.test##+js(set-constant, fixture, 42)\nsite.test##div:has-text(Advert)\n",
        ..Setup::default()
    })
    .await;
    let (_, body, _) = rig.get(Trust::System, H1, "/page", None).await.unwrap();
    assert_eq!(
        body,
        "<!doctype html><html><head><title>t</title></head><body>gzip, br</body></html>"
    );
}

#[tokio::test]
async fn a_page_the_lists_name_nothing_on_has_nothing_put_in() {
    let rig = rig(Setup {
        rules: "other.test##.banner\n##.ad-box\n",
        cosmetic: true,
        ..Setup::default()
    })
    .await;
    let (_, body, _) = rig.get(Trust::System, H1, "/page", None).await.unwrap();
    // Looked through for what goes by a class, and so asked for
    // unpacked; it has none that is named, and is as the server sent
    // it.
    assert_eq!(
        body,
        "<!doctype html><html><head><title>t</title></head><body>identity</body></html>"
    );
}

#[tokio::test]
async fn what_goes_by_a_class_the_page_has_is_hidden_at_its_end() {
    for alpn in [H1, BOTH] {
        let rig = rig(Setup {
            rules: "##.ad-box\n##.not-there\nsite.test##.ad\n",
            alpn,
            cosmetic: true,
            ..Setup::default()
        })
        .await;
        let (_, body, _) = rig
            .get(Trust::System, alpn, "/page/named", None)
            .await
            .unwrap();
        assert_eq!(
            body,
            "<!doctype html><html><head><style>.ad{display:none!important}\n</style>\
             <title>t</title></head><body>identity<div class=\"ad-box\"></div></body></html>\
             <style>.ad-box{display:none!important}\n</style>"
        );
    }
}

#[tokio::test]
async fn a_site_the_lists_except_from_such_rules_is_not_looked_through() {
    let rig = rig(Setup {
        rules: "##.ad-box\n@@||site.test^$generichide\n",
        cosmetic: true,
        ..Setup::default()
    })
    .await;
    let (_, body, _) = rig
        .get(Trust::System, H1, "/page/named", None)
        .await
        .unwrap();
    // As it came, and asked for as the app asked.
    assert!(body.contains("gzip, br") && !body.contains("<style"));
}

#[tokio::test]
async fn url_filtering_off_never_decrypts_nor_mounts_trust() {
    let rig = rig(Setup {
        domain_only: true,
        rules: "/ads/\n",
        ..Setup::default()
    })
    .await;
    // An app that would accept us is given the server itself.
    assert!(rig.handshake(Trust::System, H1).await.is_err());
    let (status, body, real) = rig
        .get(Trust::ServerOnly, H1, "/ads/x", None)
        .await
        .unwrap();
    assert!(real);
    assert_eq!((status, body.as_str()), (StatusCode::OK, "GET /ads/x"));
    assert_eq!(rig.count(Counter::Decrypted), 0);
    assert_eq!(rig.made_to_trust.load(Relaxed), 0);
}

#[tokio::test]
async fn an_app_set_to_hosts_still_has_a_host_blocked_whole_refused() {
    let rig = rig(Setup {
        domain_only: true,
        rules: "||site.test^\n",
        ..Setup::default()
    })
    .await;
    assert!(rig.handshake(Trust::ServerOnly, H1).await.is_err());
    assert_eq!(rig.hits(), 0);
    assert_eq!(rig.count(Counter::HostsBlocked), 1);
}

#[tokio::test]
async fn a_trusted_host_is_handed_on_unread_even_if_a_rule_blocks_it_whole() {
    let rig = rig(Setup {
        rules: "||site.test^\n",
        allow: "site.test\n",
        ..Setup::default()
    })
    .await;
    let (status, _, real) = rig.get(Trust::ServerOnly, H1, "/", None).await.unwrap();
    assert!(real && status == StatusCode::OK);
    assert_eq!(rig.count(Counter::HostsBlocked), 0);
    assert_eq!(rig.count(Counter::Decrypted), 0);
}

#[tokio::test(start_paused = true)]
#[allow(
    clippy::too_many_lines,
    reason = "checks TLS and UDP decisions, counters and events on a shared fixture"
)]
async fn tls_and_udp_host_checks_preserve_trust_precedence_and_mode_specific_blocks() {
    let rig = rig(Setup {
        rules: "||trusted.test^\n||blocked.test^\n||ads.test^\n@@||ads.test/pass^$image\n",
        allow: "trusted.test\nrooted.test.\nxn--bcher-kva.test\nlocal\n192.0.2.7\n2001:db8::7\n",
        ..Setup::default()
    })
    .await;
    let ctx = &rig.ctx;
    let filters = ctx.filters();
    let allow = ctx.lists.allow.read().unwrap().clone();
    let exclusions = ctx.lists.exclusions.read().unwrap().clone();
    let mut watcher = ctx.services.live.join(Some(PACKAGE.to_owned())).unwrap();
    let trusted = Verdict::Passthrough(Reason::Trusted);
    for domain_only in [false, true] {
        ctx.selection.url_filtering.store(!domain_only, Relaxed);
        let unmatched = if domain_only {
            Verdict::Passthrough(Reason::DomainOnly)
        } else {
            Verdict::Mitm
        };
        let guarded = if domain_only {
            Verdict::Block
        } else {
            unmatched
        };
        for (host, expected) in [
            ("child.trusted.test", trusted),
            ("child..trusted.test", trusted),
            ("child.rooted.test.", trusted),
            ("child.rooted.test", unmatched),
            ("child.xn--bcher-kva.test", trusted),
            ("child.bücher.test", unmatched),
            ("local", trusted),
            ("192.0.2.7", trusted),
            ("2001:db8::7", trusted),
            ("", unmatched),
            ("ads.test", guarded),
            ("blocked.test", Verdict::Block),
        ] {
            let hello = Hello {
                sni: Some(host.to_owned()),
                ..Hello::default()
            };
            assert_eq!(
                ctx.services.policy.decide(
                    PACKAGE,
                    &hello,
                    &filters,
                    &exclusions,
                    &allow,
                    domain_only
                ),
                expected,
                "TLS: {host:?} with domain_only={domain_only}"
            );
            assert_eq!(
                ctx.blocks_udp_host(UID, host),
                expected == Verdict::Block,
                "UDP: {host:?} with domain_only={domain_only}"
            );
        }
        assert!(!ctx.blocks_udp_host(UID + 1, "blocked.test"));
    }

    assert_eq!(rig.count(Counter::HostsBlocked), 3);
    assert_eq!(rig.count(Counter::Passed), 0);
    let mut out = Vec::new();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(1),
            crate::live::feed(&mut watcher, &mut out)
        )
        .await
        .is_err()
    );
    let events: Vec<serde_json::Value> = std::str::from_utf8(&out)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 3);
    for (event, host) in events
        .iter()
        .zip(["blocked.test", "ads.test", "blocked.test"])
    {
        assert_eq!(event["app"], PACKAGE);
        assert_eq!(event["host"], host);
        assert_eq!(event["verdict"], "host_blocked");
    }
}

#[tokio::test]
async fn whoever_watches_is_told_what_became_of_each_request_and_connection() {
    let rig = rig(Setup {
        rules: "/ads/\n",
        ..Setup::default()
    })
    .await;
    let mut watcher = rig
        .ctx
        .services
        .live
        .join(Some(PACKAGE.to_owned()))
        .unwrap();
    let page = Some("https://site.test/story");
    rig.get(Trust::System, H1, "/ads/a.js", page).await.unwrap();
    rig.get(Trust::System, H1, "/img/b.png", None)
        .await
        .unwrap();
    rig.ctx.services.policy.rejected(PACKAGE, HOST);
    rig.get(Trust::ServerOnly, H1, "/", None).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(
        Duration::from_millis(200),
        crate::live::feed(&mut watcher, &mut out),
    )
    .await;
    let got: Vec<serde_json::Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let seen = |e: &serde_json::Value| {
        let s = |k: &str| e[k].as_str().map(str::to_owned);
        (s("verdict"), s("url"), s("page"), s("kind"), s("reason"))
    };
    let some = |s: &str| Some(s.to_owned());
    assert_eq!(
        got.iter().map(seen).collect::<Vec<_>>(),
        [
            (
                some("blocked"),
                some("https://site.test/ads/a.js"),
                some("https://site.test/story"),
                some("script"),
                None
            ),
            (
                some("allowed"),
                some("https://site.test/img/b.png"),
                None,
                some("image"),
                None
            ),
            (some("passed"), None, None, None, some("pinned")),
        ]
    );
    assert!(got.iter().all(|e| e["app"] == PACKAGE && e["host"] == HOST));
}

#[test]
fn only_a_connection_of_an_app_to_somewhere_else_is_carried() {
    let server: SocketAddr = "93.184.216.34:443".parse().unwrap();
    let origin = Origin::of(server, 0x64, Some(10145)).unwrap();
    assert_eq!(
        (origin.dst(), origin.mark(), origin.uid()),
        (server, 0x64, 10145)
    );
    // An app of a second user.
    assert!(Origin::of(server, 0x64, Some(1_010_145)).is_some());

    // To this device itself: to where we listen, it may be.
    for local in ["127.0.0.1:40000", "[::1]:40000", "0.0.0.0:443"] {
        assert_eq!(Origin::of(local.parse().unwrap(), 0x64, Some(10145)), None);
    }

    // Nobody's, root's, the system's.
    assert_eq!(Origin::of(server, 0x64, None), None);
    assert_eq!(Origin::of(server, 0x64, Some(0)), None);
    assert_eq!(Origin::of(server, 0x64, Some(1000)), None);
    assert_eq!(Origin::of(server, 0x64, Some(1_001_000)), None);
}

#[test]
fn only_a_missing_way_counts_against_the_network() {
    let error = |code| std::io::Error::from_raw_os_error(code);
    assert!(no_way(&error(libc::ENETUNREACH)));
    assert!(no_way(&error(libc::EHOSTUNREACH)));
    // The server's doing, or ours.
    assert!(!no_way(&error(libc::ECONNREFUSED)));
    assert!(!no_way(&error(libc::EMFILE)));
    assert!(!no_way(&std::io::ErrorKind::TimedOut.into()));
}

#[tokio::test]
async fn a_handshake_that_just_ends_is_not_taken_for_a_refusal_at_once() {
    let rig = rig(Setup::default()).await;
    // A client that hangs up in the middle of its hello's answer.
    let hello = {
        let mut config = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let name = ServerName::try_from(HOST).unwrap();
        let mut conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
        let mut out = Vec::new();
        conn.write_tls(&mut out).unwrap();
        out
    };
    let mut tcp = TcpStream::connect(rig.proxy).await.unwrap();
    tcp.write_all(&hello).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(tcp);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.count(Counter::Pinned), 0);
    // And the host is still decrypted.
    let (_, _, real) = rig.get(Trust::System, H1, "/", None).await.unwrap();
    assert!(!real);
}

#[tokio::test]
async fn a_server_that_never_answers_the_hello_does_not_hold_the_app_for_ever() {
    tokio::time::pause();
    let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent_addr = silent.local_addr().unwrap();
    let rig = rig(Setup::default()).await;
    // The same daemon, in front of a server that accepts and is silent.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = listener.local_addr().unwrap();
    let ctx = rig.ctx.clone();
    let origin = Origin {
        dst: silent_addr,
        mark: 0,
        uid: UID,
    };
    let conducted = tokio::spawn(async move {
        let (app, _) = listener.accept().await.unwrap();
        conduct(ctx, app, origin).await
    });
    let held = tokio::spawn(async move {
        let mut kept = Vec::new();
        loop {
            kept.push(silent.accept().await.unwrap());
        }
    });
    let mut tcp = TcpStream::connect(proxy).await.unwrap();
    let record = {
        let config = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth();
        let name = ServerName::try_from(HOST).unwrap();
        let mut conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
        let mut out = Vec::new();
        conn.write_tls(&mut out).unwrap();
        out
    };
    tcp.write_all(&record).await.unwrap();
    drop(tcp);
    // With the clock running on by itself: the handshake is given up,
    // the connection handed on, and that ended too once the app is
    // gone and the server stays silent.
    let ended = tokio::time::timeout(HANDSHAKE_TIME + LINGER_TIME * 2, conducted).await;
    assert!(ended.is_ok(), "the connection was never let go");
    held.abort();
}

#[tokio::test(start_paused = true)]
async fn a_decrypted_upgrade_whose_server_never_closes_is_let_go_after_the_app_leaves() {
    let rig = rig(Setup::default()).await;
    let tls = rig.handshake(Trust::System, H1).await.unwrap();
    assert!(tls.get_ref().1.peer_certificates().unwrap()[0] != rig.server_leaf);
    let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(conn.with_upgrades());
    let req = Request::get("/upgrade")
        .header(HOST_HEADER, HOST)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .body(Empty::<Bytes>::new())
        .unwrap();
    let mut resp = tx.send_request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut io = TokioIo::new(hyper::upgrade::on(&mut resp).await.unwrap());
    // The app is done sending; the server never says anything again.
    io.shutdown().await.unwrap();
    let mut buf = [0u8; 1];
    let ended = tokio::time::timeout(LINGER_TIME * 2, io.read(&mut buf)).await;
    assert!(
        matches!(ended, Ok(Ok(0) | Err(_))),
        "the tunnel was never let go"
    );
}

#[tokio::test]
async fn requests_on_one_connection_are_answered_in_turn() {
    let rig = rig(Setup::default()).await;
    let tls = rig.handshake(Trust::System, H1).await.unwrap();
    let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(conn);
    for path in ["/one", "/two", "/three"] {
        let req = Request::get(path)
            .header(HOST_HEADER, HOST)
            .body(Empty::<Bytes>::new())
            .unwrap();
        let resp = tx.send_request(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, format!("GET {path}"));
    }
    assert_eq!(rig.hits(), 3);
}

/// Not a test: prints what a connection and a megabyte cost through
/// the daemon. `cargo test --release -- --ignored --nocapture measures`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "a measurement, run by hand"]
async fn measures_connections_and_throughput() {
    async fn client(rig: &Rig, config: &Arc<rustls::ClientConfig>, path: &str) -> (usize, bool) {
        let tcp = TcpStream::connect(rig.proxy).await.unwrap();
        tcp.set_nodelay(true).unwrap();
        let name = ServerName::try_from(HOST).unwrap();
        let tls = TlsConnector::from(config.clone())
            .connect(name, tcp)
            .await
            .unwrap();
        let real = tls.get_ref().1.peer_certificates().unwrap()[0] == rig.server_leaf;
        let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .unwrap();
        tokio::spawn(conn);
        let req = Request::get(path)
            .header(HOST_HEADER, HOST)
            .body(Empty::<Bytes>::new())
            .unwrap();
        let mut body = tx.send_request(req).await.unwrap().into_body();
        let mut n = 0;
        while let Some(frame) = body.frame().await {
            n += frame.unwrap().data_ref().map_or(0, Bytes::len);
        }
        (n, real)
    }
    let config = |rig: &Rig, trust: Trust| {
        let mut c = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(rig.roots(&trust))
            .with_no_client_auth();
        c.alpn_protocols = vec![b"http/1.1".to_vec()];
        Arc::new(c)
    };
    let rig = rig(Setup::default()).await;

    // A browser opening connection after connection to one site, as
    // it does: with one configuration, so that it may resume.
    let ours = config(&rig, Trust::System);
    client(&rig, &ours, "/warm").await;
    let (n, started) = (300, std::time::Instant::now());
    for _ in 0..n {
        client(&rig, &ours, "/x").await;
    }
    println!(
        "decrypted: {:?} a connection with one request",
        started.elapsed() / n
    );

    let megabytes = 256;
    let started = std::time::Instant::now();
    let (got, real) = client(&rig, &ours, &format!("/big/{megabytes}")).await;
    assert!(!real && got == megabytes << 20);
    println!(
        "decrypted: {:.0} MiB/s",
        megabytes as f64 / started.elapsed().as_secs_f64()
    );

    // The same, handed on untouched: an app that pins.
    rig.ctx.services.policy.rejected(PACKAGE, HOST);
    let theirs = config(&rig, Trust::ServerOnly);
    let started = std::time::Instant::now();
    let (got, real) = client(&rig, &theirs, &format!("/big/{megabytes}")).await;
    assert!(real && got == megabytes << 20);
    println!(
        "handed on: {:.0} MiB/s",
        megabytes as f64 / started.elapsed().as_secs_f64()
    );
}

#[test]
fn a_request_is_of_the_kind_the_browser_says_or_its_name_suggests() {
    let with = |dest: Option<&str>, path: &str| {
        let mut req = Request::get(path);
        if let Some(dest) = dest {
            req = req.header("sec-fetch-dest", dest);
        }
        kind(&req.body(()).unwrap())
    };
    assert_eq!(with(Some("script"), "/a"), "script");
    assert_eq!(with(Some("empty"), "/api"), "xmlhttprequest");
    assert_eq!(with(Some("iframe"), "/frame"), "subdocument");
    // A native app says nothing.
    assert_eq!(with(None, "/static/app.js"), "script");
    assert_eq!(with(None, "/img/logo.png"), "image");
    assert_eq!(with(None, "/v1/feed"), "other");
}

#[test]
fn a_page_gone_to_is_its_own_first_party_whatever_led_there() {
    let dir = tempfile::tempdir().unwrap();
    let rules = "||ads.example/own$document,script,first-party\n\
                 ||ads.example/other$document,script,third-party\n";
    std::fs::write(dir.path().join("list.txt"), rules).unwrap();
    let filters = Filters::load(&[dir.path()], false).unwrap();
    let blocked = |path: &str, dest: Option<&str>, referer: Option<&str>| {
        let mut req = Request::get(path);
        if let Some(dest) = dest {
            req = req.header("sec-fetch-dest", dest);
        }
        if let Some(referer) = referer {
            req = req.header(REFERER, referer);
        }
        let url = format!("https://ads.example{path}");
        evaluate_request(&filters.for_app("com.app"), &req.body(()).unwrap(), &url).blocked
    };
    let news = Some("https://news.example/");
    for referer in [None, news] {
        assert!(blocked("/own", Some("document"), referer), "{referer:?}");
        assert!(!blocked("/other", Some("document"), referer), "{referer:?}");
    }
    assert!(blocked("/other", Some("script"), news));
    assert!(!blocked("/own", Some("script"), news));
    // Made by no page it names: neither first party nor third.
    assert!(!blocked("/own", Some("script"), None));
    assert!(!blocked("/other", Some("script"), None));
}

#[test]
fn an_https_request_can_only_name_the_verified_site() {
    let get = |uri: &str, host: &str| {
        Request::get(uri)
            .header(HOST_HEADER, host)
            .body(())
            .unwrap()
    };
    assert_eq!(
        address(&get("/a?b=1", "site.test"), HOST, 443).as_deref(),
        Some("https://site.test/a?b=1")
    );
    for req in [
        get("/a", "other.test"),
        get("https://other.test/a", HOST),
        get("https://site.test/a", "other.test"),
        get("http://site.test/a", HOST),
        get("/a", "site.test:81"),
        get("/a", "user@site.test"),
    ] {
        assert!(address(&req, HOST, 443).is_none());
    }
}
