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

//! The addresses the filter lists are downloaded from, and what is known of
//! each list.
//!
//! `sources` holds an address a line. A source that is disabled stays in
//! it, as `#` and its address with nothing between: a comment, for
//! `update.rs`, that is told from others by being an address.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::files;
use crate::list_files::{MAX_LIST, check_budget, read_list};
pub use crate::list_url::valid;
use crate::list_url::{MOVED, base_file_name, copy_names, previous_address};
use crate::malformed::Malformed;
use crate::paths::Paths;

/// The file is the user's and the commands', not the network's; anything
/// far larger is no list of sources.
pub const MAX_FILE: usize = 1 << 20;
const MAX_DISABLED_BYTES: u64 = 32 << 20;

#[derive(Debug, PartialEq, Eq)]
pub struct Source {
    pub url: String,
    pub enabled: bool,
}

pub(crate) fn subscribed(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

fn parse(text: &str) -> Vec<Source> {
    text.lines()
        .map(str::trim)
        .filter_map(|line| match line.strip_prefix('#') {
            Some(url) => valid(url).then(|| Source {
                url: url.to_owned(),
                enabled: false,
            }),
            None => (!line.is_empty()).then(|| Source {
                url: line.to_owned(),
                enabled: true,
            }),
        })
        .collect()
}

/// What the sources file `file` holds, or `default` while there is none.
/// A file that cannot be told to be there is an error, not one missing:
/// what is built on the defaults would replace what the user chose.
pub(crate) fn text_or(file: &Path, default: &str) -> Result<String> {
    Ok(text_of(file)?.unwrap_or_else(|| default.to_owned()))
}

/// What the sources file `file` holds, if there is one.
pub(crate) fn text_of(file: &Path) -> Result<Option<String>> {
    if !file.try_exists()? {
        return Ok(None);
    }
    Ok(Some(moved(files::read_text(file, MAX_FILE)?)))
}

/// A saved choice follows the catalog's current address.
fn moved(text: String) -> String {
    let has = |url: &str| text.lines().any(|line| address(line) == url);
    let pending: Vec<_> = MOVED.iter().filter(|(old, _)| has(old)).collect();
    if pending.is_empty() {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let url = address(line);
        match pending.iter().find(|(old, _)| *old == url) {
            // The new address chosen as well: its line is the choice.
            Some((_, new)) if has(new) => continue,
            Some((_, new)) => {
                if line.trim().starts_with('#') {
                    out.push('#');
                }
                out.push_str(new);
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// The selection of filter lists in effect: what the sources file `file`
/// holds and each of `default` that it does not name, or `default` while
/// there is none.
///
/// A saved selection says what the user chose among the lists it names. A
/// list that became a default after it was saved is on until they turn it
/// off, as it is for a new installation.
pub(crate) fn selection(file: &Path, default: &str) -> Result<String> {
    let Some(mut text) = text_of(file)? else {
        return Ok(default.to_owned());
    };
    let unnamed: Vec<&str> = {
        let named: HashSet<&str> = text.lines().map(address).collect();
        subscribed(default)
            .filter(|url| !named.contains(url))
            .collect()
    };
    if !unnamed.is_empty() && !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for url in unnamed {
        text.push_str(url);
        text.push('\n');
    }
    Ok(text)
}

/// The list a line of the sources file names, turned on or off.
fn address(line: &str) -> &str {
    let line = line.trim();
    line.strip_prefix('#').unwrap_or(line)
}

/// The selection in effect for the lists of `paths`.
pub fn configured(paths: &Paths) -> Result<String> {
    selection(&paths.sources(), crate::catalog::default_sources())
}

/// The sources, in the order of the file; those a new installation has if
/// there is none yet.
pub fn read(paths: &Paths) -> Vec<Source> {
    parse(&configured(paths).unwrap_or_default())
}

pub enum Change {
    Add,
    Remove,
    Enable,
    Disable,
}

/// Applies one confirmed selection without disturbing other subscriptions.
pub fn configure(paths: &Paths, selection: &str) -> Result<()> {
    let mut choices = BTreeMap::new();
    for line in selection.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (url, enabled) = match line.strip_prefix('#') {
            Some(url) => (url, false),
            None => (line, true),
        };
        if !valid(url) || choices.insert(url, enabled).is_some() {
            bail!(Malformed(
                "invalid or duplicate subscription in the selection".to_owned()
            ));
        }
    }
    let path = paths.sources();
    let lock = files::lock(&path)?;
    let current = configured(paths)?;
    let mut remaining = choices.clone();
    let mut next = String::with_capacity(current.len());
    for line in current.lines() {
        let url = line.trim().trim_start_matches('#');
        if let Some(enabled) = remaining.remove(url) {
            append_source(&mut next, url, enabled);
        } else if !choices.contains_key(url) {
            next.push_str(line);
            next.push('\n');
        }
    }
    for (url, enabled) in remaining {
        append_source(&mut next, url, enabled);
    }
    validate_size(&next)?;
    files::replace(&path, next.as_bytes())?;
    prune_unsubscribed(&next, &paths.lists(), Some(&paths.list_info()), &lock)?;
    Ok(())
}

fn append_source(out: &mut String, url: &str, enabled: bool) {
    if !enabled {
        out.push('#');
    }
    out.push_str(url);
    out.push('\n');
}

fn validate_size(selection: &str) -> Result<()> {
    if selection.len() > MAX_FILE {
        bail!(Malformed(
            "subscription file exceeds its size limit".to_owned()
        ));
    }
    Ok(())
}

/// Changes the source `url`. Returns whether the active files changed or
/// were withdrawn. A disabled list can be restored without a download.
pub fn edit(paths: &Paths, url: &str, change: Change) -> Result<bool> {
    if !valid(url) {
        bail!(Malformed(format!(
            "not an https address of a list: {url:?}"
        )));
    }
    let _lock = files::lock(&paths.sources())?;
    let text = configured(paths)?;
    let disabled = format!("#{url}");
    let is = |line: &str, as_: &str| line.trim() == as_;
    let present = text.lines().any(|l| is(l, url) || is(l, &disabled));
    let mut lines: Vec<&str> = text.lines().collect();
    match change {
        Change::Add if !present => lines.push(url),
        Change::Add | Change::Enable => {
            for line in &mut lines {
                if is(line, &disabled) {
                    *line = url;
                }
            }
        }
        // A default that the file does not name is on: one that is removed
        // stays named, turned off.
        Change::Remove if !subscribed(crate::catalog::default_sources()).any(|d| d == url) => {
            lines.retain(|l| !is(l, url) && !is(l, &disabled));
        }
        Change::Remove | Change::Disable => {
            for line in &mut lines {
                if is(line, url) {
                    *line = &disabled;
                }
            }
        }
    }
    if !present && matches!(change, Change::Enable | Change::Disable) {
        bail!("not a source: {url}");
    }
    let mut new = lines.join("\n");
    new.push('\n');
    validate_size(&new)?;
    match change {
        Change::Add | Change::Enable => {
            check_restore_budget(paths, url)?;
            files::replace(&paths.sources(), new.as_bytes())?;
            let dir = paths.lists();
            let (address, disabled) = if has_copy(&dir, url, "txt") {
                (url, copy_path(&dir, url, "disabled"))
            } else {
                preferred_copy(&dir, url, "disabled")
            };
            let restored = restore_copy(&dir, address, &disabled)?;
            if has_copy(&dir, url, "txt") {
                let previous_active =
                    previous_address(url).is_some_and(|old| has_copy(&dir, old, "txt"));
                prune_previous_copy(&dir, Some(&paths.list_info()), url)?;
                return Ok(restored || previous_active);
            }
            Ok(restored)
        }
        Change::Disable | Change::Remove => {
            withdraw(paths, url, &new, matches!(change, Change::Disable))?;
            Ok(true)
        }
    }
}

fn withdraw(paths: &Paths, url: &str, selection: &str, keep_cached: bool) -> Result<()> {
    let dir = paths.lists();
    let had_current = has_copy(&dir, url, "txt") || has_disabled_copy(&dir, url);
    let occupied = if keep_cached {
        disabled_bytes(&dir)?
    } else {
        0
    };
    let mut parked = Vec::new();
    // Withdraw the active copy before recording the choice, but keep enough
    // information to restore the file if saving the choice fails.
    let result = (|| -> std::io::Result<()> {
        let active_copies = std::iter::once(url)
            .chain(previous_address(url))
            .flat_map(|address| copy_paths(&dir, address, "txt"));
        for active in active_copies {
            let disabled = active.with_extension("disabled");
            match std::fs::rename(&active, &disabled) {
                Ok(()) => parked.push((active, disabled)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
        }
        files::replace(&paths.sources(), selection.as_bytes())
    })();
    if let Err(error) = result {
        let mut recovery = Ok(());
        for (active, disabled) in parked {
            if let Err(restore) = std::fs::rename(disabled, active) {
                recovery = Err(restore);
            }
        }
        recovery.context("restoring lists after a failed subscription change")?;
        return Err(error.into());
    }
    if keep_cached {
        let mut available = MAX_DISABLED_BYTES.saturating_sub(occupied);
        for (_, disabled) in parked {
            if let Some(left) = available.checked_sub(disabled.metadata()?.len()) {
                available = left;
            } else {
                files::remove_if_present(&disabled)?;
            }
        }
    } else {
        for disabled in copy_paths(&dir, url, "disabled") {
            files::remove_if_present(&disabled)?;
        }
    }
    if !has_disabled_copy(&dir, url) {
        files::remove_if_present(&info_file(&paths.list_info(), url))?;
    }
    if !keep_cached || had_current {
        prune_previous_copy(&dir, Some(&paths.list_info()), url)?;
    }
    Ok(())
}

/// Keep the copy under its untrusted name available until a fresh download
/// is saved under the trusted name. Either copy follows subscription changes.
fn copy_paths(dir: &Path, url: &str, extension: &str) -> impl Iterator<Item = PathBuf> {
    copy_names(url).map(move |name| dir.join(name).with_extension(extension))
}

fn copy_path(dir: &Path, url: &str, extension: &str) -> PathBuf {
    let mut copies = copy_paths(dir, url, extension);
    let current = copies.next().expect("current list name");
    if !current.is_file()
        && let Some(untrusted) = copies.find(|path| path.is_file())
    {
        return untrusted;
    }
    current
}

/// Keep the original filename because it determines which rules are trusted.
pub(crate) fn active_copy<'a>(dir: &Path, url: &'a str) -> (&'a str, PathBuf) {
    preferred_copy(dir, url, "txt")
}

fn preferred_copy<'a>(dir: &Path, url: &'a str, extension: &str) -> (&'a str, PathBuf) {
    let current = copy_path(dir, url, extension);
    if let Some(old) = previous_address(url)
        && !current.is_file()
    {
        let previous = copy_path(dir, old, extension);
        if previous.is_file() {
            return (old, previous);
        }
    }
    (url, current)
}

/// Only the selected list's previous address is affected by a targeted update.
pub(crate) fn prune_previous_copy(
    dir: &Path,
    info: Option<&Path>,
    url: &str,
) -> std::io::Result<()> {
    if let Some(old) = previous_address(url) {
        for path in ["txt", "disabled", "new"]
            .into_iter()
            .flat_map(|ext| copy_paths(dir, old, ext))
        {
            files::remove_if_present(&path)?;
        }
        if let Some(info) = info {
            let metadata = info_file(info, old);
            files::remove_if_present(&metadata)?;
            files::remove_if_present(&metadata.with_extension("new"))?;
        }
    }
    Ok(())
}

fn has_copy(dir: &Path, url: &str, extension: &str) -> bool {
    copy_path(dir, url, extension).is_file()
}

pub(crate) fn has_disabled_copy(dir: &Path, url: &str) -> bool {
    has_copy(dir, url, "disabled")
}

pub(crate) fn parked_previous_copy(dir: &Path, url: &str, configured: &str) -> Option<PathBuf> {
    let old = previous_address(url)?;
    configured
        .lines()
        .map(str::trim)
        .any(|line| line.strip_prefix('#') == Some(url))
        .then(|| copy_path(dir, old, "disabled"))
        .filter(|path| path.is_file())
}

pub(crate) fn check_parked_replacement(
    dir: &Path,
    previous: &Path,
    replacement: &Path,
    bytes: usize,
) -> Result<()> {
    let existing = previous
        .metadata()?
        .len()
        .saturating_add(replacement.metadata().map_or(0, |m| m.len()));
    let total = disabled_bytes(dir)?
        .saturating_sub(existing)
        .saturating_add(bytes as u64);
    if total > MAX_DISABLED_BYTES && bytes as u64 > existing {
        bail!("disabled lists exceed their size limit");
    }
    Ok(())
}

fn check_restore_budget(paths: &Paths, url: &str) -> std::io::Result<()> {
    let dir = paths.lists();
    let (address, disabled) = preferred_copy(&dir, url, "disabled");
    if has_copy(&dir, url, "txt") || has_copy(&dir, address, "txt") {
        return Ok(());
    }
    if let Ok(metadata) = disabled.metadata() {
        let (_, active) = active_copy(&dir, url);
        check_budget(&[&dir, &paths.rules()], &[&active], metadata.len())?;
    }
    Ok(())
}

fn disabled_bytes(dir: &Path) -> std::io::Result<u64> {
    let mut occupied = 0u64;
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "disabled")
                {
                    occupied = occupied.saturating_add(entry.metadata()?.len());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e),
    }
    Ok(occupied)
}

fn restore_copy(dir: &Path, url: &str, disabled: &Path) -> std::io::Result<bool> {
    if !disabled.try_exists()? {
        return Ok(false);
    }
    if copy_path(dir, url, "txt").try_exists()? {
        files::remove_if_present(disabled)?;
        return Ok(false);
    }
    match std::fs::rename(disabled, disabled.with_extension("txt")) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// What is known of a list, kept by `abd update`.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub url: String,
    /// From its `! Title:` line.
    pub title: Option<String>,
    /// Its active rule lines.
    pub rules: Option<usize>,
    /// Which rule-counting policy produced the saved count.
    #[serde(default)]
    pub rules_format: u8,
    /// When it was last fetched and found to be a list.
    pub updated: Option<u64>,
    /// Why it was not, the last time; none once it is.
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validator: Option<Validator>,
}

pub(crate) const RULE_COUNT_FORMAT: u8 = 1;

impl Info {
    pub(crate) fn refresh_rule_count(&mut self, path: &Path) {
        if self.rules_format == RULE_COUNT_FORMAT && self.rules.is_some() {
            return;
        }
        self.rules = None;
        if let Ok(Some(text)) = read_list(path, MAX_LIST) {
            self.rules = Some(crate::rule::count(&text));
            self.rules_format = RULE_COUNT_FORMAT;
            self.title = self.title.take().or_else(|| title(&text));
        }
    }
}

/// A validator belongs to exactly one representation at one URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validator {
    pub resource: String,
    pub tag: Option<String>,
    pub modified: Option<String>,
    pub file: [u64; 5],
}

/// Where what is known of the list of `url` is kept: next to the list, in
/// a directory of its own, and not as a `*.txt` file, which would be
/// loaded as one.
pub fn info_file(dir: &Path, url: &str) -> PathBuf {
    let name = base_file_name(url);
    let stem = name.strip_suffix(".txt").unwrap_or(&name);
    dir.join(format!("{stem}.json"))
}

pub fn info(dir: &Path, url: &str) -> Option<Info> {
    let text = files::read_text(&info_file(dir, url), 64 << 10).ok()?;
    let info: Info = serde_json::from_str(&text).ok()?;
    // Two addresses can share a file name; this is the other's.
    (info.url == url).then_some(info)
}

/// A file can outlive its metadata after an interrupted update, and a
/// failed replacement can leave metadata without a usable copy.
pub(crate) fn list_info(dir: &Path, info_dir: &Path, source: &Source) -> (Info, bool) {
    let (address, path) = if source.enabled {
        active_copy(dir, &source.url)
    } else {
        preferred_copy(dir, &source.url, "disabled")
    };
    let has_copy = path
        .metadata()
        .is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_LIST as u64);
    if !has_copy && !source.enabled {
        return (Info::default(), false);
    }
    let previous = address != source.url;
    let mut known = info(info_dir, address).unwrap_or_default();
    if previous {
        known.error = info(info_dir, &source.url).and_then(|current| current.error);
    }
    if has_copy {
        known.refresh_rule_count(&path);
    } else {
        known.rules = None;
        known.updated = None;
    }
    (known, source.enabled && previous && has_copy)
}

pub fn write_info(dir: &Path, info: &Info) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_string(info)?;
    files::replace(&info_file(dir, &info.url), text.as_bytes())?;
    Ok(())
}

/// When a list was last updated, the most recently updated of them.
pub fn lists_updated(paths: &Paths) -> Option<u64> {
    let lists = paths.lists();
    let metadata = paths.list_info();
    read(paths)
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|s| info(&metadata, active_copy(&lists, &s.url).0)?.updated)
        .max()
}

/// The title a list gives itself in its header.
pub fn title(list: &str) -> Option<String> {
    list.lines()
        .take(200)
        .take_while(|l| crate::rule::is_comment(l))
        .find_map(|l| {
            let rest = l.trim().strip_prefix('!')?.trim_start();
            let title = rest.strip_prefix("Title:")?.trim();
            (!title.is_empty()).then(|| title.chars().take(200).collect())
        })
}

/// Reconciles active copies and drops copies of removed subscriptions.
/// Disabled subscriptions may keep a parked copy. The caller holds the
/// sources lock so a concurrent update cannot publish from an old snapshot.
pub fn prune_unsubscribed(
    configured: &str,
    dir: &Path,
    info: Option<&Path>,
    _lock: &files::Lock,
) -> Result<usize> {
    let mut keep = HashSet::new();
    let mut keep_info = HashSet::new();
    for source in parse(configured) {
        let url = source.url.as_str();
        let (address, file) = if source.enabled {
            active_copy(dir, url)
        } else {
            preferred_copy(dir, url, "disabled")
        };
        if let Some(info) = info
            && (source.enabled || file.is_file())
        {
            keep_info.insert(info_file(info, url));
            keep_info.insert(info_file(info, address));
        }
        keep.extend(
            file.file_name()
                .map(|name| name.to_string_lossy().into_owned()),
        );
    }
    let removed = prune(dir, |entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        (name.ends_with(".txt") || name.ends_with(".disabled")) && !keep.contains(name.as_ref())
    })?;
    if let Some(info) = info {
        prune(info, |entry| !keep_info.contains(&entry.path()))?;
    }
    Ok(removed)
}

fn prune(dir: &Path, stale: impl Fn(&std::fs::DirEntry) -> bool) -> std::io::Result<usize> {
    let entries = match std::fs::read_dir(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        result => result?,
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        if stale(&entry) && !entry.file_type()?.is_dir() && files::remove_if_present(&entry.path())?
        {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
pub(crate) mod tests;
