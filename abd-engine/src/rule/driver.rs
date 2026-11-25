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
//! Dispatch the active lines of a list to their destinations.

use super::javascript::Scripts;
use super::{
    AppRule, Kind, Reason, ResponseRule, Rule, SourcePermissions, address, html, preprocess,
};

pub(crate) trait Sink<'a> {
    fn script_table(&mut self) -> Option<&mut Scripts<'a>>;
    // Cross-cutting projections can use the original line before its payload is moved.
    fn observe(&mut self, source: &Rule<'a>);
    fn comment(&mut self, source: &Rule<'a>);
    fn matcher(&mut self, number: usize, source: &Rule<'a>);
    fn app(&mut self, number: usize, source: &Rule<'a>, app: &AppRule<'a>);
    fn response(&mut self, number: usize, line: &'a str, rule: ResponseRule<'a>);
    fn page_exception(&mut self, number: usize, source: &Rule<'a>);
    fn cosmetic(&mut self, number: usize, source: &Rule<'a>);
    fn html(&mut self, number: usize, line: &'a str, rule: html::Rule);
    fn address(&mut self, number: usize, line: &'a str, rule: address::AddressRule<'a>);
    fn script(&mut self, number: usize, source: &Rule<'a>);
    fn redirect(&mut self, number: usize, source: &Rule<'a>);
    fn not_applied(&mut self, number: usize, source: &Rule<'a>, reason: &Reason);
    fn directive(&mut self, number: usize, line: &'a str, issue: preprocess::DirectiveIssue);
}

/// The checker can give selected exceptions first use of its per-list budgets.
pub(crate) fn drive<'a>(
    text: &'a str,
    permissions: SourcePermissions,
    first: Option<fn(&str) -> bool>,
    sink: &mut impl Sink<'a>,
) -> usize {
    let early = first.into_iter().flat_map(|first| {
        preprocess::scan(text).filter(move |((_, line), issue)| issue.is_ok() && first(line))
    });
    let rest = preprocess::scan(text)
        .filter(|((_, line), issue)| issue.is_err() || first.is_none_or(|first| !first(line)));
    let mut count = 0;
    for ((number, line), issue) in early.chain(rest) {
        match issue {
            Ok(()) => {
                let scripts = if permissions.scripts() {
                    sink.script_table()
                } else {
                    None
                };
                let source = super::read_with_scripts(line, scripts);
                count += usize::from(dispatch(sink, number, source));
            }
            Err(issue) => sink.directive(number, line, issue),
        }
    }
    count
}

fn dispatch<'a>(sink: &mut impl Sink<'a>, number: usize, source: Rule<'a>) -> bool {
    sink.observe(&source);
    match source.kind {
        Kind::Comment => {
            sink.comment(&source);
            return false;
        }
        Kind::Matcher | Kind::Rewritten => sink.matcher(number, &source),
        Kind::App(ref app) => sink.app(number, &source, app),
        Kind::Response(rule) => sink.response(number, source.line, rule),
        Kind::PageException(_) => sink.page_exception(number, &source),
        Kind::Cosmetic if source.is_scriptlet() => sink.script(number, &source),
        Kind::Cosmetic => sink.cosmetic(number, &source),
        Kind::Html(rule) => sink.html(number, source.line, rule),
        Kind::Address(rule) => sink.address(number, source.line, rule),
        Kind::Redirect => sink.redirect(number, &source),
        Kind::NotApplied(ref reason) => sink.not_applied(number, &source, reason),
    }
    true
}
