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
//! Parse paths without accepting script expressions or implicit coercions.

use serde_json::Value;

use super::{Filter, Key, Member, Path, Predicate, Selector, Slice, Step};
use crate::limits::{MAX_JSON_PATH, MAX_JSON_PATH_NESTING};

impl Path {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        if text.len() > MAX_JSON_PATH {
            return None;
        }
        let mut parser = Parser(text);
        let path = parser.path(0)?;
        parser.rest().is_empty().then_some(path)
    }
}

struct Parser<'a>(&'a str);

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        self.0.trim_start()
    }

    fn take(&mut self, token: char) -> bool {
        if let Some(rest) = self.rest().strip_prefix(token) {
            self.0 = rest;
            true
        } else {
            false
        }
    }

    fn token(&mut self, accepts: impl FnMut(char) -> bool) -> &'a str {
        let text = self.rest();
        self.0 = text.trim_start_matches(accepts);
        &text[..text.len() - self.0.len()]
    }

    fn path(&mut self, nesting: usize) -> Option<Path> {
        if nesting > MAX_JSON_PATH_NESTING || !self.take('$') {
            return None;
        }
        let mut steps = Vec::new();
        loop {
            let dot = self.take('.');
            let recursive = dot && self.take('.');
            let selector = if self.take('[') {
                let selector = self.brackets(nesting)?;
                self.take(']').then_some(selector)?
            } else if dot {
                if self.take('*') {
                    Selector::Wildcard
                } else {
                    Selector::Members(vec![Member::Name(self.name()?.to_owned())])
                }
            } else {
                return Some(Path(steps));
            };
            steps.push(Step {
                recursive,
                selector,
            });
        }
    }

    fn brackets(&mut self, nesting: usize) -> Option<Selector> {
        if self.take('*') {
            return Some(Selector::Wildcard);
        }
        if self.take('?') {
            return Some(Selector::Filter(self.filter(nesting)?));
        }
        let mut members = vec![self.member()?];
        while self.take(',') {
            members.push(self.member()?);
        }
        Some(Selector::Members(members))
    }

    fn member(&mut self) -> Option<Member> {
        if self.rest().starts_with(['\'', '"']) {
            return Some(Member::Name(self.string()?));
        }
        let bare = self.subscript_token();
        if !self.take(':') {
            let bare = bare?;
            return if integer(bare) {
                Some(Member::Index(index(bare)))
            } else if !bare.starts_with('+') {
                Some(Member::Name(bare.to_owned()))
            } else {
                None
            };
        }
        let start = bare.map(bound).transpose().ok()?;
        let end = self.subscript_token().map(bound).transpose().ok()?;
        let stride = if self.take(':') {
            self.subscript_token()
                .map(bound)
                .transpose()
                .ok()?
                .unwrap_or(1)
        } else {
            1
        };
        Some(Member::Slice(Slice { start, end, stride }))
    }

    fn filter(&mut self, nesting: usize) -> Option<Filter> {
        if !self.take('(') {
            return None;
        }
        let op = self.name()?;
        let key = match self.rest().chars().next()? {
            '$' => Key::Path(self.path(nesting + 1)?),
            '\'' | '"' => {
                let key = self.string()?;
                if key.starts_with('$') {
                    let mut parser = Parser(&key);
                    let path = parser.path(nesting + 1)?;
                    if !parser.rest().is_empty() {
                        return None;
                    }
                    Key::Path(path)
                } else {
                    Key::Name(key)
                }
            }
            _ => Key::Name(self.name()?.to_owned()),
        };
        let predicate = match op {
            "has" => Predicate::Has,
            "key-eq" => Predicate::Equal(self.literal()?),
            "key-substr" => Predicate::Substring(self.string()?),
            _ => return None,
        };
        self.take(')').then_some(Filter { key, predicate })
    }

    fn literal(&mut self) -> Option<Value> {
        if self.rest().starts_with(['\'', '"']) {
            return Some(Value::String(self.string()?));
        }
        let token = self.token(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '+' | '-'));
        let value = serde_json::from_str(token).ok()?;
        matches!(value, Value::Bool(_) | Value::Null | Value::Number(_)).then_some(value)
    }

    fn subscript_token(&mut self) -> Option<&'a str> {
        if self.rest().starts_with('+') {
            let token = self.token(|ch| ch == '+' || ch.is_ascii_digit());
            return Some(token);
        }
        self.name()
    }

    fn name(&mut self) -> Option<&'a str> {
        let name = self.token(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-'));
        (!name.is_empty()).then_some(name)
    }

    /// Decode only the string escapes shared by both kinds of quotes.
    fn string(&mut self) -> Option<String> {
        let mut chars = self.rest().char_indices();
        let (_, quote @ ('\'' | '"')) = chars.next()? else {
            return None;
        };
        let mut text = String::new();
        while let Some((at, ch)) = chars.next() {
            match ch {
                ch if ch == quote => {
                    self.0 = &self.rest()[at + 1..];
                    return Some(text);
                }
                '\\' => match chars.next()?.1 {
                    escaped @ ('\'' | '"' | '\\') => text.push(escaped),
                    _ => return None,
                },
                _ => text.push(ch),
            }
        }
        None
    }
}

fn integer(token: &str) -> bool {
    let digits = token.strip_prefix(['-', '+']).unwrap_or(token);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// Saturate bounds larger than any possible document index.
fn index(token: &str) -> i64 {
    token.parse().unwrap_or_else(|_| {
        if token.starts_with('-') {
            i64::MIN
        } else {
            i64::MAX
        }
    })
}

fn bound(token: &str) -> Result<i64, ()> {
    integer(token).then(|| index(token)).ok_or(())
}
