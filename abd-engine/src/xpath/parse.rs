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

use super::lexer::{Token, lex};
use super::{Axis, Expr, Function, Op, Step, Term, Test, XPath};
use crate::limits::{MAX_XPATH_DEPTH, MAX_XPATH_EXPRESSION};

type Result<T> = std::result::Result<T, &'static str>;

impl XPath {
    pub(crate) fn parse(source: &str) -> Result<Self> {
        let expression = parse(source)?;
        expression.require_nodes("XPath must return a node-set")?;
        Ok(Self(expression))
    }
}

pub(super) fn parse(source: &str) -> Result<Expr> {
    if source.len() > MAX_XPATH_EXPRESSION {
        return Err("XPath expression is too long");
    }
    let mut parser = Parser {
        tokens: lex(source)?,
        at: 0,
        nesting: 0,
    };
    let expression = parser.binary(1)?;
    parser.expect(Token::End)?;
    Ok(expression)
}

impl Expr {
    fn new(term: Term, depth: usize) -> Result<Self> {
        if depth > MAX_XPATH_DEPTH {
            return Err("XPath syntax tree is too deep");
        }
        Ok(Self { term, depth })
    }

    fn leaf(term: Term) -> Self {
        Self { term, depth: 1 }
    }

    fn require_nodes(&self, reason: &'static str) -> Result<()> {
        let returns_nodes = match self.term {
            Term::Root
            | Term::Context
            | Term::Path(..)
            | Term::Filter(..)
            | Term::Binary(Op::Union, ..) => true,
            Term::Call(function, _) => function.returns_nodes(),
            _ => false,
        };
        returns_nodes.then_some(()).ok_or(reason)
    }
}

struct Parser<'a> {
    tokens: Vec<Token<'a>>,
    at: usize,
    nesting: usize,
}

impl<'a> Parser<'a> {
    fn current(&self) -> Token<'a> {
        self.tokens[self.at]
    }

    fn next(&mut self) -> Token<'a> {
        let token = self.current();
        if token != Token::End {
            self.at += 1;
        }
        token
    }

    fn eat(&mut self, token: Token<'a>) -> bool {
        if self.current() == token {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: Token<'a>) -> Result<()> {
        self.eat(token)
            .then_some(())
            .ok_or("unexpected XPath token")
    }

    fn nested(&mut self, min: u8) -> Result<Expr> {
        if self.nesting >= MAX_XPATH_DEPTH {
            return Err("XPath syntax is nested too deeply");
        }
        self.nesting += 1;
        let result = self.binary(min);
        self.nesting -= 1;
        result
    }

    fn binary(&mut self, min: u8) -> Result<Expr> {
        let mut left = if min <= 7 && self.eat(Token::Minus) {
            // Union binds more tightly than unary minus.
            let expr = self.nested(7)?;
            let depth = expr.depth + 1;
            Expr::new(Term::Negate(Box::new(expr)), depth)?
        } else {
            self.path()?
        };
        while let Some((op, precedence)) = operator(self.current()) {
            if precedence < min {
                break;
            }
            self.next();
            let right = self.binary(precedence + 1)?;
            if matches!(op, Op::Union) {
                left.require_nodes("XPath union needs node-sets")?;
                right.require_nodes("XPath union needs node-sets")?;
            }
            let depth = left.depth.max(right.depth) + 1;
            left = Expr::new(Term::Binary(op, Box::new(left), Box::new(right)), depth)?;
        }
        Ok(left)
    }

    fn path(&mut self) -> Result<Expr> {
        let mut steps = Vec::new();
        let base = if self.eat(Token::Slash) {
            if self.starts_step() {
                steps.push(self.step()?);
            } else {
                return Ok(Expr::leaf(Term::Root));
            }
            Expr::leaf(Term::Root)
        } else if self.eat(Token::DoubleSlash) {
            steps.push(descendant_shortcut());
            steps.push(self.step()?);
            Expr::leaf(Term::Root)
        } else if self.starts_step() {
            steps.push(self.step()?);
            Expr::leaf(Term::Context)
        } else {
            self.filter()?
        };
        while matches!(self.current(), Token::Slash | Token::DoubleSlash) {
            if self.next() == Token::DoubleSlash {
                steps.push(descendant_shortcut());
            }
            steps.push(self.step()?);
        }
        if steps.is_empty() {
            return Ok(base);
        }
        base.require_nodes("XPath path needs a node-set")?;
        let depth = steps
            .iter()
            .map(|step| depth(&step.predicates) + 1)
            .max()
            .unwrap_or(0);
        let depth = base.depth.max(depth) + 1;
        Expr::new(Term::Path(Box::new(base), steps), depth)
    }

    fn starts_step(&self) -> bool {
        match self.current() {
            Token::Name(name) => self.tokens[self.at + 1] != Token::Open || is_node_type(name),
            Token::Qualified | Token::Star | Token::At | Token::Dot | Token::DoubleDot => true,
            _ => false,
        }
    }

    fn filter(&mut self) -> Result<Expr> {
        let base = match self.next() {
            Token::Number(value) => Expr::leaf(Term::Number(value)),
            Token::Literal(value) => Expr::leaf(Term::String(value.to_owned())),
            Token::Open => {
                let expr = self.nested(1)?;
                self.expect(Token::Close)?;
                expr
            }
            Token::Name(name) => self.call(name)?,
            Token::Dollar => return Err("XPath variables are unavailable"),
            _ => return Err("expected an XPath expression"),
        };
        let predicates = self.predicates()?;
        if predicates.is_empty() {
            return Ok(base);
        }
        base.require_nodes("XPath filter needs a node-set")?;
        let depth = base.depth.max(depth(&predicates)) + 1;
        Expr::new(Term::Filter(Box::new(base), predicates), depth)
    }

    fn call(&mut self, name: &str) -> Result<Expr> {
        let function = Function::named(name).ok_or("unknown XPath function")?;
        self.expect(Token::Open)?;
        let mut args = Vec::new();
        if !self.eat(Token::Close) {
            loop {
                args.push(self.nested(1)?);
                if self.eat(Token::Close) {
                    break;
                }
                self.expect(Token::Comma)?;
            }
        }
        function.validate(&args)?;
        let depth = depth(&args) + 1;
        Expr::new(Term::Call(function, args), depth)
    }

    fn step(&mut self) -> Result<Step> {
        if matches!(self.current(), Token::Dot | Token::DoubleDot) {
            let axis = if self.next() == Token::Dot {
                Axis::SelfNode
            } else {
                Axis::Parent
            };
            return Ok(Step {
                axis,
                test: Test::Node,
                predicates: Vec::new(),
            });
        }
        let axis = if self.eat(Token::At) {
            Axis::Attribute
        } else if self.tokens[self.at + 1] == Token::Axis {
            let Token::Name(name) = self.next() else {
                return Err("invalid XPath axis");
            };
            self.next();
            axis(name)?
        } else {
            Axis::Child
        };
        let test = self.node_test()?;
        Ok(Step {
            axis,
            test,
            predicates: self.predicates()?,
        })
    }

    fn node_test(&mut self) -> Result<Test> {
        match self.next() {
            Token::Star => Ok(Test::Any),
            Token::Qualified => Err("XPath prefixes have no namespace bindings"),
            Token::Dollar => Err("XPath variables are unavailable"),
            Token::Name(name) if !self.eat(Token::Open) => Ok(Test::Name(name.to_owned())),
            Token::Name(name) if is_node_type(name) => {
                let test = match name {
                    "node" => Test::Node,
                    "text" => Test::Text,
                    "comment" => Test::Comment,
                    _ => Test::ProcessingInstruction(match self.current() {
                        Token::Literal(text) => {
                            self.next();
                            Some(text.to_owned())
                        }
                        _ => None,
                    }),
                };
                self.expect(Token::Close)?;
                Ok(test)
            }
            _ => Err("expected an XPath node test"),
        }
    }

    fn predicates(&mut self) -> Result<Vec<Expr>> {
        let mut predicates = Vec::new();
        while self.eat(Token::LeftBracket) {
            predicates.push(self.nested(1)?);
            self.expect(Token::RightBracket)?;
        }
        Ok(predicates)
    }
}

fn depth(expressions: &[Expr]) -> usize {
    expressions.iter().map(|expr| expr.depth).max().unwrap_or(0)
}

fn descendant_shortcut() -> Step {
    Step {
        axis: Axis::DescendantOrSelf,
        test: Test::Node,
        predicates: Vec::new(),
    }
}

fn is_node_type(name: &str) -> bool {
    matches!(name, "node" | "text" | "comment" | "processing-instruction")
}

fn axis(name: &str) -> Result<Axis> {
    use Axis::*;
    Ok(match name {
        "ancestor" => Ancestor,
        "ancestor-or-self" => AncestorOrSelf,
        "attribute" => Attribute,
        "child" => Child,
        "descendant" => Descendant,
        "descendant-or-self" => DescendantOrSelf,
        "following" => Following,
        "following-sibling" => FollowingSibling,
        "parent" => Parent,
        "preceding" => Preceding,
        "preceding-sibling" => PrecedingSibling,
        "self" => SelfNode,
        "namespace" => return Err("XPath namespace axis is unavailable"),
        _ => return Err("unknown XPath axis"),
    })
}

fn operator(token: Token<'_>) -> Option<(Op, u8)> {
    use Token::*;
    Some(match token {
        Name("or") => (Op::Or, 1),
        Name("and") => (Op::And, 2),
        Equal => (Op::Equal, 3),
        NotEqual => (Op::NotEqual, 3),
        Less => (Op::Less, 4),
        LessEqual => (Op::LessEqual, 4),
        Greater => (Op::Greater, 4),
        GreaterEqual => (Op::GreaterEqual, 4),
        Plus => (Op::Add, 5),
        Minus => (Op::Subtract, 5),
        Star => (Op::Multiply, 6),
        Name("div") => (Op::Divide, 6),
        Name("mod") => (Op::Modulo, 6),
        Pipe => (Op::Union, 8),
        _ => return None,
    })
}

impl Function {
    fn named(name: &str) -> Option<Self> {
        use Function::*;
        Some(match name {
            "last" => Last,
            "position" => Position,
            "count" => Count,
            "id" => Id,
            "local-name" => LocalName,
            "namespace-uri" => NamespaceUri,
            "name" => Name,
            "string" => String,
            "concat" => Concat,
            "starts-with" => StartsWith,
            "contains" => Contains,
            "substring-before" => SubstringBefore,
            "substring-after" => SubstringAfter,
            "substring" => Substring,
            "string-length" => StringLength,
            "normalize-space" => NormalizeSpace,
            "translate" => Translate,
            "boolean" => Boolean,
            "not" => Not,
            "true" => True,
            "false" => False,
            "lang" => Lang,
            "number" => Number,
            "sum" => Sum,
            "floor" => Floor,
            "ceiling" => Ceiling,
            "round" => Round,
            _ => return None,
        })
    }

    fn returns_nodes(self) -> bool {
        matches!(self, Self::Id)
    }

    fn takes(self, args: usize) -> bool {
        use Function::*;
        match self {
            Last | Position | True | False => args == 0,
            LocalName | NamespaceUri | Name | String | StringLength | NormalizeSpace | Number => {
                args <= 1
            }
            Concat => args >= 2,
            StartsWith | Contains | SubstringBefore | SubstringAfter => args == 2,
            Substring => matches!(args, 2 | 3),
            Translate => args == 3,
            _ => args == 1,
        }
    }

    fn validate(self, args: &[Expr]) -> std::result::Result<(), &'static str> {
        if !self.takes(args.len()) {
            return Err("wrong number of XPath function arguments");
        }
        if matches!(
            self,
            Self::Count | Self::Sum | Self::LocalName | Self::NamespaceUri | Self::Name
        ) && let Some(arg) = args.first()
        {
            arg.require_nodes("XPath function needs a node-set")?;
        }
        Ok(())
    }
}
