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

//! Reads bounded package snapshots in Android's publication order.

use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};

use abd_engine::files;
use anyhow::{Context as _, Result, ensure};

use super::package_inventory::SharedUsers;
use super::package_settings::read_shared_users;

const PACKAGES: &str = "/data/system/packages.list";
const MAX_PACKAGES: usize = 4 << 20;

pub(super) fn android_output(args: &[&str], max: usize) -> Result<String> {
    let mut child = Command::new("/system/bin/timeout")
        .args(["-k", "1", "5", "/system/bin/cmd"])
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take().context("Android command output")?;
    let data = files::read_limited(stdout.by_ref(), max);
    drop(stdout);
    ensure!(child.wait()?.success(), "Android command failed");
    Ok(data?)
}

pub(super) fn package_list() -> Result<String> {
    Ok(String::from_utf8(files::read_bytes(
        Path::new(PACKAGES),
        MAX_PACKAGES,
    )?)?)
}

pub(super) fn query_packages(user: u32) -> Result<String> {
    android_output(
        &[
            "package",
            "list",
            "packages",
            "--user",
            &user.to_string(),
            "-U",
        ],
        MAX_PACKAGES,
    )
}

pub(super) fn package_snapshot(user: u32) -> Result<(String, String, Option<SharedUsers>)> {
    let list = query_packages(user)?;
    let known = package_list()?;
    let shared = read_shared_users().ok();
    Ok((list, known, shared))
}
