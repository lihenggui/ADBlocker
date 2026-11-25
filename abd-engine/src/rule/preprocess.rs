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

//! Selects the active lines of one filter list.

mod condition;

const MAX_NESTING: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirectiveIssue {
    Malformed,
    Include,
}

type Line<'a> = (usize, &'a str);

struct Block<L> {
    // An invalid condition is already reported at the opening line.
    opening: Option<L>,
    parent_active: bool,
    condition: bool,
    in_else: bool,
}

pub(crate) struct Preprocessor<L> {
    blocks: Vec<Block<L>>,
    // An over-deep subtree is inactive until its matching endif. Count its
    // levels without adding frames, so it cannot consume the parent's endif.
    overflow: usize,
}

impl<L> Default for Preprocessor<L> {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            overflow: 0,
        }
    }
}

impl<L> Preprocessor<L> {
    fn active(&self) -> bool {
        self.overflow == 0
            && self
                .blocks
                .last()
                .is_none_or(|block| block.parent_active && block.condition != block.in_else)
    }

    fn open(&mut self, location: L, condition: &str) -> Option<DirectiveIssue> {
        if self.blocks.len() == MAX_NESTING {
            self.overflow += 1;
            return Some(DirectiveIssue::Malformed);
        }
        let value = condition::evaluate(condition);
        self.blocks.push(Block {
            opening: value.map(|_| location),
            parent_active: self.active(),
            condition: value.unwrap_or(false),
            in_else: false,
        });
        value.is_none().then_some(DirectiveIssue::Malformed)
    }

    fn otherwise(&mut self, tail: &str) -> Option<DirectiveIssue> {
        if !tail.is_empty() {
            return Some(DirectiveIssue::Malformed);
        }
        if self.overflow != 0 {
            return None;
        }
        match self.blocks.last_mut() {
            Some(block) if !block.in_else => {
                block.in_else = true;
                None
            }
            _ => Some(DirectiveIssue::Malformed),
        }
    }

    fn close(&mut self, tail: &str) -> Option<DirectiveIssue> {
        if !tail.is_empty() {
            return Some(DirectiveIssue::Malformed);
        }
        if self.overflow != 0 {
            self.overflow -= 1;
            return None;
        }
        self.blocks
            .pop()
            .is_none()
            .then_some(DirectiveIssue::Malformed)
    }

    pub(crate) fn read(&mut self, location: L, line: &str) -> Option<Result<(), DirectiveIssue>> {
        let Some(directive) = line.trim_start().strip_prefix("!#") else {
            return self.active().then_some(Ok(()));
        };
        let end = directive
            .find(char::is_whitespace)
            .unwrap_or(directive.len());
        let (keyword, tail) = directive.split_at(end);
        let tail = tail.trim();
        let issue = match keyword {
            "if" => self.open(location, tail),
            "else" => self.otherwise(tail),
            "endif" => self.close(tail),
            "include" => self.active().then_some(DirectiveIssue::Include),
            _ => None,
        };
        issue.map(Err)
    }
}

/// Active lines and directive issues, with their original one-based line
/// numbers. Unterminated openings are reported at the end of the scan.
pub(crate) fn scan(text: &str) -> impl Iterator<Item = (Line<'_>, Result<(), DirectiveIssue>)> {
    let mut lines = text.lines().enumerate().map(|(n, line)| (n + 1, line));
    let mut state = Preprocessor::default();
    std::iter::from_fn(move || {
        for line in lines.by_ref() {
            if let Some(event) = state.read(line, line.1) {
                return Some((line, event));
            }
        }
        while let Some(block) = state.blocks.pop() {
            if let Some(line) = block.opening {
                return Some((line, Err(DirectiveIssue::Malformed)));
            }
        }
        None
    })
}

pub(crate) fn lines(text: &str) -> impl Iterator<Item = &str> {
    scan(text).filter_map(|((_, line), event)| event.ok().map(|()| line))
}

#[cfg(test)]
mod tests;
