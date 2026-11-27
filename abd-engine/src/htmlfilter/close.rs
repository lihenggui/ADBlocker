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

//! Close open elements at explicit end tags and implied HTML boundaries.

use super::{Element, Scan, tag_in};
use crate::html::Tag;

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

impl Scan<'_> {
    pub(super) fn may_close_through_overflow(&self, tag: &Tag<'_>, overflow: &[u8]) -> bool {
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

    pub(super) fn close_for_start(&mut self, name: &[u8], start: usize) -> Option<()> {
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

    pub(super) fn close(&mut self, name: &[u8], start: usize, end: usize) -> Option<()> {
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
}
