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

//! Translate basic URL syntax inside a playlist rule value.

use super::Rejection;

pub(super) fn translate(pattern: &str) -> Result<String, Rejection> {
    let mut regex = String::new();
    let tail = if let Some(host) = pattern.strip_prefix("||") {
        regex.push_str(r"\A(?:[^./:?#@]+\.)*");
        let end = host
            .find(['/', '^', ':', '?', '#', '|'])
            .unwrap_or(host.len());
        let (host, tail) = host.split_at(end);
        if host.is_empty() || host.contains('@') {
            return Err(Rejection::UnsupportedPattern);
        }
        write(&mut regex, host, r"[^/:?#@]*");
        tail
    } else if let Some(tail) = pattern.strip_prefix('|') {
        regex.push_str(r"\A");
        tail
    } else {
        pattern
    };
    if let Some(tail) = tail.strip_suffix('|') {
        write(&mut regex, tail, ".*");
        regex.push_str(r"\z");
    } else {
        write(&mut regex, tail, ".*");
    }
    if matches!(pattern, "" | "|") {
        return Err(Rejection::EmptyPattern);
    }
    Ok(regex)
}

/// Skip the scheme and credentials so a username cannot match a hostname anchor.
pub(super) fn from_host(url: &str) -> Option<&str> {
    let (scheme, tail) = url.split_once("://")?;
    if !crate::playlist::uri::valid_scheme(scheme) {
        return None;
    }
    let authority = tail.split(['/', '?', '#']).next()?;
    let start = authority.rfind('@').map_or(0, |at| at + 1);
    (start < authority.len()).then(|| &tail[start..])
}

fn write(regex: &mut String, pattern: &str, wildcard: &str) {
    for part in pattern.split_inclusive(['*', '^']) {
        if let Some(text) = part.strip_suffix('*') {
            regex.push_str(&regex::escape(text));
            regex.push_str(wildcard);
        } else if let Some(text) = part.strip_suffix('^') {
            regex.push_str(&regex::escape(text));
            regex.push_str(r"(?:(?-i:[^a-zA-Z0-9_.%-])|\z)");
        } else {
            regex.push_str(&regex::escape(part));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::compile;

    #[test]
    fn basic_patterns_match_only_at_their_anchors_and_separators() {
        for (pattern, yes, no) in [
            (
                "||ads.example^",
                "https://one.ads.example:8443/ad",
                "https://notads.example/ad",
            ),
            (
                "||ads.example^",
                "https://ads.example",
                "https://ads.example.evil/ad",
            ),
            (
                "||ads.example^",
                "https://u:p@ads.example/ad",
                "https://ads.example@elsewhere.test/ad",
            ),
            (
                "||ads.example^",
                "https://ads.example/ad",
                "https://elsewhere.test/ads.example/ad",
            ),
            (
                r"|https:\/\/ads.example",
                "https://ads.example/ad",
                "http://other.test/https://ads.example/ad",
            ),
            (
                "clip.ts|",
                "https://ads.example/clip.ts",
                "https://ads.example/clip.ts?token=1",
            ),
            ("|ad.ts|", "ad.ts", "bad.ts"),
            ("clip^", "clip?x=1", "clip_name"),
            ("clip^", "clip", "clip%20"),
            ("clip^?", "clip/?k", "clip?k"),
            ("clip^", "clip/part", "clip-name"),
            ("clip^", "clip&x=1", "clip.ts"),
            ("part*.ts", "PART.ts", "partyXts"),
            ("part*.ts", "part/123.ts", "part/123mp4"),
            (r"odd?(a)[b]+\,\$", "x/odd?(a)[b]+,$/y", "x/odds(a)[b]+,$/y"),
        ] {
            let compiled = compile(pattern).unwrap();
            assert!(compiled.matches(yes), "{pattern} should match {yes}");
            assert!(!compiled.matches(no), "{pattern} should miss {no}");
        }
    }
}
