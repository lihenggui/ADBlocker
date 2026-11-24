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

use crate::jsonpath::{self, Path};
use crate::rule::options::{unescape, unescaped};

pub(super) fn parse(value: &str) -> Option<String> {
    if unescaped(value, b'$').next().is_some() {
        return None;
    }
    let expression = unescape(value, &[',', '$']);
    Path::parse(&expression)?;
    Some(expression)
}

pub(super) fn disables(value: Option<&str>) -> bool {
    value.is_none()
}

pub(super) fn overrides_block() -> bool {
    false
}

pub(crate) struct Compiled(Path);

pub(super) fn compile(expression: &str) -> Option<Compiled> {
    Path::parse(expression).map(Compiled)
}

impl Compiled {
    pub(super) fn apply(&self, input: &[u8], within_budget: &dyn Fn() -> bool) -> Option<Vec<u8>> {
        jsonpath::prune(&self.0, input, within_budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prune_decodes_its_path_and_compiles_and_applies_it() {
        let Some(expression) = parse(r"\$['a\,b']") else {
            panic!("expected a prune expression");
        };
        assert_eq!(expression, "$['a,b']");
        let compiled = compile(&expression).unwrap();
        assert_eq!(
            compiled
                .apply(br#"{"a,b":1,"keep":true}"#, &|| true)
                .as_deref(),
            Some(br#"{"keep":true}"#.as_slice())
        );
        assert!(parse("$['a,b']").is_none());
        assert!(compile("$[").is_none());
    }

    #[test]
    fn only_an_unvalued_prune_exception_disables_pruning() {
        assert!(disables(None));
        assert!(!disables(Some("")));
        assert!(!disables(Some(r"\$.ads")));
        assert!(!overrides_block());
    }
}
