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

//! The hosts the user trusts ("trusted sites" in the app): handed on
//! unread and never blocked, whatever the lists say, for every app. A host
//! stands for itself and every name below it.
//!
//! The file names a host a line.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Result, bail};

use crate::files;
use crate::host::{is_host, suffixes};

/// The longest name DNS has room for.
const MAX_HOST: usize = 253;

/// The file is written by the commands only; anything far larger is not
/// theirs.
pub const MAX_FILE: usize = 1 << 20;

#[derive(Default)]
pub struct Allow {
    hosts: BTreeSet<String>,
}

/// `host` as the file holds it, if it is a host.
pub fn host(host: &str) -> Option<String> {
    (host.len() <= MAX_HOST && is_host(host)).then(|| host.to_ascii_lowercase())
}

impl Allow {
    pub fn load(file: &Path) -> Self {
        Self::try_load(file).unwrap_or_default()
    }

    pub fn try_load(file: &Path) -> std::io::Result<Self> {
        let text = files::read_text(file, MAX_FILE)?;
        Ok(Self::parse(&text))
    }

    fn parse(text: &str) -> Self {
        let hosts = text.lines().filter_map(|l| host(l.trim())).collect();
        Self { hosts }
    }

    /// The hosts, in order.
    pub fn hosts(&self) -> impl Iterator<Item = &str> {
        self.hosts.iter().map(String::as_str)
    }

    /// Whether `host` is one of the hosts or below one.
    pub fn covers(&self, host: &str) -> bool {
        suffixes(host).any(|name| self.hosts.contains(name))
    }

    /// Adds `name` to the hosts in `file`, or removes it.
    pub fn edit(file: &Path, name: &str, add: bool) -> Result<()> {
        let Some(name) = host(name) else {
            bail!(crate::malformed::Malformed(format!("not a host: {name:?}")));
        };
        let _lock = files::lock(file)?;
        let mut allow = Self::try_load(file)?;
        if add {
            allow.hosts.insert(name);
        } else {
            allow.hosts.remove(&name);
        }
        let text: String = allow.hosts().map(|h| format!("{h}\n")).collect();
        files::replace_durable(file, text.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trusted_host_covers_itself_and_the_names_below_it() {
        let a = Allow::parse("Bank.Example\n\nnot a host\n||ads.example^\n");
        assert!(a.covers("bank.example"));
        assert!(a.covers("login.bank.example"));
        assert!(!a.covers("notbank.example"));
        assert!(!a.covers("example"));
        assert_eq!(a.hosts().collect::<Vec<_>>(), ["bank.example"]);
    }

    #[test]
    fn trusted_sites_keep_literal_labels_and_require_ascii_idna_spelling() {
        let a = Allow::parse(
            "bank.example.\nempty..example\nBücher.Example\nxn--caf-dma.example\nINTRANET\n192.0.2.1\n2001:DB8::1\n",
        );
        for host in [
            "login.bank.example.",
            "login..bank.example.",
            "login.empty..example",
            "shop.xn--caf-dma.example",
            "intranet",
            "login.intranet",
            "192.0.2.1",
            "2001:db8::1",
        ] {
            assert!(a.covers(host), "{host:?}");
        }
        for host in [
            "",
            "login.bank.example",
            "login.empty.example",
            "xn--bcher-kva.example",
            "shop.café.example",
            "intranet.",
            "192.0.2.10",
            "2001:db8::10",
        ] {
            assert!(!a.covers(host), "{host:?}");
        }
    }

    #[test]
    fn hosts_are_added_and_removed_and_nothing_else_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("allow");
        Allow::edit(&file, "intranet.example", true).unwrap();
        Allow::edit(&file, "Bank.example", true).unwrap();
        Allow::edit(&file, "bank.example", true).unwrap();
        assert_eq!(
            Allow::load(&file).hosts().collect::<Vec<_>>(),
            ["bank.example", "intranet.example"]
        );
        Allow::edit(&file, "intranet.example", false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "bank.example\n");
        for bad in ["", "a b", "a\nb", "*.example", &"a".repeat(254)] {
            assert!(Allow::edit(&file, bad, true).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn editing_trusted_sites_leaves_unreadable_choices_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("allow");
        for contents in [vec![0xff], vec![b' '; MAX_FILE + 1]] {
            std::fs::write(&file, &contents).unwrap();
            for add in [true, false] {
                assert!(Allow::edit(&file, "bank.example", add).is_err());
                assert_eq!(std::fs::read(&file).unwrap(), contents);
                assert!(!file.with_extension("new").exists());
            }
        }
    }
}
