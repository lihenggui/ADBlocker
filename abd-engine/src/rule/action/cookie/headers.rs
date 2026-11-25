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

use crate::list_regex::{MatchCache, SearchBudget};
use hyper::HeaderMap;
use hyper::header::{COOKIE, HeaderName, HeaderValue, SET_COOKIE};

use super::{Attributes, Change, Compiled, age_digits, cookie_name, date};

struct Search<'a> {
    rules: Vec<(&'a Compiled, MatchCache)>,
    budget: SearchBudget,
}

impl<'a> Search<'a> {
    fn new(rules: impl Iterator<Item = &'a Compiled>) -> Self {
        Self {
            rules: rules.map(|rule| (rule, None)).collect(),
            budget: SearchBudget::default(),
        }
    }
}

pub(crate) fn request<'a>(headers: &mut HeaderMap, rules: impl Iterator<Item = &'a Compiled>) {
    if !headers.contains_key(COOKIE) {
        return;
    }
    let mut search = Search::new(rules);
    if search
        .rules
        .iter()
        .any(|(rule, _)| rule.change == Change::Suppress)
    {
        edit_fields(headers, COOKIE, |text| request_field(text, &mut search));
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
    let mut search = Search::new(rules);
    if search
        .rules
        .iter()
        .any(|(rule, _)| rule.change != Change::Exempt)
    {
        // An absolute expiry has second precision. Round the clock up so
        // a fractional second cannot round the remaining lifetime up.
        let now = i64::try_from(now_ms().div_ceil(1000)).unwrap_or(i64::MAX);
        edit_fields(headers, SET_COOKIE, |text| {
            response_field(text, &mut search, now)
        });
    }
}

enum Edit {
    Keep,
    Remove,
    Replace(Vec<u8>),
}

fn edit_fields(headers: &mut HeaderMap, name: HeaderName, mut edit: impl FnMut(&[u8]) -> Edit) {
    let mut changed = false;
    let fields: Vec<_> = headers
        .get_all(&name)
        .iter()
        .filter_map(|value| {
            match edit(value.as_bytes()) {
                Edit::Keep => Some(value.clone()),
                Edit::Remove => {
                    changed = true;
                    None
                }
                Edit::Replace(text) => {
                    // Leave the original field if the rewritten value is unusable.
                    let Ok(mut replacement) = HeaderValue::from_bytes(&text) else {
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

fn request_field(text: &[u8], search: &mut Search<'_>) -> Edit {
    let Ok(text) = std::str::from_utf8(text) else {
        return Edit::Keep;
    };
    let mut kept = Vec::new();
    let mut removed = false;
    for pair in text.split(';').map(str::trim_ascii) {
        let Some(name) = pair_name(pair) else {
            return Edit::Keep;
        };
        if matches!(change(name.as_bytes(), search), Some(Change::Suppress)) {
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
        Edit::Replace(kept.join("; ").into_bytes())
    }
}

fn change(name: &[u8], search: &mut Search<'_>) -> Option<Change> {
    let mut result = None;
    for (rule, cache) in &mut search.rules {
        if !rule.matches(name, cache, &mut search.budget) {
            continue;
        }
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

fn response_field(text: &[u8], search: &mut Search<'_>, now: i64) -> Edit {
    // RFC 6265 §5.2 decides whether there is a name-value pair before
    // interpreting any attributes. Treat its name and value as bytes.
    let mut fields = text.split(|&byte| byte == b';').map(<[u8]>::trim_ascii);
    let pair = fields.next().unwrap_or_default();
    let Some(equals) = pair.iter().position(|&byte| byte == b'=') else {
        return Edit::Keep;
    };
    let name = pair[..equals].trim_ascii();
    if name.is_empty() {
        return Edit::Keep;
    }
    match change(name, search) {
        Some(Change::Suppress) => Edit::Remove,
        Some(Change::Attributes(attributes)) => rewrite(pair, fields, attributes, now),
        Some(Change::Exempt) | None => Edit::Keep,
    }
}

fn rewrite<'a>(
    pair: &[u8],
    fields: impl Iterator<Item = &'a [u8]>,
    attributes: Attributes,
    now: i64,
) -> Edit {
    let mut rewritten = pair.to_vec();
    let mut server_max_age = None;
    let mut server_expires = None;
    for field in fields {
        let equals = field
            .iter()
            .position(|&byte| byte == b'=')
            .unwrap_or(field.len());
        let name = field[..equals].trim_ascii_end();
        let value = field.get(equals + 1..).unwrap_or_default();
        if name.eq_ignore_ascii_case(b"max-age") {
            let age = std::str::from_utf8(value).ok().and_then(parse_max_age);
            if attributes.max_age.is_some() || age.is_none() {
                server_max_age = age.or(server_max_age);
                continue;
            }
        } else if name.eq_ignore_ascii_case(b"expires") {
            let expires = std::str::from_utf8(value).ok().and_then(date::parse);
            if attributes.max_age.is_some() || expires.is_none() {
                server_expires = expires.or(server_expires);
                continue;
            }
        } else if name.is_empty()
            || (attributes.same_site.is_some() && name.eq_ignore_ascii_case(b"samesite"))
        {
            continue;
        }
        rewritten.extend_from_slice(b"; ");
        rewritten.extend_from_slice(field);
    }
    if let Some(limit) = attributes.max_age {
        let lifetime =
            server_max_age.or_else(|| server_expires.map(|expires| expires.saturating_sub(now)));
        let max_age = limit.min(lifetime.unwrap_or(limit)).max(0);
        rewritten.extend_from_slice(format!("; Max-Age={max_age}").as_bytes());
    }
    if let Some(same_site) = attributes.same_site {
        rewritten.extend_from_slice(b"; SameSite=");
        rewritten.extend_from_slice(same_site.as_str().as_bytes());
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

    #[test]
    fn cookie_names_can_be_opaque_header_bytes() {
        let rule = parse("/.+/", false).unwrap().compile().unwrap();
        let mut headers = HeaderMap::new();
        headers.append(
            SET_COOKIE,
            HeaderValue::from_bytes(b"\xFF=1; Path=/").unwrap(),
        );
        response(&mut headers, std::iter::once(&rule));
        assert!(!headers.contains_key(SET_COOKIE));
    }

    #[test]
    fn the_cookie_selector_rejects_excessive_regex_fallback_work_at_load() {
        let spec = parse("/[ab]*a[ab]{5000}c/", false).unwrap();
        assert_eq!(
            spec.compile().err(),
            Some(super::super::Rejection::InvalidExpression)
        );
    }

    #[test]
    fn suppression_matches_cookie_names_independently_of_values_and_attributes() {
        for selector in ["track", "/.+/", "/^_ga.*/"] {
            let name = if selector.starts_with("/^_ga") {
                "_ga_1"
            } else {
                "track"
            };
            let rule = parse(selector, false).unwrap().compile().unwrap();
            let mut headers = HeaderMap::new();
            for suffix in [
                "1; Path=/",
                "1; Path=/;",
                "1; Max-Age=oops",
                "one two,three; Secure",
            ] {
                headers.append(
                    SET_COOKIE,
                    HeaderValue::from_str(&format!("{name}={suffix}")).unwrap(),
                );
            }
            headers.append(SET_COOKIE, HeaderValue::from_static("no-pair"));
            response(&mut headers, std::iter::once(&rule));
            assert_eq!(
                headers.get_all(SET_COOKIE).iter().collect::<Vec<_>>(),
                ["no-pair"],
                "{selector}"
            );
        }
    }

    #[test]
    fn rewriting_a_cookie_keeps_the_pair_and_parseable_attributes() {
        let rule = parse("track;sameSite=lax", false)
            .unwrap()
            .compile()
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.append(
            SET_COOKIE,
            HeaderValue::from_static("track=a b,c; Path=/; Max-Age=oops; Secure;"),
        );
        response(&mut headers, std::iter::once(&rule));
        assert_eq!(
            headers[SET_COOKIE],
            "track=a b,c; Path=/; Secure; SameSite=Lax"
        );
    }

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

        // RFC 6265 §5.2 finds a name-value pair before checking attributes.
        fn pair(field: &HeaderValue) -> Option<&[u8]> {
            let pair = field.as_bytes().split(|&b| b == b';').next()?.trim_ascii();
            let equals = pair.iter().position(|&b| b == b'=')?;
            (!pair[..equals].trim_ascii().is_empty()).then_some(pair)
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
