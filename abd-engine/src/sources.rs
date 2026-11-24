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
use crate::list_url::{base_file_name, file_name};
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

/// Catalog lists whose address changed after a release saved the old one.
/// Every reader of the file comes through `text_of`, so a saved choice
/// follows the list, and the next update prunes the copy of the old address
/// with its information like that of any list no longer subscribed.
const MOVED: [(&str, &str); 1] = [(
    // AdGuard leaves out of its uBlock build the rules that name apps.
    "https://filters.adtidy.org/extension/ublock/filters/11.txt",
    "https://filters.adtidy.org/android/filters/11.txt",
)];

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
            Ok(restore_copy(&paths.lists(), url)?)
        }
        Change::Disable | Change::Remove => {
            withdraw(paths, url, &new, matches!(change, Change::Disable))?;
            Ok(true)
        }
    }
}

fn withdraw(paths: &Paths, url: &str, selection: &str, keep_cached: bool) -> Result<()> {
    let dir = paths.lists();
    let occupied = if keep_cached {
        disabled_bytes(&dir)?
    } else {
        0
    };
    let mut parked = None;
    // Withdraw the active copy before recording the choice, but keep enough
    // information to restore the file if saving the choice fails.
    let result = (|| -> std::io::Result<()> {
        let active = copy_path(&dir, url);
        let disabled = active.with_extension("disabled");
        match std::fs::rename(&active, &disabled) {
            Ok(()) => parked = Some((active, disabled)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        files::replace(&paths.sources(), selection.as_bytes())
    })();
    if let Err(error) = result {
        let mut recovery = Ok(());
        if let Some((active, disabled)) = parked
            && let Err(restore) = std::fs::rename(disabled, active)
        {
            recovery = Err(restore);
        }
        recovery.context("restoring lists after a failed subscription change")?;
        return Err(error.into());
    }
    if keep_cached {
        if let Some((_, disabled)) = parked {
            let bytes = disabled.metadata()?.len();
            if bytes > MAX_DISABLED_BYTES.saturating_sub(occupied) {
                files::remove_if_present(&disabled)?;
            }
        }
    } else {
        files::remove_if_present(&copy_path(&dir, url).with_extension("disabled"))?;
    }
    if !has_disabled_copy(&dir, url) {
        files::remove_if_present(&info_file(&paths.list_info(), url))?;
    }
    Ok(())
}

fn copy_path(dir: &Path, url: &str) -> PathBuf {
    dir.join(file_name(url))
}

pub(crate) fn has_disabled_copy(dir: &Path, url: &str) -> bool {
    copy_path(dir, url).with_extension("disabled").is_file()
}

fn check_restore_budget(paths: &Paths, url: &str) -> std::io::Result<()> {
    let dir = paths.lists();
    let active = copy_path(&dir, url);
    let bytes = if active.is_file() {
        0
    } else {
        active
            .with_extension("disabled")
            .metadata()
            .map_or(0, |metadata| metadata.len())
    };
    if bytes != 0 {
        check_budget(&[&dir, &paths.rules()], &[], bytes)?;
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

fn restore_copy(dir: &Path, url: &str) -> std::io::Result<bool> {
    let active = copy_path(dir, url);
    let disabled = active.with_extension("disabled");
    if !disabled.try_exists()? {
        return Ok(false);
    }
    if active.try_exists()? {
        files::remove_if_present(&disabled)?;
        return Ok(false);
    }
    match std::fs::rename(disabled, active) {
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
    /// Its lines that are not comments.
    pub rules: Option<usize>,
    /// When it was last fetched and found to be a list.
    pub updated: Option<u64>,
    /// Why it was not, the last time; none once it is.
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validator: Option<Validator>,
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
pub(crate) fn list_info(dir: &Path, info_dir: &Path, source: &Source) -> Info {
    let path = copy_path(dir, &source.url);
    let path = if source.enabled {
        path
    } else {
        path.with_extension("disabled")
    };
    let copy = Some(path).filter(|path| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_LIST as u64)
    });
    if copy.is_none() && !source.enabled {
        return Info::default();
    }
    let mut known = info(info_dir, &source.url).unwrap_or_default();
    if let Some(path) = copy {
        if known.rules.is_none()
            && let Ok(Some(text)) = read_list(&path, MAX_LIST)
        {
            known.rules = Some(crate::rule_text::count(&text));
            known.title = known.title.or_else(|| title(&text));
        }
    } else {
        known.rules = None;
        known.updated = None;
    }
    known
}

pub fn write_info(dir: &Path, info: &Info) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_string(info)?;
    files::replace(&info_file(dir, &info.url), text.as_bytes())?;
    Ok(())
}

/// When a list was last updated, the most recently updated of them.
pub fn lists_updated(paths: &Paths) -> Option<u64> {
    read(paths)
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|s| info(&paths.list_info(), &s.url)?.updated)
        .max()
}

/// The title a list gives itself in its header.
pub fn title(list: &str) -> Option<String> {
    list.lines()
        .take(200)
        .take_while(|l| crate::rule_text::is_comment(l))
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
        if let Some(info) = info
            && (source.enabled || has_disabled_copy(dir, url))
        {
            keep_info.insert(info_file(info, url));
        }
        if !source.enabled {
            let file = copy_path(dir, url).with_extension("disabled");
            keep.extend(
                file.file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
            );
            continue;
        }
        keep.insert(file_name(url));
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
pub(crate) mod tests {
    use super::*;
    use crate::list_url::MAX_URL;

    /// The lines that turn off each default but those of `named`, which
    /// the file has lines of its own for.
    pub(crate) fn defaults_off(named: &[&str]) -> String {
        subscribed(crate::catalog::default_sources())
            .filter(|url| !named.contains(url))
            .map(|url| format!("#{url}\n"))
            .collect()
    }

    const A: &str = "https://a.example/list.txt";
    const B: &str = "https://b.example/other.txt";

    #[test]
    fn a_default_that_a_saved_selection_does_not_name_is_on_until_it_is_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let defaults: Vec<_> = subscribed(crate::catalog::default_sources()).collect();
        let (first, rest) = defaults.split_first().unwrap();
        std::fs::write(paths.sources(), format!("{A}\n#{first}")).unwrap();
        let on = |url: &str| read(&paths).iter().any(|s| s.url == url && s.enabled);
        assert!(on(A) && !on(first));
        assert!(rest.iter().all(|url| on(url)));
        assert_eq!(read(&paths).len(), defaults.len() + 1);

        // Removed, it is not a default the file does not name.
        let removed = rest[0];
        edit(&paths, removed, Change::Remove).unwrap();
        assert!(!on(removed));
        assert!(
            std::fs::read_to_string(paths.sources())
                .unwrap()
                .contains(&format!("#{removed}\n"))
        );
        edit(&paths, A, Change::Remove).unwrap();
        assert!(read(&paths).iter().all(|s| s.url != A));
    }

    #[test]
    fn a_confirmed_selection_disables_all_its_lists_without_resetting_other_subscriptions() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let off = defaults_off(&[]);
        std::fs::write(paths.sources(), format!("# Lists\n{A}\n{B}\n{off}")).unwrap();
        configure(&paths, &format!("#{A}")).unwrap();
        assert_eq!(
            std::fs::read_to_string(paths.sources()).unwrap(),
            format!("# Lists\n#{A}\n{B}\n{off}")
        );
        let saved = std::fs::read_to_string(paths.sources()).unwrap();
        assert!(configure(&paths, &format!("#{B}\ninvalid")).is_err());
        assert_eq!(std::fs::read_to_string(paths.sources()).unwrap(), saved);
        configure(&paths, &format!("#{B}")).unwrap();
        assert!(read(&paths).iter().all(|source| !source.enabled));
    }

    #[test]
    fn a_sources_file_that_cannot_be_looked_at_is_not_replaced_by_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        // A link to itself: whether a file is there cannot be told.
        std::os::unix::fs::symlink(paths.sources(), paths.sources()).unwrap();
        assert!(configure(&paths, &format!("#{A}")).is_err());
        assert!(edit(&paths, A, Change::Add).is_err());
        let still = std::fs::symlink_metadata(paths.sources()).unwrap();
        assert!(still.file_type().is_symlink());
    }

    #[test]
    fn a_source_is_disabled_by_a_hash_against_its_address_only() {
        let sources = parse(
            "# The filter lists, one address a line.\n#https://a.example/x\nhttps://b.example/y\n#not an address\n\n",
        );
        assert_eq!(
            sources,
            [
                Source {
                    url: "https://a.example/x".to_owned(),
                    enabled: false
                },
                Source {
                    url: "https://b.example/y".to_owned(),
                    enabled: true
                },
            ]
        );
    }

    #[test]
    fn failing_to_save_a_disable_keeps_the_active_copy() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(paths.lists()).unwrap();
        std::fs::write(paths.sources(), format!("{A}\n")).unwrap();
        std::fs::create_dir(paths.sources().with_extension("new")).unwrap();
        let list = paths.lists().join(file_name(A));
        std::fs::write(&list, "||ads.example^\n").unwrap();

        assert!(edit(&paths, A, Change::Disable).is_err());
        assert!(read(&paths)[0].enabled);
        assert_eq!(std::fs::read_to_string(list).unwrap(), "||ads.example^\n");
        assert!(!has_disabled_copy(&paths.lists(), A));
    }

    #[test]
    fn list_status_prefers_the_local_copy_when_download_metadata_is_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(paths.lists()).unwrap();
        std::fs::write(paths.sources(), format!("{A}\n{B}\n")).unwrap();
        std::fs::write(
            paths.lists().join(file_name(A)),
            "! Title: First\n||ads.example^\n",
        )
        .unwrap();
        write_info(
            &paths.list_info(),
            &Info {
                url: B.to_owned(),
                rules: Some(2),
                updated: Some(5),
                error: Some("timed out".to_owned()),
                ..Info::default()
            },
        )
        .unwrap();

        let status = crate::protocol::lists(&paths);
        assert_eq!(status[0].rules, Some(1));
        assert_eq!(status[0].title.as_deref(), Some("First"));
        assert_eq!(status[1].rules, None);
        assert_eq!(status[1].updated, None);
        assert_eq!(status[1].error.as_deref(), Some("timed out"));
    }

    #[test]
    fn a_disabled_source_stops_filtering_and_can_be_enabled_without_a_download() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let off = defaults_off(&[]);
        std::fs::create_dir_all(paths.lists()).unwrap();
        std::fs::write(paths.sources(), format!("# Lists\n{A}\n{B}\n{off}")).unwrap();
        let list = paths.lists().join(file_name(A));
        std::fs::write(&list, "||ads.example^\n").unwrap();
        write_info(
            &paths.list_info(),
            &Info {
                url: A.to_owned(),
                updated: Some(5),
                ..Info::default()
            },
        )
        .unwrap();

        assert!(edit(&paths, A, Change::Disable).unwrap());
        assert_eq!(
            std::fs::read_to_string(paths.sources()).unwrap(),
            format!("# Lists\n#{A}\n{B}\n{off}")
        );
        assert!(!list.exists() && info(&paths.list_info(), A).is_some());
        assert!(
            !crate::filter::Filters::load(&[&paths.lists()], false)
                .unwrap()
                .blocks_host("ads.example")
        );
        let reported = crate::protocol::lists(&paths);
        assert!(!reported[0].enabled);
        assert_eq!(reported[0].updated, Some(5));
        assert_eq!(
            read(&paths)[0],
            Source {
                url: A.to_owned(),
                enabled: false
            }
        );

        assert!(edit(&paths, A, Change::Enable).unwrap());
        assert!(
            crate::filter::Filters::load(&[&paths.lists()], false)
                .unwrap()
                .blocks_host("ads.example")
        );
        assert_eq!(
            std::fs::read_to_string(paths.sources()).unwrap(),
            format!("# Lists\n{A}\n{B}\n{off}")
        );
        // Twice is once.
        edit(&paths, A, Change::Enable).unwrap();
        edit(&paths, A, Change::Add).unwrap();
        assert_eq!(read(&paths).iter().filter(|s| s.enabled).count(), 2);

        edit(&paths, A, Change::Disable).unwrap();
        edit(&paths, A, Change::Remove).unwrap();
        assert!(!has_disabled_copy(&paths.lists(), A));
        assert!(info(&paths.list_info(), A).is_none());
    }

    #[test]
    fn sources_are_added_and_removed_and_only_https_addresses_are_taken() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        // No file yet: the defaults, and the new one after them.
        edit(&paths, A, Change::Add).unwrap();
        let sources = read(&paths);
        assert!(sources.len() > 1 && sources.last().unwrap().url == A);
        edit(&paths, A, Change::Disable).unwrap();
        edit(&paths, A, Change::Remove).unwrap();
        assert!(read(&paths).iter().all(|s| s.url != A));
        assert!(edit(&paths, A, Change::Enable).is_err());
        let long = format!("https://a.example/{}", "x".repeat(MAX_URL));
        for bad in [
            "http://a.example/l.txt",
            "https://",
            "https://a.example/a b",
            "https://a.example/\nx",
            &long,
        ] {
            assert!(edit(&paths, bad, Change::Add).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_saved_choice_of_a_moved_list_is_read_as_its_new_address() {
        let (old, new) = MOVED[0];
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let off = defaults_off(&[MOVED[0].1]);
        std::fs::write(paths.sources(), format!("# Lists\n{A}\n#{old}\n{off}")).unwrap();
        assert_eq!(
            read(&paths)[..2],
            [
                Source {
                    url: A.to_owned(),
                    enabled: true
                },
                Source {
                    url: new.to_owned(),
                    enabled: false
                },
            ]
        );
        edit(&paths, new, Change::Enable).unwrap();
        assert_eq!(
            std::fs::read_to_string(paths.sources()).unwrap(),
            format!("# Lists\n{A}\n{new}\n{off}")
        );

        std::fs::write(paths.sources(), format!("{old}\n#{new}\n{off}")).unwrap();
        assert_eq!(
            read(&paths)[..1],
            [Source {
                url: new.to_owned(),
                enabled: false
            }]
        );
    }

    #[test]
    fn the_copy_of_a_moved_list_is_pruned_with_what_is_known_of_it() {
        let (old, new) = MOVED[0];
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(paths.lists()).unwrap();
        std::fs::write(paths.sources(), format!("{old}\n")).unwrap();
        let copy = paths.lists().join(file_name(old));
        std::fs::write(&copy, "||ads.example^\n").unwrap();
        write_info(
            &paths.list_info(),
            &Info {
                url: old.to_owned(),
                ..Info::default()
            },
        )
        .unwrap();

        let lock = files::lock(&paths.sources()).unwrap();
        let selection = configured(&paths).unwrap();
        prune_unsubscribed(&selection, &paths.lists(), Some(&paths.list_info()), &lock).unwrap();
        assert!(!copy.exists());
        assert!(!info_file(&paths.list_info(), old).exists());
        assert_eq!(read(&paths)[0].url, new);
    }

    #[test]
    fn a_list_names_itself_in_its_header() {
        assert_eq!(
            title("[Adblock Plus 2.0]\n! Title: EasyList\n||a^\n").as_deref(),
            Some("EasyList")
        );
        assert_eq!(title("||a^\n! Title: Not the header\n"), None);
        assert_eq!(title("! Title:\n"), None);
    }
}
