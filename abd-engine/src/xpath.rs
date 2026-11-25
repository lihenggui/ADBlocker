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

//! XPath 1.0 for trusted XML response rules. The namespace axis, namespace
//! bindings and variables are unavailable in this filter context.

mod budget;
mod document;
mod evaluate;
mod functions;
mod lexer;
mod nodes;
mod parse;
mod value;

pub(crate) use document::prune;

pub(crate) struct XPath(Expr);

struct Expr {
    term: Term,
    depth: usize,
}

enum Term {
    Root,
    Context,
    Number(f64),
    String(String),
    Negate(Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
    Call(Function, Vec<Expr>),
    Filter(Box<Expr>, Vec<Expr>),
    Path(Box<Expr>, Vec<Step>),
}

#[derive(Clone, Copy)]
enum Op {
    Or,
    And,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Union,
}

struct Step {
    axis: Axis,
    test: Test,
    predicates: Vec<Expr>,
}

#[derive(Clone, Copy)]
enum Axis {
    Ancestor,
    AncestorOrSelf,
    Attribute,
    Child,
    Descendant,
    DescendantOrSelf,
    Following,
    FollowingSibling,
    Parent,
    Preceding,
    PrecedingSibling,
    SelfNode,
}

enum Test {
    Any,
    Name(String),
    Node,
    Text,
    Comment,
    ProcessingInstruction(Option<String>),
}

#[derive(Clone, Copy)]
enum Function {
    Last,
    Position,
    Count,
    Id,
    LocalName,
    NamespaceUri,
    Name,
    String,
    Concat,
    StartsWith,
    Contains,
    SubstringBefore,
    SubstringAfter,
    Substring,
    StringLength,
    NormalizeSpace,
    Translate,
    Boolean,
    Not,
    True,
    False,
    Lang,
    Number,
    Sum,
    Floor,
    Ceiling,
    Round,
}

fn xml_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r')
}

#[cfg(test)]
mod tests;
