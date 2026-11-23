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

//! Reading and replacing the small files the commands and the daemon share.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

/// Replaces `file` in one step: whoever reads it sees the old text or the
/// new, never half of either. The file in between ends in `.new`, which
/// is never read as a list.
pub fn replace(file: &Path, content: &[u8]) -> std::io::Result<()> {
    let tmp = file.with_extension("new");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, file)
}

/// Replaces a user choice and makes both the contents and the rename durable.
pub fn replace_durable(file: &Path, content: &[u8]) -> std::io::Result<()> {
    let tmp = file.with_extension("new");
    replace_synced(
        file,
        &tmp,
        File::options().write(true).create(true).truncate(true),
        |pending| pending.write_all(content),
    )?;
    let dir = file.parent().filter(|dir| !dir.as_os_str().is_empty());
    File::open(dir.unwrap_or_else(|| Path::new(".")))?.sync_all()
}

/// Write and sync a staged file before publishing it by rename. The caller
/// chooses how to open the stage, how to clean up failures and when to sync
/// the containing directory.
pub fn replace_synced<E: From<std::io::Error>>(
    file: &Path,
    temporary: &Path,
    options: &OpenOptions,
    write: impl FnOnce(&mut File) -> Result<(), E>,
) -> Result<(), E> {
    let mut pending = options.open(temporary)?;
    write(&mut pending)?;
    pending.sync_all()?;
    std::fs::rename(temporary, file)?;
    Ok(())
}

/// Removes a file if it exists and reports whether one was removed.
pub fn remove_if_present(file: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(file) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// How long the lock is waited for. It is held for the reading and the
/// writing of a few lines: longer than this, and whoever holds it is stuck.
const LOCK_TIME: Duration = Duration::from_secs(2);

/// Held while a file beside it is read, changed and replaced: see [`lock`].
#[derive(Debug)]
pub struct Lock(#[allow(dead_code, reason = "held open: closing it is the unlocking")] File);

/// Takes the lock of the directory `beside` is in, for as long as what is
/// returned lives. A file changed by more than one program is read and
/// replaced under it: of two changes made at once, the second would
/// otherwise be made to what the first had read, and undo it.
///
/// The system gives the lock up when its holder ends, however it ends.
pub fn lock(beside: &Path) -> std::io::Result<Lock> {
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(beside.with_file_name("lock"))?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Lock(file)),
            Err(TryLockError::WouldBlock) if started.elapsed() < LOCK_TIME => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(std::io::Error::other(
                    "the data is being changed by another",
                ));
            }
            Err(TryLockError::Error(e)) => return Err(e),
        }
    }
}

/// The text of `file`, if it is there and no longer than `max`. Missing
/// is empty; longer, or not text, is an error.
pub fn read_text(file: &Path, max: usize) -> std::io::Result<String> {
    match read_bytes(file, max) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        bytes => as_text(bytes?),
    }
}

/// Reads at most `max` bytes. Check size first so an oversized file
/// costs nothing to read, and limit the read in case it grows meanwhile.
pub fn read_bytes(file: &Path, max: usize) -> std::io::Result<Vec<u8>> {
    read_file_bytes(File::open(file)?, max)
}

/// Applies the same size bound to an already-open file.
pub fn read_file_bytes(file: File, max: usize) -> std::io::Result<Vec<u8>> {
    if file.metadata()?.len() > max as u64 {
        return Err(too_long(max));
    }
    read_bounded(file, max)
}

/// What `from` holds, as text, if it is no longer than `max`.
pub fn read_limited(from: impl std::io::Read, max: usize) -> std::io::Result<String> {
    as_text(read_bounded(from, max)?)
}

fn read_bounded(from: impl std::io::Read, max: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    from.take(max as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(too_long(max));
    }
    Ok(bytes)
}

fn too_long(max: usize) -> std::io::Error {
    std::io::Error::other(format!("longer than {max} bytes"))
}

fn as_text(bytes: Vec<u8>) -> std::io::Result<String> {
    String::from_utf8(bytes).map_err(|_| std::io::Error::other("not text"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_durable_replacement_leaves_only_the_new_contents_at_the_final_path() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["settings", "apps", "allow", "pauses"] {
            let file = dir.path().join(name);
            replace_durable(&file, b"old choice\n").unwrap();
            replace_durable(&file, b"new choice\n").unwrap();
            assert_eq!(std::fs::read(&file).unwrap(), b"new choice\n");
            assert!(!file.with_extension("new").exists());
        }
    }

    #[test]
    fn exclusive_staging_leaves_an_existing_stage_and_destination_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("choice");
        let stage = file.with_extension("new");
        std::fs::write(&file, b"published").unwrap();
        std::fs::write(&stage, b"existing stage").unwrap();

        let error = replace_synced(
            &file,
            &stage,
            File::options().write(true).create_new(true),
            |pending| pending.write_all(b"competing writer"),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&file).unwrap(), b"published");
        assert_eq!(std::fs::read(&stage).unwrap(), b"existing stage");
    }

    #[test]
    fn the_lock_is_one_holder_s_at_a_time_and_free_when_it_is_done() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("apps"), dir.path().join("pauses"));
        let held = lock(&a).unwrap();
        // The same lock, whichever file of the directory it is taken for;
        // and not waited for without end.
        let started = Instant::now();
        assert!(lock(&b).is_err());
        assert!(started.elapsed() >= LOCK_TIME);
        drop(held);
        lock(&b).unwrap();
    }

    #[test]
    fn a_stream_is_read_only_until_it_exceeds_its_bound() {
        let mut from = std::io::Cursor::new(b"123456789");
        assert!(read_limited(&mut from, 3).is_err());
        assert_eq!(from.position(), 4);
    }

    #[test]
    fn a_file_is_read_whole_only_within_its_bound() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        assert_eq!(read_text(&file, 4).unwrap(), "");
        replace(&file, b"1234").unwrap();
        assert_eq!(read_text(&file, 4).unwrap(), "1234");
        assert!(read_text(&file, 3).is_err());
        replace(&file, &[0xff]).unwrap();
        assert!(read_text(&file, 4).is_err());
        assert!(!dir.path().join("f.new").exists());
    }
}
