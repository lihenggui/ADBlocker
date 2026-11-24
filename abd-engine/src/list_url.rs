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
    let base = base_file_name(url);
    if trusted_download(url) {
        format!("{}.trusted.txt", base.trim_end_matches(".txt"))
    } else {
        base
    }
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

pub(crate) fn trusted_download(url: &str) -> bool {
    Trust::Scriptlets.urls().contains(&url) || Trust::ResponseRules.urls().contains(&url)
}

/// Copies made before response-rule trust stay usable for ordinary filtering
/// until fetched again from the trusted address.
pub(crate) fn untrusted_file_name(url: &str) -> Option<String> {
    (Trust::ResponseRules.urls().contains(&url) && !Trust::Scriptlets.urls().contains(&url))
        .then(|| base_file_name(url))
}

pub(crate) fn trusted_list_file(path: &Path, trust: Trust) -> bool {
    // User rules live in a separate directory. A custom URL cannot produce
    // the reserved suffix, even if its base filename hash collides.
    path.parent().and_then(Path::file_name) == Some("lists".as_ref())
        && path.file_name().is_some_and(|name| {
            trust
                .urls()
                .iter()
                .any(|url| name == file_name(url).as_str())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

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
