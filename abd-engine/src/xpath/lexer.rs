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

use super::xml_space;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Token<'a> {
    End,
    Name(&'a str),
    Qualified,
    Number(f64),
    Literal(&'a str),
    Dollar,
    Slash,
    DoubleSlash,
    Dot,
    DoubleDot,
    At,
    Axis,
    Open,
    Close,
    LeftBracket,
    RightBracket,
    Comma,
    Pipe,
    Plus,
    Minus,
    Star,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

pub(super) fn lex(mut text: &str) -> Result<Vec<Token<'_>>, &'static str> {
    use Token::*;
    let mut tokens = Vec::new();
    loop {
        text = text.trim_start_matches(xml_space);
        let Some(ch) = text.chars().next() else {
            tokens.extend([End, End]);
            return Ok(tokens);
        };
        let rest = &text[ch.len_utf8()..];
        let (token, used) = match ch {
            '/' => pair(rest, '/', DoubleSlash, Slash),
            '.' if !rest.starts_with(|ch: char| ch.is_ascii_digit()) => {
                pair(rest, '.', DoubleDot, Dot)
            }
            '<' => pair(rest, '=', LessEqual, Less),
            '>' => pair(rest, '=', GreaterEqual, Greater),
            '!' if rest.starts_with('=') => (NotEqual, 2),
            ':' if rest.starts_with(':') => (Axis, 2),
            '\'' | '"' => {
                let end = rest.find(ch).ok_or("unterminated XPath literal")?;
                let literal = &rest[..end];
                if !literal.chars().all(xml_char) {
                    return Err("invalid character in XPath literal");
                }
                (Literal(literal), end + 2)
            }
            '.' | '0'..='9' => number(text)?,
            '$' => (Dollar, 1),
            '@' => (At, 1),
            '(' => (Open, 1),
            ')' => (Close, 1),
            '[' => (LeftBracket, 1),
            ']' => (RightBracket, 1),
            ',' => (Comma, 1),
            '|' => (Pipe, 1),
            '+' => (Plus, 1),
            '-' => (Minus, 1),
            '*' => (Star, 1),
            '=' => (Equal, 1),
            ch if name_start(ch) => name(text),
            _ => return Err("invalid XPath token"),
        };
        tokens.push(token);
        text = &text[used..];
    }
}

fn pair<'a>(rest: &str, next: char, double: Token<'a>, single: Token<'a>) -> (Token<'a>, usize) {
    if rest.starts_with(next) {
        (double, 2)
    } else {
        (single, 1)
    }
}

fn number(text: &str) -> Result<(Token<'_>, usize), &'static str> {
    let digits = |ch: char| ch.is_ascii_digit();
    let mut len = text.len() - text.trim_start_matches(digits).len();
    if let Some(fraction) = text[len..].strip_prefix('.') {
        len = text.len() - fraction.trim_start_matches(digits).len();
    }
    let value = text[..len].parse().map_err(|_| "invalid XPath number")?;
    Ok((Token::Number(value), len))
}

fn name(text: &str) -> (Token<'_>, usize) {
    let len = text.len() - text.trim_start_matches(name_char).len();
    if let Some(rest) = text[len..].strip_prefix(':') {
        if rest.starts_with('*') {
            return (Token::Qualified, len + 2);
        }
        if rest.starts_with(name_start) {
            return (
                Token::Qualified,
                text.len() - rest.trim_start_matches(name_char).len(),
            );
        }
    }
    (Token::Name(&text[..len]), len)
}

// XML Names, excluding the colon, which separates a QName's two NCNames.
fn name_start(ch: char) -> bool {
    matches!(
        ch,
        'A'..='Z' | '_' | 'a'..='z' | '\u{c0}'..='\u{d6}' | '\u{d8}'..='\u{f6}'
        | '\u{f8}'..='\u{2ff}' | '\u{370}'..='\u{37d}' | '\u{37f}'..='\u{1fff}'
        | '\u{200c}'..='\u{200d}' | '\u{2070}'..='\u{218f}' | '\u{2c00}'..='\u{2fef}'
        | '\u{3001}'..='\u{d7ff}' | '\u{f900}'..='\u{fdcf}' | '\u{fdf0}'..='\u{fffd}'
        | '\u{10000}'..='\u{effff}'
    )
}

fn name_char(ch: char) -> bool {
    name_start(ch)
        || matches!(ch, '-' | '.' | '0'..='9' | '\u{b7}' | '\u{300}'..='\u{36f}' | '\u{203f}'..='\u{2040}')
}

pub(super) fn valid_pi_target(target: &str) -> bool {
    let mut chars = target.chars();
    chars.next().is_some_and(|ch| ch == ':' || name_start(ch))
        && chars.all(|ch| ch == ':' || name_char(ch))
        && !target.eq_ignore_ascii_case("xml")
}

fn xml_char(ch: char) -> bool {
    xml_space(ch)
        || matches!(ch, '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}
