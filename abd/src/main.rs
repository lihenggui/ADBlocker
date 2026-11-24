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

//! The filtering daemon and the commands that go with it.
//!
//!   abd run                    filter the traffic of the chosen apps
//!   abd trust remove           stop every app trusting the authority
//!   abd trust mount <pid> <uid>   run by the daemon: see trust.rs
//!   abd trust unmount <pid> <uid>   likewise
//!   abd unhook                 remove the packet filter and routing rules
//!   abd module-stop            stop the supervisor and the daemon
//!   abd reload                 have the daemon read its lists and apps again
//!
//! and the commands of `cli.rs` (`cli::USAGE`): what the app, the module's
//! scripts and the user run.
//!
//! The data is in /data/adb/adblocker, or in $ABD_DATA. Rules of your own go
//! in `rules/*.txt` there, in the syntax of the lists.

mod abx;
mod android;
mod capture;
mod cli;
mod daemon;
mod dns;
mod handoff;
mod lifecycle;
mod mounts;
mod paths;
mod properties;

mod rules;
mod sockdiag;
mod system_roots;
mod trust;
mod watch;

use std::path::Path;

use abd_engine::malformed::Malformed;
use anyhow::{Context as _, Result, bail};

use paths::Paths;

/// What `abd trust remove` exits with when the copy of the certificate
/// store had to be left on disk (see `trust::remove`).
const EXIT_STORE_KEPT: i32 = 3;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // Whoever starts us, and with whatever umask: nothing written here is
    // for anyone but root. The directory holds the authority's key and
    // what the apps talk to.
    // SAFETY: sets the file mode mask of this process; cannot fail.
    #[allow(unsafe_code, reason = "umask")]
    unsafe {
        libc::umask(0o077);
    }
    // Network cleanup must work even if the data directory is inaccessible.
    if args == ["unhook"] {
        return unhook();
    }
    // Before any command reads the default lists, so that the daemon and
    // every command see the same ones. A device without a locale has the
    // general defaults.
    abd_engine::catalog::set_device_languages(properties::locale().as_deref());
    // So must leaving the user's store as Android has it, which every app
    // reads: first at each start, and last on uninstall, after which
    // nothing of ours runs. Filtering goes on whatever becomes of it.
    if (args == ["run"] || args == ["trust", "remove"])
        && let Err(error) = trust::repair_user_store()
    {
        eprintln!("trust: the user's store: {error:#}");
    }
    let data = std::env::var("ABD_DATA").unwrap_or_else(|_| paths::DATA.to_owned());
    let paths = Paths::new(Path::new(&data));
    make_private_data_dir(&data)?;

    let done = match args.as_slice() {
        ["run"] => {
            if std::env::var_os("ABD_EXPECT_PID").is_some() {
                lifecycle::await_pid(&paths)?;
            }
            abd_engine::runtime::build()?.block_on(daemon::run(paths))
        }
        // Before any thread is started: see trust.rs.
        ["trust", "mount", pid, uid] => {
            let pid: i32 = pid.parse().context("pid")?;
            match trust::mount_in(&paths, pid, uid.parse().context("uid")?)? {
                trust::Mounting::Mounted => Ok(()),
                trust::Mounting::Had => std::process::exit(trust::EXIT_HAD),
                // Gone before it could be looked at: no failure.
                trust::Mounting::Gone => std::process::exit(trust::EXIT_GONE),
            }
        }
        // As above: before any thread is started.
        ["trust", "unmount", pid, uid] => {
            let pid: i32 = pid.parse().context("pid")?;
            match trust::unmount_from(&paths, pid, uid.parse().context("uid")?)? {
                trust::Unmounted::Absent => Ok(()),
                trust::Unmounted::Gone => std::process::exit(trust::EXIT_GONE),
                trust::Unmounted::StillMounted => {
                    bail!("the stores are still mounted in process {pid}")
                }
            }
        }
        ["trust", "remove"] => {
            // What the uninstaller goes by: the data may all be removed
            // only if the copy of the store is mounted nowhere any more.
            let keep = std::env::var_os("ABD_KEEP_TRUST_STORES").is_some();
            if !trust::remove(&paths, keep)? {
                std::process::exit(EXIT_STORE_KEPT);
            }
            Ok(())
        }
        ["module-stop"] => lifecycle::stop(&paths),
        ["reload"] => {
            lifecycle::reload(&paths);
            Ok(())
        }
        _ => cli::run(&paths, &args),
    };
    if let Err(e) = &done
        && let Some(malformed) = e.downcast_ref::<Malformed>()
    {
        eprintln!("abd: {malformed}\n\n{}", cli::USAGE);
        std::process::exit(2);
    }
    done
}

/// Removes the rules of both captures from any state, with no daemon
/// running. The lookup rules need no lock and go first, with the TCP
/// routes, before anything waits for the packet filter's.
fn unhook() -> Result<()> {
    let withdrawn = dns::withdraw();
    let tcp = rules::unhook();
    let lookups = dns::clear();
    withdrawn.and(tcp).and(lookups)
}

fn make_private_data_dir(data: &str) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(data)
        .with_context(|| format!("create {data}"))?;
    std::fs::set_permissions(data, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}
