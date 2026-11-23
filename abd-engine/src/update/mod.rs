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

//! Downloads the filter lists.

mod http;
mod kind;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use tokio_rustls::TlsConnector;

use http::{Download, get};
pub(crate) use kind::ListKind;

use crate::files;
use crate::list_files::check_growth;
pub use crate::list_url::{file_name, split};
use crate::paths::Paths;
use crate::protocol_types::{ListState, Update};
pub use crate::sources::prune_unsubscribed;
use crate::sources::{self, Info, Validator, subscribed};

/// A server sending more than a list may hold is not sending a list.
const MAX_SIZE: usize = crate::list_files::MAX_LIST;
const TIMEOUT: Duration = Duration::from_secs(60);

/// The longest reason a list failed that is kept and told.
const MAX_ERROR: usize = 200;
/// Downloads and publishes both kinds of lists, or only the selected filter
/// list. The update reports each download and its result.
pub(crate) async fn refresh(
    paths: &Paths,
    roots: Arc<rustls::RootCertStore>,
    only: Option<&str>,
    report: &mut dyn FnMut(ListKind, Update),
) -> Result<(Tally, Tally)> {
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let tls = TlsConnector::from(Arc::new(config));
    let fetch = |url: String, saved: Option<Validator>| {
        let tls = tls.clone();
        async move { get(&url, &tls, saved.as_ref()).await }
    };
    let filters = fetch_validated(
        Subscription {
            sources: &paths.sources(),
            default: crate::catalog::default_sources(),
            kind: ListKind::Filter,
            dir: &paths.lists(),
            info: Some(&paths.list_info()),
            only,
        },
        &fetch,
        &mut |event| report(ListKind::Filter, event),
    )
    .await?;
    if only.is_some() {
        return Ok((filters, Tally::default()));
    }
    let exclusions = fetch_validated(
        Subscription {
            sources: &paths.exclusion_sources(),
            default: crate::catalog::exclusion_sources(),
            kind: ListKind::HttpsExclusion,
            dir: &paths.exclusions(),
            info: Some(&paths.exclusions().join("info")),
            only: None,
        },
        &fetch,
        &mut |event| report(ListKind::HttpsExclusion, event),
    )
    .await?;
    Ok((filters, exclusions))
}

/// How the lists of one sources file fared.
#[derive(Debug, Default)]
pub struct Tally {
    pub updated: usize,
    pub unchanged: usize,
    pub failed: usize,
    removed: usize,
}

impl Tally {
    pub(crate) fn changed(&self) -> usize {
        self.updated + self.removed
    }

    pub fn done(&self, exclusions_ready: bool, exclusions_failed: usize) -> Update {
        Update::Done {
            updated: self.updated,
            unchanged: self.unchanged,
            failed: self.failed,
            exclusions_ready,
            exclusions_failed,
        }
    }
}

/// `e`, short enough to be kept and shown.
fn reason(e: &anyhow::Error) -> String {
    let mut text = format!("{e:#}");
    if text.len() > MAX_ERROR {
        let mut end = MAX_ERROR;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

fn configured_sources(sources: &Path, default: &str) -> Result<String> {
    // Rollback can restore the file's absence during a download. The
    // defaults are still subscribed when that happens.
    if sources.try_exists()? {
        Ok(files::read_text(sources, sources::MAX_FILE)?)
    } else {
        Ok(default.to_owned())
    }
}

/// Fetches every list named in `sources` (created with `default` if it is
/// not there) into `dir` with `get`, and removes from `dir` the lists no
/// longer named. With `info`, what is known of each list is kept there
/// (`sources::Info`). `report` is told what happens as it does.
pub async fn fetch_all<G, F>(
    sources: &Path,
    default: &str,
    dir: &Path,
    info: Option<&Path>,
    get: &G,
    report: &mut dyn FnMut(Update),
) -> Result<Tally>
where
    G: Fn(String) -> F,
    F: Future<Output = Result<Bytes>>,
{
    fetch_validated(
        Subscription {
            sources,
            default,
            kind: ListKind::Filter,
            dir,
            info,
            only: None,
        },
        &|url, _| {
            let future = get(url);
            async move { Ok(Download::Body(future.await?, None)) }
        },
        report,
    )
    .await
}

#[derive(Clone, Copy)]
struct Subscription<'a> {
    sources: &'a Path,
    default: &'a str,
    kind: ListKind,
    dir: &'a Path,
    info: Option<&'a Path>,
    only: Option<&'a str>,
}

struct Outcome {
    state: ListState,
    rules: Option<usize>,
    error: Option<String>,
}

enum Commit {
    Finished(Outcome),
    RetryWithoutValidator,
}

async fn fetch_validated<G, F>(
    lists: Subscription<'_>,
    get: &G,
    report: &mut dyn FnMut(Update),
) -> Result<Tally>
where
    G: Fn(String, Option<Validator>) -> F,
    F: Future<Output = Result<Download>>,
{
    let urls: Vec<String> = {
        let _lock = files::lock(lists.sources)?;
        let configured = lists.kind.start_sources(lists.sources, lists.default)?;
        let mut enabled = subscribed(&configured);
        match lists.only {
            Some(target) => enabled
                .find(|url| *url == target)
                .map(str::to_owned)
                .into_iter()
                .collect(),
            None => enabled.map(str::to_owned).collect(),
        }
    };
    std::fs::create_dir_all(lists.dir)?;

    report(Update::Start { lists: urls.len() });
    let mut tally = Tally::default();
    for url in &urls {
        let event = |state, rules, error| Update::List {
            url: url.clone(),
            state,
            rules,
            error,
        };
        report(event(ListState::Fetching, None, None));
        let Outcome {
            state,
            rules,
            error,
        } = lists.fetch_one(url, get).await?;
        match state {
            ListState::Updated => tally.updated += 1,
            ListState::Unchanged => tally.unchanged += 1,
            ListState::Failed => tally.failed += 1,
            ListState::Fetching => unreachable!("a fetch has finished"),
        }
        // A reader can be slow. It never holds the data lock.
        report(event(state, rules, error));
    }
    if lists.only.is_none() {
        tally.removed = lists.cleanup()?;
    }
    Ok(tally)
}

impl Subscription<'_> {
    async fn fetch_one<G, F>(self, url: &str, get: &G) -> Result<Outcome>
    where
        G: Fn(String, Option<Validator>) -> F,
        F: Future<Output = Result<Download>>,
    {
        let file = self.dir.join(file_name(url));
        let saved = self
            .info
            .and_then(|dir| sources::info(dir, url))
            .and_then(|i| i.validator)
            .filter(|v| Some(v.file) == file_stamp(&file));
        let mut condition = saved.as_ref();
        let bundled = matches!(self.kind, ListKind::HttpsExclusion)
            && subscribed(self.default).any(|source| source == url);
        loop {
            let fetched = match tokio::time::timeout(
                TIMEOUT,
                get(url.to_owned(), condition.cloned()),
            )
            .await
            {
                Ok(Ok(Download::Body(body, validator))) if self.kind.accepts(&body, bundled) => {
                    Ok(Download::Body(body, validator))
                }
                Ok(Ok(Download::Unchanged)) if condition.is_some() => Ok(Download::Unchanged),
                Ok(Ok(_)) => Err("the answer is not a list".to_owned()),
                Ok(Err(e)) => Err(reason(&e)),
                Err(_) => Err("timed out".to_owned()),
            };
            match self.commit(url, &file, condition, fetched)? {
                Commit::Finished(outcome) => return Ok(outcome),
                Commit::RetryWithoutValidator => condition = None,
            }
        }
    }

    fn commit(
        self,
        url: &str,
        file: &Path,
        condition: Option<&Validator>,
        fetched: Result<Download, String>,
    ) -> Result<Commit> {
        let _lock = files::lock(self.sources)?;
        let configured = configured_sources(self.sources, self.default)?;
        if !subscribed(&configured).any(|source| source == url) {
            return Ok(Commit::Finished(Outcome {
                state: ListState::Unchanged,
                rules: None,
                error: None,
            }));
        }
        let fetched = fetched.and_then(|download| match download {
            Download::Body(body, validator) => self
                .prepare_body(file, body)
                .map(|body| Download::Body(body, validator))
                .map_err(|e| reason(&e)),
            Download::Unchanged => Ok(Download::Unchanged),
        });
        let previous = || {
            self.info
                .and_then(|dir| sources::info(dir, url))
                .unwrap_or_else(|| Info {
                    url: url.to_owned(),
                    ..Info::default()
                })
        };
        let (state, known) = match fetched {
            Ok(Download::Body(body, validator)) => commit_body(url, file, body, validator)?,
            Ok(Download::Unchanged) => {
                let mut known = previous();
                // A concurrent edit or replacement invalidates the response.
                // Retry once without a validator, outside the lock.
                if known.validator.as_ref() != condition
                    || condition.is_none_or(|v| Some(v.file) != file_stamp(file))
                {
                    return Ok(Commit::RetryWithoutValidator);
                }
                known.error = None;
                known.updated = Some(crate::clock::now());
                (ListState::Unchanged, known)
            }
            Err(why) => {
                let mut known = previous();
                known.error = Some(why);
                (ListState::Failed, known)
            }
        };
        if let Some(dir) = self.info {
            sources::write_info(dir, &known)?;
        }
        Ok(Commit::Finished(Outcome {
            state,
            rules: if state == ListState::Failed {
                None
            } else {
                known.rules
            },
            error: known.error,
        }))
    }

    fn prepare_body(self, file: &Path, body: Bytes) -> Result<Bytes> {
        if matches!(self.kind, ListKind::Filter) {
            let base = self.sources.parent().unwrap_or_else(|| Path::new("."));
            check_growth(&[self.dir, &Paths::new(base).rules()], &[file], body.len())?;
        }
        self.kind.retain_previous(file, body)
    }

    fn cleanup(self) -> Result<usize> {
        let lock = files::lock(self.sources)?;
        let configured = configured_sources(self.sources, self.default)?;
        prune_unsubscribed(&configured, self.dir, self.info, &lock)
    }
}

fn commit_body(
    url: &str,
    file: &Path,
    body: Bytes,
    validator: Option<Validator>,
) -> Result<(ListState, Info)> {
    let same = files::read_bytes(file, body.len()).is_ok_and(|old| old == body);
    let state = if same {
        ListState::Unchanged
    } else {
        files::replace(file, &body)?;
        ListState::Updated
    };
    let text = std::str::from_utf8(&body).unwrap_or_default();
    let known = Info {
        url: url.to_owned(),
        title: sources::title(text),
        rules: Some(crate::rule_text::count(text)),
        updated: Some(crate::clock::now()),
        error: None,
        validator: validator.and_then(|mut v| {
            v.file = file_stamp(file)?;
            Some(v)
        }),
    };
    Ok((state, known))
}

fn file_stamp(file: &Path) -> Option<[u64; 5]> {
    use std::os::unix::fs::MetadataExt as _;
    let m = file.metadata().ok()?;
    (m.is_file() && m.len() <= MAX_SIZE as u64).then(|| {
        [
            m.dev(),
            m.ino(),
            m.len(),
            m.ctime() as u64,
            m.ctime_nsec() as u64,
        ]
    })
}

#[cfg(test)]
mod tests;
