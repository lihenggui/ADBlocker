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

use super::action::parameter_name;
use super::modifiers::{REMOVEPARAM, kind_bit};
use super::options::Options;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Rejection {
    RemoveAll,
    Expression,
    Inverted,
    UnsupportedName,
    NegatedModifier,
}

pub(super) fn validate(options: &Options<'_>) -> Result<(), Rejection> {
    for token in options.tokens().filter(|token| token.name() == REMOVEPARAM) {
        if token.raw_name != REMOVEPARAM {
            return Err(Rejection::NegatedModifier);
        }
        let name = token.value.unwrap_or_default();
        if name.starts_with('/') || name.starts_with("~/") {
            return Err(Rejection::Expression);
        }
        if name.starts_with('~') {
            return Err(Rejection::Inverted);
        }
        validate_name(name)?;
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), Rejection> {
    if name.is_empty() {
        return Err(Rejection::RemoveAll);
    }
    if !parameter_name(name) {
        return Err(Rejection::UnsupportedName);
    }
    Ok(())
}

pub(super) fn document_default(options: &Options<'_>) -> bool {
    options.tokens().any(|token| token.raw_name == REMOVEPARAM)
        && !options.tokens().any(|token| {
            kind_bit(token.name()).is_some()
                || matches!(
                    token.name(),
                    "all" | "background" | "beacon" | "object-subrequest"
                )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{Kind, Reason, read};

    #[test]
    fn unsupported_removeparam_forms_keep_their_specific_reason() {
        for (rule, expected) in [
            ("||example.org^$removeparam", Rejection::RemoveAll),
            ("$removeparam=", Rejection::RemoveAll),
            ("@@||example.com^$removeparam", Rejection::RemoveAll),
            (
                "$removeparam=/^(utm_source|utm_medium|utm_term)=/",
                Rejection::Expression,
            ),
            (
                "$removeparam=/^(utm_content|utm_campaign|utm_referrer)=/",
                Rejection::Expression,
            ),
            ("$removeparam=/utm_.*/", Rejection::Expression),
            (
                "$removeparam=/^utm_source=campaign$/",
                Rejection::Expression,
            ),
            (
                "$removeparam=/^(gclid|yclid|fbclid)=/",
                Rejection::Expression,
            ),
            (
                "@@||example.com^$removeparam=/^(gclid|yclid|fbclid)=/",
                Rejection::Expression,
            ),
            (
                "||example.com^$removeparam=/^(yclid|fbclid)=/",
                Rejection::Expression,
            ),
            ("$removeparam=~/regexp/", Rejection::Expression),
            ("$removeparam=~param", Rejection::Inverted),
            ("$removeparam=%24param", Rejection::UnsupportedName),
            ("$~removeparam=param", Rejection::NegatedModifier),
        ] {
            let rule = read(rule);
            assert!(
                matches!(&rule.kind, Kind::NotApplied(Reason::RemoveParam(error)) if error == &expected),
                "{}",
                rule.line
            );
            assert!(rule.matcher_input().is_none());
        }
    }
}
