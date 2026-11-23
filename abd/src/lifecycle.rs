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

//! Stops the supervisor and the daemon of an installed module, and signals
//! the daemon.
//!
//! A pid file outlives the process it was written for, and its number may
//! by now be another program's. A process is therefore held by a
//! descriptor before anything is asked about it, and signalled through
//! that descriptor only once it is seen to be root's and to be running
//! what the file was written for.

#![allow(unsafe_code, reason = "pidfd system calls")]

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};
use std::{fs, io, io::Read as _, os::unix::fs::MetadataExt as _, path::Path};

use anyhow::{Context as _, ensure};

use crate::paths::{Module as _, Paths};

/// A process held by a descriptor, so that a signal meant for it cannot
/// reach another process that was given its pid in the meantime.
struct Process(OwnedFd);

impl Process {
    fn open(pid: i32) -> io::Result<Self> {
        // SAFETY: a system call on two integers.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: just opened, and owned by nothing else.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd as i32) }))
    }

    /// A pidfd becomes readable when this process exits, including before it is reaped.
    fn exited(&self, timeout_ms: i32) -> io::Result<bool> {
        let mut pfd = libc::pollfd {
            fd: self.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pfd` is one valid pollfd, as the count says.
        if unsafe { libc::poll(&mut pfd, 1, timeout_ms) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(pfd.revents & (libc::POLLIN | libc::POLLHUP) != 0)
    }

    /// Send `signal`. A process that has ended in the meantime gets none.
    fn signal(&self, signal: i32) -> io::Result<()> {
        // SAFETY: a system call on a descriptor we own; no siginfo is passed.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.0.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

/// What a pid file was written for.
#[derive(Clone, Copy)]
enum Program {
    /// `sh <module>/supervise.sh`
    Supervisor,
    /// `<module>/bin/abd run`
    Daemon,
}

fn pid_text(file: &Path) -> io::Result<String> {
    match fs::symlink_metadata(file) {
        Ok(metadata) if metadata.is_file() => {
            if metadata.len() > 32 {
                return Ok(String::new());
            }
            abd_engine::files::read_text(file, 32)
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PID file is not a regular file",
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// The supervisor may be scheduled after its child. Do not start capture
/// until the PID file names this process in full.
pub fn await_pid(paths: &Paths) -> io::Result<()> {
    wait_for_pid(&paths.pid(), std::process::id(), Duration::from_secs(3))
}

fn wait_for_pid(file: &Path, pid: u32, timeout: Duration) -> io::Result<()> {
    let expected = format!("{pid}\n");
    let deadline = Instant::now() + timeout;
    loop {
        if pid_text(file)? == expected {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "daemon PID was not published",
            ));
        }
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

fn published_pid(text: &str) -> Option<i32> {
    // A short write can leave digits that name a different process.
    text.strip_suffix('\n')?
        .trim()
        .parse()
        .ok()
        .filter(|pid| *pid > 1)
}

/// The process a pid file names, if it still is what the file was written
/// for. A file that is missing or holds no pid names nothing.
fn held(file: &Path, program: Program) -> io::Result<Option<(i32, Process)>> {
    let text = pid_text(file)?;
    let Some(pid) = published_pid(&text) else {
        return Ok(None);
    };
    let process = match Process::open(pid) {
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)) => return Ok(None),
        result => result?,
    };
    let dir = Path::new("/proc").join(pid.to_string());
    let read = || -> io::Result<bool> {
        if process.exited(0)? || dir.metadata()?.uid() != 0 {
            return Ok(false);
        }
        let mut cmdline = Vec::new();
        fs::File::open(dir.join("cmdline"))?
            .take(8192)
            .read_to_end(&mut cmdline)?;
        Ok(matches_command(&cmdline, program))
    };
    match read() {
        Ok(true) => Ok(Some((pid, process))),
        Ok(false) => Ok(None),
        // Gone while it was being looked at.
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Stops the supervisor, then the daemon. The supervisor first: it stops
/// the daemon itself and waits while it removes its rules, and would
/// restart one stopped behind its back.
pub fn stop(paths: &Paths) -> anyhow::Result<()> {
    let supervisor =
        stop_one(&paths.supervisor_pid(), Program::Supervisor).context("stopping the supervisor");
    // A bad supervisor record must not prevent stopping a known daemon.
    let daemon = stop_one(&paths.pid(), Program::Daemon).context("stopping the daemon");
    supervisor.and(daemon)
}

fn stop_one(file: &Path, program: Program) -> anyhow::Result<()> {
    if let Some((_, process)) = held(file, program).context("reading a pid file")? {
        process.signal(libc::SIGTERM)?;
        if !process.exited(5000)? {
            process.signal(libc::SIGKILL)?;
            ensure!(
                process.exited(1000)?,
                "a process of the module would not end"
            );
        }
    }
    // The supervisor may already have removed its child's pid file.
    if let Err(e) = fs::remove_file(file)
        && e.kind() != io::ErrorKind::NotFound
    {
        return Err(e.into());
    }
    Ok(())
}

/// The pid of the running daemon, if one runs. Signals nothing.
pub fn daemon(paths: &Paths) -> Option<i32> {
    held(&paths.pid(), Program::Daemon)
        .ok()?
        .map(|(pid, _)| pid)
}

/// Asks a running daemon to read its lists and apps again. Does nothing
/// without one.
pub fn reload(paths: &Paths) {
    if let Ok(Some((_, daemon))) = held(&paths.pid(), Program::Daemon) {
        // It can fail only for a daemon that ended since it was found,
        // and the next one reads everything as it starts.
        let _ = daemon.signal(libc::SIGHUP);
    }
}

/// Whether a command line is that of `program`, in whichever directory the
/// module is: a root manager may run a newly installed module's scripts
/// from another path than the one the running daemon was started from.
fn matches_command(cmdline: &[u8], program: Program) -> bool {
    let mut args = cmdline.split(|b| *b == 0);
    let command = args.next().unwrap_or_default();
    let argument = args.next().unwrap_or_default();
    let name = |path: &[u8]| -> Vec<u8> {
        path.rsplit(|b| *b == b'/')
            .next()
            .unwrap_or_default()
            .to_vec()
    };
    match program {
        Program::Daemon => command.ends_with(b"/bin/abd") && argument == b"run",
        Program::Supervisor => {
            let is_shell = |name: &[u8]| matches!(name, b"sh" | b"ash" | b"mksh");
            let script = |path: &[u8]| path.ends_with(b"/supervise.sh");
            let program = name(command);
            if matches!(program.as_slice(), b"busybox" | b"toybox") {
                is_shell(argument) && args.next().is_some_and(script)
            } else {
                is_shell(&program) && script(argument)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_supervisor_from_android_or_the_root_manager_shell() {
        let is = |cmdline: &[u8]| matches_command(cmdline, Program::Supervisor);
        assert!(is(
            b"/system/bin/sh\0/data/adb/modules/adblocker/supervise.sh\0"
        ));
        assert!(is(
            b"/data/adb/magisk/busybox\0ash\0/data/adb/modules/adblocker/supervise.sh\0"
        ));
        assert!(!is(b"sh\0-c\0/data/adb/modules/adblocker/supervise.sh\0"));
        assert!(!is(b"log\0/data/adb/modules/adblocker/supervise.sh\0"));
        assert!(!is(b"/system/bin/sh\0/sdcard/supervise.sh.txt\0"));
    }

    #[test]
    fn recognizes_the_daemon_only_as_it_is_run() {
        let is = |cmdline: &[u8]| matches_command(cmdline, Program::Daemon);
        assert!(is(b"/data/adb/modules/adblocker/bin/abd\0run\0"));
        assert!(is(b"/data/adb/modules_update/adblocker/bin/abd\0run\0"));
        assert!(!is(b"/data/adb/modules/adblocker/bin/abd\0update\0"));
        assert!(!is(b"abd\0run\0"));
        assert!(!is(b"/data/local/tmp/abd\0run\0"));
    }

    #[test]
    fn a_pid_file_that_names_nothing_stops_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abd.pid");
        assert!(held(&file, Program::Daemon).unwrap().is_none());
        for text in ["", "garbage\n", "-5\n", "1\n", "99999999\n"] {
            fs::write(&file, text).unwrap();
            assert!(held(&file, Program::Daemon).unwrap().is_none(), "{text:?}");
        }
    }

    #[test]
    fn capture_waits_for_the_complete_pid_written_for_this_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abd.pid");
        let pid = std::process::id();
        assert_eq!(published_pid("1234"), None);
        assert_eq!(published_pid("1234\n"), Some(1234));
        for text in [String::new(), format!("{pid}"), format!("{}\n", pid + 1)] {
            fs::write(&file, text).unwrap();
            let error = wait_for_pid(&file, pid, Duration::ZERO).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        }
        fs::write(&file, format!("{pid}\n")).unwrap();
        wait_for_pid(&file, pid, Duration::ZERO).unwrap();
        fs::remove_file(&file).unwrap();
        fs::create_dir(&file).unwrap();
        let error = wait_for_pid(&file, pid, Duration::ZERO).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_supervisor_pid_file_error_does_not_skip_daemon_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        fs::create_dir(paths.supervisor_pid()).unwrap();
        fs::write(paths.pid(), format!("{}\n", i32::MAX)).unwrap();

        assert!(stop(&paths).is_err());
        assert!(!paths.pid().exists());
    }

    #[test]
    fn a_process_that_is_not_the_daemon_is_never_signalled() {
        // This test process: alive, not root's daemon, and named in the file.
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let pid = format!("{}\n", std::process::id());
        fs::write(paths.pid(), &pid).unwrap();
        fs::write(paths.supervisor_pid(), &pid).unwrap();
        reload(&paths);
        assert_eq!(daemon(&paths), None);
        stop(&paths).unwrap();
        assert!(!paths.pid().exists() && !paths.supervisor_pid().exists());
    }
}
