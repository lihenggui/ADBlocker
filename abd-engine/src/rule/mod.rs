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

//! Read a list line and select the engine that can handle it.

use std::borrow::Cow;

use adblock::filters::cosmetic::CosmeticFilterMask;
use adblock::filters::network::NetworkFilter;
use adblock::lists::{FilterParseError, ParseOptions, ParsedLine, RuleTypes, parse_filter};

use self::modifiers::{Kind as ModifierKind, PageModifier, lookup, may_have_owned_options};
use self::options::{OptionToken, Options, Quoting, unescaped};

pub(crate) mod action;
mod adguard;
mod comment;
pub(crate) mod html;
pub(crate) mod javascript;
mod modifiers;
pub(crate) mod options;
mod regex;
mod response;

pub use comment::{count, is_comment};
pub(crate) use modifiers::kind_bit;
pub(crate) use response::{Action, ResponseRule};

pub(crate) struct Rule<'a> {
    pub(crate) line: &'a str,
    pub(crate) kind: Kind<'a>,
    literal: Option<Options<'a>>,
    hosts_file: Option<&'a str>,
    translated: Option<adguard::Translation>,
}

pub(crate) enum Kind<'a> {
    Comment,
    Matcher,
    App(AppRule<'a>),
    Response(ResponseRule<'a>),
    PageException(PageException<'a>),
    Cosmetic,
    Html(html::Rule),
    NotApplied(Reason),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    Modifier(&'static str),
    HtmlFiltering(html::Rejection),
    AdGuard(adguard::Unsupported),
}

pub(crate) struct AppRule<'a> {
    options: Options<'a>,
    page: Option<PageException<'a>>,
}

/// The app value and the remaining matcher options after validation.
pub(crate) struct AppCondition<'a> {
    pub(crate) pattern: &'a str,
    pub(crate) apps: &'a str,
    pub(crate) options: Vec<&'a str>,
}

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

pub(crate) fn read(line: &str) -> Rule<'_> {
    read_with_scripts(line, None)
}

pub(crate) fn read_with_scripts<'a>(
    line: &'a str,
    scripts: Option<&mut javascript::Scripts<'a>>,
) -> Rule<'a> {
    let line = line.trim();
    let mut rule = Rule {
        line,
        kind: Kind::Matcher,
        literal: None,
        hosts_file: hosts_rule(line),
        translated: None,
    };
    if is_comment(line) {
        rule.kind = Kind::Comment;
        return rule;
    }
    if let Some(fields) = html::fields(line) {
        rule.kind = match html::read(line, fields) {
            Ok(parsed) => Kind::Html(parsed),
            Err(reason) => Kind::NotApplied(Reason::HtmlFiltering(reason)),
        };
        return rule;
    }
    if javascript::uses_reserved_name(line) {
        rule.kind = Kind::NotApplied(Reason::AdGuard(adguard::Unsupported::JavaScript(
            javascript::Rejection::ReservedName,
        )));
        return rule;
    }
    if let Some(translated) = adguard::translate(line, scripts) {
        match translated {
            Ok(translated) => {
                rule.translated = Some(translated);
                rule.kind = Kind::Cosmetic;
            }
            Err(reason) => rule.kind = Kind::NotApplied(Reason::AdGuard(reason)),
        }
        return rule;
    }
    // Only engine-owned modifiers need their values scanned past escaped
    // separators. Ordinary matcher options retain the matcher's spelling.
    let custom = may_have_owned_options(line)
        .then(|| Options::read(line, Quoting::AdGuard))
        .flatten()
        .filter(|options| network_condition(line, options.pattern));
    let custom = match custom.map(ResponseRule::read) {
        Some(Ok(response)) => {
            rule.kind = Kind::Response(response);
            return rule;
        }
        Some(Err(options)) if options.tokens().any(|token| token.name() == options::APP) => {
            Some(options)
        }
        _ => None,
    };
    let page = if rule.exception() {
        if let Some(options) = custom.as_ref().filter(|options| options.also_literal()) {
            PageException::read(options)
        } else {
            rule.literal = Options::read(line, Quoting::Literal);
            rule.literal.as_ref().and_then(PageException::read)
        }
    } else {
        None
    };
    rule.kind = if let Some(options) = custom {
        Kind::App(AppRule { options, page })
    } else if let Some(page) = page {
        Kind::PageException(page)
    } else if cosmetic(line) {
        Kind::Cosmetic
    } else if let Some(name) = rule.unsupported_modifier() {
        Kind::NotApplied(Reason::Modifier(name))
    } else {
        Kind::Matcher
    };
    rule
}

impl<'a> Rule<'a> {
    pub(crate) fn exception(&self) -> bool {
        self.line.starts_with("@@")
    }

    pub(crate) fn literal_options(&self) -> impl Iterator<Item = OptionToken<'a>> + '_ {
        let options = self.literal.as_ref().or_else(|| match &self.kind {
            Kind::App(AppRule { options, .. }) if self.exception() && options.also_literal() => {
                Some(options)
            }
            _ => None,
        });
        options.into_iter().flat_map(Options::tokens)
    }

    pub(crate) fn page_exception(&self) -> Option<&PageException<'a>> {
        match &self.kind {
            Kind::PageException(page) => Some(page),
            Kind::App(app) => app.page.as_ref(),
            _ => None,
        }
    }

    pub(crate) fn is_scriptlet(&self) -> bool {
        if let Some(translated) = &self.translated {
            return translated.scriptlet;
        }
        self.line
            .split_once('#')
            .is_some_and(|(_, suffix)| suffix.starts_with("#+js(") || suffix.starts_with("?#+js("))
    }

    pub(crate) fn matcher_input(&self) -> Option<Cow<'_, str>> {
        if let Some(translated) = &self.translated {
            return Some(Cow::Borrowed(&translated.line));
        }
        match self.kind {
            Kind::App(_)
            | Kind::Response(_)
            | Kind::Html(_)
            | Kind::NotApplied(Reason::AdGuard(_) | Reason::HtmlFiltering(_)) => None,
            _ => Some(self.normalized()),
        }
    }

    /// The original diagnostic index contains every nonempty normalized
    /// line, including lines that cannot match by themselves.
    pub(crate) fn normalized(&self) -> Cow<'a, str> {
        self.hosts_file
            .map(|host| Cow::Owned(format!("||{host}^")))
            .unwrap_or(Cow::Borrowed(self.line))
    }

    pub(crate) fn whole_host(&self) -> Option<(Cow<'a, str>, &'a str)> {
        whole_host(self.line, self.hosts_file)
    }

    pub(crate) fn has_tag(&self) -> bool {
        self.literal_options().any(|token| token.name() == "tag")
    }

    pub(crate) fn destinations(&self) -> impl Iterator<Item = &'a str> {
        self.literal_options()
            .filter(|token| token.name() == "to")
            .filter_map(|token| token.value)
            .flat_map(|domains| domains.split('|'))
    }

    pub(crate) fn http_only_anchor(&self) -> bool {
        self.line.starts_with("@@|http://")
    }

    pub(crate) fn exception_host(&self) -> Option<ExceptionHost<'a>> {
        let pattern = self.line.strip_prefix("@@")?;
        if let Some(host) = pattern.strip_prefix("||") {
            return Some(ExceptionHost::Anchored(host_token(host)));
        }
        let pattern = pattern.strip_prefix('|').unwrap_or(pattern);
        let authority = ["https://", "http://", "://"]
            .into_iter()
            .find_map(|scheme| pattern.strip_prefix(scheme));
        let absolute = (pattern.starts_with("https://") || pattern.starts_with("http://"))
            && authority.is_some_and(|host| !host.starts_with('/'));
        Some(ExceptionHost::Leading {
            host: host_token(authority.unwrap_or(pattern).trim_start_matches('/')),
            has_scheme: authority.is_some(),
            absolute,
        })
    }

    pub(crate) fn network(&self) -> Option<NetworkFilter<'a>> {
        if self.translated.is_some()
            || matches!(
                self.kind,
                Kind::App(_)
                    | Kind::Response(_)
                    | Kind::Html(_)
                    | Kind::NotApplied(Reason::AdGuard(_) | Reason::HtmlFiltering(_))
            )
        {
            return None;
        }
        parse_network(self.line)
    }

    /// Even a page modifier or a known unsupported modifier can occur
    /// on a line the matcher accepts. The CLI keeps its existing verdict.
    pub(crate) fn matcher_check(&self) -> MatcherCheck {
        if let Some(translated) = &self.translated {
            return if translated.requires_trusted_scriptlets {
                MatcherCheck::Cosmetic
            } else {
                MatcherCheck::AppliedCosmetic
            };
        }
        if matches!(self.kind, Kind::Cosmetic)
            || (matches!(self.kind, Kind::NotApplied(Reason::HtmlFiltering(_)))
                && (self.line.contains("$$") || cosmetic(self.line)))
        {
            return MatcherCheck::Cosmetic;
        }
        match parse_filter(self.line, false, ParseOptions::default()) {
            Ok(ParsedLine::Network(_)) => MatcherCheck::Network,
            Ok(ParsedLine::Cosmetic(_)) | Err(FilterParseError::Cosmetic(_)) => {
                MatcherCheck::Cosmetic
            }
            Err(FilterParseError::Unsupported) if self.line.contains("$$") => {
                MatcherCheck::Cosmetic
            }
            Err(_) => MatcherCheck::Invalid,
        }
    }

    fn unsupported_modifier(&self) -> Option<&'static str> {
        // Prefer the existing tokens when the guard projection needed them.
        let find = |token: OptionToken<'_>| {
            let entry = lookup(token.name())?;
            (entry.kind == ModifierKind::Unsupported)
                .then(|| {
                    entry
                        .names
                        .iter()
                        .copied()
                        .find(|&name| name == token.name())
                })
                .flatten()
        };
        if self.literal.is_some() {
            self.literal_options().find_map(find)
        } else {
            let (_, suffix) = self.line.rsplit_once('$')?;
            options::literal_tokens(suffix).find_map(find)
        }
    }
}

pub(crate) enum ExceptionHost<'a> {
    Anchored(&'a str),
    Leading {
        host: &'a str,
        has_scheme: bool,
        absolute: bool,
    },
}

/// What a leading or anchored rule pattern says before the path.
fn host_token(pattern: &str) -> &str {
    &pattern[..pattern
        .find(['/', '^', '?', ':', '|', '$'])
        .unwrap_or(pattern.len())]
}

pub(crate) enum MatcherCheck {
    Network,
    AppliedCosmetic,
    Cosmetic,
    Invalid,
}

impl<'a> AppRule<'a> {
    pub(crate) fn condition(&self) -> Option<AppCondition<'a>> {
        let all = &self.options;
        if unescaped(all.suffix, b'$').next().is_some() {
            return None;
        }
        let mut apps = None;
        let mut options = Vec::new();
        for token in all.tokens() {
            if token.raw_name == options::APP
                && let Some(names) = token.value
            {
                // Two app conditions would have to hold together. A
                // projected source condition holds when one name does.
                if apps.replace(names).is_some() {
                    return None;
                }
            } else if lookup(token.name()).is_some_and(|m| m.kind == ModifierKind::Source) {
                return None;
            } else {
                options.push(token.raw);
            }
        }
        Some(AppCondition {
            pattern: all.pattern,
            apps: apps?,
            options,
        })
    }
}

impl<'a> PageException<'a> {
    fn read(options: &Options<'a>) -> Option<Self> {
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

fn network_condition(line: &str, pattern: &str) -> bool {
    !pattern.contains('#')
        || matches!(
            parse_filter(line, false, cosmetic_options()),
            Err(FilterParseError::Unsupported)
        )
}

fn cosmetic(line: &str) -> bool {
    line.contains('#')
        && !matches!(
            parse_filter(line, false, cosmetic_options()),
            Err(FilterParseError::Unsupported)
        )
}

/// The generic cosmetic engine takes its rules directly from the source
/// lists, with the default permissions of that matcher.
pub(crate) fn generic_cosmetic(line: &str) -> bool {
    if !line.contains('#') || html::fields(line).is_some() || javascript::uses_reserved_name(line) {
        return false;
    }
    matches!(parse_filter(line, false, cosmetic_options()), Ok(ParsedLine::Cosmetic(rule))
        if rule.mask.contains(CosmeticFilterMask::UNHIDE)
            || !rule.has_hostname_constraint()
            || rule.hidden_generic_rule().is_some()
    )
}

fn cosmetic_options() -> ParseOptions {
    ParseOptions {
        rule_types: RuleTypes::CosmeticOnly,
        ..ParseOptions::default()
    }
}

pub(crate) fn parse_network(line: &str) -> Option<NetworkFilter<'_>> {
    crate::public_suffix::install();
    let options = ParseOptions {
        rule_types: RuleTypes::NetworkOnly,
        ..ParseOptions::default()
    };
    match parse_filter(line, false, options).ok()? {
        ParsedLine::Network(rule) => Some(rule),
        _ => None,
    }
}

pub(crate) fn badfilter_candidate(line: &str) -> bool {
    line.trim().rsplit_once('$').is_some_and(|(_, suffix)| {
        options::literal_options(suffix).any(|option| option == "badfilter")
    })
}

fn registrable(host: &str) -> bool {
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && crate::public_suffix::Name::parse(host).is_some_and(|name| name.root().is_some())
}

fn hosts_rule(line: &str) -> Option<&str> {
    if !line.starts_with('0') && !line.starts_with('1') {
        return None;
    }
    let mut fields = line.split_ascii_whitespace();
    match (fields.next(), fields.next(), fields.next()) {
        (Some("0.0.0.0" | "127.0.0.1"), Some(host), None) if registrable(host) => Some(host),
        _ => None,
    }
}

/// A host anchor with no request conditions, or an equivalent hosts-file line.
fn whole_host<'a>(line: &'a str, hosts_file: Option<&'a str>) -> Option<(Cow<'a, str>, &'a str)> {
    if let Some(rest) = line.strip_prefix("@@").unwrap_or(line).strip_prefix("||") {
        let rest = rest.strip_suffix("$badfilter").unwrap_or(rest);
        let host = rest.strip_suffix('^')?;
        return registrable(host).then_some((Cow::Borrowed(line), host));
    }
    let host = hosts_file?;
    Some((Cow::Owned(format!("||{host}^")), host))
}

pub(crate) fn whole_pattern(line: &str) -> Option<(Cow<'_, str>, &str)> {
    let line = line.trim();
    whole_host(line, hosts_rule(line))
}

#[cfg(test)]
mod tests;
