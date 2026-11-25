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

//! Parse an Expires attribute using RFC 6265 section 5.1.1.

use time::{Date, Month, Time};

pub(super) fn parse(value: &str) -> Option<i64> {
    let mut time = None;
    let mut day = None;
    let mut month = None;
    let mut year = None;
    for token in value.as_bytes().split(
        |byte| matches!(*byte, b'\t' | 0x20..=0x2f | 0x3b..=0x40 | 0x5b..=0x60 | 0x7b..=0x7e),
    ) {
        if time.is_none()
            && let Some(found) = time_fields(token)
        {
            time = Some(found);
        } else if day.is_none()
            && let Some((found, _)) = digits(token, 1, 2)
        {
            day = Some(found);
        } else if month.is_none()
            && let Some(found) = month_field(token)
        {
            month = Some(found);
        } else if year.is_none()
            && let Some((found, _)) = digits(token, 2, 4)
        {
            year = Some(found);
        }
    }
    let year = match year? {
        year @ 0..=69 => 2000 + year,
        year @ 70..=99 => 1900 + year,
        year @ 1601.. => year,
        _ => return None,
    };
    let date = Date::from_calendar_date(i32::from(year), month?, day? as u8).ok()?;
    let (hour, minute, second) = time?;
    let time = Time::from_hms(hour, minute, second).ok()?;
    Some(date.with_time(time).assume_utc().unix_timestamp())
}

fn digits(token: &[u8], min: usize, max: usize) -> Option<(u16, &[u8])> {
    let count = token
        .iter()
        .take(max + 1)
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if !(min..=max).contains(&count) {
        return None;
    }
    let (prefix, rest) = token.split_at(count);
    let number = prefix
        .iter()
        .fold(0, |number, byte| number * 10 + u16::from(byte - b'0'));
    Some((number, rest))
}

fn time_fields(token: &[u8]) -> Option<(u8, u8, u8)> {
    let (hour, token) = digits(token, 1, 2)?;
    let (minute, token) = digits(token.strip_prefix(b":")?, 1, 2)?;
    let (second, _) = digits(token.strip_prefix(b":")?, 1, 2)?;
    Some((hour as u8, minute as u8, second as u8))
}

fn month_field(token: &[u8]) -> Option<Month> {
    let mut prefix: [u8; 3] = token.get(..3)?.try_into().ok()?;
    prefix.make_ascii_lowercase();
    Some(match &prefix {
        b"jan" => Month::January,
        b"feb" => Month::February,
        b"mar" => Month::March,
        b"apr" => Month::April,
        b"may" => Month::May,
        b"jun" => Month::June,
        b"jul" => Month::July,
        b"aug" => Month::August,
        b"sep" => Month::September,
        b"oct" => Month::October,
        b"nov" => Month::November,
        b"dec" => Month::December,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn cookie_dates_accept_rfc_examples_and_common_server_formats() {
        for (value, epoch) in [
            ("Wed, 09 Jun 2021 10:18:14 GMT", 1_623_233_894),
            ("Sun, 06 Nov 1994 08:49:37 GMT", 784_111_777),
            ("Sunday, 06-Nov-94 08:49:37 GMT", 784_111_777),
            ("Sun Nov  6 08:49:37 1994", 784_111_777),
            ("Wed, 9-Jun-2021 10:18:14 GMT", 1_623_233_894),
            ("10:18:14utc\t9th/JUNE/2021year", 1_623_233_894),
        ] {
            assert_eq!(parse(value), Some(epoch), "{value}");
        }
    }

    #[test]
    fn cookie_dates_resolve_short_years_and_validate_the_calendar() {
        for (value, epoch) in [
            ("01 Jan 70 0:0:0 GMT", 0),
            ("01 Jan 69 00:00:00 GMT", 3_124_224_000),
            ("29 Feb 00 00:00:00 GMT", 951_782_400),
            ("01 Jan 1601 00:00:00 GMT", -11_644_473_600),
        ] {
            assert_eq!(parse(value), Some(epoch), "{value}");
        }
        for invalid in [
            "",
            "Wed, 9 Jun 2021 GMT",
            "9 Foo 2021 10:18:14 GMT",
            "9 Jun 1 10:18:14 GMT",
            "9 Jun 20210 10:18:14 GMT",
            "9 Jun 1600 10:18:14 GMT",
            "0 Jun 2021 10:18:14 GMT",
            "31 Jun 2021 10:18:14 GMT",
            "29 Feb 1900 00:00:00 GMT",
            "9 Jun 2021 24:00:00 GMT",
            "9 Jun 2021 10:60:14 GMT",
            "9 Jun 2021 10:18:60 GMT",
            "9 Jun 2021 10:18:140 GMT",
            "9 Jun 2021 99:00:00 10:18:14 GMT",
        ] {
            assert_eq!(parse(invalid), None, "{invalid}");
        }
    }
}
