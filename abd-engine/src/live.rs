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

//! What happens to the filtered apps' connections, told as it happens to
//! `abd watch` and to nothing else.
//!
//! This is the only place the daemon hands what the apps talk to to
//! anyone: to a process of root's that asked, over a socket in the data
//! directory, which only root can reach. Nothing of it is written or
//! logged. While nobody watches, a connection pays one atomic load for it.
//!
//! Each watcher has a queue of its own, of [`QUEUE`] events. A watcher
//! that falls behind loses events and is told how many; a connection never
//! waits for it.

use std::io::{self, BufRead as _, Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::protocol_types::{Dropped, Event, Reason, Seen};

/// The most watchers at a time: the app's screen, and a root shell.
pub const MAX_WATCHERS: usize = 2;
/// The events a watcher may fall behind by before it loses some.
pub const QUEUE: usize = 256;
/// What is told of an address or a page at most: an address can be as
/// long as the app likes.
const MAX_URL: usize = 2048;
/// Events written in one go.
const BATCH: usize = 64;
/// How long a watcher may take to read what it is sent, and to say what
/// it watches. Longer, it is let go.
const WRITE_TIME: Duration = Duration::from_secs(30);
const HELLO_TIME: Duration = Duration::from_secs(5);
const MAX_HELLO: u64 = 512;
/// Connections to the socket served at a time, watchers included.
const MAX_CONNECTIONS: usize = 8;
/// The longest line `abd watch` passes on.
const MAX_LINE: u64 = 64 << 10;

struct Watcher {
    id: u64,
    /// Only this app's, if any.
    app: Option<String>,
    queue: mpsc::Sender<Arc<Event>>,
    dropped: Arc<AtomicU64>,
}

pub struct Hub {
    /// How many watch: what a connection looks at, without a lock.
    watching: AtomicUsize,
    watchers: Mutex<Vec<Watcher>>,
    capacity: usize,
    ids: AtomicU64,
    /// Milliseconds since the epoch.
    clock: fn() -> u64,
}

/// A watcher, until dropped.
pub struct Joined {
    hub: Arc<Hub>,
    id: u64,
    events: mpsc::Receiver<Arc<Event>>,
    dropped: Arc<AtomicU64>,
}

impl Drop for Joined {
    fn drop(&mut self) {
        let mut watchers = self.hub.watchers.lock().unwrap();
        watchers.retain(|w| w.id != self.id);
        self.hub.watching.store(watchers.len(), Relaxed);
    }
}

/// At most `max` bytes of `s`, cut where a character begins.
fn cut(s: &str, max: usize) -> String {
    s[..s.floor_char_boundary(max)].to_owned()
}

impl Hub {
    pub fn new() -> Arc<Self> {
        Self::with(QUEUE, crate::clock::now_ms)
    }

    pub fn with(capacity: usize, clock: fn() -> u64) -> Arc<Self> {
        Arc::new(Self {
            watching: AtomicUsize::new(0),
            watchers: Mutex::default(),
            capacity,
            ids: AtomicU64::new(0),
            clock,
        })
    }

    /// A request in a decrypted connection, blocked or let through.
    pub fn request(
        &self,
        app: &str,
        host: &str,
        blocked: bool,
        url: &str,
        page: Option<&str>,
        kind: &'static str,
    ) {
        self.send(app, |time| Event {
            time,
            app: app.to_owned(),
            verdict: if blocked {
                Seen::Blocked
            } else {
                Seen::Allowed
            },
            host: host.to_owned(),
            url: Some(cut(url, MAX_URL)),
            page: page.map(|p| cut(p, MAX_URL)),
            kind: Some(kind),
            reason: None,
        });
    }

    /// A connection refused for its host.
    pub fn host_blocked(&self, app: &str, host: impl std::fmt::Display) {
        self.connection(app, host, Seen::HostBlocked, None);
    }

    /// A connection handed on unread.
    pub fn passed(&self, app: &str, host: &str, reason: Reason) {
        self.connection(app, host, Seen::Passed, Some(reason));
    }

    pub(crate) fn address_passed(&self, app: &str, endpoint: std::net::SocketAddr) {
        self.connection(app, endpoint, Seen::Passed, None);
    }

    fn connection(
        &self,
        app: &str,
        host: impl std::fmt::Display,
        verdict: Seen,
        reason: Option<Reason>,
    ) {
        self.send(app, |time| Event {
            time,
            app: app.to_owned(),
            verdict,
            host: host.to_string(),
            url: None,
            page: None,
            kind: None,
            reason,
        });
    }

    /// Gives the event `make` makes to every watcher of `app`; made only
    /// if there is one.
    fn send(&self, app: &str, make: impl FnOnce(u64) -> Event) {
        if self.watching.load(Relaxed) == 0 {
            return;
        }
        let watchers = self.watchers.lock().unwrap();
        let mut make = Some(make);
        let mut event: Option<Arc<Event>> = None;
        for w in watchers.iter() {
            if w.app.as_deref().is_some_and(|a| a != app) {
                continue;
            }
            let event = event.get_or_insert_with(|| {
                let make = make.take().expect("made once");
                Arc::new(make((self.clock)()))
            });
            if let Err(mpsc::error::TrySendError::Full(_)) = w.queue.try_send(event.clone()) {
                w.dropped.fetch_add(1, Relaxed);
            }
        }
    }

    /// A new watcher, of `app`'s connections or of all; none if there are
    /// [`MAX_WATCHERS`] already.
    pub fn join(self: &Arc<Self>, app: Option<String>) -> Option<Joined> {
        let mut watchers = self.watchers.lock().unwrap();
        if watchers.len() >= MAX_WATCHERS {
            return None;
        }
        let (queue, events) = mpsc::channel(self.capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let id = self.ids.fetch_add(1, Relaxed);
        watchers.push(Watcher {
            id,
            app,
            queue,
            dropped: dropped.clone(),
        });
        self.watching.store(watchers.len(), Relaxed);
        Some(Joined {
            hub: self.clone(),
            id,
            events,
            dropped,
        })
    }
}

fn line(batch: &mut Vec<u8>, value: &impl Serialize) {
    if serde_json::to_writer(&mut *batch, value).is_ok() {
        batch.push(b'\n');
    }
}

/// Writes what `joined` is told to `out`, an event a line, until writing
/// fails or takes longer than [`WRITE_TIME`].
pub async fn feed(joined: &mut Joined, out: &mut (impl AsyncWrite + Unpin)) -> io::Result<()> {
    let mut batch = Vec::new();
    while let Some(event) = joined.events.recv().await {
        batch.clear();
        line(&mut batch, &*event);
        for _ in 1..BATCH {
            match joined.events.try_recv() {
                Ok(event) => line(&mut batch, &*event),
                Err(_) => break,
            }
        }
        // Once what was queued is out: events were left out after it.
        if joined.events.is_empty() {
            let dropped = joined.dropped.swap(0, Relaxed);
            if dropped > 0 {
                line(&mut batch, &Dropped { dropped });
            }
        }
        tokio::time::timeout(WRITE_TIME, out.write_all(&batch))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    }
    Ok(())
}

/// Listens on `path` for `abd watch`. Only a process of the user the
/// daemon runs as, root, can connect: the socket is that user's alone, in
/// a directory that is too.
pub fn listen(path: &Path, hub: Arc<Hub>) -> io::Result<()> {
    crate::files::remove_if_present(path)?;
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    // Ours, as we made it.
    let owner = std::fs::metadata(path)?.uid();
    let serving = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                // Out of descriptors, most likely.
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            if serving.fetch_add(1, Relaxed) >= MAX_CONNECTIONS {
                serving.fetch_sub(1, Relaxed);
                continue;
            }
            let (hub, serving) = (hub.clone(), serving.clone());
            tokio::spawn(async move {
                serve(hub, stream, owner).await;
                serving.fetch_sub(1, Relaxed);
            });
        }
    });
    Ok(())
}

/// One connection to the socket. It says what it watches in a line, `all`
/// or `app <package>`, and is answered `ok`, then the events, or `busy`.
async fn serve(hub: Arc<Hub>, stream: UnixStream, owner: u32) {
    if !stream.peer_cred().is_ok_and(|c| c.uid() == owner) {
        return;
    }
    let (read, mut write) = stream.into_split();
    let mut read = tokio::io::BufReader::new(read);
    let mut hello = String::new();
    let asked = tokio::time::timeout(
        HELLO_TIME,
        (&mut read).take(MAX_HELLO).read_line(&mut hello),
    )
    .await;
    if !matches!(asked, Ok(Ok(n)) if n > 0) {
        return;
    }
    let app = match hello.trim_end().split_once(' ') {
        None if hello.trim_end() == "all" => None,
        Some(("app", key)) if crate::apps::Identity::parse(key).is_some() => Some(key.to_owned()),
        _ => return,
    };
    let Some(mut joined) = hub.join(app) else {
        let _ = tokio::time::timeout(WRITE_TIME, write.write_all(b"busy\n")).await;
        return;
    };
    if !matches!(
        tokio::time::timeout(WRITE_TIME, write.write_all(b"ok\n")).await,
        Ok(Ok(()))
    ) {
        return;
    }
    let mut rest = [0u8; 64];
    tokio::select! {
        _ = feed(&mut joined, &mut write) => {}
        // A watcher says nothing more: anything, or the end, is its leaving.
        _ = read.read(&mut rest) => {}
    }
}

/// `abd watch`: passes on what the daemon tells, a line at a time, until
/// standard input ends or standard output is closed.
///
/// `running` is whether the daemon runs, as whoever asks found out: with
/// nobody listening at `path` it says which of the two is the matter.
pub fn watch(path: &Path, app: Option<&str>, running: bool) -> Result<()> {
    let stream = std::os::unix::net::UnixStream::connect(path).context(if running {
        "the daemon runs, but could not open what it tells this on: see its log"
    } else {
        "the daemon is not running"
    })?;
    let mut from = join(stream, app)?;
    std::thread::spawn(|| {
        let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
        std::process::exit(0);
    });
    copy_lines(&mut from, &mut io::stdout().lock())
}

/// Says what is watched on `stream`, and reads the answer.
fn join(
    mut stream: std::os::unix::net::UnixStream,
    app: Option<&str>,
) -> Result<io::BufReader<std::os::unix::net::UnixStream>> {
    stream.set_read_timeout(Some(HELLO_TIME))?;
    let hello = match app {
        Some(app) => format!("app {app}\n"),
        None => "all\n".to_owned(),
    };
    stream.write_all(hello.as_bytes())?;
    let mut from = io::BufReader::new(stream);
    let mut answer = String::new();
    from.by_ref().take(MAX_HELLO).read_line(&mut answer)?;
    match answer.trim_end() {
        "ok" => {}
        "busy" => bail!("watched {MAX_WATCHERS} times already"),
        _ => bail!("the daemon did not answer"),
    }
    from.get_ref().set_read_timeout(None)?;
    Ok(from)
}

/// Passes lines on from `from` to `to` until `to` is closed. The end of
/// `from` is the daemon's: it stopped, or let go of a reader that read
/// nothing for `WRITE_TIME`.
fn copy_lines(from: &mut impl io::BufRead, to: &mut impl io::Write) -> Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if from.by_ref().take(MAX_LINE).read_until(b'\n', &mut line)? == 0 {
            bail!("the daemon ended the watch: it stopped, or what it sent was not read");
        }
        if to.write_all(&line).and_then(|()| to.flush()).is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passed(hub: &Hub, app: &str, n: usize) {
        for i in 0..n {
            hub.passed(app, &format!("h{i}.example"), Reason::Pinned);
        }
    }

    fn lines(out: &[u8]) -> Vec<serde_json::Value> {
        out.split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).unwrap())
            .collect()
    }

    #[test]
    fn nothing_is_made_while_nobody_watches() {
        let hub = Hub::new();
        hub.send("com.app", |_| panic!("an event was made for nobody"));
        let watcher = hub.join(None).unwrap();
        drop(watcher);
        hub.send("com.app", |_| {
            panic!("an event was made for a watcher gone")
        });
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_watcher_loses_events_and_is_told_how_many_and_nothing_waits_for_it() {
        let hub = Hub::with(8, || 1);
        let mut watcher = hub.join(None).unwrap();
        // Nobody reads meanwhile: every send returns at once all the same.
        let started = std::time::Instant::now();
        passed(&hub, "com.app", 10_000);
        assert!(started.elapsed() < Duration::from_secs(1));
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(1), feed(&mut watcher, &mut out)).await;
        let got = lines(&out);
        assert_eq!(got.len(), 9);
        assert_eq!(got[0]["host"], "h0.example");
        assert_eq!(got[8], serde_json::json!({ "dropped": 9992 }));
    }

    #[tokio::test(start_paused = true)]
    async fn a_watcher_of_one_app_is_told_of_that_app_only() {
        let hub = Hub::with(8, || 1);
        let mut watcher = hub.join(Some("com.b".to_owned())).unwrap();
        passed(&hub, "com.a", 3);
        passed(&hub, "com.b", 1);
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(1), feed(&mut watcher, &mut out)).await;
        let got = lines(&out);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["app"], "com.b");
    }

    #[test]
    fn no_more_than_a_few_watch_at_a_time() {
        let hub = Hub::new();
        let first = hub.join(None).unwrap();
        let _second = hub.join(None).unwrap();
        assert!(hub.join(None).is_none());
        drop(first);
        assert!(hub.join(None).is_some());
    }

    #[test]
    fn an_address_told_is_cut_where_a_character_begins() {
        assert_eq!(cut("abc", 2), "ab");
        assert_eq!(cut("aé", 2), "a");
        assert_eq!(cut("a", 9), "a");
    }

    #[tokio::test]
    async fn a_watcher_on_the_socket_is_told_and_a_reader_that_never_reads_holds_nothing_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("watch.sock");
        let hub = Hub::with(QUEUE, || 1);
        listen(&path, hub.clone()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        let mut client = UnixStream::connect(&path).await.unwrap();
        client.write_all(b"all\n").await.unwrap();
        let mut answer = [0u8; 3];
        client.read_exact(&mut answer).await.unwrap();
        assert_eq!(&answer, b"ok\n");
        // Without yielding: the daemon's writer has had no chance to run.
        passed(&hub, "com.app", 100_000);
        let mut read = tokio::io::BufReader::new(client);
        let mut got = Vec::new();
        loop {
            let mut l = String::new();
            read.read_line(&mut l).await.unwrap();
            got.push(serde_json::from_str::<serde_json::Value>(&l).unwrap());
            if got.last().unwrap().get("dropped").is_some() {
                break;
            }
        }
        assert_eq!(got.len(), QUEUE + 1);
        assert_eq!(got[QUEUE]["dropped"], 100_000 - QUEUE as u64);

        // Gone: its place is free.
        drop(read);
        for _ in 0..100 {
            if hub.watching.load(Relaxed) == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(hub.watching.load(Relaxed), 0);
    }

    #[tokio::test]
    async fn abd_watch_passes_only_the_selected_work_groups_events_until_the_daemon_stops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("watch.sock");
        let hub = Hub::with(QUEUE, || 7);
        listen(&path, hub.clone()).unwrap();
        let client = {
            let path = path.clone();
            tokio::task::spawn_blocking(move || {
                let stream = std::os::unix::net::UnixStream::connect(path).unwrap();
                let mut from = join(stream, Some("37:@com.shared")).unwrap();
                let mut out = Vec::new();
                let ended = copy_lines(&mut from, &mut out);
                (out, ended)
            })
        };
        while hub.watching.load(Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        passed(&hub, "@com.shared", 1);
        hub.host_blocked("37:@com.shared", "ads.example");
        // The daemon gone: the client's line ends.
        tokio::time::sleep(Duration::from_millis(50)).await;
        hub.watchers.lock().unwrap().clear();
        let (out, ended) = client.await.unwrap();
        assert!(ended.is_err());
        assert_eq!(
            lines(&out),
            [serde_json::json!({
                "time": 7, "app": "37:@com.shared", "verdict": "host_blocked", "host": "ads.example",
                "url": null, "page": null, "kind": null, "reason": null
            })]
        );
    }

    #[test]
    fn abd_watch_stops_when_its_reader_is_gone() {
        struct Closed;
        impl io::Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut from = io::Cursor::new(b"{}\n{}\n".to_vec());
        assert!(copy_lines(&mut from, &mut Closed).is_ok());
    }

    #[test]
    fn with_nobody_to_watch_it_says_whether_the_daemon_is_the_matter() {
        let dir = tempfile::tempdir().unwrap();
        let nobody = dir.path().join("watch.sock");
        let said = |running| watch(&nobody, None, running).unwrap_err().to_string();
        assert_eq!(said(false), "the daemon is not running");
        // Running, and not listening: not sent to start what runs.
        assert!(said(true).starts_with("the daemon runs"));
    }
}
