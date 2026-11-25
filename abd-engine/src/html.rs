// SPDX-License-Identifier: GPL-3.0-only
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

//! Byte-level markup and charset detection shared by HTML body edits.

use std::ops::Range;

use encoding_rs::Encoding;
use hyper::{HeaderMap, header::CONTENT_TYPE};

pub(crate) fn find_ascii(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|bytes| bytes.eq_ignore_ascii_case(needle))
}

pub(crate) fn space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

pub(crate) fn attr_end(bytes: &[u8], from: usize, until: impl Fn(u8) -> bool) -> Option<usize> {
    bytes[from..]
        .iter()
        .copied()
        .position(until)
        .map(|i| from + i)
}

// The end of a plain start tag, with attributes parsed so '>' inside a
// quoted value cannot become a place to put markup.
pub(crate) fn attributes(bytes: &[u8], mut at: usize) -> Option<(usize, Vec<Attribute>)> {
    let mut attrs = Vec::new();
    loop {
        while space(*bytes.get(at)?) {
            at += 1;
        }
        match bytes[at] {
            b'>' => return Some((at + 1, attrs)),
            b'/' if *bytes.get(at + 1)? == b'>' => return Some((at + 2, attrs)),
            _ => {}
        }
        let end = attr_end(bytes, at, |b| space(b) || matches!(b, b'/' | b'>' | b'='))?;
        let name = at..end;
        if name.is_empty()
            || bytes[name.clone()]
                .iter()
                .any(|b| matches!(b, b'\0' | b'\'' | b'"' | b'<'))
        {
            return None;
        }
        at = end;
        while space(*bytes.get(at)?) {
            at += 1;
        }
        let value = if bytes[at] == b'=' {
            at += 1;
            while space(*bytes.get(at)?) {
                at += 1;
            }
            if matches!(bytes[at], b'\'' | b'"') {
                let end = attr_end(bytes, at + 1, |b| b == bytes[at])?;
                let span = at + 1..end;
                at = end + 1;
                Some(span)
            } else {
                let end = attr_end(bytes, at, |b| space(b) || b == b'>')?;
                let span = at..end;
                at = end;
                Some(span)
            }
        } else {
            None
        };
        attrs.push(Attribute { name, value });
    }
}

pub(crate) struct Attribute {
    pub(crate) name: Range<usize>,
    pub(crate) value: Option<Range<usize>>,
}

// Parse character references in an encoding declaration. Ambiguous entity
// spellings leave the response on the CSS-only path.
fn attribute_value(bytes: &[u8]) -> Option<String> {
    let mut rest = std::str::from_utf8(bytes).ok()?;
    let mut value = String::new();
    while let Some((prefix, suffix)) = rest.split_once('&') {
        value.push_str(prefix);
        let (entity, next) = suffix.split_once(';')?;
        let c = match entity {
            "amp" | "AMP" => '&',
            "quot" | "QUOT" => '"',
            "apos" => '\'',
            "lt" | "LT" => '<',
            "gt" | "GT" => '>',
            entity if entity.starts_with('#') => {
                let entity = &entity[1..];
                let (radix, digits) = entity
                    .strip_prefix(['x', 'X'])
                    .map_or((10, entity), |s| (16, s));
                let raw = u32::from_str_radix(digits, radix).ok()?;
                char::from_u32(raw).filter(|c| c.is_ascii() && *c != '\0')?
            }
            _ => return None,
        };
        value.push(c);
        rest = next;
    }
    value.push_str(rest);
    value.is_ascii().then_some(value)
}

pub(crate) fn comment_end(bytes: &[u8]) -> Option<usize> {
    let offset = bytes.strip_prefix(b"<!--")?;
    if offset.starts_with(b">") || offset.starts_with(b"->") {
        return None;
    }
    let end = find_ascii(offset, b"-->")?;
    (find_ascii(&offset[..end], b"--!>").is_none()).then_some(end + 7)
}

/// Where in the beginning of a page the style goes.
#[derive(Debug, PartialEq, Eq)]
pub enum Place {
    /// After this many bytes: right after the tag that begins the head.
    At(usize),
    /// Not to be told yet from this much of the page.
    More,
    /// The page does not begin the way one with a head does. Whatever it
    /// is, it is not for us to put anything into.
    Nowhere,
}

/// Finds where the head of a page begins, going by nothing but what may
/// come before it: white space, comments, the document type and the tag
/// of the root. Anything else first, and the page has no head that was
/// written out: text put there could land in the middle of a script.
pub fn place(html: &[u8]) -> Place {
    let find = |from: usize, what: &[u8]| {
        html[from..]
            .windows(what.len())
            .position(|w| w == what)
            .map(|at| from + at + what.len())
    };
    if !html.is_empty() && html.len() < 3 && b"\xef\xbb\xbf".starts_with(html) {
        return Place::More;
    }
    let mut at = if html.starts_with(b"\xef\xbb\xbf") {
        3
    } else {
        0
    };
    loop {
        while html.get(at).copied().is_some_and(space) {
            at += 1;
        }
        match html.get(at) {
            None => return Place::More,
            Some(b'<') => {}
            Some(_) => return Place::Nowhere,
        }
        let rest = &html[at + 1..];
        if rest.starts_with(b"!--") {
            match comment_end(&html[at..]) {
                Some(end) => at += end,
                None => return Place::More,
            }
            continue;
        }
        if b"!--".starts_with(rest) {
            return Place::More;
        }
        let Some(len) = rest
            .iter()
            .position(|b| space(*b) || matches!(b, b'>' | b'/'))
        else {
            // No tag that may come before the head is this long.
            return if rest.len() > 16 {
                Place::Nowhere
            } else {
                Place::More
            };
        };
        let name = rest[..len].to_ascii_lowercase();
        if !matches!(name.as_slice(), b"!doctype" | b"html" | b"head") {
            return Place::Nowhere;
        }
        let end = if name == b"!doctype" {
            find(at + 1 + len, b">")
        } else {
            attributes(html, at + 1 + len).map(|(end, _)| end)
        };
        let Some(end) = end else { return Place::More };
        if name == b"head" {
            return Place::At(end);
        }
        at = end;
    }
}

pub(crate) fn meta_charset(tag: &[u8], attrs: &[Attribute]) -> Option<Option<String>> {
    let find = |name: &[u8]| {
        attrs
            .iter()
            .find(|a| tag[a.name.clone()].eq_ignore_ascii_case(name))
    };
    let value = |attr: &Attribute| -> Option<String> {
        attribute_value(attr.value.as_ref().map_or(b"", |r| &tag[r.clone()]))
    };
    if let Some(charset) = find(b"charset") {
        return Some(Some(value(charset)?));
    }
    if let (Some(equiv), Some(content)) = (find(b"http-equiv"), find(b"content"))
        && value(equiv)?.eq_ignore_ascii_case("content-type")
    {
        return Some(charset(&value(content)?).map(str::to_owned));
    }
    Some(None)
}

fn charset(content: &str) -> Option<&str> {
    content.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['\'', '"']))
    })
}

pub(crate) const MAX_CHARSET_PREFIX: usize = 16 << 10;

pub(crate) fn transport_charset(headers: &HeaderMap) -> Option<&str> {
    charset(headers.get(CONTENT_TYPE)?.to_str().ok()?)
}

/// A BOM takes precedence over the transport and document declarations.
/// An unknown or non-ASCII-compatible encoding makes a byte scan unsafe.
pub(crate) fn encoding(bytes: &[u8], label: Option<&str>) -> Option<Option<&'static Encoding>> {
    if bytes.starts_with(b"\x00\x00\xfe\xff") || bytes.starts_with(b"\xff\xfe\x00\x00") {
        return None;
    }
    if let Some((encoding, _)) = Encoding::for_bom(bytes) {
        return Some(Some(encoding));
    }
    match label {
        Some(label) => Some(Some(Encoding::for_label(label.as_bytes())?)),
        None => Some(None),
    }
}

pub(crate) struct Tag<'a> {
    pub(crate) name: &'a [u8],
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) end: usize,
    pub(crate) self_closed: bool,
    pub(crate) closing: bool,
}

pub(crate) fn is_tag(bytes: &[u8]) -> bool {
    bytes
        .strip_prefix(b"<")
        .map(|rest| rest.strip_prefix(b"/").unwrap_or(rest))
        .and_then(|rest| rest.first())
        .is_some_and(u8::is_ascii_alphabetic)
}

pub(crate) fn tag(bytes: &[u8]) -> Option<Tag<'_>> {
    let closing = bytes.get(1) == Some(&b'/');
    let start = if closing { 2 } else { 1 };
    let name_end = attr_end(bytes, start, |b| space(b) || matches!(b, b'/' | b'>'))?;
    let name = &bytes[start..name_end];
    let (end, attrs) = attributes(bytes, name_end)?;
    let self_closed = bytes.get(end - 2) == Some(&b'/')
        && attrs
            .last()
            .and_then(|attr| attr.value.as_ref())
            .is_none_or(|span| span.end < end - 1);
    Some(Tag {
        name,
        attrs,
        end,
        self_closed,
        closing,
    })
}

/// Return the start and end of the raw element's own closing tag.
pub(crate) fn raw_end(bytes: &[u8], name: &[u8]) -> Option<(usize, usize)> {
    let mut cursor = 0;
    while let Some(offset) = bytes[cursor..].iter().position(|&b| b == b'<') {
        let close = cursor + offset;
        let rest = &bytes[close + 1..];
        if let Some(tail) = rest.strip_prefix(b"/")
            && tail
                .get(..name.len())
                .is_some_and(|tag| tag.eq_ignore_ascii_case(name))
            && tail
                .get(name.len())
                .is_some_and(|&b| space(b) || matches!(b, b'>' | b'/'))
        {
            let (end, _) = attributes(&bytes[close..], 2 + name.len())?;
            return Some((close, close + end));
        }
        cursor = close + 1;
    }
    None
}

/// Comments, CDATA and declarations can contain apparent element tags.
pub(crate) fn declaration_end(bytes: &[u8]) -> Option<usize> {
    if bytes.starts_with(b"<!--") {
        return comment_end(bytes);
    }
    if let Some(rest) = bytes.strip_prefix(b"<![CDATA[") {
        return rest
            .windows(3)
            .position(|w| w == b"]]>")
            .map(|end| end + 12);
    }
    if bytes.starts_with(b"<?") {
        return bytes.windows(2).position(|w| w == b"?>").map(|end| end + 2);
    }
    let mut quote = None;
    let mut brackets = 0;
    for (at, byte) in bytes.iter().copied().enumerate().skip(2) {
        match byte {
            b if quote == Some(b) => quote = None,
            _ if quote.is_some() => {}
            b'\'' | b'"' => quote = Some(byte),
            b'[' => brackets += 1,
            b']' if brackets > 0 => brackets -= 1,
            b'>' if brackets == 0 => return Some(at + 1),
            _ => {}
        }
    }
    None
}

pub(crate) fn raw(name: &[u8]) -> bool {
    [
        b"script".as_slice(),
        b"style",
        b"textarea",
        b"title",
        b"xmp",
        b"iframe",
        b"noembed",
        b"noframes",
    ]
    .iter()
    .any(|raw| name.eq_ignore_ascii_case(raw))
}

/// Find a charset declaration in head markup, including an implicit head.
pub(crate) fn document_encoding(
    bytes: &[u8],
    transport: Option<&str>,
) -> Option<&'static Encoding> {
    let declared;
    let effective = if transport.is_some() || Encoding::for_bom(bytes).is_some() {
        transport
    } else {
        declared = match head_charset(&bytes[..bytes.len().min(MAX_CHARSET_PREFIX)], false) {
            HeadCharset::Found(label) => label,
            HeadCharset::More => None,
            HeadCharset::Ambiguous => return None,
        };
        declared.as_deref()
    };
    let encoding = encoding(bytes, effective)?.unwrap_or(encoding_rs::UTF_8);
    encoding.is_ascii_compatible().then_some(encoding)
}

pub(crate) enum HeadCharset {
    Found(Option<String>),
    More,
    Ambiguous,
}

/// Inspect head markup for a charset. Injection waits for an unambiguous
/// explicit head, while a whole-body scan can use a charset as soon as
/// it can read it, including in an implicit head.
pub(crate) fn head_charset(mut rest: &[u8], explicit: bool) -> HeadCharset {
    let mut charset = None;
    loop {
        while rest.first().copied().is_some_and(space) {
            rest = &rest[1..];
        }
        if rest.is_empty() {
            return HeadCharset::More;
        }
        if rest.starts_with(b"<!--")
            || (!explicit && (rest.starts_with(b"<!") || rest.starts_with(b"<?")))
        {
            let Some(end) = declaration_end(rest) else {
                return HeadCharset::More;
            };
            rest = &rest[end..];
            continue;
        }
        if b"<!--".starts_with(rest) {
            return HeadCharset::More;
        }
        if !rest.starts_with(b"<") {
            return HeadCharset::Found(charset);
        }
        let Some(tag) = tag(rest) else {
            return HeadCharset::More;
        };
        let name = tag.name.to_ascii_lowercase();
        match (tag.closing, name.as_slice()) {
            (true, b"head") | (false, b"body") => return HeadCharset::Found(charset),
            (false, b"base" | b"basefont" | b"bgsound" | b"link" | b"html" | b"head") => {}
            (false, b"meta") if charset.is_none() => {
                let Some(label) = meta_charset(rest, &tag.attrs) else {
                    return HeadCharset::Ambiguous;
                };
                charset = label;
                if charset.is_some() && !explicit {
                    return HeadCharset::Found(charset);
                }
            }
            (false, b"meta") => {}
            (false, b"script" | b"style" | b"title" | b"noframes" | b"noscript") => {
                let raw = &rest[tag.end..];
                let Some((close, end)) = raw_end(raw, tag.name) else {
                    return HeadCharset::More;
                };
                if explicit && name == b"script" && find_ascii(&raw[..close], b"<!--").is_some() {
                    return HeadCharset::Ambiguous;
                }
                rest = &raw[end..];
                continue;
            }
            // An explicit head with unfamiliar markup can change where
            // the browser applies subsequent declarations and policies.
            _ if explicit => return HeadCharset::Ambiguous,
            _ => return HeadCharset::Found(charset),
        }
        rest = &rest[tag.end..];
    }
}
