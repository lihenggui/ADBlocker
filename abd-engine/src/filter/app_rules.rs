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

//! Rules for named apps (`$app=`), an option adblock-rust does not have.
//!
//! They are kept apart from the web rules, in engines where the source of
//! a request is the app that made it: the option is rewritten as the
//! source condition those engines index. The page a request came from is
//! not a source there, so a block that also needs one is left out, and an
//! exception that names one applies whatever the page.
//!
//! The members of a shared UID are not told apart. A block for one of
//! them is applied to none, and an exception for any app is applied to
//! all.

use std::collections::HashSet;

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions, RuleTypes};
use adblock::request::Request;

use super::host_rules::{host_only, project_guard};
use super::{Check, network, source_option};
use crate::apps::{Identity, valid_package};

/// The apps of exceptions that a shared UID is matched against. A list
/// naming more is not one of rules for apps people have.
const MAX_EXCEPTED: usize = 4096;

/// An exception for every address, as a guard: the exception it stands
/// for could not be narrowed to hosts.
const ANY_HOST: &str = "@@|https://";

/// The option that the apps of a rule are written as for adblock-rust.
const SOURCE: &str = "domain=";

pub(crate) struct AppRules {
    pub(super) blocks: Engine,
    exceptions: Engine,
    /// The blocks of a whole host, for a connection that stays encrypted.
    pub(super) hosts: Engine,
    /// Where an exception may apply, for a host refused before a request.
    guards: Engine,
    /// The apps that exceptions name, sorted.
    excepted: Vec<u64>,
}

/// One app among the app rules.
pub(crate) struct Scope {
    sources: Vec<u64>,
    blocks: bool,
}

struct Scoped<'a> {
    pattern: &'a str,
    options: Vec<&'a str>,
    /// As in the rule, in lower case: adblock-rust hashes what it is given.
    apps: String,
}

fn name_hash(package: &str) -> u64 {
    adblock::utils::fast_hash(&package.to_ascii_lowercase())
}

fn scoped(line: &str) -> Option<Scoped<'_>> {
    let (pattern, all) = line.trim().rsplit_once('$')?;
    let exception = pattern.starts_with("@@");
    let mut apps = None;
    let mut options = Vec::new();
    for option in all.split(',') {
        if let Some(names) = option.strip_prefix("app=") {
            // Two conditions on the app would have to hold together, and
            // the source condition holds when one of its names does.
            if apps.replace(names).is_some() {
                return None;
            }
        } else if source_option(option) {
            if !exception {
                return None;
            }
        } else {
            options.push(option);
        }
    }
    let apps = apps?;
    apps.split('|')
        .all(|name| valid_package(name.strip_prefix('~').unwrap_or(name)))
        .then(|| Scoped {
            pattern,
            options,
            apps: apps.to_ascii_lowercase(),
        })
}

impl Scoped<'_> {
    fn exception(&self) -> bool {
        self.pattern.starts_with("@@")
    }

    /// The rule for every app.
    fn unscoped(&self) -> String {
        let mut line = self.pattern.to_owned();
        let mut separator = '$';
        for option in &self.options {
            line.push(separator);
            line.push_str(option);
            separator = ',';
        }
        line
    }

    fn for_apps(&self, rule: &str) -> String {
        let separator = if rule.contains('$') { ',' } else { '$' };
        format!("{rule}{separator}{SOURCE}{}", self.apps)
    }

    fn line(&self) -> String {
        self.for_apps(&self.unscoped())
    }

    fn whole_host(&self) -> Option<String> {
        self.options
            .is_empty()
            .then(|| host_only(self.pattern))
            .flatten()
            .map(|rule| self.for_apps(&rule))
    }

    fn guards(&self, out: &mut String) {
        let mut found = String::new();
        // A rule taken back with `$badfilter` is left out before this.
        let any_host = project_guard(&self.unscoped(), &HashSet::new(), &mut found);
        for guard in found.lines().chain(any_host.then_some(ANY_HOST)) {
            out.push_str(&self.for_apps(guard));
            out.push('\n');
        }
    }

    fn named(&self) -> impl Iterator<Item = u64> + '_ {
        self.apps
            .split('|')
            .filter(|name| !name.starts_with('~'))
            .map(name_hash)
    }
}

/// The rule with its app as a source, if it is one for apps that the
/// engine applies.
fn rewritten(line: &str) -> Option<String> {
    let line = scoped(line)?.line();
    network(&line).is_some().then_some(line)
}

/// The rule that an engine here names, as one for apps again: its apps
/// are its last option.
fn shown(rule: String) -> String {
    match rule.rsplit_once(SOURCE) {
        Some((rest, apps)) => format!("{rest}app={apps}"),
        None => rule,
    }
}

/// How a rule for apps is named when it matched, if `line` is one that the
/// engine applies: with its apps last and in lower case.
pub(crate) fn named(line: &str) -> Option<String> {
    rewritten(line).map(shown)
}

fn engine(rules: String, debug: bool) -> Engine {
    let mut set = FilterSet::new(debug);
    let options = ParseOptions {
        rule_types: RuleTypes::NetworkOnly,
        ..ParseOptions::default()
    };
    set.add_filter_list(rules, options);
    Engine::new_with_filter_set(set)
}

fn swap_source<T>(
    req: &mut Request,
    sources: &mut Option<Vec<u64>>,
    f: impl FnOnce(&Request) -> T,
) -> T {
    std::mem::swap(&mut req.source_hostname_hashes, sources);
    let result = f(req);
    std::mem::swap(&mut req.source_hostname_hashes, sources);
    result
}

impl AppRules {
    /// The app rules of `lists`, if they have any.
    pub(crate) fn project<'a>(lists: impl Iterator<Item = &'a str>, debug: bool) -> Option<Self> {
        let rules: Vec<_> = lists
            .flat_map(str::lines)
            .filter_map(scoped)
            .filter_map(|rule| {
                let line = rule.line();
                let parsed = network(&line)?;
                Some((rule, (parsed.get_id(), parsed.is_badfilter()), line))
            })
            .collect();
        if rules.is_empty() {
            return None;
        }
        let disabled: HashSet<u64> = rules
            .iter()
            .filter_map(|(_, (id, bad), _)| bad.then_some(*id))
            .collect();
        let [mut blocks, mut exceptions, mut hosts, mut guards] = [const { String::new() }; 4];
        let mut excepted = Vec::new();
        for (rule, (id, bad), line) in rules {
            if bad || disabled.contains(&id) {
                continue;
            }
            let kept = if rule.exception() {
                rule.guards(&mut guards);
                excepted.extend(rule.named());
                &mut exceptions
            } else {
                if let Some(host) = rule.whole_host() {
                    hosts.push_str(&host);
                    hosts.push('\n');
                }
                &mut blocks
            };
            kept.push_str(&line);
            kept.push('\n');
        }
        excepted.sort_unstable();
        excepted.dedup();
        excepted.truncate(MAX_EXCEPTED);
        Some(Self {
            blocks: engine(blocks, debug),
            exceptions: engine(exceptions, debug),
            hosts: engine(hosts, false),
            guards: engine(guards, false),
            excepted,
        })
    }

    /// The engines as they are kept, each to be read back by
    /// [`AppRules::compiled`] in this order.
    pub(crate) fn serialized(&self) -> [Vec<u8>; 5] {
        let excepted = self.excepted.iter().flat_map(|name| name.to_be_bytes());
        [
            self.blocks.serialize(),
            self.exceptions.serialize(),
            self.hosts.serialize(),
            self.guards.serialize(),
            excepted.collect(),
        ]
    }

    /// What [`AppRules::serialized`] kept, if it is whole.
    pub(crate) fn compiled(kept: [&[u8]; 5]) -> Option<Self> {
        let [blocks, exceptions, hosts, guards, excepted] = kept;
        let engine = |compiled: &[u8]| {
            let mut engine = Engine::default();
            // Checked by the engine against its own checksum.
            engine.deserialize(compiled).ok()?;
            Some(engine)
        };
        let (names, rest) = excepted.as_chunks::<8>();
        if !rest.is_empty() {
            return None;
        }
        Some(Self {
            blocks: engine(blocks)?,
            exceptions: engine(exceptions)?,
            hosts: engine(hosts)?,
            guards: engine(guards)?,
            excepted: names.iter().map(|name| u64::from_be_bytes(*name)).collect(),
        })
    }

    /// `app` as the engine names it: a package, or a shared UID's group.
    pub(super) fn scope(&self, app: &str) -> Scope {
        match Identity::parse(app) {
            Some(identity) if !identity.is_group() => Scope {
                sources: vec![name_hash(identity.package)],
                blocks: true,
            },
            _ => Scope {
                sources: self.excepted.clone(),
                blocks: false,
            },
        }
    }

    /// Judges `req` by the rules of `web` and those of the app together:
    /// a block of either is taken back by an exception of either, and an
    /// `$important` block by none. `blocks` is the engine of this that
    /// stands beside `web`.
    pub(super) fn check(
        &self,
        web: &Engine,
        blocks: &Engine,
        req: &mut Request,
        scope: &Scope,
    ) -> Check {
        let found = web.check_network_request(req);
        let web_rule = found.filter.is_some();
        let mut check = Check {
            blocked: found.should_block(),
            rule: found.filter.and_then(|rule| rule.raw_line),
        };
        if found.important {
            return check;
        }
        let mut app = Some(scope.sources.clone());
        let own = swap_source(req, &mut app, |req| {
            let own = if scope.blocks {
                blocks.check_network_request_subset(req, false, false)
            } else {
                Default::default()
            };
            let excepted = !own.important
                && (web_rule || own.filter.is_some())
                && self
                    .exceptions
                    .check_network_request_subset(req, true, true)
                    .exception
                    .is_some();
            (own, excepted)
        });
        let (own, excepted) = own;
        if own.important {
            check.blocked = true;
        } else if excepted {
            check.blocked = false;
        } else if !web_rule && own.filter.is_some() {
            // The web rules had no block to look for an exception to.
            check.blocked = web
                .check_network_request_subset(req, true, true)
                .exception
                .is_none();
        }
        if !web_rule {
            check.rule = own.filter.and_then(|rule| rule.raw_line).map(shown);
        }
        check
    }

    /// Whether no exception of the app could apply to a request of `req`'s
    /// host.
    pub(super) fn unguarded(&self, req: &mut Request, scope: &Scope) -> bool {
        swap_source(req, &mut Some(scope.sources.clone()), |req| {
            self.guards
                .check_network_request_subset(req, false, true)
                .exception
                .is_none()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::Filters;

    fn filters(rules: &str) -> Filters {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("list.txt"), rules).unwrap();
        Filters::load(&[dir.path()], true).unwrap()
    }

    fn blocked(filters: &Filters, app: &str, url: &str) -> bool {
        filters
            .for_app(app)
            .check_with_page(url, None, false, "other", "GET")
            .blocked
    }

    #[test]
    fn a_rule_for_an_app_blocks_that_app_and_no_other() {
        let f = filters("||shop.example^$app=com.reader|com.Viewer\n");
        for app in ["com.reader", "com.viewer", "12:com.reader"] {
            assert!(blocked(&f, app, "https://api.shop.example/ad"), "{app}");
            assert!(f.for_app(app).blocks_domain("api.shop.example"), "{app}");
            assert!(f.for_app(app).blocks_host("api.shop.example"), "{app}");
        }
        for app in ["com.shop", "com.reader.lite", "reader", "@com.reader"] {
            assert!(!blocked(&f, app, "https://api.shop.example/ad"), "{app}");
            assert!(!f.for_app(app).blocks_domain("api.shop.example"), "{app}");
            assert!(!f.for_app(app).blocks_host("api.shop.example"), "{app}");
        }
        assert!(!f.blocks_host("api.shop.example"));
        let any = f.check("https://api.shop.example/ad", "", "other", "GET");
        assert!(!any.blocked);
    }

    #[test]
    fn a_rule_for_every_app_but_one_spares_that_one() {
        let f = filters("||shop.example^$app=~com.shop\n");
        assert!(blocked(&f, "com.reader", "https://shop.example/"));
        assert!(f.for_app("com.reader").blocks_host("shop.example"));
        assert!(!blocked(&f, "com.shop", "https://shop.example/"));
        assert!(!f.for_app("com.shop").blocks_domain("shop.example"));
    }

    #[test]
    fn an_exception_for_an_app_takes_back_a_block_for_every_app() {
        let f = filters("||ads.example^\n@@||ads.example^$app=com.reader\n");
        assert!(!blocked(&f, "com.reader", "https://ads.example/a.js"));
        assert!(!f.for_app("com.reader").blocks_domain("ads.example"));
        assert!(!f.for_app("com.reader").blocks_host("ads.example"));
        assert!(blocked(&f, "com.other", "https://ads.example/a.js"));
        assert!(f.for_app("com.other").blocks_host("ads.example"));
        assert!(f.blocks_host("ads.example"));
    }

    #[test]
    fn an_exception_for_every_app_takes_back_a_block_for_one() {
        let f = filters("||shop.example^$app=com.reader\n@@||shop.example/pay\n");
        assert!(blocked(&f, "com.reader", "https://shop.example/ad"));
        assert!(!blocked(&f, "com.reader", "https://shop.example/pay"));
        // A request to the host may be the one excepted.
        assert!(!f.for_app("com.reader").blocks_host("shop.example"));
        assert!(f.for_app("com.reader").blocks_domain("shop.example"));
    }

    #[test]
    fn a_host_is_not_refused_before_a_request_an_app_exception_may_allow() {
        let f = filters(
            "||ads.example^\n||wide.example^\n\
             @@||ads.example/ok.js$app=com.reader\n\
             @@/consent.$app=com.wide\n",
        );
        assert!(!f.for_app("com.reader").blocks_host("ads.example"));
        assert!(blocked(&f, "com.reader", "https://ads.example/ad.js"));
        assert!(!blocked(&f, "com.reader", "https://ads.example/ok.js"));
        assert!(f.for_app("com.reader").blocks_host("wide.example"));
        assert!(f.for_app("com.other").blocks_host("ads.example"));
        // An exception without a host can apply to any.
        assert!(!f.for_app("com.wide").blocks_host("wide.example"));
        assert!(!blocked(&f, "com.wide", "https://wide.example/consent.js"));
    }

    #[test]
    fn an_important_block_is_taken_back_by_no_exception() {
        let f = filters(
            "||ads.example^$important,app=com.reader\n@@||ads.example^\n\
             ||web.example^$important\n@@||web.example^$app=com.reader\n",
        );
        assert!(blocked(&f, "com.reader", "https://ads.example/"));
        assert!(!blocked(&f, "com.other", "https://ads.example/"));
        assert!(blocked(&f, "com.reader", "https://web.example/"));
    }

    #[test]
    fn a_shared_uid_gets_the_exceptions_of_every_app_and_the_blocks_of_none() {
        let f = filters(
            "||ads.example^\n@@||ads.example^$app=com.member\n\
             ||shop.example^$app=com.member\n\
             ||other.example^\n@@||other.example^$app=~com.member\n",
        );
        let group = "@com.shared";
        assert!(!blocked(&f, group, "https://ads.example/"));
        assert!(!f.for_app(group).blocks_host("ads.example"));
        assert!(!blocked(&f, group, "https://shop.example/"));
        assert!(!f.for_app(group).blocks_domain("shop.example"));
        // Which member it is not is not known either.
        assert!(blocked(&f, group, "https://other.example/"));
    }

    #[test]
    fn a_block_that_needs_a_page_is_left_out_and_an_exception_does_not_need_it() {
        let f = filters(
            "||shop.example^$app=com.reader,third-party\n\
             ||ads.example^\n@@||ads.example^$app=com.reader,domain=news.example\n",
        );
        assert!(!blocked(&f, "com.reader", "https://shop.example/"));
        assert!(!blocked(&f, "com.reader", "https://ads.example/"));
    }

    #[test]
    fn a_rule_for_an_app_is_taken_back_by_its_badfilter() {
        let f = filters(
            "||shop.example^$app=com.reader\n||shop.example^$app=com.reader,badfilter\n\
             ||ads.example^$app=com.reader\n",
        );
        assert!(!blocked(&f, "com.reader", "https://shop.example/"));
        assert!(!f.for_app("com.reader").blocks_domain("shop.example"));
        assert!(blocked(&f, "com.reader", "https://ads.example/"));
    }

    #[test]
    fn the_rule_that_matched_is_named_with_its_apps() {
        let rule = "||shop.example^$script,app=com.reader";
        let f = filters("||shop.example^$app=com.Reader,script\n");
        assert_eq!(
            super::named("||shop.example^$app=com.Reader,script").as_deref(),
            Some(rule)
        );
        let check = f.for_app("com.reader").check_with_page(
            "https://shop.example/a.js",
            None,
            false,
            "script",
            "GET",
        );
        assert!(check.blocked);
        assert_eq!(check.rule.as_deref(), Some(rule));
    }

    #[test]
    fn rules_for_apps_are_kept_with_the_compiled_lists() {
        let dir = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let rules = "||ads.example^\n@@||ads.example^$app=com.member\n\
                     ||shop.example^$app=com.reader\n";
        std::fs::write(dir.path().join("list.txt"), rules).unwrap();
        for _ in 0..2 {
            let f = Filters::load_cached(&[dir.path()], cache.path()).unwrap();
            assert!(f.for_app("com.reader").blocks_host("shop.example"));
            assert!(!f.for_app("com.member").blocks_host("ads.example"));
            assert!(!f.for_app("@com.shared").blocks_host("ads.example"));
            assert!(f.for_app("com.reader").blocks_host("ads.example"));
        }
        assert!(cache.path().join("app-blocks").exists());
    }
}
