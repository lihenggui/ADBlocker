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

use anyhow::Result;

use super::{Ctx, changed, unknown};
use crate::settings::{self, Settings};

pub(super) fn run(
    ctx: &Ctx<'_>,
    args: &[&str],
    _input: &mut dyn Read,
    _out: &mut dyn Write,
) -> Result<()> {
    let Ctx { paths, host, .. } = *ctx;
    match args {
        ["scope", value @ ("chosen" | "all")] => {
            Settings::set(&paths.settings(), settings::SCOPE, value)?;
            changed(host)
        }
        [
            name @ (settings::URL_FILTERING | settings::COSMETIC),
            say @ ("on" | "off"),
        ] => {
            Settings::set(&paths.settings(), name, say)?;
            changed(host)
        }
        _ => unknown(args),
    }
}
