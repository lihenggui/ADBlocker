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
//! not a source there, so a rule that also needs one is left out.
//!
//! The members of a shared UID are not told apart, so none of these rules
//! is applied to them: which app a rule would need to know is not known.
//! An exception for one app that allows every address, as lists have them,
//! would otherwise allow it to every such group.

use std::collections::HashSet;

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions, RuleTypes};
use adblock::request::Request;

use super::Check;
use super::app_patterns::{Patterns, Scope};
use super::host_rules::{GuardCoverage, can_guard, write_guards};
use super::redirects::RedirectProjections;
use crate::rule::{self, AppRule, whole_pattern};

/// An exception for every address, as a guard: the exception it stands
/// for could not be narrowed to hosts.
const ANY_HOST: &str = "@@|https://";

/// The option that the apps of a rule are written as for adblock-rust.
pub(super) const SOURCE: &str = "domain=";

pub(crate) struct AppRules {
    pub(super) blocks: Engine,
    exceptions: Engine,
    /// The blocks of a whole host, for a connection that stays encrypted.
    pub(super) hosts: Engine,
    /// Where an exception may apply, for a host refused before a request.
    guards: Engine,
}

impl Scope<'_> {
    /// Test a guard whose source condition names apps rather than pages.
    pub(super) fn unguarded(&self, req: &mut Request, guards: &Engine) -> bool {
        self.with_source(req, |req| super::unguarded(guards, req))
    }

    fn with_source<T>(&self, req: &mut Request, f: impl FnOnce(&Request) -> T) -> T {
        let previous = req.source_hostname_hashes.replace(self.hashes.to_vec());
        let result = f(req);
        req.source_hostname_hashes = previous;
        result
    }
}

struct Scoped<'a> {
    pattern: &'a str,
    options: Vec<&'a str>,
    /// As in the rule, in lower case: adblock-rust hashes what it is given.
    apps: String,
}

struct Projected<'a> {
    rule: Scoped<'a>,
    id: u64,
    bad: bool,
    is_redirect: bool,
    redirect_exception: bool,
    line: String,
}

fn scoped<'a>(rule: &AppRule<'a>, patterns: &mut Patterns) -> Option<Projected<'a>> {
    let condition = rule.condition()?;
    let prepared = patterns.prepare(condition.apps)?;
    let rule = Scoped {
        pattern: condition.pattern,
        options: condition.options,
        apps: prepared.encoded.clone(),
    };
    let line = rule.for_apps(&rule.unscoped(), !rule.options.is_empty());
    let source = rule::read(&line);
    let parsed = source.network()?;
    patterns.keep(prepared);
    Some(Projected {
        rule,
        id: parsed.get_id(),
        bad: parsed.is_badfilter(),
        is_redirect: parsed.is_redirect(),
        redirect_exception: source.redirect_exception,
        line: source.normalized().into_owned(),
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

    fn for_apps(&self, rule: &str, has_options: bool) -> String {
        let separator = if has_options { ',' } else { '$' };
        format!("{rule}{separator}{SOURCE}{}", self.apps)
    }

    fn whole_host(&self) -> Option<(String, &str)> {
        self.options
            .is_empty()
            .then(|| whole_pattern(self.pattern))
            .flatten()
            .map(|(rule, host)| (self.for_apps(&rule, false), host))
    }

    fn guards(&self, out: &mut String) {
        let mut found = String::new();
        // A rule taken back with `$badfilter` is left out before this.
        let line = self.unscoped();
        let source = rule::read(&line);
        let any_host = source
            .network()
            .filter(|rule| can_guard(&source, rule))
            .is_some_and(|rule| {
                matches!(
                    write_guards(&source, &rule, &mut found),
                    GuardCoverage::Unbounded
                )
            });
        for guard in found.lines().chain(any_host.then_some(ANY_HOST)) {
            out.push_str(&self.for_apps(guard, false));
            out.push('\n');
        }
    }
}

/// The rule that an engine names, with its app entries restored.
fn shown(rule: String, patterns: &Patterns) -> String {
    match rule.rsplit_once(SOURCE) {
        Some((rest, apps)) => format!("{rest}app={}", patterns.show(apps)),
        None => rule,
    }
}

/// How a rule for apps is named when it matched.
pub(crate) fn name(rule: &AppRule<'_>) -> Option<String> {
    let mut patterns = Patterns::default();
    let projected = scoped(rule, &mut patterns)?;
    Some(shown(projected.line, &patterns))
}

pub(crate) fn valid(rule: &AppRule<'_>, patterns: &mut Patterns) -> bool {
    scoped(rule, patterns).is_some()
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

#[derive(Default)]
pub(crate) struct AppProjections<'a> {
    rules: Vec<Projected<'a>>,
}

impl<'a> AppProjections<'a> {
    pub(crate) fn add(&mut self, rule: &AppRule<'a>, patterns: &mut Patterns) {
        self.rules.extend(scoped(rule, patterns));
    }

    /// Tell the host projection each host an app rule blocks whole.
    pub(crate) fn compile(
        self,
        debug: bool,
        mut visit_host: impl FnMut(&str, &str),
        mut redirects: Option<&mut RedirectProjections>,
    ) -> Option<AppRules> {
        let rules = self.rules;
        if rules.is_empty() {
            return None;
        }
        let disabled: HashSet<u64> = rules
            .iter()
            .filter_map(|entry| entry.bad.then_some(entry.id))
            .collect();
        let [mut blocks, mut exceptions, mut hosts, mut guards] = [const { String::new() }; 4];
        for Projected {
            rule,
            id,
            bad,
            is_redirect,
            redirect_exception,
            line,
        } in rules
        {
            if bad || disabled.contains(&id) {
                continue;
            }
            if is_redirect && let Some(redirects) = &mut redirects {
                redirects.add_app(&line, redirect_exception, &rule.apps);
            }
            if redirect_exception {
                continue;
            }
            let kept = if rule.exception() {
                rule.guards(&mut guards);
                &mut exceptions
            } else {
                if let Some((line, host)) = rule.whole_host() {
                    visit_host(host, &rule.apps);
                    hosts.push_str(&line);
                    hosts.push('\n');
                }
                &mut blocks
            };
            kept.push_str(&line);
            kept.push('\n');
        }
        Some(AppRules {
            blocks: engine(blocks, debug),
            exceptions: engine(exceptions, debug),
            hosts: engine(hosts, false),
            guards: engine(guards, false),
        })
    }
}

impl AppRules {
    /// The engines as they are kept, each to be read back by
    /// [`AppRules::compiled`] in this order.
    pub(crate) fn serialized(&self) -> [Vec<u8>; 4] {
        [
            self.blocks.serialize(),
            self.exceptions.serialize(),
            self.hosts.serialize(),
            self.guards.serialize(),
        ]
    }

    /// What [`AppRules::serialized`] kept, if it is whole.
    pub(crate) fn compiled(kept: [&[u8]; 4]) -> Option<Self> {
        let [blocks, exceptions, hosts, guards] = kept;
        let engine = |compiled: &[u8]| {
            let mut engine = Engine::default();
            // Checked by the engine against its own checksum.
            engine.deserialize(compiled).ok()?;
            Some(engine)
        };
        Some(Self {
            blocks: engine(blocks)?,
            exceptions: engine(exceptions)?,
            hosts: engine(hosts)?,
            guards: engine(guards)?,
        })
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
        scope: &Scope<'_>,
    ) -> Check {
        let found = web.check_network_request(req);
        let web_rule = found.filter.is_some();
        let mut check = Check {
            blocked: found.should_block(),
            rule: found.filter.and_then(|rule| rule.raw_line),
            redirect: None,
        };
        if found.important {
            return check;
        }
        let own = scope.with_source(req, |req| {
            let own = blocks.check_network_request_subset(req, false, false);
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
        // The rule that decided: the app's where no web rule matched, and
        // where its `$important` block stands against a web exception.
        if !web_rule || own.important {
            check.rule = own
                .filter
                .and_then(|rule| rule.raw_line)
                .map(|rule| shown(rule, scope.patterns));
        }
        check
    }

    /// Whether no exception of the app could apply to a request of `req`'s
    /// host.
    pub(super) fn unguarded(
        &self,
        req: &mut Request,
        scope: &Scope<'_>,
        web: Option<&Engine>,
    ) -> bool {
        scope.with_source(req, |req| {
            super::unguarded(&self.guards, req)
                && web.is_none_or(|guards| super::unguarded(guards, req))
        })
    }
}

#[cfg(test)]
fn named(line: &str) -> Option<String> {
    match rule::read(line).kind {
        rule::Kind::App(app) => name(&app),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Filters, Page};

    fn filters(rules: &str) -> Filters {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("list.txt"), rules).unwrap();
        Filters::load(&[dir.path()], true).unwrap()
    }

    fn blocked(filters: &Filters, app: &str, url: &str) -> bool {
        filters
            .for_app(app)
            .check_with_page(url, Page::NONE, "other", "GET")
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

        let f = filters("||x.example^\n@@||x.example^\n||x.example^$important,app=com.reader\n");
        let check = f.for_app("com.reader").check_with_page(
            "https://x.example/",
            Page::NONE,
            "other",
            "GET",
        );
        assert!(check.blocked);
        assert_eq!(
            check.rule.as_deref(),
            Some("||x.example^$important,app=com.reader")
        );
    }

    #[test]
    fn a_shared_uid_is_judged_by_the_rules_for_every_app_alone() {
        let f = filters(
            "||ads.example^\n@@||ads.example^$app=com.member\n\
             ||shop.example^$app=com.member\n\
             ||other.example^\n@@*$app=com.cinema\n",
        );
        let group = "@com.shared";
        assert!(blocked(&f, group, "https://ads.example/"));
        assert!(f.for_app(group).blocks_host("ads.example"));
        assert!(!blocked(&f, group, "https://shop.example/"));
        assert!(!f.for_app(group).blocks_domain("shop.example"));
        // An exception that allows one app every address allows it no other.
        assert!(blocked(&f, group, "https://other.example/"));
        assert!(f.for_app(group).blocks_domain("other.example"));
        assert!(blocked(&f, "com.member", "https://other.example/"));
        assert!(!blocked(&f, "com.cinema", "https://other.example/"));
        assert!(!f.for_app("com.cinema").blocks_host("other.example"));
    }

    #[test]
    fn a_rule_for_an_app_that_needs_a_page_is_left_out() {
        let f = filters(
            "||shop.example^$app=com.reader,third-party\n\
             ||ads.example^\n@@||ads.example^$app=com.reader,domain=news.example\n",
        );
        assert!(!blocked(&f, "com.reader", "https://shop.example/"));
        assert!(blocked(&f, "com.reader", "https://ads.example/"));
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
            Page::NONE,
            "script",
            "GET",
        );
        assert!(check.blocked);
        assert_eq!(check.rule.as_deref(), Some(rule));
    }

    #[test]
    fn the_reference_app_examples_match_their_named_packages() {
        for (entries, yes, no) in [
            ("org.example.app", "org.example.app", "org.example.app1"),
            (
                "org.example.app1|org.example.app2",
                "org.example.app2",
                "org.other",
            ),
            ("~org.example.app", "org.other", "org.example.app"),
            (
                "~org.example.app1|~org.example.app2",
                "org.other",
                "org.example.app2",
            ),
            (
                "org.example.*",
                "ORG.EXAMPLE.utility",
                "org.examples.utility",
            ),
            (
                r"/org\.example\.[a-z0-9_]+/",
                "x.org.example.app1",
                "org.example.A",
            ),
            (
                r"/^org\.example\.app\$\|^org\.example\.[ab].*/",
                "org.example.beta",
                "org.example.c",
            ),
            (
                r"org.example.app|org.example.*|/org\.example\.[a-z]+/",
                "x.org.example.abc",
                "org.other",
            ),
            (
                r"/com\..*\.music/",
                "com.example.music",
                "com.example.video",
            ),
            (
                r"/^org\.example\.App\$/",
                "org.example.App",
                "org.example.app",
            ),
            (
                r"/(?i)^org\.example\.App\$/",
                "org.example.app",
                "x.org.example.app",
            ),
            (
                r"/[\/\,]\|^com\.reader\$/|com.viewer",
                "com.reader",
                "com.reader.lite",
            ),
            (r"~com.reader*|~/^com\.viewer\$/", "com.other", "com.viewer"),
        ] {
            let rule = format!("||baddomain.com^$app={entries}");
            let f = filters(&rule);
            let url = "https://baddomain.com/a";
            assert!(blocked(&f, yes, url), "{rule} {yes}");
            assert!(!blocked(&f, no, url), "{rule} {no}");
        }
    }

    #[test]
    fn a_pattern_block_and_its_app_and_page_guards_share_the_same_scope() {
        let f = filters(
            r"||graph.facebook.com^$app=com.vitastudio.*
@@||graph.facebook.com/ok$app=/\.reader\$/
@@||graph.facebook.com/browser$domain=page.example",
        );
        let host = "graph.facebook.com";
        let url = "https://graph.facebook.com/";
        assert!(
            f.for_app_with_pages("com.vitastudio.game", false)
                .blocks_host(host)
        );
        assert!(!f.for_app("com.vitastudio.game").blocks_host(host));
        assert!(
            !f.for_app_with_pages("com.vitastudio.reader", false)
                .blocks_host(host)
        );
        assert!(!blocked(&f, "com.vitastudio.reader", &format!("{url}ok")));
        assert!(blocked(&f, "com.vitastudio.reader", &format!("{url}ad")));
        for other in [
            "com.other",
            "@com.vitastudio.game",
            "42:@com.vitastudio.reader",
        ] {
            assert!(!f.for_app(other).blocks_host(host));
            assert!(!blocked(&f, other, url));
        }
    }

    #[test]
    fn an_invalid_app_option_cannot_turn_into_a_web_block() {
        let f = filters(r"||bad.example^$app=/bad$script");
        let check = f.for_app("com.reader").check_with_page(
            "https://bad.example/ad.js",
            Page::NONE,
            "script",
            "GET",
        );
        assert!(!check.blocked);
        assert!(!f.for_app("com.reader").blocks_host("bad.example"));
    }

    #[test]
    fn exceeding_the_pattern_limit_drops_the_whole_rule_across_lists() {
        use crate::limits::MAX_APP_PATTERNS;

        let dir = tempfile::tempdir().unwrap();
        let text: String = (0..MAX_APP_PATTERNS)
            .map(|p| {
                format!(
                    "||ads.example^$app=com.p{p}.*
"
                )
            })
            .collect();
        std::fs::write(dir.path().join("a.txt"), text).unwrap();
        std::fs::write(
            dir.path().join("b.txt"),
            "||over.example^$app=com.reader|com.new.*
\
             ||reuse.example^$app=com.p0.*
||plain.example^$app=com.reader",
        )
        .unwrap();
        let f = Filters::load(&[dir.path()], true).unwrap();
        assert!(!blocked(&f, "com.reader", "https://over.example/"));
        assert!(!blocked(&f, "com.new.reader", "https://over.example/"));
        assert!(blocked(&f, "com.p0.reader", "https://reuse.example/"));
        assert!(blocked(&f, "com.reader", "https://plain.example/"));
    }

    #[test]
    fn rules_for_apps_are_kept_with_the_compiled_lists() {
        let dir = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let rules = r"||ads.example^
@@||ads.example^$app=/^com\.member\$/
||shop.example^$app=com.read*";
        std::fs::write(dir.path().join("list.txt"), rules).unwrap();
        let built = Filters::load_cached(&[dir.path()], cache.path()).unwrap();
        let cached = Filters::read_cache(cache.path(), built.generation.unwrap()).unwrap();
        for f in [built, cached] {
            assert!(f.for_app("com.reader").blocks_host("shop.example"));
            assert!(!f.for_app("com.other").blocks_host("shop.example"));
            assert!(!f.for_app("com.member").blocks_host("ads.example"));
            assert!(f.for_app("@com.shared").blocks_host("ads.example"));
            assert!(f.for_app("com.reader").blocks_host("ads.example"));
        }
        assert!(cache.path().join("app-blocks").exists());
    }
}
