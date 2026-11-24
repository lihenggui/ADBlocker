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

//! The root-side checkpoint while Android prepares a switch from VPN.

use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use abd_engine::malformed::Malformed;
use abd_engine::{allow, files, filter, settings, sources, update};
use anyhow::{Result, bail, ensure};

use crate::paths::Paths;

const CHECKPOINT: &str = "handoff";
const FORMAT: &[u8] = b"abd handoff 3\n";
const TOKEN_LEN: usize = 36;
const MISSING: u32 = u32::MAX;

fn valid_token(token: &[u8]) -> bool {
    token.len() == TOKEN_LEN
        && token.iter().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(b)
            }
        })
}

pub fn run(paths: &Paths, args: &[&str]) -> Result<()> {
    let [action @ ("begin" | "rollback" | "finish"), token] = args else {
        bail!(Malformed(
            "handoff takes begin, rollback or finish and a UUID".into()
        ));
    };
    let token = token.as_bytes();
    if !valid_token(token) {
        bail!(Malformed("handoff takes a canonical lowercase UUID".into()));
    }

    let checkpoint = paths.dir().join(CHECKPOINT);
    // This is the same data lock held by all three settings writers.
    let lock = files::lock(&checkpoint)?;
    let saved = open_checkpoint(&checkpoint)?;
    if *action == "begin" {
        if saved.is_some_and(|(id, _)| id == token) {
            files::remove_if_present(&checkpoint.with_extension("new"))?;
        } else {
            // A fresh transaction can supersede one whose token was lost.
            write_atomic(&checkpoint, |file| save(paths, file, token))?;
        }
        sync_dir(paths.dir())?;
        return Ok(());
    }
    if let Some((id, mut file)) = saved {
        ensure!(id == token, "the root handoff belongs to a different token");
        if *action == "rollback" {
            restore(paths, &mut file, &lock)?;
            // Retain the checkpoint through the reload request so an
            // interrupted rollback can send it again.
            crate::lifecycle::reload(paths);
        }
    }
    files::remove_if_present(&checkpoint.with_extension("new"))?;
    files::remove_if_present(&checkpoint)?;
    sync_dir(paths.dir())?;
    Ok(())
}

fn open_checkpoint(checkpoint: &Path) -> Result<Option<([u8; TOKEN_LEN], File)>> {
    let mut file = match File::open(checkpoint) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        result => result?,
    };
    let mut format = [0; FORMAT.len()];
    let mut token = [0; TOKEN_LEN];
    file.read_exact(&mut format)?;
    file.read_exact(&mut token)?;
    ensure!(
        format == FORMAT && valid_token(&token),
        "invalid root handoff checkpoint"
    );
    Ok(Some((token, file)))
}

fn save(paths: &Paths, file: &mut File, token: &[u8]) -> Result<()> {
    file.write_all(FORMAT)?;
    file.write_all(token)?;
    for (path, limit) in [
        (paths.sources(), sources::MAX_FILE),
        (paths.user_rules(), filter::MAX_LIST),
        (paths.allow(), allow::MAX_FILE),
        (paths.settings(), settings::MAX_SIZE),
        (paths.apps(), abd_engine::apps::MAX_FILE),
    ] {
        match files::read_bytes(&path, limit) {
            Ok(bytes) => {
                file.write_all(&u32::try_from(bytes.len())?.to_le_bytes())?;
                file.write_all(&bytes)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                file.write_all(&MISSING.to_le_bytes())?
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn load_entry(file: &mut File, limit: usize) -> Result<Option<Vec<u8>>> {
    let mut size = [0; 4];
    file.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size);
    if size == MISSING {
        return Ok(None);
    }
    let size = size as usize;
    ensure!(size <= limit, "invalid root handoff checkpoint size");
    let mut bytes = vec![0; size];
    file.read_exact(&mut bytes)?;
    Ok(Some(bytes))
}

fn restore(paths: &Paths, file: &mut File, lock: &files::Lock) -> Result<()> {
    let sources = load_entry(file, sources::MAX_FILE)?;
    let rules = load_entry(file, filter::MAX_LIST)?;
    let allow = load_entry(file, allow::MAX_FILE)?;
    let settings = load_entry(file, settings::MAX_SIZE)?;
    let apps = load_entry(file, abd_engine::apps::MAX_FILE)?;
    ensure!(file.read(&mut [0])? == 0, "invalid root handoff checkpoint");

    for (path, bytes) in [
        (paths.sources(), sources.as_deref()),
        (paths.user_rules(), rules.as_deref()),
        (paths.allow(), allow.as_deref()),
        (paths.apps(), apps.as_deref()),
        (paths.settings(), settings.as_deref()),
    ] {
        if let Some(bytes) = bytes {
            fs::create_dir_all(parent(&path))?;
            write_atomic(&path, |file| Ok(file.write_all(bytes)?))?;
        } else {
            files::remove_if_present(&path)?;
        }
    }
    // Read back as every reader reads it, so a list that moved keeps its
    // new copy.
    let configured = sources::configured(paths)?;
    update::prune_unsubscribed(&configured, &paths.lists(), Some(&paths.list_info()), lock)?;
    for dir in [paths.rules(), paths.lists(), paths.list_info()] {
        sync_dir(&dir)?;
    }
    sync_dir(paths.dir())?;
    Ok(())
}

/// The old file survives until everything in its replacement is on disk.
fn write_atomic(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let temporary = path.with_extension("new");
    files::remove_if_present(&temporary)?;
    let result = files::replace_synced(
        path,
        &temporary,
        File::options().write(true).create_new(true).mode(0o600),
        write,
    );
    if result.is_err() {
        let _ = files::remove_if_present(&temporary);
    }
    result
}

fn parent(path: &Path) -> &Path {
    path.parent().expect("a file under the root data directory")
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    match File::open(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result?.sync_all(),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    const TOKEN: &str = "92883ed4-48f5-434d-bafe-db87c3aa1832";
    const FRESH: &str = "89c94538-ae45-4e2e-8a71-db34d4384fab";
    const ENABLED: &str = "https://old.example/list.txt";
    const DISABLED: &str = "https://disabled.example/list.txt";
    const STAGED: &str = "https://staged.example/list.txt";

    fn handoff(paths: &Paths, action: &str, token: &str) -> Result<()> {
        crate::cli::run(paths, &["handoff", action, token])
    }

    fn cache(paths: &Paths, url: &str, rule: &str) {
        fs::create_dir_all(paths.lists()).unwrap();
        fs::write(paths.lists().join(update::file_name(url)), rule).unwrap();
        sources::write_info(
            &paths.list_info(),
            &sources::Info {
                url: url.to_owned(),
                ..sources::Info::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn rollback_restores_exact_bytes_and_prunes_inactive_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let original_sources = format!("# root preferences\r\n{ENABLED}\r\n#{DISABLED}\r\n");
        let original_allow = b"Bank.Example\r\n";
        fs::create_dir_all(paths.rules()).unwrap();
        fs::write(paths.sources(), &original_sources).unwrap();
        fs::write(paths.user_rules(), b"").unwrap();
        fs::write(paths.allow(), original_allow).unwrap();
        fs::write(paths.settings(), b"url-filtering off\r\n").unwrap();
        cache(&paths, ENABLED, "||kept.example^\n");
        handoff(&paths, "begin", TOKEN).unwrap();
        let checkpoint = dir.path().join(CHECKPOINT);
        assert_eq!(
            fs::metadata(&checkpoint).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::write(paths.sources(), format!("{STAGED}\n{DISABLED}\n")).unwrap();
        fs::write(paths.user_rules(), b"||staged-rules.example^\r\n").unwrap();
        fs::write(paths.allow(), b"staged-trust.example\n").unwrap();
        fs::write(paths.settings(), b"url-filtering on\n").unwrap();
        cache(&paths, STAGED, "||staged-list.example^\n");
        cache(&paths, DISABLED, "||disabled-list.example^\n");
        // Retrying a preparation must keep its very first baseline.
        handoff(&paths, "begin", TOKEN).unwrap();
        handoff(&paths, "rollback", TOKEN).unwrap();
        handoff(&paths, "rollback", TOKEN).unwrap();

        assert_eq!(
            fs::read(paths.sources()).unwrap(),
            original_sources.as_bytes()
        );
        assert!(fs::read(paths.user_rules()).unwrap().is_empty());
        assert_eq!(fs::read(paths.allow()).unwrap(), original_allow);
        assert_eq!(
            fs::read(paths.settings()).unwrap(),
            b"url-filtering off\r\n"
        );
        assert!(sources::info(&paths.list_info(), ENABLED).is_some());
        for url in [STAGED, DISABLED] {
            assert!(!paths.lists().join(update::file_name(url)).exists());
            assert!(sources::info(&paths.list_info(), url).is_none());
        }
        let lists = filter::Filters::load(&[&paths.lists(), &paths.rules()], false).unwrap();
        assert!(lists.blocks_host("kept.example"));
        assert!(!lists.blocks_host("staged-list.example"));
        assert!(!lists.blocks_host("staged-rules.example"));
        assert!(!checkpoint.exists());
    }

    #[test]
    fn rollback_restores_absent_files_and_retains_cached_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let default = sources::read(&paths).remove(0).url;
        cache(&paths, &default, "||default.example^\n");
        handoff(&paths, "begin", TOKEN).unwrap();

        fs::create_dir_all(paths.rules()).unwrap();
        for path in [paths.sources(), paths.user_rules(), paths.allow()] {
            fs::write(path, b"").unwrap();
        }
        cache(&paths, STAGED, "||not-subscribed.example^\n");
        handoff(&paths, "rollback", TOKEN).unwrap();

        for path in [paths.sources(), paths.user_rules(), paths.allow()] {
            assert!(!path.exists());
        }
        assert_eq!(sources::read(&paths)[0].url, default);
        assert!(paths.lists().join(update::file_name(&default)).exists());
        assert!(!paths.lists().join(update::file_name(STAGED)).exists());
    }

    #[test]
    fn rollback_can_retry_after_restoring_only_the_first_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        fs::create_dir_all(paths.rules()).unwrap();
        fs::write(paths.sources(), ENABLED).unwrap();
        fs::write(paths.user_rules(), b"original rules\r\n").unwrap();
        fs::write(paths.allow(), b"original.example\n").unwrap();
        handoff(&paths, "begin", TOKEN).unwrap();
        fs::write(paths.sources(), STAGED).unwrap();
        fs::write(paths.allow(), b"staged.example\n").unwrap();

        fs::remove_file(paths.user_rules()).unwrap();
        fs::remove_dir(paths.rules()).unwrap();
        fs::write(paths.rules(), b"a file obstructs the restore").unwrap();
        assert!(handoff(&paths, "rollback", TOKEN).is_err());
        assert_eq!(fs::read(paths.sources()).unwrap(), ENABLED.as_bytes());
        assert_eq!(fs::read(paths.allow()).unwrap(), b"staged.example\n");
        assert!(dir.path().join(CHECKPOINT).exists());

        fs::remove_file(paths.rules()).unwrap();
        handoff(&paths, "rollback", TOKEN).unwrap();
        assert_eq!(fs::read(paths.user_rules()).unwrap(), b"original rules\r\n");
        assert_eq!(fs::read(paths.allow()).unwrap(), b"original.example\n");
        assert!(!dir.path().join(CHECKPOINT).exists());
    }

    #[test]
    fn a_fresh_token_supersedes_the_old_one_and_finish_leaves_the_current_settings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        handoff(&paths, "rollback", TOKEN).unwrap();
        handoff(&paths, "finish", TOKEN).unwrap();
        handoff(&paths, "begin", TOKEN).unwrap();
        fs::write(paths.allow(), b"fresh-baseline.example\n").unwrap();
        handoff(&paths, "begin", FRESH).unwrap();
        for action in ["rollback", "finish"] {
            let error = handoff(&paths, action, TOKEN).unwrap_err();
            assert!(error.downcast_ref::<Malformed>().is_none());
        }
        fs::write(paths.allow(), b"published.example\n").unwrap();
        handoff(&paths, "rollback", FRESH).unwrap();
        assert_eq!(
            fs::read(paths.allow()).unwrap(),
            b"fresh-baseline.example\n"
        );

        handoff(&paths, "begin", FRESH).unwrap();
        fs::write(paths.allow(), b"published.example\n").unwrap();
        handoff(&paths, "finish", FRESH).unwrap();
        handoff(&paths, "finish", FRESH).unwrap();
        assert_eq!(fs::read(paths.allow()).unwrap(), b"published.example\n");
    }

    #[test]
    fn an_oversized_input_does_not_publish_or_replace_a_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        handoff(&paths, "begin", TOKEN).unwrap();
        fs::create_dir_all(paths.rules()).unwrap();
        File::create(paths.user_rules())
            .unwrap()
            .set_len(filter::MAX_LIST as u64 + 1)
            .unwrap();

        assert!(handoff(&paths, "begin", FRESH).is_err());
        let checkpoint = dir.path().join(CHECKPOINT);
        assert_eq!(
            open_checkpoint(&checkpoint).unwrap().unwrap().0,
            TOKEN.as_bytes()
        );
        assert!(!checkpoint.with_extension("new").exists());
        handoff(&paths, "rollback", TOKEN).unwrap();
        assert!(!paths.user_rules().exists());
    }

    #[test]
    fn a_failed_checkpoint_write_keeps_the_original_and_cleans_its_private_stage() {
        let dir = tempfile::tempdir().unwrap();
        let checkpoint = dir.path().join(CHECKPOINT);
        let temporary = checkpoint.with_extension("new");
        let unrelated = dir.path().join("unrelated");
        fs::write(&checkpoint, b"previous checkpoint").unwrap();
        fs::write(&unrelated, b"unrelated contents").unwrap();
        std::os::unix::fs::symlink(&unrelated, &temporary).unwrap();

        let error = write_atomic(&checkpoint, |file| {
            assert_eq!(file.metadata()?.permissions().mode() & 0o777, 0o600);
            file.write_all(b"partial replacement")?;
            bail!("interrupted checkpoint write");
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "interrupted checkpoint write");
        assert_eq!(fs::read(&checkpoint).unwrap(), b"previous checkpoint");
        assert_eq!(fs::read(&unrelated).unwrap(), b"unrelated contents");
        assert_eq!(
            fs::symlink_metadata(&temporary).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );

        write_atomic(&checkpoint, |file| Ok(file.write_all(b"new checkpoint")?)).unwrap();
        assert_eq!(fs::read(&checkpoint).unwrap(), b"new checkpoint");
        assert_eq!(
            fs::metadata(&checkpoint).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!temporary.exists());
    }

    #[test]
    fn handoff_rejects_an_invalid_command_or_token_before_reading_settings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        for args in [
            &["handoff"][..],
            &["handoff", "begin", "../other"],
            &["handoff", "begin", "92883ED4-48F5-434D-BAFE-DB87C3AA1832"],
            &["handoff", "abort", TOKEN],
        ] {
            assert!(
                crate::cli::run(&paths, args)
                    .unwrap_err()
                    .downcast_ref::<Malformed>()
                    .is_some()
            );
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
