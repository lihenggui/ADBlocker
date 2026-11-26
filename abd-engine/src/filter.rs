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

//! The compiled filter lists for host-level and request-level decisions.
//!
//! A parsed request can be judged by its URL and any source it identifies.
//! A request that names no page is taken as AdGuard takes one without a
//! referrer: a rule for named pages is not for it, block or exception, and
//! it is in the first party of what it asks for.
//! Address rules can decide a connection before it is read. A whole-host
//! block can refuse it once the name is known, unless an exception names it.

use std::borrow::Cow;
use std::collections::HashSet;
use std::net::SocketAddr;

use adblock::Engine;
use adblock::filters::network::{NetworkFilter, NetworkFilterMask as Mask};
use adblock::request::Request;
use adblock::url_parser::{RequestUrl, parse_url};
use adblock::utils::fast_hash;

pub(crate) use self::app_patterns::Patterns;
use self::app_patterns::Scope;
pub(crate) use self::app_rules::{
    AppProjections, AppRules, name as app_rule, validate as validate_app_rule,
};
pub(crate) use self::matcher::{MatcherList, MatcherLists, OriginalLines};
use self::page_exceptions::PageExceptions;
use self::response_rules::ResponseRules;
use crate::apps::Identity;
pub use crate::list_files::{ListFiles, MAX_LIST};
use crate::rule::javascript::ScriptTable;

mod address_rules;
mod app_patterns;
mod app_rules;
mod cache;
mod host_rules;
mod html_rules;
mod load;
mod matcher;
mod page_exceptions;
mod redirects;
pub(crate) mod removeparam;
mod response_rules;

pub use address_rules::AddressDecision;
pub(crate) use address_rules::AddressProjections;
pub(crate) use page_exceptions::valid_page_exception;
pub use redirects::Redirect;
pub(crate) use response_rules::validate as validate_response_rule;

struct SeenAnchors<'a>(HashSet<Cow<'a, str>>);

impl<'a> SeenAnchors<'a> {
    fn with_capacity(capacity: usize) -> Self {
        Self(HashSet::with_capacity(capacity))
    }

    fn insert(&mut self, anchor: &str, source: &'a str) -> bool {
        self.0.insert(if anchor == source {
            Cow::Borrowed(source)
        } else {
            Cow::Owned(anchor.to_owned())
        })
    }
}

pub struct Filters {
    pub generation: Option<u64>,
    requests: RequestRules,
    parameters: Engine,
    scripts: ScriptTable,
    page_exceptions: PageExceptions,
    hosts: Engine,
    /// The hosts that exceptions name.
    host_guards: Engine,
    /// The hosts that exceptions for named pages name. `None` when no
    /// list has one.
    page_host_guards: Option<Engine>,
    /// The same two kinds for the hosts that only a rule for apps blocks
    /// whole, each for the apps of that rule.
    app_host_guards: Option<Engine>,
    app_page_host_guards: Option<Engine>,
    /// The budget left guards out: no host may be refused early.
    guards_incomplete: bool,
    /// `None` when no list has a rule for an app.
    apps: Option<AppRules>,
    patterns: Patterns,
    addresses: address_rules::AddressRules,
    responses: Option<ResponseRules>,
    html: html_rules::HtmlRules,
    pub rules: usize,
    pub host_rules: usize,
}

/// The HTTPS URL parser preserves these names verbatim.
fn plain_dns_name(host: &str) -> bool {
    // A letter-only suffix leaves IP address spellings to the URL parser.
    host.len() <= 253
        && host.rsplit_once('.').is_some_and(|(_, suffix)| {
            suffix.len() >= 2 && suffix.bytes().all(|b| b.is_ascii_lowercase())
        })
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with("xn--")
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn app_package(app: &str) -> Option<&str> {
    Identity::parse(app)
        .filter(|identity| !identity.is_group())
        .map(|identity| identity.package)
}

fn domain_named(domains: &[u64], name: &str) -> bool {
    domains.binary_search(&fast_hash(name)).is_ok()
}

fn is_network_exception(rule: &NetworkFilter<'_>) -> bool {
    rule.mask.contains(Mask::IS_EXCEPTION)
        && rule.mask.intersects(Mask::FIRST_PARTY | Mask::THIRD_PARTY)
        && !rule.is_badfilter()
        && !rule.is_csp()
        && !rule.is_generic_hide()
        && !rule.is_removeparam()
}

/// What a request says of the page that made it.
#[derive(Clone, Copy)]
pub(crate) enum Page<'a> {
    Named(&'a RequestUrl),
    /// The request is for a page, which is its own.
    Itself,
    /// Not named: by an app outside a page, or by a browser that keeps the
    /// page to itself and says only whether it is of another site.
    Unnamed {
        cross_site: bool,
    },
}

impl Page<'_> {
    /// No page, as an app's own request has none.
    pub(crate) const NONE: Self = Self::Unnamed { cross_site: false };
}

/// The matcher of requests.
pub(crate) struct RequestRules {
    pub(crate) full: Engine,
    pub(crate) redirects: Option<Engine>,
    pub(crate) originals: OriginalLines,
}

impl RequestRules {
    pub(crate) fn check(&self, url: &str, source: &str, kind: &str, method: &str) -> Check {
        self.check_for(None, url, source, kind, method)
    }

    /// As [`RequestRules::check`], with the rules of `apps` for the app
    /// named with them.
    pub(crate) fn check_for(
        &self,
        apps: Option<(&AppRules, Scope<'_>)>,
        url: &str,
        source: &str,
        kind: &str,
        method: &str,
    ) -> Check {
        let parsed_source = (source != url).then(|| parse_http_page(source)).flatten();
        let page = if source == url {
            Page::Itself
        } else {
            parsed_source.as_ref().map_or(Page::NONE, Page::Named)
        };
        let Some(mut req) = Self::request_with_page(url, page, kind, method) else {
            return Check::default();
        };
        self.check_request(
            &mut req,
            apps.as_ref().map(|(rules, scope)| (*rules, scope)),
        )
    }

    /// The request for `url`, made by the page `source` if it names one. A
    /// document is its own page.
    fn request_with_page(url: &str, page: Page<'_>, kind: &str, method: &str) -> Option<Request> {
        let url = parse_url(&crate::host::canonical_http_url(url)?)?;
        let source = match page {
            Page::Itself => Some(&url).filter(|page| is_http_page(page)),
            Page::Named(source) => Some(source),
            Page::Unnamed { .. } => None,
        };
        let third_party = match (source, page) {
            (Some(source), _) => !source.domain().eq_ignore_ascii_case(url.domain()),
            // A browser says which side its page is on without naming it.
            // With no page at all there is no other party to be the third.
            (None, Page::Unnamed { cross_site }) => cross_site,
            (None, _) => false,
        };
        // No named page means there are no source hashes to add.
        let source_hostname = source.map_or("", RequestUrl::hostname);
        let hostname = url.hostname();
        let mut request = Request::preparsed(
            &url.url,
            hostname,
            source_hostname,
            kind,
            third_party,
            method,
        );
        add_lowercase_hashes(hostname, &mut request.hostname_hashes);
        add_lowercase_hashes(source_hostname, &mut request.source_hostname_hashes);
        Some(request)
    }

    fn check_request(&self, req: &mut Request, apps: Option<(&AppRules, &Scope<'_>)>) -> Check {
        let mut check = match apps {
            Some((rules, scope)) => {
                rules.check(&self.full, &rules.blocks, req, scope, Some(&self.originals))
            }
            None => {
                let found = self.full.check_network_request(req);
                Check {
                    blocked: found.should_block(),
                    rule: found.filter.and_then(|rule| self.originals.shown(rule)),
                    redirect: None,
                    rewritten_url: found.rewritten_url,
                }
            }
        };
        if check.blocked {
            check.rewritten_url = None;
        }
        if check.blocked
            && let Some(engine) = &self.redirects
        {
            check.redirect = redirects::find(engine, req, apps.map(|(_, scope)| scope));
        }
        check
    }
}

fn add_lowercase_hashes(host: &str, hashes: &mut Option<Vec<u64>>) {
    // adblock hashes ASCII names as supplied. Add the canonical spelling
    // for DNS rules such as denyallow while keeping literal list entries
    // and the original URL spelling usable.
    if host.bytes().any(|byte| byte.is_ascii_uppercase()) {
        hashes
            .get_or_insert_default()
            .extend(crate::host::suffixes(&host.to_ascii_lowercase()).map(fast_hash));
    }
}

fn is_http_page(page: &RequestUrl) -> bool {
    matches!(page.schema(), "http" | "https")
}

pub(crate) fn parse_http_page(source: &str) -> Option<RequestUrl> {
    crate::public_suffix::install();
    parse_url(&crate::host::canonical_http_url(source)?).filter(is_http_page)
}

impl Filters {
    /// The rules as they apply to `app`, which the engine names by its
    /// package or its shared UID's group. Nothing is known here of the
    /// requests it has made, so it is taken to have pages.
    pub fn for_app<'a>(&'a self, app: &'a str) -> AppFilters<'a> {
        self.for_app_with_pages(app, true)
    }

    /// As [`Filters::for_app`], for an app known to have shown a page or
    /// not (`Policy::has_shown_page`).
    pub(crate) fn for_app_with_pages<'a>(
        &'a self,
        app: &'a str,
        has_shown_page: bool,
    ) -> AppFilters<'a> {
        AppFilters {
            filters: self,
            app: Some(app),
            scope: if self.apps.is_some() || self.responses.is_some() {
                self.patterns.scope(app)
            } else {
                None
            },
            has_shown_page,
        }
    }

    /// The rules that apply whatever the app.
    fn for_any_app(&self) -> AppFilters<'_> {
        AppFilters {
            filters: self,
            app: None,
            scope: None,
            has_shown_page: true,
        }
    }

    /// [`AppFilters::blocks_host`], by the rules for every app.
    pub fn blocks_host(&self, host: &str) -> bool {
        self.for_any_app().blocks_host(host)
    }

    pub fn has_address_rules(&self) -> bool {
        !self.addresses.is_empty()
    }

    /// [`AppFilters::blocks_domain`], by the rules for every app.
    pub fn blocks_domain(&self, host: &str) -> bool {
        self.for_any_app().blocks_domain(host)
    }

    fn host_request(host: &str) -> Option<Request> {
        let authority = crate::host::canonical_authority(host, "https", 443)?;
        if authority.explicit_port {
            return None;
        }
        let host = authority.host;
        let url = format!("https://{host}/");
        if plain_dns_name(&host) {
            // Source and target are the same, without a public suffix lookup.
            Some(Request::preparsed(
                &url, &host, &host, "other", false, "GET",
            ))
        } else {
            let url = parse_url(&url)?;
            let host = url.hostname();
            Some(Request::preparsed(
                &url.url, host, host, "other", false, "GET",
            ))
        }
    }

    /// What the lists have hidden on the page at `url` (`cosmetic.rs`).
    pub fn hiding(&self, url: &str) -> Hiding {
        // Not the address of a page: nothing to hide, and nothing to
        // look through.
        let Some(url) = crate::host::canonical_http_url(url) else {
            return Hiding::default();
        };
        let Some(parsed) = parse_url(&url) else {
            return Hiding::default();
        };
        let exceptions = &self.page_exceptions;
        let page = if exceptions.all.is_some()
            || exceptions.script.is_some()
            || exceptions.element.is_some()
            || exceptions.specific.is_some()
        {
            let host = parsed.hostname();
            Some(Request::preparsed(
                &parsed.url,
                host,
                host,
                "document",
                false,
                "GET",
            ))
        } else {
            None
        };
        let excepted = |engine: Option<&Engine>| {
            page.as_ref()
                .is_some_and(|page| PageExceptions::matches(engine, page))
        };
        if excepted(exceptions.all.as_ref()) {
            return Hiding::default();
        }
        let found = self.requests.full.url_cosmetic_resources(&url);
        let no_scripts = excepted(exceptions.script.as_ref());
        let no_elements = excepted(exceptions.element.as_ref());
        let generic = exceptions
            .specific
            .as_ref()
            .and_then(|(rules, generic)| excepted(Some(rules)).then_some(generic));
        let actions = if no_elements || generic.is_some() {
            HashSet::new()
        } else {
            found.procedural_actions
        };
        let selectors = match generic {
            _ if no_elements => HashSet::new(),
            Some(_) if found.generichide => HashSet::new(),
            Some(engine) => engine.url_cosmetic_resources(&url).hide_selectors,
            None => found.hide_selectors,
        };
        Hiding {
            css: crate::cosmetic::style(selectors),
            script: crate::cosmetic::script(
                if no_scripts {
                    String::new()
                } else {
                    found.injected_script
                },
                actions,
            ),
            by_name: (!no_elements && !found.generichide).then_some(found.exceptions),
        }
    }

    /// The style for the rules that go by a class or an id, of every site,
    /// for a page on which those `classes` and `ids` are: of the thousands
    /// there are, the few that name something the page has.
    pub fn hiding_by_name(
        &self,
        hiding: &Hiding,
        classes: &HashSet<String>,
        ids: &HashSet<String>,
    ) -> Option<String> {
        let except = hiding.by_name.as_ref()?;
        crate::cosmetic::style(
            self.requests
                .full
                .hidden_class_id_selectors(classes, ids, except),
        )
    }

    /// Judges a decrypted request. `source` is the page that made it.
    pub fn check(&self, url: &str, source: &str, kind: &str, method: &str) -> Check {
        self.requests.check(url, source, kind, method)
    }
}

/// The rules as they apply to one app.
pub struct AppFilters<'a> {
    filters: &'a Filters,
    app: Option<&'a str>,
    scope: Option<Scope<'a>>,
    /// An exception for named pages can only apply to a request that shows
    /// one, and so only to an app that has.
    has_shown_page: bool,
}

/// Whether no exception among `guards` names the host of `request`.
fn unguarded(guards: &Engine, request: &Request) -> bool {
    guards
        .check_network_request_subset(request, false, true)
        .exception
        .is_none()
}

impl AppFilters<'_> {
    /// Judge the socket's destination before reading the app's traffic.
    pub fn address_decision(&self, endpoint: SocketAddr) -> AddressDecision {
        self.filters.addresses.decide(endpoint, self.app)
    }

    fn app_rules(&self) -> Option<(&AppRules, &Scope<'_>)> {
        self.filters.apps.as_ref().zip(self.scope.as_ref())
    }

    /// Before a request can be read, leave room for exceptions that name
    /// the host. Only apps that have shown a page need guards for named pages.
    pub fn blocks_host(&self, host: &str) -> bool {
        Filters::host_request(host).is_some_and(|mut request| {
            self.blocks_unguarded(&mut request)
                && (!self.has_shown_page || self.unguarded_for_pages(&mut request))
        })
    }

    fn unguarded_for_pages(&self, request: &mut Request) -> bool {
        let filters = self.filters;
        filters
            .page_host_guards
            .as_ref()
            .is_none_or(|guards| unguarded(guards, request))
            && filters.app_page_host_guards.as_ref().is_none_or(|guards| {
                self.scope
                    .as_ref()
                    .is_none_or(|scope| scope.unguarded(request, guards))
            })
    }

    /// For a connection of an app none of whose requests can be read: the
    /// block of its host stands unless an exception names that host and
    /// needs no page.
    pub fn blocks_unread(&self, host: &str) -> bool {
        Filters::host_request(host).is_some_and(|mut request| self.blocks_unguarded(&mut request))
    }

    fn blocks_unguarded(&self, request: &mut Request) -> bool {
        !self.filters.guards_incomplete
            && self.blocks_whole(request)
            && unguarded(&self.filters.host_guards, request)
            && self.app_rules().is_none_or(|(rules, scope)| {
                rules.unguarded(request, scope, self.filters.app_host_guards.as_ref())
            })
    }

    /// Domain mode uses only explicit whole-domain blocks and exceptions.
    pub fn blocks_domain(&self, host: &str) -> bool {
        Filters::host_request(host).is_some_and(|mut request| self.blocks_whole(&mut request))
    }

    fn blocks_whole(&self, request: &mut Request) -> bool {
        let hosts = &self.filters.hosts;
        match self.app_rules() {
            Some((rules, scope)) => {
                rules
                    .check(hosts, &rules.hosts, request, scope, None)
                    .blocked
            }
            None => hosts.check_network_request(request).should_block(),
        }
    }

    pub(crate) fn url_rules_disabled(&self, url: &str, page: Page<'_>, method: &str) -> bool {
        self.filters
            .page_exceptions
            .url_rules_disabled(url, page, method)
    }

    pub(crate) fn csp(
        &self,
        url: &str,
        page: Page<'_>,
        kind: &str,
        method: &str,
    ) -> Option<String> {
        if self
            .filters
            .page_exceptions
            .url_rules_disabled(url, page, method)
        {
            return None;
        }
        let requests = &self.filters.requests;
        let csp_method = match kind {
            "document" => crate::rule::csp::DOCUMENT_METHOD,
            "subdocument" => crate::rule::csp::SUBDOCUMENT_METHOD,
            _ => return None,
        };
        let mut request = RequestRules::request_with_page(url, page, kind, csp_method)?;
        let web = requests.full.get_csp_directives(&request);
        let app = self.app_rules().and_then(|(rules, scope)| {
            scope.with_source(&mut request, |req| rules.blocks.get_csp_directives(req))
        });
        match (web, app) {
            (Some(mut web), Some(app)) => {
                web.push(',');
                web.push_str(&app);
                Some(web)
            }
            (web, app) => web.or(app),
        }
    }

    pub(crate) fn parameters_for_replacement(
        &self,
        url: &str,
        page: Page<'_>,
        kind: &str,
        method: &str,
    ) -> Option<String> {
        if !url.contains('?') || !self.permits_parameter_removal(url, page, method) {
            return None;
        }
        let mut req = RequestRules::request_with_page(url, page, kind, method)?;
        let web = self
            .filters
            .parameters
            .check_network_request(&req)
            .rewritten_url;
        let app = self.app_rules().and_then(|(rules, scope)| {
            scope.with_source(&mut req, |req| {
                rules.parameters.check_network_request(req).rewritten_url
            })
        });
        match (web, app) {
            (Some(web), Some(app)) => removeparam::merge(&req.url, web, app),
            (web, app) => web.or(app),
        }
    }

    fn permits_parameter_removal(&self, url: &str, page: Page<'_>, method: &str) -> bool {
        removeparam::supported_method(method) && !self.url_rules_disabled(url, page, method)
    }

    pub(crate) fn check_with_page(
        &self,
        url: &str,
        page: Page<'_>,
        kind: &str,
        method: &str,
    ) -> Check {
        let requests = &self.filters.requests;
        let Some(mut req) = RequestRules::request_with_page(url, page, kind, method) else {
            return Check::default();
        };
        let mut check = requests.check_request(&mut req, self.app_rules());
        if check.rewritten_url.is_some() && !self.permits_parameter_removal(url, page, method) {
            check.rewritten_url = None;
        }
        check
    }
}

/// What the lists have hidden on one page.
#[derive(Default)]
pub struct Hiding {
    /// The style for what they name there outright: the rules for the
    /// site, and of those for every site the ones that go by neither a
    /// class nor an id.
    pub css: Option<String>,
    pub(crate) script: Option<String>,
    /// Whether the rules of every site that go by a class or an id apply
    /// to the page as well, and then which of them are excepted for it.
    by_name: Option<HashSet<String>>,
}

impl Hiding {
    /// Whether the page is to be looked through for its classes and ids
    /// (`Filters::hiding_by_name`).
    pub fn by_name(&self) -> bool {
        self.by_name.is_some()
    }

    /// Whether anything may come to be hidden on the page at all.
    pub fn any(&self) -> bool {
        self.css.is_some() || self.script.is_some() || self.by_name()
    }
}

#[derive(Default)]
pub struct Check {
    pub blocked: bool,
    pub redirect: Option<Redirect>,
    pub rewritten_url: Option<String>,
    /// The rule that matched, when the engine was built with debug info.
    pub rule: Option<String>,
}

#[cfg(test)]
mod matcher_contract;
#[cfg(test)]
mod tests;
