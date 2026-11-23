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

use super::app::chosen;
use super::{Ctx, app_identity, changed, unknown};
use crate::clock;
use crate::malformed::Malformed;
use crate::pause::{MAX_MINUTES, Pauses};

pub(super) fn run(
    ctx: &Ctx<'_>,
    args: &[&str],
    _input: &mut dyn Read,
    _out: &mut dyn Write,
) -> Result<()> {
    let Ctx { paths, host, .. } = *ctx;
    match args {
        ["pause", rest @ ..] => {
            let n = match rest {
                [] => None,
                ["--minutes", n] => Some(minutes(n)?),
                _ => bail!(Malformed("pause takes --minutes <n> or nothing".to_owned())),
            };
            let now = clock::now();
            Pauses::change(&paths.pauses(), now, |p| {
                p.pause_all(n.map(|n| now + n * 60));
            })?;
            changed(host)
        }
        ["resume"] => {
            Pauses::change(&paths.pauses(), clock::now(), Pauses::resume_all)?;
            changed(host)
        }
        _ => unknown(args),
    }
}

pub(super) fn app(ctx: &Ctx<'_>, args: &[&str]) -> Result<()> {
    let Ctx { paths, host, .. } = *ctx;
    match args {
        ["app", "pause", name, "--minutes", n] => {
            let (name, n) = (app_identity(name)?, minutes(n)?);
            chosen(ctx, name)?;
            let now = clock::now();
            Pauses::change(&paths.pauses(), now, |p| p.pause_app(name, now + n * 60))?;
            changed(host)
        }
        ["app", "resume", name] => {
            let name = app_identity(name)?;
            Pauses::change(&paths.pauses(), clock::now(), |p| p.resume_app(name))?;
            changed(host)
        }
        _ => unknown(args),
    }
}

fn minutes(n: &str) -> Result<u64> {
    match n.parse::<u64>() {
        Ok(n) if (1..=MAX_MINUTES).contains(&n) => Ok(n),
        _ => bail!(Malformed(format!(
            "not a number of minutes from 1 to {}: {n:?}",
            MAX_MINUTES
        ))),
    }
}
