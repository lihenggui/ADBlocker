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

//! Inventory and bounded reads for the list files on disk.

use std::io;
use std::path::{Path, PathBuf};

use crate::files;

/// The most a list may hold, and all of them together. The lists of the
/// default sources are 5 MiB together; what is far beyond that is not a
/// list, and would be memory the device does without.
pub const MAX_LIST: usize = 16 << 20;
pub(crate) const MAX_LISTS: usize = 64 << 20;
pub(crate) const MAX_COMPILED: usize = 128 << 20;

#[derive(Debug)]
enum LimitExceeded {
    List,
    Collection,
}

impl std::fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::List => write!(
                f,
                "A filter list exceeds the {} MiB per-list limit",
                MAX_LIST >> 20
            ),
            Self::Collection => write!(
                f,
                "Enabled filter lists and custom rules exceed the {} MiB limit",
                MAX_LISTS >> 20
            ),
        }
    }
}

impl std::error::Error for LimitExceeded {}

pub(crate) fn exceeds_budget(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|cause| cause.is::<LimitExceeded>())
}

pub(crate) fn check_budget(dirs: &[&Path], replacing: &[&Path], bytes: u64) -> io::Result<()> {
    let mut total = bytes;
    for file in list_paths(dirs)? {
        if !replacing.contains(&file.as_path()) {
            total = total.saturating_add(file.metadata()?.len());
        }
    }
    if total > MAX_LISTS as u64 {
        return Err(io::Error::other(LimitExceeded::Collection));
    }
    Ok(())
}

pub(crate) fn check_growth(dirs: &[&Path], replacing: &[&Path], bytes: usize) -> io::Result<()> {
    let mut previous = 0u64;
    for file in replacing {
        match file.metadata() {
            Ok(metadata) => previous = previous.saturating_add(metadata.len()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    // Previously published data may already exceed the budget. Allow writes
    // that reduce it, including when more than one list needs to shrink.
    if bytes as u64 > previous {
        check_budget(dirs, replacing, bytes as u64)?;
    }
    Ok(())
}

/// The `*.txt` files of `dirs` in the order they are loaded, and a number
/// that changes when any of them does: with their names, sizes and times
/// of last change, and with the version of this program, since a new one
/// may read the same lists differently.
pub struct ListFiles {
    pub(crate) paths: Vec<PathBuf>,
    pub generation: u64,
}

impl ListFiles {
    pub fn read(dirs: &[&Path]) -> std::io::Result<Self> {
        use std::hash::{Hash as _, Hasher as _};
        use std::os::unix::fs::MetadataExt as _;
        let mut paths = list_paths(dirs)?;
        paths.sort();
        let mut hasher = std::hash::DefaultHasher::new();
        env!("CARGO_PKG_VERSION").hash(&mut hasher);
        6u32.hash(&mut hasher); // Compiled rule policy, input budget and cache format.
        crate::catalog::cache_identity().hash(&mut hasher);
        for file in &paths {
            file.hash(&mut hasher);
            let m = file.metadata()?;
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
                .hash(&mut hasher);
        }
        Ok(Self {
            paths,
            generation: hasher.finish(),
        })
    }
}

fn list_paths(dirs: &[&Path]) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for dir in dirs {
        let entries = match std::fs::read_dir(dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            entries => entries?,
        };
        for entry in entries {
            let file = entry?.path();
            if file.extension().is_some_and(|e| e == "txt") {
                paths.push(file);
            }
        }
    }
    Ok(paths)
}

pub(crate) fn readable_paths(dirs: &[&Path]) -> Vec<PathBuf> {
    let mut files: Vec<_> = dirs
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|path| path.extension().is_some_and(|e| e == "txt"))
        .collect();
    files.sort();
    files
}

/// Reads lists in order. If the collection does not fit, the caller cannot
/// publish any part of this generation.
pub(crate) fn read_lists(
    paths: &[PathBuf],
) -> impl Iterator<Item = std::io::Result<(&Path, String)>> {
    let mut remaining = MAX_LISTS;
    paths.iter().filter_map(move |path| {
        let metadata = match path.metadata() {
            Ok(metadata) => metadata,
            Err(error) => return Some(Err(error)),
        };
        if !metadata.is_file() {
            return None;
        }
        if metadata.len() > MAX_LIST as u64 {
            return Some(Err(io::Error::other(LimitExceeded::List)));
        }
        if metadata.len() > remaining as u64 {
            return Some(Err(io::Error::other(LimitExceeded::Collection)));
        }
        let bytes = match files::read_bytes(path, remaining.min(MAX_LIST)) {
            Ok(bytes) => bytes,
            Err(error) => return Some(Err(error)),
        };
        let text = String::from_utf8(bytes).ok()?;
        remaining -= text.len();
        Some(Ok((path.as_path(), text)))
    })
}

pub(crate) fn read_list(path: &Path, limit: usize) -> std::io::Result<Option<String>> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Ok(None);
    }
    Ok(String::from_utf8(files::read_bytes(path, limit)?).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_collection_that_exceeds_the_budget_cannot_be_published() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..MAX_LISTS / MAX_LIST {
            std::fs::File::create(dir.path().join(format!("{n}.txt")))
                .unwrap()
                .set_len(MAX_LIST as u64)
                .unwrap();
        }
        assert!(check_budget(&[dir.path()], &[], 0).is_ok());
        assert!(exceeds_budget(
            &check_budget(&[dir.path()], &[], 1).unwrap_err()
        ));
        std::fs::write(dir.path().join("over-limit.txt"), "||ads.example^\n").unwrap();
        let files = ListFiles::read(&[dir.path()]).unwrap().paths;
        assert!(exceeds_budget(
            &read_lists(&files)
                .collect::<io::Result<Vec<_>>>()
                .unwrap_err()
        ));
        assert!(crate::filter::Filters::load(&[dir.path()], false).is_err());
    }
}
