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

use anyhow::{Context, Result, anyhow, bail, ensure};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::Request;
use hyper::header::{
    ACCEPT_ENCODING, ETAG, HOST, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, LOCATION,
    USER_AGENT,
};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::MAX_SIZE;
use crate::list_url::{same_origin, split, trusted_download, valid};
use crate::sources::Validator;

const MAX_REDIRECTS: usize = 3;

pub(super) enum Download {
    Body(Body),
    Unchanged,
}

pub(super) struct Body {
    pub(super) bytes: Bytes,
    pub(super) validator: Option<Validator>,
    redirected_to: Option<String>,
}

impl Body {
    pub(super) fn new(bytes: Bytes, validator: Option<Validator>) -> Self {
        Self {
            bytes,
            validator,
            redirected_to: None,
        }
    }

    pub(super) fn resource<'a>(&'a self, requested: &'a str) -> &'a str {
        self.redirected_to.as_deref().unwrap_or(requested)
    }
}

#[derive(Clone, Copy)]
pub(super) enum Redirects {
    List,
    Include { trusted: bool },
}

pub(super) fn conditional(
    url: &str,
    saved: Option<&Validator>,
) -> Option<(hyper::header::HeaderName, hyper::header::HeaderValue)> {
    let saved = saved.filter(|v| v.resource == url)?;
    let (name, value) = match (&saved.tag, &saved.modified) {
        (Some(tag), _) => (IF_NONE_MATCH, tag),
        (None, Some(date)) => (IF_MODIFIED_SINCE, date),
        _ => return None,
    };
    Some((name, hyper::header::HeaderValue::from_str(value).ok()?))
}

pub(super) fn validator(url: &str, headers: &hyper::HeaderMap) -> Option<Validator> {
    let value = |name| {
        let s = headers
            .get::<hyper::header::HeaderName>(name)?
            .to_str()
            .ok()?;
        (!s.is_empty() && s.len() <= 512).then(|| s.to_owned())
    };
    let tag = value(ETAG).filter(|s| {
        let tag = s.strip_prefix("W/").unwrap_or(s);
        tag.len() >= 2
            && tag.starts_with('"')
            && tag.ends_with('"')
            && tag[1..tag.len() - 1]
                .bytes()
                .all(|b| b >= 0x21 && b != b'"')
    });
    let modified = value(LAST_MODIFIED);
    ((tag.is_some() || modified.is_some()) && url.len() <= 4096).then(|| Validator {
        resource: url.to_owned(),
        tag,
        modified,
        file: [0; 5],
    })
}

async fn get_once(
    url: &str,
    tls: &TlsConnector,
    saved: Option<&Validator>,
) -> Result<Result<Download, String>> {
    let (host, port, path) = split(url)?;
    let tcp = TcpStream::connect((host, port)).await?;
    let stream = tls
        .connect(ServerName::try_from(host.to_owned())?, tcp)
        .await?;
    let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    tokio::spawn(conn);
    let condition = conditional(url, saved);
    let mut req = Request::get(path)
        .header(HOST, host)
        .header(USER_AGENT, "abd")
        .header(ACCEPT_ENCODING, "identity");
    if let Some((name, value)) = &condition {
        req = req.header(name, value);
    }
    let req = req.body(Empty::<Bytes>::new())?;
    let resp = tx.send_request(req).await?;
    if resp.status() == hyper::StatusCode::NOT_MODIFIED {
        ensure!(
            condition.is_some(),
            "unexpected 304 without a conditional request"
        );
        return Ok(Ok(Download::Unchanged));
    }
    if resp.status().is_redirection() {
        let to = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok());
        return Ok(Err(to.context("redirect without a location")?.to_owned()));
    }
    ensure!(
        resp.status().is_success(),
        "{} answered {}",
        host,
        resp.status()
    );
    let saved = validator(url, resp.headers());
    let body = Limited::new(resp.into_body(), MAX_SIZE)
        .collect()
        .await
        .map_err(|e| anyhow!("reading the list: {e}"))?;
    Ok(Ok(Download::Body(Body::new(body.to_bytes(), saved))))
}

pub(super) async fn get(
    url: &str,
    tls: &TlsConnector,
    saved: Option<&Validator>,
    redirects: Redirects,
) -> Result<Download> {
    let mut resource = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        match get_once(&resource, tls, saved).await? {
            Ok(Download::Body(mut body)) => {
                body.redirected_to = (resource != url).then_some(resource);
                return Ok(Download::Body(body));
            }
            Ok(Download::Unchanged) => return Ok(Download::Unchanged),
            Err(_) if matches!(redirects, Redirects::List) && trusted_download(url) => {
                bail!("a trusted list URL cannot redirect");
            }
            Err(_) if matches!(redirects, Redirects::Include { trusted: true }) => {
                bail!("an include of a trusted list cannot redirect");
            }
            Err(to) => {
                if matches!(redirects, Redirects::Include { .. }) {
                    ensure!(
                        valid(&to) && same_origin(url, &to),
                        "an include cannot redirect outside its HTTPS origin"
                    );
                }
                resource = to;
            }
        }
    }
    bail!("too many redirects")
}
