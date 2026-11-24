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

use std::collections::HashMap;
use std::sync::LazyLock;

use super::action::ActionKind;
use super::options::{APP, OptionToken, PRUNE, REPLACE};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PageModifier {
    Document,
    Content,
    Urlblock,
    Element,
    Specific,
    Generic,
    Script,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    App,
    Type(u16),
    Source,
    Destination,
    Flag,
    Page(PageModifier),
    Action(ActionKind, Compatibility),
    Unsupported,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Compatibility {
    /// Qualifiers are removed and adblock-rust checks the rest with CSP.
    Matcher,
    /// Only explicitly supported qualifiers and matcher options are accepted.
    Only(&'static [Allowed]),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Allowed {
    name: &'static str,
    negatable: bool,
}

const fn allow(name: &'static str) -> Allowed {
    Allowed {
        name,
        negatable: false,
    }
}

const fn allow_negation(name: &'static str) -> Allowed {
    Allowed {
        name,
        negatable: true,
    }
}

const PRUNE_OPTIONS: &[Allowed] = &[
    allow(APP),
    allow("domain"),
    allow_negation("third-party"),
    allow_negation("first-party"),
    allow("all"),
    allow("important"),
    allow("match-case"),
    allow("xmlhttprequest"),
];

#[derive(Clone, Copy, Debug)]
enum Value {
    None,
    Required,
    Optional,
}

pub(super) struct Modifier {
    pub(super) names: &'static [&'static str],
    pub(super) kind: Kind,
    value: Value,
}

const fn modifier(names: &'static [&'static str], kind: Kind, value: Value) -> Modifier {
    Modifier { names, kind, value }
}

use Kind::{Action, App, Destination, Flag, Page, Source, Type, Unsupported};
use PageModifier::{Content, Document, Element, Generic, Script, Specific, Urlblock};
use Value::{None as FlagValue, Optional, Required};

static MODIFIERS: &[Modifier] = &[
    modifier(&[APP], App, Required),
    modifier(
        &[REPLACE],
        Action(ActionKind::Replace, Compatibility::Matcher),
        Optional,
    ),
    modifier(
        &[PRUNE],
        Action(ActionKind::Prune, Compatibility::Only(PRUNE_OPTIONS)),
        Optional,
    ),
    modifier(&["domain", "from"], Source, Required),
    modifier(&["third-party", "3p"], Source, FlagValue),
    modifier(&["first-party", "1p"], Source, FlagValue),
    modifier(&["to"], Destination, Required),
    modifier(&["tag"], Flag, Required),
    modifier(&["badfilter"], Flag, FlagValue),
    modifier(&["all"], Flag, FlagValue),
    modifier(&["important"], Flag, FlagValue),
    modifier(&["match-case"], Flag, FlagValue),
    modifier(&["document", "doc"], Page(Document), FlagValue),
    modifier(&["content"], Page(Content), FlagValue),
    modifier(&["urlblock"], Page(Urlblock), FlagValue),
    modifier(&["elemhide", "ehide"], Page(Element), FlagValue),
    modifier(&["specifichide", "shide"], Page(Specific), FlagValue),
    modifier(&["generichide", "ghide"], Page(Generic), FlagValue),
    modifier(&["jsinject"], Page(Script), FlagValue),
    // The bit numbers are part of the response rule cache.
    modifier(&["script"], Type(1 << 0), FlagValue),
    modifier(&["xmlhttprequest", "xhr"], Type(1 << 1), FlagValue),
    modifier(&["subdocument"], Type(1 << 3), FlagValue),
    modifier(&["image"], Type(1 << 4), FlagValue),
    modifier(&["stylesheet", "css"], Type(1 << 5), FlagValue),
    modifier(&["font"], Type(1 << 6), FlagValue),
    modifier(&["media"], Type(1 << 7), FlagValue),
    modifier(&["object"], Type(1 << 8), FlagValue),
    modifier(&["other"], Type(1 << 9), FlagValue),
    modifier(&["websocket"], Type(1 << 10), FlagValue),
    modifier(&["ping"], Type(1 << 11), FlagValue),
    modifier(&["popup"], Unsupported, FlagValue),
    modifier(&["popunder"], Unsupported, FlagValue),
    modifier(&["stealth"], Unsupported, Optional),
    modifier(&["extension"], Unsupported, Optional),
    modifier(&["network"], Unsupported, FlagValue),
    modifier(&["denyallow"], Unsupported, Required),
    modifier(&["cookie"], Unsupported, Optional),
    modifier(&["header"], Unsupported, Required),
    modifier(&["hls"], Unsupported, Required),
    modifier(&["xmlprune"], Unsupported, Required),
    modifier(&["removeheader"], Unsupported, Required),
    modifier(&["urltransform"], Unsupported, Required),
    modifier(&["referrerpolicy"], Unsupported, Required),
    modifier(&["strict-third-party", "strict3p"], Unsupported, FlagValue),
    modifier(&["strict-first-party", "strict1p"], Unsupported, FlagValue),
];

pub(super) fn may_have_owned_options(line: &str) -> bool {
    static OWNED_NAMES: LazyLock<Vec<&str>> = LazyLock::new(|| {
        MODIFIERS
            .iter()
            .filter(|modifier| matches!(modifier.kind, App | Action(..)))
            .flat_map(|modifier| modifier.names.iter().copied())
            .collect()
    });
    OWNED_NAMES.iter().any(|name| line.contains(name))
}

pub(super) fn lookup(name: &str) -> Option<&'static Modifier> {
    static BY_NAME: LazyLock<HashMap<&str, &Modifier>> = LazyLock::new(|| {
        MODIFIERS
            .iter()
            .flat_map(|modifier| modifier.names.iter().map(move |&name| (name, modifier)))
            .collect()
    });
    BY_NAME.get(name).copied()
}

pub(crate) fn kind_bit(name: &str) -> Option<u16> {
    match lookup(name)?.kind {
        Type(bit) => Some(bit),
        Page(Document) => Some(1 << 2),
        _ => None,
    }
}

impl Modifier {
    pub(super) fn accepts_value(&self, value: Option<&str>) -> bool {
        match self.value {
            FlagValue => value.is_none(),
            Required => value.is_some_and(|v| !v.is_empty()),
            Optional => true,
        }
    }
}

impl Compatibility {
    pub(super) fn accepts(self, token: OptionToken<'_>) -> bool {
        match self {
            Self::Matcher => true,
            Self::Only(allowed) => {
                let (name, negated) = token
                    .raw_name
                    .strip_prefix('~')
                    .map_or((token.raw_name, false), |name| (name, true));
                lookup(name).is_some_and(|modifier| {
                    modifier.accepts_value(token.value)
                        && allowed.iter().any(|entry| {
                            entry.name == modifier.names[0] && (!negated || entry.negatable)
                        })
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_modifier_name_resolves_to_its_entry() {
        for modifier in MODIFIERS {
            for name in modifier.names {
                assert!(std::ptr::eq(lookup(name).unwrap(), modifier), "{name}");
            }
        }
    }

    #[test]
    fn aliases_have_the_same_entry_and_type_bit() {
        for (name, alias) in [
            ("domain", "from"),
            ("third-party", "3p"),
            ("xmlhttprequest", "xhr"),
            ("document", "doc"),
            ("elemhide", "ehide"),
        ] {
            assert!(std::ptr::eq(lookup(name).unwrap(), lookup(alias).unwrap()));
            assert_eq!(kind_bit(name), kind_bit(alias));
        }
    }

    #[test]
    fn value_required_and_valueless_modifiers_reject_the_wrong_form() {
        let app = lookup("app").unwrap();
        assert!(!app.accepts_value(None));
        assert!(!app.accepts_value(Some("")));
        assert!(app.accepts_value(Some("com.reader")));
        let important = lookup("important").unwrap();
        assert!(important.accepts_value(None));
        assert!(!important.accepts_value(Some("yes")));
        let replace = lookup("replace").unwrap();
        assert!(replace.accepts_value(None));
        assert!(replace.accepts_value(Some("/a/b/")));
    }
}
