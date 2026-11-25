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

use self::modifiers::{
    APP, Kind as ModifierKind, NETWORK, PageModifier, lookup, may_have_owned_options,
};
use self::options::{OptionToken, Options, Quoting};
use self::quoting::unescaped;

pub(crate) mod action;
pub(crate) mod address;
mod adguard;
mod comment;
pub(crate) mod csp;
mod denyallow;
pub(crate) mod html;
pub(crate) mod javascript;
mod modifiers;
pub(crate) mod options;
pub(crate) mod preprocess;
pub(crate) mod quoting;
mod regex;
mod removeparam;
mod response;

pub use comment::{count, is_comment};
pub(crate) use denyallow::matcher_applies as denyallow_matcher_applies;
pub(crate) use modifiers::kind_bit;
pub(crate) use response::{Action, ResponseRule};

pub(crate) struct Rule<'a> {
    pub(crate) line: &'a str,
    pub(crate) kind: Kind<'a>,
    literal: Option<Options<'a>>,
    hosts_file: Option<&'a str>,
    translated: Option<adguard::Translation>,
    rewritten: Option<String>,
    noop: bool,
    projected_type: bool,
    noop_host: Option<&'a str>,
    pub(crate) denyallow: bool,
    pub(crate) redirect_exception: bool,
}

pub(crate) enum Kind<'a> {
    Comment,
    Matcher,
    App(AppRule<'a>),
    Address(address::AddressRule<'a>),
    Response(ResponseRule<'a>),
    PageException(PageException<'a>),
    Cosmetic,
    Html(html::Rule),
    NotApplied(Reason),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    Modifier(&'static str),
    DenyAllow(denyallow::Rejection),
    RegexSourceDomain,
    NetworkRegex,
    Csp(csp::Rejection),
    RemoveParam(removeparam::Rejection),
    Address(address::Rejection),
    HtmlFiltering(html::Rejection),
    AdGuard(adguard::Unsupported),
}

pub(crate) struct AppRule<'a> {
    options: Options<'a>,
    page: Option<PageException<'a>>,
    denyallow: Option<denyallow::Condition>,
    pub(crate) original: Option<&'a str>,
}

/// The app value and the remaining matcher options after validation.
pub(crate) struct AppCondition<'a, 'options> {
    pub(crate) pattern: &'a str,
    pub(crate) apps: &'a str,
    pub(crate) options: Vec<&'options str>,
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
        rewritten: None,
        noop: false,
        projected_type: false,
        noop_host: None,
        denyallow: false,
        redirect_exception: false,
    };
    if is_comment(line) {
        rule.kind = Kind::Comment;
        return rule;
    }
    // A regex condition can contain text that otherwise starts a cosmetic rule.
    let custom = may_have_owned_options(line)
        .then(|| Options::read(line, Quoting::AdGuard))
        .flatten();
    if let Some(address) = custom
        .as_ref()
        .and_then(|options| address::AddressRule::read(line, options))
    {
        rule.kind = match address {
            Ok(address) => Kind::Address(address),
            Err(reason) => Kind::NotApplied(Reason::Address(reason)),
        };
        return rule;
    }
    if let Some(fields) = html::fields(line) {
        rule.kind = match html::read(line, fields) {
            Ok(parsed) => Kind::Html(parsed),
            Err(reason) => Kind::NotApplied(Reason::HtmlFiltering(reason)),
        };
        return rule;
    }
    // An action value can contain the characters that mark a cosmetic rule.
    if custom.as_ref().is_some_and(|options| {
        !options.pattern.contains('#')
            && options.tokens().any(|option| {
                matches!(
                    option.name(),
                    modifiers::CSP | modifiers::HLS | modifiers::REMOVEPARAM
                ) || option.name() == modifiers::URLTRANSFORM
                    || option.name().eq_ignore_ascii_case(modifiers::REMOVEHEADER)
            })
    }) {
        rule.read_options(custom);
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
    rule.read_options(custom.filter(|options| network_condition(line, options.pattern)));
    rule
}

impl<'a> Rule<'a> {
    fn read_options(&mut self, custom: Option<Options<'a>>) {
        let line = self.line;
        let mut denyallow = None;
        let mut app = None;
        if let Some(options) = custom {
            self.noop = options.noop;
            if let Err(reason) = csp::validate(&options) {
                self.kind = Kind::NotApplied(Reason::Csp(reason));
                return;
            }
            if let Err(reason) = removeparam::validate(&options) {
                self.kind = Kind::NotApplied(Reason::RemoveParam(reason));
                return;
            }
            match network_qualifiers(&options) {
                Ok(condition) => denyallow = condition,
                Err(reason) => {
                    self.kind = Kind::NotApplied(reason);
                    return;
                }
            }
            match ResponseRule::read(options) {
                Ok(response) => {
                    self.kind = Kind::Response(response);
                    return;
                }
                Err(options) if options.tokens().any(|token| token.name() == APP) => {
                    app = Some(options);
                }
                Err(options) => {
                    self.rewrite_network(&options, denyallow.as_ref());
                }
            }
        }
        self.denyallow = denyallow.is_some();
        if app.is_none() && self.has_redirect_option() && !cosmetic(line) {
            let (redirect, bare) =
                rewrite_redirect_options(self.rewritten.as_deref().unwrap_or(line));
            self.rewritten = redirect.or(self.rewritten.take());
            self.redirect_exception = bare;
        }
        let page = if self.exception() && !self.redirect_exception {
            if let Some(options) = app.as_ref().filter(|options| options.also_literal()) {
                PageException::read(options)
            } else {
                self.literal = Options::read(line, Quoting::Literal);
                self.literal.as_ref().and_then(PageException::read)
            }
        } else {
            None
        };
        self.kind = if let Some(options) = app {
            Kind::App(AppRule {
                options,
                page,
                denyallow,
                original: self.original(),
            })
        } else if let Some(page) = page {
            Kind::PageException(page)
        } else if cosmetic(line) {
            Kind::Cosmetic
        } else if let Some(name) = self.unsupported_modifier() {
            Kind::NotApplied(Reason::Modifier(name))
        } else {
            Kind::Matcher
        };
    }

    fn rewrite_network(&mut self, options: &Options<'a>, denyallow: Option<&denyallow::Condition>) {
        self.rewritten = if let Some(condition) = denyallow {
            Some(condition.rewrite(options))
        } else if let Some(projected) = csp::project(options) {
            self.projected_type = true;
            Some(projected)
        } else if options.noop
            && options.tokens().next().is_none()
            && let Some((line, host)) = whole_pattern(options.pattern)
        {
            self.noop_host = Some(host);
            Some(line.into_owned())
        } else {
            options.without_noops()
        };
        if removeparam::document_default(options) {
            let mut line = self
                .rewritten
                .take()
                .unwrap_or_else(|| self.line.to_owned());
            line.push_str(",document");
            self.rewritten = Some(line);
            self.projected_type = true;
        }
    }

    /// Text for diagnostics when matcher syntax differs from the list.
    pub(crate) fn original(&self) -> Option<&'a str> {
        (self.noop || self.denyallow || self.projected_type).then_some(self.line)
    }

    pub(crate) fn has_redirect_option(&self) -> bool {
        let Some((_, suffix)) = self.line.rsplit_once('$') else {
            return false;
        };
        suffix.contains("redirect")
            || suffix.contains("rewrite")
            || suffix.contains("empty")
            || suffix.contains("mp4")
    }

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
        if self.redirect_exception {
            return None;
        }
        if let Some(translated) = &self.translated {
            return Some(Cow::Borrowed(&translated.line));
        }
        match self.kind {
            Kind::App(_)
            | Kind::Address(_)
            | Kind::Response(_)
            | Kind::Html(_)
            | Kind::NotApplied(
                Reason::AdGuard(_)
                | Reason::HtmlFiltering(_)
                | Reason::DenyAllow(_)
                | Reason::RegexSourceDomain
                | Reason::NetworkRegex
                | Reason::Csp(_)
                | Reason::RemoveParam(_)
                | Reason::Address(_),
            ) => None,
            _ => Some(self.normalized()),
        }
    }

    /// The original diagnostic index contains every nonempty normalized
    /// line, including lines that cannot match by themselves.
    pub(crate) fn normalized(&self) -> Cow<'_, str> {
        if let Some(line) = &self.rewritten {
            return Cow::Borrowed(line);
        }
        self.hosts_file
            .map(|host| Cow::Owned(format!("||{host}^")))
            .unwrap_or(Cow::Borrowed(self.line))
    }

    pub(crate) fn whole_host(&self) -> Option<(Cow<'_, str>, &'a str)> {
        if matches!(
            self.kind,
            Kind::Address(_) | Kind::NotApplied(Reason::Address(_))
        ) {
            return None;
        }
        if let Some(host) = self.noop_host {
            return Some((Cow::Borrowed(self.rewritten.as_deref()?), host));
        }
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

    pub(crate) fn network(&self) -> Option<NetworkFilter<'_>> {
        if self.translated.is_some()
            || matches!(
                self.kind,
                Kind::App(_)
                    | Kind::Address(_)
                    | Kind::Response(_)
                    | Kind::Html(_)
                    | Kind::NotApplied(
                        Reason::AdGuard(_)
                            | Reason::HtmlFiltering(_)
                            | Reason::DenyAllow(_)
                            | Reason::RegexSourceDomain
                            | Reason::NetworkRegex
                            | Reason::Csp(_)
                            | Reason::RemoveParam(_)
                            | Reason::Address(_)
                    )
            )
        {
            return None;
        }
        parse_network(self.rewritten.as_deref().unwrap_or(self.line))
    }

    /// Even a page modifier or a known unsupported modifier can occur
    /// on a line the matcher accepts. The CLI keeps its existing verdict.
    pub(crate) fn matcher_check(&self) -> MatcherCheck {
        if matches!(
            self.kind,
            Kind::NotApplied(
                Reason::DenyAllow(_)
                    | Reason::RegexSourceDomain
                    | Reason::NetworkRegex
                    | Reason::Csp(_)
                    | Reason::RemoveParam(_)
                    | Reason::Address(_)
            )
        ) {
            return MatcherCheck::Invalid;
        }
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
        let line = self.rewritten.as_deref().unwrap_or(self.line);
        match parse_filter(line, false, ParseOptions::default()) {
            Ok(ParsedLine::Network(rule)) => {
                if self.denyallow && !denyallow::matcher_applies(line, rule) {
                    MatcherCheck::Invalid
                } else {
                    MatcherCheck::Network
                }
            }
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

fn network_qualifiers(options: &Options<'_>) -> Result<Option<denyallow::Condition>, Reason> {
    let denyallow = denyallow::Condition::read(options).map_err(Reason::DenyAllow)?;
    if options.noop
        && options
            .tokens()
            .any(|token| options::source_regex_end(token.raw).is_some())
    {
        return Err(Reason::RegexSourceDomain);
    }
    if options.noop
        && !regex::network_pattern_supported(
            options.pattern,
            options.tokens().any(|token| token.raw_name == "match-case"),
        )
    {
        return Err(Reason::NetworkRegex);
    }
    Ok(denyallow)
}

pub(crate) const REDIRECT_EXCEPTION: &str = "redirect=@abd-all-redirects";

fn rewrite_redirect_options(line: &str) -> (Option<String>, bool) {
    let Some((pattern, suffix)) = line.rsplit_once('$') else {
        return (None, false);
    };
    let exception = pattern.starts_with("@@");
    let rewrite = |token: OptionToken<'_>| {
        let modifier = lookup(token.raw_name)?;
        if !modifier.accepts_value(token.value) {
            return None;
        }
        match modifier.kind {
            ModifierKind::LegacyRedirect(option) => Some(option),
            ModifierKind::Redirect if exception && token.value.is_none() => {
                Some(REDIRECT_EXCEPTION)
            }
            _ => None,
        }
    };
    if !options::literal_tokens(suffix).any(|token| rewrite(token).is_some()) {
        return (None, false);
    }
    let mut rewritten = format!("{pattern}$");
    let mut bare = false;
    for (index, token) in options::literal_tokens(suffix).enumerate() {
        if index != 0 {
            rewritten.push(',');
        }
        let replacement = rewrite(token);
        let option = replacement.unwrap_or(token.raw);
        bare |= replacement == Some(REDIRECT_EXCEPTION);
        rewritten.push_str(option);
    }
    (Some(rewritten), bare)
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
    pub(crate) fn has_denyallow(&self) -> bool {
        self.denyallow.is_some()
    }

    pub(crate) fn condition(&self) -> Option<AppCondition<'a, '_>> {
        let all = &self.options;
        if unescaped(all.suffix, b'$').next().is_some() {
            return None;
        }
        let mut apps = None;
        let mut options = Vec::new();
        for token in all.tokens() {
            if token.raw_name == APP
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
                options.push(
                    self.denyallow
                        .as_ref()
                        .map_or(token.raw, |condition| condition.option(token)),
                );
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
    if line.contains(NETWORK)
        && matches!(
            read(line).kind,
            Kind::Address(_) | Kind::NotApplied(Reason::Address(_))
        )
    {
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
