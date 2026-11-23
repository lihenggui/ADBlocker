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

//! Where a host's registrable name begins: the public suffix list, read
//! as data.
//!
//! Read into a table once, the list takes a tenth of the space it takes
//! compiled into code, where it was the largest part of each native
//! binary, and a lookup is one probe per label. It answers as the `psl`
//! and `addr` crates do, for the engine and for `adblock`, whose resolver
//! this is: [`install`] before `adblock` parses a rule or a URL.

use std::sync::{LazyLock, Once};

use adblock::url_parser::ResolvesDomain;
use rustc_hash::FxHashMap;

/// The rules of the public suffix list, as the `psl` crate of the version
/// in Cargo.lock carries them (its `data/rules.txt`), which the tests
/// compare with this. A newer `psl` is a newer list to copy here.
const RULES: &str = include_str!("../resources/psl/rules.txt");

/// The version of `psl` whose rules [`RULES`] are.
#[cfg(test)]
const RULES_OF_PSL: &str = "2.1.258";

/// A name that is itself a rule: `co.uk`.
const SUFFIX: u8 = 1;
/// A name under which every label is a suffix: `*.ck`.
const WILDCARD: u8 = 1 << 1;
/// A name a wildcard would make a suffix, which is not: `!www.ck`.
const EXCEPTION: u8 = 1 << 2;
/// A name some rule ends with, so that a longer one can still match.
const NODE: u8 = 1 << 3;

static TABLE: LazyLock<FxHashMap<Box<str>, u8>> = LazyLock::new(|| {
    let mut table = FxHashMap::default();
    for line in RULES.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (name, flag) = if let Some(name) = line.strip_prefix('!') {
            (name, EXCEPTION)
        } else if let Some(name) = line.strip_prefix("*.") {
            (name, WILDCARD)
        } else {
            (line, SUFFIX)
        };
        // Hosts come in either form, the list in the one it was written in.
        let ascii = (!name.is_ascii())
            .then(|| idna::domain_to_ascii(name).ok())
            .flatten();
        for name in std::iter::once(name).chain(ascii.as_deref()) {
            *table.entry(name.into()).or_default() |= flag | NODE;
            let mut rest = name;
            while let Some((_, parent)) = rest.split_once('.') {
                *table.entry(parent.into()).or_default() |= NODE;
                rest = parent;
            }
        }
    }
    table
});

/// The length of the public suffix `name` ends with, without a final dot:
/// the longest rule that matches, an exception before any, and a label
/// the list does not know taken for one, walked from the last label as
/// `psl` walks its tree.
fn suffix_len(name: &str) -> usize {
    let mut best = 0;
    // What a wildcard of the name one label shorter makes a suffix.
    let mut wild = false;
    let mut start = name.len();
    loop {
        let (candidate, dot) = match name[..start].rfind('.') {
            Some(dot) => (&name[dot + 1..], Some(dot)),
            None => (name, None),
        };
        let flags = TABLE.get(candidate).copied().unwrap_or_default();
        if flags & EXCEPTION != 0 {
            // The suffix is the exception less its first label.
            return candidate.split_once('.').map_or(0, |(_, rest)| rest.len());
        }
        // A wildcard covers only the labels no rule names below it: with
        // `*.ex.futurecms.at` listed, `ex.futurecms.at` is not one of
        // `*.futurecms.at`. So `psl` had it.
        if flags & SUFFIX != 0 || (wild && flags & NODE == 0) || best == 0 {
            best = candidate.len();
        }
        wild = flags & WILDCARD != 0;
        match dot {
            Some(dot) if flags & NODE != 0 => start = dot,
            _ => return best,
        }
    }
}

const MAX_DOMAIN_LEN: usize = 253;
const MAX_LABELS_COUNT: usize = 127;
const MAX_LABEL_LEN: usize = 63;

/// A name `addr` would take for a domain name: its validity, as `addr`
/// tells it, with the public suffix it ends with.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Name<'a> {
    full: &'a str,
    suffix_len: usize,
}

impl<'a> Name<'a> {
    pub(crate) fn parse(name: &'a str) -> Option<Self> {
        let stripped = name.strip_suffix('.').unwrap_or(name);
        if stripped.contains('.') {
            is_domain_name(stripped)?;
        } else {
            is_label(stripped, true)?;
        }
        let fqdn = usize::from(name.len() != stripped.len());
        let len = suffix_len(stripped);
        (len != 0).then_some(Self {
            full: name,
            suffix_len: len + fqdn,
        })
    }

    pub(crate) fn suffix(&self) -> &'a str {
        &self.full[self.full.len() - self.suffix_len..]
    }

    /// The registrable name: the suffix and the label before it.
    pub(crate) fn root(&self) -> Option<&'a str> {
        let rest = self
            .full
            .get(..self.full.len().checked_sub(self.suffix_len + 1)?)?;
        let offset = rest.rfind('.').map_or(0, |dot| dot + 1);
        self.full.get(offset..)
    }
}

fn is_domain_name(domain: &str) -> Option<()> {
    // No name has more characters than bytes: most need no counting.
    if domain.len() > MAX_DOMAIN_LEN && domain.chars().count() > MAX_DOMAIN_LEN {
        return None;
    }
    let dots = domain.bytes().filter(|&b| b == b'.').count();
    if dots + 1 > MAX_LABELS_COUNT {
        return None;
    }
    for (i, label) in domain.split('.').enumerate() {
        is_label(label, i == dots)?;
    }
    Some(())
}

fn is_label(label: &str, is_tld: bool) -> Option<()> {
    let ascii_symbol = |c: char| c.is_ascii() && !c.is_alphanumeric();
    let fine = !label.is_empty()
        && (label.len() <= MAX_LABEL_LEN || label.chars().count() <= MAX_LABEL_LEN)
        && !(is_tld && label.parse::<f64>().is_ok())
        && !label.starts_with(ascii_symbol)
        && !label.ends_with(ascii_symbol)
        && !label.contains(|c: char| c != '-' && ascii_symbol(c));
    fine.then_some(())
}

/// `adblock`'s question of a host, answered as its own resolver did.
struct Resolver;

impl ResolvesDomain for Resolver {
    fn get_host_domain(&self, host: &str) -> (usize, usize) {
        if host.is_empty() {
            return (0, 0);
        }
        match Name::parse(host) {
            None => (0, host.len()),
            Some(name) => {
                let domain = name.root().unwrap_or_else(|| name.suffix());
                (host.len() - domain.len(), host.len())
            }
        }
    }

    fn is_public_suffix(&self, domain: &str) -> bool {
        match Name::parse(domain) {
            Some(name) => name.root().is_none(),
            None => !domain.contains('.'),
        }
    }
}

/// Gives `adblock` this resolver, which it needs before it parses a rule
/// or a URL; it panics without one. Cheap after the first call.
pub(crate) fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Set once, here only: nothing else could have set it.
        let _ = adblock::url_parser::set_domain_resolver(Box::new(Resolver));
    });
}

#[cfg(test)]
mod tests {
    use addr::parser::DomainName as _;

    use super::{Name, RULES, Resolver, ResolvesDomain as _};

    /// Names around every rule of the list, in both forms, with what
    /// makes `addr` refuse one, the way `psl` and `addr` see them.
    fn names() -> Vec<String> {
        let mut names = Vec::new();
        for rule in RULES.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let bare = rule.trim_start_matches('!').trim_start_matches("*.");
            let ascii = idna::domain_to_ascii(bare).unwrap_or_default();
            for base in [bare, ascii.as_str()] {
                for prefix in ["", "x.", "www.", "a.b.", "-x.", "1.", "x..", "é."] {
                    names.push(format!("{prefix}{base}"));
                    names.push(format!("{prefix}{base}."));
                }
            }
        }
        for odd in [
            "",
            ".",
            "..",
            "a",
            "a.",
            "localhost",
            "1",
            "1.2.3.4",
            "x.123",
            "x.inf",
            "x.nan",
            "x.1e5",
            "UPPER.COM",
            "a.UPPER.com",
            "a_b.com",
            "a.b_c.com",
            "x.unknowntld",
            "x.y.unknowntld",
            "-a.com",
            "a-.com",
            "a..com",
        ] {
            names.push(odd.to_owned());
        }
        names.push(format!("{}.com", "a".repeat(64)));
        names.push(format!("{}.com", "a.".repeat(130)));
        names.push(format!("{}.com", "abcdefghi.".repeat(25)));
        names
    }

    /// The tests only try names around the rules this list has: one that
    /// only a newer `psl` has would never be tried.
    #[test]
    fn the_list_is_that_of_the_psl_in_cargo_lock() {
        let lock = include_str!("../../Cargo.lock");
        let locked = lock
            .split("[[package]]")
            .find(|p| p.contains("\nname = \"psl\"\n"))
            .and_then(|p| p.lines().find_map(|l| l.strip_prefix("version = ")))
            .map(|v| v.trim_matches('"'));
        assert_eq!(
            locked,
            Some(super::RULES_OF_PSL),
            "copy data/rules.txt of the psl in Cargo.lock to resources/psl/rules.txt \
             and set RULES_OF_PSL to its version",
        );
    }

    #[test]
    fn every_name_is_taken_apart_as_addr_and_psl_did() {
        for name in names() {
            let ours = Name::parse(&name);
            let theirs = addr::psl::List.parse_domain_name(&name);
            assert_eq!(ours.is_some(), theirs.is_ok(), "{name:?}");
            if let (Some(ours), Ok(theirs)) = (ours, theirs) {
                assert_eq!(ours.suffix(), theirs.suffix(), "{name:?}");
                assert_eq!(ours.root(), theirs.root(), "{name:?}");
            }
            let raw = psl::suffix(name.as_bytes()).map(|s| s.as_bytes().len());
            let stripped = name.strip_suffix('.').unwrap_or(&name);
            let fqdn = usize::from(stripped.len() != name.len());
            let len = super::suffix_len(stripped) + fqdn;
            assert_eq!((len != 0).then_some(len), raw, "{name:?}");
        }
    }

    #[test]
    fn adblock_is_told_of_every_name_what_its_own_resolver_said() {
        for host in names() {
            let theirs = match addr::psl::List.parse_domain_name(&host) {
                _ if host.is_empty() => (0, 0),
                Err(_) => (0, host.len()),
                Ok(d) => (
                    host.len() - d.root().unwrap_or_else(|| d.suffix()).len(),
                    host.len(),
                ),
            };
            assert_eq!(Resolver.get_host_domain(&host), theirs, "{host:?}");
            let public = match addr::psl::List.parse_domain_name(&host) {
                Ok(parsed) => parsed.root().is_none(),
                Err(_) => !host.contains('.'),
            };
            assert_eq!(Resolver.is_public_suffix(&host), public, "{host:?}");
        }
    }

    /// How long a lookup takes, against what `addr` and `psl` took:
    /// `cargo test --release -p abd-engine public_suffix -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn a_lookup_takes_no_longer_than_it_did() {
        use std::hint::black_box;
        use std::time::Instant;
        let hosts: Vec<String> = [
            "www.google.com",
            "pagead2.googlesyndication.com",
            "a.b.c.example.co.uk",
            "cdn.jsdelivr.net",
            "x.y.blogspot.com",
            "static.xx.fbcdn.net",
            "api.weibo.cn",
            "img.alicdn.com",
            "foo.bar.unknowntld",
            "news.bbc.co.uk",
            "a.ex.futurecms.at",
            "s3.dualstack.us-east-1.amazonaws.com",
            "telemetry.example.github.io",
        ]
        .iter()
        .map(|h| (*h).to_owned())
        .collect();
        let started = Instant::now();
        let _ = super::TABLE.len();
        let table = started.elapsed();
        let rounds = 200_000;
        let time = |f: &dyn Fn(&str) -> usize| {
            let start = Instant::now();
            for _ in 0..rounds {
                for host in &hosts {
                    black_box(f(black_box(host)));
                }
            }
            start.elapsed().as_nanos() as f64 / (rounds * hosts.len()) as f64
        };
        let ours = time(&|h| Name::parse(h).and_then(|n| n.root()).map_or(0, str::len));
        let addr = time(&|h| {
            addr::psl::List
                .parse_domain_name(h)
                .ok()
                .and_then(|n| n.root())
                .map_or(0, str::len)
        });
        let ours_raw = time(&|h| super::suffix_len(h));
        let psl_raw = time(&|h| psl::suffix(h.as_bytes()).map_or(0, |s| s.as_bytes().len()));
        println!("table built in {table:?}");
        println!("parse and root: {ours:.1} ns, addr {addr:.1} ns");
        println!("suffix alone:   {ours_raw:.1} ns, psl {psl_raw:.1} ns");
    }
}
