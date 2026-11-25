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

use std::borrow::Cow;
use std::ops::Range;

use roxmltree::{Attribute, Document, Node, NodeType};

use super::budget::{Budget, Result};
use super::{Axis, Test, xml_space};

#[derive(Clone, Copy)]
pub(super) struct Context<'a, 'input> {
    pub(super) item: Item<'a, 'input>,
    pub(super) position: usize,
    pub(super) size: usize,
}

impl<'a, 'input> Context<'a, 'input> {
    pub(super) fn root(doc: &'a Document<'input>) -> Self {
        Self {
            item: Item::Tree(doc.root()),
            position: 1,
            size: 1,
        }
    }

    pub(super) fn position(self) -> f64 {
        self.position as f64
    }

    pub(super) fn size(self) -> f64 {
        self.size as f64
    }
}

#[derive(Clone, Copy)]
pub(super) enum Item<'a, 'input> {
    Tree(Node<'a, 'input>),
    Attribute(Node<'a, 'input>, Attribute<'a, 'input>),
}

impl<'a, 'input: 'a> Item<'a, 'input> {
    pub(super) fn owner(self) -> Node<'a, 'input> {
        match self {
            Self::Tree(node) | Self::Attribute(node, _) => node,
        }
    }

    fn parent(self) -> Option<Node<'a, 'input>> {
        match self {
            Self::Tree(node) => node.parent(),
            Self::Attribute(owner, _) => Some(owner),
        }
    }

    pub(super) fn order(self) -> (u32, usize) {
        let attribute = match self {
            Self::Tree(_) => 0,
            Self::Attribute(_, attribute) => attribute.range().start + 1,
        };
        (self.owner().id().get(), attribute)
    }

    pub(super) fn local_name(self) -> &'a str {
        match self {
            Self::Tree(node) => node
                .pi()
                .map_or_else(|| node.tag_name().name(), |pi| pi.target),
            Self::Attribute(_, attr) => attr.name(),
        }
    }

    // The XPath 1.0 errata leave relative namespace names implementation-dependent.
    pub(super) fn namespace(self) -> &'a str {
        match self {
            Self::Tree(node) => node.tag_name().namespace(),
            Self::Attribute(_, attr) => attr.namespace(),
        }
        .unwrap_or_default()
    }

    pub(super) fn qualified_name(self) -> &'a str {
        // Multiple prefixes can bind the same URI. Read the actual spelling
        // from the source instead of looking up a prefix for the URI.
        let source = self.owner().document().input_text();
        let start = match self {
            Self::Tree(node) if node.is_element() => node.range().start + 1,
            Self::Attribute(_, attr) => attr.range().start,
            _ => return self.local_name(),
        };
        let name = &source[start..];
        &name[..name
            .find(|ch| xml_space(ch) || matches!(ch, '=' | '/' | '>'))
            .unwrap_or(name.len())]
    }

    pub(super) fn value(self, work: &mut Budget<'_>) -> Result<Cow<'a, str>> {
        let text = match self {
            Self::Attribute(_, attr) => attr.value(),
            Self::Tree(node) => match node.node_type() {
                NodeType::Root | NodeType::Element => {
                    let mut result = String::new();
                    for desc in node.descendants().skip(1) {
                        work.visit()?;
                        if desc.is_text() {
                            let text = desc.text().unwrap_or_default();
                            work.text(text.len())?;
                            result.push_str(text);
                        }
                    }
                    return Ok(Cow::Owned(result));
                }
                NodeType::Text => node.text().unwrap_or_default(),
                NodeType::Comment => {
                    return normalized_lines(node.text().unwrap_or_default(), work);
                }
                NodeType::PI => {
                    return normalized_lines(
                        node.pi().and_then(|pi| pi.value).unwrap_or_default(),
                        work,
                    );
                }
            },
        };
        Ok(Cow::Borrowed(text))
    }

    pub(super) fn removal(self, source: &str) -> Option<Range<usize>> {
        match self {
            Self::Attribute(_, attr) => {
                let range = attr.range();
                let start = source[..range.start].trim_end_matches(xml_space).len();
                Some(start..range.end)
            }
            Self::Tree(node)
                if node.is_root()
                    || (node.is_element()
                        && node.parent().is_some_and(|parent| parent.is_root())) =>
            {
                None
            }
            Self::Tree(node) if node.is_text() => {
                let parent = node.parent()?;
                let end = if let Some(next) = node.next_sibling() {
                    next.range().start
                } else {
                    // roxmltree merges text and CDATA but keeps only the first
                    // token's range. The closing tag delimits the last text.
                    let range = parent.range();
                    range.start + source[range].rfind('<')?
                };
                Some(node.range().start..end)
            }
            Self::Tree(node) => Some(node.range()),
        }
    }

    fn matches(self, axis: Axis, test: &Test) -> bool {
        let principal = match self {
            Self::Tree(node) => !matches!(axis, Axis::Attribute) && node.is_element(),
            Self::Attribute(..) => matches!(axis, Axis::Attribute),
        };
        match test {
            Test::Any => principal,
            Test::Name(name) => {
                principal && self.namespace().is_empty() && self.local_name() == name
            }
            Test::Node => {
                !matches!(self, Self::Tree(node) if node.is_text() && node.text() == Some(""))
            }
            Test::Text => {
                matches!(self, Self::Tree(node) if node.is_text() && node.text() != Some(""))
            }
            Test::Comment => matches!(self, Self::Tree(node) if node.is_comment()),
            Test::ProcessingInstruction(target) => matches!(self, Self::Tree(node) if
                node.pi().is_some_and(|pi| target.as_ref().is_none_or(|name| pi.target == name))),
        }
    }

    pub(super) fn axis(self, axis: Axis, test: &Test, work: &mut Budget<'_>) -> Result<Vec<Self>> {
        use Axis::*;
        let mut matches = Vec::new();
        let mut keep = |item: Self| {
            work.visit()?;
            if item.matches(axis, test) {
                matches.push(item);
            }
            Ok(())
        };
        if matches!(axis, SelfNode | AncestorOrSelf | DescendantOrSelf) {
            keep(self)?;
        }
        match axis {
            SelfNode => (),
            Parent => self
                .parent()
                .into_iter()
                .map(Self::Tree)
                .try_for_each(keep)?,
            Ancestor | AncestorOrSelf => {
                if let Some(parent) = self.parent() {
                    parent.ancestors().map(Self::Tree).try_for_each(keep)?;
                }
            }
            Following => following(self).map(Self::Tree).try_for_each(keep)?,
            Preceding => {
                if !self.owner().is_root() {
                    preceding(self.owner(), work, &mut matches, test)?;
                }
            }
            _ => {
                if let Self::Tree(node) = self {
                    match axis {
                        Attribute => node
                            .attributes()
                            .map(|attr| Self::Attribute(node, attr))
                            .try_for_each(keep)?,
                        Child => node.children().map(Self::Tree).try_for_each(keep)?,
                        Descendant | DescendantOrSelf => node
                            .descendants()
                            .skip(1)
                            .map(Self::Tree)
                            .try_for_each(keep)?,
                        FollowingSibling => node
                            .next_siblings()
                            .skip(1)
                            .map(Self::Tree)
                            .try_for_each(keep)?,
                        PrecedingSibling => node
                            .prev_siblings()
                            .skip(1)
                            .map(Self::Tree)
                            .try_for_each(keep)?,
                        _ => unreachable!("remaining XPath axes"),
                    }
                }
            }
        }
        Ok(matches)
    }
}

fn following<'a, 'input>(item: Item<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    let owner = item.owner();
    let first = match item {
        Item::Attribute(..) => owner.first_child(),
        Item::Tree(_) => None,
    }
    .or_else(|| owner.ancestors().find_map(|node| node.next_sibling()));
    let doc = owner.document();
    first
        .into_iter()
        .flat_map(move |node| (node.id().get()..).map_while(|id| doc.get_node(id.into())))
}

fn preceding<'a, 'input>(
    owner: Node<'a, 'input>,
    work: &mut Budget<'_>,
    matches: &mut Vec<Item<'a, 'input>>,
    test: &Test,
) -> Result<()> {
    let doc = owner.document();
    let mut ancestors = owner.ancestors().skip(1).peekable();
    for id in (1..owner.id().get()).rev() {
        work.visit()?;
        if ancestors.peek().is_some_and(|node| node.id().get() == id) {
            ancestors.next();
            continue;
        }
        let item = Item::Tree(doc.get_node(id.into()).expect("XPath preceding node id"));
        if item.matches(Axis::Preceding, test) {
            matches.push(item);
        }
    }
    Ok(())
}

fn normalized_lines<'a>(text: &'a str, work: &mut Budget<'_>) -> Result<Cow<'a, str>> {
    work.text(text.len())?;
    if !text.contains('\r') {
        return Ok(Cow::Borrowed(text));
    }
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((line, after)) = rest.split_once('\r') {
        work.check()?;
        result.push_str(line);
        result.push('\n');
        rest = after.strip_prefix('\n').unwrap_or(after);
    }
    result.push_str(rest);
    Ok(Cow::Owned(result))
}
