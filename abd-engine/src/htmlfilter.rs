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

//! Remove byte ranges from a whole HTML document without reserializing it.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;
use encoding_rs::{Encoding, UTF_8};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::html::{self, Tag};
use crate::limits::{MAX_HTML_DECODED, MAX_HTML_DEPTH, MAX_HTML_ELEMENTS};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Selector {
    pub(crate) tag: Option<String>,
    pub(crate) attributes: Vec<Attribute>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Attribute {
    pub(crate) name: String,
    pub(crate) value: Option<AttributeValue>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum AttributeValue {
    Contains(String),
    StartsWith(String),
    EndsWith(String),
}

impl AttributeValue {
    fn matches(&self, value: &str) -> bool {
        match self {
            Self::Contains(text) => value.contains(text),
            Self::StartsWith(text) => value.starts_with(text),
            Self::EndsWith(text) => value.ends_with(text),
        }
    }
}

/// Shared selector steps and the compiled content predicates of the last step.
pub(crate) struct Compiled {
    pub(crate) steps: Vec<Selector>,
    pub(crate) content: Vec<Content>,
}

pub(crate) enum Content {
    Contains(String),
    Matches(Regex),
    Glob(Regex),
    Length {
        min: Option<usize>,
        max: Option<usize>,
        allow_newlines: bool,
    },
}

pub(crate) fn apply(
    body: Bytes,
    rules: &[Arc<Compiled>],
    charset: Option<&str>,
    within_budget: &dyn Fn() -> bool,
) -> Option<Bytes> {
    if rules.is_empty() {
        return Some(body);
    }
    let Some(encoding) = html::document_encoding(&body, charset) else {
        return Some(body);
    };
    let mut scan = Scan {
        input: &body,
        rules,
        encoding,
        within_budget,
        open: Vec::new(),
        removed: Vec::new(),
        elements: 0,
        decoded: 0,
    };
    scan.document()?;
    if scan.removed.is_empty() {
        return Some(body);
    }
    let removed: usize = scan.removed.iter().map(Range::len).sum();
    let mut output = Vec::with_capacity(body.len() - removed);
    let mut cursor = 0;
    for range in scan.removed {
        output.extend_from_slice(&body[cursor..range.start]);
        cursor = range.end;
    }
    output.extend_from_slice(&body[cursor..]);
    Some(Bytes::from(output))
}

struct Element<'a> {
    tag: Tag<'a>,
    start: usize,
}

struct Scan<'a> {
    input: &'a [u8],
    rules: &'a [Arc<Compiled>],
    encoding: &'static Encoding,
    within_budget: &'a dyn Fn() -> bool,
    open: Vec<Element<'a>>,
    removed: Vec<Range<usize>>,
    elements: usize,
    decoded: usize,
}

impl<'a> Scan<'a> {
    fn document(&mut self) -> Option<()> {
        let mut at = 0;
        let mut overflow: Option<Overflow<'_>> = None;
        while let Some(offset) = self.input[at..].iter().position(|&b| b == b'<') {
            if !(self.within_budget)() {
                return None;
            }
            at += offset;
            let rest = &self.input[at..];
            if rest.starts_with(b"<!") || rest.starts_with(b"<?") {
                let Some(end) = html::declaration_end(rest) else {
                    break;
                };
                at += end;
                continue;
            }
            if !html::is_tag(rest) {
                at += 1;
                continue;
            }
            let Some(tag) = html::tag(rest) else { break };
            let start = at;
            at += tag.end;
            if let Some(suppressed) = &mut overflow {
                if suppressed.advance(&tag, self.input, &mut at)? {
                    overflow = None;
                } else if tag.closing
                    && !tag.name.eq_ignore_ascii_case(suppressed.name)
                    && self
                        .open
                        .iter()
                        .any(|element| element.tag.name.eq_ignore_ascii_case(tag.name))
                {
                    // This may close a tracked ancestor through untracked
                    // elements. Leave its extent alone when it is uncertain.
                    return None;
                }
            } else if tag.closing {
                self.close(tag.name, start, at)?;
            } else if self.open.len() == MAX_HTML_DEPTH {
                if !tag.self_closed && !void(tag.name) {
                    if html::raw(tag.name) {
                        skip_raw(self.input, &tag, &mut at)?;
                    } else {
                        overflow = Some(Overflow {
                            name: tag.name,
                            depth: 1,
                        });
                    }
                }
            } else {
                let Some((inner_end, end)) = element_end(self.input, &tag, at) else {
                    if at == self.input.len() || html::raw(tag.name) {
                        break;
                    }
                    self.open.push(Element { tag, start });
                    continue;
                };
                self.test(Element { tag, start }, inner_end, end)?;
                at = end;
            }
        }
        (self.within_budget)().then_some(())
    }

    fn close(&mut self, name: &[u8], start: usize, end: usize) -> Option<()> {
        let Some(index) = self
            .open
            .iter()
            .rposition(|element| element.tag.name.eq_ignore_ascii_case(name))
        else {
            return Some(());
        };
        while self.open.len() > index + 1 {
            let element = self.open.pop()?;
            self.test(element, start, start)?;
        }
        let element = self.open.pop()?;
        self.test(element, start, end)
    }

    fn test(&mut self, element: Element<'a>, inner_end: usize, end: usize) -> Option<()> {
        self.elements += 1;
        if self.elements > MAX_HTML_ELEMENTS {
            return None;
        }
        let inner = &self.input[element.start + element.tag.end..inner_end];
        let mut decoded = None;
        let mut length = None;
        for rule in self.rules {
            let (target, parents) = rule.steps.split_last()?;
            if !matches_step(
                self.input,
                self.encoding,
                &mut self.decoded,
                &element,
                target,
            )? || self.open.len() < parents.len()
            {
                continue;
            }
            let mut ancestors_match = true;
            for (ancestor, step) in self.open.iter().rev().zip(parents.iter().rev()) {
                if !matches_step(self.input, self.encoding, &mut self.decoded, ancestor, step)? {
                    ancestors_match = false;
                    break;
                }
            }
            if !ancestors_match {
                continue;
            }
            if rule.content.is_empty() {
                self.remove(element.start..end);
                break;
            }
            let text = match &decoded {
                Some(text) => text,
                None => decoded.insert(decode(self.encoding, &mut self.decoded, inner)?),
            };
            let mut matches = true;
            for condition in &rule.content {
                if !(self.within_budget)() {
                    return None;
                }
                if !matches_content(condition, text, &mut length) {
                    matches = false;
                    break;
                }
            }
            if matches {
                self.remove(element.start..end);
                break;
            }
        }
        Some(())
    }

    fn remove(&mut self, range: Range<usize>) {
        // A match is seen at its end, after all matches it encloses.
        while self
            .removed
            .last()
            .is_some_and(|child| child.start >= range.start)
        {
            self.removed.pop();
        }
        self.removed.push(range);
    }
}

fn matches_step(
    input: &[u8],
    encoding: &'static Encoding,
    decoded: &mut usize,
    element: &Element<'_>,
    step: &Selector,
) -> Option<bool> {
    if step
        .tag
        .as_ref()
        .is_some_and(|name| !element.tag.name.eq_ignore_ascii_case(name.as_bytes()))
    {
        return Some(false);
    }
    let tag = &input[element.start..element.start + element.tag.end];
    for condition in &step.attributes {
        let Some(attr) =
            element.tag.attrs.iter().find(|attr| {
                tag[attr.name.clone()].eq_ignore_ascii_case(condition.name.as_bytes())
            })
        else {
            return Some(false);
        };
        let Some(expected) = &condition.value else {
            continue;
        };
        let value = attr
            .value
            .as_ref()
            .map_or(b"".as_slice(), |range| &tag[range.clone()]);
        let value = if encoding == UTF_8 || value.is_ascii() {
            match std::str::from_utf8(value) {
                Ok(text) => Cow::Borrowed(text),
                Err(_) => decode(encoding, decoded, value)?,
            }
        } else {
            decode(encoding, decoded, value)?
        };
        if !expected.matches(&value) {
            return Some(false);
        }
    }
    Some(true)
}

fn decode<'a>(
    encoding: &'static Encoding,
    decoded: &mut usize,
    bytes: &'a [u8],
) -> Option<Cow<'a, str>> {
    *decoded += bytes.len();
    if *decoded > MAX_HTML_DECODED {
        return None;
    }
    Some(encoding.decode_without_bom_handling(bytes).0)
}

fn matches_content(condition: &Content, text: &str, length: &mut Option<usize>) -> bool {
    match condition {
        Content::Contains(needle) => text.contains(needle),
        Content::Matches(regex) | Content::Glob(regex) => regex.is_match(text),
        Content::Length {
            min,
            max,
            allow_newlines,
        } => {
            let len = *length.get_or_insert_with(|| text.chars().count());
            min.is_none_or(|min| len >= min)
                && max.is_none_or(|max| len <= max)
                && (*allow_newlines || !text.contains('\n'))
        }
    }
}

fn void(name: &[u8]) -> bool {
    [
        b"area".as_slice(),
        b"base",
        b"br",
        b"col",
        b"embed",
        b"hr",
        b"img",
        b"input",
        b"link",
        b"meta",
        b"source",
        b"track",
        b"wbr",
    ]
    .iter()
    .any(|void| name.eq_ignore_ascii_case(void))
}

fn element_end(input: &[u8], tag: &Tag<'_>, after: usize) -> Option<(usize, usize)> {
    if tag.self_closed || void(tag.name) {
        return Some((after, after));
    }
    if html::raw(tag.name) {
        let (start, end) = html::raw_end(&input[after..], tag.name)?;
        return Some((after + start, after + end));
    }
    None
}

fn skip_raw(input: &[u8], tag: &Tag<'_>, at: &mut usize) -> Option<()> {
    *at += html::raw_end(&input[*at..], tag.name)?.1;
    Some(())
}

/// Inside a subtree beyond the depth limit, keep just the outer name and
/// count identical nested names so a same-name tag cannot resume testing.
struct Overflow<'a> {
    name: &'a [u8],
    depth: usize,
}

impl Overflow<'_> {
    fn advance(&mut self, tag: &Tag<'_>, input: &[u8], at: &mut usize) -> Option<bool> {
        if tag.closing {
            if tag.name.eq_ignore_ascii_case(self.name) {
                self.depth -= 1;
            }
        } else if !tag.self_closed && !void(tag.name) {
            if html::raw(tag.name) {
                skip_raw(input, tag, at)?;
            } else if tag.name.eq_ignore_ascii_case(self.name) {
                self.depth += 1;
            }
        }
        Some(self.depth == 0)
    }
}

#[cfg(test)]
mod tests;
