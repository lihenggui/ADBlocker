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

use super::{Translation, Unsupported, arguments, names};

pub(super) fn translate(
    domains: &str,
    marker: &str,
    body: &str,
) -> Result<Translation, Unsupported> {
    let body = body.trim();
    if !body.starts_with("//scriptlet(") {
        return Err(Unsupported::JavaScript);
    }
    let args = body
        .strip_prefix("//scriptlet(")
        .and_then(|text| text.strip_suffix(')'))
        .and_then(read_args)
        .ok_or(Unsupported::ScriptletSyntax)?;
    if !crate::cosmetic::safe_raw_text(body) {
        return Err(Unsupported::UnsafeScriptlet);
    }
    let mut line = format!("{domains}{marker}+js(");
    let mut trusted = false;
    if let Some((name, args)) = args.split_first() {
        let resource = names::resolve(name)?;
        trusted = resource.starts_with("trusted-");
        let args = arguments::adapt(name, resource, args.to_vec())
            .ok_or(Unsupported::ScriptletArguments)?;
        // adblock-rust rejects a lone argument in object notation at injection.
        if matches!(&args[..], [arg] if arg.starts_with('{') && arg.ends_with('}')) {
            return Err(Unsupported::ScriptletArguments);
        }
        line.push_str(resource);
        for arg in args {
            line.push_str(", ");
            write_arg(&mut line, &arg).ok_or(Unsupported::ScriptletArguments)?;
        }
    } else if marker != "#@#" {
        return Err(Unsupported::ScriptletSyntax);
    }
    line.push(')');
    Ok(Translation {
        line,
        scriptlet: true,
        trusted,
    })
}

pub(super) fn read_args(mut input: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    input = input.trim_start();
    if input.is_empty() {
        return Some(args);
    }
    loop {
        let quote = input.chars().next().filter(|ch| matches!(ch, '\'' | '"'))?;
        let (arg, rest) = quoted(&input[1..], quote)?;
        args.push(arg);
        let rest = rest.trim_start();
        if rest.is_empty() {
            return Some(args);
        }
        input = rest.strip_prefix(',')?.trim_start();
    }
}

fn quoted(input: &str, quote: char) -> Option<(String, &str)> {
    let mut arg = String::new();
    let mut escapes = 0;
    for (index, ch) in input.char_indices() {
        if ch == quote {
            if escapes % 2 == 0 {
                return Some((arg, &input[index + 1..]));
            }
            arg.pop();
        }
        escapes = if ch == '\\' { escapes + 1 } else { 0 };
        arg.push(ch);
    }
    None
}

/// adblock-rust 0.13.6 keeps a quoted argument verbatim if its delimiter
/// is absent. Its escape normalization strips comma escapes even in the
/// quoted branch, so inserting an escape before a quote is lossy.
pub(super) fn write_arg(out: &mut String, arg: &str) -> Option<()> {
    let plain = !arg.is_empty() && arg.trim() == arg && !arg.starts_with(['\'', '"', '`']);
    let trailing_escape = arg.bytes().rev().take_while(|&b| b == b'\\').count() % 2 != 0;
    if plain && !arg.contains(',') {
        out.push_str(arg);
        if trailing_escape {
            // Keep a subsequent comma outside the run of backslashes.
            out.push(' ');
        }
        return Some(());
    }
    if !trailing_escape
        && let Some(delimiter) = ['\'', '"', '`'].into_iter().find(|&ch| !arg.contains(ch))
    {
        out.push(delimiter);
        out.push_str(arg);
        out.push(delimiter);
        return Some(());
    }
    if plain && !trailing_escape && unquoted_commas(out, arg) {
        return Some(());
    }
    None
}

fn unquoted_commas(out: &mut String, arg: &str) -> bool {
    let mut escapes = 0;
    for ch in arg.chars() {
        if ch == ',' {
            if escapes % 2 != 0 {
                return false;
            }
            out.push('\\');
        }
        escapes = if ch == '\\' { escapes + 1 } else { 0 };
        out.push(ch);
    }
    true
}
