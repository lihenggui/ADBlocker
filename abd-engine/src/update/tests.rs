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

use anyhow::anyhow;
use hyper::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};

use super::http::{conditional, validator};
use super::kind::looks_like_a_list;
use super::*;

mod includes;
mod server;

const FIRST: &str = "https://first.example/list.txt";
const SECOND: &str = "https://second.example/list.txt";

const MOVED_FROM: &str = crate::list_url::MOVED[0].0;
const MOVED_TO: &str = crate::list_url::MOVED[0].1;

fn moved_list(previous: &str, current: &str) -> (tempfile::TempDir, Paths, Info) {
    let (dir, paths) = data();
    let off = sources::tests::defaults_off(&[current]);
    std::fs::write(paths.sources(), format!("{previous}\n{off}")).unwrap();
    std::fs::create_dir_all(paths.lists()).unwrap();
    let copy = paths
        .lists()
        .join(crate::list_url::copy_names(previous).next().unwrap());
    std::fs::write(&copy, "! Title: Previous copy\n||old.example^\n").unwrap();
    let known = Info {
        url: previous.to_owned(),
        title: Some("Previous copy".to_owned()),
        rules: Some(1),
        updated: Some(123),
        validator: Some(Validator {
            resource: previous.to_owned(),
            tag: Some("\"old\"".to_owned()),
            modified: None,
            file: file_stamp(&copy).unwrap(),
        }),
        ..Info::default()
    };
    sources::write_info(&paths.list_info(), &known).unwrap();
    (dir, paths, known)
}

async fn replace_moved_list(previous: &str, current: &str, only: Option<&str>) {
    let (_dir, paths, known) = moved_list(previous, current);
    let sub = Subscription {
        sources: &paths.sources(),
        default: "",
        kind: ListKind::Filter,
        dir: &paths.lists(),
        info: Some(&paths.list_info()),
        only,
    };
    let old = paths
        .lists()
        .join(crate::list_url::copy_names(previous).next().unwrap());
    let new = paths.lists().join(file_name(current));
    let unrelated = paths.lists().join(file_name(SECOND));
    if only.is_some() {
        std::fs::write(&unrelated, "||unrelated.example^\n").unwrap();
    }
    let load = || crate::filter::Filters::load_cached(&[&paths.lists()], &paths.cache()).unwrap();
    let before = load().generation;
    for (result, error) in [
        (Err(anyhow!("connection refused")), "connection refused"),
        (Ok(Download::Unchanged), "the answer is not a list"),
        (
            Ok(Download::Body(http::Body::new(
                Bytes::from_static(b"<html>Error</html>"),
                None,
            ))),
            "the answer is not a list",
        ),
    ] {
        let result = std::cell::RefCell::new(Some(result));
        let fetch = |url, validator: Option<Validator>, _| {
            assert_eq!(url, current);
            assert!(validator.is_none());
            std::future::ready(result.borrow_mut().take().unwrap())
        };
        let tally = fetch_validated(sub, &fetch, &mut |_| {}).await.unwrap();
        assert_eq!((tally.failed, tally.changed()), (1, 0));
        assert!(old.exists() && !new.exists());
        assert_eq!(
            sources::info(&paths.list_info(), previous).as_ref(),
            Some(&known)
        );
        let filters = load();
        assert_eq!(filters.generation, before);
        assert!(filters.blocks_host("old.example"));
        let lists = crate::protocol::lists(&paths);
        let list = &lists[0];
        assert_eq!(list.url, current);
        assert_eq!(list.error.as_deref(), Some(error));
        assert_eq!((list.rules, list.updated), (Some(1), Some(123)));
        assert!(list.using_previous_address);
    }

    let fetch = |url, validator: Option<Validator>, _| {
        assert_eq!(url, current);
        assert!(validator.is_none());
        std::future::ready(Ok(Download::Body(http::Body::new(
            Bytes::from_static(b"||new.example^\n"),
            None,
        ))))
    };
    let tally = fetch_validated(sub, &fetch, &mut |_| {}).await.unwrap();
    assert_eq!((tally.failed, tally.changed()), (0, 1));
    assert!(!old.exists() && new.is_file());
    assert!(!sources::info_file(&paths.list_info(), previous).exists());
    assert_eq!(unrelated.exists(), only.is_some());
    let filters = load();
    assert_ne!(filters.generation, before);
    assert!(!filters.blocks_host("old.example"));
    assert!(filters.blocks_host("new.example"));
    let lists = crate::protocol::lists(&paths);
    assert_eq!(lists[0].rules, Some(1));
    assert!(lists[0].error.is_none() && !lists[0].using_previous_address);
}

#[tokio::test]
async fn a_successful_update_replaces_the_parked_previous_copy_of_a_disabled_list() {
    let &(previous, current) = crate::list_url::MOVED.first().unwrap();
    let (_dir, paths, _) = moved_list(previous, current);
    let old = paths
        .lists()
        .join(crate::list_url::copy_names(previous).next().unwrap());
    let new = paths.lists().join(file_name(current));
    sources::edit(&paths, current, sources::Change::Disable).unwrap();

    let sub = Subscription {
        sources: &paths.sources(),
        default: "",
        kind: ListKind::Filter,
        dir: &paths.lists(),
        info: Some(&paths.list_info()),
        only: Some(current),
    };
    let fetch = |url, validator: Option<Validator>, _| {
        assert_eq!(url, current);
        assert!(validator.is_none());
        std::future::ready(Ok(Download::Body(http::Body::new(
            Bytes::from_static(b"||new.example^\n"),
            None,
        ))))
    };
    let tally = fetch_validated(sub, &fetch, &mut |_| {}).await.unwrap();

    assert_eq!((tally.updated, tally.failed), (1, 0));
    assert!(!old.exists() && !old.with_extension("disabled").exists());
    assert!(!sources::info_file(&paths.list_info(), previous).exists());
    assert!(!new.exists() && new.with_extension("disabled").is_file());
    sources::edit(&paths, current, sources::Change::Enable).unwrap();
    let filters = crate::filter::Filters::load(&[&paths.lists()], false).unwrap();
    assert!(filters.blocks_host("new.example"));
    assert!(!filters.blocks_host("old.example"));
}

#[tokio::test]
async fn a_full_update_keeps_the_moved_list_until_its_replacement_is_validated() {
    for &(previous, current) in crate::list_url::MOVED {
        replace_moved_list(previous, current, None).await;
    }
}

#[tokio::test]
async fn a_targeted_update_keeps_the_moved_list_until_its_replacement_is_validated() {
    for &(previous, current) in crate::list_url::MOVED {
        replace_moved_list(previous, current, Some(current)).await;
    }
}

#[tokio::test]
async fn the_input_budget_counts_a_moved_list_replacement_in_place_of_its_previous_copy() {
    let (_dir, paths, _) = moved_list(MOVED_FROM, MOVED_TO);
    let old = paths.lists().join(file_name(MOVED_FROM));
    let old_len = old.metadata().unwrap().len();
    std::fs::create_dir(paths.rules()).unwrap();
    for n in 0..crate::list_files::MAX_LISTS / MAX_SIZE {
        let len = if n == 0 {
            MAX_SIZE as u64 - old_len - 1
        } else {
            MAX_SIZE as u64
        };
        std::fs::File::create(paths.rules().join(format!("{n}.txt")))
            .unwrap()
            .set_len(len)
            .unwrap();
    }
    for (growth, expected_failed) in [(2, 1), (1, 0)] {
        let fetch = |_| {
            let mut body = b"||new.example^\n".to_vec();
            body.resize(old_len as usize + growth, b'\n');
            std::future::ready(Ok(Bytes::from(body)))
        };
        let tally = fetch_all(
            &paths.sources(),
            "",
            &paths.lists(),
            Some(&paths.list_info()),
            &fetch,
            &mut |_| {},
        )
        .await
        .unwrap();
        assert_eq!(tally.failed, expected_failed);
        assert_eq!(old.exists(), expected_failed > 0);
    }
}

#[tokio::test]
async fn an_update_prunes_the_previous_copy_of_a_disabled_moved_list() {
    let (_dir, paths, _) = moved_list(MOVED_FROM, MOVED_TO);
    std::fs::write(paths.sources(), format!("#{MOVED_FROM}\n")).unwrap();
    let tally = fetch_all(
        &paths.sources(),
        "",
        &paths.lists(),
        Some(&paths.list_info()),
        &|_| -> std::future::Ready<Result<Bytes>> { panic!("a disabled list is not fetched") },
        &mut |_| {},
    )
    .await
    .unwrap();
    assert_eq!(tally.changed(), 1);
    assert!(!paths.lists().join(file_name(MOVED_FROM)).exists());
    assert!(!sources::info_file(&paths.list_info(), MOVED_FROM).exists());
}

#[tokio::test]
async fn updating_adds_a_new_bundled_exclusion_source_to_an_existing_selection() {
    let (_dir, paths) = data();
    let sources = paths.exclusion_sources();
    let directory = paths.exclusions();
    let bundled = crate::catalog::exclusion_sources();
    let (added, previous) = bundled.split_once('\n').unwrap();
    let previous = format!("{previous}{FIRST}");
    std::fs::write(&sources, &previous).unwrap();
    std::fs::create_dir(&directory).unwrap();
    let first_file = directory.join(file_name(FIRST));
    std::fs::write(&first_file, "previous.example\n").unwrap();
    let reply = |_, _, _| {
        std::future::ready(Ok(Download::Body(http::Body::new(
            Bytes::from_static(b"updated.example\n"),
            None,
        ))))
    };
    for _ in 0..2 {
        let tally = fetch_validated(
            Subscription {
                sources: &sources,
                default: bundled,
                kind: ListKind::HttpsExclusion,
                dir: &directory,
                info: None,
                only: None,
            },
            &reply,
            &mut |_| {},
        )
        .await
        .unwrap();
        assert_eq!(tally.updated + tally.unchanged, bundled.lines().count() + 1);
        assert_eq!(
            std::fs::read_to_string(&sources).unwrap(),
            format!("{previous}\n{added}\n")
        );
    }
    let exclusions = crate::exclusions::Exclusions::load(&directory);
    assert!(exclusions.ready());
    assert!(exclusions.covers("com.example.app", "previous.example"));
    assert!(exclusions.covers("com.example.app", "updated.example"));
}

#[tokio::test]
async fn a_single_word_response_cannot_replace_a_bundled_exclusion_feed() {
    let (_dir, paths) = data();
    let bundled = crate::catalog::exclusion_sources();
    let url = bundled.lines().next().unwrap();
    let source = paths.exclusion_sources();
    let directory = paths.exclusions();
    std::fs::write(&source, format!("{url}\n{FIRST}\n")).unwrap();
    std::fs::create_dir(&directory).unwrap();
    let file = directory.join(file_name(url));
    std::fs::write(&file, "old.example\n").unwrap();
    let reply = |url: String, _, _| {
        let body = if url == FIRST {
            "intranet\n"
        } else {
            "Unavailable\n"
        };
        std::future::ready(Ok(Download::Body(http::Body::new(
            Bytes::from_static(body.as_bytes()),
            None,
        ))))
    };
    let tally = fetch_validated(
        Subscription {
            sources: &source,
            default: bundled,
            kind: ListKind::HttpsExclusion,
            dir: &directory,
            info: None,
            only: None,
        },
        &reply,
        &mut |_| {},
    )
    .await
    .unwrap();
    assert_eq!(tally.failed, bundled.lines().count());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old.example\n");
    assert!(crate::exclusions::Exclusions::load(&directory).covers("com.app", "intranet"));
}

#[tokio::test]
async fn exclusion_updates_keep_prior_protections_and_reject_non_exclusion_data() {
    let (_dir, paths) = data();
    let source = paths.exclusion_sources();
    std::fs::write(&source, FIRST).unwrap();
    let directory = paths.exclusions();
    for (body, failed) in [
        ("old.example\n", 0),
        ("new.example\n", 0),
        ("[Adblock Plus 2.0]\n||example.org^\n", 1),
    ] {
        let reply = |_, _, _| {
            std::future::ready(Ok(Download::Body(http::Body::new(
                Bytes::from_static(body.as_bytes()),
                None,
            ))))
        };
        let tally = fetch_validated(
            Subscription {
                sources: &source,
                default: "",
                kind: ListKind::HttpsExclusion,
                dir: &directory,
                info: None,
                only: None,
            },
            &reply,
            &mut |_| {},
        )
        .await
        .unwrap();
        assert_eq!(tally.failed, failed);
    }
    let exclusions = crate::exclusions::Exclusions::load(&directory);
    assert!(exclusions.covers("com.example.app", "old.example"));
    assert!(exclusions.covers("com.example.app", "new.example"));
}

fn data() -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::write(paths.sources(), format!("{FIRST}\n")).unwrap();
    (dir, paths)
}

async fn fetch_lists<G, F>(paths: &Paths, get: &G) -> Tally
where
    G: Fn(String) -> F,
    F: Future<Output = Result<Bytes>>,
{
    fetch_all(
        &paths.sources(),
        "",
        &paths.lists(),
        Some(&paths.list_info()),
        get,
        &mut |_| {},
    )
    .await
    .unwrap()
}

#[test]
fn only_the_resource_that_supplied_a_validator_receives_it() {
    let mut headers = hyper::HeaderMap::new();
    headers.insert(ETAG, "W/\"one\"".parse().unwrap());
    headers.insert(
        LAST_MODIFIED,
        "Sun, 06 Nov 1994 08:49:37 GMT".parse().unwrap(),
    );
    let saved = validator(SECOND, &headers).unwrap();
    assert_eq!(conditional(SECOND, Some(&saved)).unwrap().0, IF_NONE_MATCH);
    assert!(conditional(FIRST, Some(&saved)).is_none());
    headers.insert(ETAG, "*".parse().unwrap());
    let saved = validator(SECOND, &headers).unwrap();
    assert_eq!(
        conditional(SECOND, Some(&saved)).unwrap().0,
        IF_MODIFIED_SINCE
    );
}

#[tokio::test]
async fn a_conditional_download_reuses_only_the_copy_that_was_validated() {
    use std::cell::Cell;
    let (_dir, paths) = data();
    let file = paths.lists().join(file_name(FIRST));
    let calls = Cell::new(0);
    let download = |_, saved: Option<Validator>, _| {
        let n = calls.get();
        calls.set(n + 1);
        let reply = match n {
            0 | 3 => {
                assert!(saved.is_none());
                let mut headers = hyper::HeaderMap::new();
                headers.insert(ETAG, "\"one\"".parse().unwrap());
                Download::Body(http::Body::new(
                    Bytes::from_static(b"! Title: List\n||ads.example^"),
                    validator(FIRST, &headers),
                ))
            }
            1 | 2 | 4 => {
                assert!(saved.is_some());
                if n == 2 {
                    std::fs::remove_file(&file).unwrap();
                }
                if n == 4 {
                    sources::edit(&paths, FIRST, sources::Change::Remove).unwrap();
                }
                Download::Unchanged
            }
            _ => panic!("unexpected download"),
        };
        std::future::ready(Ok(reply))
    };
    for (updated, unchanged) in [(1, 0), (0, 1), (1, 0), (0, 1)] {
        let tally = fetch_validated(
            Subscription {
                sources: &paths.sources(),
                default: "",
                kind: ListKind::Filter,
                dir: &paths.lists(),
                info: Some(&paths.list_info()),
                only: None,
            },
            &download,
            &mut |_| {},
        )
        .await
        .unwrap();
        assert_eq!(
            (tally.updated, tally.unchanged, tally.failed),
            (updated, unchanged, 0)
        );
        if calls.get() == 2 {
            let info = sources::info(&paths.list_info(), FIRST).unwrap();
            assert_eq!((info.title.as_deref(), info.rules), (Some("List"), Some(1)));
            assert_eq!(file_stamp(&file), info.validator.map(|v| v.file));
        }
    }
    assert_eq!(calls.get(), 5);
    assert!(!file.exists());
    assert!(sources::info(&paths.list_info(), FIRST).is_none());
}

#[tokio::test]
async fn https_revalidation_reads_no_body_and_rejects_an_unsolicited_not_modified() {
    use http_body_util::Full;
    use hyper::{Response, StatusCode};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let calls = AtomicUsize::new(0);
    let server = server::serve(move |req| {
        let n = calls.fetch_add(1, Ordering::Relaxed);
        assert_eq!(
            req.headers().get(IF_NONE_MATCH).map(|v| v.as_bytes()),
            (n == 1).then_some(b"\"one\"".as_slice())
        );
        if n == 0 {
            Response::builder()
                .header(ETAG, "\"one\"")
                .body(Full::new(Bytes::from_static(b"||ads.example^")))
        } else {
            Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .body(Full::new(Bytes::new()))
        }
        .unwrap()
    })
    .await;
    let url = format!("{}/list.txt", server.origin);
    let connector = &server.connector;
    let Download::Body(body) = get(&url, connector, None, Redirects::List).await.unwrap() else {
        panic!()
    };
    assert_eq!(body.bytes.as_ref(), b"||ads.example^");
    assert!(matches!(
        get(&url, connector, body.validator.as_ref(), Redirects::List)
            .await
            .unwrap(),
        Download::Unchanged
    ));
    let result = get(&url, connector, None, Redirects::List).await;
    assert!(
        matches!(result, Err(e) if e.to_string() == "unexpected 304 without a conditional request")
    );
}

#[tokio::test]
async fn a_download_over_the_total_budget_keeps_its_previous_copy_and_reports_failure() {
    use crate::list_files::MAX_LISTS;

    let (_dir, paths) = data();
    let previous = b"||old.example^\n";
    fetch_lists(&paths, &|_| {
        std::future::ready(Ok(Bytes::from_static(previous)))
    })
    .await;
    std::fs::create_dir(paths.rules()).unwrap();
    for index in 0..MAX_LISTS / MAX_SIZE {
        let size = MAX_SIZE - if index == 0 { previous.len() } else { 0 };
        std::fs::File::create(paths.rules().join(format!("{index}.txt")))
            .unwrap()
            .set_len(size as u64)
            .unwrap();
    }

    let result = fetch_lists(&paths, &|_| {
        std::future::ready(Ok(Bytes::from_static(
            b"||new-that-does-not-fit.example^\n",
        )))
    })
    .await;
    assert_eq!((result.updated, result.failed), (0, 1));
    assert_eq!(
        std::fs::read(paths.lists().join(file_name(FIRST))).unwrap(),
        previous
    );
    let info = sources::info(&paths.list_info(), FIRST).unwrap();
    assert!(info.error.unwrap().contains("limit"));
    assert_eq!(info.rules, Some(1));
}

#[tokio::test]
async fn a_targeted_update_only_fetches_the_requested_enabled_subscription() {
    let (_dir, paths) = data();
    std::fs::write(paths.sources(), format!("{FIRST}\n{SECOND}\n{SECOND}\n")).unwrap();
    std::fs::create_dir_all(paths.lists()).unwrap();
    let first = paths.lists().join(file_name(FIRST));
    std::fs::write(&first, "||first.example^\n").unwrap();
    let calls = std::cell::Cell::new(0);
    let download = |url, _, _| {
        assert_eq!(url, SECOND);
        calls.set(calls.get() + 1);
        std::future::ready(Ok(Download::Body(http::Body::new(
            Bytes::from_static(b"||second.example^\n"),
            None,
        ))))
    };
    let target = Subscription {
        sources: &paths.sources(),
        default: "",
        kind: ListKind::Filter,
        dir: &paths.lists(),
        info: Some(&paths.list_info()),
        only: Some(SECOND),
    };
    let mut updates = Vec::new();
    let tally = fetch_validated(target, &download, &mut |event| updates.push(event))
        .await
        .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(tally.updated, 1);
    assert!(matches!(updates[0], Update::Start { lists: 1 }));
    assert_eq!(
        std::fs::read_to_string(first).unwrap(),
        "||first.example^\n"
    );
    assert!(paths.lists().join(file_name(SECOND)).exists());

    sources::edit(&paths, SECOND, sources::Change::Disable).unwrap();
    let tally = fetch_validated(target, &download, &mut |_| {})
        .await
        .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(tally.updated, 0);
    assert!(!paths.lists().join(file_name(SECOND)).exists());
}

#[tokio::test]
async fn lists_disabled_or_removed_during_download_do_not_return() {
    for (remove, success) in [(false, true), (true, true), (false, false), (true, false)] {
        let (_dir, paths) = data();
        fetch_lists(&paths, &|_| {
            std::future::ready(Ok(Bytes::from_static(b"||old.example^\n")))
        })
        .await;
        let file = paths.lists().join(file_name(FIRST));
        assert!(file.exists());
        assert!(sources::info(&paths.list_info(), FIRST).is_some());

        let fetch = |_| {
            let change = if remove {
                sources::Change::Remove
            } else {
                sources::Change::Disable
            };
            assert!(sources::edit(&paths, FIRST, change).unwrap());
            let result = if success {
                Ok(Bytes::from_static(b"||ads.example^\n"))
            } else {
                Err(anyhow!("the list was unavailable"))
            };
            std::future::ready(result)
        };
        let tally = fetch_lists(&paths, &fetch).await;
        assert!(
            !sources::read(&paths)
                .iter()
                .any(|s| s.url == FIRST && s.enabled)
        );
        let filters = crate::filter::Filters::load(&[&paths.lists()], false).unwrap();
        assert!(!filters.blocks_host("old.example"));
        assert!(!filters.blocks_host("ads.example"));
        assert!(!file.exists());
        assert_eq!(sources::info(&paths.list_info(), FIRST).is_none(), remove);
        assert_eq!(sources::has_disabled_copy(&paths.lists(), FIRST), !remove);
        assert_eq!((tally.updated, tally.unchanged, tally.failed), (0, 1, 0));
    }
}

#[tokio::test]
async fn cleanup_keeps_a_list_added_and_fetched_during_an_update() {
    let (_dir, paths) = data();
    let fetch = |_| async {
        sources::edit(&paths, SECOND, sources::Change::Add).unwrap();
        fetch_lists(&paths, &|url| {
            let body: &'static [u8] = if url == FIRST {
                b"||first.example^\n"
            } else {
                b"||second.example^\n"
            };
            std::future::ready(Ok(Bytes::from_static(body)))
        })
        .await;
        Ok(Bytes::from_static(b"||first.example^\n"))
    };
    fetch_lists(&paths, &fetch).await;
    let filters = crate::filter::Filters::load(&[&paths.lists()], false).unwrap();
    assert!(filters.blocks_host("first.example") && filters.blocks_host("second.example"));
    assert!(sources::info(&paths.list_info(), SECOND).is_some());
}

#[tokio::test]
async fn removing_an_unsubscribed_list_counts_as_a_change_to_reload() {
    let (_dir, paths) = data();
    fetch_lists(&paths, &|_| {
        std::future::ready(Ok(Bytes::from_static(b"||old.example^\n")))
    })
    .await;
    std::fs::write(paths.sources(), format!("#{FIRST}\n")).unwrap();

    let tally = fetch_lists(&paths, &|_| -> std::future::Ready<Result<Bytes>> {
        panic!("an unsubscribed list should never be fetched")
    })
    .await;
    assert_eq!(tally.changed(), 1);
    assert!(!paths.lists().join(file_name(FIRST)).exists());
    assert!(sources::info(&paths.list_info(), FIRST).is_none());
}

#[tokio::test]
async fn a_download_can_finish_for_a_default_list_after_sources_becomes_absent() {
    let (_dir, paths) = data();
    let sources = paths.sources();
    let fetch = |_| {
        let _lock = files::lock(&sources).unwrap();
        std::fs::remove_file(&sources).unwrap();
        std::future::ready(Ok(Bytes::from_static(b"||default.example^\n")))
    };
    let tally = fetch_all(
        &sources,
        FIRST,
        &paths.lists(),
        Some(&paths.list_info()),
        &fetch,
        &mut |_| {},
    )
    .await
    .unwrap();
    assert!(!sources.exists());
    assert_eq!(tally.updated, 1);
    assert!(paths.lists().join(file_name(FIRST)).exists());
}

#[tokio::test]
async fn downloading_the_default_lists_does_not_save_them_as_a_choice() {
    let (_dir, paths) = data();
    let sources = paths.sources();
    std::fs::remove_file(&sources).unwrap();
    let fetch = |_| std::future::ready(Ok(Bytes::from_static(b"||default.example^\n")));
    let tally = fetch_all(
        &sources,
        &format!("{FIRST}\n"),
        &paths.lists(),
        Some(&paths.list_info()),
        &fetch,
        &mut |_| {},
    )
    .await
    .unwrap();
    assert_eq!(tally.updated, 1);
    assert!(paths.lists().join(file_name(FIRST)).exists());
    // Defaults that change later, with the catalog or the device's
    // language, still reach a selection nobody made.
    assert!(!sources.exists());
}

#[test]
fn rejects_an_error_page_in_place_of_a_list() {
    assert!(looks_like_a_list(b"[Adblock Plus 2.0]\n||ads.example^\n"));
    assert!(!looks_like_a_list(b"\n<!DOCTYPE html><html>"));
    assert!(!looks_like_a_list(b""));
}
