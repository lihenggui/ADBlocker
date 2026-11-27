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

use self::host_pattern::{host_pattern, host_token, hosts_rule, starts_with_ipv6_host, whole_host};
use self::modifiers::{APP, Kind as ModifierKind, NETWORK, lookup, may_have_owned_options};
use self::options::{OptionToken, Options, Quoting, literal_parts};
use self::quoting::unescaped;

pub(crate) mod action;
pub(crate) mod address;
mod adguard;
mod check;
mod comment;
pub(crate) mod csp;
mod denyallow;
pub(crate) mod driver;
mod host_pattern;
pub(crate) mod html;
pub(crate) mod javascript;
mod modifiers;
pub(crate) mod options;
mod page_exception;
mod permissions;
pub(crate) mod preprocess;
pub(crate) mod quoting;
mod reason;
mod regex;
mod removeparam;
mod response;

pub use comment::{count, is_comment};
pub(crate) use denyallow::matcher_applies as denyallow_matcher_applies;
pub(crate) use host_pattern::{
    ExceptionHost, canonical_host_options, is_bare_host_anchor, whole_pattern,
};
pub(crate) use modifiers::kind_bit;
pub(crate) use page_exception::{Elements, PageException};
pub use permissions::SourcePermissions;
pub(crate) use reason::{Code, RuleReason};
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
    Rewritten,
    /// A candidate for the dedicated redirect matcher.
    Redirect,
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
    Cookie(action::cookie::Rejection),
    Address(address::Rejection),
    HtmlFiltering(html::Rejection),
    AdGuard(adguard::Unsupported),
}

impl Reason {
    // Modifiers passed to the upstream matcher get its verdict.
    fn uses_matcher(&self) -> bool {
        match self {
            Self::Modifier(name) => *name != modifiers::XMLPRUNE,
            Self::DenyAllow(_)
            | Self::RegexSourceDomain
            | Self::NetworkRegex
            | Self::Csp(_)
            | Self::RemoveParam(_)
            | Self::Cookie(_)
            | Self::Address(_)
            | Self::HtmlFiltering(_)
            | Self::AdGuard(_) => false,
        }
    }
}

enum MatcherInput<'a> {
    Native(&'a str),
    Rewritten(&'a str),
    Hosts(&'a str),
    Translated(&'a str),
    RedirectOnly(&'a str),
    None,
}

pub(crate) struct AppRule<'a> {
    options: Options<'a>,
    page: Option<PageException<'a>>,
    denyallow: Option<denyallow::Condition>,
    pub(crate) original: &'a str,
}

/// The app value and the remaining matcher options after validation.
pub(crate) struct AppCondition<'a, 'options> {
    pub(crate) pattern: &'a str,
    pub(crate) apps: &'a str,
    pub(crate) options: Vec<&'options str>,
}

pub(crate) fn scriptlet_resources() -> &'static [adblock::resources::Resource] {
    static RESOURCES: std::sync::LazyLock<Vec<adblock::resources::Resource>> =
        std::sync::LazyLock::new(|| {
            serde_json::from_str(include_str!("../../resources/scriptlets.json"))
                .expect("bundled scriptlet resources are valid")
        });
    &RESOURCES
}

pub(crate) fn read(line: &str) -> Rule<'_> {
    read_with_scripts(line, None)
}

/// Preliminary test for exceptions that may need a slot in a bounded table.
pub(crate) fn may_reserve_exception(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("@@") && (line.contains("app=") || line.contains("network"))
        || line.contains("#@%#")
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
    if is_bare_host_anchor(line) {
        return rule;
    }
    // A regex condition can contain text that otherwise starts a cosmetic rule.
    let custom = may_have_owned_options(line)
        .then(|| Options::read(line, Quoting::AdGuard))
        .flatten();
    if custom.as_ref().is_some_and(|options| {
        options
            .tokens()
            .any(|token| token.name() == modifiers::XMLPRUNE)
            && (options::regex_option_separator(line).is_some()
                || network_condition(line, options.pattern))
    }) {
        rule.kind = Kind::NotApplied(Reason::Modifier(modifiers::XMLPRUNE));
        return rule;
    }
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
    if custom.as_ref().is_some_and(has_action_options) {
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
                    self.kind = match response.cookie_rejection() {
                        Some(reason) => Kind::NotApplied(Reason::Cookie(reason)),
                        None => Kind::Response(response),
                    };
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
        let redirect = app.is_none() && self.has_redirect_option() && !cosmetic(line);
        if redirect {
            self.rewrite_redirect();
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
                original: line,
            })
        } else if let Some(page) = page {
            Kind::PageException(page)
        } else if cosmetic(line) {
            Kind::Cosmetic
        } else if let Some(name) = self.unsupported_modifier() {
            Kind::NotApplied(Reason::Modifier(name))
        } else if redirect {
            Kind::Redirect
        } else if self.rewritten.is_some() || self.hosts_file.is_some() {
            Kind::Rewritten
        } else {
            Kind::Matcher
        };
    }

    fn rewrite_redirect(&mut self) {
        let (line, bare) = rewrite_redirect_options(self.rewritten.as_deref().unwrap_or(self.line));
        self.rewritten = line.or(self.rewritten.take());
        self.redirect_exception = bare;
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
        let Some((_, suffix)) = literal_parts(self.line) else {
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

    fn matcher_route(&self) -> MatcherInput<'_> {
        match &self.kind {
            Kind::Comment | Kind::Matcher => MatcherInput::Native(self.line),
            Kind::Rewritten | Kind::Redirect | Kind::Cosmetic | Kind::PageException(_) => {
                self.projected_matcher_input()
            }
            Kind::NotApplied(reason) if reason.uses_matcher() => self.projected_matcher_input(),
            Kind::App(_)
            | Kind::Address(_)
            | Kind::Response(_)
            | Kind::Html(_)
            | Kind::NotApplied(_) => MatcherInput::None,
        }
    }

    fn projected_matcher_input(&self) -> MatcherInput<'_> {
        if self.redirect_exception {
            MatcherInput::RedirectOnly(self.rewritten.as_deref().unwrap_or(self.line))
        } else if let Some(translated) = &self.translated {
            MatcherInput::Translated(&translated.line)
        } else if let Some(line) = &self.rewritten {
            MatcherInput::Rewritten(line)
        } else if let Some(host) = self.hosts_file {
            MatcherInput::Hosts(host)
        } else {
            MatcherInput::Native(self.line)
        }
    }

    pub(crate) fn matcher_input(&self) -> Option<Cow<'_, str>> {
        match self.matcher_route() {
            MatcherInput::Native(line)
            | MatcherInput::Rewritten(line)
            | MatcherInput::Translated(line) => {
                if matches!(self.kind, Kind::Cosmetic | Kind::Comment) {
                    Some(Cow::Borrowed(line))
                } else {
                    Some(canonical_host_options(line))
                }
            }
            MatcherInput::Hosts(host) => Some(host_pattern(host)),
            MatcherInput::RedirectOnly(_) | MatcherInput::None => None,
        }
    }

    /// The original diagnostic index contains every nonempty normalized
    /// line, including lines that cannot match by themselves.
    pub(crate) fn normalized(&self) -> Cow<'_, str> {
        if let Some(line) = &self.rewritten {
            return Cow::Borrowed(line);
        }
        self.hosts_file
            .map(host_pattern)
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
        const PREFIX: &str = "@@|http://";
        self.line
            .get(..PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PREFIX))
    }

    pub(crate) fn exception_host(&self) -> Option<ExceptionHost<'a>> {
        let pattern = self.line.strip_prefix("@@")?;
        if let Some(host) = pattern.strip_prefix("||") {
            return Some(if starts_with_ipv6_host(host) {
                ExceptionHost::Ipv6
            } else {
                ExceptionHost::Anchored(host_token(host))
            });
        }
        let anchored = pattern.starts_with('|');
        let pattern = pattern.strip_prefix('|').unwrap_or(pattern);
        let scheme = pattern.split_once("://").filter(|(scheme, _)| {
            scheme.is_empty()
                || scheme.eq_ignore_ascii_case("https")
                || scheme.eq_ignore_ascii_case("http")
        });
        let absolute =
            scheme.is_some_and(|(scheme, host)| !scheme.is_empty() && !host.starts_with('/'));
        let host = scheme.map_or(pattern, |(_, host)| host);
        // Unless anchored at the start of the URL, the literal can match
        // anywhere in it, such as in a query on another host.
        if anchored && scheme.is_some() && starts_with_ipv6_host(host) {
            return Some(ExceptionHost::Ipv6);
        }
        Some(ExceptionHost::Leading {
            host: host_token(host.trim_start_matches('/')),
            has_scheme: scheme.is_some(),
            absolute,
        })
    }

    pub(crate) fn network(&self) -> Option<NetworkFilter<'_>> {
        match self.matcher_route() {
            MatcherInput::Native(line)
            | MatcherInput::Rewritten(line)
            | MatcherInput::RedirectOnly(line) => parse_network(line),
            // The host-only projection parses the synthesized anchor itself.
            MatcherInput::Hosts(_) => parse_network(self.line),
            MatcherInput::Translated(_) | MatcherInput::None => None,
        }
    }

    fn unsupported_modifier(&self) -> Option<&'static str> {
        // Prefer the existing tokens when the guard projection needed them.
        let find = |token: OptionToken<'_>| {
            let entry = lookup(token.name())?;
            (matches!(
                entry.kind,
                ModifierKind::Unsupported | ModifierKind::Inapplicable
            ))
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
            let (_, suffix) = literal_parts(self.line)?;
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
    let Some((pattern, suffix)) = literal_parts(line) else {
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

fn has_action_options(options: &Options<'_>) -> bool {
    // An action value can contain the characters that mark a cosmetic rule.
    !options.pattern.contains('#')
        && options.tokens().any(|option| {
            matches!(
                option.name(),
                modifiers::COOKIE | modifiers::CSP | modifiers::HLS | modifiers::REMOVEPARAM
            ) || option.name() == modifiers::URLTRANSFORM
                || option.name().eq_ignore_ascii_case(modifiers::REMOVEHEADER)
        })
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
    literal_parts(line.trim()).is_some_and(|(_, suffix)| {
        options::literal_options(suffix).any(|option| option == "badfilter")
    })
}

#[cfg(test)]
mod tests;
