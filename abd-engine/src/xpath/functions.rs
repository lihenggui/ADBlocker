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

use std::collections::HashMap;

use roxmltree::NS_XML_URI;

use super::budget::{Budget, Result};
use super::nodes::Context;
use super::value::Value;
use super::{Function, xml_space};
use crate::limits::MAX_REWRITE_BODY;

impl Function {
    pub(super) fn call<'a, 'input: 'a>(
        self,
        args: Vec<Value<'a, 'input>>,
        context: Context<'a, 'input>,
        work: &mut Budget<'_>,
    ) -> Result<Value<'a, 'input>> {
        use Function::*;
        let mut args = args.into_iter();
        let mut next = || args.next().ok_or(());
        Ok(match self {
            Last => Value::Number(context.size()),
            Position => Value::Number(context.position()),
            Count => Value::Number(next()?.nodes()?.len() as f64),
            // Without a DTD no attribute is declared to be of type ID.
            Id => Value::Nodes(Vec::new()),
            LocalName | NamespaceUri | Name => {
                let item = first_or_context(args.next(), context)?;
                let value = item.map(|item| match self {
                    LocalName => item.local_name(),
                    NamespaceUri => item.namespace(),
                    _ => item.qualified_name(),
                });
                Value::String(value.unwrap_or_default().into())
            }
            String | StringLength | NormalizeSpace | Number => {
                let arg = args
                    .next()
                    .unwrap_or_else(|| Value::Nodes(vec![context.item]));
                return self.with_context_default(arg, work);
            }
            Concat => Value::String(concat(args, work)?.into()),
            StartsWith | Contains | SubstringBefore | SubstringAfter => {
                let text = next()?.string(work)?;
                let pattern = next()?.string(work)?;
                work.text(text.len() + pattern.len())?;
                match self {
                    StartsWith => Value::Boolean(text.starts_with(pattern.as_ref())),
                    Contains => Value::Boolean(text.contains(pattern.as_ref())),
                    SubstringBefore => Value::String(
                        text.split_once(pattern.as_ref())
                            .map_or("", |(before, _)| before)
                            .to_owned()
                            .into(),
                    ),
                    SubstringAfter => Value::String(
                        text.split_once(pattern.as_ref())
                            .map_or("", |(_, after)| after)
                            .to_owned()
                            .into(),
                    ),
                    _ => unreachable!(),
                }
            }
            Substring => {
                let text = next()?.string(work)?;
                let start = round(next()?.number(work)?);
                let end = match args.next() {
                    Some(length) => start + round(length.number(work)?),
                    None => f64::INFINITY,
                };
                Value::String(substring(&text, start, end, work)?.into())
            }
            Translate => {
                let text = next()?.string(work)?;
                let from = next()?.string(work)?;
                let to = next()?.string(work)?;
                Value::String(translate(&text, &from, &to, work)?.into())
            }
            Boolean => Value::Boolean(next()?.boolean()),
            Not => Value::Boolean(!next()?.boolean()),
            True => Value::Boolean(true),
            False => Value::Boolean(false),
            Lang => Value::Boolean(lang(context, &next()?.string(work)?, work)?),
            Sum => Value::Number(sum(next()?.nodes()?, work)?),
            Floor => Value::Number(next()?.number(work)?.floor()),
            Ceiling => Value::Number(next()?.number(work)?.ceil()),
            Round => Value::Number(round(next()?.number(work)?)),
        })
    }

    fn with_context_default<'a, 'input: 'a>(
        self,
        arg: Value<'a, 'input>,
        work: &mut Budget<'_>,
    ) -> Result<Value<'a, 'input>> {
        if matches!(self, Self::Number) {
            return Ok(Value::Number(arg.number(work)?));
        }
        let text = arg.string(work)?;
        Ok(match self {
            Self::String => Value::String(text),
            Self::StringLength => {
                work.text(text.len())?;
                Value::Number(text.chars().count() as f64)
            }
            Self::NormalizeSpace => Value::String(normalize_space(&text, work)?.into()),
            _ => unreachable!(),
        })
    }
}

fn first_or_context<'a, 'input: 'a>(
    arg: Option<Value<'a, 'input>>,
    context: Context<'a, 'input>,
) -> Result<Option<super::nodes::Item<'a, 'input>>> {
    match arg {
        Some(value) => Ok(value.nodes()?.first().copied()),
        None => Ok(Some(context.item)),
    }
}

fn concat<'a, 'input: 'a>(
    args: impl Iterator<Item = Value<'a, 'input>>,
    work: &mut Budget<'_>,
) -> Result<String> {
    let mut result = String::new();
    for arg in args {
        let value = arg.string(work)?;
        work.text(value.len())?;
        if value.len() > MAX_REWRITE_BODY - result.len() {
            return Err(());
        }
        result.push_str(&value);
    }
    Ok(result)
}

fn substring(text: &str, start: f64, end: f64, work: &mut Budget<'_>) -> Result<String> {
    work.text(text.len())?;
    let mut result = String::new();
    for (index, ch) in text.chars().enumerate() {
        work.check()?;
        let position = (index + 1) as f64;
        if position >= end {
            break;
        }
        if position >= start && position < end {
            result.push(ch);
        }
    }
    Ok(result)
}

fn normalize_space(text: &str, work: &mut Budget<'_>) -> Result<String> {
    work.text(text.len())?;
    let mut output = String::new();
    for word in text.split(xml_space).filter(|word| !word.is_empty()) {
        work.check()?;
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(word);
    }
    Ok(output)
}

fn translate(text: &str, from: &str, to: &str, work: &mut Budget<'_>) -> Result<String> {
    work.text(text.len() + from.len() + to.len())?;
    let mut map = HashMap::new();
    let mut to = to.chars();
    for ch in from.chars() {
        work.check()?;
        let replacement = to.next();
        map.entry(ch).or_insert(replacement);
    }
    let mut output = String::new();
    for ch in text.chars() {
        work.check()?;
        if let Some(ch) = map.get(&ch).copied().unwrap_or(Some(ch)) {
            if ch.len_utf8() > MAX_REWRITE_BODY - output.len() {
                return Err(());
            }
            output.push(ch);
        }
    }
    work.text(output.len())?;
    Ok(output)
}

fn lang(context: Context<'_, '_>, language: &str, work: &mut Budget<'_>) -> Result<bool> {
    for node in context.item.owner().ancestors() {
        work.visit()?;
        if let Some(tag) = node.attribute((NS_XML_URI, "lang")) {
            work.text(tag.len())?;
            let prefix = tag.get(..language.len());
            return Ok(
                prefix.is_some_and(|prefix| prefix.eq_ignore_ascii_case(language))
                    && matches!(tag.as_bytes().get(language.len()), None | Some(b'-')),
            );
        }
    }
    Ok(false)
}

fn sum(nodes: Vec<super::nodes::Item<'_, '_>>, work: &mut Budget<'_>) -> Result<f64> {
    let mut sum = 0.0;
    for node in nodes {
        work.visit()?;
        sum += super::value::number(&node.value(work)?, work)?;
    }
    Ok(sum)
}

fn round(value: f64) -> f64 {
    if (-0.5..0.0).contains(&value) {
        return -0.0;
    }
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}
