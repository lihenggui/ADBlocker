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

//! The VPN's descriptor, as the system hands it over: a read is one packet
//! the apps sent, a write one packet for them.

#![allow(
    unsafe_code,
    reason = "fcntl on the VPN's descriptor, and its registering"
)]

use std::fs::File;
use std::io::{self, Read as _, Write};
use std::os::fd::{AsRawFd as _, OwnedFd};

use tokio::io::unix::AsyncFd;

use crate::tunnel::Tun;

/// Closed when dropped, which is what takes the VPN down.
pub struct Fd(AsyncFd<File>);

impl Fd {
    /// Takes the descriptor, which is ours alone from here on. Must be
    /// called where the runtime is.
    pub fn new(fd: OwnedFd) -> io::Result<Self> {
        // Nothing may wait in a read or a write: the one task that owns
        // the stack does both, and everything else beside.
        // SAFETY: reads and sets the flags of a descriptor we own; no
        // memory is involved.
        let set = unsafe {
            let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
            flags >= 0 && libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
        };
        if !set {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the file owns the descriptor, which stays open and the
        // same for as long as the file lives, and the file lives in what
        // is returned.
        let registered = unsafe { AsyncFd::register(File::from(fd)) };
        Ok(Self(registered.map_err(io::Error::from)?))
    }
}

impl Tun for Fd {
    async fn recv(&mut self, into: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut ready = self.0.readable().await?;
            // Not ready after all: waited for again.
            if let Ok(read) = ready.try_io(|fd| fd.get_ref().read(into)) {
                return read;
            }
        }
    }

    fn send(&mut self, packet: &[u8]) -> io::Result<()> {
        write_packet(self.0.get_ref(), packet)
    }
}

fn write_packet(mut out: impl Write, packet: &[u8]) -> io::Result<()> {
    // A second write would send the remainder as another packet.
    if out.write(packet)? == packet.len() {
        Ok(())
    } else {
        Err(io::ErrorKind::WriteZero.into())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixDatagram;

    use super::*;

    #[test]
    fn a_short_write_fails_the_whole_packet() {
        let mut written = [0; 2];
        let error = write_packet(written.as_mut_slice(), b"packet").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
        assert_eq!(&written, b"pa");
    }

    #[tokio::test]
    async fn a_read_is_one_packet_and_a_write_another() {
        // Datagrams keep their bounds, as the packets of a VPN do.
        let (system, ours) = UnixDatagram::pair().unwrap();
        let mut fd = Fd::new(ours.into()).unwrap();
        system.send(b"first").unwrap();
        system.send(b"second, longer").unwrap();
        let mut buf = [0u8; 64];
        let n = fd.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"first");
        let n = fd.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"second, longer");

        fd.send(b"for an app").unwrap();
        let n = system.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"for an app");
    }

    #[tokio::test]
    async fn a_read_waits_for_a_packet_without_holding_the_thread() {
        let (system, ours) = UnixDatagram::pair().unwrap();
        let mut fd = Fd::new(ours.into()).unwrap();
        let mut buf = [0u8; 16];
        let waited = tokio::time::timeout(std::time::Duration::from_millis(50), fd.recv(&mut buf));
        assert!(waited.await.is_err());
        system.send(b"late").unwrap();
        assert_eq!(fd.recv(&mut buf).await.unwrap(), 4);
    }
}
