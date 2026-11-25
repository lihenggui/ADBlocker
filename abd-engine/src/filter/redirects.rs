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

//! Let one matcher resolve redirect priorities across both kinds of rules.

use std::collections::HashSet;
use std::sync::LazyLock;

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions, RuleTypes};
use adblock::request::Request;
use adblock::resources::Resource;
use adblock::utils::fast_hash;
use base64::{Engine as _, prelude::BASE64_STANDARD};
use bytes::Bytes;
use hyper::header::HeaderValue;

use super::app_patterns::Scope;
use crate::rule::{REDIRECT_EXCEPTION, Rule};

const APP_PREFIX: &str = "@abd-redirect-";
const NO_APP: &str = "@abd-redirect-no-app";

fn app_name(hash: u64) -> String {
    format!("{APP_PREFIX}{hash:x}")
}

/// The fixed resource to send in place of a blocked request's response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    pub content_type: HeaderValue,
    pub body: Bytes,
}

impl Redirect {
    fn decode(data_url: &str) -> Option<Self> {
        let (mime, data) = data_url.strip_prefix("data:")?.split_once(";base64,")?;
        Some(Self {
            content_type: HeaderValue::from_str(mime).ok()?,
            body: BASE64_STANDARD.decode(data).ok()?.into(),
        })
    }
}

pub(super) fn use_resources(engine: &mut Engine) {
    // The build-time bundle embeds 104,577 bytes of JSON for the fixed
    // substitutes. The separate matcher keeps them out of scriptlet lookup.
    static RESOURCES: LazyLock<Vec<Resource>> = LazyLock::new(|| {
        serde_json::from_str(include_str!("../../resources/redirects.json"))
            .expect("bundled redirect resources are valid")
    });
    engine.use_resources(RESOURCES.iter().cloned());
}

pub(super) fn find(
    engine: &Engine,
    req: &mut Request,
    scope: Option<&Scope<'_>>,
) -> Option<Redirect> {
    let before = req.source_hostname_hashes.as_ref().map(Vec::len);
    let hashes = req.source_hostname_hashes.get_or_insert_default();
    match scope {
        Some(scope) => hashes.extend(scope.hashes.iter().map(|&hash| fast_hash(&app_name(hash)))),
        None => hashes.push(fast_hash(NO_APP)),
    }
    let found = engine.check_network_request(req);
    if let Some(before) = before {
        req.source_hostname_hashes
            .as_mut()
            .expect("hashes inserted above")
            .truncate(before);
    } else {
        req.source_hostname_hashes = None;
    }
    if found.important {
        // A matching bare redirect exception suppresses all substitutes.
        return None;
    }
    Redirect::decode(found.redirect.as_deref()?)
}

#[derive(Default)]
pub(crate) struct RedirectProjections(String);

impl RedirectProjections {
    pub(crate) fn add_web(&mut self, source: &Rule<'_>, disabled: &HashSet<u64>) {
        // Parsing an ordinary rule a second time here is unnecessary.
        if !source.has_redirect_option() {
            return;
        }
        if source.network().is_some_and(|rule| {
            rule.is_redirect() && !rule.is_badfilter() && !disabled.contains(&rule.get_id())
        }) {
            self.add(&source.normalized(), source.redirect_exception, None);
        }
    }

    pub(super) fn add_app(&mut self, line: &str, bare: bool, apps: &str) {
        self.add(line, bare, Some(apps));
    }

    fn add(&mut self, line: &str, bare: bool, apps: Option<&str>) {
        let Some((pattern, options)) = line.rsplit_once('$') else {
            return;
        };
        let pattern = if bare {
            pattern.strip_prefix("@@").expect("bare redirect exception")
        } else {
            pattern
        };
        self.0.push_str(pattern);
        // adblock-rust rejects a valueless redirect. Its matcher can still
        // judge that exception's conditions as an important rule here.
        let mut separator = '$';
        for option in options.split(',') {
            if option == "important" {
                continue;
            }
            self.0.push(separator);
            if bare && option == REDIRECT_EXCEPTION {
                self.0.push_str("important");
            } else if let Some((name, value)) = option.split_once('=')
                && matches!(name.trim_start_matches('~'), "domain" | "from")
            {
                self.0.push_str(name);
                self.0.push('=');
                self.source_names(value, apps.is_some());
            } else {
                self.0.push_str(option);
            }
            separator = ',';
        }
        self.0.push('\n');
    }

    fn source_names(&mut self, names: &str, app: bool) {
        for (index, value) in names.split('|').enumerate() {
            if index != 0 {
                self.0.push('|');
            }
            let name = if let Some(name) = value.strip_prefix('~') {
                self.0.push('~');
                name
            } else {
                value
            };
            if app {
                self.0.push_str(&app_name(fast_hash(name)));
            } else {
                // An internal app name cannot be the hostname of a page.
                if name.starts_with(APP_PREFIX) {
                    self.0.push('@');
                }
                self.0.push_str(name);
            }
        }
        if app && names.starts_with('~') {
            self.0.push_str("|~");
            self.0.push_str(NO_APP);
        }
    }

    pub(crate) fn compile(self) -> Option<Engine> {
        if self.0.is_empty() {
            return None;
        }
        let mut set = FilterSet::new(false);
        set.add_filter_list(
            self.0,
            ParseOptions {
                rule_types: RuleTypes::NetworkOnly,
                ..ParseOptions::default()
            },
        );
        Some(Engine::new_with_filter_set(set))
    }
}
