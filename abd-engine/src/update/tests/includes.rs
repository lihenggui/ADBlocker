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

use std::collections::HashMap;
use std::sync::Mutex;

use http_body_util::Full;
use hyper::header::LOCATION;
use hyper::{Response, StatusCode};

use super::server::{Server, serve};
use super::*;
use crate::limits::MAX_INCLUDES;
use crate::list_url::{Trust, include_address, trusted_list_file};
use crate::rule::SourcePermissions;

const ROOT: &str = "/path/list.txt";
const CHILD: &str = "/path/child.txt";
const OLD: &str = "||old.example^\n";

type Requests = Vec<(String, Option<String>)>;

#[derive(Clone)]
enum Reply {
    File(Bytes, &'static str),
    Redirect(String),
}

#[derive(Default)]
struct Exchange {
    replies: Mutex<HashMap<String, Reply>>,
    requests: Mutex<Requests>,
}

struct Rig {
    _dir: tempfile::TempDir,
    paths: Paths,
    server: Server,
    exchange: Arc<Exchange>,
    url: String,
}

impl Rig {
    async fn start(trusted: bool) -> Self {
        let exchange = Arc::new(Exchange::default());
        let server = serve({
            let exchange = exchange.clone();
            move |req| {
                let path = req.uri().path_and_query().unwrap().as_str();
                let tag = req
                    .headers()
                    .get(IF_NONE_MATCH)
                    .map(|v| v.to_str().unwrap());
                exchange
                    .requests
                    .lock()
                    .unwrap()
                    .push((path.to_owned(), tag.map(str::to_owned)));
                let reply = exchange.replies.lock().unwrap().get(path).cloned();
                let response = Response::builder();
                let (response, body) = match reply {
                    Some(Reply::File(_, etag)) if tag == Some(etag) => {
                        (response.status(StatusCode::NOT_MODIFIED), Bytes::new())
                    }
                    Some(Reply::File(body, etag)) => (response.header(ETAG, etag), body),
                    Some(Reply::Redirect(url)) => (
                        response.status(StatusCode::FOUND).header(LOCATION, url),
                        Bytes::new(),
                    ),
                    None => (response.status(StatusCode::NOT_FOUND), Bytes::new()),
                };
                response.body(Full::new(body)).unwrap()
            }
        })
        .await;
        let url = if trusted {
            Trust::Scripts.urls()[0].to_owned()
        } else {
            format!("{}{ROOT}", server.origin)
        };
        let (_dir, paths) = data();
        std::fs::write(paths.sources(), format!("{url}\n")).unwrap();
        Self {
            _dir,
            paths,
            server,
            exchange,
            url,
        }
    }

    fn file(&self, path: &str, data: impl Into<Bytes>, etag: &'static str) {
        self.exchange
            .replies
            .lock()
            .unwrap()
            .insert(path.to_owned(), Reply::File(data.into(), etag));
    }

    fn redirect(&self, path: &str, url: String) {
        self.exchange
            .replies
            .lock()
            .unwrap()
            .insert(path.to_owned(), Reply::Redirect(url));
    }

    fn requests(&self) -> Requests {
        std::mem::take(&mut *self.exchange.requests.lock().unwrap())
    }

    async fn update(&self) -> Tally {
        let fetch = |url: String, saved: Option<Validator>, redirects| {
            // Map the exact catalog address to the local TLS rig only at the
            // transport boundary, after the resolver has selected its policy.
            let catalog_host = crate::list_url::split(&self.url).unwrap().0;
            let resource = if crate::list_url::trusted_download(&self.url) {
                let path = url
                    .strip_prefix(&format!("https://{catalog_host}"))
                    .unwrap();
                format!("{}{path}", self.server.origin)
            } else {
                url
            };
            async move { get(&resource, &self.server.connector, saved.as_ref(), redirects).await }
        };
        fetch_validated(
            Subscription {
                sources: &self.paths.sources(),
                default: "",
                kind: ListKind::Filter,
                dir: &self.paths.lists(),
                info: Some(&self.paths.list_info()),
                only: None,
            },
            &fetch,
            &mut |_| {},
        )
        .await
        .unwrap()
    }

    fn stored(&self) -> std::path::PathBuf {
        self.paths.lists().join(file_name(&self.url))
    }

    fn text(&self) -> String {
        std::fs::read_to_string(self.stored()).unwrap()
    }

    fn info(&self) -> Info {
        sources::info(&self.paths.list_info(), &self.url).unwrap()
    }

    fn assert_failure(&self, why: &str) {
        let error = self.info().error.unwrap();
        assert!(error.contains(why), "{error}");
    }
}

fn requests(entries: &[(&str, Option<&str>)]) -> Requests {
    entries
        .iter()
        .map(|(path, tag)| (path.to_string(), tag.map(str::to_owned)))
        .collect()
}

#[test]
fn include_addresses_follow_the_reference_examples_and_stay_inside_the_directory() {
    let base = "https://example.org/path/filter.txt?old=1#old";
    for arg in [
        "https://example.org/path/includedfile.txt",
        "https://EXAMPLE.org:443/path/includedfile.txt",
        "HTTPS://example.org/path/includedfile.txt",
        "/includedfile.txt",
        "includedfile.txt",
        "./section/../includedfile.txt",
        "section/%2e%2E/includedfile.txt",
    ] {
        assert_eq!(
            include_address(base, arg).as_deref(),
            Some("https://example.org/path/includedfile.txt"),
            "{arg}"
        );
    }
    assert_eq!(
        include_address(base, "includedfile.txt?new=1#new").as_deref(),
        Some("https://example.org/path/includedfile.txt?new=1")
    );
    assert_eq!(
        include_address(base, "sub//includedfile.txt").as_deref(),
        Some("https://example.org/path/sub//includedfile.txt")
    );
    for arg in [
        "../path2/includedfile.txt",
        "https://domain.com/path/includedfile.txt",
        "http://example.org/path/includedfile.txt",
        "https://example.org:444/path/includedfile.txt",
        "https://example.org/path2/includedfile.txt",
        "../path-extra/includedfile.txt",
        "%2e%2e/path2/includedfile.txt",
        ".%2E/path2/includedfile.txt",
        "nested//../../../path2/includedfile.txt",
        "..%2fpath2/includedfile.txt",
        "..%5Cpath2/includedfile.txt",
        "%252e%252e/includedfile.txt",
        r"..\path2\includedfile.txt",
        "//domain.com/path/includedfile.txt",
        "",
    ] {
        assert_eq!(include_address(base, arg), None, "{arg}");
    }
}

#[tokio::test]
async fn the_stored_list_inlines_only_active_valid_includes_and_keeps_its_title() {
    let rig = Rig::start(false).await;
    let root = format!(
        "[Adblock Plus 2.0]\r\n!#if (adguard_app_android)\n!#include /child.txt\n\
         !#else\n!#include inactive.txt\n!#endif\n! Title: Parent\n||parent.example^\n\
         !#include {}/path/absolute.txt\n!#include relative.txt\n\
         !#include ../path2/includedfile.txt\n!#include https://domain.com/path/includedfile.txt\n",
        rig.server.origin
    );
    rig.file(ROOT, root, "\"root\"");
    rig.file(
        CHILD,
        "! Title: Child\n! Description: header\n||child.example^",
        "\"child\"",
    );
    rig.file(
        "/path/absolute.txt",
        "||absolute.example^\n",
        "\"absolute\"",
    );
    rig.file(
        "/path/relative.txt",
        "||relative.example^\n",
        "\"relative\"",
    );
    assert_eq!(rig.update().await.updated, 1);
    assert_eq!(
        rig.requests(),
        requests(&[
            (ROOT, None),
            (CHILD, None),
            ("/path/absolute.txt", None),
            ("/path/relative.txt", None)
        ])
    );
    let stored = rig.text();
    assert!(stored.contains("! >>> include: /child.txt\n!! Title: Child\n! Description: header\n||child.example^\n! <<< include: /child.txt\n"));
    assert!(stored.contains("!#include inactive.txt\n"));
    let checked = crate::rulecheck::check(&stored, SourcePermissions::USER);
    assert_eq!(checked.rules, 4);
    assert_eq!(
        checked
            .issues
            .iter()
            .map(|issue| issue.text.as_str())
            .collect::<Vec<_>>(),
        [
            "!#include ../path2/includedfile.txt",
            "!#include https://domain.com/path/includedfile.txt",
        ]
    );
    assert_eq!(sources::title(&stored).as_deref(), Some("Parent"));
    let info = rig.info();
    assert_eq!(info.title.as_deref(), Some("Parent"));
    assert_eq!(info.rules, Some(4));
    let filters =
        crate::filter::Filters::load_cached(&[&rig.paths.lists()], &rig.paths.cache()).unwrap();
    assert!(filters.blocks_host("child.example"));
}

#[tokio::test]
async fn nested_files_resolve_from_their_own_directory_and_duplicate_files_end_cycles() {
    let rig = Rig::start(false).await;
    rig.file(
        ROOT,
        "!#include child.txt\n!#include ./child.txt#again\n",
        "\"root\"",
    );
    rig.file(
        CHILD,
        "!#include ./list.txt\n!#include sub/next.txt\n",
        "\"child\"",
    );
    rig.file("/path/sub/next.txt", "!#include last.txt\n", "\"next\"");
    rig.file("/path/sub/last.txt", "||nested.example^\n", "\"last\"");
    assert_eq!(rig.update().await.updated, 1);
    assert_eq!(
        rig.requests(),
        requests(&[
            (ROOT, None),
            (CHILD, None),
            ("/path/sub/next.txt", None),
            ("/path/sub/last.txt", None)
        ])
    );
    assert_eq!(rig.info().rules, Some(1));
    assert_eq!(rig.text().matches("Skipped duplicate include").count(), 2);
    assert!(sources::title(&rig.text()).is_none());
}

#[tokio::test]
async fn too_many_nested_includes_fail_before_fetching_the_deepest_file() {
    let rig = Rig::start(false).await;
    rig.file(ROOT, "!#include one.txt\n", "\"root\"");
    for (from, to) in [("one", "two"), ("two", "three"), ("three", "four")] {
        rig.file(
            &format!("/path/{from}.txt"),
            format!("!#include {to}.txt\n"),
            "\"body\"",
        );
    }
    assert_eq!(rig.update().await.failed, 1);
    rig.assert_failure("include four.txt: include depth exceeds 3");
    assert_eq!(rig.requests().len(), 4);
    assert!(!rig.stored().exists());
}

#[tokio::test]
async fn too_many_files_fail_before_fetching_the_extra_file() {
    let rig = Rig::start(false).await;
    let includes = (0..=MAX_INCLUDES)
        .map(|i| format!("!#include {i}.txt\n"))
        .collect::<String>();
    rig.file(ROOT, includes, "\"root\"");
    for i in 0..=MAX_INCLUDES {
        rig.file(&format!("/path/{i}.txt"), OLD, "\"body\"");
    }
    assert_eq!(rig.update().await.failed, 1);
    rig.assert_failure("include 32.txt: list has more than 32 includes");
    assert_eq!(rig.requests().len(), MAX_INCLUDES + 1);
    assert!(!rig.stored().exists());
}

#[tokio::test]
async fn expanded_size_includes_the_inserted_text_and_the_unmodified_suffix() {
    let rig = Rig::start(false).await;
    let mut root = String::from("!#include child.txt\n!");
    root.push_str(&"a".repeat(MAX_SIZE / 2));
    rig.file(ROOT, root, "\"root\"");
    rig.file(CHILD, "!".repeat(MAX_SIZE / 2), "\"child\"");
    assert_eq!(rig.update().await.failed, 1);
    rig.assert_failure("include child.txt: expanded list exceeds the 16 MiB limit");
    assert!(!rig.stored().exists());
}

#[tokio::test]
async fn failed_includes_leave_the_previous_copy_and_validators_in_use() {
    let rig = Rig::start(false).await;
    rig.file(ROOT, "!#include keep.txt\n", "\"old\"");
    rig.file("/path/keep.txt", OLD, "\"keep\"");
    assert_eq!(rig.update().await.updated, 1);
    let old = rig.text();
    let info = rig.info();
    rig.file(ROOT, "!#include child.txt\n", "\"new\"");
    for (reply, error) in [
        (None, "404"),
        (
            Some(Reply::File(
                Bytes::from_static(b"<html>Error</html>"),
                "\"bad\"",
            )),
            "the answer is not a list",
        ),
        (
            Some(Reply::Redirect(format!(
                "{}/other.txt",
                rig.server.origin.replace("127.0.0.1", "localhost")
            ))),
            "outside its HTTPS origin",
        ),
        (
            Some(Reply::File(
                Bytes::from(vec![b'a'; MAX_SIZE + 1]),
                "\"big\"",
            )),
            "reading the list",
        ),
    ] {
        if let Some(reply) = reply {
            rig.exchange
                .replies
                .lock()
                .unwrap()
                .insert(CHILD.to_owned(), reply);
        }
        rig.requests();
        assert_eq!(rig.update().await.failed, 1);
        assert_eq!(
            rig.requests(),
            requests(&[(ROOT, Some("\"old\"")), (CHILD, None)])
        );
        assert_eq!(rig.text(), old);
        let mut failed = rig.info();
        let reason = failed.error.take().unwrap();
        assert!(
            reason.starts_with("include child.txt:") && reason.contains(error),
            "{reason}"
        );
        assert_eq!(failed, info);
    }
}

#[tokio::test]
async fn revalidation_keeps_the_copy_when_all_files_match_and_refreshes_a_changed_child() {
    let rig = Rig::start(false).await;
    rig.file(
        ROOT,
        "!#include child.txt\n!#include next.txt\n",
        "\"root\"",
    );
    rig.file(CHILD, OLD, "\"old\"");
    rig.file("/path/next.txt", "||keep.example^\n", "\"keep\"");
    assert_eq!(rig.update().await.updated, 1);
    assert_eq!(
        rig.requests(),
        requests(&[(ROOT, None), (CHILD, None), ("/path/next.txt", None)])
    );
    let stamp = file_stamp(&rig.stored());
    assert_eq!(rig.update().await.unchanged, 1);
    assert_eq!(
        rig.requests(),
        requests(&[
            (ROOT, Some("\"root\"")),
            (CHILD, Some("\"old\"")),
            ("/path/next.txt", Some("\"keep\""))
        ])
    );
    assert_eq!(file_stamp(&rig.stored()), stamp);
    rig.file("/path/next.txt", "||new.example^\n", "\"new\"");
    assert_eq!(rig.update().await.updated, 1);
    assert_eq!(
        rig.requests(),
        requests(&[
            (ROOT, Some("\"root\"")),
            (CHILD, Some("\"old\"")),
            ("/path/next.txt", Some("\"keep\"")),
            (ROOT, None),
            (CHILD, None)
        ])
    );
    let stored = rig.text();
    assert!(stored.contains("||old.example^") && stored.contains("||new.example^"));
    assert!(!stored.contains("||keep.example^"));
    let info = rig.info();
    let includes = info.includes.unwrap();
    assert_eq!(includes[1].path, "next.txt");
    assert_eq!(
        includes[1].validator.as_ref().unwrap().tag.as_deref(),
        Some("\"new\"")
    );
}

#[tokio::test]
async fn a_child_can_revalidate_when_the_root_has_no_validator() {
    let rig = Rig::start(false).await;
    rig.file(ROOT, "!#include child.txt\n", "");
    rig.file(CHILD, OLD, "\"child\"");
    assert_eq!(rig.update().await.updated, 1);
    assert!(rig.info().validator.is_none());
    rig.requests();
    assert_eq!(rig.update().await.unchanged, 1);
    assert_eq!(
        rig.requests(),
        requests(&[(ROOT, None), (CHILD, Some("\"child\"")), (CHILD, None)])
    );
}

#[tokio::test]
async fn a_changed_including_file_can_remove_a_missing_dependency() {
    let rig = Rig::start(false).await;
    rig.file(ROOT, "!#include child.txt\n", "\"root\"");
    rig.file(CHILD, "!#include gone.txt\n", "\"old\"");
    rig.file("/path/gone.txt", OLD, "\"gone\"");
    assert_eq!(rig.update().await.updated, 1);
    rig.requests();
    rig.file(CHILD, "||new.example^\n", "\"new\"");
    rig.exchange
        .replies
        .lock()
        .unwrap()
        .remove("/path/gone.txt");
    assert_eq!(rig.update().await.updated, 1);
    assert_eq!(
        rig.requests(),
        requests(&[
            (ROOT, Some("\"root\"")),
            (CHILD, Some("\"old\"")),
            (ROOT, None)
        ])
    );
    assert!(rig.text().contains("||new.example^"));
    assert_eq!(rig.info().includes.unwrap().len(), 1);
}

#[tokio::test]
async fn a_marked_list_keeps_trust_for_direct_same_origin_includes_and_rejects_redirects() {
    let rig = Rig::start(true).await;
    let (_, _, root) = crate::list_url::split(&rig.url).unwrap();
    let (directory, _) = root.rsplit_once('/').unwrap();
    let child = format!("{directory}/child.txt");
    rig.file(
        root,
        "!#include child.txt\n!#include https://another.example/foreign.txt\n",
        "\"root\"",
    );
    rig.file(&child, OLD, "\"child\"");
    assert_eq!(rig.update().await.updated, 1);
    assert_eq!(rig.requests(), requests(&[(root, None), (&child, None)]));
    let old = rig.text();
    for trust in [Trust::Scriptlets, Trust::Scripts, Trust::ResponseRules] {
        assert!(trusted_list_file(&rig.stored(), trust));
    }
    assert!(old.contains(OLD));
    rig.redirect(
        &child,
        format!("{}{directory}/elsewhere.txt", rig.server.origin),
    );
    assert_eq!(rig.update().await.failed, 1);
    rig.assert_failure("include child.txt: an include of a trusted list cannot redirect");
    assert_eq!(
        rig.requests(),
        requests(&[(root, Some("\"root\"")), (&child, Some("\"child\""))])
    );
    assert_eq!(rig.text(), old);
}

#[tokio::test]
async fn custom_lists_follow_same_origin_redirects_without_gaining_trust() {
    let rig = Rig::start(false).await;
    rig.file(ROOT, "!#include child.txt\n", "\"root\"");
    rig.redirect(CHILD, format!("{}/path/sub/target.txt", rig.server.origin));
    rig.file(
        "/path/sub/target.txt",
        "!#include sibling.txt\n",
        "\"target\"",
    );
    rig.file("/path/sub/sibling.txt", OLD, "\"child\"");
    assert_eq!(rig.update().await.updated, 1);
    assert_eq!(rig.requests().len(), 4);
    assert!(rig.text().contains(OLD));
    assert!(!trusted_list_file(&rig.stored(), Trust::Scripts));
    assert_eq!(rig.update().await.unchanged, 1);
    assert_eq!(
        rig.requests(),
        requests(&[
            (ROOT, Some("\"root\"")),
            (CHILD, None),
            ("/path/sub/target.txt", Some("\"target\"")),
            ("/path/sub/sibling.txt", Some("\"child\""))
        ])
    );
}

#[tokio::test]
async fn legacy_list_info_is_read_and_a_fresh_body_expands_its_includes() {
    let rig = Rig::start(false).await;
    std::fs::create_dir_all(rig.paths.lists()).unwrap();
    std::fs::write(rig.stored(), "! Title: Parent\n!#include child.txt\n").unwrap();
    let old = serde_json::json!({
        "url": rig.url,
        "title": "Parent",
        "rules": 0,
        "rules_format": 1,
        "updated": 123,
        "error": "old failure",
        "validator": {
            "resource": rig.url,
            "tag": "\"root\"",
            "modified": null,
            "file": file_stamp(&rig.stored()).unwrap()
        }
    });
    std::fs::create_dir_all(rig.paths.list_info()).unwrap();
    std::fs::write(
        sources::info_file(&rig.paths.list_info(), &rig.url),
        old.to_string(),
    )
    .unwrap();
    let info = rig.info();
    assert_eq!(info.title.as_deref(), Some("Parent"));
    assert_eq!(info.updated, Some(123));
    assert_eq!(info.error.as_deref(), Some("old failure"));
    assert!(info.validator.is_some() && info.includes.is_none());
    rig.file(ROOT, "! Title: Parent\n!#include child.txt\n", "\"root\"");
    rig.file(CHILD, OLD, "\"child\"");
    assert_eq!(rig.update().await.updated, 1);
    assert_eq!(rig.requests(), requests(&[(ROOT, None), (CHILD, None)]));
    assert!(rig.text().contains(OLD));
    let info = rig.info();
    assert_eq!(info.title.as_deref(), Some("Parent"));
    assert_eq!(info.rules, Some(1));
    assert!(info.validator.is_some() && info.error.is_none());
    assert_eq!(info.includes.unwrap().len(), 1);
}
