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

//! Apply cookie actions to individual fields without collecting an HTTP body.

use std::fmt::Write as _;

use hyper::HeaderMap;
use hyper::header::{COOKIE, HeaderName, HeaderValue, SET_COOKIE};

use super::{Attributes, Change, Compiled, age_digits, cookie_name, date};

pub(crate) fn request<'a>(headers: &mut HeaderMap, rules: impl Iterator<Item = &'a Compiled>) {
    if !headers.contains_key(COOKIE) {
        return;
    }
    let rules: Vec<_> = rules.collect();
    if rules.iter().any(|rule| rule.change == Change::Suppress) {
        edit_fields(headers, COOKIE, |text| request_field(text, &rules));
    }
}

pub(crate) fn response<'a>(headers: &mut HeaderMap, rules: impl Iterator<Item = &'a Compiled>) {
    response_with_clock(headers, rules, crate::clock::now_ms);
}

fn response_with_clock<'a>(
    headers: &mut HeaderMap,
    rules: impl Iterator<Item = &'a Compiled>,
    now_ms: impl FnOnce() -> u64,
) {
    if !headers.contains_key(SET_COOKIE) {
        return;
    }
    let rules: Vec<_> = rules.collect();
    if rules.iter().any(|rule| rule.change != Change::Exempt) {
        // An absolute expiry has second precision. Round the clock up so
        // a fractional second cannot round the remaining lifetime up.
        let now = i64::try_from(now_ms().div_ceil(1000)).unwrap_or(i64::MAX);
        edit_fields(headers, SET_COOKIE, |text| {
            response_field(text, &rules, now)
        });
    }
}

enum Edit {
    Keep,
    Remove,
    Replace(String),
}

fn edit_fields(headers: &mut HeaderMap, name: HeaderName, edit: impl Fn(&str) -> Edit) {
    let mut changed = false;
    let fields: Vec<_> = headers
        .get_all(&name)
        .iter()
        .filter_map(|value| {
            match value.to_str().ok().map_or(Edit::Keep, &edit) {
                Edit::Keep => Some(value.clone()),
                Edit::Remove => {
                    changed = true;
                    None
                }
                Edit::Replace(text) => {
                    // Leave the original field if the rewritten value is unusable.
                    let Ok(mut replacement) = HeaderValue::from_str(&text) else {
                        return Some(value.clone());
                    };
                    changed |= replacement != *value;
                    replacement.set_sensitive(value.is_sensitive());
                    Some(replacement)
                }
            }
        })
        .collect();
    if changed {
        headers.remove(&name);
        for field in fields {
            headers.append(&name, field);
        }
    }
}

fn pair_name(pair: &str) -> Option<&str> {
    let (name, value) = pair.split_once('=')?;
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    (cookie_name(name)
        && value
            .bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b',' | b';' | b'\\')))
    .then_some(name)
}

fn request_field(text: &str, rules: &[&Compiled]) -> Edit {
    let mut kept = Vec::new();
    let mut removed = false;
    for pair in text.split(';').map(str::trim_ascii) {
        let Some(name) = pair_name(pair) else {
            return Edit::Keep;
        };
        if matches!(change(name, rules), Some(Change::Suppress)) {
            removed = true;
        } else {
            kept.push(pair);
        }
    }
    if !removed {
        Edit::Keep
    } else if kept.is_empty() {
        Edit::Remove
    } else {
        Edit::Replace(kept.join("; "))
    }
}

fn change(name: &str, rules: &[&Compiled]) -> Option<Change> {
    let mut result = None;
    for rule in rules.iter().filter(|rule| rule.matches(name)) {
        result = Some(match (result, rule.change) {
            (_, Change::Exempt) => return None,
            (Some(Change::Suppress), _) | (_, Change::Suppress) => Change::Suppress,
            (Some(Change::Attributes(previous)), Change::Attributes(mut next)) => {
                next.max_age = min_age(previous.max_age, next.max_age);
                next.same_site = next.same_site.or(previous.same_site);
                Change::Attributes(next)
            }
            (_, change) => change,
        });
    }
    result
}

fn min_age(first: Option<i64>, second: Option<i64>) -> Option<i64> {
    first.into_iter().chain(second).min()
}

fn response_field(text: &str, rules: &[&Compiled], now: i64) -> Edit {
    let mut fields = text.split(';').map(str::trim_ascii);
    let Some(pair) = fields.next() else {
        return Edit::Keep;
    };
    let Some(edit) = pair_name(pair).and_then(|name| change(name, rules)) else {
        return Edit::Keep;
    };
    let mut kept = vec![pair];
    let mut server_max_age = None;
    let mut server_expires = None;
    let attributes = match edit {
        Change::Attributes(attributes) => attributes,
        Change::Suppress | Change::Exempt => Attributes::default(),
    };
    for field in fields {
        let (name, value) = field.split_once('=').unwrap_or((field, ""));
        let name = name.trim_ascii_end();
        let is_max_age = name.eq_ignore_ascii_case("max-age");
        if attributes.max_age.is_some() {
            if is_max_age {
                server_max_age = parse_max_age(value).or(server_max_age);
                continue;
            }
            if name.eq_ignore_ascii_case("expires") {
                // An invalid date is ignored, including after a valid Expires.
                server_expires = date::parse(value).or(server_expires);
                continue;
            }
        }
        if field.is_empty()
            || field.bytes().any(|b| !b.is_ascii() || b.is_ascii_control())
            || (is_max_age && parse_max_age(value).is_none())
        {
            return Edit::Keep;
        }
        if attributes.same_site.is_some() && name.eq_ignore_ascii_case("samesite") {
            continue;
        }
        kept.push(field);
    }
    if edit == Change::Suppress {
        return Edit::Remove;
    }
    let mut rewritten = kept.join("; ");
    if let Some(limit) = attributes.max_age {
        let lifetime =
            server_max_age.or_else(|| server_expires.map(|expires| expires.saturating_sub(now)));
        let max_age = limit.min(lifetime.unwrap_or(limit)).max(0);
        write!(rewritten, "; Max-Age={max_age}").expect("writing to a string");
    }
    if let Some(same_site) = attributes.same_site {
        write!(rewritten, "; SameSite={}", same_site.as_str()).expect("writing to a string");
    }
    Edit::Replace(rewritten)
}

fn parse_max_age(value: &str) -> Option<i64> {
    let age = age_digits(value.trim_ascii_start())?;
    // A value outside the integer range is still beyond any rule's limit
    // or already expired, depending on its sign.
    Some(age.parse().unwrap_or(if age.starts_with('-') {
        i64::MIN
    } else {
        i64::MAX
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::action::cookie::parse;

    // Wed, 09 Jun 2021 10:17:44 GMT.
    const NOW_MS: u64 = 1_623_233_864_000;

    fn rewritten_cookies<const N: usize>(
        limit: i64,
        now_ms: u64,
        fields: [&str; N],
    ) -> Vec<String> {
        let rule = parse(&format!("NAME;maxAge={limit}"), false)
            .unwrap()
            .compile()
            .unwrap();
        let mut headers = HeaderMap::new();
        for field in fields {
            headers.append(SET_COOKIE, HeaderValue::from_str(field).unwrap());
        }
        response_with_clock(&mut headers, std::iter::once(&rule), || now_ms);
        headers
            .get_all(SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn max_age_is_capped_by_the_remaining_server_lifetime() {
        let cases = [
            (
                "NAME=soon; eXPiRes=Wed, 09 Jun 2021 10:18:14 GMT; Secure",
                "NAME=soon; Secure; Max-Age=30",
            ),
            (
                "NAME=later; Expires=Wed, 09 Jun 2021 10:19:54 GMT",
                "NAME=later; Max-Age=60",
            ),
            (
                "NAME=now; Expires=Wed, 09 Jun 2021 10:17:44 GMT",
                "NAME=now; Max-Age=0",
            ),
            (
                "NAME=past; Expires=Sun, 06 Nov 1994 08:49:37 GMT",
                "NAME=past; Max-Age=0",
            ),
            (
                "NAME=precedence; mAX-aGE=45; Expires=Sun, 06 Nov 1994 08:49:37 GMT",
                "NAME=precedence; Max-Age=45",
            ),
            (
                "NAME=server_cap; Max-Age=120; Expires=Wed, 09 Jun 2021 10:17:45 GMT",
                "NAME=server_cap; Max-Age=60",
            ),
            ("NAME=negative; Max-Age=-24", "NAME=negative; Max-Age=0"),
            (
                "NAME=zero; Max-Age=0; Expires=Wed, 09 Jun 2021 10:18:14 GMT",
                "NAME=zero; Max-Age=0",
            ),
            (
                "NAME=session; HttpOnly",
                "NAME=session; HttpOnly; Max-Age=60",
            ),
            (
                "NAME=invalid_date; Expires=not a date",
                "NAME=invalid_date; Max-Age=60",
            ),
            (
                "NAME=invalid_date_with_age; Expires=not a date; Max-Age=12",
                "NAME=invalid_date_with_age; Max-Age=12",
            ),
            (
                "NAME=invalid_age; Max-Age=oops; Expires=Wed, 09 Jun 2021 10:18:14 GMT",
                "NAME=invalid_age; Max-Age=30",
            ),
            ("malformed", "malformed"),
        ];
        assert_eq!(
            rewritten_cookies(60, NOW_MS, cases.map(|(field, _)| field)),
            cases.map(|(_, expected)| expected)
        );
    }

    #[test]
    fn the_last_valid_attribute_per_kind_sets_the_server_lifetime() {
        let cases = [
            (
                "NAME=age; Max-Age=3; Max-Age=40; Max-Age=invalid",
                "NAME=age; Max-Age=40",
            ),
            (
                "NAME=date; Expires=Wed, 09 Jun 2021 10:18:14 GMT; Expires=Wed, 09-Jun-2021\t10:18:04 GMT; Expires=invalid",
                "NAME=date; Max-Age=20",
            ),
        ];
        assert_eq!(
            rewritten_cookies(60, NOW_MS, cases.map(|(field, _)| field)),
            cases.map(|(_, expected)| expected)
        );
    }

    #[test]
    fn the_time_until_expires_rounds_down_and_negative_limits_expire_immediately() {
        let expiring = "NAME=soon; Expires=Wed, 09 Jun 2021 10:17:45 GMT";
        assert_eq!(
            rewritten_cookies(60, NOW_MS + 1, [expiring]),
            ["NAME=soon; Max-Age=0"]
        );
        assert_eq!(
            rewritten_cookies(-60, NOW_MS, ["NAME=session"]),
            ["NAME=session; Max-Age=0"]
        );
    }

    mod properties {
        use proptest::prelude::*;

        use super::*;
        use crate::test_support::{arbitrary_bytes, arbitrary_text, mutate};

        fn field() -> impl Strategy<Value = Vec<u8>> {
            let structured = (
                "[A-Za-z0-9_-]{1,16}",
                "[A-Za-z0-9_+=!/:.-]{0,22}",
                any::<bool>(),
                prop::sample::select(vec![
                    "",
                    "; Path=/; Secure",
                    "; Max-Age=999; HttpOnly",
                    "; SameSite=Strict",
                    "; Expires=Wed, 09 Jun 2021 10:18:14 GMT",
                    "; Max-Age=-9999999999999999999999",
                    "; Max-Age=invalid",
                    "; Partitioned; Priority=High",
                ]),
                prop::option::of((any::<prop::sample::Index>(), prop::option::of(any::<u8>()))),
            )
                .prop_map(|(name, value, quoted, attributes, mutation)| {
                    let pair = if quoted {
                        format!("{name}=\"{value}\"")
                    } else {
                        format!("{name}={value}")
                    };
                    mutate(format!("{pair}{attributes}").into_bytes(), mutation)
                });
            prop_oneof![arbitrary_bytes(), structured]
        }

        // RFC 6265 cookie-pair, expressed independently of the editor.
        fn pair(field: &HeaderValue) -> Option<(&str, &str)> {
            let pair = field
                .to_str()
                .ok()?
                .split(';')
                .next()?
                .trim_matches([' ', '\t']);
            let (name, value) = pair.split_once('=')?;
            let octets = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            let name_ok = !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^`|~_".contains(&b));
            let value_ok = octets
                .bytes()
                .all(|b| matches!(b, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e));
            (name_ok && value_ok).then_some((name, value))
        }

        proptest! {
            #[test]
            fn cookie_rules_and_fields_return_and_rewritten_fields_keep_the_cookie_pair(
                bytes in field(),
                input in prop_oneof![
                    arbitrary_text(),
                    (1..600, any::<bool>()).prop_map(|(age, exact)| {
                        let selector = if exact { "session" } else { "/^[A-Za-z0-9_-]+$/" };
                        format!("{selector};maxAge={age};sameSite=lax")
                    }),
                ],
                now in any::<u64>(),
            ) {
                let rule = parse(&input, false).and_then(|spec| spec.compile()).ok();
                let Ok(original) = HeaderValue::from_bytes(&bytes) else { return Ok(()); };
                let mut headers = HeaderMap::new();
                headers.insert(SET_COOKIE, original.clone());

                // This rule exercises rewriting even when the arbitrary rule rejects its input.
                let cap = parse(";maxAge=60;sameSite=lax", false).unwrap().compile().unwrap();
                response_with_clock(&mut headers, rule.iter().chain([&cap]), || now);
                if let Some(changed) = headers.get(SET_COOKIE).filter(|value| *value != original) {
                    let parsed = pair(changed);
                    prop_assert!(parsed.is_some());
                    prop_assert_eq!(parsed, pair(&original));
                }
            }
        }
    }
}
