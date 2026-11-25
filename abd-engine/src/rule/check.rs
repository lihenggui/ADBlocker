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

use std::sync::LazyLock;

use adblock::filters::cosmetic::{CosmeticFilter, CosmeticFilterError, CosmeticFilterMask};
use adblock::filters::network::NetworkFilterError;
use adblock::lists::{FilterParseError, ParseOptions, ParsedLine, parse_filter};
use adblock::resources::{InMemoryResourceStorage, PermissionMask, ResourceStorage};

use super::modifiers::{self, Kind as ModifierKind};
use super::options::{Options, Quoting};
use super::{
    Code, Kind, Reason, Rule, RuleReason, SourcePermissions, adguard, denyallow, javascript,
};

mod mapping;

impl Rule<'_> {
    pub(crate) fn check_matcher(&self, permissions: SourcePermissions) -> Result<(), RuleReason> {
        if let Kind::NotApplied(reason) = &self.kind {
            return self.check_rejection(reason, permissions);
        }
        self.check_with_matcher(permissions, None)
    }

    fn check_with_matcher(
        &self,
        permissions: SourcePermissions,
        fallback: Option<&Reason>,
    ) -> Result<(), RuleReason> {
        let line = self
            .translated
            .as_ref()
            .map(|translation| translation.line.as_str())
            .or(self.rewritten.as_deref())
            .unwrap_or(self.line);
        match parse_filter(line, false, permissions.matcher()) {
            Ok(ParsedLine::Network(rule)) => {
                if self.denyallow && !denyallow::matcher_applies(line, rule) {
                    return Err(Code::DenyallowMatcher.into());
                }
                Ok(())
            }
            Ok(ParsedLine::Cosmetic(rule)) => self.check_cosmetic(&rule, permissions),
            Err(error) => Err(match fallback {
                Some(reason) => reason.report(self.line),
                None => match error {
                    FilterParseError::Network(error) => network_error(line, error),
                    FilterParseError::Cosmetic(error) => cosmetic_error(error),
                    FilterParseError::Unsupported
                    | FilterParseError::Empty
                    | FilterParseError::InvalidExpiresInterval => Code::NetworkSyntax.into(),
                },
            }),
        }
    }

    pub(crate) fn check_rejection(
        &self,
        reason: &Reason,
        permissions: SourcePermissions,
    ) -> Result<(), RuleReason> {
        if reason.uses_matcher() {
            return self.check_with_matcher(permissions, Some(reason));
        }
        if matches!(
            reason,
            Reason::AdGuard(adguard::Unsupported::JavaScript(
                javascript::Rejection::Untrusted
            ))
        ) {
            // Diagnose malformed JavaScript before reporting a missing
            // source permission, without using the actual script table.
            let mut scripts = javascript::Scripts::default();
            super::read_with_scripts(self.line, Some(&mut scripts)).check_matcher(permissions)?;
        }
        Err(reason.report(self.line))
    }

    fn check_cosmetic(
        &self,
        parsed: &CosmeticFilter,
        permissions: SourcePermissions,
    ) -> Result<(), RuleReason> {
        if let Some(translated) = &self.translated {
            if translated.requires_trusted_scriptlets
                && !PermissionMask::from_bits(1).is_injectable_by(permissions.matcher().permissions)
            {
                return Err(scriptlet_trust(self.line, permissions));
            }
            return Ok(());
        }
        if parsed.mask.contains(CosmeticFilterMask::SCRIPT_INJECT) {
            if !crate::cosmetic::safe_raw_text(self.line) {
                return Err(Code::UnsafeScriptlet.into());
            }
            return check_scriptlet(self.line, parsed, permissions);
        }
        if parsed
            .plain_css_selector()
            .is_some_and(|selector| !crate::cosmetic::safe_selector(selector))
        {
            return Err(Code::CosmeticUnsafe.into());
        }
        Ok(())
    }
}

fn scriptlet_trust(line: &str, permissions: SourcePermissions) -> RuleReason {
    RuleReason::scriptlet_trust(scriptlet_name(line), permissions.is_user())
}

fn check_scriptlet(
    line: &str,
    parsed: &CosmeticFilter,
    permissions: SourcePermissions,
) -> Result<(), RuleReason> {
    if parsed.mask.contains(CosmeticFilterMask::UNHIDE) {
        return Ok(());
    }
    let name = scriptlet_name(line);
    let Some(adblock::filters::cosmetic::CosmeticFilterOperator::CssSelector(args)) =
        parsed.selector.first()
    else {
        return Err(Code::ScriptletSyntax.into());
    };
    static RESOURCES: LazyLock<ResourceStorage> = LazyLock::new(|| {
        ResourceStorage::from_backend(InMemoryResourceStorage::from_resources(
            super::scriptlet_resources().iter().cloned(),
        ))
    });
    let can_inject = |args, mask| !RESOURCES.get_scriptlet_resources([(args, mask)]).is_empty();
    if can_inject(args, parsed.permission) {
        Ok(())
    } else if can_inject(args, PermissionMask::from_bits(u8::MAX)) {
        Err(scriptlet_trust(line, permissions))
    } else if can_inject(name, PermissionMask::from_bits(u8::MAX)) {
        Err(RuleReason::named(Code::ScriptletArguments, name))
    } else {
        Err(RuleReason::named(Code::ScriptletNoCounterpart, name))
    }
}

fn network_error(line: &str, error: NetworkFilterError) -> RuleReason {
    use NetworkFilterError::*;
    match error {
        NoRegex | FullRegexUnsupported | RegexParsingError(_) => Code::NetworkRegex.into(),
        MatchCaseWithoutFullRegex => RuleReason::named(Code::InvalidOption, "match-case"),
        UnrecognisedOption if line.starts_with('/') && line.ends_with('/') => {
            Code::NetworkRegex.into()
        }
        NoSupportedDomains => Code::NetworkDomains.into(),
        UnrecognisedOption => unrecognized_option(line),
        _ => Code::NetworkSyntax.into(),
    }
}

fn unrecognized_option(line: &str) -> RuleReason {
    let Some(options) = Options::read(line, Quoting::Literal) else {
        return Code::NetworkSyntax.into();
    };
    for token in options.tokens() {
        if let Some(modifier) = modifiers::lookup(token.name()) {
            let code = if !modifier.accepts_value(token.value) {
                Code::InvalidOption
            } else {
                match modifier.kind {
                    ModifierKind::Unsupported => Code::UnsupportedOption,
                    ModifierKind::Inapplicable => Code::InapplicableOption,
                    _ => continue,
                }
            };
            return RuleReason::named(code, token.name());
        }
        // Some aliases are implemented by the upstream matcher alone.
        let probe = format!("{}${}", options.pattern, token.raw);
        if matches!(
            parse_filter(&probe, false, ParseOptions::default()),
            Err(FilterParseError::Network(
                NetworkFilterError::UnrecognisedOption
            ))
        ) {
            return RuleReason::named(Code::InvalidOption, token.name());
        }
    }
    Code::NetworkSyntax.into()
}

fn cosmetic_error(error: CosmeticFilterError) -> RuleReason {
    use CosmeticFilterError::*;
    match error {
        UnsupportedSyntax
        | HtmlFilteringUnsupported
        | LocationModifiersUnsupported
        | ProceduralFilterWithMultipleSelectors => Code::CosmeticUnsupported,
        GenericUnhide | GenericScriptInject | GenericAction | DoubleNegation => Code::CosmeticScope,
        InvalidScriptletArgs => Code::ScriptletSyntax,
        PunycodeError
        | InvalidActionSpecifier
        | MissingSharp
        | InvalidCssStyle
        | InvalidCssSelector
        | EmptyRule => Code::CosmeticSyntax,
    }
    .into()
}

fn scriptlet_name(line: &str) -> &str {
    line.split_once("//scriptlet(")
        .map(|(_, args)| args)
        .or_else(|| line.split_once("+js(").map(|(_, args)| args))
        .map_or("", |args| {
            let args = args.trim_start();
            if let Some(quote) = args
                .chars()
                .next()
                .filter(|ch| matches!(ch, '\'' | '"' | '`'))
            {
                args[1..].split(quote).next().unwrap_or_default()
            } else {
                args.split([',', ')']).next().unwrap_or_default().trim()
            }
        })
}
