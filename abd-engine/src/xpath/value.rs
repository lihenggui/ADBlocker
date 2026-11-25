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

use super::Op;
use super::budget::{Budget, Result};
use super::nodes::Item;
use super::xml_space;

pub(super) enum Value<'a, 'input> {
    Nodes(Vec<Item<'a, 'input>>),
    Boolean(bool),
    Number(f64),
    String(Cow<'a, str>),
}

impl<'a, 'input: 'a> Value<'a, 'input> {
    pub(super) fn nodes(self) -> Result<Vec<Item<'a, 'input>>> {
        match self {
            Self::Nodes(nodes) => Ok(nodes),
            _ => Err(()),
        }
    }

    pub(super) fn boolean(&self) -> bool {
        match self {
            Self::Nodes(nodes) => !nodes.is_empty(),
            Self::Boolean(value) => *value,
            Self::Number(value) => *value != 0.0 && !value.is_nan(),
            Self::String(value) => !value.is_empty(),
        }
    }

    pub(super) fn number(self, work: &mut Budget<'_>) -> Result<f64> {
        match self {
            Self::Boolean(value) => Ok(u8::from(value).into()),
            Self::Number(value) => Ok(value),
            other => number(&other.string(work)?, work),
        }
    }

    pub(super) fn string(self, work: &mut Budget<'_>) -> Result<Cow<'a, str>> {
        Ok(match self {
            Self::Nodes(nodes) => match nodes.first() {
                Some(node) => node.value(work)?,
                None => "".into(),
            },
            Self::Boolean(true) => "true".into(),
            Self::Boolean(false) => "false".into(),
            Self::Number(number) if number.is_nan() => "NaN".into(),
            Self::Number(f64::INFINITY) => "Infinity".into(),
            Self::Number(f64::NEG_INFINITY) => "-Infinity".into(),
            Self::Number(0.0) => "0".into(),
            Self::Number(number) if number.fract() == 0.0 => format!("{number:.0}").into(),
            Self::Number(number) => number.to_string().into(),
            Self::String(value) => value,
        })
    }

    pub(super) fn compare(self, op: Op, other: Self, work: &mut Budget<'_>) -> Result<bool> {
        let equality = matches!(op, Op::Equal | Op::NotEqual);
        match (self, other) {
            (left @ Self::Boolean(_), right) | (left, right @ Self::Boolean(_)) if equality => {
                Ok(equal(op, left.boolean(), right.boolean()))
            }
            (left @ Self::Nodes(_), right @ Self::Boolean(_))
            | (left @ Self::Boolean(_), right @ Self::Nodes(_)) => Ok(numbers(
                op,
                u8::from(left.boolean()).into(),
                u8::from(right.boolean()).into(),
            )),
            (Self::Nodes(left), Self::Nodes(right)) => compare_sets(op, &left, &right, work),
            (Self::Nodes(nodes), other) => compare_set(op, &nodes, other, work),
            (other, Self::Nodes(nodes)) => compare_set(reverse(op), &nodes, other, work),
            (Self::String(left), Self::String(right)) if equality => {
                strings(op, &left, &right, work)
            }
            (left, right) => Ok(numbers(op, left.number(work)?, right.number(work)?)),
        }
    }
}

fn compare_sets(
    op: Op,
    left: &[Item<'_, '_>],
    right: &[Item<'_, '_>],
    work: &mut Budget<'_>,
) -> Result<bool> {
    for left in left {
        work.visit()?;
        let left = left.value(work)?;
        let equality = matches!(op, Op::Equal | Op::NotEqual);
        let numeric = if equality { 0.0 } else { number(&left, work)? };
        for right in right {
            work.visit()?;
            let right = right.value(work)?;
            let matched = if equality {
                strings(op, &left, &right, work)?
            } else {
                numbers(op, numeric, number(&right, work)?)
            };
            if matched {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn compare_set(
    op: Op,
    nodes: &[Item<'_, '_>],
    other: Value<'_, '_>,
    work: &mut Budget<'_>,
) -> Result<bool> {
    if nodes.is_empty() {
        return Ok(false);
    }
    if matches!(op, Op::Equal | Op::NotEqual)
        && let Value::String(right) = other
    {
        for node in nodes {
            work.visit()?;
            if strings(op, &node.value(work)?, &right, work)? {
                return Ok(true);
            }
        }
    } else {
        let right = other.number(work)?;
        for node in nodes {
            work.visit()?;
            if numbers(op, number(&node.value(work)?, work)?, right) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn number(text: &str, work: &mut Budget<'_>) -> Result<f64> {
    work.text(text.len())?;
    let text = text.trim_matches(xml_space);
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if (whole.is_empty() && fraction.is_empty())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return Ok(f64::NAN);
    }
    Ok(text.parse().unwrap_or(f64::NAN))
}

fn strings(op: Op, left: &str, right: &str, work: &mut Budget<'_>) -> Result<bool> {
    if left.len() == right.len() {
        work.text(left.len())?;
    }
    Ok(equal(op, left, right))
}

fn equal<T: PartialEq>(op: Op, left: T, right: T) -> bool {
    match op {
        Op::Equal => left == right,
        Op::NotEqual => left != right,
        _ => unreachable!("XPath equality operator"),
    }
}

fn numbers(op: Op, left: f64, right: f64) -> bool {
    match op {
        Op::Less => left < right,
        Op::LessEqual => left <= right,
        Op::Greater => left > right,
        Op::GreaterEqual => left >= right,
        _ => equal(op, left, right),
    }
}

fn reverse(op: Op) -> Op {
    match op {
        Op::Less => Op::Greater,
        Op::LessEqual => Op::GreaterEqual,
        Op::Greater => Op::Less,
        Op::GreaterEqual => Op::LessEqual,
        _ => op,
    }
}
