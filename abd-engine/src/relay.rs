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

//! Joins two connections, for one that is handed on untouched.
//!
//! The bytes are moved by the kernel, from one socket to the other through
//! a pipe (`splice`), and never copied into this process and out again.
//! What is handed on is what the daemon cannot or must not read, which is
//! often the bulk of an app's traffic: the video of an app that pins.

#![allow(unsafe_code, reason = "pipe2 and splice")]

use std::io;
use std::net::Shutdown;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::io::Interest;
use tokio::net::TcpStream;

/// The most moved at a time: what a pipe holds unless made larger.
const CHUNK: usize = 64 * 1024;

/// A pipe, as the kernel's buffer between two sockets.
struct Pipe {
    read: OwnedFd,
    write: OwnedFd,
}

impl Pipe {
    fn new() -> io::Result<Self> {
        let mut fds = [0; 2];
        // SAFETY: `fds` is the two integers pipe2 writes.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both were just opened, and are owned by nothing else.
        unsafe {
            Ok(Self {
                read: OwnedFd::from_raw_fd(fds[0]),
                write: OwnedFd::from_raw_fd(fds[1]),
            })
        }
    }
}

/// Moves up to `len` bytes from one descriptor to another, one of them a
/// pipe, without waiting.
fn splice(from: RawFd, to: RawFd, len: usize) -> io::Result<usize> {
    // SAFETY: a system call on two descriptors the callers hold open; no
    // offsets are passed, as neither is a file.
    let n = unsafe {
        libc::splice(
            from,
            std::ptr::null_mut(),
            to,
            std::ptr::null_mut(),
            len,
            libc::SPLICE_F_MOVE | libc::SPLICE_F_NONBLOCK,
        )
    };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

/// Moves everything `from` sends to `to`, until `from` has sent its last,
/// and then tells `to` so.
async fn one_way(from: &TcpStream, to: &TcpStream, pipe: Pipe) -> io::Result<()> {
    loop {
        let moved = loop {
            from.readable().await?;
            // The pipe is empty here, so this waits on the socket alone.
            let into_pipe = || splice(from.as_raw_fd(), pipe.write.as_raw_fd(), CHUNK);
            match from.try_io(Interest::READABLE, into_pipe) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                result => break result?,
            }
        };
        if moved == 0 {
            break;
        }
        // All of it out again before more comes in.
        let mut left = moved;
        while left > 0 {
            to.writable().await?;
            let out_of_pipe = || splice(pipe.read.as_raw_fd(), to.as_raw_fd(), left);
            match to.try_io(Interest::WRITABLE, out_of_pipe) {
                Ok(n) => left -= n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
        }
    }
    socket2::SockRef::from(to).shutdown(Shutdown::Write)
}

/// Whether the kernel moves bytes from a TCP socket to a pipe and on to
/// another, found out once by having it move one. Every Android kernel
/// should; one that does not would have every connection handed on fail,
/// and the apps that pin lose their network, so it is not assumed.
pub fn kernel_splices() -> bool {
    static WORKS: OnceLock<bool> = OnceLock::new();
    *WORKS.get_or_init(|| {
        let moved = || -> io::Result<bool> {
            use std::io::{Read as _, Write as _};
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
            let mut from = std::net::TcpStream::connect(listener.local_addr()?)?;
            let (mid, _) = listener.accept()?;
            let out = std::net::TcpStream::connect(listener.local_addr()?)?;
            let (mut to, _) = listener.accept()?;
            let pipe = Pipe::new()?;
            from.write_all(b"s")?;
            // Arrived before it is asked for: this is the loopback
            // interface, but the wait is bounded all the same.
            mid.set_read_timeout(Some(Duration::from_secs(1)))?;
            mid.peek(&mut [0u8; 1])?;
            if splice(mid.as_raw_fd(), pipe.write.as_raw_fd(), 1)? != 1
                || splice(pipe.read.as_raw_fd(), out.as_raw_fd(), 1)? != 1
            {
                return Ok(false);
            }
            to.set_read_timeout(Some(Duration::from_secs(1)))?;
            let mut got = [0u8; 1];
            to.read_exact(&mut got)?;
            Ok(got == *b"s")
        };
        let works = moved().unwrap_or(false);
        if !works {
            eprintln!("relay: the kernel does not splice here; copying instead");
        }
        works
    })
}

/// Joins `a` and `b` until both have ended, or until `linger` after the
/// first has: a peer that never closes must not keep the connection of one
/// long gone.
pub async fn join(a: TcpStream, b: TcpStream, linger: Duration) {
    let pipes = if kernel_splices() {
        Pipe::new().and_then(|there| Ok((there, Pipe::new()?)))
    } else {
        Err(io::ErrorKind::Unsupported.into())
    };
    join_with_pipes(a, b, linger, pipes).await;
}

async fn join_with_pipes(
    a: TcpStream,
    b: TcpStream,
    linger: Duration,
    pipes: io::Result<(Pipe, Pipe)>,
) {
    let Ok((there, back)) = pipes else {
        // No splicing, or no descriptors to spare for pipes: the bytes are
        // copied.
        join_streams(a, b, linger).await;
        return;
    };
    let there = one_way(&a, &b, there);
    let back = one_way(&b, &a, back);
    tokio::pin!(there, back);
    tokio::select! {
        _ = &mut there => { let _ = tokio::time::timeout(linger, back).await; }
        _ = &mut back => { let _ = tokio::time::timeout(linger, there).await; }
    }
}

/// Like the socket relay, but the HTTP implementation may still hold bytes
/// read ahead of an upgrade. Take them from the upgraded stream first.
pub(crate) async fn join_streams(
    a: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    b: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    linger: Duration,
) {
    use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
    async fn copy(mut from: impl AsyncRead + Unpin, mut to: impl AsyncWrite + Unpin) {
        let _ = tokio::io::copy(&mut from, &mut to).await;
        let _ = to.shutdown().await;
    }
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    let there = copy(ar, bw);
    let back = copy(br, aw);
    tokio::pin!(there, back);
    tokio::select! {
        _ = &mut there => { let _ = tokio::time::timeout(linger, back).await; }
        _ = &mut back => { let _ = tokio::time::timeout(linger, there).await; }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// Two connections to this process, joined: what is written to the
    /// first returned comes out of the second.
    async fn joined(linger: Duration, splice: bool) -> (TcpStream, TcpStream) {
        async fn pair() -> (TcpStream, TcpStream) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let near = TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap();
            (near, listener.accept().await.unwrap().0)
        }
        let ((app, a), (server, b)) = (pair().await, pair().await);
        if splice {
            tokio::spawn(join(a, b, linger));
        } else {
            tokio::spawn(join_with_pipes(
                a,
                b,
                linger,
                Err(io::ErrorKind::Unsupported.into()),
            ));
        }
        (app, server)
    }

    #[test]
    fn this_kernel_splices() {
        // Or the two tests below would pass without testing it.
        assert!(kernel_splices());
    }

    #[tokio::test]
    async fn what_goes_in_one_end_comes_out_the_other_both_ways() {
        let (mut app, mut server) = joined(Duration::from_secs(5), true).await;
        // More than any buffer on the way holds at once.
        let sent: Vec<u8> = (0..8 << 20).map(|i: u32| (i % 251) as u8).collect();
        let expected = sent.clone();
        let writer = tokio::spawn(async move {
            app.write_all(&sent).await.unwrap();
            app.shutdown().await.unwrap();
            let mut reply = Vec::new();
            app.read_to_end(&mut reply).await.unwrap();
            reply
        });
        let mut got = Vec::new();
        server.read_to_end(&mut got).await.unwrap();
        assert!(got == expected, "the bytes arrived changed");
        server.write_all(b"done").await.unwrap();
        server.shutdown().await.unwrap();
        assert_eq!(writer.await.unwrap(), b"done");
    }

    #[tokio::test(start_paused = true)]
    async fn a_peer_that_never_closes_is_let_go_some_time_after_the_other_left() {
        assert_linger(true).await;
    }

    #[tokio::test(start_paused = true)]
    async fn copying_also_lets_a_peer_go_after_the_linger_time() {
        assert_linger(false).await;
    }

    async fn assert_linger(splice: bool) {
        let (app, mut server) = joined(Duration::from_secs(30), splice).await;
        drop(app);
        // The server is told the app has finished, and says nothing.
        let mut buf = [0u8; 1];
        assert_eq!(server.read(&mut buf).await.unwrap(), 0);
        tokio::time::sleep(Duration::from_secs(31)).await;
        // By now its connection has been closed under it.
        let ended = server.write_all(&[0u8; 1 << 20]).await;
        let ended = ended.and(server.write_all(&[0u8; 1 << 20]).await);
        assert!(ended.is_err());
    }
}
