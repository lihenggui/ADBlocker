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

pub(super) const APP: &str = "app";
pub(super) const NETWORK: &str = "network";
pub(super) const DENYALLOW: &str = "denyallow";
pub(super) const PRUNE: &str = "jsonprune";
pub(super) const XMLPRUNE: &str = "xmlprune";
pub(super) const HLS: &str = "hls";
pub(super) const REPLACE: &str = "replace";
pub(super) const REMOVEHEADER: &str = "removeheader";
pub(super) const URLTRANSFORM: &str = "urltransform";
pub(super) const COOKIE: &str = "cookie";
pub(super) const CSP: &str = "csp";
pub(super) const REMOVEPARAM: &str = "removeparam";

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
    Address(Compatibility),
    Type(u16),
    Source,
    Destination,
    DenyAllow,
    Noop,
    Flag,
    Redirect,
    LegacyRedirect(&'static str),
    Page(PageModifier),
    Action(ActionKind, Compatibility),
    Unsupported,
    Inapplicable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Compatibility {
    /// Qualifiers are removed and adblock-rust checks the rest with CSP.
    Matcher,
    RemoveHeader,
    UrlTransform,
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
const PLAYLIST_OR_XML_OPTIONS: &[Allowed] = &[
    allow(APP),
    allow("domain"),
    allow_negation("third-party"),
    allow("important"),
    allow("match-case"),
    allow("xmlhttprequest"),
    // The reference permits the strict forms. Their matching semantics
    // are unavailable to HLS and XML pruning rules, as with jsonprune.
];
const COOKIE_OPTIONS: &[Allowed] = &[
    allow_negation("domain"),
    allow("important"),
    allow_negation("third-party"),
];
const NETWORK_OPTIONS: &[Allowed] = &[allow(NETWORK), allow(APP), allow("important")];

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

use Kind::{
    Action, Address, App, DenyAllow, Destination, Flag, Inapplicable, LegacyRedirect, Noop, Page,
    Redirect, Source, Type, Unsupported,
};
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
    modifier(
        &[HLS],
        Action(
            ActionKind::Hls,
            Compatibility::Only(PLAYLIST_OR_XML_OPTIONS),
        ),
        Optional,
    ),
    modifier(
        &[REMOVEHEADER],
        Action(ActionKind::RemoveHeader, Compatibility::RemoveHeader),
        Optional,
    ),
    modifier(
        &[URLTRANSFORM],
        Action(ActionKind::UrlTransform, Compatibility::UrlTransform),
        Optional,
    ),
    modifier(
        &[REMOVEPARAM],
        Action(ActionKind::RemoveParamException, Compatibility::Matcher),
        Optional,
    ),
    modifier(&["domain", "from"], Source, Required),
    modifier(&["third-party", "3p"], Source, FlagValue),
    modifier(&["first-party", "1p"], Source, FlagValue),
    modifier(&["to"], Destination, Required),
    modifier(&["method"], Flag, Required),
    modifier(&[DENYALLOW], DenyAllow, Required),
    modifier(&["_"], Noop, FlagValue),
    modifier(&["tag"], Flag, Required),
    modifier(&["badfilter"], Flag, FlagValue),
    modifier(&["all"], Flag, FlagValue),
    modifier(&["important"], Flag, FlagValue),
    modifier(&["match-case"], Flag, FlagValue),
    modifier(&["redirect", "redirect-rule"], Redirect, Optional),
    modifier(&["rewrite"], Redirect, Required),
    modifier(&["empty"], LegacyRedirect("redirect=nooptext"), FlagValue),
    modifier(&["mp4"], LegacyRedirect("redirect=noopmp4-1s"), FlagValue),
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
    modifier(&["popup"], Inapplicable, FlagValue),
    modifier(&["popunder"], Inapplicable, FlagValue),
    modifier(&["stealth"], Inapplicable, Optional),
    modifier(&["extension"], Inapplicable, Optional),
    modifier(
        &[NETWORK],
        Address(Compatibility::Only(NETWORK_OPTIONS)),
        FlagValue,
    ),
    modifier(
        &[COOKIE],
        Action(ActionKind::Cookie, Compatibility::Only(COOKIE_OPTIONS)),
        Optional,
    ),
    modifier(&["header"], Unsupported, Required),
    modifier(
        &[XMLPRUNE],
        Action(
            ActionKind::XmlPrune,
            Compatibility::Only(PLAYLIST_OR_XML_OPTIONS),
        ),
        Optional,
    ),
    modifier(&["referrerpolicy"], Unsupported, Required),
    modifier(&["strict-third-party", "strict3p"], Unsupported, FlagValue),
    modifier(&["strict-first-party", "strict1p"], Unsupported, FlagValue),
];

pub(super) fn may_have_owned_options(line: &str) -> bool {
    static OWNED_NAMES: LazyLock<Vec<&str>> = LazyLock::new(|| {
        MODIFIERS
            .iter()
            .filter(|modifier| matches!(modifier.kind, App | Address(..) | Action(..) | DenyAllow))
            .flat_map(|modifier| modifier.names.iter().copied())
            .filter(|name| *name != HLS)
            .collect()
    });
    line.contains(CSP)
        || line.contains("$_")
        || line.contains(",_")
        || line.contains("$hls")
        || line.contains(",hls")
        || OWNED_NAMES.iter().any(|name| line.contains(name))
        || line
            .split_once('$')
            .is_some_and(|(_, suffix)| suffix.split([',', '$']).any(is_removeheader_option))
}

pub(super) fn is_removeheader_option(text: &str) -> bool {
    text.split(['=', ',']).next().is_some_and(|name| {
        name.trim_start_matches('~')
            .eq_ignore_ascii_case(REMOVEHEADER)
    })
}

pub(super) fn lookup(name: &str) -> Option<&'static Modifier> {
    static BY_NAME: LazyLock<HashMap<&str, &Modifier>> = LazyLock::new(|| {
        MODIFIERS
            .iter()
            .flat_map(|modifier| modifier.names.iter().map(move |&name| (name, modifier)))
            .collect()
    });
    if name.starts_with('_') && name.bytes().all(|byte| byte == b'_') {
        BY_NAME.get("_").copied()
    } else {
        BY_NAME
            .get(name)
            .or_else(|| {
                name.eq_ignore_ascii_case(REMOVEHEADER)
                    .then(|| &BY_NAME[REMOVEHEADER])
            })
            .copied()
    }
}

pub(super) fn is_noop(text: &str) -> bool {
    text.starts_with('_') && lookup(text).is_some_and(|modifier| modifier.kind == Noop)
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
    pub(super) fn accepts(self, raw_name: &str, value: Option<&str>) -> bool {
        match self {
            Self::Matcher => true,
            Self::Only(_) | Self::RemoveHeader | Self::UrlTransform => {
                let (name, negated) = raw_name
                    .strip_prefix('~')
                    .map_or((raw_name, false), |name| (name, true));
                lookup(name).is_some_and(|modifier| {
                    modifier.accepts_value(value) && self.accepts_option(modifier, negated)
                })
            }
        }
    }

    fn accepts_option(self, modifier: &Modifier, negated: bool) -> bool {
        let name = modifier.names[0];
        match self {
            Self::Matcher => true,
            Self::Only(allowed) => allowed
                .iter()
                .any(|entry| entry.name == name && (!negated || entry.negatable)),
            Self::RemoveHeader => {
                matches!(modifier.kind, Type(_) | Page(Document))
                    || name == "third-party"
                    || (!negated
                        && matches!(
                            name,
                            APP | "domain"
                                | "strict-first-party"
                                | "strict-third-party"
                                | "important"
                                | "match-case"
                        ))
            }
            Self::UrlTransform => {
                matches!(modifier.kind, Type(_) | Page(Document))
                    || name == "third-party"
                    || (!negated
                        && matches!(
                            name,
                            "domain"
                                | "important"
                                | "match-case"
                                | "badfilter"
                                | "to"
                                | "method"
                                | DENYALLOW
                                | "all"
                        ))
                // Popup traffic cannot be identified by the HTTP engine.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_modifier_name_has_a_corpus_example() {
        use std::collections::HashSet;

        use crate::rule::options::{Options, Quoting};

        let text = include_str!("../../../testdata/rules/corpus.txt");
        let seen: HashSet<_> = text
            .lines()
            .filter(|line| !line.starts_with('!'))
            .filter_map(|line| Options::read(line, Quoting::AdGuard))
            .flat_map(|options| {
                options
                    .tokens()
                    .map(|token| token.name())
                    .collect::<Vec<_>>()
            })
            .collect();
        for name in MODIFIERS.iter().flat_map(|modifier| modifier.names) {
            // The reader strips valid noop tokens before exposing options.
            if *name == "_" {
                assert!(text.lines().any(|line| line.ends_with("$_")));
            } else {
                assert!(seen.contains(name), "missing corpus modifier {name}");
            }
        }
    }

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
