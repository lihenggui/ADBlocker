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
//! For a connection that stays encrypted, only an unconditional block with
//! no possible network exception may be applied.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::Path;

use adblock::Engine;
use adblock::filters::network::{NetworkFilter, NetworkFilterMask as Mask};
use adblock::lists::{FilterSet, ParseOptions, ParsedLine, RuleTypes, parse_filter};
use adblock::request::Request;
use adblock::resources::{PermissionMask, Resource};
use adblock::url_parser::{RequestUrl, parse_url};
use anyhow::Result;

use self::host_rules::{HostRules, host_only};
use self::page_exceptions::PageExceptions;
pub use crate::list_files::{ListFiles, MAX_LIST};
use crate::list_files::{exceeds_budget, read_lists, readable_paths};

mod cache;
mod host_rules;
mod page_exceptions;

pub struct Filters {
    pub generation: Option<u64>,
    requests: RequestRules,
    page_exceptions: PageExceptions,
    hosts: Engine,
    /// `None` when the lists allow a pattern on potentially any host.
    host_guards: Option<Engine>,
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

impl ListFiles {
    pub fn filters(&self, cache: Option<&Path>, debug: bool) -> Result<Filters> {
        let cache = cache.filter(|_| !debug);
        if let Some(kept) = cache.and_then(|dir| Filters::read_cache(dir, self.generation)) {
            return Ok(kept);
        }
        let built = Filters::build(read_lists(&self.paths), Some(self.generation), debug)?;
        if let Some(dir) = cache {
            built.write_cache(dir, self.generation);
        }
        Ok(built)
    }
}

fn opts(line: &str) -> impl Iterator<Item = &str> {
    line.rsplit_once('$')
        .into_iter()
        .flat_map(|(_, options)| options.split(','))
}

fn option_name(option: &str) -> &str {
    let name = option.split_once('=').map_or(option, |(name, _)| name);
    name.trim_start_matches('~')
}

fn source_option(option: &str) -> bool {
    matches!(
        option_name(option),
        "domain" | "from" | "first-party" | "1p" | "third-party" | "3p"
    )
}

fn to_domains(line: &str) -> impl Iterator<Item = &str> {
    opts(line)
        .filter(|option| option_name(option) == "to")
        .filter_map(|option| option.split_once('=').map(|(_, domains)| domains))
        .flat_map(|domains| domains.split('|'))
}

fn domain_named(domains: &[u64], name: &str) -> bool {
    domains
        .binary_search(&adblock::utils::fast_hash(name))
        .is_ok()
}

fn project_exception(out: &mut String, line: &str, rule: &NetworkFilter<'_>) {
    let Some((pattern, _)) = line.rsplit_once('$') else {
        return;
    };
    out.push_str(pattern);
    let mut separator = '$';
    for option in opts(line).filter(|option| !source_option(option) && option_name(option) != "to")
    {
        out.push(separator);
        out.push_str(option);
        separator = ',';
    }
    // adblock-rust may have indexed broad or exclusion-only destinations
    // by source. Re-index them by the request's destination hashes so
    // removing the unknown source condition cannot drop the exception.
    let mut names = to_domains(line).filter(|entry| {
        let (name, domains) = entry
            .strip_prefix('~')
            .map_or((*entry, rule.opt_to_domains.as_deref()), |name| {
                (name, rule.opt_not_to_domains.as_deref())
            });
        domains.is_some_and(|domains| domain_named(domains, name))
    });
    if let Some(first) = names.next() {
        out.push(separator);
        out.push_str("from=");
        out.push_str(first);
        for name in names {
            out.push('|');
            out.push_str(name);
        }
    }
    out.push('\n');
}

fn network(line: &str) -> Option<NetworkFilter<'_>> {
    let options = ParseOptions {
        rule_types: RuleTypes::NetworkOnly,
        ..ParseOptions::default()
    };
    match parse_filter(line, false, options).ok()? {
        ParsedLine::Network(rule) => Some(rule),
        _ => None,
    }
}

pub(crate) fn network_line(line: &str) -> Cow<'_, str> {
    let line = line.trim();
    const HOSTS_IPS: &[&str] = &["127.0.0.1", "0.0.0.0"];
    if HOSTS_IPS.iter().any(|ip| {
        line.strip_prefix(ip)
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_whitespace()))
    }) && let Some(rule) = host_only(line)
    {
        return Cow::Owned(rule);
    }
    Cow::Borrowed(line)
}

fn badfilters<'a>(lists: impl IntoIterator<Item = &'a str>) -> HashSet<u64> {
    lists
        .into_iter()
        .flat_map(|text| text.lines().map(str::trim))
        .filter(|line| opts(line).any(|option| option == "badfilter"))
        .filter_map(network)
        .filter(NetworkFilter::is_badfilter)
        .map(|rule| rule.get_id())
        .collect()
}

fn is_network_exception(rule: &NetworkFilter<'_>) -> bool {
    rule.mask.contains(Mask::IS_EXCEPTION)
        && rule.mask.intersects(Mask::FIRST_PARTY | Mask::THIRD_PARTY)
        && !rule.is_badfilter()
        && !rule.is_csp()
        && !rule.is_generic_hide()
        && !rule.is_removeparam()
}

/// The full matcher and, if any rules depend on a source, the matcher for
/// requests without one. A missing source cannot satisfy a block's
/// source condition. It may satisfy either side of an exception, so its
/// independently matchable destination, URL, type and method still apply.
pub(crate) struct RequestRules {
    pub(crate) full: Engine,
    pub(crate) unknown: Option<Engine>,
}

impl RequestRules {
    pub(crate) fn build(lists: Vec<String>, debug: bool, options: ParseOptions) -> Self {
        let disabled = badfilters(lists.iter().map(String::as_str));
        let sources = lists
            .iter()
            .map(|text| (text.as_str(), options.permissions));
        Self::build_with_badfilters(sources, debug, options, &disabled)
    }

    fn build_with_badfilters<'a>(
        lists: impl Iterator<Item = (&'a str, PermissionMask)> + Clone,
        debug: bool,
        options: ParseOptions,
        disabled: &HashSet<u64>,
    ) -> Self {
        let has_context = lists.clone().any(|(text, _)| {
            text.lines().any(|line| {
                let line = line.trim();
                opts(line).any(source_option) && network(line).is_some()
            })
        });
        let mut full = FilterSet::new(debug);
        let mut unknown = FilterSet::new(debug);
        let unknown_options = ParseOptions {
            rule_types: RuleTypes::NetworkOnly,
            ..options
        };
        for (text, permissions) in lists {
            let mut normalized = String::with_capacity(text.len());
            let mut projected = String::new();
            for line in text.lines() {
                let line = network_line(line);
                // Reject unsafe calls before their helpers can consume
                // the page's script budget. This also sees raw controls
                // before adblock-rust serializes the arguments.
                if line.split_once('#').is_some_and(|(_, suffix)| {
                    suffix.starts_with("#+js(") || suffix.starts_with("?#+js(")
                }) && !crate::cosmetic::safe_raw_text(&line)
                {
                    continue;
                }
                normalized.push_str(&line);
                normalized.push('\n');
                if has_context
                    && let Some(rule) = network(&line)
                    && !rule.is_badfilter()
                    && !disabled.contains(&rule.get_id())
                {
                    let needs_source = rule.opt_domains.is_some()
                        || rule.opt_not_domains.is_some()
                        || !rule.mask.contains(Mask::FIRST_PARTY | Mask::THIRD_PARTY);
                    if !needs_source {
                        projected.push_str(&line);
                        projected.push('\n');
                    } else if is_network_exception(&rule) {
                        project_exception(&mut projected, &line, &rule);
                    }
                }
            }
            full.add_filter_list(
                normalized,
                ParseOptions {
                    permissions,
                    ..options
                },
            );
            if has_context {
                unknown.add_filter_list(projected, unknown_options);
            }
        }
        Self {
            full: Engine::new_with_filter_set(full),
            unknown: has_context.then(|| Engine::new_with_filter_set(unknown)),
        }
    }

    pub(crate) fn check(&self, url: &str, source: &str, kind: &str, method: &str) -> Check {
        let known = parse_http_page(source).is_some();
        let source = if known { source } else { "" };
        let Ok(req) = Request::new(url, source, kind, method) else {
            return Check::default();
        };
        self.check_request(req, known)
    }

    fn check_with_page(
        &self,
        url: &str,
        source: Option<&RequestUrl>,
        document: bool,
        kind: &str,
        method: &str,
    ) -> Check {
        let Some(url) = parse_url(url) else {
            return Check::default();
        };
        let source = if document {
            Some(&url).filter(|page| is_http_page(page))
        } else {
            source
        };
        let req = Request::preparsed(
            &url.url,
            url.hostname(),
            source.map_or("", RequestUrl::hostname),
            kind,
            source.is_none_or(|source| source.domain() != url.domain()),
            method,
        );
        self.check_request(req, source.is_some())
    }

    fn check_request(&self, mut req: Request, known: bool) -> Check {
        let engine = if let (false, Some(unknown)) = (known, &self.unknown) {
            // Only projected destination conditions use this index in
            // the unknown-source engine.
            req.source_hostname_hashes.clone_from(&req.hostname_hashes);
            unknown
        } else {
            &self.full
        };
        let r = engine.check_network_request(&req);
        Check {
            blocked: r.should_block(),
            rule: r.filter.and_then(|f| f.raw_line),
        }
    }
}

fn is_http_page(page: &RequestUrl) -> bool {
    matches!(page.schema(), "http" | "https")
}

pub(crate) fn parse_http_page(source: &str) -> Option<RequestUrl> {
    parse_url(source).filter(is_http_page)
}

fn use_scriptlets(engine: &mut Engine) {
    static RESOURCES: std::sync::LazyLock<Vec<Resource>> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../resources/scriptlets.json"))
            .expect("bundled scriptlet resources are valid")
    });
    engine.use_resources(RESOURCES.iter().cloned());
}

impl Filters {
    /// Loads every `*.txt` list in `dirs`. With `debug` the engine keeps the
    /// text of each rule, to name the one that matched; it costs memory.
    pub fn load(dirs: &[&Path], debug: bool) -> Result<Self> {
        Self::open(dirs, None, debug)
    }

    /// As [`Filters::load`], but from what was kept in `cache` when these
    /// same lists were loaded before, and keeping it there otherwise.
    ///
    /// Reading the lists is a tenth of a second of one core at full work
    /// and tens of megabytes passed through memory, at every start and
    /// every reload, for lists that change once a day at most.
    pub fn load_cached(dirs: &[&Path], cache: &Path) -> Result<Self> {
        Self::open(dirs, Some(cache), false)
    }

    fn open(dirs: &[&Path], cache: Option<&Path>, debug: bool) -> Result<Self> {
        let load = || ListFiles::read(dirs)?.filters(cache, debug);
        match load() {
            Ok(filters) => Ok(filters),
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(exceeds_budget) =>
            {
                Err(e)
            }
            Err(e) => {
                // No previous engine at startup: use the readable lists.
                // An incomplete generation must never enter the disk cache.
                eprintln!("lists: loading readable lists only: {e}");
                let paths = readable_paths(dirs);
                Self::build(
                    read_lists(&paths)
                        .filter(|entry| entry.as_ref().err().is_none_or(exceeds_budget)),
                    None,
                    debug,
                )
            }
        }
    }

    fn build<'a>(
        entries: impl Iterator<Item = std::io::Result<(&'a Path, String)>>,
        generation: Option<u64>,
        debug: bool,
    ) -> Result<Self> {
        let lists = entries.collect::<std::io::Result<Vec<_>>>()?;
        let rules_count = lists.iter().map(|(_, text)| text.lines().count()).sum();
        let texts = || lists.iter().map(|(_, text)| text.as_str());
        let disabled = badfilters(texts());
        let host_rules = HostRules::project(texts(), &disabled);
        let mut requests = RequestRules::build_with_badfilters(
            lists.iter().map(|(path, text)| {
                let permissions =
                    PermissionMask::from_bits(u8::from(crate::list_url::trusted_list_file(path)));
                (text.as_str(), permissions)
            }),
            debug,
            ParseOptions::default(),
            &disabled,
        );
        use_scriptlets(&mut requests.full);
        Ok(Self {
            generation,
            requests,
            page_exceptions: PageExceptions::build(texts()),
            hosts: Engine::new_with_filter_set(host_rules.whole),
            host_guards: host_rules.guards.map(Engine::new_with_filter_set),
            rules: rules_count,
            host_rules: host_rules.count,
        })
    }

    /// A full-mode early block must leave room for any web exception.
    pub fn blocks_host(&self, host: &str) -> bool {
        let Some(guards) = &self.host_guards else {
            return false;
        };
        Self::host_request(host).is_some_and(|request| {
            self.hosts.check_network_request(&request).should_block()
                && guards
                    .check_network_request_subset(&request, false, true)
                    .exception
                    .is_none()
        })
    }

    /// Domain mode uses only explicit whole-domain blocks and exceptions.
    pub fn blocks_domain(&self, host: &str) -> bool {
        Self::host_request(host)
            .is_some_and(|request| self.hosts.check_network_request(&request).should_block())
    }

    fn host_request(host: &str) -> Option<Request> {
        let url = format!("https://{host}/");
        if plain_dns_name(host) {
            // Source and target are the same, without a public suffix lookup.
            Some(Request::preparsed(&url, host, host, "other", false, "GET"))
        } else {
            Request::new(&url, &url, "other", "GET").ok()
        }
    }

    /// What the lists have hidden on the page at `url` (`cosmetic.rs`).
    pub fn hiding(&self, url: &str) -> Hiding {
        // Not the address of a page: nothing to hide, and nothing to
        // look through.
        let Ok(page) = Request::new(url, url, "document", "GET") else {
            return Hiding::default();
        };
        if PageExceptions::matches(self.page_exceptions.all.as_ref(), &page) {
            return Hiding::default();
        }
        let found = self.requests.full.url_cosmetic_resources(url);
        let no_elements = PageExceptions::matches(self.page_exceptions.element.as_ref(), &page);
        let generic = self
            .page_exceptions
            .specific
            .as_ref()
            .and_then(|(rules, generic)| {
                PageExceptions::matches(Some(rules), &page).then_some(generic)
            });
        let actions = if no_elements || generic.is_some() {
            HashSet::new()
        } else {
            found.procedural_actions
        };
        let selectors = match generic {
            _ if no_elements => HashSet::new(),
            Some(_) if found.generichide => HashSet::new(),
            Some(engine) => engine.url_cosmetic_resources(url).hide_selectors,
            None => found.hide_selectors,
        };
        Hiding {
            css: crate::cosmetic::style(selectors),
            script: crate::cosmetic::script(found.injected_script, actions),
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

    pub(crate) fn check_with_page(
        &self,
        url: &str,
        source: Option<&RequestUrl>,
        document: bool,
        kind: &str,
        method: &str,
    ) -> Check {
        self.requests
            .check_with_page(url, source, document, kind, method)
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
    /// The rule that matched, when the engine was built with debug info.
    pub rule: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filters(rules: &str) -> Filters {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("list.txt"), rules).unwrap();
        Filters::load(&[dir.path()], true).unwrap()
    }

    const HIDING: &str = "site.example##.ad\n\
                          site.example,other.example##a[href^=\"https://ads.example/\"]\n\
                          other.example##.banner\n\
                          ##div[data-ad]\n\
                          ##.by-class\n\
                          excepted.example#@#div[data-ad]\n";

    #[test]
    fn scriptlets_and_procedural_rules_respect_cosmetic_exceptions() {
        let base = "site.test##+js(set-constant, adbFixture, 42)\nsite.test##.ad\nsite.test##div:has-text(Advert)\n##.generic\n";
        let active = filters(base).hiding("https://site.test/");
        let script = active.script.unwrap();
        assert!(
            script.contains(r#"setConstant("adbFixture", "42")"#),
            "{script}"
        );
        assert!(script.contains(r#"{"type":"has-text","arg":"Advert"}"#));
        for (exception, scriptlet, procedural, specific, generic) in [
            ("site.test#@#+js()", false, true, true, true),
            (
                "site.test#@#+js(set-constant, adbFixture, 42)",
                false,
                true,
                true,
                true,
            ),
            ("site.test#@#div:has-text(Advert)", true, false, true, true),
            ("site.test#@#.ad", true, true, false, true),
            ("@@||site.test^$generichide", true, true, true, false),
            ("@@||site.test^$ghide", true, true, true, false),
            ("@@||site.test^$specifichide", true, false, false, true),
            ("@@||site.test^$shide", true, false, false, true),
            ("@@||site.test^$elemhide", true, false, false, false),
            ("@@||site.test^$ehide", true, false, false, false),
            ("@@||site.test^$shide,ghide", true, false, false, false),
            ("@@||site.test^$document", false, false, false, false),
            ("@@||site.test^$ehide,doc", false, false, false, false),
            ("@@||site.test^$shide,ehide", true, false, false, false),
            ("@@||site.test/allowed$document", true, true, true, true),
        ] {
            let filters = filters(&format!("{base}{exception}\n"));
            let found = filters.hiding("https://site.test/");
            assert_eq!(
                found.script.is_some(),
                scriptlet || procedural,
                "{exception}"
            );
            let script = found.script.as_deref().unwrap_or_default();
            assert_eq!(script.contains("adbFixture"), scriptlet, "{exception}");
            assert_eq!(
                script.contains(r#"arg":"Advert""#),
                procedural,
                "{exception}"
            );
            assert_eq!(found.css.is_some(), specific, "{exception}");
            assert_eq!(found.by_name(), generic, "{exception}");
        }
    }

    #[test]
    fn only_a_fresh_catalog_list_can_invoke_trusted_scriptlets_even_after_cache_reload() {
        let dir = tempfile::tempdir().unwrap();
        let lists = dir.path().join("lists");
        let rules = dir.path().join("rules");
        let cache = dir.path().join("cache");
        std::fs::create_dir(&lists).unwrap();
        std::fs::create_dir(&rules).unwrap();
        let stock =
            "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/filters.txt";
        let name = crate::list_url::file_name(stock);
        let custom = crate::list_url::file_name(&format!("{stock}?custom"));
        for (path, host) in [
            (lists.join(custom), "custom.test"),
            (rules.join(&name), "user.test"),
            (lists.join(name), "catalog.test"),
        ] {
            std::fs::write(
                path,
                format!("{host}##+js(trusted-set, abdTrusted, json:42)\n"),
            )
            .unwrap();
        }
        let dirs = [&lists as &Path, &rules as &Path];
        let built = Filters::load_cached(&dirs, &cache).unwrap();
        let kept = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
        for filters in [built, kept] {
            for host in ["custom.test", "user.test"] {
                assert!(
                    filters.hiding(&format!("https://{host}/")).script.is_none(),
                    "{host}"
                );
            }
            let script = filters.hiding("https://catalog.test/").script.unwrap();
            assert!(script.contains(r#"trustedSetConstant("abdTrusted", "json:42")"#));
        }
    }

    #[test]
    fn unsafe_scriptlet_arguments_leave_other_lists_scriptlets_active() {
        let dir = tempfile::tempdir().unwrap();
        let safe = "site.test##+js(set-constant, keptFixture, 42)\nsite.test##.ad\n";
        let unsafe_rules = "site.test##+js(set-constant, endTag, '</sCrIpT><script>arbitrary()</script>')\n\
            site.test##+js(set-constant, comment, '<!--')\n\
            site.test##+js(set-constant, nulByte, 'a\0b')\n\
            site.test##+js(set-constant, carriageReturn, 'a\rb')\n";
        std::fs::write(dir.path().join("safe.txt"), safe).unwrap();
        std::fs::write(dir.path().join("unsafe.txt"), unsafe_rules).unwrap();
        let found = Filters::load(&[dir.path()], true)
            .unwrap()
            .hiding("https://site.test/");
        let script = found.script.unwrap();
        assert!(script.contains(r#"setConstant("keptFixture", "42")"#));
        for skipped in [
            "endTag",
            "arbitrary()",
            "comment",
            "nulByte",
            "carriageReturn",
        ] {
            assert!(!script.contains(&format!("\"{skipped}\"")), "{skipped}");
        }
        assert!(!script.to_ascii_lowercase().contains("</script"));
        assert!(!script.contains("<!--"));
        assert!(found.css.is_some());

        let found = filters(unsafe_rules).hiding("https://site.test/");
        assert!(found.script.is_none());
    }

    #[test]
    fn helpers_for_a_rejected_call_do_not_consume_the_script_budget() {
        let url = "https://site.test/";
        let small = "site.test##+js(set-constant, keptFixture, 42";
        let base = filters(&format!("{small})")).hiding(url).script.unwrap();
        let padding = "x".repeat((256 << 10) - base.len());
        let valid = format!("{small}{padding})");
        let expected = filters(&valid).hiding(url).script.unwrap();
        assert_eq!(expected.len(), 256 << 10);
        let found = filters(&format!("{valid}\nsite.test##+js(prevent-fetch, '<!--')")).hiding(url);
        assert_eq!(found.script, Some(expected));
    }

    #[test]
    fn hides_on_a_page_what_the_lists_name_there() {
        let f = filters(HIDING);
        assert_eq!(
            f.hiding("https://www.site.example/news/1").css.unwrap(),
            ".ad{display:none!important}\n\
             a[href^=\"https://ads.example/\"]{display:none!important}\n\
             div[data-ad]{display:none!important}\n"
        );
        // Of the rules for every site, not the ones that go by a class:
        // those wait for the classes of the page to be known.
        assert_eq!(
            f.hiding("https://elsewhere.example/").css.unwrap(),
            "div[data-ad]{display:none!important}\n"
        );
        assert_eq!(f.hiding("https://excepted.example/").css, None);
        assert_eq!(f.hiding("not an address").css, None);
        assert!(!f.hiding("not an address").any());
        let none = filters("||ads.example^\n").hiding("https://site.example/");
        assert_eq!(none.css, None);
    }

    #[test]
    fn hides_by_class_and_id_what_a_page_is_found_to_have() {
        let f = filters(
            "##.by-class\n\
             ###by-id\n\
             ##.not-on-the-page\n\
             excepted.example#@#.by-class\n\
             @@||quiet.example^$generichide\n",
        );
        let names =
            |names: &[&str]| -> HashSet<String> { names.iter().map(|n| (*n).to_owned()).collect() };
        let (classes, ids) = (names(&["by-class", "content"]), names(&["by-id", "main"]));
        let by_name = |url: &str| {
            let hiding = f.hiding(url);
            f.hiding_by_name(&hiding, &classes, &ids)
        };
        assert_eq!(
            by_name("https://site.example/").unwrap(),
            "#by-id{display:none!important}\n.by-class{display:none!important}\n"
        );
        // A rule the lists take back for one site.
        assert_eq!(
            by_name("https://excepted.example/").unwrap(),
            "#by-id{display:none!important}\n"
        );
        // A site excepted from every such rule is not looked through.
        assert!(!f.hiding("https://quiet.example/").by_name());
        assert_eq!(by_name("https://quiet.example/"), None);
        // Nothing the page has is named.
        let hiding = f.hiding("https://site.example/");
        assert_eq!(
            f.hiding_by_name(&hiding, &names(&["content"]), &names(&[])),
            None
        );
    }

    #[test]
    fn what_is_hidden_is_kept_with_the_lists_in_the_cache() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(dir.path().join("list.txt"), HIDING).unwrap();
        let load = || Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        let (built, kept) = (load(), load());
        let url = "https://site.example/";
        assert!(built.hiding(url).css.is_some());
        assert_eq!(built.hiding(url).css, kept.hiding(url).css);
        let (classes, none) = (HashSet::from(["by-class".to_owned()]), HashSet::new());
        let by_name = |f: &Filters| f.hiding_by_name(&f.hiding(url), &classes, &none);
        assert!(by_name(&built).is_some());
        assert_eq!(by_name(&built), by_name(&kept));
    }

    #[test]
    fn element_hiding_exceptions_keep_scriptlets_after_cache_reload() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(
            dir.path().join("list.txt"),
            "site.test##+js(set-constant, adbFixture, 42)\nsite.test##.ad\n\
             site.test##div:has-text(Advert)\n##.generic\n@@||site.test^$ehide\n",
        )
        .unwrap();
        let built = Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        let kept = Filters::read_cache(cache.path(), built.generation.unwrap()).unwrap();
        for filters in [built, kept] {
            let hiding = filters.hiding("https://site.test/");
            assert!(hiding.css.is_none());
            assert!(!hiding.by_name());
            let script = hiding.script.unwrap();
            assert!(script.contains("adbFixture"));
            assert!(!script.contains("Advert"));
        }
    }

    #[test]
    fn keeps_only_rules_that_block_a_whole_host() {
        assert_eq!(
            host_only("||ads.example.com^").as_deref(),
            Some("||ads.example.com^")
        );
        assert_eq!(
            host_only("0.0.0.0 ads.example.com").as_deref(),
            Some("||ads.example.com^")
        );
        assert_eq!(host_only("||example.com^$third-party"), None);
        assert_eq!(host_only("||example.com/ads^"), None);
        assert_eq!(
            host_only("@@||example.com^").as_deref(),
            Some("@@||example.com^")
        );
        assert_eq!(
            host_only("||example.com^$badfilter").as_deref(),
            Some("||example.com^$badfilter")
        );
        assert_eq!(host_only("@@||example.com^$script"), None);
        assert_eq!(host_only("127.0.0.1 localhost"), None);
        assert_eq!(host_only("! a comment"), None);
        // Wider than anything anybody registered.
        assert_eq!(host_only("||com^"), None);
        assert_eq!(host_only("||co.uk^"), None);
        assert_eq!(host_only("0.0.0.0 uk"), None);
        assert_eq!(host_only("||^"), None);
        assert_eq!(
            host_only("||ads.example.co.uk^").as_deref(),
            Some("||ads.example.co.uk^")
        );
        assert_eq!(host_only("example.com##.ad"), None);
    }

    #[test]
    fn a_host_is_blocked_whole_only_by_a_rule_that_says_so() {
        let f = filters("||ads.example^\n||shop.example^$third-party\n||news.example/banner\n");
        assert!(f.blocks_host("ads.example"));
        assert!(f.blocks_host("cdn.ads.example"));
        assert!(!f.blocks_host("shop.example"));
        assert!(!f.blocks_host("news.example"));
        assert!(!f.blocks_host("notads.example"));
    }

    #[test]
    fn a_host_the_lists_take_back_is_not_blocked_whole() {
        let f = filters(
            "||ads.example^\n@@||ads.example^\n||cdn.example^\n||cdn.example^$badfilter\n||left.example^\n",
        );
        assert!(!f.blocks_host("ads.example"));
        assert!(!f.blocks_host("cdn.example"));
        assert!(f.blocks_host("left.example"));
    }

    #[test]
    fn the_host_lookup_matches_a_fully_parsed_request() {
        let f = filters(
            "||ads.example^\n\
             @@||safe.ads.example^\n\
             ||disabled.example^\n\
             ||disabled.example^$badfilter\n\
             0.0.0.0 static.example.co.uk\n\
             ||xn--bcher-kva.example^\n\
             ||shop.example^$third-party\n\
             ||news.example/banner\n",
        );
        let longest = [
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(61),
        ]
        .join(".");
        for (host, plain) in [
            ("ads.example", true),
            ("7.eu-west-1.ads.example", true),
            ("safe.ads.example", true),
            ("child.safe.ads.example", true),
            ("disabled.example", true),
            ("static.example.co.uk", true),
            ("shop.example", true),
            ("news.example", true),
            ("notads.example", true),
            (&format!("{}.ads.example", "a".repeat(63)), true),
            (&longest, true),
            (&format!("{longest}d"), false),
            (&format!("{}.ads.example", "a".repeat(64)), false),
            ("-x.ads.example", false),
            ("x-.ads.example", false),
            ("ads..example", false),
            ("ads.example.", false),
            ("ADS.example", false),
            ("local", false),
            ("", false),
            ("127.0.0.1", false),
            ("0x7f.0x1", false),
            ("ads.123", false),
            ("[2001:db8::1]", false),
            ("bücher.example", false),
            ("xn--bcher-kva.example", false),
            ("ads\u{3002}example", false),
            ("ad%73.example", false),
            ("x_y.ads.example", false),
            ("ads.example:8443", false),
            ("user@ads.example", false),
            ("ads.example/path", false),
            ("ads.example?query", false),
            ("ads.example#fragment", false),
            ("\tads.example", false),
        ] {
            assert_eq!(plain_dns_name(host), plain, "{host:?}");
            let url = format!("https://{host}/");
            let parsed = Request::new(&url, &url, "other", "GET");
            if plain {
                let request = parsed.as_ref().unwrap();
                assert_eq!(request.hostname, host);
                assert_eq!(request.url, url);
                assert!(!request.is_third_party);
            }
            let expected = parsed.is_ok_and(|request| {
                let found = f.hosts.check_network_request(&request);
                found.filter.is_some() && found.exception.is_none()
            });
            assert_eq!(f.blocks_host(host), expected, "{host:?}");
        }
    }

    #[test]
    fn web_exceptions_defer_full_host_checks_across_files_and_cached_generations() {
        let (lists, local, cache) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        std::fs::write(lists.path().join("list.txt"),
            "||ads.example^\n||media.example^\n||scoped.example^\n||unrelated.example^\n0.0.0.0 static.example\n"
        ).unwrap();
        std::fs::write(
            local.path().join("local.txt"),
            "@@||ads.example/allowed^$script,third-party,domain=reader.example\n\
             @@|https://media.example/play$media\n\
             @@/help^$to=scoped.example\n\
             @@||static.example/allowed^\n\
             @@||unrelated.example/test^$script,badfilter\n\
             @@*$generichide\n",
        )
        .unwrap();
        let dirs = [lists.path(), local.path()];
        let _ = Filters::load_cached(&dirs, cache.path()).unwrap();
        let list_files = ListFiles::read(&dirs).unwrap();
        for f in [
            Filters::load(&dirs, true).unwrap(),
            Filters::read_cache(cache.path(), list_files.generation).unwrap(),
        ] {
            for host in [
                "ads.example",
                "media.example",
                "scoped.example",
                "static.example",
            ] {
                assert!(!f.blocks_host(host), "{host}");
                assert!(f.blocks_domain(host), "{host}");
            }
            assert!(f.blocks_host("unrelated.example"));
            let page = "https://reader.example/";
            let check = |url, kind| f.check(url, page, kind, "GET").blocked;
            assert!(!check("https://ads.example/allowed/one.js", "script"));
            assert!(check("https://ads.example/allowed/one.png", "image"));
            assert!(check("https://ads.example/blocked.js", "script"));
            assert!(!check("https://media.example/play", "media"));
            assert!(!check("https://scoped.example/help", "other"));
            assert!(check("https://static.example/blocked.js", "script"));
            assert!(!check("https://static.example/allowed/one.js", "script"));
        }
    }

    #[test]
    fn rules_that_cannot_allow_an_https_request_do_not_defer_early_blocking() {
        let f = filters(
            "||ads.example^\n@@|http://$script\n@@|ws://$script\n\
                         @@*$generichide\n@@*$tag=inactive\n@@*$1p,3p\n",
        );
        assert!(f.blocks_host("ads.example"));
        assert!(
            f.check("https://ads.example/file.js", "", "script", "GET")
                .blocked
        );
        assert!(
            !f.check("http://ads.example/file.js", "", "script", "GET")
                .blocked
        );
    }

    #[test]
    fn a_destination_unscoped_exception_defers_only_full_mode_host_checks() {
        let f =
            filters("||ads.example^\n||pass.example^\n@@||pass.example^\n@@/downloads/$image\n");
        assert!(!f.blocks_host("ads.example"));
        assert!(f.blocks_domain("ads.example"));
        assert!(!f.blocks_domain("pass.example"));
        assert!(
            !f.check("https://ads.example/downloads/a.png", "", "image", "GET")
                .blocked
        );
        assert!(
            f.check("https://ads.example/blocked/a.png", "", "image", "GET")
                .blocked
        );
    }

    #[test]
    fn a_disabled_exception_cannot_mask_a_block_by_host_or_request() {
        let f = filters(
            "||ads.example^\n@@/allowed^$3p,script,domain=reader.example\n\
                         @@/allowed^$script,third-party,from=reader.example,badfilter\n",
        );
        assert!(f.blocks_host("ads.example"));
        for source in ["", "https://reader.example/"] {
            assert!(
                f.check("https://ads.example/allowed/a.js", source, "script", "GET")
                    .blocked
            );
        }
    }

    #[test]
    fn a_missing_source_matches_only_independent_blocks_and_possible_exceptions() {
        let f = filters(
            "||third.example^$3p\n||first.example^$1p\n\
                         ||either.example^$3p\n||either.example^$1p\n\
                         ||context.example^$from=~reader.example\n\
                         ||ads.example^\n@@||ads.example/allowed^$~third-party,script\n\
                         @@||ads.example/reading^$domain=reader.example,script,method=post\n\
                         /promo^$to=neutral.example,image\n",
        );
        let check = |host_path: &str, page, kind, method| {
            f.check(&format!("https://{host_path}"), page, kind, method)
                .blocked
        };
        for no_page in ["", "not a source", "null", "about:blank"] {
            for url in [
                "third.example/ads",
                "first.example/ads",
                "either.example/ads",
                "context.example/ads",
            ] {
                assert!(!check(url, no_page, "script", "GET"), "{url}");
            }
            assert!(check("ads.example/blocked", no_page, "script", "GET"));
            assert!(!check("ads.example/allowed/a.js", no_page, "script", "GET"));
            assert!(!check(
                "ads.example/reading/a.js",
                no_page,
                "script",
                "POST"
            ));
            assert!(check("ads.example/reading/a.js", no_page, "script", "GET"));
            assert!(check("ads.example/allowed/a.png", no_page, "image", "GET"));
            assert!(check(
                "neutral.example/promo/a.png",
                no_page,
                "image",
                "GET"
            ));
            assert!(!check("other.example/promo/a.png", no_page, "image", "GET"));
        }
        assert!(check(
            "third.example/ads",
            "https://reader.example/",
            "script",
            "GET"
        ));
        assert!(!check(
            "third.example/ads",
            "https://third.example/",
            "script",
            "GET"
        ));
        assert!(check(
            "first.example/ads",
            "https://first.example/",
            "script",
            "GET"
        ));
        assert!(check(
            "ads.example/allowed/a.js",
            "https://reader.example/",
            "script",
            "GET"
        ));
        assert!(!check(
            "ads.example/reading/a.js",
            "https://reader.example/",
            "script",
            "POST"
        ));
        assert!(check(
            "ads.example/reading/a.js",
            "https://other.example/",
            "script",
            "POST"
        ));
    }

    #[test]
    fn projected_exceptions_retain_broad_and_exclusion_only_destination_options() {
        let f = filters(
            "||ads.example.com^\n||ads.example.net^\n\
             @@*$from=reader.example,to=com,image\n\
             @@*$from=reader.example,to=~ads.example.com,script\n",
        );
        let check = |host, source, kind| {
            f.check(&format!("https://{host}/resource"), source, kind, "GET")
                .blocked
        };
        for source in ["", "https://reader.example/"] {
            assert!(!check("ads.example.com", source, "image"));
            assert!(check("ads.example.net", source, "image"));
            assert!(check("ads.example.com", source, "script"));
            assert!(!check("ads.example.net", source, "script"));
        }
        assert!(check(
            "ads.example.com",
            "https://elsewhere.example/",
            "image"
        ));
        assert!(check(
            "ads.example.net",
            "https://elsewhere.example/",
            "script"
        ));
    }

    #[test]
    fn a_request_is_judged_by_its_address_its_page_and_its_kind() {
        let f = filters("/banner/*$image\n||tracker.example^$third-party\n");
        let page = "https://news.example/story";
        assert!(
            f.check("https://news.example/banner/1.png", page, "image", "GET")
                .blocked
        );
        assert!(
            !f.check("https://news.example/banner/1.js", page, "script", "GET")
                .blocked
        );
        assert!(
            f.check("https://tracker.example/t.js", page, "script", "GET")
                .blocked
        );
        // The tracker's own site, visited.
        let own = "https://tracker.example/";
        assert!(
            !f.check("https://tracker.example/t.js", own, "script", "GET")
                .blocked
        );
    }

    #[test]
    fn an_exception_lets_through_what_a_rule_blocks() {
        let f = filters("||ads.example^\n@@||ads.example/allowed^\n");
        let page = "https://news.example/";
        assert!(
            f.check("https://ads.example/x.js", page, "script", "GET")
                .blocked
        );
        assert!(
            !f.check("https://ads.example/allowed/x.js", page, "script", "GET")
                .blocked
        );
    }

    #[test]
    fn names_the_rule_that_matched_when_asked_to_keep_them() {
        let f = filters("||ads.example^\n");
        let check = f.check(
            "https://ads.example/x",
            "https://a.example/",
            "other",
            "GET",
        );
        assert_eq!(check.rule.as_deref(), Some("||ads.example^"));
    }

    #[test]
    fn lists_are_read_from_every_directory_and_only_from_txt_files() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(a.path().join("one.txt"), "||one.example^\n").unwrap();
        std::fs::write(b.path().join("two.txt"), "||two.example^\n").unwrap();
        std::fs::write(b.path().join("two.txt.new"), "||half.example^\n").unwrap();
        let missing = b.path().join("missing");
        let f = Filters::load(&[a.path(), b.path(), &missing], false).unwrap();
        assert!(f.blocks_host("one.example") && f.blocks_host("two.example"));
        assert!(!f.blocks_host("half.example"));
        assert_eq!((f.rules, f.host_rules), (2, 2));
    }

    #[test]
    fn an_incomplete_startup_load_is_used_without_caching_it() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let missing = dir.path().join("later.txt");
        std::os::unix::fs::symlink(dir.path().join("absent"), &missing).unwrap();
        std::fs::write(dir.path().join("ok.txt"), "||one.example^").unwrap();
        let filters = Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        assert!(filters.blocks_host("one.example"));
        assert!(filters.generation.is_none());
        assert!(!cache.path().join("key").exists());
        std::fs::remove_file(missing).unwrap();
        assert!(
            Filters::load_cached(&[dir.path()], cache.path())
                .unwrap()
                .generation
                .is_some()
        );
    }

    #[test]
    fn a_file_that_is_no_list_is_left_out_and_the_rest_still_filter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "||one.example^\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let f = Filters::load(&[dir.path()], false).unwrap();
        assert!(f.blocks_host("one.example"));
        assert_eq!(f.rules, 1);
    }

    #[test]
    fn a_load_that_left_out_a_list_not_in_utf8_is_not_cached() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(dir.path().join("a.txt"), "||one.example^\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let filters = Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        assert!(filters.blocks_host("one.example"));
        assert!(filters.generation.is_none());
        assert!(!cache.path().join("key").exists());
    }

    #[test]
    fn lists_loaded_before_are_taken_from_the_cache_until_one_changes() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let list = dir.path().join("list.txt");
        std::fs::write(&list, "||one.example^\n/banner/*$image\n").unwrap();
        let load = || Filters::load_cached(&[dir.path()], cache.path()).unwrap();

        let first = load();
        assert!(cache.path().join("key").exists());
        // The lists gone from under the cache: only it can answer now.
        let key = ListFiles::read(&[dir.path()]).unwrap().generation;
        let kept = Filters::read_cache(cache.path(), key).unwrap();
        assert!(kept.blocks_host("one.example"));
        let page = "https://news.example/";
        assert!(
            kept.check("https://a.example/banner/1.png", page, "image", "GET")
                .blocked
        );
        assert_eq!(
            (kept.rules, kept.host_rules),
            (first.rules, first.host_rules)
        );

        // A list changed: what was kept is no longer for these lists.
        std::fs::write(&list, "||two.example^\n").unwrap();
        let second = load();
        assert!(second.blocks_host("two.example") && !second.blocks_host("one.example"));
    }

    /// Not a test: prints what reading the lists costs, from the files
    /// and from the cache. `ABD_LISTS=<dir> cargo test --release -- --ignored --nocapture measures`
    #[test]
    #[ignore = "a measurement, run by hand"]
    fn measures_loading_the_lists() {
        let Some(dir) = std::env::var_os("ABD_LISTS") else {
            return;
        };
        let rss = || {
            std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|status| {
                    status
                        .lines()
                        .find_map(|line| line.strip_prefix("VmRSS:").map(|v| v.trim().to_owned()))
                })
        };
        // A supplied cache allows each variant to be loaded in its own
        // process, without counting memory retained by the allocator
        // after an earlier load.
        if let Some(cache) = std::env::var_os("ABD_LIST_CACHE") {
            let started = std::time::Instant::now();
            let f = Filters::load_cached(&[Path::new(&dir)], Path::new(&cache)).unwrap();
            println!(
                "{} lines, projected={}: {:?}, VmRSS={:?}",
                f.rules,
                f.requests.unknown.is_some(),
                started.elapsed(),
                rss()
            );
            return;
        }
        let cache = tempfile::tempdir().unwrap();
        let load = || {
            let started = std::time::Instant::now();
            let f = Filters::load_cached(&[Path::new(&dir)], cache.path()).unwrap();
            (started.elapsed(), f.rules, rss())
        };
        let (from_lists, rules, lists_rss) = load();
        let (from_cache, _, cache_rss) = load();
        println!(
            "{rules} lines: {from_lists:?} from the lists (VmRSS={lists_rss:?}), \
             {from_cache:?} from the cache (VmRSS={cache_rss:?})"
        );
    }

    #[test]
    fn a_new_generation_removes_stale_optional_engines() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let list = dir.path().join("list.txt");
        std::fs::write(
            &list,
            "||ads.example^$third-party\n@@||page.example^$document\n\
             @@||images.example^$ehide\n@@||news.example^$shide\n##div[data-ad]\n",
        )
        .unwrap();
        let load = || Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        let first = load();
        let optional = [
            "unknown",
            "no-cosmetic",
            "no-element-hiding",
            "no-specific-cosmetic",
            "generic-css",
        ];
        for name in optional {
            assert!(cache.path().join(name).is_file(), "{name}");
        }

        std::fs::write(&list, "||ads.example^\n").unwrap();
        let second = load();
        assert_ne!(first.generation, second.generation);
        for name in optional {
            assert!(!cache.path().join(name).exists(), "{name}");
        }
        assert!(Filters::read_cache(cache.path(), first.generation.unwrap()).is_none());
        let kept = Filters::read_cache(cache.path(), second.generation.unwrap()).unwrap();
        assert!(kept.blocks_host("ads.example"));
    }

    #[test]
    fn a_damaged_cache_is_not_used() {
        let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(dir.path().join("list.txt"), "||one.example^\n").unwrap();
        let load = || Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        load();
        let full = cache.path().join("full");
        let mut bytes = std::fs::read(&full).unwrap();
        bytes.truncate(bytes.len() / 2);
        std::fs::write(&full, bytes).unwrap();
        // Read from the lists again, and right.
        assert!(load().blocks_host("one.example"));
        std::fs::write(cache.path().join("hosts"), b"not an engine").unwrap();
        assert!(load().blocks_host("one.example"));
    }

    #[test]
    fn what_is_not_an_address_is_let_through() {
        let f = filters("||ads.example^\n");
        assert!(
            !f.check("not an address", "also not", "other", "GET")
                .blocked
        );
        assert!(!f.blocks_host(""));
    }

    #[test]
    fn reusing_the_parsed_page_keeps_request_context_and_canonicalization() {
        let f = filters(
            "||ads.example^$third-party\n@@||ads.example/keep$domain=news.example\n\
             ||news.example/banner$first-party",
        );
        for url in [
            "https://AdS.Example/ad.js",
            "https://ads.example/keep",
            "https://news.example/banner",
        ] {
            for source in [
                "https://news.example/page",
                "https://ads.example",
                "",
                "file:///page",
            ] {
                for document in [false, true] {
                    let page = parse_http_page(source);
                    let actual = f.check_with_page(url, page.as_ref(), document, "script", "GET");
                    let expected =
                        f.check(url, if document { url } else { source }, "script", "GET");
                    assert_eq!(
                        (actual.blocked, actual.rule),
                        (expected.blocked, expected.rule)
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "a measurement, run by hand"]
    fn measures_reusing_request_page_parsing() {
        let f = filters("||ads.example^$third-party");
        let url = "https://ads.example/banner";
        let source = "https://news.example/story";
        let count = 50_000;
        let start = std::time::Instant::now();
        for _ in 0..count {
            // The old request path parsed the header for matching and
            // again for recording, in addition to the matcher's parsing.
            assert!(std::hint::black_box(parse_http_page(source)).is_some());
            assert!(f.check(url, source, "image", "GET").blocked);
            assert!(std::hint::black_box(parse_http_page(source)).is_some());
        }
        println!(
            "separate request page parsing: {:?}/request",
            start.elapsed() / count
        );
        let start = std::time::Instant::now();
        for _ in 0..count {
            let page = std::hint::black_box(parse_http_page(source));
            assert!(
                f.check_with_page(url, page.as_ref(), false, "image", "GET")
                    .blocked
            );
        }
        println!(
            "shared request page parsing: {:?}/request",
            start.elapsed() / count
        );
    }
}
