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
use regex_automata::meta::{Cache, Regex};
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
    Never,
    Exact(String),
    Word(String),
    DashPrefix(String),
    StartsWith(String),
    EndsWith(String),
}

impl AttributeValue {
    fn matches(&self, value: &str) -> bool {
        match self {
            Self::Contains(text) => value.contains(text),
            Self::Never => false,
            Self::Exact(text) => value == text,
            Self::Word(text) => value
                .split(html_space)
                .any(|word| !word.is_empty() && word == text),
            Self::DashPrefix(text) => {
                value == text
                    || value
                        .strip_prefix(text)
                        .is_some_and(|tail| tail.starts_with('-'))
            }
            Self::StartsWith(text) => value.starts_with(text),
            Self::EndsWith(text) => value.ends_with(text),
        }
    }
}

fn html_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0c')
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

impl Compiled {
    pub(crate) fn regex_cost(&self) -> (usize, usize) {
        self.content
            .iter()
            .filter_map(|condition| match condition {
                Content::Matches(regex) | Content::Glob(regex) => Some(regex.memory_usage()),
                _ => None,
            })
            .fold((0, 0), |(count, bytes), size| (count + 1, bytes + size))
    }
}

pub(crate) fn apply(
    body: Bytes,
    rules: &[Arc<Compiled>],
    charset: Option<&str>,
    xhtml: bool,
    within_budget: &dyn Fn() -> bool,
) -> Option<Bytes> {
    if rules.is_empty() {
        return Some(body);
    }
    let Some(encoding) = html::document_encoding(&body, charset) else {
        return Some(body);
    };
    let document = Decoded::new(&body, encoding, within_budget)?;
    let mut scan = Scan {
        input: &document.text,
        xhtml,
        rules,
        caches: (0..rules.len()).map(|_| Vec::new()).collect(),
        within_budget,
        open: Vec::new(),
        removed: Vec::new(),
        elements: 0,
    };
    scan.document()?;
    if scan.removed.is_empty() {
        return Some(body);
    }
    let mut output = Vec::with_capacity(body.len());
    let mut cursor = 0;
    for range in scan.removed {
        let start = document.original(range.start);
        if !append(&mut output, &body[cursor..start]) {
            return Some(body);
        }
        cursor = document.original(range.end);
    }
    if !append(&mut output, &body[cursor..]) {
        return Some(body);
    }
    Some(Bytes::from(output))
}

/// Scan decoded text once. Cuts are made only at markup boundaries,
/// so only changes in the byte offset at an angle bracket need recording.
struct Decoded<'a> {
    text: Cow<'a, str>,
    offsets: Vec<(usize, usize)>,
}

impl<'a> Decoded<'a> {
    fn new(
        body: &'a [u8],
        encoding: &'static Encoding,
        within_budget: &dyn Fn() -> bool,
    ) -> Option<Self> {
        if body.len() > MAX_HTML_DECODED {
            return None;
        }
        if (encoding == UTF_8 || body.is_ascii())
            && let Ok(text) = std::str::from_utf8(body)
        {
            return Some(Self {
                text: Cow::Borrowed(text),
                offsets: Vec::new(),
            });
        }
        let mut text = String::with_capacity(body.len());
        let mut offsets = Vec::new();
        let mut start = 0;
        let mut delta = 0isize;
        // ASCII-compatible encodings do not use '<' or '>' as trail bytes.
        for (at, &byte) in body
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b, b'<' | b'>'))
        {
            if !within_budget() {
                return None;
            }
            text.push_str(&encoding.decode_without_bom_handling(&body[start..at]).0);
            let next_delta = text.len().cast_signed() - at.cast_signed();
            if delta != next_delta {
                offsets.push((text.len(), at));
                delta = next_delta;
            }
            text.push(char::from(byte));
            start = at + 1;
        }
        text.push_str(&encoding.decode_without_bom_handling(&body[start..]).0);
        Some(Self {
            text: Cow::Owned(text),
            offsets,
        })
    }

    fn original(&self, at: usize) -> usize {
        let n = self
            .offsets
            .partition_point(|&(position, _)| position <= at);
        n.checked_sub(1).map_or(at, |index| {
            let (decoded, original) = self.offsets[index];
            original + (at - decoded)
        })
    }
}

fn append(output: &mut Vec<u8>, kept: &[u8]) -> bool {
    // A literal '<' beside a cut must not turn the following text into markup.
    if output.ends_with(b"<")
        && kept
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'/' | b'!' | b'?'))
    {
        return false;
    }
    output.extend_from_slice(kept);
    true
}

// Start-tag boundaries for omitted end tags, HTML Standard §13.1.2.4.
// Table starts also close the cells, rows and sections the parser leaves
// before reprocessing the new tag in its enclosing table insertion mode.
const TABLE_SECTIONS: &[&[u8]] = &[b"thead", b"tbody", b"tfoot"];
const TABLE_SECTION_STARTS: &[&[u8]] = &[
    b"caption",
    b"col",
    b"colgroup",
    b"thead",
    b"tbody",
    b"tfoot",
];
const TABLE_ROW_STARTS: &[&[u8]] = &[
    b"caption",
    b"col",
    b"colgroup",
    b"thead",
    b"tbody",
    b"tfoot",
    b"tr",
];
const TABLE_CELL_STARTS: &[&[u8]] = &[
    b"caption",
    b"col",
    b"colgroup",
    b"thead",
    b"tbody",
    b"tfoot",
    b"tr",
    b"td",
    b"th",
];
const START_TAG_CLOSURES: &[StartTagClosure] = &[
    StartTagClosure {
        open: &[b"li"],
        next: &[b"li"],
        scope: Scope::ListItem,
    },
    StartTagClosure {
        open: &[b"dt", b"dd"],
        next: &[b"dt", b"dd"],
        scope: Scope::Within(&[b"dl"]),
    },
    StartTagClosure {
        open: &[b"p"],
        next: &[
            b"address",
            b"article",
            b"aside",
            b"blockquote",
            b"details",
            b"div",
            b"dl",
            b"fieldset",
            b"figcaption",
            b"figure",
            b"footer",
            b"form",
            b"h1",
            b"h2",
            b"h3",
            b"h4",
            b"h5",
            b"h6",
            b"header",
            b"hgroup",
            b"hr",
            b"main",
            b"menu",
            b"nav",
            b"ol",
            b"p",
            b"pre",
            b"search",
            b"section",
            b"table",
            b"ul",
        ],
        scope: Scope::Button,
    },
    StartTagClosure {
        open: &[b"rt", b"rp"],
        next: &[b"rt", b"rp"],
        scope: Scope::Within(&[b"ruby", b"rtc"]),
    },
    StartTagClosure {
        open: &[b"option"],
        next: &[b"option", b"optgroup", b"hr"],
        scope: Scope::Within(&[b"select", b"datalist", b"optgroup"]),
    },
    StartTagClosure {
        open: &[b"optgroup"],
        next: &[b"optgroup", b"hr"],
        scope: Scope::Within(&[b"select", b"datalist"]),
    },
    StartTagClosure {
        open: TABLE_SECTIONS,
        next: TABLE_SECTION_STARTS,
        scope: Scope::Table,
    },
    StartTagClosure {
        open: &[b"tr"],
        next: TABLE_ROW_STARTS,
        scope: Scope::Table,
    },
    StartTagClosure {
        open: &[b"td", b"th"],
        next: TABLE_CELL_STARTS,
        scope: Scope::Table,
    },
    StartTagClosure {
        open: &[b"colgroup"],
        next: &[
            b"caption",
            b"colgroup",
            b"thead",
            b"tbody",
            b"tfoot",
            b"tr",
            b"td",
            b"th",
        ],
        scope: Scope::Table,
    },
    StartTagClosure {
        open: &[b"caption"],
        next: TABLE_CELL_STARTS,
        scope: Scope::Table,
    },
];

struct StartTagClosure {
    open: &'static [&'static [u8]],
    next: &'static [&'static [u8]],
    scope: Scope,
}

impl StartTagClosure {
    fn boundary(&self, stack: &[Element<'_>]) -> Option<usize> {
        stack
            .iter()
            .enumerate()
            .rev()
            .take_while(|(_, element)| !self.scope.stops_at(element.tag.name))
            .find_map(|(index, element)| tag_in(element.tag.name, self.open).then_some(index))
    }
}

enum Scope {
    ListItem,
    Button,
    Table,
    Within(&'static [&'static [u8]]),
}

impl Scope {
    // HTML Standard §13.2.4.2 list-item, button and table scopes. The
    // scanner stops at foreign roots to stay within the current namespace.
    fn stops_at(&self, name: &[u8]) -> bool {
        if tag_in(name, &[b"html", b"table", b"template", b"math", b"svg"]) {
            return true;
        }
        if matches!(self, Self::Table) {
            return false;
        }
        if tag_in(
            name,
            &[b"applet", b"caption", b"marquee", b"object", b"td", b"th"],
        ) {
            return true;
        }
        match self {
            Self::ListItem => tag_in(name, &[b"ol", b"ul", b"menu"]),
            Self::Button => name.eq_ignore_ascii_case(b"button"),
            Self::Within(containers) => tag_in(name, containers),
            Self::Table => false,
        }
    }
}

fn tag_in(name: &[u8], names: &[&[u8]]) -> bool {
    names
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

struct Element<'a> {
    tag: Tag<'a>,
    start: usize,
    foreign: bool,
}

struct Scan<'a> {
    input: &'a str,
    xhtml: bool,
    rules: &'a [Arc<Compiled>],
    caches: Vec<Vec<Option<Box<Cache>>>>,
    within_budget: &'a dyn Fn() -> bool,
    open: Vec<Element<'a>>,
    removed: Vec<Range<usize>>,
    elements: usize,
}

impl<'a> Scan<'a> {
    fn document(&mut self) -> Option<()> {
        let input = self.input.as_bytes();
        let mut at = 0;
        let mut overflow: Option<Overflow<'_>> = None;
        while let Some(offset) = input[at..].iter().position(|&b| b == b'<') {
            if !(self.within_budget)() {
                return None;
            }
            at += offset;
            let rest = &input[at..];
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
                if suppressed.advance(&tag, input, &mut at)? {
                    overflow = None;
                } else if self.may_close_through_overflow(&tag, suppressed.name) {
                    // An untracked element may change the extent of a tracked ancestor.
                    return None;
                }
            } else if tag.closing {
                self.close(tag.name, start, at)?;
            } else if !self.start_tag(tag, start, &mut at, &mut overflow)? {
                break;
            }
        }
        (self.within_budget)().then_some(())
    }

    fn start_tag(
        &mut self,
        tag: Tag<'a>,
        start: usize,
        at: &mut usize,
        overflow: &mut Option<Overflow<'a>>,
    ) -> Option<bool> {
        let input = self.input.as_bytes();
        let foreign = self.foreign(tag.name);
        let html = !foreign && !self.xhtml;
        if html {
            self.close_for_start(tag.name, start)?;
        }
        let complete = if html {
            void(tag.name)
        } else {
            tag.self_closed
        };
        let raw = html && html::raw(tag.name);
        if self.open.len() == MAX_HTML_DEPTH {
            if !complete {
                if raw {
                    skip_raw(input, &tag, at)?;
                } else {
                    *overflow = Some(Overflow {
                        name: tag.name,
                        depth: 1,
                        foreign: !html,
                    });
                }
            }
            return Some(true);
        }
        let element = Element {
            tag,
            start,
            foreign,
        };
        if complete {
            self.test(element, *at, *at)?;
        } else if raw {
            let Some((start, end)) = html::raw_end(&input[*at..], element.tag.name) else {
                return Some(false);
            };
            self.test(element, *at + start, *at + end)?;
            *at += end;
        } else if *at == input.len() {
            return Some(false);
        } else {
            self.open.push(element);
        }
        Some(true)
    }

    fn foreign(&self, name: &[u8]) -> bool {
        if tag_in(name, &[b"svg", b"math"]) {
            return true;
        }
        self.open.last().is_some_and(|parent| {
            parent.foreign
                && !tag_in(parent.tag.name, &[b"foreignobject", b"desc", b"title"])
                && (!tag_in(parent.tag.name, &[b"mi", b"mo", b"mn", b"ms", b"mtext"])
                    || tag_in(name, &[b"mglyph", b"malignmark"]))
        })
    }

    fn may_close_through_overflow(&self, tag: &Tag<'_>, overflow: &[u8]) -> bool {
        if tag.closing {
            !tag.name.eq_ignore_ascii_case(overflow)
                && self
                    .open
                    .iter()
                    .any(|element| element.tag.name.eq_ignore_ascii_case(tag.name))
        } else {
            START_TAG_CLOSURES
                .iter()
                .any(|rule| tag_in(tag.name, rule.next) && rule.boundary(&self.open).is_some())
        }
    }

    fn close_for_start(&mut self, name: &[u8], start: usize) -> Option<()> {
        for rule in START_TAG_CLOSURES
            .iter()
            .filter(|rule| tag_in(name, rule.next))
        {
            if let Some(index) = rule.boundary(&self.open) {
                self.close_implied(index, start)?;
            }
        }
        Some(())
    }

    fn close_implied(&mut self, len: usize, start: usize) -> Option<()> {
        while self.open.len() > len {
            let element = self.open.pop()?;
            self.test(element, start, start)?;
        }
        Some(())
    }

    fn close(&mut self, name: &[u8], start: usize, end: usize) -> Option<()> {
        let Some(index) = self
            .open
            .iter()
            .rposition(|element| element.tag.name.eq_ignore_ascii_case(name))
        else {
            return Some(());
        };
        self.close_implied(index + 1, start)?;
        let element = self.open.pop()?;
        self.test(element, start, end)
    }

    fn test(&mut self, element: Element<'a>, inner_end: usize, end: usize) -> Option<()> {
        self.elements += 1;
        if self.elements > MAX_HTML_ELEMENTS {
            return None;
        }
        let inner = &self.input[element.start + element.tag.end..inner_end];
        let mut length = None;
        for (rule, caches) in self.rules.iter().zip(&mut self.caches) {
            let (target, parents) = rule.steps.split_last()?;
            if !matches_step(self.input, &element, target) || self.open.len() < parents.len() {
                continue;
            }
            let mut ancestors_match = true;
            for (ancestor, step) in self.open.iter().rev().zip(parents.iter().rev()) {
                if !matches_step(self.input, ancestor, step) {
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
            let mut matches = true;
            for (index, condition) in rule.content.iter().enumerate() {
                if !(self.within_budget)() {
                    return None;
                }
                if !matches_content(condition, inner, &mut length, caches, index) {
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

fn matches_step(input: &str, element: &Element<'_>, step: &Selector) -> bool {
    if step
        .tag
        .as_ref()
        .is_some_and(|name| !element.tag.name.eq_ignore_ascii_case(name.as_bytes()))
    {
        return false;
    }
    let tag = &input[element.start..element.start + element.tag.end];
    step.attributes.iter().all(|condition| {
        let Some(attr) = element
            .tag
            .attrs
            .iter()
            .find(|attr| tag[attr.name.clone()].eq_ignore_ascii_case(&condition.name))
        else {
            return false;
        };
        condition.value.as_ref().is_none_or(|expected| {
            let value = attr.value.as_ref().map_or("", |range| &tag[range.clone()]);
            expected.matches(value)
        })
    })
}

fn matches_content(
    condition: &Content,
    text: &str,
    length: &mut Option<usize>,
    caches: &mut Vec<Option<Box<Cache>>>,
    index: usize,
) -> bool {
    match condition {
        Content::Contains(needle) => text.contains(needle),
        Content::Matches(regex) | Content::Glob(regex) => {
            if index >= caches.len() {
                caches.resize_with(index + 1, || None);
            }
            let cache = caches[index].get_or_insert_with(|| Box::new(regex.create_cache()));
            crate::list_regex::matches_with(regex, cache, text.as_bytes())
        }
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

fn skip_raw(input: &[u8], tag: &Tag<'_>, at: &mut usize) -> Option<()> {
    *at += html::raw_end(&input[*at..], tag.name)?.1;
    Some(())
}

/// Inside a subtree beyond the depth limit, keep just the outer name and
/// count identical nested names so a same-name tag cannot resume testing.
struct Overflow<'a> {
    name: &'a [u8],
    depth: usize,
    foreign: bool,
}

impl Overflow<'_> {
    fn advance(&mut self, tag: &Tag<'_>, input: &[u8], at: &mut usize) -> Option<bool> {
        if tag.closing {
            if tag.name.eq_ignore_ascii_case(self.name) {
                self.depth -= 1;
            }
        } else if !(if self.foreign {
            tag.self_closed
        } else {
            void(tag.name)
        }) {
            if !self.foreign && html::raw(tag.name) {
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
