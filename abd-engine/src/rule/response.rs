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
    pub(crate) pattern: &'a str,
    pub(crate) options: Vec<&'a str>,
    pub(crate) spec: Option<ActionSpec>,
    pub(crate) kind: ActionKind,
    pub(crate) include: u16,
    pub(crate) exclude: u16,
    pub(crate) apps: Option<&'a str>,
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
        self.options
            .without_noops()
            .map_or(Cow::Borrowed(line), Cow::Owned)
    }

    pub(crate) fn action(&self) -> Option<Action<'a>> {
        let options = &self.options;
        if options.written_len() > MAX_RESPONSE_RULE {
            return None;
        }
        let pattern = options.pattern.strip_prefix("@@");
        let exception = pattern.is_some();
        let value = options.token(self.action_index).value;
        let mut action = Action {
            pattern: pattern.unwrap_or(options.pattern),
            options: Vec::new(),
            spec: action_spec(self.kind, value, exception)?,
            kind: self.kind,
            include: 0,
            exclude: 0,
            apps: None,
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
                action.take_qualifier(token);
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
    fn take_qualifier(&mut self, option: OptionToken<'a>) {
        // AdGuard's reference omits $all for $jsonprune, but its lists use it.
        if option.raw == "all" {
            return;
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
    }
}
