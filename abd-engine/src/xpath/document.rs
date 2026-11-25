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

use std::ops::Range;

use bytes::Bytes;
use roxmltree::{Document, ParsingOptions};

use super::XPath;
use crate::limits::{MAX_REWRITE_BODY, MAX_XML_NODES};

mod preflight;

/// Keep the original allocation if no range can be removed. Exhausting
/// the evaluation budget abandons the whole response rewrite.
pub(crate) fn prune(path: &XPath, input: Bytes, within_budget: &dyn Fn() -> bool) -> Option<Bytes> {
    if !within_budget() {
        return None;
    }
    let Some((prefix, doc)) = parse(&input, within_budget) else {
        return Some(input);
    };
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for item in path.select(&doc, within_budget).ok()? {
        if !within_budget() {
            return None;
        }
        if let Some(range) = item.removal(doc.input_text()) {
            let range = range.start + prefix..range.end + prefix;
            // Document order already puts enclosing elements before their
            // attributes and children. Coalesce adjacent cuts too.
            if let Some(last) = ranges.last_mut().filter(|last| range.start <= last.end) {
                last.end = last.end.max(range.end);
            } else {
                ranges.push(range);
            }
        }
    }
    Some(cut(input, ranges))
}

fn parse<'a>(input: &'a [u8], within_budget: &dyn Fn() -> bool) -> Option<(usize, Document<'a>)> {
    if input.len() > MAX_REWRITE_BODY {
        return None;
    }
    let source = preflight::check(std::str::from_utf8(input).ok()?, within_budget)?;
    let prefix = input.len() - source.len();
    let doc = Document::parse_with_options(
        source,
        ParsingOptions {
            nodes_limit: MAX_XML_NODES,
            ..ParsingOptions::default()
        },
    )
    .ok()?;
    let mut remaining = MAX_XML_NODES as usize;
    for node in doc.descendants() {
        if !within_budget() {
            return None;
        }
        remaining = remaining.checked_sub(1 + node.attributes().len())?;
    }
    Some((prefix, doc))
}

fn cut(input: Bytes, ranges: Vec<Range<usize>>) -> Bytes {
    if ranges.is_empty() {
        return input;
    }
    let removed: usize = ranges.iter().map(Range::len).sum();
    let mut output = Vec::with_capacity(input.len() - removed);
    let mut start = 0;
    for range in ranges {
        if !append(&mut output, &input[start..range.start]) {
            return input;
        }
        start = range.end;
    }
    if !append(&mut output, &input[start..]) {
        return input;
    }
    Bytes::from(output)
}

fn append(output: &mut Vec<u8>, part: &[u8]) -> bool {
    // Cutting markup between text nodes can create a forbidden CDATA end
    // delimiter across the join. Keep the original in that case.
    if (output.ends_with(b"]]") && part.starts_with(b">"))
        || (output.ends_with(b"]") && part.starts_with(b"]>"))
    {
        return false;
    }
    output.extend_from_slice(part);
    true
}
