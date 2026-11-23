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

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use tokio::net::TcpListener;

use super::*;

const APP: u32 = 10001;
const PAUSED: u32 = 10002;

/// The system as a test has it: an upstream server on loopback stands for
/// every server, and answers sent to the app are kept.
struct Fake {
    active: BTreeSet<u32>,
    owner: Option<(u32, Option<u32>)>,
    local: bool,
    upstream: SocketAddr,
    dialed: AtomicUsize,
    replies: Mutex<Vec<(SocketAddr, SocketAddr, Vec<u8>)>>,
    /// Whether another program holds the servers' port.
    port_held: bool,
    gave_up: AtomicUsize,
}

impl Fake {
    fn new(upstream: SocketAddr) -> Self {
        Self {
            active: BTreeSet::from([APP]),
            owner: Some((APP, Some(0xf0065))),
            local: false,
            upstream,
            dialed: AtomicUsize::new(0),
            replies: Mutex::default(),
            port_held: false,
            gave_up: AtomicUsize::new(0),
        }
    }

    fn replies(&self) -> Vec<(SocketAddr, SocketAddr, Vec<u8>)> {
        std::mem::take(&mut self.replies.lock().unwrap())
    }
}

impl Host for Fake {
    fn blocked(&self, uid: u32, query: &[u8]) -> Option<Vec<u8>> {
        let question = dns::Question::parse(query, false)?;
        (self.active.contains(&uid) && question.host()?.ends_with("ads.example"))
            .then(|| question.local_answer(0, false))
    }

    fn udp_owner(&self, _: SocketAddr, _: SocketAddr) -> Option<(u32, Option<u32>)> {
        self.owner
    }

    fn tcp_owner(&self, _: SocketAddr, _: SocketAddr) -> Option<u32> {
        self.owner.map(|(uid, _)| uid)
    }

    fn is_local(&self, _: IpAddr) -> bool {
        self.local
    }

    fn udp(&self, _: &Origin) -> io::Result<UdpSocket> {
        self.dialed.fetch_add(1, Relaxed);
        let socket = std::net::UdpSocket::bind("127.0.0.1:0")?;
        socket.connect(self.upstream)?;
        socket.set_nonblocking(true)?;
        UdpSocket::from_std(socket)
    }

    fn tcp(&self, _: Origin) -> Pin<Box<dyn Future<Output = io::Result<TcpStream>> + Send>> {
        self.dialed.fetch_add(1, Relaxed);
        Box::pin(TcpStream::connect(self.upstream))
    }

    fn reply(&self, server: SocketAddr, client: SocketAddr, answer: &[u8]) -> Result<(), Unsent> {
        if self.port_held {
            return Err(Unsent::Never(io::ErrorKind::AddrInUse.into()));
        }
        let reply = (server, client, answer.to_vec());
        self.replies.lock().unwrap().push(reply);
        Ok(())
    }

    fn give_up(&self, _: &io::Error) {
        self.gave_up.fetch_add(1, Relaxed);
    }

    fn verbose(&self) -> bool {
        false
    }
}

fn query(name: &str, kind: u16) -> Vec<u8> {
    let mut bytes = vec![0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 1];
    for label in name.split('.') {
        bytes.push(label.len() as u8);
        bytes.extend_from_slice(label.as_bytes());
    }
    bytes.extend_from_slice(&[0, (kind >> 8) as u8, kind as u8, 0, 1]);
    // An OPT record asking for large answers, which is sent on as it is.
    bytes.extend_from_slice(&[0, 0, 41, 0x10, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}

/// What a server answers: the query, as a response, made `extra` longer.
fn answer_to(query: &[u8], extra: usize) -> Vec<u8> {
    let mut answer = query.to_vec();
    answer[2] |= 0x80;
    answer.resize(query.len() + extra, 0);
    answer
}

const CLIENT: &str = "127.0.0.1:40000";
const SERVER: &str = "192.0.2.53:53";

/// A server that answers each query it gets first with one that answers
/// something else, then with `extra` more bytes than it was asked.
async fn udp_server(extra: usize) -> (SocketAddr, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = server.local_addr().unwrap();
    let serving = tokio::spawn(async move {
        let mut got = Vec::new();
        let mut buffer = vec![0; MAX_DATAGRAM];
        while let Ok(Ok((n, peer))) =
            timeout(Duration::from_millis(500), server.recv_from(&mut buffer)).await
        {
            let query = buffer[..n].to_vec();
            let mut stray = answer_to(&query, 0);
            stray[1] ^= 1;
            server.send_to(&stray, peer).await.unwrap();
            server
                .send_to(&answer_to(&query, extra), peer)
                .await
                .unwrap();
            got.push(query);
        }
        got
    });
    (addr, serving)
}

async fn look_up(host: &Arc<Fake>, query: &[u8]) {
    let (client, server) = (CLIENT.parse().unwrap(), SERVER.parse().unwrap());
    datagram(
        host.clone(),
        Slots::pending(),
        query.to_vec(),
        client,
        server,
    )
    .await;
}

#[tokio::test]
async fn a_blocked_lookup_is_answered_here_with_no_lifetime_and_nothing_is_sent_on() {
    let (upstream, serving) = udp_server(0).await;
    let host = Arc::new(Fake::new(upstream));
    let asked = query("tracker.ads.example", 1);
    look_up(&host, &asked).await;
    let replies = host.replies();
    assert_eq!(replies.len(), 1);
    let (from, to, answer) = &replies[0];
    assert_eq!(
        (*from, *to),
        (SERVER.parse().unwrap(), CLIENT.parse().unwrap())
    );
    let question = dns::Question::parse(&asked, false).unwrap();
    assert_eq!(*answer, question.local_answer(0, false));
    assert_eq!(host.dialed.load(Relaxed), 0);
    assert!(serving.await.unwrap().is_empty());
}

#[tokio::test]
async fn an_allowed_lookup_is_sent_on_unchanged_and_a_large_answer_is_relayed_whole() {
    let (upstream, serving) = udp_server(3000).await;
    let host = Arc::new(Fake::new(upstream));
    let asked = query("example.org", 28);
    look_up(&host, &asked).await;
    let replies = host.replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].2, answer_to(&asked, 3000));
    assert_eq!(serving.await.unwrap(), [asked]);
}

#[tokio::test]
async fn a_lookup_of_an_app_no_longer_active_is_sent_on_even_for_a_blocked_name() {
    let (upstream, serving) = udp_server(0).await;
    let mut fake = Fake::new(upstream);
    fake.owner = Some((PAUSED, None));
    let host = Arc::new(fake);
    let asked = query("tracker.ads.example", 1);
    look_up(&host, &asked).await;
    assert_eq!(host.replies()[0].2, answer_to(&asked, 0));
    assert_eq!(serving.await.unwrap(), [asked]);
}

#[tokio::test]
async fn a_lookup_that_cannot_be_read_is_sent_on_and_its_answer_known_by_its_id() {
    let (upstream, serving) = udp_server(0).await;
    let host = Arc::new(Fake::new(upstream));
    let mut asked = query("tracker.ads.example", 1);
    asked[5] = 2;
    look_up(&host, &asked).await;
    assert_eq!(host.replies()[0].2, answer_to(&asked, 0));
    assert_eq!(serving.await.unwrap(), [asked]);
}

#[tokio::test]
async fn a_lookup_of_nobody_of_the_system_or_to_this_device_is_dropped_unanswered() {
    let (upstream, serving) = udp_server(0).await;
    for (owner, local) in [
        (None, false),
        (Some((1000, None)), false),
        (Some((APP, None)), true),
    ] {
        let mut fake = Fake::new(upstream);
        fake.owner = owner;
        fake.local = local;
        let host = Arc::new(fake);
        look_up(&host, &query("tracker.ads.example", 1)).await;
        look_up(&host, &query("example.org", 1)).await;
        assert!(host.replies().is_empty());
        assert_eq!(host.dialed.load(Relaxed), 0);
    }
    assert!(serving.await.unwrap().is_empty());
}

#[tokio::test]
async fn lookups_are_given_up_when_no_answer_can_be_sent_as_the_server() {
    let (upstream, serving) = udp_server(0).await;
    let mut fake = Fake::new(upstream);
    fake.port_held = true;
    let host = Arc::new(fake);
    look_up(&host, &query("example.org", 1)).await;
    look_up(&host, &query("tracker.ads.example", 1)).await;
    assert!(host.replies().is_empty());
    assert_eq!(host.gave_up.load(Relaxed), 2);
    assert_eq!(serving.await.unwrap().len(), 1);
}

#[test]
fn only_what_came_by_our_route_to_port_53_elsewhere_is_served() {
    let route =
        |client: &str, server: &str| by_our_route(client.parse().unwrap(), server.parse().unwrap());
    assert!(route(CLIENT, SERVER));
    assert!(route(
        &format!("[{SOURCE_V6}]:40000"),
        "[2001:4860:4860::8888]:53"
    ));
    // Another source: sent to the listener some other way.
    assert!(!route("10.0.2.16:40000", SERVER));
    assert!(!route("[::1]:40000", "[2001:4860:4860::8888]:53"));
    // To this device, or to nowhere in particular.
    for server in [
        "127.0.0.53:53",
        "0.0.0.0:53",
        "255.255.255.255:53",
        "224.0.0.251:53",
    ] {
        assert!(!route(CLIENT, server), "{server}");
    }
    assert!(!route(
        &format!("[{SOURCE_V6}]:40000"),
        &format!("[{SOURCE_V6}]:53")
    ));
    assert!(!route(CLIENT, "192.0.2.53:5353"));
}

#[test]
fn one_app_cannot_take_every_place_and_places_come_back() {
    let slots = Slots::new(3, 2);
    let first = slots.take_for(APP).unwrap();
    let _second = slots.take_for(APP).unwrap();
    assert!(slots.take_for(APP).is_none());
    let other = slots.take_for(PAUSED);
    assert!(other.is_some());
    drop(first);
    assert!(slots.take_for(APP).is_some());

    let all: Vec<_> = (0..3).map(|_| slots.take().unwrap()).collect();
    assert!(slots.take().is_none());
    drop(all);
    assert!(slots.take().is_some());
    drop(other);
    assert!(!slots.taken.lock().unwrap().1.contains_key(&PAUSED));
}

/// A server over TCP that answers every query of every connection, and
/// counts the connections.
async fn tcp_server() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counted = connections.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counted.fetch_add(1, Relaxed);
            tokio::spawn(async move {
                while let Ok(query) = dns::read_frame(&mut stream).await {
                    let answer = answer_to(&query, 1000);
                    dns::write_frame(&mut stream, &answer).await.unwrap();
                }
            });
        }
    });
    (addr, connections)
}

/// The app's end of a connection, and ours.
async fn connection() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    (app, listener.accept().await.unwrap().0)
}

fn origin() -> Origin {
    Origin::of(SERVER.parse().unwrap(), 0, Some(APP)).unwrap()
}

#[tokio::test]
async fn queries_share_one_connection_in_turn_and_a_blocked_one_is_answered_here() {
    let (upstream, connections) = tcp_server().await;
    let host = Arc::new(Fake::new(upstream));
    let (mut app, ours) = connection().await;
    let conversing = tokio::spawn({
        let host = host.clone();
        async move { converse(&*host, &origin(), ours).await }
    });
    let (first, blocked, second) = (
        query("example.org", 1),
        query("tracker.ads.example", 65),
        query("example.com", 28),
    );
    for asked in [&first, &blocked, &second] {
        dns::write_frame(&mut app, asked).await.unwrap();
    }
    assert_eq!(
        dns::read_frame(&mut app).await.unwrap(),
        answer_to(&first, 1000)
    );
    let question = dns::Question::parse(&blocked, false).unwrap();
    assert_eq!(
        dns::read_frame(&mut app).await.unwrap(),
        question.local_answer(0, false)
    );
    assert_eq!(
        dns::read_frame(&mut app).await.unwrap(),
        answer_to(&second, 1000)
    );
    drop(app);
    assert!(conversing.await.unwrap().is_err());
    assert_eq!(connections.load(Relaxed), 1);
    assert_eq!(host.dialed.load(Relaxed), 1);
}

#[tokio::test]
async fn a_query_longer_than_any_lookup_ends_the_connection() {
    let (upstream, connections) = tcp_server().await;
    let host = Arc::new(Fake::new(upstream));
    let (mut app, ours) = connection().await;
    dns::write_frame(&mut app, &vec![0; MAX_QUERY + 1])
        .await
        .unwrap();
    let ended = converse(&*host, &origin(), ours).await;
    assert_eq!(ended.unwrap_err().kind(), io::ErrorKind::InvalidData);
    assert_eq!(connections.load(Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn an_idle_connection_is_let_go() {
    let (upstream, _) = tcp_server().await;
    let host = Arc::new(Fake::new(upstream));
    let (_app, ours) = connection().await;
    let ended = converse(&*host, &origin(), ours).await;
    assert_eq!(ended.unwrap_err().kind(), io::ErrorKind::TimedOut);
}
