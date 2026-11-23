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
use hyper::Request;
use hyper::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use hyper_util::rt::TokioIo;

use super::http::{conditional, validator};
use super::kind::looks_like_a_list;
use super::*;

const FIRST: &str = "https://first.example/list.txt";
const SECOND: &str = "https://second.example/list.txt";

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
    let reply = |_, _| {
        std::future::ready(Ok(Download::Body(
            Bytes::from_static(b"updated.example\n"),
            None,
        )))
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
    let reply = |url: String, _| {
        let body = if url == FIRST {
            "intranet\n"
        } else {
            "Unavailable\n"
        };
        std::future::ready(Ok(Download::Body(
            Bytes::from_static(body.as_bytes()),
            None,
        )))
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
        let reply = |_, _| {
            std::future::ready(Ok(Download::Body(
                Bytes::from_static(body.as_bytes()),
                None,
            )))
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
    let download = |_, saved: Option<Validator>| {
        let n = calls.get();
        calls.set(n + 1);
        let reply = match n {
            0 | 3 => {
                assert!(saved.is_none());
                let mut headers = hyper::HeaderMap::new();
                headers.insert(ETAG, "\"one\"".parse().unwrap());
                Download::Body(
                    Bytes::from_static(b"! Title: List\n||ads.example^"),
                    validator(FIRST, &headers),
                )
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
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.cert.der().clone()).unwrap();
    let client = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());
    let server = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key.into())
        .unwrap();
    let tls = tokio_rustls::TlsAcceptor::from(Arc::new(server));
    let task = tokio::spawn(async move {
        for n in 0..3 {
            let (tcp, _) = listener.accept().await.unwrap();
            let stream = tls.accept(tcp).await.unwrap();
            let service =
                hyper::service::service_fn(move |req: Request<hyper::body::Incoming>| async move {
                    assert_eq!(
                        req.headers().get(IF_NONE_MATCH).map(|v| v.as_bytes()),
                        (n == 1).then_some(b"\"one\"".as_slice())
                    );
                    let reply = if n == 0 {
                        Response::builder()
                            .header(ETAG, "\"one\"")
                            .body(Full::new(Bytes::from_static(b"||ads.example^")))
                    } else {
                        Response::builder()
                            .status(StatusCode::NOT_MODIFIED)
                            .body(Full::new(Bytes::new()))
                    };
                    Ok::<_, std::convert::Infallible>(reply.unwrap())
                });
            hyper::server::conn::http1::Builder::new()
                .keep_alive(false)
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        }
    });
    let url = format!("https://{addr}/list.txt");
    let connector = TlsConnector::from(Arc::new(client));
    let Download::Body(body, saved) = get(&url, &connector, None).await.unwrap() else {
        panic!()
    };
    assert_eq!(body.as_ref(), b"||ads.example^");
    assert!(matches!(
        get(&url, &connector, saved.as_ref()).await.unwrap(),
        Download::Unchanged
    ));
    assert!(get(&url, &connector, None).await.is_err());
    task.await.unwrap();
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
    let download = |url, _| {
        assert_eq!(url, SECOND);
        calls.set(calls.get() + 1);
        std::future::ready(Ok(Download::Body(
            Bytes::from_static(b"||second.example^\n"),
            None,
        )))
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

#[test]
fn rejects_an_error_page_in_place_of_a_list() {
    assert!(looks_like_a_list(b"[Adblock Plus 2.0]\n||ads.example^\n"));
    assert!(!looks_like_a_list(b"\n<!DOCTYPE html><html>"));
    assert!(!looks_like_a_list(b""));
}
