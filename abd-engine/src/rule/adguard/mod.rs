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

//! Project AdGuard cosmetic rules onto the existing cosmetic matcher.

use adblock::filters::cosmetic::{CosmeticFilterAction, CosmeticFilterError};
use adblock::lists::{FilterParseError, ParsedLine, parse_filter};

use super::cosmetic_options;
use super::javascript::{self, Scripts};

mod arguments;
mod names;
mod scriptlets;
mod style;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Unsupported {
    JavaScript(javascript::Rejection),
    ScriptletSyntax,
    ScriptletName,
    ScriptletResource,
    ScriptletArguments,
    UnsafeScriptlet,
    StyleSyntax,
    StyleDeclarations,
    Matcher,
}

pub(super) struct Translation {
    pub(super) line: String,
    pub(super) scriptlet: bool,
    pub(super) requires_trusted_scriptlets: bool,
}

pub(super) fn translate<'a>(
    line: &'a str,
    scripts: Option<&mut Scripts<'a>>,
) -> Option<Result<Translation, Unsupported>> {
    // The matcher's explicit network anchors take precedence over hashes.
    if line.starts_with('|') || line.starts_with("@@|") {
        return None;
    }
    let (domains, rest) = line.split_once('#')?;
    let (marker, body) = match rest.as_bytes().first()? {
        b'%' | b'$' | b'@' if !domains.contains("$$") => rest.split_once('#')?,
        _ => return None,
    };
    let (scriptlet, target) = match marker {
        "%" => (true, "##"),
        "@%" => (true, "#@#"),
        "$" => (false, "##"),
        "@$" => (false, "#@#"),
        "$?" => (false, "#?#"),
        "@$?" => (false, "#@?#"),
        _ => return None,
    };
    let body = body.trim();
    if scriptlet && !body.starts_with("//scriptlet(") {
        return Some(translate_javascript(domains, target, body, scripts));
    }
    Some(
        checked_domains(domains)
            .and_then(|domains| {
                if scriptlet {
                    scriptlets::translate(&domains, target, body)
                } else {
                    style::translate(&domains, target, body)
                }
            })
            .and_then(validate),
    )
}

fn translate_javascript<'a>(
    domains: &str,
    marker: &str,
    body: &'a str,
    scripts: Option<&mut Scripts<'a>>,
) -> Result<Translation, Unsupported> {
    use javascript::Rejection;
    if domains.is_empty() {
        return Err(Unsupported::JavaScript(Rejection::NoDomains));
    }
    let scripts = scripts.ok_or(Unsupported::JavaScript(Rejection::Untrusted))?;
    javascript::check_body(body).map_err(Unsupported::JavaScript)?;
    let domains = checked_domains(domains)?;
    let id = scripts.number(body);
    let name = javascript::name(id);
    let translated = validate(Translation {
        line: format!("{domains}{marker}+js({name})"),
        scriptlet: true,
        requires_trusted_scriptlets: false,
    })?;
    scripts.keep(body, id).map_err(Unsupported::JavaScript)?;
    Ok(translated)
}

fn checked_domains(domains: &str) -> Result<String, Unsupported> {
    if domains.is_empty() {
        return Ok(String::new());
    }
    let names: Vec<_> = domains.split(',').map(str::trim).collect();
    if names.iter().all(|name| name.starts_with('~'))
        || names.iter().any(|name| {
            let domain = name.strip_prefix('~').unwrap_or(name);
            let domain = domain.strip_suffix(".*").unwrap_or(domain);
            // The matcher silently ignores regex domains in a mixed list.
            // Dropping a negated regex could broaden the scope.
            !domain.starts_with(char::is_alphanumeric)
                || !domain.chars().all(|c| {
                    !c.is_ascii() || c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')
                })
        })
    {
        return Err(Unsupported::Matcher);
    }
    Ok(names.join(",").to_ascii_lowercase())
}

fn validate(translated: Translation) -> Result<Translation, Unsupported> {
    // Use the same parser as the matcher, including selector and domain rules.
    crate::public_suffix::install();
    match parse_filter(&translated.line, false, cosmetic_options()) {
        Ok(ParsedLine::Cosmetic(rule)) => {
            if let Some(CosmeticFilterAction::Style(declarations)) = rule.action
                && !style::supported(&declarations)
            {
                return Err(Unsupported::StyleDeclarations);
            }
            Ok(translated)
        }
        Err(FilterParseError::Cosmetic(CosmeticFilterError::InvalidCssStyle)) => {
            Err(Unsupported::StyleDeclarations)
        }
        _ => Err(Unsupported::Matcher),
    }
}

#[cfg(test)]
mod tests;
