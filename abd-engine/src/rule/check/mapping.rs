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

use super::scriptlet_name;
use crate::rule::options::{Options, Quoting};
use crate::rule::{
    Code, Reason, RuleReason, action, address, adguard, csp, denyallow, html, javascript,
    modifiers, preprocess, removeparam,
};

impl Reason {
    pub(super) fn report(&self, line: &str) -> RuleReason {
        match self {
            Self::Modifier(name) => {
                let modifier = modifiers::lookup(name).expect("known modifier");
                let valid = Options::read(line, Quoting::Literal).is_some_and(|options| {
                    options
                        .tokens()
                        .any(|token| token.name() == *name && modifier.accepts_value(token.value))
                });
                let code = if !valid {
                    Code::InvalidOption
                } else {
                    match modifier.kind {
                        modifiers::Kind::Unsupported => Code::UnsupportedOption,
                        modifiers::Kind::Inapplicable => Code::InapplicableOption,
                        _ => unreachable!("modifier is not set aside"),
                    }
                };
                RuleReason::named(code, name)
            }
            Self::DenyAllow(reason) => (*reason).into(),
            Self::RegexSourceDomain => Code::RegexSourceDomain.into(),
            Self::NetworkRegex => Code::NetworkRegex.into(),
            Self::Csp(reason) => (*reason).into(),
            Self::RemoveParam(reason) => (*reason).into(),
            Self::Cookie(reason) => (*reason).into(),
            Self::Address(reason) => (*reason).into(),
            Self::HtmlFiltering(reason) => (*reason).into(),
            Self::AdGuard(reason) => reason.report(line),
        }
    }
}

impl From<preprocess::DirectiveIssue> for RuleReason {
    fn from(issue: preprocess::DirectiveIssue) -> Self {
        match issue {
            preprocess::DirectiveIssue::Malformed => Code::MalformedDirective.into(),
            preprocess::DirectiveIssue::Include => Code::IncludeNotFollowed.into(),
        }
    }
}

impl From<denyallow::Rejection> for RuleReason {
    fn from(reason: denyallow::Rejection) -> Self {
        match reason {
            denyallow::Rejection::PatternNamesHost => Code::DenyallowPatternNamesHost.into(),
            denyallow::Rejection::NegatedModifier => Code::DenyallowNegatedModifier.into(),
            denyallow::Rejection::NegatedDomain => Code::DenyallowNegatedDomain.into(),
            denyallow::Rejection::WildcardTld => Code::DenyallowWildcardTld.into(),
            denyallow::Rejection::RegexDomain => Code::DenyallowRegexDomain.into(),
            denyallow::Rejection::EmptyDomain => Code::DenyallowEmptyDomain.into(),
            denyallow::Rejection::InvalidDomain => Code::DenyallowInvalidDomain.into(),
            denyallow::Rejection::Duplicate => Code::DenyallowDuplicate.into(),
            denyallow::Rejection::WithTo => Code::DenyallowWithTo.into(),
            denyallow::Rejection::WithResponseAction => Code::DenyallowWithResponseAction.into(),
        }
    }
}

impl From<address::Rejection> for RuleReason {
    fn from(reason: address::Rejection) -> Self {
        match reason {
            address::Rejection::Modifiers => Code::AddressModifiers.into(),
            address::Rejection::Endpoint => Code::AddressEndpoint.into(),
            address::Rejection::Expression => Code::AddressExpression.into(),
        }
    }
}

impl From<html::Rejection> for RuleReason {
    fn from(reason: html::Rejection) -> Self {
        match reason {
            html::Rejection::Size => Code::HtmlSize.into(),
            html::Rejection::Domain => Code::HtmlDomain.into(),
            html::Rejection::Selector => Code::HtmlSelector.into(),
            html::Rejection::Attribute => Code::HtmlAttribute.into(),
            html::Rejection::AttributeOperator => Code::HtmlAttributeOperator.into(),
            html::Rejection::ContentAttribute => Code::HtmlContentAttribute.into(),
            html::Rejection::ChildContent => Code::HtmlChildContent.into(),
            html::Rejection::PseudoClass => Code::HtmlPseudoClass.into(),
            html::Rejection::ResponseHeader => Code::HtmlResponseHeader.into(),
            html::Rejection::RegexFlags => Code::HtmlRegexFlags.into(),
            html::Rejection::Regex => Code::HtmlRegex.into(),
            html::Rejection::LookAround => Code::HtmlLookaround.into(),
            html::Rejection::Length => Code::HtmlLength.into(),
        }
    }
}

impl From<javascript::Rejection> for RuleReason {
    fn from(reason: javascript::Rejection) -> Self {
        match reason {
            javascript::Rejection::NoDomains => Code::JavascriptNoDomains.into(),
            javascript::Rejection::Untrusted => Code::JavascriptUntrusted.into(),
            javascript::Rejection::Empty => Code::JavascriptEmpty.into(),
            javascript::Rejection::Length => Code::JavascriptLength.into(),
            javascript::Rejection::Template => Code::JavascriptTemplate.into(),
            javascript::Rejection::Unsafe => Code::JavascriptUnsafe.into(),
            javascript::Rejection::Count => Code::JavascriptCount.into(),
            javascript::Rejection::TableSize => Code::JavascriptTableSize.into(),
            javascript::Rejection::ReservedName => Code::JavascriptReservedName.into(),
        }
    }
}

impl From<csp::Rejection> for RuleReason {
    fn from(reason: csp::Rejection) -> Self {
        match reason {
            csp::Rejection::Empty => Code::CspEmpty.into(),
            csp::Rejection::InvalidValue => Code::CspInvalidValue.into(),
            csp::Rejection::Reporting => Code::CspReporting.into(),
            csp::Rejection::Negated => Code::CspNegated.into(),
            csp::Rejection::Method => Code::CspMethod.into(),
        }
    }
}

impl From<removeparam::Rejection> for RuleReason {
    fn from(reason: removeparam::Rejection) -> Self {
        match reason {
            removeparam::Rejection::RemoveAll => Code::RemoveparamRemoveAll.into(),
            removeparam::Rejection::Expression => Code::RemoveparamExpression.into(),
            removeparam::Rejection::Inverted => Code::RemoveparamInverted.into(),
            removeparam::Rejection::UnsupportedName => Code::RemoveparamUnsupportedName.into(),
            removeparam::Rejection::NegatedModifier => Code::RemoveparamNegatedModifier.into(),
        }
    }
}

impl From<action::cookie::Rejection> for RuleReason {
    fn from(reason: action::cookie::Rejection) -> Self {
        match reason {
            action::cookie::Rejection::TooLong => Code::CookieTooLong.into(),
            action::cookie::Rejection::InvalidName => Code::CookieInvalidName.into(),
            action::cookie::Rejection::InvalidExpression => Code::CookieInvalidExpression.into(),
            action::cookie::Rejection::UnescapedSeparator => Code::CookieUnescapedSeparator.into(),
            action::cookie::Rejection::InvalidAttribute => Code::CookieInvalidAttribute.into(),
            action::cookie::Rejection::DuplicateAttribute => Code::CookieDuplicateAttribute.into(),
            action::cookie::Rejection::InvalidMaxAge => Code::CookieInvalidMaxAge.into(),
            action::cookie::Rejection::InvalidSameSite => Code::CookieInvalidSameSite.into(),
            action::cookie::Rejection::AttributesOnException => {
                Code::CookieAttributesOnException.into()
            }
        }
    }
}

impl From<action::hls::Rejection> for RuleReason {
    fn from(reason: action::hls::Rejection) -> Self {
        match reason {
            action::hls::Rejection::EmptyPattern => Code::HlsEmptyPattern.into(),
            action::hls::Rejection::UnescapedSeparator => Code::HlsUnescapedSeparator.into(),
            action::hls::Rejection::InnerModifiers => Code::HlsInnerModifiers.into(),
            action::hls::Rejection::MissingDelimiter => Code::HlsMissingDelimiter.into(),
            action::hls::Rejection::InvalidFlags => Code::HlsInvalidFlags.into(),
            action::hls::Rejection::UnsupportedPattern => Code::HlsUnsupportedPattern.into(),
        }
    }
}

impl From<action::removeheader::Rejection> for RuleReason {
    fn from(reason: action::removeheader::Rejection) -> Self {
        match reason {
            action::removeheader::Rejection::InvalidName => Code::RemoveheaderInvalidName.into(),
            action::removeheader::Rejection::ProtectedByAdGuard => {
                Code::RemoveheaderProtectedByAdGuard.into()
            }
            action::removeheader::Rejection::RequiredForRequest => {
                Code::RemoveheaderRequiredForRequest.into()
            }
            action::removeheader::Rejection::RequiredForResponse => {
                Code::RemoveheaderRequiredForResponse.into()
            }
        }
    }
}

impl adguard::Unsupported {
    pub(super) fn report(&self, line: &str) -> RuleReason {
        use adguard::Unsupported::*;
        match self {
            JavaScript(reason) => (*reason).into(),
            ScriptletName | ScriptletResource => {
                RuleReason::named(Code::ScriptletNoCounterpart, scriptlet_name(line))
            }
            ScriptletSyntax => Code::ScriptletSyntax.into(),
            ScriptletArguments => RuleReason::named(Code::ScriptletArguments, scriptlet_name(line)),
            UnsafeScriptlet => Code::UnsafeScriptlet.into(),
            StyleSyntax => Code::StyleSyntax.into(),
            StyleDeclarations => Code::StyleDeclarations.into(),
            Matcher => Code::Matcher.into(),
        }
    }
}
