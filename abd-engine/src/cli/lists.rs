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

use std::io::{Read, Write};

use anyhow::{Result, bail};

use super::{Ctx, changed, print_json, unknown};
use crate::malformed::Malformed;
use crate::{files, protocol, sources};

pub(super) fn run(
    ctx: &Ctx<'_>,
    args: &[&str],
    input: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<()> {
    let Ctx { paths, host, .. } = *ctx;
    match args {
        ["lists"] => {
            for list in protocol::lists(paths) {
                let state = if list.enabled { "on " } else { "off" };
                match list.error {
                    Some(e) => writeln!(out, "{state} {}  ({e})", list.url)?,
                    None => writeln!(out, "{state} {}", list.url)?,
                }
            }
            Ok(())
        }
        ["lists", "--json"] => print_json(out, &protocol::lists(paths)),
        ["list", "configure"] => {
            let selection = files::read_limited(input, sources::MAX_FILE)?;
            sources::configure(paths, &selection)?;
            changed(host)
        }
        ["list", change, url] => {
            let change = match *change {
                "add" => sources::Change::Add,
                "remove" => sources::Change::Remove,
                "enable" => sources::Change::Enable,
                "disable" => sources::Change::Disable,
                _ => bail!(Malformed(format!("unknown command: list {change}"))),
            };
            let edited = sources::edit(paths, url, change);
            // A write can have changed the active files before cleanup failed.
            if !matches!(edited, Ok(false)) {
                host.reload();
            }
            edited.map(|_| ())
        }
        _ => unknown(args),
    }
}
