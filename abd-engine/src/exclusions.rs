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

//! The hosts whose connections are never decrypted, whatever the app: the
//! sites of banks and payment services, and others where what passes is
//! too sensitive to be read by anything in between.
//!
//! They come from lists, downloaded like the filter lists and in the
//! format of AdGuard's HTTPS exclusions: a host a line, which stands for
//! itself and every name below it; in quotes, for itself alone; and with
//! `$app=` and package names, for those apps only. A whole-label wildcard
//! matches one nonempty DNS label.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::Path;

use crate::host::{is_host, suffixes};
use crate::list_files::{ListFiles, MAX_LIST, read_list, readable_paths};

/// For whom a host is excluded.
#[derive(Debug, PartialEq, Eq)]
struct Rule {
    /// Not the names below it.
    exact: bool,
    /// Only these packages; every app if none are named.
    apps: Vec<String>,
    /// The labels before a wildcard. The map key holds the labels after it.
    wildcard_prefix: Option<String>,
}

impl Rule {
    fn has_qualified_name(&self, key: &str) -> bool {
        let name = key.trim_end_matches('.');
        (name.contains('.') || self.wildcard_prefix.is_some())
            && name.split('.').all(|label| {
                label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                    && label
                        .as_bytes()
                        .last()
                        .is_some_and(u8::is_ascii_alphanumeric)
            })
    }

    fn matches(&self, package: Option<&str>, ancestors: &str) -> bool {
        if let Some(package) = package
            && !self.apps.is_empty()
            && !self.apps.iter().any(|app| app == package)
        {
            return false;
        }
        let Some(prefix) = &self.wildcard_prefix else {
            return !self.exact || ancestors.is_empty();
        };
        let Some(left) = ancestors.strip_suffix('.') else {
            return false;
        };
        let (before, label) = left.rsplit_once('.').unwrap_or(("", left));
        !label.is_empty()
            && (before == prefix
                || (!self.exact
                    && (prefix.is_empty()
                        || before
                            .strip_suffix(prefix)
                            .is_some_and(|rest| rest.ends_with('.')))))
    }
}

#[derive(Default)]
pub struct Exclusions {
    generation: Option<u64>,
    ready: bool,
    hosts: HashMap<String, Vec<Rule>>,
}

fn parse(line: &str) -> Option<(String, Rule)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
        return None;
    }
    let (host, apps) = match line.split_once("$app=") {
        Some((host, apps)) => (host, apps.split('|').map(str::to_owned).collect()),
        None => (line, Vec::new()),
    };
    let (host, exact) = match host.strip_prefix('"').and_then(|h| h.strip_suffix('"')) {
        Some(quoted) => (quoted, true),
        None => (host, false),
    };
    let (key, wildcard_prefix) = if let Some(suffix) = host.strip_prefix("*.") {
        (normalize_host(suffix)?, Some(String::new()))
    } else if let Some((prefix, suffix)) = host.split_once(".*.") {
        (normalize_host(suffix)?, Some(normalize_host(prefix)?))
    } else {
        (normalize_host(host)?, None)
    };
    Some((
        key,
        Rule {
            exact,
            apps,
            wildcard_prefix,
        },
    ))
}

fn normalize_host(host: &str) -> Option<String> {
    if host.is_ascii() {
        is_host(host).then(|| host.to_ascii_lowercase())
    } else {
        idna::domain_to_ascii_strict(host)
            .ok()
            .filter(|host| is_host(host))
    }
}

/// The feed may also contain rules for other AdGuard implementations.
/// Only rules this engine understands can replace or extend its policy.
pub(crate) fn supported_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|line| parse(line).is_some())
}

/// A bundled feed must contain a qualified name, since a single-word
/// server response can also parse as a private host name.
pub(crate) fn usable_bundled_feed(text: &str) -> bool {
    text.lines()
        .filter_map(parse)
        .any(|(key, rule)| rule.has_qualified_name(&key))
}

impl Exclusions {
    /// Reads every `*.txt` list in `dir`. A file that cannot be read, or
    /// is too large to be a list, is left out.
    pub fn load(dir: &Path) -> Self {
        if let Ok(Some(loaded)) = Self::default().changed(dir) {
            return loaded;
        }
        // At startup there is no previous set to retain. Keep protecting
        // the hosts in every readable list and retry the others on reload.
        let mut partial = Self::default();
        let mut loaded = HashSet::new();
        for path in readable_paths(&[dir]) {
            let _ = partial.read_file(&path, &mut loaded);
        }
        partial.ready = complete(&loaded);
        partial
    }

    pub fn changed(&self, dir: &Path) -> std::io::Result<Option<Self>> {
        let lists = ListFiles::read(&[dir])?;
        if self.generation == Some(lists.generation) {
            return Ok(None);
        }
        let mut loaded = Self {
            generation: Some(lists.generation),
            ..Self::default()
        };
        let mut files = HashSet::new();
        for path in &lists.paths {
            loaded.read_file(path, &mut files)?;
        }
        loaded.ready = complete(&files);
        Ok(Some(loaded))
    }

    fn read_file(&mut self, path: &Path, loaded: &mut HashSet<OsString>) -> std::io::Result<()> {
        if let Some(text) = read_list(path, MAX_LIST)? {
            let mut usable = false;
            for (host, rule) in text.lines().filter_map(parse) {
                usable |= rule.has_qualified_name(&host);
                self.hosts.entry(host).or_default().push(rule);
            }
            if let Some(name) = path.file_name().filter(|_| usable) {
                loaded.insert(name.to_owned());
            }
        }
        Ok(())
    }

    /// Every bundled source has a usable copy, which may be from an earlier update.
    pub fn ready(&self) -> bool {
        self.ready
    }

    #[cfg(test)]
    pub(crate) fn assume_ready(mut self) -> Self {
        self.ready = true;
        self
    }

    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// Whether connections of `package` to `host` are left encrypted.
    pub fn covers(&self, package: &str, host: &str) -> bool {
        // The socket identifies the group, so the engine cannot rule out any
        // package-specific exclusion for this host, including for a new member.
        let package = crate::apps::Identity::parse(package)
            .filter(|identity| !identity.is_group())
            .map(|identity| identity.package);
        suffixes(host).any(|name| {
            let ancestors = &host[..host.len() - name.len()];
            self.hosts
                .get(name)
                .is_some_and(|rules| rules.iter().any(|rule| rule.matches(package, ancestors)))
        })
    }
}

fn complete(loaded: &HashSet<OsString>) -> bool {
    crate::catalog::exclusion_sources()
        .lines()
        .all(|url| loaded.contains(std::ffi::OsStr::new(&crate::list_url::file_name(url))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_requires_a_valid_copy_of_every_bundled_exclusion_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = crate::catalog::exclusion_sources()
            .lines()
            .map(|url| dir.path().join(crate::list_url::file_name(url)));
        let missing = paths.next().unwrap();
        for path in paths {
            std::fs::write(path, "bank.example\n").unwrap();
        }
        assert!(!Exclusions::load(dir.path()).ready());
        std::fs::write(&missing, "// Empty feed\n").unwrap();
        assert!(!Exclusions::load(dir.path()).ready());
        std::fs::write(dir.path().join("private.txt"), "intranet\n").unwrap();
        for bad in ["Unavailable\n", "Unavailable.\n", "Bad..Format\n"] {
            std::fs::write(&missing, bad).unwrap();
            let partial = Exclusions::load(dir.path());
            assert!(!partial.ready(), "{bad}");
            assert!(partial.covers("com.app", "intranet"));
        }
        std::fs::write(&missing, "sensitive.example\n").unwrap();
        assert!(Exclusions::load(dir.path()).ready());
    }

    #[test]
    fn unreadable_lists_preserve_the_other_exclusions_and_are_retried() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("unavailable.txt");
        std::os::unix::fs::symlink(dir.path().join("absent"), &file).unwrap();
        std::fs::write(dir.path().join("banks.txt"), "bank.example").unwrap();
        let partial = Exclusions::load(dir.path());
        assert!(partial.covers("app", "bank.example"));
        assert!(partial.changed(dir.path()).is_err());
        std::fs::remove_file(file).unwrap();
        let recovered = partial.changed(dir.path()).unwrap().unwrap();
        assert!(recovered.changed(dir.path()).unwrap().is_none());
    }

    fn exclusions(text: &str) -> Exclusions {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("list.txt"), text).unwrap();
        Exclusions::load(dir.path())
    }

    #[test]
    fn a_host_stands_for_itself_and_every_name_below_it() {
        let e = exclusions("// banks\nbank.example\n\n# another comment\n");
        assert!(e.covers("com.app", "bank.example"));
        assert!(e.covers("com.app", "login.secure.bank.example"));
        assert!(!e.covers("com.app", "notbank.example"));
        assert!(!e.covers("com.app", "example"));
        assert!(!e.covers("com.app", "bank.example.evil.test"));
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn a_quoted_host_stands_for_itself_alone() {
        let e = exclusions("\"pay.example\"\n");
        assert!(e.covers("com.app", "pay.example"));
        assert!(!e.covers("com.app", "www.pay.example"));
    }

    #[test]
    fn a_host_named_for_some_apps_is_excluded_for_those_only() {
        let e = exclusions("cdn.example$app=org.mozilla.firefox|com.android.chrome\n");
        assert!(e.covers("com.android.chrome", "a.cdn.example"));
        assert!(e.covers("org.mozilla.firefox", "cdn.example"));
        assert!(!e.covers("com.other", "cdn.example"));
    }

    #[test]
    fn a_group_connection_preserves_package_specific_host_exclusions() {
        let e = exclusions("bank.example$app=com.member\n");
        for group in ["@com.shared", "37:@com.shared"] {
            assert!(e.covers(group, "bank.example"));
            assert!(!e.covers(group, "other.example"));
        }
        assert!(!e.covers("37:com.other", "bank.example"));
    }

    #[test]
    fn names_are_compared_without_regard_to_case_of_the_list() {
        let e = exclusions("Bank.Example\n");
        // Names from a ClientHello are lower case already.
        assert!(e.covers("com.app", "bank.example"));
    }

    #[test]
    fn internationalized_names_match_the_ascii_name_sent_in_tls() {
        let e = exclusions("\"Bücher.Example\"$app=com.reader\n");
        assert!(e.covers("com.reader", "xn--bcher-kva.example"));
        assert!(!e.covers("com.reader", "sub.xn--bcher-kva.example"));
        assert!(!e.covers("com.other", "xn--bcher-kva.example"));
    }

    #[test]
    fn exclusions_keep_literal_labels_and_normalize_idna_in_the_list() {
        let e = exclusions(
            "bank.example.\nempty..example\nBücher.Example\nINTRANET\n192.0.2.1\n2001:DB8::1\n\"*.pay.example.\"$app=com.app\n",
        );
        for host in [
            "login.bank.example.",
            "login..bank.example.",
            "login.empty..example",
            "shop.xn--bcher-kva.example",
            "intranet",
            "login.intranet",
            "192.0.2.1",
            "2001:db8::1",
            "login.pay.example.",
        ] {
            assert!(e.covers("com.app", host), "{host:?}");
        }
        for host in [
            "",
            "login.bank.example",
            "login.empty.example",
            "shop.bücher.example",
            "intranet.",
            "192.0.2.10",
            "2001:db8::10",
            ".pay.example.",
            "login..pay.example.",
            "login.pay.example",
        ] {
            assert!(!e.covers("com.app", host), "{host:?}");
        }
        assert!(e.covers("37:@com.shared", "login.pay.example."));
        assert!(!e.covers("37:com.other", "login.pay.example."));
    }

    #[test]
    fn a_wildcard_matches_one_label_between_fixed_labels() {
        let e = exclusions("ping.*.service.example\n");
        for host in [
            "ping.android.service.example",
            "child.ping.android.service.example",
        ] {
            assert!(e.covers("com.app", host), "{host}");
        }
        for host in [
            "ping.service.example",
            "ping.android.extra.service.example",
            "notping.android.service.example",
            "ping.android.service.example.other",
        ] {
            assert!(!e.covers("com.app", host), "{host}");
        }
    }

    #[test]
    fn a_quoted_leading_wildcard_respects_the_app_scope_and_label_count() {
        let e = exclusions("\"*.service.example\"$app=com.reader\n");
        assert!(e.covers("com.reader", "one.service.example"));
        assert!(!e.covers("com.other", "one.service.example"));
        assert!(!e.covers("com.reader", "two.one.service.example"));
        assert!(!e.covers("com.reader", "service.example"));
    }

    #[test]
    fn what_is_not_a_host_is_not_a_rule() {
        let e = exclusions("||ads.example^\nbank.example/path\n*\n<html>\n1.2.3.4\n");
        assert_eq!(e.len(), 1);
        assert!(e.covers("com.app", "1.2.3.4"));
    }

    #[test]
    fn an_oversized_file_is_skipped_whole_and_other_lists_still_exclude() {
        let dir = tempfile::tempdir().unwrap();
        let oversized = dir.path().join("oversized.txt");
        std::fs::write(&oversized, "oversized.example\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(oversized)
            .unwrap()
            .set_len(MAX_LIST as u64 + 1)
            .unwrap();
        std::fs::write(dir.path().join("list.txt"), "bank.example\n").unwrap();
        let e = Exclusions::load(dir.path());
        assert!(e.covers("com.app", "bank.example"));
        assert!(!e.covers("com.app", "oversized.example"));
    }

    #[test]
    fn nothing_is_excluded_without_lists() {
        let missing = tempfile::tempdir().unwrap().path().join("none");
        assert!(!Exclusions::load(&missing).covers("com.app", "bank.example"));
    }
}
