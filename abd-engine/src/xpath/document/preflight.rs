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

use super::super::xml_space;
use crate::limits::{
    MAX_XML_ATTRIBUTES, MAX_XML_DEPTH, MAX_XML_NAMESPACE_DECLARATIONS, MAX_XML_PARSE_MARKERS,
};

/// Bound the work that takes place inside the XML parser before it can
/// return a tree. In particular, duplicate attribute and namespace checks
/// are quadratic in the number of declarations.
pub(super) fn check<'a>(text: &'a str, within_budget: &dyn Fn() -> bool) -> Option<&'a str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = match text.strip_prefix("<?xml") {
        Some(rest) if rest.starts_with(xml_space) => declaration(rest)?,
        _ => text,
    };
    if text.starts_with('\u{feff}') {
        return None;
    }
    let mut markers = text.bytes().filter(|byte| matches!(byte, b'<' | b'='));
    if markers.nth(MAX_XML_PARSE_MARKERS).is_some() {
        return None;
    }
    let mut rest = text;
    let mut depth: usize = 0;
    let mut namespaces = 0;
    while let Some((_, markup)) = rest.split_once('<') {
        if !within_budget() {
            return None;
        }
        rest = if let Some(comment) = markup.strip_prefix("!--") {
            comment.split_once("-->")?.1
        } else if let Some(cdata) = markup.strip_prefix("![CDATA[") {
            cdata.split_once("]]>")?.1
        } else if let Some(pi) = markup.strip_prefix('?') {
            let (contents, after) = pi.split_once("?>")?;
            let target = contents.split(xml_space).next()?;
            if !super::super::lexer::valid_pi_target(target) {
                return None;
            }
            after
        } else if markup.starts_with('!') {
            // No DTD means no declarations that can expand general entities.
            return None;
        } else if let Some(close) = markup.strip_prefix('/') {
            depth = depth.checked_sub(1)?;
            close.split_once('>')?.1
        } else {
            let (tag, after) = start_tag(markup, &mut namespaces)?;
            if depth == MAX_XML_DEPTH {
                return None;
            }
            depth += usize::from(!tag.ends_with('/'));
            after
        };
    }
    (depth == 0).then_some(text)
}

fn start_tag<'a>(mut text: &'a str, namespaces: &mut usize) -> Option<(&'a str, &'a str)> {
    let mut attributes = 0;
    loop {
        let index = text.find(['=', '>', '<'])?;
        let (name, rest) = text.split_at(index);
        match rest.as_bytes()[0] {
            b'>' => return Some((name, &rest[1..])),
            b'=' => {
                attributes += 1;
                if attributes > MAX_XML_ATTRIBUTES {
                    return None;
                }
                let name = name.trim_end_matches(xml_space).rsplit(xml_space).next()?;
                if name == "xmlns" || name.starts_with("xmlns:") {
                    *namespaces += 1;
                    if *namespaces > MAX_XML_NAMESPACE_DECLARATIONS {
                        return None;
                    }
                }
                text = quoted(rest[1..].trim_start_matches(xml_space))?.1;
            }
            _ => return None,
        }
    }
}

fn declaration(text: &str) -> Option<&str> {
    let (mut fields, after) = text.split_once("?>")?;
    let version = field(&mut fields, "version")?.strip_prefix("1.")?;
    if version.is_empty() || !version.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if fields.trim_start_matches(xml_space).starts_with("encoding")
        && !field(&mut fields, "encoding")?.eq_ignore_ascii_case("utf-8")
    {
        return None;
    }
    if fields
        .trim_start_matches(xml_space)
        .starts_with("standalone")
        && !matches!(field(&mut fields, "standalone")?, "yes" | "no")
    {
        return None;
    }
    fields.trim_matches(xml_space).is_empty().then_some(after)
}

fn field<'a>(fields: &mut &'a str, name: &str) -> Option<&'a str> {
    let text = fields
        .strip_prefix(xml_space)?
        .trim_start_matches(xml_space);
    let value = text
        .strip_prefix(name)?
        .trim_start_matches(xml_space)
        .strip_prefix('=')?;
    let (value, rest) = quoted(value.trim_start_matches(xml_space))?;
    *fields = rest;
    Some(value)
}

fn quoted(text: &str) -> Option<(&str, &str)> {
    let quote = text.chars().next().filter(|ch| matches!(ch, '\'' | '"'))?;
    text[1..].split_once(quote)
}
