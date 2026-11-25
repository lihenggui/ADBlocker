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

use roxmltree::Document;

use super::budget::{Budget, Result};
use super::nodes::{Context, Item};
use super::value::Value;
use super::{Expr, Op, Step, Term, XPath};

pub(super) struct Evaluation<'a> {
    pub(super) budget: Budget<'a>,
}

impl<'budget> Evaluation<'budget> {
    pub(super) fn new(within_budget: &'budget dyn Fn() -> bool) -> Self {
        Self {
            budget: Budget::new(within_budget),
        }
    }

    pub(super) fn evaluate<'a, 'input: 'a>(
        &mut self,
        expr: &Expr,
        context: Context<'a, 'input>,
    ) -> Result<Value<'a, 'input>> {
        self.budget.check()?;
        Ok(match &expr.term {
            Term::Root => Value::Nodes(vec![Item::Tree(context.item.owner().document().root())]),
            Term::Context => Value::Nodes(vec![context.item]),
            Term::Number(value) => Value::Number(*value),
            Term::String(value) => Value::String(value.clone().into()),
            Term::Negate(expr) => {
                Value::Number(-self.evaluate(expr, context)?.number(&mut self.budget)?)
            }
            Term::Binary(op, left, right) => self.binary(*op, left, right, context)?,
            Term::Call(function, expressions) => {
                let args = expressions
                    .iter()
                    .map(|expr| self.evaluate(expr, context))
                    .collect::<Result<_>>()?;
                function.call(args, context, &mut self.budget)?
            }
            Term::Filter(base, predicates) => {
                let nodes = self.evaluate(base, context)?.nodes()?;
                Value::Nodes(self.predicates(nodes, predicates)?)
            }
            Term::Path(base, steps) => {
                let mut nodes = self.evaluate(base, context)?.nodes()?;
                for step in steps {
                    nodes = self.step(nodes, step)?;
                }
                Value::Nodes(nodes)
            }
        })
    }

    fn binary<'a, 'input: 'a>(
        &mut self,
        op: Op,
        left: &Expr,
        right: &Expr,
        context: Context<'a, 'input>,
    ) -> Result<Value<'a, 'input>> {
        use Op::*;
        let left = self.evaluate(left, context)?;
        match op {
            Or if left.boolean() => return Ok(Value::Boolean(true)),
            And if !left.boolean() => return Ok(Value::Boolean(false)),
            _ => (),
        }
        let right = self.evaluate(right, context)?;
        Ok(match op {
            And | Or => Value::Boolean(right.boolean()),
            Equal | NotEqual | Less | LessEqual | Greater | GreaterEqual => {
                Value::Boolean(left.compare(op, right, &mut self.budget)?)
            }
            Union => {
                let mut nodes = left.nodes()?;
                nodes.extend(right.nodes()?);
                Value::Nodes(document_order(nodes))
            }
            _ => {
                let left = left.number(&mut self.budget)?;
                let right = right.number(&mut self.budget)?;
                Value::Number(match op {
                    Add => left + right,
                    Subtract => left - right,
                    Multiply => left * right,
                    Divide => left / right,
                    Modulo => left % right,
                    _ => unreachable!("XPath arithmetic operator"),
                })
            }
        })
    }

    fn step<'a, 'input: 'a>(
        &mut self,
        nodes: Vec<Item<'a, 'input>>,
        step: &Step,
    ) -> Result<Vec<Item<'a, 'input>>> {
        let mut result = Vec::new();
        for node in nodes {
            self.budget.visit()?;
            let matching = node.axis(step.axis, &step.test, &mut self.budget)?;
            result.extend(self.predicates(matching, &step.predicates)?);
        }
        self.budget.check()?;
        Ok(document_order(result))
    }

    fn predicates<'a, 'input: 'a>(
        &mut self,
        mut nodes: Vec<Item<'a, 'input>>,
        predicates: &[Expr],
    ) -> Result<Vec<Item<'a, 'input>>> {
        for predicate in predicates {
            let size = nodes.len();
            let mut kept = Vec::new();
            for (index, item) in nodes.into_iter().enumerate() {
                self.budget.visit()?;
                let position = index + 1;
                let context = Context {
                    item,
                    position,
                    size,
                };
                let result = self.evaluate(predicate, context)?;
                let keep = match result {
                    Value::Number(number) => number == context.position(),
                    other => other.boolean(),
                };
                if keep {
                    kept.push(item);
                }
            }
            nodes = kept;
        }
        Ok(nodes)
    }
}

fn document_order<'a, 'input>(mut nodes: Vec<Item<'a, 'input>>) -> Vec<Item<'a, 'input>> {
    nodes.sort_unstable_by_key(|node| node.order());
    nodes.dedup_by_key(|node| node.order());
    nodes
}

impl XPath {
    pub(super) fn select<'a, 'input: 'a>(
        &self,
        doc: &'a Document<'input>,
        within_budget: &dyn Fn() -> bool,
    ) -> Result<Vec<Item<'a, 'input>>> {
        Evaluation::new(within_budget)
            .evaluate(&self.0, Context::root(doc))?
            .nodes()
    }
}
