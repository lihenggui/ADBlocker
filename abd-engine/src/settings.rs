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

//! What the user agreed to have done, for every app at once.
//!
//! Each line has a name and a value. Unknown settings are preserved for
//! newer versions. An unknown value is read as the narrower choice: a switch
//! is off, and the scope is the chosen apps.

use std::path::Path;

use anyhow::Result;

/// The most that is read of the file: a few lines.
pub const MAX_SIZE: usize = 4096;

pub const URL_FILTERING: &str = "url-filtering";
pub const COSMETIC: &str = "cosmetic";
pub const SCOPE: &str = "scope";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    Chosen,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Whether request-level rules apply to cleartext HTTP and eligible HTTPS.
    pub url_filtering: bool,
    pub scope: Scope,
    /// Whether what the lists name is hidden on the pages that are
    /// decrypted (`cosmetic.rs`): the one thing that changes what a
    /// server sent.
    pub cosmetic: bool,
}

impl Settings {
    /// The defaults until the user changes them, the same for every
    /// program that runs the engine. URL filtering is on: what it needs
    /// before HTTPS is decrypted (usable exclusions, a verified server, an
    /// app that trusts the authority) is checked for each connection.
    pub const UNSAID: Self = Self {
        url_filtering: true,
        scope: Scope::Chosen,
        cosmetic: false,
    };

    /// What the file says, and `unsaid` for what it does not: the program
    /// that runs the engine knows what its user agreed to by installing
    /// it.
    pub fn try_read(file: &Path, unsaid: Settings) -> std::io::Result<Self> {
        let text = crate::files::read_text(file, MAX_SIZE)?;
        let mut settings = unsaid;
        for line in text.lines() {
            let mut f = line.split_ascii_whitespace();
            match (f.next(), f.next()) {
                (Some(URL_FILTERING), Some(value)) => settings.url_filtering = value == "on",
                (Some(SCOPE), Some("all")) => settings.scope = Scope::All,
                (Some(SCOPE), Some(_)) => settings.scope = Scope::Chosen,
                (Some(COSMETIC), Some(value)) => settings.cosmetic = value == "on",
                _ => {}
            }
        }
        Ok(settings)
    }

    /// Writes one of them, leaving the other lines.
    pub fn set(file: &Path, name: &str, value: &str) -> Result<()> {
        let _lock = crate::files::lock(file)?;
        let text = crate::files::read_text(file, MAX_SIZE)?;
        let mut lines: Vec<&str> = text
            .lines()
            .filter(|line| line.split_ascii_whitespace().next() != Some(name))
            .collect();
        let line = format!("{name} {value}");
        lines.push(&line);
        crate::files::replace_durable(file, (lines.join("\n") + "\n").as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: Settings = Settings {
        url_filtering: true,
        scope: crate::settings::Scope::Chosen,
        cosmetic: false,
    };
    const OFF: Settings = Settings {
        url_filtering: false,
        scope: crate::settings::Scope::Chosen,
        cosmetic: false,
    };

    fn read(file: &Path, unsaid: Settings) -> Settings {
        Settings::try_read(file, unsaid).unwrap()
    }

    #[test]
    fn what_the_file_does_not_say_is_as_the_program_has_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings");
        assert_eq!(read(&file, ON), ON);
        assert_eq!(read(&file, OFF), OFF);
        std::fs::write(&file, "later on\n").unwrap();
        assert_eq!(read(&file, ON), ON);
    }

    #[test]
    fn url_filtering_is_on_until_the_user_turns_it_off() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings");
        assert_eq!(read(&file, Settings::UNSAID), ON);
        Settings::set(&file, SCOPE, "chosen").unwrap();
        assert_eq!(read(&file, Settings::UNSAID), ON);
        Settings::set(&file, URL_FILTERING, "off").unwrap();
        assert_eq!(read(&file, Settings::UNSAID), OFF);
        Settings::set(&file, URL_FILTERING, "on").unwrap();
        assert_eq!(read(&file, Settings::UNSAID), ON);
    }

    #[test]
    fn a_value_not_known_is_taken_for_off() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings");
        for text in [
            "url-filtering off\n",
            "url-filtering some\n",
            "url-filtering ON\n",
        ] {
            std::fs::write(&file, text).unwrap();
            assert_eq!(read(&file, ON), OFF, "{text:?}");
        }
        std::fs::write(&file, "url-filtering on\n").unwrap();
        assert_eq!(read(&file, OFF), ON);
    }

    #[test]
    fn scope_defaults_to_chosen_and_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings");
        assert_eq!(read(&file, OFF).scope, Scope::Chosen);
        Settings::set(&file, SCOPE, "all").unwrap();
        assert_eq!(read(&file, OFF).scope, Scope::All);
        Settings::set(&file, SCOPE, "chosen").unwrap();
        assert_eq!(read(&file, OFF).scope, Scope::Chosen);
    }

    #[test]
    fn setting_one_leaves_the_lines_of_a_newer_program() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings");
        std::fs::write(&file, "decrypt on\nlater on\n").unwrap();
        assert_eq!(read(&file, OFF), OFF);
        Settings::set(&file, URL_FILTERING, "off").unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "decrypt on\nlater on\nurl-filtering off\n"
        );
        assert_eq!(read(&file, ON), OFF);
    }

    #[test]
    fn setting_one_leaves_unreadable_settings_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings");
        for contents in [vec![0xff], vec![b' '; MAX_SIZE + 1]] {
            std::fs::write(&file, &contents).unwrap();
            assert!(Settings::set(&file, URL_FILTERING, "off").is_err());
            assert_eq!(std::fs::read(&file).unwrap(), contents);
            assert!(!file.with_extension("new").exists());
        }
    }
}
