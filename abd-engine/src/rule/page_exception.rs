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

//! Recognize page exceptions and project their URL conditions.

use super::modifiers::{Kind as ModifierKind, PageModifier, lookup};
use super::options::{OptionToken, Options};

#[derive(Clone, Copy)]
pub(crate) enum Elements {
    All,
    Specific,
}

pub(crate) struct PageException<'a> {
    pattern: &'a str,
    pub(crate) document: bool,
    pub(crate) content: bool,
    pub(crate) urlblock: bool,
    pub(crate) script: bool,
    pub(crate) elements: Option<Elements>,
}

impl<'a> PageException<'a> {
    pub(super) fn read(options: &Options<'a>) -> Option<Self> {
        let (mut document, mut content, mut urlblock) = (false, false, false);
        let (mut element, mut specific, mut generic) = (false, false, false);
        let mut script = false;
        for token in options.tokens() {
            match page_modifier(token) {
                Some(PageModifier::Document) => document = true,
                Some(PageModifier::Content) => content = true,
                Some(PageModifier::Urlblock) => urlblock = true,
                Some(PageModifier::Element) => element = true,
                Some(PageModifier::Specific) => specific = true,
                Some(PageModifier::Generic) => generic = true,
                Some(PageModifier::Script) => script = true,
                None => (),
            }
        }
        let content = content && !document;
        let elements = if element || (specific && generic) {
            Some(Elements::All)
        } else if specific {
            Some(Elements::Specific)
        } else {
            None
        };
        (document || content || urlblock || elements.is_some() || script).then_some(Self {
            pattern: options.pattern,
            document,
            content,
            urlblock,
            script,
            elements,
        })
    }

    pub(crate) fn project(&self, options: impl Iterator<Item = OptionToken<'a>>) -> String {
        let mut projected = format!("{}$document", self.pattern);
        for token in options {
            // The bare extension flag targets separate userscripts.
            if page_modifier(token).is_none() && !(self.script && token.raw == "extension") {
                projected.push(',');
                projected.push_str(token.raw);
            }
        }
        projected
    }
}

fn page_modifier(token: OptionToken<'_>) -> Option<PageModifier> {
    let modifier = lookup(token.raw_name)?;
    match modifier.kind {
        ModifierKind::Page(page) if modifier.accepts_value(token.value) => Some(page),
        _ => None,
    }
}
