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

//! Evaluation of bounded boolean expressions over filter-list constants.

// Both formats use these capabilities across an entire list.
const CONSTANTS: &[(&str, bool)] = &[
    // The engine reads AdGuard rule forms.
    ("adguard", true),
    // The catalog takes AdGuard's Android builds.
    ("adguard_app_android", true),
    // The matcher reads uBlock Origin rule forms.
    ("ext_ublock", true),
    // The engine filters traffic on mobile devices.
    ("env_mobile", true),
    // Android WebView and most Android browsers use Chromium. Published
    // uBlock rules under this token include site fixes for those browsers,
    // such as Chromium-specific redirects and anti-adblock scriptlets.
    ("env_chromium", true),
    // The engine can remove elements from HTML responses.
    ("cap_html_filtering", true),
    // Styles injected into HTML are author styles, not browser user styles.
    ("cap_user_stylesheet", false),
    // Authors use this constant to disable a block.
    ("false", false),
];

const MAX_EXPRESSION: usize = 1024;
const MAX_DEPTH: usize = 32;

pub(super) fn evaluate(input: &str) -> Option<bool> {
    if input.len() > MAX_EXPRESSION {
        return None;
    }
    let mut parser = Parser(input);
    let value = parser.or(0)?;
    parser.0.trim_start().is_empty().then_some(value)
}

struct Parser<'a>(&'a str);

impl Parser<'_> {
    fn take(&mut self, token: &str) -> bool {
        if let Some(tail) = self.0.trim_start().strip_prefix(token) {
            self.0 = tail;
            true
        } else {
            false
        }
    }

    fn or(&mut self, depth: usize) -> Option<bool> {
        let mut value = self.and(depth)?;
        while self.take("||") {
            value |= self.and(depth)?;
        }
        Some(value)
    }

    fn and(&mut self, depth: usize) -> Option<bool> {
        let mut value = self.unary(depth)?;
        while self.take("&&") {
            value &= self.unary(depth)?;
        }
        Some(value)
    }

    fn unary(&mut self, depth: usize) -> Option<bool> {
        if depth > MAX_DEPTH {
            return None;
        }
        if self.take("!") {
            return Some(!self.unary(depth + 1)?);
        }
        if self.take("(") {
            let value = self.or(depth + 1)?;
            return self.take(")").then_some(value);
        }
        self.identifier()
    }

    fn identifier(&mut self) -> Option<bool> {
        let s = self.0.trim_start();
        if !s
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        {
            return None;
        }
        let end = s
            .bytes()
            .position(|b| !(b.is_ascii_alphanumeric() || b == b'_'))
            .unwrap_or(s.len());
        let (token, rest) = s.split_at(end);
        self.0 = rest;
        Some(
            CONSTANTS
                .iter()
                .any(|&(name, value)| name == token && value),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_use_boolean_precedence_and_parentheses() {
        for (condition, expected) in [
            ("ext_ublock", true),
            ("!env_firefox", true),
            ("false", false),
            ("!false", true),
            ("!!false", false),
            ("ext_ublock && !env_mobile", false),
            ("ext_ublock || env_firefox && false", true),
            ("(ext_ublock || env_firefox) && false", false),
            (" ! false && ( false || adguard ) ", true),
            ("!adguard && false || env_mobile", true),
            ("unknown_token42", false),
            ("!unknown_token42", true),
            ("env_chromium && !cap_user_stylesheet", true),
        ] {
            assert_eq!(evaluate(condition), Some(expected), "{condition}");
        }
    }

    #[test]
    fn malformed_conditions_are_rejected_even_when_the_value_is_already_known() {
        for condition in [
            "",
            " ",
            "!",
            "()",
            "(false",
            "false)",
            "false true",
            "false & adguard",
            "false | adguard",
            "&& adguard",
            "adguard ||",
            "adguard || @",
            "false && @",
            "adguard &&&& false",
            "adguard ||| false",
            "adguard == false",
            "adguard && ()",
            "false # comment",
            "false-true",
            "12",
        ] {
            assert_eq!(evaluate(condition), None, "{condition}");
        }
    }

    #[test]
    fn condition_length_and_recursion_are_bounded() {
        let allowed = format!("{} false", "!".repeat(MAX_DEPTH));
        assert_eq!(evaluate(&allowed), Some(false));
        assert_eq!(evaluate(&format!("!{allowed}")), None);
        let nested = format!(
            "{}false{}",
            "(".repeat(MAX_DEPTH + 1),
            ")".repeat(MAX_DEPTH + 1)
        );
        assert_eq!(evaluate(&nested), None);
        let long = format!("{}false", "false || ".repeat(MAX_EXPRESSION / 9 + 1));
        assert_eq!(evaluate(&long), None);
    }
}
