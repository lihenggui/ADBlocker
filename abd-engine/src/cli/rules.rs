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

use anyhow::{Context as _, Result, bail};

use super::{Ctx, print_json, unknown};
use crate::list_files::{MAX_LIST, check_growth};
use crate::paths::Paths;
use crate::{files, rulecheck};

pub(super) fn run(
    ctx: &Ctx<'_>,
    args: &[&str],
    input: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<()> {
    let Ctx { paths, host, .. } = *ctx;
    match args {
        ["rules", "get"] => {
            let text = files::read_text(&paths.user_rules(), MAX_LIST)?;
            write!(out, "{text}")?;
            Ok(())
        }
        ["rules", "set"] | ["rules", "append"] | ["rules", "append", "--unique"] => {
            let mode = match args {
                ["rules", "set"] => RulesWrite::Replace,
                ["rules", "append", "--unique"] => RulesWrite::AppendUnique,
                _ => RulesWrite::Append,
            };
            if write_rules(paths, stdin(input)?, mode)? {
                host.reload();
            }
            Ok(())
        }
        ["rules", "check"] => {
            for issue in rulecheck::check(&stdin(input)?, rulecheck::SourcePermissions::USER).issues
            {
                writeln!(
                    out,
                    "{}: {:?} ({}): {}",
                    issue.line, issue.kind, issue.reason, issue.text
                )?;
            }
            Ok(())
        }
        ["rules", "check", "--json"] => print_json(
            out,
            &rulecheck::check(&stdin(input)?, rulecheck::SourcePermissions::USER),
        ),
        _ => unknown(args),
    }
}

/// The largest input taken: that of a list.
fn stdin(input: &mut dyn Read) -> Result<String> {
    files::read_limited(input, MAX_LIST).context("standard input")
}

enum RulesWrite {
    Replace,
    Append,
    AppendUnique,
}

/// Reads and changes user.txt under the same lock for CLI processes and VPN commands.
fn write_rules(paths: &Paths, input: String, mode: RulesWrite) -> Result<bool> {
    std::fs::create_dir_all(paths.rules())?;
    let _lock = files::lock(&paths.settings())?;
    let file = paths.user_rules();
    let dirs = [&*paths.lists(), &*paths.rules()];
    if matches!(mode, RulesWrite::Replace) {
        check_growth(&dirs, &[&file], input.len())?;
        files::replace(&file, input.as_bytes())?;
        return Ok(true);
    }
    let mut current = files::read_text(&file, MAX_LIST)?;
    if matches!(mode, RulesWrite::AppendUnique) {
        let rule = input.trim();
        if rule.is_empty() || rule.lines().count() != 1 {
            bail!("expected one rule");
        }
        if current.lines().any(|line| line.trim() == rule) {
            return Ok(false);
        }
    }
    if input.is_empty() {
        return Ok(false);
    }
    let separator = !current.is_empty() && !current.ends_with('\n');
    if current.len() + usize::from(separator) + input.len() > MAX_LIST {
        bail!("combined rules exceed the size limit");
    }
    if separator {
        current.push('\n');
    }
    current.push_str(&input);
    check_growth(&dirs, &[&file], current.len())?;
    files::replace(&file, current.as_bytes())?;
    Ok(true)
}
