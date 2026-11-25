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

use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;

use super::http::{Body, Download, Redirects};
use super::{ListDownload, ListKind, request};
use crate::limits::{MAX_EXPANDED_LIST, MAX_INCLUDE_DEPTH, MAX_INCLUDES};
use crate::list_url::{include_address, include_identity, same_origin, trusted_download, valid};
use crate::rule::preprocess::{DirectiveIssue, Preprocessor};
use crate::sources::{self, Included, Info, Validator};

pub(super) async fn download<G, F>(url: &str, saved: Option<&Info>, get: &G) -> Result<ListDownload>
where
    G: Fn(String, Option<Validator>, Redirects) -> F,
    F: Future<Output = Result<Download>>,
{
    // The exact marked catalog URL is fetched without redirects. The
    // resolver only admits same-origin addresses, and the client refuses
    // every redirect for their requests, so all inserted rules share that trust.
    let trusted = trusted_download(url);
    let mut recheck = Recheck {
        previous: saved
            .and_then(|info| info.includes.as_deref())
            .unwrap_or_default(),
        checked: 0,
        changed: None,
        redirects: Redirects::Include { trusted },
    };
    let validator = saved.and_then(|info| info.validator.as_ref());
    let body = match request(
        get,
        url,
        validator,
        Redirects::List,
        ListKind::Filter,
        false,
    )
    .await?
    {
        Download::Body(body) => body,
        Download::Unchanged => {
            let resource = if trusted {
                url
            } else {
                &validator.expect("a conditional request").resource
            };
            if recheck.unchanged(resource, get).await? {
                return Ok(ListDownload::Unchanged);
            }
            full_body(get, url, Redirects::List).await?
        }
    };
    let mut resolver = Resolver {
        get,
        recheck,
        seen: [url, body.resource(url)]
            .into_iter()
            .filter_map(include_identity)
            .collect(),
        included: Vec::new(),
        state: Preprocessor::default(),
        text: String::new(),
        last_path: None,
    };
    let root = std::str::from_utf8(&body.bytes)?;
    resolver.expand(body.resource(url), root, 0).await?;
    if let Some(title) = sources::title(root)
        && sources::title(&resolver.text).as_ref() != Some(&title)
    {
        // An include in the header can move the title past the header scan.
        let title = format!("! Title: {title}\n");
        resolver.room(title.len())?;
        resolver.text.insert_str(0, &title);
    }
    Ok(ListDownload::Body(
        Bytes::from(resolver.text),
        body.validator,
        Some(resolver.included),
    ))
}

struct Recheck<'a> {
    previous: &'a [Included],
    checked: usize,
    changed: Option<(usize, Body)>,
    redirects: Redirects,
}

impl Recheck<'_> {
    async fn unchanged<G, F>(&mut self, root: &str, get: &G) -> Result<bool>
    where
        G: Fn(String, Option<Validator>, Redirects) -> F,
        F: Future<Output = Result<Download>>,
    {
        for (index, include) in self.previous.iter().enumerate() {
            let reply = async {
                ensure!(
                    valid(&include.url) && same_origin(root, &include.url),
                    "saved include is outside the list's HTTPS origin"
                );
                self.request(get, &include.url, include.validator.as_ref())
                    .await
            }
            .await
            .with_context(|| format!("include {}", include.path))?;
            if let Download::Body(body) = reply {
                self.changed = Some((index, body));
                return Ok(false);
            }
            self.checked += 1;
        }
        Ok(true)
    }

    async fn body<G, F>(&mut self, get: &G, url: &str) -> Result<Body>
    where
        G: Fn(String, Option<Validator>, Redirects) -> F,
        F: Future<Output = Result<Download>>,
    {
        if let Some((index, include)) = self.previous.iter().enumerate().find(|(_, i)| i.url == url)
        {
            if self
                .changed
                .as_ref()
                .is_some_and(|(changed, _)| *changed == index)
            {
                return Ok(self.changed.take().expect("the changed include").1);
            }
            if index >= self.checked
                && let Some(validator) = &include.validator
                && let Download::Body(body) = self.request(get, url, Some(validator)).await?
            {
                return Ok(body);
            }
        }
        full_body(get, url, self.redirects).await
    }

    async fn request<G, F>(&self, get: &G, url: &str, saved: Option<&Validator>) -> Result<Download>
    where
        G: Fn(String, Option<Validator>, Redirects) -> F,
        F: Future<Output = Result<Download>>,
    {
        request(get, url, saved, self.redirects, ListKind::Filter, false).await
    }
}

async fn full_body<G, F>(get: &G, url: &str, redirects: Redirects) -> Result<Body>
where
    G: Fn(String, Option<Validator>, Redirects) -> F,
    F: Future<Output = Result<Download>>,
{
    match request(get, url, None, redirects, ListKind::Filter, false).await? {
        Download::Body(body) => Ok(body),
        Download::Unchanged => bail!("unexpected 304 without a conditional request"),
    }
}

struct Resolver<'a, G> {
    get: &'a G,
    recheck: Recheck<'a>,
    seen: HashSet<String>,
    included: Vec<Included>,
    state: Preprocessor<()>,
    text: String,
    last_path: Option<String>,
}

impl<G, F> Resolver<'_, G>
where
    G: Fn(String, Option<Validator>, Redirects) -> F,
    F: Future<Output = Result<Download>>,
{
    async fn expand(&mut self, resource: &str, text: &str, depth: usize) -> Result<()> {
        for raw in text.split_inclusive('\n') {
            let line = raw.lines().next().unwrap_or_default();
            if self.state.read((), line) == Some(Err(DirectiveIssue::Include)) {
                let path = line.trim_start()["!#include".len()..].trim();
                if let Some(url) = include_address(resource, path) {
                    self.insert(path, url, depth + 1).await?;
                    continue;
                }
            }
            if depth != 0 && sources::title_comment(line).is_some() {
                // A fragment's title must be an ordinary comment even when
                // the list's metadata has to be rebuilt from its stored text.
                self.push("!")?;
            }
            self.push(raw)?;
        }
        Ok(())
    }

    async fn insert(&mut self, path: &str, url: String, depth: usize) -> Result<()> {
        self.last_path = Some(path.to_owned());
        if !self.seen.insert(url.clone()) {
            return self.push(&format!("! Skipped duplicate include: {path}\n"));
        }
        let body = self
            .fetch(&url, depth)
            .await
            .with_context(|| format!("include {path}"))?;
        let resource = body.resource(&url);
        let text = std::str::from_utf8(&body.bytes)?;
        let append = resource == url
            || include_identity(resource).is_none_or(|identity| self.seen.insert(identity));
        // Preorder lets revalidation stop before probing children removed
        // by a changed parent.
        self.included.push(Included {
            path: path.to_owned(),
            url: url.clone(),
            validator: body.validator.clone(),
        });
        self.push(&format!("! >>> include: {path}\n"))?;
        if append {
            // The conditions see the same sequence of directives as a
            // reader of the expanded text will see.
            Box::pin(self.expand(resource, text, depth)).await?;
            if !text.ends_with('\n') {
                self.push("\n")?;
            }
        }
        self.last_path = Some(path.to_owned());
        self.push(&format!("! <<< include: {path}\n"))
    }

    async fn fetch(&mut self, url: &str, depth: usize) -> Result<Body> {
        ensure!(
            depth <= MAX_INCLUDE_DEPTH,
            "include depth exceeds {MAX_INCLUDE_DEPTH}"
        );
        ensure!(
            self.included.len() < MAX_INCLUDES,
            "list has more than {MAX_INCLUDES} includes"
        );
        self.recheck.body(self.get, url).await
    }

    fn push(&mut self, text: &str) -> Result<()> {
        self.room(text.len())?;
        self.text.push_str(text);
        Ok(())
    }

    fn room(&self, bytes: usize) -> Result<()> {
        ensure!(
            self.text.len() + bytes <= MAX_EXPANDED_LIST,
            "include {}: expanded list exceeds the {} MiB limit",
            self.last_path.as_deref().unwrap_or_default(),
            MAX_EXPANDED_LIST >> 20
        );
        Ok(())
    }
}
