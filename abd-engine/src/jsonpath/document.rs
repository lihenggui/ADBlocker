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
//! Preserve the wrapper and copy retained JSON tokens as written, dropping
//! the whitespace between tokens in a pruned body.

use std::borrow::Cow;

use serde_json::Value;

use super::Path;
use super::evaluate::{Selection, Slot};
use crate::limits::MAX_REWRITE_RESULT;

/// An invalid or unchanged document is passed on verbatim. Exhausting
/// the traversal budget abandons the entire response rewrite.
pub(crate) fn prune(
    path: &Path,
    input: &[u8],
    within_budget: &dyn Fn() -> bool,
) -> Option<Vec<u8>> {
    let Some((text, value)) = document(input) else {
        return Some(input.to_vec());
    };
    let selection = path.select(&value, within_budget).ok()?;
    if matches!(selection, Selection::Keep) {
        return Some(input.to_vec());
    }
    let start = text.as_ptr() as usize - input.as_ptr() as usize;
    let mut output = Vec::with_capacity(input.len());
    output.extend_from_slice(&input[..start]);
    if Tokens(text).write(&value, selection, &mut output).is_none() {
        // Value collapses duplicate keys. If the plan cannot be mapped
        // back to the source unambiguously, keep the original document.
        return Some(input.to_vec());
    }
    output.extend_from_slice(&input[start + text.len()..]);
    (output.len() <= MAX_REWRITE_RESULT).then_some(output)
}

fn document(input: &[u8]) -> Option<(&str, Value)> {
    let text = std::str::from_utf8(input).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = text.trim_matches(json_space);
    if let Ok(value) = serde_json::from_str(text) {
        return Some((text, value));
    }
    let (callback, _) = text.split_once('(')?;
    if !callback.split('.').all(js_name) {
        return None;
    }
    let call = text
        .strip_suffix(';')
        .unwrap_or(text)
        .trim_end_matches(json_space);
    let body = call.strip_suffix(')')?;
    let body = body.strip_prefix(callback)?.strip_prefix('(')?;
    let body = body.trim_matches(json_space);
    Some((body, serde_json::from_str(body).ok()?))
}

fn json_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\r' | '\n')
}

fn js_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let start = |byte: u8| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$');
    bytes.next().is_some_and(start) && bytes.all(|byte| start(byte) || byte.is_ascii_digit())
}

/// The document has been validated, so only token boundaries are needed here.
struct Tokens<'a>(&'a str);

impl<'a> Tokens<'a> {
    fn next(&mut self) -> Option<&'a str> {
        let text = self.0.trim_start_matches(json_space);
        let end = match *text.as_bytes().first()? {
            b'{' | b'}' | b'[' | b']' | b':' | b',' => 1,
            b'"' => {
                let mut escaped = false;
                text.bytes().enumerate().skip(1).find_map(|(at, byte)| {
                    let end = (byte == b'"' && !escaped).then_some(at + 1);
                    escaped = byte == b'\\' && !escaped;
                    end
                })?
            }
            _ => text
                .find(|ch| json_space(ch) || matches!(ch, ',' | ']' | '}'))
                .unwrap_or(text.len()),
        };
        self.0 = &text[end..];
        Some(&text[..end])
    }

    /// Emit or skip one subtree without re-encoding any of its JSON tokens.
    fn copy(&mut self, output: &mut Vec<u8>, keep: bool) -> Option<()> {
        let mut depth = 0;
        loop {
            let token = self.next()?;
            if keep {
                output.extend_from_slice(token.as_bytes());
            }
            match token {
                "[" | "{" => depth += 1,
                "]" | "}" if depth > 0 => depth -= 1,
                _ => (),
            }
            if depth == 0 {
                return Some(());
            }
        }
    }

    fn write(
        &mut self,
        value: &Value,
        selection: Selection<'_>,
        output: &mut Vec<u8>,
    ) -> Option<()> {
        let mut selected = match selection {
            Selection::Keep => return self.copy(output, true),
            Selection::Remove => return self.copy(output, false),
            Selection::Children(children) => children,
        };
        let (open, close, len) = match value {
            Value::Object(members) => ("{", "}", members.len()),
            Value::Array(items) => ("[", "]", items.len()),
            _ => return None,
        };
        (self.next()? == open).then_some(())?;
        output.extend_from_slice(open.as_bytes());
        let mut comma = false;
        for at in 0..len {
            if at > 0 && self.next()? != "," {
                return None;
            }
            let (slot, raw, child) = match value {
                Value::Object(members) => {
                    let raw = self.next()?;
                    let name = member_name(raw)?;
                    let (key, child) = members.get_key_value(name.as_ref())?;
                    (self.next()? == ":").then_some(())?;
                    (Slot::Name(key), Some(raw), child)
                }
                Value::Array(items) => (Slot::Index(at), None, items.get(at)?),
                _ => return None,
            };
            let selection = selected.remove(&slot).unwrap_or(Selection::Keep);
            if !matches!(selection, Selection::Remove) {
                if std::mem::replace(&mut comma, true) {
                    output.push(b',');
                }
                if let Some(raw) = raw {
                    output.extend_from_slice(raw.as_bytes());
                    output.push(b':');
                }
            }
            self.write(child, selection, output)?;
        }
        // Value counts unique keys. An additional member makes the
        // mapping ambiguous, including when keys use different escapes.
        (self.next()? == close).then_some(())?;
        output.extend_from_slice(close.as_bytes());
        Some(())
    }
}

fn member_name(token: &str) -> Option<Cow<'_, str>> {
    let name = token.strip_prefix('"')?.strip_suffix('"')?;
    if name.contains('\\') {
        serde_json::from_str::<String>(token).ok().map(Cow::Owned)
    } else {
        Some(Cow::Borrowed(name))
    }
}
