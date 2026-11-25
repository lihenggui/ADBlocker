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

mod cookie;
mod csp;
mod hls;
mod removeheader;
mod removeparam;
mod urltransform;

const HOST: &str = "site.test";
const PACKAGE: &str = "com.app";
const UID: u32 = 10001;

/// A server, a daemon in front of it, and what is needed to be a
/// client of either.
struct Rig {
    ctx: Arc<Ctx>,
    /// Where the app connects: the daemon.
    proxy: SocketAddr,
    origin: Origin,
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
    /// The settings file, when the user has saved choices.
    settings: Option<&'static str>,
    /// The hosts the user trusts, a line each.
    allow: &'static str,
    /// The original destination port and the test socket it maps to.
    plaintext: Option<(u16, SocketAddr)>,
    plaintext_destination: Option<SocketAddr>,
    /// An HTTPS destination port represented by the test server's socket.
    https_destination: Option<u16>,
    additional_name: Option<&'static str>,
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
            settings: None,
            allow: "",
            plaintext: None,
            plaintext_destination: None,
            https_destination: Some(443),
            additional_name: None,
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
    let names = std::iter::once(HOST).chain(setup.additional_name);
    let mut leaf = CertificateParams::new(names.map(str::to_owned).collect::<Vec<_>>()).unwrap();
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
                    if req.uri().path() == "/raw-host" {
                        let host = req
                            .headers()
                            .get(hyper::header::HOST)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("-");
                        let authority = req.uri().authority().map_or("-", |value| value.as_str());
                        let body = format!("{host}|{authority}");
                        return std::future::ready(Ok(Response::new(
                            Full::new(Bytes::from(body)).boxed(),
                        )));
                    }
                    if let Some(response) = delayed_or_late_fixture(req.uri().path()) {
                        return std::future::ready(Ok(response));
                    }
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
                            .body(Full::new(Bytes::new()).boxed())
                            .unwrap()));
                    }
                    if let Some(response) = replacement_fixture(&req)
                        .or_else(|| cookie::fixture(&req))
                        .or_else(|| removeheader::fixture(&req))
                        .or_else(|| hls::fixture(&req))
                        .or_else(|| urltransform::fixture(&req))
                    {
                        return std::future::ready(Ok(response.map(BodyExt::boxed)));
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
                                            .body(
                                                Full::new(Bytes::from_static(b"<body>no head"))
                                                    .boxed(),
                                            )
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
                    std::future::ready(Ok::<_, hyper::Error>(resp.map(BodyExt::boxed)))
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
    let rules = dir.path().join("rules");
    std::fs::create_dir(&rules).unwrap();
    std::fs::write(rules.join("list.txt"), setup.rules).unwrap();
    std::fs::write(dir.path().join("allow"), setup.allow).unwrap();
    let settings_file = dir.path().join("settings");
    if let Some(text) = setup.settings {
        std::fs::write(&settings_file, text).unwrap();
    }
    let settings = Settings::try_read(&settings_file, Settings::UNSAID).unwrap();
    let (events, _) = mpsc::unbounded_channel();
    let trusting = setup.trusting;
    let made_to_trust = Arc::new(AtomicUsize::new(0));
    let asked = made_to_trust.clone();
    let plaintext = setup.plaintext;
    let https_destination = setup.https_destination;
    let original_dst = if let Some((port, _)) = plaintext {
        setup
            .plaintext_destination
            .unwrap_or(SocketAddr::from(([192, 0, 2, 42], port)))
    } else if let Some(port) = https_destination {
        SocketAddr::from(([192, 0, 2, 42], port))
    } else {
        server_addr
    };
    let uid = setup.uid;
    let dials = Arc::new(AtomicUsize::new(0));
    let dialed = dials.clone();
    let tls = Tls::new(Authority::open(&dir.path().join("ca")).unwrap(), roots);
    let ctx = Arc::new(Ctx {
        lists: Lists {
            filters: RwLock::new(Arc::new(Filters::load(&[&rules], true).unwrap())),
            exclusions: RwLock::new(Arc::new(setup.exclusions)),
            allow: RwLock::new(Arc::new(Allow::load(&dir.path().join("allow")))),
        },
        selection: Selection {
            apps: RwLock::new(Arc::new(BTreeMap::from([(UID, Arc::from(PACKAGE))]))),
            profile_tls: RwLock::default(),
            url_filtering: AtomicBool::new(!setup.domain_only),
            https_inspection_available: AtomicBool::new(true),
            cosmetic: AtomicBool::new(settings.cosmetic),
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
                    let dst = if let Some((_, socket)) = plaintext {
                        assert_eq!(origin.dst, original_dst);
                        assert_eq!(origin.mark, 42);
                        assert_eq!(origin.uid, uid);
                        socket
                    } else if https_destination.is_some() {
                        assert_eq!(origin.dst, original_dst);
                        server_addr
                    } else {
                        origin.dst
                    };
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
        dst: original_dst,
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
        origin,
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
        self.handshake_to(HOST, trust, alpn, ech).await
    }

    async fn handshake_to(
        &self,
        host: &'static str,
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
        let name = ServerName::try_from(host).unwrap();
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
        self.request_response(trust, alpn, path, referer, |req| req)
            .await
    }

    async fn request_response(
        &self,
        trust: Trust,
        alpn: &[&str],
        path: &str,
        referer: Option<&str>,
        customize: impl FnOnce(hyper::http::request::Builder) -> hyper::http::request::Builder,
    ) -> Result<(hyper::HeaderMap, StatusCode, String, bool)> {
        let (headers, status, bytes, real) = self
            .request_response_bytes(trust, alpn, path, referer, customize)
            .await?;
        Ok((headers, status, String::from_utf8(bytes.to_vec())?, real))
    }

    async fn request_response_bytes(
        &self,
        trust: Trust,
        alpn: &[&str],
        path: &str,
        referer: Option<&str>,
        customize: impl FnOnce(hyper::http::request::Builder) -> hyper::http::request::Builder,
    ) -> Result<(hyper::HeaderMap, StatusCode, Bytes, bool)> {
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
        let req = customize(req).body(Empty::<Bytes>::new())?;
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
        Ok((parts.headers, parts.status, body, real))
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
    plain_request_named(rig, path, "ads.example").await
}

async fn plain_request_named(rig: &Rig, path: &str, host: &str) -> TcpStream {
    let mut app = TcpStream::connect(rig.proxy).await.unwrap();
    app.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
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
async fn https_authority_spellings_are_blocked_and_exceptions_still_apply() {
    use hyper::header::HeaderValue;

    for host in ["site.test.", "SITE.TEST", "site.test:443"] {
        for (rules, expected) in [
            ("||site.test/ads$script\n", StatusCode::FORBIDDEN),
            (
                "||site.test/ads$script\n@@||site.test/ads$script\n",
                StatusCode::OK,
            ),
        ] {
            let rig = rig(Setup {
                rules,
                https_destination: Some(443),
                ..Setup::default()
            })
            .await;
            let mut watcher = rig.ctx.services.live.join(None).unwrap();
            let (_, status, _, real) = rig
                .request_response(Trust::System, H1, "/ads", None, |mut req| {
                    req.headers_mut()
                        .unwrap()
                        .insert(HOST_HEADER, HeaderValue::from_static(host));
                    req.header("sec-fetch-dest", "script")
                })
                .await
                .unwrap();
            assert!(!real);
            assert_eq!(status, expected, "{host}: {rules}");
            assert_eq!(rig.hits(), usize::from(expected == StatusCode::OK));
            let event = only_watch_event(&mut watcher).await;
            assert_eq!(event["url"], "https://site.test/ads");
        }
    }
}

#[tokio::test]
async fn cleartext_authority_spellings_are_blocked_and_exceptions_still_apply() {
    for host in ["ads.example.", "ads.example:80"] {
        for (rules, expected) in [
            ("||ads.example/blocked^\n", StatusCode::FORBIDDEN),
            (
                "||ads.example/blocked^\n@@||ads.example/blocked^\n",
                StatusCode::OK,
            ),
        ] {
            let (rig, server) = plain_rig(Setup {
                rules,
                ..Setup::default()
            })
            .await;
            let upstream = (expected == StatusCode::OK).then(|| {
                tokio::spawn(async move {
                    let (tcp, _) = server.accept().await.unwrap();
                    let service = service_fn(|req: Request<Incoming>| async move {
                        assert_eq!(req.headers()[HOST_HEADER], host);
                        Ok::<_, hyper::Error>(Response::new(Empty::<Bytes>::new()))
                    });
                    hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(tcp), service)
                        .await
                        .unwrap();
                })
            });
            let response = plain_response(plain_request_named(&rig, "/blocked", host).await).await;
            assert!(
                response.starts_with(&format!("HTTP/1.1 {} ", expected.as_u16())),
                "{host}: {response}"
            );
            assert_eq!(
                rig.dials.load(Relaxed),
                usize::from(expected == StatusCode::OK)
            );
            if let Some(upstream) = upstream {
                upstream.await.unwrap();
            }
        }
    }
}

#[tokio::test]
async fn a_non_default_https_port_is_kept_for_a_port_rule() {
    let rig = rig(Setup {
        rules: "|https://site.test:8443/ads$script\n",
        https_destination: Some(8443),
        ..Setup::default()
    })
    .await;
    let (_, status, _, real) = rig
        .request_response(Trust::System, H1, "/ads", None, |mut req| {
            req.headers_mut().unwrap().insert(
                HOST_HEADER,
                hyper::header::HeaderValue::from_static("SITE.TEST:8443"),
            );
            req.header("sec-fetch-dest", "script")
        })
        .await
        .unwrap();
    assert!(!real);
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(rig.hits(), 0);
}

#[tokio::test]
async fn an_unnamed_https_port_uses_the_verified_connection_port_for_rules() {
    for alpn in [H1, &["h2"][..]] {
        for (rules, expected) in [
            (
                "|https://site.test:8443/ads$script\n",
                StatusCode::FORBIDDEN,
            ),
            ("|https://site.test:9443/ads$script\n", StatusCode::OK),
        ] {
            let rig = rig(Setup {
                rules,
                alpn,
                https_destination: Some(8443),
                ..Setup::default()
            })
            .await;
            let (_, status, _, real) = rig
                .request_response(Trust::System, alpn, "/ads", None, |req| {
                    req.header("sec-fetch-dest", "script")
                })
                .await
                .unwrap();
            assert!(!real);
            assert_eq!(status, expected, "{alpn:?}: {rules}");
            assert_eq!(rig.hits(), usize::from(expected == StatusCode::OK));
        }
    }
}

#[tokio::test]
async fn http2_authority_uses_the_same_rules_and_preserves_the_origin_authority() {
    for (rules, expected) in [
        ("||site.test/ads$script\n", StatusCode::FORBIDDEN),
        (
            "||site.test/ads$script\n@@||site.test/ads$script\n",
            StatusCode::OK,
        ),
    ] {
        let rig = rig(Setup {
            rules,
            alpn: &["h2"],
            https_destination: Some(443),
            ..Setup::default()
        })
        .await;
        let (_, status, _, _) = rig
            .request_response(Trust::System, &["h2"], "/ads", None, |req| {
                req.uri("https://SITE.TEST.:443/ads")
                    .header("sec-fetch-dest", "script")
            })
            .await
            .unwrap();
        assert_eq!(status, expected);
        assert_eq!(rig.hits(), usize::from(expected == StatusCode::OK));
        if expected == StatusCode::OK {
            let (_, status, body, _) = rig
                .request_response(Trust::System, &["h2"], "/raw-host", None, |req| {
                    req.uri("https://SITE.TEST.:443/raw-host")
                })
                .await
                .unwrap();
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, "site.test|SITE.TEST.:443");
        }
    }
}

#[tokio::test]
async fn a_capitalized_referer_uses_the_named_page_rule() {
    for (rules, expected) in [
        (
            "||site.test/ads$script,domain=page.test\n",
            StatusCode::FORBIDDEN,
        ),
        (
            "||site.test/ads$script,domain=page.test\n@@||site.test/ads$script,domain=page.test\n",
            StatusCode::OK,
        ),
    ] {
        let rig = rig(Setup {
            rules,
            ..Setup::default()
        })
        .await;
        let (_, status, _, _) = rig
            .request_response(
                Trust::System,
                H1,
                "/ads",
                Some("https://PAGE.TEST/article"),
                |req| req.header("sec-fetch-dest", "script"),
            )
            .await
            .unwrap();
        assert_eq!(status, expected);
        assert_eq!(rig.hits(), usize::from(expected == StatusCode::OK));
    }
}

#[tokio::test]
async fn a_capitalized_origin_uses_the_named_page_rule() {
    let rig = rig(Setup {
        rules: "||site.test/ads$script,domain=page.test\n",
        ..Setup::default()
    })
    .await;
    let (_, status, _, _) = rig
        .request_response(Trust::System, H1, "/ads", None, |req| {
            req.header("origin", "https://PAGE.TEST.:443")
                .header("sec-fetch-dest", "script")
        })
        .await
        .unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(rig.hits(), 0);
}

#[tokio::test]
async fn the_response_type_recheck_keeps_the_canonical_request_url() {
    let rig = rig(Setup {
        rules: "||site.test/creative/small$image\n",
        ..Setup::default()
    })
    .await;
    let (_, status, _, _) = rig
        .request_response(Trust::System, H1, "/creative/small", None, |mut req| {
            req.headers_mut().unwrap().insert(
                HOST_HEADER,
                hyper::header::HeaderValue::from_static("SITE.TEST."),
            );
            req
        })
        .await
        .unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(rig.hits(), 1);
}

#[tokio::test]
async fn the_vpn_udp_host_check_uses_the_canonical_name() {
    let rig = rig(Setup {
        rules: "||ads.example^\n",
        ..Setup::default()
    })
    .await;
    assert!(rig.ctx.blocks_udp_host(UID, "ads.example"));
    assert!(rig.ctx.blocks_udp_host(UID, "ADS.EXAMPLE."));
    assert!(!rig.ctx.blocks_udp_host(UID, "ads.example.."));
}

#[tokio::test]
async fn malformed_authorities_are_refused_before_the_origin_receives_a_request() {
    let rig = rig(Setup::default()).await;
    let (_, status, _, _) = rig
        .request_response(Trust::System, H1, "/ads", None, |mut req| {
            req.headers_mut().unwrap().insert(
                HOST_HEADER,
                hyper::header::HeaderValue::from_static("site.test.."),
            );
            req
        })
        .await
        .unwrap();
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    assert_eq!(rig.hits(), 0);

    let (rig, _server) = plain_rig(Setup::default()).await;
    let response = plain_response(plain_request_named(&rig, "/ads", "ads.example..").await).await;
    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    assert_eq!(rig.dials.load(Relaxed), 0);
}

#[tokio::test]
async fn an_allowed_https_request_reaches_the_origin_with_its_original_host() {
    let rig = rig(Setup {
        https_destination: Some(443),
        ..Setup::default()
    })
    .await;
    let (_, status, body, _) = rig
        .request_response(Trust::System, H1, "/raw-host", None, |mut req| {
            req.headers_mut().unwrap().insert(
                HOST_HEADER,
                hyper::header::HeaderValue::from_static("SITE.TEST.:443"),
            );
            req
        })
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "SITE.TEST.:443|-");
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
async fn a_cleartext_banner_named_by_host_and_referer_is_blocked() {
    use tokio::io::AsyncBufReadExt as _;

    let mut results = Vec::new();
    for (name, referer, close, rules) in [
        ("baseline", false, false, "||fixture.example/ads/\n"),
        ("referer", true, false, "||fixture.example/ads/\n"),
        ("close", false, true, "||fixture.example/ads/\n"),
        ("referer and close", true, true, "||fixture.example/ads/\n"),
        ("shorter path", true, true, "||fixture.example/ads\n"),
        ("no path", true, true, "||fixture.example^\n"),
    ] {
        let (rig, server) = plain_rig(Setup {
            rules,
            plaintext_destination: Some(SocketAddr::from(([198, 18, 0, 2], 80))),
            ..Setup::default()
        })
        .await;
        let upstream = tokio::spawn(async move {
            let (mut tcp, _) = server.accept().await.unwrap();
            let mut buf = [0; 1024];
            assert!(tcp.read(&mut buf).await.unwrap() > 0);
            tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });
        let mut app = TcpStream::connect(rig.proxy).await.unwrap();
        let mut request = "GET /ads/banner HTTP/1.1\r\nHost: fixture.example\r\n".to_owned();
        if referer {
            request.push_str("Referer: http://fixture.example/articles/today\r\n");
        }
        if close {
            request.push_str("Connection: close\r\n");
        }
        request.push_str("\r\n");
        app.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::io::BufReader::new(app).read_line(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        results.push((name, response, rig.dials.load(Relaxed)));
        upstream.abort();
    }
    assert!(
        results
            .iter()
            .all(|(_, response, dials)| response.starts_with("HTTP/1.1 403 ") && *dials == 0),
        "{results:?}"
    );
}

#[tokio::test]
async fn an_https_banner_named_by_host_and_referer_is_blocked() {
    for alpn in [H1, &["h2"][..]] {
        let rig = rig(Setup {
            rules: "||site.test/ads/\n",
            alpn,
            ..Setup::default()
        })
        .await;
        let (status, _, real) = rig
            .get(
                Trust::System,
                alpn,
                "/ads/banner",
                Some("https://site.test/articles/today"),
            )
            .await
            .unwrap();
        assert_eq!(status, StatusCode::FORBIDDEN, "{alpn:?}");
        assert!(!real);
        assert_eq!(rig.hits(), 0);
    }
}

#[tokio::test]
async fn a_cleartext_image_is_blocked_without_waiting_for_its_body() {
    let (rig, server) = plain_rig(Setup {
        rules: "||ads.example/creative$image\n",
        ..Setup::default()
    })
    .await;
    let upstream = tokio::spawn(async move {
        let (mut tcp, _) = server.accept().await.unwrap();
        let mut request = String::new();
        let mut reader = tokio::io::BufReader::new(&mut tcp);
        use tokio::io::AsyncBufReadExt as _;
        loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
            if line == "\r\n" {
                break;
            }
            request.push_str(&line);
        }
        assert!(request.starts_with("GET /creative HTTP/1.1\r\n"));
        tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 10\r\n\r\n")
            .await
            .unwrap();
        std::future::pending::<()>().await;
    });
    let response = plain_response(plain_request(&rig, "/creative").await).await;
    assert!(response.starts_with("HTTP/1.1 403 "), "{response}");
    assert!(response.ends_with("\r\n\r\n"), "{response}");
    assert_eq!(rig.count(Counter::RequestsBlocked), 1);
    assert_eq!(rig.count(Counter::RequestsAllowed), 0);
    upstream.abort();
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
    let (rig, server) = plain_rig(Setup {
        rules: "||ads.example/socket$removeheader=request:x-keep\n\
                ||ads.example/socket$removeheader=x-keep",
        ..Setup::default()
    })
    .await;
    let upstream = tokio::spawn(async move {
        let (tcp, _) = server.accept().await.unwrap();
        let (upgrade, upgraded) = tokio::sync::oneshot::channel();
        let upgrade = std::sync::Mutex::new(Some(upgrade));
        let service = service_fn(|mut req: Request<Incoming>| {
            assert_eq!(req.uri().path(), "/socket");
            assert_eq!(req.headers()["upgrade"], "websocket");
            assert_eq!(req.headers()["x-keep"], "request");
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
                        .header("x-keep", "response")
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
            .header("x-keep", "request")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let mut resp = tx.send_request(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(resp.headers()["x-keep"], "response");
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
async fn cleartext_http_on_port_8080_is_passed_through_unread() {
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rig = rig(Setup {
        rules: "||ads.example^\n",
        plaintext: Some((8080, server.local_addr().unwrap())),
        ..Setup::default()
    })
    .await;
    let mut app = TcpStream::connect(rig.proxy).await.unwrap();
    let prefix = b"GET /blocked HTTP/1.1\r\nHost: ads.example\r\n";
    app.write_all(prefix).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        let (mut up, _) = server.accept().await.unwrap();
        let mut delivered = vec![0; prefix.len()];
        up.read_exact(&mut delivered).await.unwrap();
        assert_eq!(delivered, prefix);

        // A raw relay forwards the prefix before the headers are complete.
        app.write_all(b"\r\n").await.unwrap();
        let mut end = [0; 2];
        up.read_exact(&mut end).await.unwrap();
        assert_eq!(&end, b"\r\n");
    })
    .await
    .expect("the request should reach the upstream without HTTP classification");
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
async fn a_decrypted_http2_connection_advertises_a_bounded_stream_count() {
    let rig = rig(Setup {
        alpn: &["h2"],
        ..Setup::default()
    })
    .await;
    let mut tls = rig.handshake(Trust::System, &["h2"]).await.unwrap();
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
    assert_ne!(
        tls.get_ref().1.peer_certificates().unwrap()[0],
        rig.server_leaf
    );
    let settings = tokio::time::timeout(Duration::from_secs(2), async {
        tls.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\0\0\0\x04\0\0\0\0\0")
            .await
            .unwrap();
        let mut header = [0u8; 9];
        tls.read_exact(&mut header).await.unwrap();
        assert_eq!(&header[3..], &[4, 0, 0, 0, 0, 0]);
        let len = u32::from_be_bytes([0, header[0], header[1], header[2]]) as usize;
        assert!(len <= 16384 && len.is_multiple_of(6));
        let mut settings = vec![0; len];
        tls.read_exact(&mut settings).await.unwrap();
        settings
    })
    .await
    .unwrap();
    assert!(settings.as_chunks::<6>().0.contains(&[0, 3, 0, 0, 0, 128]));
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
async fn a_request_without_a_page_is_no_third_party() {
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
async fn page_exceptions_defer_tls_and_lookup_blocks_after_the_app_shows_a_page() {
    const ALLOWED: &str = "files.test";
    let rig = rig(Setup {
        rules: "||site.test^\n@@||site.test^$script,domain=files.test\n",
        additional_name: Some(ALLOWED),
        ..Setup::default()
    })
    .await;

    assert!(rig.handshake(Trust::System, H1).await.is_err());
    assert_eq!(rig.hits(), 0);
    assert_eq!(rig.dials.load(Relaxed), 0);
    assert_eq!(rig.count(Counter::HostsBlocked), 1);
    assert_eq!(rig.count(Counter::Decrypted), 0);
    assert!(rig.ctx.blocks_udp_host(UID, HOST));

    let page = "https://files.test/index.html";
    let tls = rig
        .handshake_to(ALLOWED, Trust::System, H1, None)
        .await
        .unwrap();
    let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(conn);
    let request = Request::get("/start")
        .header(HOST_HEADER, ALLOWED)
        .header(REFERER, page)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let response = tx.send_request(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body().collect().await.unwrap();
    assert!(!rig.ctx.blocks_udp_host(UID, HOST));

    for (path, referer, expected) in [
        ("/asset.js", Some(page), StatusCode::OK),
        ("/asset.js", None, StatusCode::FORBIDDEN),
        (
            "/asset.js",
            Some("https://other.test/"),
            StatusCode::FORBIDDEN,
        ),
        ("/asset.png", Some(page), StatusCode::FORBIDDEN),
    ] {
        let (status, _, real) = rig.get(Trust::System, H1, path, referer).await.unwrap();
        assert_eq!(status, expected);
        assert!(!real);
    }
    assert_eq!(rig.hits(), 2);
    assert_eq!(rig.count(Counter::HostsBlocked), 2);
    assert_eq!(rig.count(Counter::Decrypted), 5);
    assert_eq!(rig.count(Counter::RequestsBlocked), 3);
}

#[tokio::test]
async fn fetch_site_on_a_judged_request_records_the_app_even_when_blocked() {
    let (rig, server) = plain_rig(Setup {
        rules: "||ads.example^\n@@|https://ads.example/$script,domain=files.test\n",
        ..Setup::default()
    })
    .await;
    tokio::spawn(async move {
        let (tcp, _) = server.accept().await.unwrap();
        let service = service_fn(|_: Request<Incoming>| async {
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
    tokio::spawn(conn);
    let send = |fetch_site| {
        let mut request = Request::get("/asset.js").header(HOST_HEADER, "ads.example");
        if fetch_site {
            request = request.header("sec-fetch-site", "cross-site");
        }
        request.body(Empty::<Bytes>::new()).unwrap()
    };
    let first = tx.send_request(send(false)).await.unwrap();
    assert_eq!(first.status(), StatusCode::FORBIDDEN);
    first.into_body().collect().await.unwrap();
    assert!(!rig.ctx.services.policy.has_shown_page(PACKAGE));

    let selection = &rig.ctx.selection.apps;
    let active = std::mem::take(&mut *selection.write().unwrap());
    let unselected = tx.send_request(send(true)).await.unwrap();
    assert_eq!(unselected.status(), StatusCode::OK);
    unselected.into_body().collect().await.unwrap();
    assert!(!rig.ctx.services.policy.has_shown_page(PACKAGE));

    *selection.write().unwrap() = active;
    assert!(rig.ctx.blocks_udp_host(UID, "ads.example"));
    let selected = tx.send_request(send(true)).await.unwrap();
    assert_eq!(selected.status(), StatusCode::FORBIDDEN);
    selected.into_body().collect().await.unwrap();
    assert!(!rig.ctx.blocks_udp_host(UID, "ads.example"));
}

#[tokio::test]
async fn a_full_page_history_defers_early_blocks_for_an_app_it_could_not_keep() {
    let rig = rig(Setup {
        rules: "||site.test^\n@@||site.test/ok.js$domain=files.test\n",
        ..Setup::default()
    })
    .await;
    let limit = limits::MAX_APPS_WITH_PAGES;
    for app in 1..limit {
        rig.ctx.services.policy.note_page(&format!("com.app{app}"));
    }
    assert!(rig.ctx.blocks_udp_host(UID, HOST));
    rig.ctx.services.policy.note_page("com.last");
    assert!(!rig.ctx.blocks_udp_host(UID, HOST));
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
        rig.ctx.app_filters(&rig.ctx.filters(), PACKAGE),
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
    let mut rig = rig(Setup {
        server_trusted: false,
        ..Setup::default()
    })
    .await;
    rig.ctx.services.policy.rejected(PACKAGE, HOST);
    rig.ctx.services.policy.process_started(PACKAGE);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    rig.proxy = listener.local_addr().unwrap();
    // Returning from the relay releases the try. Finishing the response
    // alone does not establish that the relay has returned.
    let (response, relay) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(rig.get(Trust::ServerOnly, H1, "/", None), async {
            let (app, _) = listener.accept().await?;
            conduct(rig.ctx.clone(), app, rig.origin).await
        })
    })
    .await
    .unwrap();
    let (_, _, real) = response.unwrap();
    assert!(real);
    relay.unwrap();
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
async fn an_app_not_made_to_trust_us_is_refused_a_host_that_no_exception_names() {
    let unnamed = rig(Setup {
        // This exception has only a path, so the host can be refused
        // without asking the server.
        rules: "||site.test^\n@@/pass^$image\n",
        trusting: Trusting::No,
        ..Setup::default()
    })
    .await;
    assert!(unnamed.handshake(Trust::System, H1).await.is_err());
    assert_eq!(unnamed.hits(), 0);
    assert_eq!(unnamed.dials.load(Relaxed), 0);
    assert_eq!(unnamed.count(Counter::HostsBlocked), 1);
    assert_eq!(unnamed.count(Counter::Passed), 0);

    // An exception that names the host leaves it to the app.
    let named = rig(Setup {
        rules: "||site.test^\n@@||site.test/pass^$image\n@@/other^$image\n",
        trusting: Trusting::No,
        ..Setup::default()
    })
    .await;
    assert!(named.get(Trust::ServerOnly, H1, "/", None).await.unwrap().2);
    assert_eq!(named.count(Counter::HostsBlocked), 0);

    // A page exception leaves the host open only while requests can be read.
    let page_only = rig(Setup {
        rules: "||site.test^\n@@||site.test/pass^$image,domain=files.test\n",
        trusting: Trusting::No,
        ..Setup::default()
    })
    .await;
    page_only.ctx.services.policy.note_page(PACKAGE);
    assert!(!page_only.ctx.blocks_udp_host(UID, HOST));
    assert!(page_only.handshake(Trust::System, H1).await.is_err());
    assert_eq!(page_only.dials.load(Relaxed), 0);
    assert_eq!(page_only.count(Counter::HostsBlocked), 1);
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
        Arc::new(BTreeMap::from([(work_uid, Arc::from("31:com.app"))]));
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
        Arc::new(BTreeMap::from([(uid, Arc::from("31:com.app"))]));
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
async fn an_address_block_refuses_a_silent_client_with_or_without_url_filtering() {
    for domain_only in [false, true] {
        let rig = rig(Setup {
            rules: "192.0.2.42:80$network",
            plaintext: Some((80, "127.0.0.1:1".parse().unwrap())),
            domain_only,
            ..Setup::default()
        })
        .await;
        let mut watcher = rig.ctx.services.live.join(None).unwrap();
        let mut app = TcpStream::connect(rig.proxy).await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(2), app.read_u8())
            .await
            .expect("refused without waiting for the client's first byte")
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
        assert_eq!(rig.count(Counter::HostsBlocked), 1);
        assert_eq!(rig.count(Counter::Refused), 0);
        assert_eq!(rig.count(Counter::Decrypted), 0);
        assert_eq!(rig.dials.load(Relaxed), 0);
        assert_eq!(rig.made_to_trust.load(Relaxed), 0);
        let event = only_watch_event(&mut watcher).await;
        assert_eq!(event["app"], PACKAGE);
        assert_eq!(event["host"], "192.0.2.42:80");
        assert_eq!(event["verdict"], "host_blocked");
        assert!(event["url"].is_null());
    }
}

#[tokio::test]
async fn an_address_exception_bypasses_even_a_host_block() {
    let rig = rig(Setup {
        rules: "127.0.0.1$network\n@@127.0.0.1$network\n||site.test^\n",
        https_destination: None,
        ..Setup::default()
    })
    .await;
    let mut watcher = rig.ctx.services.live.join(None).unwrap();
    let (status, _, real) = rig.get(Trust::ServerOnly, H1, "/", None).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(real);
    assert_eq!(rig.hits(), 1);
    assert_eq!(rig.dials.load(Relaxed), 1);
    assert_eq!(rig.made_to_trust.load(Relaxed), 0);
    assert_eq!(rig.count(Counter::Passed), 1);
    assert_eq!(rig.count(Counter::HostsBlocked), 0);
    let event = only_watch_event(&mut watcher).await;
    assert_eq!(event["verdict"], "passed");
    let endpoint: SocketAddr = event["host"].as_str().unwrap().parse().unwrap();
    assert!(endpoint.ip().is_loopback());
    assert!(event["reason"].is_null());
}

#[tokio::test]
async fn an_address_block_for_another_app_does_not_prevent_inspection() {
    let rig = rig(Setup {
        rules: "127.0.0.1$network,app=com.other",
        ..Setup::default()
    })
    .await;
    let (status, _, real) = rig.get(Trust::System, H1, "/", None).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(!real);
    assert_eq!(rig.count(Counter::Decrypted), 1);
    assert_eq!(rig.count(Counter::HostsBlocked), 0);
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
async fn adguard_scriptlet_and_style_reach_the_page_like_their_matcher_forms() {
    let adguard = "site.test#%#//scriptlet('set-constant', 'fixture', '42')\n\
                   site.test#$?#div:contains(Advert) { opacity: 0; }\n";
    let ublock = "site.test##+js(set-constant, fixture, 42)\n\
                  site.test#?#div:contains(Advert):style(opacity: 0;)\n";
    let mut pages = Vec::new();
    for rules in [adguard, ublock] {
        let rig = rig(Setup {
            rules,
            ..Setup::default()
        })
        .await;
        let (_, status, page, _) = rig
            .get_response(Trust::System, H1, "/page", None)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains(r#"setConstant("fixture", "42")"#));
        assert!(page.contains(r#"{"type":"style","arg":"opacity: 0;"}"#));
        assert!(page.contains("<head><script>"));
        pages.push(page);
    }
    assert_eq!(pages[0], pages[1]);
}

#[tokio::test]
async fn a_decrypted_page_runs_matching_javascript_only_with_cosmetics_enabled() {
    for settings in [None, Some("cosmetic off\n")] {
        let rig = rig(Setup {
            rules: "site.test#%#window.adbFixture = 42; // trailing comment\n\
                    other.test#%#window.foreignFixture = true;\n",
            settings,
            ..Setup::default()
        })
        .await;
        let (status, page, real) = rig.get(Trust::System, H1, "/page", None).await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(!real);
        assert_eq!(
            page.contains("(function(){\nwindow.adbFixture = 42; // trailing comment\n})();"),
            settings.is_none()
        );
        assert!(!page.contains("foreignFixture"));
    }
}

#[tokio::test]
async fn without_a_settings_file_matching_elements_are_hidden_on_decrypted_pages() {
    for alpn in [H1, BOTH] {
        let rig = rig(Setup {
            rules: HIDING,
            alpn,
            ..Setup::default()
        })
        .await;
        let (status, body, real) = rig.get(Trust::System, alpn, "/page", None).await.unwrap();
        assert!(!real);
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
async fn with_cosmetic_off_a_decrypted_page_is_passed_on_byte_for_byte() {
    let rig = rig(Setup {
        rules: "site.test##.ad\nsite.test##+js(set-constant, fixture, 42)\nsite.test##div:has-text(Advert)\n",
        settings: Some("cosmetic off\n"),
        ..Setup::default()
    })
    .await;
    let (status, body, real) = rig.get(Trust::System, H1, "/page", None).await.unwrap();
    assert!(!real);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        "<!doctype html><html><head><title>t</title></head><body>gzip, br</body></html>"
    );
}

#[tokio::test]
async fn a_page_the_lists_name_nothing_on_has_nothing_put_in() {
    let rig = rig(Setup {
        rules: "other.test##.banner\n##.ad-box\n",
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
                    ctx.app_filters(&filters, PACKAGE),
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

fn replacement_fixture(req: &Request<Incoming>) -> Option<Response<Full<Bytes>>> {
    let path = req.uri().path().strip_prefix("/replace/")?;
    let asked = req
        .headers()
        .get(ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("none");
    let mut resp = Response::builder().header("etag", "\"fixture\"");
    if path == "partial" {
        resp = resp
            .status(StatusCode::PARTIAL_CONTENT)
            .header("content-range", "bytes 0-15/20");
    }
    if path == "packed" {
        // These bytes are opaque when any non-identity encoding is declared.
        resp = resp.header("content-encoding", "gzip");
    }
    let mime = match path {
        "json" => "application/json",
        "binary" => "application/octet-stream",
        "image" => "image/png",
        "html" => "text/html",
        _ => "text/plain",
    };
    let body = match path {
        "json" => r#"{"ad":true,"keep":"original"}"#.to_owned(),
        "image" => "image\0payload\r\n".to_owned(),
        "html" => "<html><body>original</body></html>".to_owned(),
        _ => format!("original {asked}"),
    };
    Some(
        resp.header("content-type", mime)
            .header("content-length", body.len())
            .header("x-request-encoding", asked)
            .body(Full::new(Bytes::from(body)))
            .unwrap(),
    )
}

const REPLACEMENT_RULES: &str = "||site.test/replace/$replace=/original/changed/\n";

async fn only_watch_event(watcher: &mut crate::live::Joined) -> serde_json::Value {
    let mut out = Vec::new();
    {
        let mut feed = std::pin::pin!(crate::live::feed(watcher, &mut out));
        // The response has arrived and all its events are already queued.
        std::future::poll_fn(|cx| {
            assert!(feed.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
    }
    serde_json::from_slice(&out).unwrap()
}

fn substitute_gif() -> Vec<u8> {
    use base64::{Engine as _, prelude::BASE64_STANDARD};

    BASE64_STANDARD
        .decode(
            include_str!("../../resources/ublock/web_accessible_resources/1x1.gif.base64")
                .replace('\n', ""),
        )
        .unwrap()
}

#[tokio::test]
async fn substitutions_send_the_resource_and_report_a_single_block() {
    let script = include_bytes!("../../resources/ublock/web_accessible_resources/noop.js");
    let html = include_bytes!("../../resources/ublock/web_accessible_resources/noop.html");
    let gif = substitute_gif();
    for (path, dest, kind, mime, expected, hits, alpn) in [
        (
            "/ad.js",
            Some("script"),
            "script",
            "application/javascript",
            script.as_slice(),
            0,
            H1,
        ),
        (
            "/ad.gif",
            Some("image"),
            "image",
            "image/gif",
            gif.as_slice(),
            0,
            BOTH,
        ),
        (
            "/replace/image",
            None,
            "image",
            "image/gif",
            gif.as_slice(),
            1,
            BOTH,
        ),
        (
            "/ad-frame",
            Some("iframe"),
            "subdocument",
            "text/html",
            html.as_slice(),
            0,
            H1,
        ),
    ] {
        let rig = rig(Setup {
            rules: "||site.test/ad.js$script,redirect=noopjs\n\
                    ||site.test/ad.gif$image,redirect=1x1-transparent.gif\n\
                    ||site.test/replace/image$image,redirect=1x1-transparent.gif\n\
                    ||site.test/ad-frame$subdocument,redirect=noopframe\n",
            settings: Some("cosmetic off\n"),
            alpn,
            ..Setup::default()
        })
        .await;
        let mut watcher = rig.ctx.services.live.join(None).unwrap();
        let (headers, status, body, real) = rig
            .request_response_bytes(Trust::System, alpn, path, None, |req| {
                let req = req.header("origin", "https://page.test");
                match dest {
                    Some(dest) => req.header("sec-fetch-dest", dest),
                    None => req,
                }
            })
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(!real);
        assert_substitute_headers(&headers, mime, expected.len());
        assert_eq!(body.as_ref(), expected);
        assert_eq!(rig.hits(), hits);
        assert_eq!(rig.count(Counter::RequestsBlocked), 1);
        assert_eq!(rig.count(Counter::RequestsAllowed), 0);
        let event = only_watch_event(&mut watcher).await;
        assert_eq!(event["verdict"], "blocked");
        assert_eq!(event["kind"], kind);
        assert_eq!(event["url"], format!("https://{HOST}{path}"));
        assert_eq!(event["app"], PACKAGE);
    }
}

#[tokio::test]
async fn click_to_load_frames_receive_an_empty_block_response() {
    let rig = rig(Setup {
        rules: "||site.test/ad-frame$subdocument,redirect=click2load.html",
        ..Setup::default()
    })
    .await;
    let (headers, status, body, _) = rig
        .request_response(Trust::System, H1, "/ad-frame", None, |req| {
            req.header("sec-fetch-dest", "iframe")
        })
        .await
        .unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.is_empty());
    assert!(!headers.contains_key("location"));
    assert_eq!(rig.hits(), 0);
    assert_eq!(rig.count(Counter::RequestsBlocked), 1);
}

fn assert_substitute_headers(headers: &hyper::HeaderMap, mime: &str, len: usize) {
    assert_eq!(headers["content-type"], mime);
    assert_eq!(headers["content-length"], len.to_string());
    assert_eq!(headers["access-control-allow-origin"], "*");
    assert_eq!(headers["cache-control"], "no-store");
    assert!(!headers.contains_key("set-cookie"));
    assert!(!headers.contains_key("etag"));
    if mime == "text/html" {
        assert_eq!(
            headers["content-security-policy"],
            "default-src 'none'; sandbox"
        );
    }
}

#[tokio::test]
async fn a_redirect_rule_without_a_block_leaves_the_upstream_response_intact() {
    let rig = rig(Setup {
        rules: "||site.test/replace/image$image,redirect-rule=1x1-transparent.gif",
        ..Setup::default()
    })
    .await;
    let mut watcher = rig.ctx.services.live.join(None).unwrap();
    let (headers, status, body, _) = rig
        .get_response(Trust::System, H1, "/replace/image", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["etag"], "\"fixture\"");
    assert_eq!(body.as_bytes(), b"image\0payload\r\n");
    assert_eq!(rig.hits(), 1);
    assert_eq!(rig.count(Counter::RequestsBlocked), 0);
    assert_eq!(rig.count(Counter::RequestsAllowed), 1);
    assert_eq!(only_watch_event(&mut watcher).await["verdict"], "allowed");
}

#[tokio::test]
async fn an_image_response_supplies_the_missing_type_for_the_final_verdict() {
    for (rules, dest, status, kind) in [
        (
            "||site.test/replace/image$image,domain=page.test,method=POST\n",
            None,
            StatusCode::FORBIDDEN,
            "image",
        ),
        (
            "||site.test/replace/image$image,app=com.app\n",
            None,
            StatusCode::FORBIDDEN,
            "image",
        ),
        (
            "||site.test/replace/image$image\n@@||site.test/replace/image$image\n",
            None,
            StatusCode::OK,
            "image",
        ),
        (
            "||site.test/replace/image$image,important\n@@||site.test/replace/image$image\n",
            None,
            StatusCode::FORBIDDEN,
            "image",
        ),
        (
            "||site.test/replace/image$image\n",
            Some("empty"),
            StatusCode::OK,
            "xmlhttprequest",
        ),
        ("", None, StatusCode::OK, "image"),
    ] {
        let rig = rig(Setup {
            rules,
            alpn: BOTH,
            ..Setup::default()
        })
        .await;
        let mut watcher = rig.ctx.services.live.join(None).unwrap();
        let path = "/replace/image";
        let page = "https://page.test/article";
        let (headers, actual, body, real) = rig
            .request_response(Trust::System, BOTH, path, Some(page), |req| {
                let req = req.method(Method::POST).header(ACCEPT_ENCODING, "gzip, br");
                match dest {
                    Some(dest) => req.header("sec-fetch-dest", dest),
                    None => req,
                }
            })
            .await
            .unwrap();
        assert_eq!(actual, status, "{rules}");
        assert!(!real);
        assert_eq!(rig.hits(), 1);
        let blocked = status == StatusCode::FORBIDDEN;
        assert_eq!(rig.count(Counter::RequestsBlocked), u64::from(blocked));
        assert_eq!(rig.count(Counter::RequestsAllowed), u64::from(!blocked));
        if blocked {
            assert!(body.is_empty());
            assert!(!headers.contains_key("etag"));
        } else {
            assert_eq!(body.as_bytes(), b"image\0payload\r\n");
            assert_eq!(headers["etag"], "\"fixture\"");
            assert_eq!(headers["x-request-encoding"], "gzip, br");
        }
        let event = only_watch_event(&mut watcher).await;
        assert_eq!(
            event["verdict"],
            if blocked { "blocked" } else { "allowed" }
        );
        assert_eq!(event["kind"], kind);
        assert_eq!(event["url"], format!("https://{HOST}{path}"));
        assert_eq!(event["page"], page);
        assert_eq!(event["app"], PACKAGE);
    }
}

#[tokio::test]
async fn html_from_a_request_of_assumed_type_does_not_identify_a_page() {
    let rig = rig(Setup {
        rules: "||site.test/replace/html$document,subdocument\nsite.test##body\n",
        ..Setup::default()
    })
    .await;
    let (_, status, body, _) = rig
        .get_response(Trust::System, H1, "/replace/html", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "<html><body>original</body></html>");
}

#[tokio::test]
async fn a_response_type_selects_rewrites_and_their_exceptions() {
    for (rules, encoding, body) in [
        (
            "||site.test/replace/json$xmlhttprequest,replace=/original/changed/\n",
            "identity",
            r#"{"ad":true,"keep":"changed"}"#,
        ),
        (
            r"||site.test/replace/json$xmlhttprequest,jsonprune=\$.ad",
            "identity",
            r#"{"keep":"original"}"#,
        ),
        (
            "||site.test/replace/json$xmlhttprequest,replace=/original/changed/\n@@||site.test/replace/json$xmlhttprequest,replace\n",
            "gzip, br",
            r#"{"ad":true,"keep":"original"}"#,
        ),
        (
            "||site.test/replace/json$script,replace=/original/changed/\n",
            "identity",
            r#"{"ad":true,"keep":"original"}"#,
        ),
    ] {
        let rig = rig(Setup {
            rules,
            ..Setup::default()
        })
        .await;
        let (headers, status, actual, _) = rig
            .request_response(Trust::System, H1, "/replace/json", None, |req| {
                req.header(ACCEPT_ENCODING, "gzip, br")
            })
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["x-request-encoding"], encoding, "{rules}");
        assert_eq!(actual, body, "{rules}");
    }
}

#[tokio::test]
async fn a_decrypted_text_response_is_replaced_and_the_origin_is_asked_for_identity() {
    for alpn in [H1, BOTH] {
        let rig = rig(Setup {
            rules: REPLACEMENT_RULES,
            alpn,
            ..Setup::default()
        })
        .await;
        let (headers, status, body, real) = rig
            .request_response(Trust::System, alpn, "/replace/text", None, |req| {
                req.header(ACCEPT_ENCODING, "gzip, br")
            })
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(!real);
        assert_eq!(body, "changed identity");
        assert!(!headers.contains_key("content-length"));
        assert!(!headers.contains_key("etag"));
    }
}

#[tokio::test]
async fn ineligible_responses_keep_their_original_headers_and_body() {
    let rig = rig(Setup {
        rules: REPLACEMENT_RULES,
        ..Setup::default()
    })
    .await;
    for (path, status) in [
        ("packed", StatusCode::OK),
        ("binary", StatusCode::OK),
        ("partial", StatusCode::PARTIAL_CONTENT),
    ] {
        let (headers, actual, body, _) = rig
            .get_response(Trust::System, H1, &format!("/replace/{path}"), None)
            .await
            .unwrap();
        assert_eq!(actual, status);
        assert_eq!(body, "original identity");
        assert!(headers.contains_key("content-length"));
        assert!(headers.contains_key("etag"));
    }
}

#[tokio::test]
async fn head_and_range_requests_keep_their_encoding_and_response_headers() {
    let rig = rig(Setup {
        rules: REPLACEMENT_RULES,
        ..Setup::default()
    })
    .await;
    let (headers, status, body, _) = rig
        .request_response(Trust::System, H1, "/replace/text", None, |req| {
            req.method("HEAD").header(ACCEPT_ENCODING, "gzip")
        })
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());
    assert_eq!(headers["content-length"], "13");
    assert!(headers.contains_key("etag"));
    let (headers, status, body, _) = rig
        .request_response(Trust::System, H1, "/replace/text", None, |req| {
            req.header("range", "bytes=0-15")
                .header(ACCEPT_ENCODING, "gzip")
        })
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "original gzip");
    assert!(headers.contains_key("etag"));
}

#[tokio::test]
async fn response_rules_leave_unselected_apps_alone() {
    let unselected = rig(Setup {
        rules: REPLACEMENT_RULES,
        uid: UID + 1,
        ..Setup::default()
    })
    .await;
    let (headers, status, body, real) = unselected
        .get_response(Trust::ServerOnly, H1, "/replace/text", None)
        .await
        .unwrap();
    assert!(real);
    assert_eq!((status, body.as_str()), (StatusCode::OK, "original none"));
    assert!(headers.contains_key("etag"));
}

#[tokio::test]
async fn a_matching_response_rule_takes_precedence_over_a_request_block() {
    let rig = rig(Setup {
        rules: "||site.test/replace/\n||site.test/replace/text$replace=/original/changed/\n",
        ..Setup::default()
    })
    .await;
    let (status, body, _) = rig
        .get(Trust::System, H1, "/replace/text", None)
        .await
        .unwrap();
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "changed identity")
    );
    assert_eq!(rig.hits(), 1);
    assert_eq!(rig.count(Counter::RequestsAllowed), 1);
    assert_eq!(rig.count(Counter::RequestsBlocked), 0);
}

#[tokio::test]
async fn a_request_block_stands_when_no_response_action_applies() {
    for rules in [
        "||site.test/replace/\n||site.test/replace/text$script,replace=/original/changed/\n",
        "||site.test/replace/\n||site.test/replace/text$app=com.other,replace=/original/changed/\n",
        "||site.test/replace/\n||site.test/replace/text$replace=/original/changed/\n@@||site.test/replace/text$replace\n",
    ] {
        let rig = rig(Setup {
            rules,
            ..Setup::default()
        })
        .await;
        let (status, body, _) = rig
            .get(Trust::System, H1, "/replace/text", None)
            .await
            .unwrap();
        assert_eq!((status, body.as_str()), (StatusCode::FORBIDDEN, ""));
        assert_eq!(rig.hits(), 0);
        assert_eq!(rig.count(Counter::RequestsBlocked), 1);
    }
}

#[tokio::test]
async fn a_page_is_replaced_before_the_cosmetic_style_is_inserted() {
    let rig = rig(Setup {
        rules: "site.test##.ad\n||site.test/page$replace=/identity/replaced/,document\n||site.test/page$replace=/display:none/changed/,document\n",
        ..Setup::default()
    }).await;
    let (_, status, body, _) = rig
        .get_response(Trust::System, H1, "/page", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<body>replaced</body>"));
    assert!(body.contains("<style>.ad{display:none!important}"));
}

const PRUNE_RULES: &str = r"||site.test/replace/json$jsonprune=\$.ad";

#[tokio::test]
async fn jsonprune_removes_items_from_a_decrypted_response() {
    for alpn in [H1, BOTH] {
        let rig = rig(Setup {
            rules: PRUNE_RULES,
            alpn,
            ..Setup::default()
        })
        .await;
        let (headers, status, body, _) = rig
            .get_response(Trust::System, alpn, "/replace/json", None)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"keep":"original"}"#);
        assert!(!headers.contains_key("etag"));
    }
}

#[tokio::test]
async fn a_pruning_action_does_not_override_a_request_block() {
    let rig = rig(Setup {
        rules: r"||site.test/replace/json
||site.test/replace/json$jsonprune=\$.ad",
        ..Setup::default()
    })
    .await;
    let (status, body, _) = rig
        .get(Trust::System, H1, "/replace/json", None)
        .await
        .unwrap();
    assert_eq!((status, body.as_str()), (StatusCode::FORBIDDEN, ""));
    assert_eq!(rig.hits(), 0);
    assert_eq!(rig.count(Counter::RequestsBlocked), 1);
}

#[tokio::test]
async fn the_decrypted_body_is_pruned_before_being_replaced() {
    let rig = rig(Setup {
        rules: r"||site.test/replace/json$jsonprune=\$.ad
||site.test/replace/$replace=/original/changed/
||site.test/replace/json$replace=/ad/advertisement/",
        ..Setup::default()
    })
    .await;
    let (status, body, _) = rig
        .get(Trust::System, H1, "/replace/json", None)
        .await
        .unwrap();
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, r#"{"keep":"changed"}"#)
    );
}

#[test]
fn only_replacement_actions_override_request_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join("rules");
    std::fs::create_dir(&rules).unwrap();
    let request = Request::builder().uri("/replace/json").body(()).unwrap();
    for (actions, blocked) in [
        (PRUNE_RULES, true),
        ("||site.test/replace/json$replace=/original/changed/", false),
    ] {
        std::fs::write(
            rules.join("user.txt"),
            format!("||site.test/replace/json\n{actions}\n"),
        )
        .unwrap();
        let filters = Filters::load(&[&rules], false).unwrap();
        assert_eq!(
            evaluate_request(
                &filters.for_app("com.app"),
                &request,
                "https://site.test/replace/json"
            )
            .blocked,
            blocked,
            "{actions}"
        );
    }
}

#[tokio::test]
async fn html_removes_elements_before_replacement_and_cosmetic_injection() {
    let rig = rig(Setup {
        rules: r#"site.test$$div[class="ad-box"]
site.test##title
||site.test/page$replace=/ad-box/kept/,document
||site.test/page$replace=/identity/replaced/,document
||site.test/page$replace=/display:none/changed/,document
"#,
        ..Setup::default()
    })
    .await;
    let (status, body, _) = rig
        .get(Trust::System, H1, "/page/named", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<head><style>title{display:none!important}"));
    assert!(body.contains("<body>replaced</body>"), "{body}");
}

#[tokio::test]
async fn html_filtering_does_not_request_identity_on_other_hosts() {
    let rig = rig(Setup {
        rules: "other.test$$div[class=ad-box]\n@@||site.test^$elemhide",
        ..Setup::default()
    })
    .await;
    let (status, body, _) = rig
        .get(Trust::System, H1, "/page/named", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<body>gzip, br<div class=\"ad-box\">"),
        "{body}"
    );
}

#[tokio::test]
async fn html_filtering_off_leaves_the_elements_and_the_request_encoding() {
    let rig = rig(Setup {
        rules: "site.test$$div[class=ad-box]",
        settings: Some("cosmetic off\n"),
        ..Setup::default()
    })
    .await;
    let (status, body, _) = rig
        .get(Trust::System, H1, "/page/named", None)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<body>gzip, br<div class=\"ad-box\">"),
        "{body}"
    );
}

#[tokio::test]
async fn an_assumed_request_answered_with_html_uses_its_own_host() {
    let rig = rig(Setup {
        rules: "site.test$$div[class=ad-box]",
        ..Setup::default()
    })
    .await;
    let (_, status, body, _) = rig
        .request_response(
            Trust::System,
            H1,
            "/page/named",
            Some("https://other.test/"),
            |mut req| {
                req.headers_mut().unwrap().remove("sec-fetch-dest");
                req
            },
        )
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<body>identity</body>"), "{body}");
}

#[tokio::test]
async fn a_subdocument_uses_its_own_html_rules_and_page_exceptions() {
    let rig = rig(Setup {
        rules: "site.test$$div[class=ad-box]\n@@||site.test/page/excepted$content\n@@||site.test^$elemhide",
        ..Setup::default()
    })
    .await;
    for (path, expected) in [
        ("/page/named", "<body>identity</body>"),
        (
            "/page/excepted/named",
            "<body>gzip, br<div class=\"ad-box\">",
        ),
    ] {
        let (_, status, body, _) = rig
            .request_response(
                Trust::System,
                H1,
                path,
                Some("https://other.test/"),
                |mut req| {
                    req.headers_mut().unwrap().insert(
                        "sec-fetch-dest",
                        hyper::header::HeaderValue::from_static("iframe"),
                    );
                    req
                },
            )
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(expected), "{body}");
    }
}

#[tokio::test]
async fn generic_html_rules_request_identity_only_for_a_known_document() {
    for rule in ["##^div[class=ad-box]", "~other.test$$div[class=ad-box]"] {
        let rig = rig(Setup {
            rules: rule,
            ..Setup::default()
        })
        .await;
        for (known, expected) in [
            (false, "<body>gzip, br<div class=\"ad-box\">"),
            (true, "<body>identity</body>"),
        ] {
            let (_, status, body, _) = rig
                .request_response(Trust::System, H1, "/page/named", None, |mut req| {
                    if !known {
                        req.headers_mut().unwrap().remove("sec-fetch-dest");
                    }
                    req
                })
                .await
                .unwrap();
            assert_eq!(status, StatusCode::OK);
            assert!(body.contains(expected), "{rule}: {body}");
        }
    }
}

type FixtureBody = http_body_util::combinators::BoxBody<Bytes, std::convert::Infallible>;

fn delayed_or_late_fixture(path: &str) -> Option<Response<FixtureBody>> {
    let (mime, body) = match path {
        "/creative/small" => ("image/png", Full::new(Bytes::from_static(b"small")).boxed()),
        "/creative/large" => (
            "image/png",
            Full::new(Bytes::from(vec![0; 4 << 20])).boxed(),
        ),
        "/creative/slow" => (
            "image/png",
            delayed_body(b"", b"image", Duration::from_millis(900)),
        ),
        "/page/delayed" => (
            "text/html",
            delayed_body(
                b"<html><head><title>original</title>",
                b"</head><body>keep</body></html>",
                Duration::from_millis(400),
            ),
        ),
        "/page/html" => (
            "APPLICATION/XHTML+XML",
            Full::new(Bytes::from_static(b"<script/>keep<script>ads=1</script>")).boxed(),
        ),
        "/page/plain" => (
            "text/plain",
            Full::new(Bytes::from_static(b"<script>ads=1</script>")).boxed(),
        ),
        "/page/json" => (
            "application/json",
            Full::new(Bytes::from_static(br#"{"data":"<script>ads=1</script>"}"#)).boxed(),
        ),
        _ => return None,
    };
    Some(
        Response::builder()
            .header("content-type", mime)
            .body(body)
            .unwrap(),
    )
}

fn delayed_body(first: &'static [u8], next: &'static [u8], delay: Duration) -> FixtureBody {
    use hyper::body::Frame;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    struct Frames {
        first: Option<Bytes>,
        next: Option<Bytes>,
        timer: Pin<Box<tokio::time::Sleep>>,
    }

    impl futures_core::Stream for Frames {
        type Item = Result<Frame<Bytes>, std::convert::Infallible>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            if let Some(first) = self.first.take() {
                return Poll::Ready(Some(Ok(Frame::data(first))));
            }
            if self.next.is_some() {
                std::task::ready!(self.timer.as_mut().poll(cx));
            }
            Poll::Ready(self.next.take().map(|next| Ok(Frame::data(next))))
        }
    }

    http_body_util::StreamBody::new(Frames {
        first: Some(Bytes::from_static(first)),
        next: Some(Bytes::from_static(next)),
        timer: Box::pin(tokio::time::sleep(delay)),
    })
    .boxed()
}

#[tokio::test]
async fn only_html_representations_are_edited_for_a_document_request() {
    let rig = rig(Setup {
        rules: "site.test##^script:has-text(ads=1)",
        ..Setup::default()
    })
    .await;
    for (path, expected) in [
        ("/page/plain", "<script>ads=1</script>"),
        ("/page/json", r#"{"data":"<script>ads=1</script>"}"#),
        ("/page/html", "<script/>keep"),
    ] {
        let (status, body, _) = rig.get(Trust::System, H1, path, None).await.unwrap();
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::OK, expected),
            "{path}"
        );
    }
}

#[tokio::test]
async fn scriptlets_wait_for_the_rewritten_body_but_keep_the_short_streaming_deadline() {
    for (rules, edit) in [
        (
            "site.test##+js(set, adbFixture, true)\nsite.test##.ad",
            false,
        ),
        (
            "site.test##+js(set, adbFixture, true)\nsite.test##.ad\n||site.test/page/delayed$replace=/original/edited/",
            true,
        ),
    ] {
        let rig = rig(Setup {
            rules,
            ..Setup::default()
        })
        .await;
        let (status, body, _) = rig
            .get(Trust::System, H1, "/page/delayed", None)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("display:none!important"), "{body}");
        assert_eq!(body.contains("adbFixture"), edit, "{body}");
        assert!(
            body.contains(if edit {
                "<title>edited</title>"
            } else {
                "<title>original</title>"
            }),
            "{body}"
        );
    }
}

fn followup(path: &str, h2: bool) -> Request<Empty<Bytes>> {
    Request::get(if h2 {
        format!("https://{HOST}{path}")
    } else {
        path.to_owned()
    })
    .header(HOST_HEADER, HOST)
    .body(Empty::new())
    .unwrap()
}

async fn h1_sender(rig: &Rig) -> hyper::client::conn::http1::SendRequest<Empty<Bytes>> {
    let tls = rig.handshake(Trust::System, H1).await.unwrap();
    let (tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(conn);
    tx
}

#[tokio::test]
async fn late_block_keeps_http1_reusable_or_explicitly_closes_it() {
    for size in ["small", "large", "slow"] {
        let rig = rig(Setup {
            rules: "||site.test/creative$image",
            ..Setup::default()
        })
        .await;
        let mut tx = h1_sender(&rig).await;
        let reply = tx
            .send_request(followup(&format!("/creative/{size}"), false))
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            reply.headers().contains_key("connection"),
            size != "small",
            "{size}"
        );
        assert!(
            reply
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty()
        );
        if size != "small" {
            tx = h1_sender(&rig).await;
        }
        for path in ["/next", "/third"] {
            tx.ready().await.unwrap();
            let reply = tx.send_request(followup(path, false)).await.unwrap();
            assert_eq!(reply.status(), StatusCode::OK);
            assert_eq!(
                reply.into_body().collect().await.unwrap().to_bytes(),
                format!("GET {path}")
            );
        }
        assert_eq!(rig.dials.load(Relaxed), if size == "small" { 1 } else { 2 });
    }
}

#[tokio::test]
async fn late_block_does_not_close_http2_for_small_large_or_slow_bodies() {
    let rig = rig(Setup {
        rules: "||site.test/creative$image",
        alpn: BOTH,
        ..Setup::default()
    })
    .await;
    let tls = rig.handshake(Trust::System, BOTH).await.unwrap();
    let (mut tx, conn) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
            .await
            .unwrap();
    tokio::spawn(conn);
    for size in ["small", "large", "slow"] {
        let reply = tx
            .send_request(followup(&format!("/creative/{size}"), true))
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::FORBIDDEN);
        assert!(!reply.headers().contains_key("connection"));
        assert!(
            reply
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty()
        );
        for path in ["/next", "/third"] {
            let reply = tx.send_request(followup(path, true)).await.unwrap();
            assert_eq!(reply.status(), StatusCode::OK);
            assert_eq!(
                reply.into_body().collect().await.unwrap().to_bytes(),
                format!("GET {path}")
            );
        }
    }
    assert_eq!(rig.dials.load(Relaxed), 1);
}
