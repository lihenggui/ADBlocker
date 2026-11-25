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

use std::borrow::Cow;

use super::action::{ActionKind, ActionSpec};
use super::modifiers::{APP, Compatibility, Kind, kind_bit, lookup};
use super::options::{OptionToken, Options};
use crate::limits::MAX_RESPONSE_RULE;

pub(crate) struct ResponseRule<'a> {
    options: Options<'a>,
    action_index: usize,
    kind: ActionKind,
    compatibility: Compatibility,
}

pub(crate) struct Action<'a> {
    pub(crate) pattern: Cow<'a, str>,
    pub(crate) options: Vec<&'a str>,
    pub(crate) spec: Option<ActionSpec>,
    pub(crate) kind: ActionKind,
    pub(crate) include: u16,
    pub(crate) exclude: u16,
    pub(crate) apps: Option<&'a str>,
    pub(crate) strict_third_party: Option<bool>,
    pub(crate) exception: bool,
}

fn action_kind(token: OptionToken<'_>) -> Option<(ActionKind, Compatibility)> {
    match lookup(token.raw_name)?.kind {
        Kind::Action(kind, compatibility) => Some((kind, compatibility)),
        _ => None,
    }
}

impl<'a> ResponseRule<'a> {
    /// Retain the first action even if later validation rejects the rule.
    pub(super) fn read(options: Options<'a>) -> Result<Self, Options<'a>> {
        let action = options.tokens().enumerate().find_map(|(index, token)| {
            action_kind(token).map(|(kind, compatibility)| (index, kind, compatibility))
        });
        match action {
            Some((action_index, kind, compatibility)) => Ok(Self {
                options,
                action_index,
                kind,
                compatibility,
            }),
            None => Err(options),
        }
    }

    pub(crate) fn normalized(&self, line: &'a str) -> Cow<'a, str> {
        // An incompatible noop must not share the valid rule's budget key.
        if matches!(self.kind, ActionKind::Hls | ActionKind::RemoveHeader) {
            return Cow::Borrowed(line);
        }
        self.options
            .without_noops()
            .map_or(Cow::Borrowed(line), Cow::Owned)
    }

    pub(crate) fn action(&self) -> Option<Action<'a>> {
        let options = &self.options;
        if options.written_len() > MAX_RESPONSE_RULE
            || (matches!(self.kind, ActionKind::Hls | ActionKind::RemoveHeader) && options.noop)
        {
            return None;
        }
        let exception = options.pattern.starts_with("@@");
        let pattern = options
            .pattern
            .strip_prefix("@@")
            .unwrap_or(options.pattern);
        let value = options.token(self.action_index).value;
        let mut action = Action {
            pattern: Cow::Borrowed(pattern),
            options: Vec::new(),
            spec: action_spec(self.kind, value, exception)?,
            kind: self.kind,
            include: 0,
            exclude: 0,
            apps: None,
            strict_third_party: None,
            exception,
        };
        for (index, token) in options.tokens().enumerate() {
            if index == self.action_index {
                continue;
            }
            if action_kind(token).is_some()
                || !self.compatibility.accepts(token.raw_name, token.value)
            {
                return None;
            }
            if token.raw_name == APP
                && let Some(names) = token.value
            {
                if action.apps.replace(names).is_some() {
                    return None;
                }
            } else {
                action.take_qualifier(token)?;
            }
        }
        if matches!(self.kind, ActionKind::Hls | ActionKind::RemoveHeader) {
            let match_case = action.options.contains(&"match-case");
            if match_case {
                action.pattern = super::regex::case_sensitive_pattern(pattern);
            }
            if !super::regex::network_pattern_supported(&action.pattern, match_case) {
                return None;
            }
        }
        Some(action)
    }

    #[cfg(test)]
    pub(crate) fn from_parts(pattern: &'a str, suffix: &'a str) -> Option<Self> {
        Self::read(Options::split(
            pattern,
            suffix,
            super::options::Quoting::AdGuard,
        ))
        .ok()
    }

    #[cfg(test)]
    pub(crate) fn parts(&self) -> (&'a str, &'a str) {
        (self.options.pattern, self.options.suffix)
    }
}

fn action_spec(
    kind: ActionKind,
    value: Option<&str>,
    exception: bool,
) -> Option<Option<ActionSpec>> {
    if exception && kind.disables(value) {
        Some(None)
    } else {
        Some(Some(kind.parse(value.filter(|v| !v.is_empty())?)?))
    }
}

impl<'a> Action<'a> {
    fn take_qualifier(&mut self, option: OptionToken<'a>) -> Option<()> {
        // AdGuard's reference omits $all for $jsonprune, but its lists use it.
        if option.raw == "all" {
            return Some(());
        }
        if self.kind == ActionKind::RemoveHeader
            && let Some(third_party) = match option.raw_name {
                "strict-first-party" | "strict1p" => Some(false),
                "strict-third-party" | "strict3p" => Some(true),
                _ => None,
            }
        {
            if self
                .strict_third_party
                .is_some_and(|previous| previous != third_party)
            {
                return None;
            }
            self.strict_third_party = Some(third_party);
            return Some(());
        }
        let (name, negated) = option
            .raw_name
            .strip_prefix('~')
            .map_or((option.raw_name, false), |name| (name, true));
        match option.value.is_none().then(|| kind_bit(name)).flatten() {
            Some(bit) if negated => self.exclude |= bit,
            Some(bit) => self.include |= bit,
            None => self.options.push(option.raw),
        }
        Some(())
    }
}
