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

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Result, ensure};
use bytes::Bytes;

use super::MAX_SIZE;
use crate::sources::subscribed;
use crate::{files, sources};

#[derive(Clone, Copy)]
pub(crate) enum ListKind {
    Filter,
    HttpsExclusion,
}

impl ListKind {
    pub(super) fn count(self, text: &str) -> usize {
        match self {
            Self::Filter => crate::rule::count(text),
            Self::HttpsExclusion => text
                .lines()
                .filter(|line| !crate::rule::is_comment(line))
                .count(),
        }
    }

    /// The selection in effect. Rollback can restore the file's absence
    /// during a download: the defaults are still subscribed then.
    pub(super) fn sources(self, file: &Path, bundled: &str) -> Result<String> {
        match self {
            Self::Filter => sources::selection(file, bundled),
            Self::HttpsExclusion => sources::text_or(file, bundled),
        }
    }

    pub(super) fn start_sources(self, file: &Path, bundled: &str) -> Result<String> {
        if matches!(self, Self::Filter) {
            // Filter defaults stay unsaved so that they follow the catalog
            // and the device's languages until the user chooses.
            return self.sources(file, bundled);
        }
        let Some(mut configured) = sources::text_of(file)? else {
            files::replace(file, bundled.as_bytes())?;
            return Ok(bundled.to_owned());
        };
        // Exclusions added to the catalog must also reach installations that
        // already have a sources file. An explicitly empty selection still
        // leaves decryption disabled.
        let current: HashSet<_> = subscribed(&configured).collect();
        if current.is_empty() {
            return Ok(configured);
        }
        let missing: Vec<_> = subscribed(bundled)
            .filter(|url| !current.contains(url))
            .collect();
        if !missing.is_empty() {
            if !configured.ends_with('\n') {
                configured.push('\n');
            }
            for url in missing {
                configured.push_str(url);
                configured.push('\n');
            }
            ensure!(
                configured.len() <= sources::MAX_FILE,
                "HTTPS exclusion sources exceed their size limit"
            );
            files::replace(file, configured.as_bytes())?;
        }
        Ok(configured)
    }

    pub(super) fn accepts(self, body: &[u8], bundled: bool) -> bool {
        if body.len() > MAX_SIZE || !looks_like_a_list(body) {
            return false;
        }
        match self {
            Self::Filter => true,
            Self::HttpsExclusion if bundled => {
                std::str::from_utf8(body).is_ok_and(crate::exclusions::usable_bundled_feed)
            }
            Self::HttpsExclusion => std::str::from_utf8(body)
                .is_ok_and(|text| crate::exclusions::supported_lines(text).next().is_some()),
        }
    }

    pub(super) fn retain_previous(self, file: &Path, new: Bytes) -> Result<Bytes> {
        if matches!(self, Self::Filter) {
            return Ok(new);
        }
        let old = match files::read_bytes(file, MAX_SIZE) {
            Ok(old) => old,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(new),
            Err(e) => return Err(e.into()),
        };
        let old = std::str::from_utf8(&old).unwrap_or_default();
        let new_text = std::str::from_utf8(&new)?;
        let mut seen: HashSet<_> = crate::exclusions::supported_lines(new_text).collect();
        let mut retained = crate::exclusions::supported_lines(old)
            .filter(|line| seen.insert(line))
            .peekable();
        if retained.peek().is_none() {
            return Ok(new);
        }
        // A downloaded update can keep traffic encrypted but cannot broaden
        // the user's consent to decryption by dropping a previous exclusion.
        let mut merged = new.to_vec();
        if !merged.ends_with(b"\n") {
            merged.push(b'\n');
        }
        for line in retained {
            ensure!(
                merged.len() + line.len() < MAX_SIZE,
                "retained HTTPS exclusions exceed their size limit"
            );
            merged.extend_from_slice(line.as_bytes());
            merged.push(b'\n');
        }
        Ok(Bytes::from(merged))
    }
}

/// An error page served with status 200 must not replace a list.
pub(super) fn looks_like_a_list(body: &[u8]) -> bool {
    let start = body.iter().position(|b| !b.is_ascii_whitespace());
    start.is_some_and(|i| body[i] != b'<') && std::str::from_utf8(body).is_ok()
}
