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

use super::{Translation, Unsupported};

pub(super) fn translate(
    domains: &str,
    marker: &str,
    body: &str,
) -> Result<Translation, Unsupported> {
    let (selector, declarations) = split_rule(body.trim()).ok_or(Unsupported::StyleSyntax)?;
    if [":style(", ":remove-attr(", ":remove-class(", ":remove()"]
        .iter()
        .any(|action| selector.contains(action))
    {
        return Err(Unsupported::StyleSyntax);
    }
    let remove = declarations
        .trim_end_matches(';')
        .split_once(':')
        .is_some_and(|(key, value)| key.trim() == "remove" && value.trim() == "true");
    let line = if remove {
        format!("{domains}{marker}{selector}:remove()")
    } else {
        if declarations.split(';').any(|part| {
            part.split_once(':')
                .is_some_and(|(name, _)| name.trim() == "remove")
        }) {
            return Err(Unsupported::StyleSyntax);
        }
        format!("{domains}{marker}{selector}:style({declarations})")
    };
    Ok(Translation {
        line,
        scriptlet: false,
        trusted: false,
    })
}

fn split_rule(body: &str) -> Option<(&str, &str)> {
    if body.starts_with('@') {
        return None;
    }
    let mut quote = None;
    let mut escaped = false;
    let mut blocks = Vec::new();
    let mut opening = None;
    for (index, ch) in body.char_indices() {
        if std::mem::take(&mut escaped) {
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(end) = quote {
            if ch == end {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => blocks.push(')'),
            '[' => blocks.push(']'),
            ')' | ']' if blocks.pop() != Some(ch) => return None,
            '{' if blocks.is_empty() => {
                if opening.replace(index).is_some() {
                    return None;
                }
            }
            '}' if blocks.is_empty() => {
                let open = opening?;
                let selector = body[..open].trim();
                let declarations = body[open + 1..index].trim();
                return (index + 1 == body.len()
                    && !selector.is_empty()
                    && !declarations.is_empty())
                .then_some((selector, declarations));
            }
            _ => (),
        }
    }
    None
}

/// The procedural runtime applies inline declarations with this restricted
/// value grammar. Recognize the same subset when reporting a translated rule.
pub(super) fn supported(declarations: &str) -> bool {
    let mut parts = declarations
        .split(';')
        .filter(|part| !part.trim().is_empty());
    parts.next().is_some_and(safe_declaration)
        && parts.by_ref().take(15).all(safe_declaration)
        && parts.next().is_none()
}

fn safe_declaration(part: &str) -> bool {
    let Some((property, value)) = part.split_once(':') else {
        return false;
    };
    let property = property.trim();
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    let value = lower
        .strip_suffix("!important")
        .unwrap_or(&lower)
        .trim_end();
    property.starts_with(|ch: char| ch.is_ascii_alphabetic())
        && property
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-')
        && !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b" .,%+#()/-".contains(&b))
        && value.split('(').rev().skip(1).all(safe_function)
}

fn safe_function(prefix: &str) -> bool {
    let is_name = |ch: char| ch.is_ascii_alphabetic() || ch == '-';
    let name = prefix
        .trim_end()
        .rsplit(|ch| !is_name(ch))
        .next()
        .unwrap_or_default();
    name.is_empty()
        || !prefix.ends_with(' ')
            && matches!(
                name,
                "rgb"
                    | "rgba"
                    | "hsl"
                    | "hsla"
                    | "calc"
                    | "min"
                    | "max"
                    | "clamp"
                    | "translate"
                    | "translatex"
                    | "translatey"
                    | "scale"
                    | "scalex"
                    | "scaley"
                    | "rotate"
            )
}
