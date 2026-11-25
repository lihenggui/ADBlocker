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

use crate::protocol_types::IssueKind;

macro_rules! codes {
    ($($code:ident => $kind:ident, $text:literal;)*) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub(crate) enum Code { $($code,)* }

        impl Code {
            #[cfg(test)]
            pub(crate) const ALL: &'static [Self] = &[$(Self::$code,)*];

            pub(crate) const fn kind(self) -> IssueKind {
                match self { $(Self::$code => IssueKind::$kind,)* }
            }

            pub(crate) const fn as_str(self) -> &'static str {
                match self { $(Self::$code => $text,)* }
            }
        }
    };
}

codes! {
    InvalidOption => Invalid, "option";
    UnsupportedOption => Unsupported, "option";
    InapplicableOption => Inapplicable, "option";
    NetworkSyntax => Invalid, "network-syntax";
    NetworkRegex => Unsupported, "network-regex";
    RegexSourceDomain => Unsupported, "regex-source-domain";
    NetworkDomains => Invalid, "network-domains";
    DenyallowMatcher => Invalid, "denyallow-matcher";
    AppCondition => Invalid, "app-condition";
    AppPattern => Invalid, "app-pattern";
    AddressLimit => Unsupported, "address-limit";
    ResponseLimit => Unsupported, "response-limit";
    ResponseQualifier => Invalid, "response-qualifier";
    ResponseCondition => Invalid, "response-condition";
    ResponseTrust => Untrusted, "response-trust";
    ReplaceValue => Invalid, "replace-value";
    JsonpruneValue => Invalid, "jsonprune-value";
    XmlpruneValue => Invalid, "xmlprune-value";
    UrltransformValue => Invalid, "urltransform-value";
    CosmeticSyntax => Invalid, "cosmetic-syntax";
    CosmeticScope => Invalid, "cosmetic-scope";
    CosmeticUnsupported => Unsupported, "cosmetic-unsupported";
    CosmeticUnsafe => Invalid, "cosmetic-unsafe";
    ScriptletNoCounterpart => Unsupported, "scriptlet-no-counterpart";
    ScriptletTrust => Untrusted, "scriptlet-trust";
    IncludeNotFollowed => Unsupported, "include-not-followed";
    MalformedDirective => Invalid, "malformed-directive";
    DenyallowPatternNamesHost => Invalid, "denyallow-pattern-names-host";
    DenyallowNegatedModifier => Invalid, "denyallow-negated-modifier";
    DenyallowNegatedDomain => Invalid, "denyallow-negated-domain";
    DenyallowWildcardTld => Invalid, "denyallow-wildcard-tld";
    DenyallowRegexDomain => Invalid, "denyallow-regex-domain";
    DenyallowEmptyDomain => Invalid, "denyallow-empty-domain";
    DenyallowInvalidDomain => Invalid, "denyallow-invalid-domain";
    DenyallowDuplicate => Invalid, "denyallow-duplicate";
    DenyallowWithTo => Invalid, "denyallow-with-to";
    DenyallowWithResponseAction => Invalid, "denyallow-with-response-action";
    AddressModifiers => Invalid, "address-modifiers";
    AddressEndpoint => Invalid, "address-endpoint";
    AddressExpression => Invalid, "address-expression";
    HtmlSize => Unsupported, "html-size";
    HtmlDomain => Invalid, "html-domain";
    HtmlSelector => Invalid, "html-selector";
    HtmlAttribute => Invalid, "html-attribute";
    HtmlAttributeOperator => Unsupported, "html-attribute-operator";
    HtmlContentAttribute => Unsupported, "html-content-attribute";
    HtmlChildContent => Unsupported, "html-child-content";
    HtmlPseudoClass => Unsupported, "html-pseudo-class";
    HtmlResponseHeader => Unsupported, "html-response-header";
    HtmlRegexFlags => Unsupported, "html-regex-flags";
    HtmlRegex => Invalid, "html-regex";
    HtmlGenerationLimit => Unsupported, "html-generation-limit";
    HtmlLookaround => Unsupported, "html-lookaround";
    HtmlLength => Invalid, "html-length";
    JavascriptNoDomains => Invalid, "javascript-no-domains";
    JavascriptUntrusted => Untrusted, "javascript-untrusted";
    JavascriptEmpty => Invalid, "javascript-empty";
    JavascriptLength => Unsupported, "javascript-length";
    JavascriptTemplate => Unsupported, "javascript-template";
    JavascriptUnsafe => Invalid, "javascript-unsafe";
    JavascriptCount => Unsupported, "javascript-count";
    JavascriptTableSize => Unsupported, "javascript-table-size";
    JavascriptReservedName => Invalid, "javascript-reserved-name";
    CspEmpty => Invalid, "csp-empty";
    CspInvalidValue => Invalid, "csp-invalid-value";
    CspReporting => Unsupported, "csp-reporting";
    CspNegated => Invalid, "csp-negated";
    CspMethod => Invalid, "csp-method";
    RemoveparamRemoveAll => Unsupported, "removeparam-remove-all";
    RemoveparamExpression => Unsupported, "removeparam-expression";
    RemoveparamInverted => Unsupported, "removeparam-inverted";
    RemoveparamUnsupportedName => Unsupported, "removeparam-unsupported-name";
    RemoveparamNegatedModifier => Invalid, "removeparam-negated-modifier";
    CookieTooLong => Unsupported, "cookie-too-long";
    CookieInvalidName => Invalid, "cookie-invalid-name";
    CookieInvalidExpression => Invalid, "cookie-invalid-expression";
    CookieUnescapedSeparator => Invalid, "cookie-unescaped-separator";
    CookieInvalidAttribute => Invalid, "cookie-invalid-attribute";
    CookieDuplicateAttribute => Invalid, "cookie-duplicate-attribute";
    CookieInvalidMaxAge => Invalid, "cookie-invalid-max-age";
    CookieInvalidSameSite => Invalid, "cookie-invalid-same-site";
    CookieAttributesOnException => Invalid, "cookie-attributes-on-exception";
    HlsEmptyPattern => Invalid, "hls-empty-pattern";
    HlsUnescapedSeparator => Invalid, "hls-unescaped-separator";
    HlsInnerModifiers => Unsupported, "hls-inner-modifiers";
    HlsMissingDelimiter => Invalid, "hls-missing-delimiter";
    HlsInvalidFlags => Invalid, "hls-invalid-flags";
    HlsUnsupportedPattern => Unsupported, "hls-unsupported-pattern";
    RemoveheaderInvalidName => Invalid, "removeheader-invalid-name";
    RemoveheaderProtectedByAdGuard => Invalid, "removeheader-protected-header";
    RemoveheaderRequiredForRequest => Unsupported, "removeheader-required-for-request";
    RemoveheaderRequiredForResponse => Unsupported, "removeheader-required-for-response";
    ScriptletSyntax => Invalid, "scriptlet-syntax";
    ScriptletArguments => Unsupported, "scriptlet-arguments";
    UnsafeScriptlet => Invalid, "unsafe-scriptlet";
    StyleSyntax => Invalid, "style-syntax";
    StyleDeclarations => Unsupported, "style-declarations";
    Matcher => Unsupported, "adguard-cosmetic-matcher";
}

/// A rule's verdict and its optional offending name. The wire output keeps
/// the class separate from the detail so clients can render new codes.
#[derive(Debug)]
pub(crate) struct RuleReason {
    code: Code,
    name: Option<String>,
    kind: IssueKind,
}

impl From<Code> for RuleReason {
    fn from(code: Code) -> Self {
        Self {
            code,
            name: None,
            kind: code.kind(),
        }
    }
}

impl RuleReason {
    pub(crate) fn named(code: Code, name: &str) -> Self {
        Self {
            code,
            name: (!name.is_empty()).then(|| name.to_owned()),
            kind: code.kind(),
        }
    }

    pub(crate) fn kind(&self) -> IssueKind {
        self.kind
    }

    pub(super) fn scriptlet_trust(name: &str, user: bool) -> Self {
        let mut reason = Self::named(Code::ScriptletTrust, name);
        if user {
            reason.kind = IssueKind::Unsupported;
        }
        reason
    }

    pub(crate) fn detail(&self) -> String {
        let code = self.code.as_str();
        match &self.name {
            Some(name) => format!("{code}:{name}"),
            None => code.to_owned(),
        }
    }
}
