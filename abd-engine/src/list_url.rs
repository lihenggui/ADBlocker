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

//! List source addresses and the stable filenames derived from them.

use std::path::Path;

use anyhow::{Context as _, Result, ensure};

pub(crate) use crate::catalog::Trust;

/// The longest address taken.
pub(crate) const MAX_URL: usize = 1024;

const PREVIOUS_UBLOCK_FILTERS: &str =
    "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/filters.txt";
const PREVIOUS_UBLOCK_PRIVACY: &str =
    "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/privacy.txt";
const PREVIOUS_UBLOCK_BADWARE: &str =
    "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/badware.txt";
const PREVIOUS_TRUSTED: &[&str] = &[
    PREVIOUS_UBLOCK_FILTERS,
    PREVIOUS_UBLOCK_PRIVACY,
    PREVIOUS_UBLOCK_BADWARE,
];

/// Catalog addresses saved by earlier releases and their replacements.
pub(crate) const MOVED: &[(&str, &str)] = &[
    (
        "https://filters.adtidy.org/extension/ublock/filters/11.txt",
        "https://filters.adtidy.org/android/filters/11.txt",
    ),
    (
        "https://filters.adtidy.org/extension/ublock/filters/224.txt",
        "https://filters.adtidy.org/android/filters/224.txt",
    ),
    (
        "https://filters.adtidy.org/extension/ublock/filters/2_without_easylist.txt",
        "https://filters.adtidy.org/android/filters/2.txt",
    ),
    (
        PREVIOUS_UBLOCK_FILTERS,
        "https://ublockorigin.github.io/uAssetsCDN/filters/filters.min.txt",
    ),
    (
        PREVIOUS_UBLOCK_PRIVACY,
        "https://ublockorigin.github.io/uAssetsCDN/filters/privacy.min.txt",
    ),
    (
        PREVIOUS_UBLOCK_BADWARE,
        "https://ublockorigin.github.io/uAssetsCDN/filters/badware.min.txt",
    ),
];

pub(crate) fn previous_address(url: &str) -> Option<&'static str> {
    MOVED
        .iter()
        .find_map(|&(old, new)| (new == url).then_some(old))
}

/// Whether `url` is an address a list can be fetched from.
pub fn valid(url: &str) -> bool {
    url.len() <= MAX_URL
        && url.bytes().all(|b| b.is_ascii_graphic())
        && split(url)
            .is_ok_and(|(host, _, _)| rustls::pki_types::ServerName::try_from(host).is_ok())
}

/// The most of an address that is kept in the name of its file.
const MAX_NAME: usize = 100;

/// The file a list is kept in: its address, with what a file name cannot
/// hold replaced, for whoever looks into the directory; and a number made
/// from the whole address. Without it two addresses that differ only in
/// what was replaced or cut off would have one file, and the list of one
/// be taken for the other's, and deleted with it.
pub fn file_name(url: &str) -> String {
    if trusted_download(url) {
        trusted_file_name(url)
    } else {
        base_file_name(url)
    }
}

fn trusted_file_name(url: &str) -> String {
    let base = base_file_name(url);
    format!("{}.trusted.txt", base.trim_end_matches(".txt"))
}

/// The name without the suffix reserved for trusted lists: that of every
/// other list, and the stem of the file of what is known of a list.
pub(crate) fn base_file_name(url: &str) -> String {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let rest = rest.strip_suffix(".txt").unwrap_or(rest);
    let name: String = rest
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_NAME)
        .collect();
    // FNV-1a, written out: the name of a file must be the same in every
    // version, which no hash of the standard library promises to be.
    let hash = url.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    // Read back by the extension: see `Filters::load`.
    format!("{name}-{hash:016x}.txt")
}

pub fn split(url: &str) -> Result<(&str, u16, &str)> {
    let rest = url
        .strip_prefix("https://")
        .context("only https addresses are fetched")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().context("port")?),
        None => (authority, 443),
    };
    ensure!(!host.is_empty(), "no host in {url}");
    Ok((host, port, path))
}

pub(crate) fn same_origin(first: &str, second: &str) -> bool {
    matches!((split(first), split(second)), (Ok((a, p, _)), Ok((b, q, _)))
        if p == q && a.eq_ignore_ascii_case(b))
}

/// An include addresses a file within the containing file's directory.
pub(crate) fn include_address(including: &str, argument: &str) -> Option<String> {
    if argument.len() > MAX_URL || !argument.bytes().all(|b| b.is_ascii_graphic()) {
        return None;
    }
    let (host, port, containing_path) = split(including).ok()?;
    let containing_path = normalized_path(path_and_query(containing_path).0)?;
    let (directory, _) = containing_path.rsplit_once('/')?;
    let directory = format!("{directory}/");
    let is_absolute = argument
        .get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"));
    let absolute;
    let candidate = if is_absolute {
        absolute = format!("https://{}", &argument[8..]);
        if !same_origin(including, &absolute) {
            return None;
        }
        split(&absolute).ok()?.2
    } else {
        // The reference treats a single leading slash as directory-relative.
        if argument.starts_with("//") || argument.split(['/', '?', '#']).next()?.contains(':') {
            return None;
        }
        argument.strip_prefix('/').unwrap_or(argument)
    };
    let (path, query) = path_and_query(candidate);
    if path.is_empty() {
        return None;
    }
    let relative;
    let path = if is_absolute {
        path
    } else {
        relative = format!("{directory}{path}");
        &relative
    };
    let path = normalized_path(path)?;
    if !path.starts_with(&directory) {
        return None;
    }
    let address = https_address(host, port, &path, query);
    valid(&address).then_some(address)
}

/// Identify equivalent spellings of the root when checking for cycles.
pub(crate) fn include_identity(url: &str) -> Option<String> {
    let (host, port, request) = split(url).ok()?;
    let (path, query) = path_and_query(request);
    Some(https_address(host, port, &normalized_path(path)?, query))
}

fn path_and_query(request: &str) -> (&str, &str) {
    let request = request.split('#').next().unwrap_or_default();
    request.split_at(request.find('?').unwrap_or(request.len()))
}

fn https_address(host: &str, port: u16, path: &str, query: &str) -> String {
    let host = host.to_ascii_lowercase();
    if port == 443 {
        format!("https://{host}{path}{query}")
    } else {
        format!("https://{host}:{port}{path}{query}")
    }
}

fn normalized_path(path: &str) -> Option<String> {
    let mut segments = Vec::new();
    let mut input = path.strip_prefix('/')?.split('/').peekable();
    while let Some(segment) = input.next() {
        let lower = segment.to_ascii_lowercase();
        if segment.contains('\\')
            || ["%2f", "%5c", "%25"]
                .iter()
                .any(|encoded| lower.contains(encoded))
        {
            return None;
        }
        match lower.as_str() {
            "." | "%2e" => (),
            ".." | ".%2e" | "%2e." | "%2e%2e" => {
                segments.pop()?;
            }
            _ => {
                segments.push(segment);
                continue;
            }
        }
        if input.peek().is_none() {
            segments.push("");
        }
    }
    Some(format!("/{}", segments.join("/")))
}

pub(crate) fn trusted_download(url: &str) -> bool {
    [Trust::Scriptlets, Trust::Scripts, Trust::ResponseRules]
        .into_iter()
        .any(|trust| trust.urls().contains(&url))
}

/// Copies made before response-rule trust stay usable for ordinary filtering
/// until fetched again from the trusted address. Mobile Ads already has
/// copies under this name when its Android feed gains scriptlet trust.
pub(crate) fn untrusted_file_name(url: &str) -> Option<String> {
    (Trust::ResponseRules.urls().contains(&url)
        && (!Trust::Scriptlets.urls().contains(&url)
            || url == "https://filters.adtidy.org/android/filters/11.txt"))
        .then(|| base_file_name(url))
}

pub(crate) fn copy_names(url: &str) -> impl Iterator<Item = String> {
    let (primary, fallback) = if PREVIOUS_TRUSTED.contains(&url) {
        (trusted_file_name(url), Some(base_file_name(url)))
    } else {
        (file_name(url), untrusted_file_name(url))
    };
    std::iter::once(primary).chain(fallback)
}

pub(crate) fn trusted_list_file(path: &Path, trust: Trust) -> bool {
    let was_trusted = matches!(trust, Trust::Scriptlets | Trust::ResponseRules);
    // User rules live in a separate directory. A custom URL cannot produce
    // the reserved suffix, even if its base filename hash collides.
    path.parent().and_then(Path::file_name) == Some("lists".as_ref())
        && path.file_name().is_some_and(|name| {
            trust.urls().iter().any(|url| name == file_name(url).as_str())
                // Only copies published with the old trusted suffix keep
                // their permissions while waiting for the replacement.
                || (was_trusted
                    && PREVIOUS_TRUSTED
                        .iter()
                        .any(|url| name == trusted_file_name(url).as_str()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ublock_trust_moves_to_the_exact_cdn_addresses_and_preserves_already_trusted_copies() {
        let lists = Path::new("lists");
        for &(old, new) in MOVED
            .iter()
            .filter(|(old, _)| PREVIOUS_TRUSTED.contains(old))
        {
            assert!(!trusted_download(old));
            assert!(trusted_download(new));
            let old_plain = lists.join(file_name(old));
            let old_trusted = lists.join(trusted_file_name(old));
            let new_trusted = lists.join(file_name(new));
            let renamed = lists.join(file_name(&format!("{new}?custom")));
            let forged = lists.join(trusted_file_name(&format!("{new}?custom")));
            for trust in [Trust::Scriptlets, Trust::ResponseRules] {
                let urls = trust.urls();
                assert!(urls.contains(&new));
                assert!(!urls.contains(&old));
            }
            for path in [old_trusted, new_trusted] {
                assert!(trusted_list_file(&path, Trust::Scriptlets));
                assert!(trusted_list_file(&path, Trust::ResponseRules));
                assert!(!trusted_list_file(&path, Trust::Scripts));
            }
            for path in [old_plain, renamed, forged] {
                assert!(!trusted_list_file(&path, Trust::Scriptlets));
                assert!(!trusted_list_file(&path, Trust::ResponseRules));
                assert!(!trusted_list_file(&path, Trust::Scripts));
            }
        }
    }
    #[test]
    fn splits_an_address_into_host_port_and_path() {
        assert_eq!(
            split("https://a.example/x/y.txt").unwrap(),
            ("a.example", 443, "/x/y.txt")
        );
        assert_eq!(
            split("https://a.example:8443").unwrap(),
            ("a.example", 8443, "/")
        );
        assert!(split("http://a.example/").is_err());
    }

    #[test]
    fn names_the_file_of_a_list_after_its_address() {
        let name = file_name("https://a.example/x/y.txt");
        assert!(name.starts_with("a.example_x_y-") && name.ends_with(".txt"));
        assert!(file_name("https://a.example/list?id=3").starts_with("a.example_list_id_3-"));
        // The same in every version: files of an older one are found.
        assert_eq!(name, "a.example_x_y-1dd2b8330bc21664.txt");
    }

    #[test]
    fn two_addresses_never_share_a_file() {
        // The same once what a name cannot hold is replaced.
        assert_ne!(
            file_name("https://a.example/x/y.txt"),
            file_name("https://a.example/x_y.txt")
        );
        // The same as far as a name goes.
        let long = format!("https://a.example/{}", "x".repeat(200));
        let (one, other) = (
            file_name(&format!("{long}1")),
            file_name(&format!("{long}2")),
        );
        assert_ne!(one, other);
        assert!(one.len() <= MAX_NAME + 21);
    }
}
