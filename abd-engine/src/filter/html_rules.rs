// SPDX-License-Identifier: GPL-3.0-only
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

//! Select HTML rules by the document's own host, independently of the
//! URL matcher. Rules without a positive domain require a known document
//! request. Exceptions cancel rules by their trimmed selector text.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::htmlfilter::Compiled;
use crate::limits::{MAX_HTML_RULES, MAX_HTML_RULES_PER_PAGE};
use crate::public_suffix::Name;
use crate::rule::html::{Budget, Domain, Rule, Spec};

#[derive(Default)]
pub(super) struct HtmlProjections<'a> {
    rules: BTreeMap<(bool, &'a str), Spec>,
    dropped: usize,
}

pub(super) struct HtmlSpecs {
    rules: Vec<Spec>,
    dropped: usize,
}

pub(super) enum HtmlOmissions {
    Rules(usize),
    ExceptionLimit,
}

impl HtmlOmissions {
    pub(super) fn report(self) {
        match self {
            Self::Rules(0) => (),
            Self::Rules(dropped) => {
                eprintln!("lists: omitted {dropped} HTML rules at the generation limit");
            }
            Self::ExceptionLimit => {
                eprintln!("lists: omitted HTML filtering at the exception generation limit");
            }
        }
    }
}

impl<'a> HtmlProjections<'a> {
    pub(super) fn add(&mut self, text: &'a str, rule: Rule) {
        // Retain exemptions before removals if the generation fills up.
        self.rules.insert((!rule.spec.exception, text), rule.spec);
        if self.rules.len() > MAX_HTML_RULES {
            self.rules.pop_last();
            self.dropped += 1;
        }
    }

    pub(super) fn prepare(self) -> HtmlSpecs {
        HtmlSpecs {
            rules: self.rules.into_values().collect(),
            dropped: self.dropped,
        }
    }
}

impl HtmlSpecs {
    pub(super) fn compile(self) -> (HtmlRules, HtmlOmissions) {
        let mut index = HtmlRules::default();
        let mut budget = Budget::default();
        let mut dropped = self.dropped;
        for spec in self.rules {
            // Compile only retained specs so rejected rules cannot accumulate regex heap.
            let compiled = spec.compile().expect("validated HTML rule compiles");
            if budget.keep(&compiled).is_err() {
                if spec.exception {
                    return (HtmlRules::default(), HtmlOmissions::ExceptionLimit);
                }
                dropped += 1;
                continue;
            }
            index.add(Rule { spec, compiled });
        }
        (index, HtmlOmissions::Rules(dropped))
    }
}

#[derive(Default)]
pub(super) struct HtmlRules {
    rules: Vec<(Spec, Arc<Compiled>)>,
    suffixes: HashMap<String, Vec<usize>>,
    wildcards: HashMap<String, Vec<usize>>,
    global: Vec<usize>,
}

impl HtmlRules {
    fn add(&mut self, rule: Rule) {
        let id = self.rules.len();
        let mut positive = false;
        for domain in rule.spec.domains.iter().filter(|domain| !domain.exclude) {
            positive = true;
            let entries = if domain.wildcard {
                &mut self.wildcards
            } else {
                &mut self.suffixes
            };
            let ids = entries.entry(domain.name.clone()).or_default();
            if ids.last() != Some(&id) {
                ids.push(id);
            }
        }
        if !positive {
            self.global.push(id);
        }
        self.rules.push((rule.spec, Arc::new(rule.compiled)));
    }

    pub(super) fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub(super) fn serialized(&self) -> Option<Vec<u8>> {
        (!self.rules.is_empty()).then(|| {
            let specs: Vec<_> = self.rules.iter().map(|(spec, _)| spec).collect();
            serde_json::to_vec(&specs).expect("HTML rule specs can be serialized")
        })
    }

    pub(super) fn cached(data: &[u8]) -> Option<Self> {
        let specs: Vec<Spec> = serde_json::from_slice(data).ok()?;
        if specs.len() > MAX_HTML_RULES {
            return None;
        }
        let mut index = Self::default();
        let mut budget = Budget::default();
        for spec in specs {
            let compiled = spec.compile().ok()?;
            budget.keep(&compiled).ok()?;
            index.add(Rule { spec, compiled });
        }
        Some(index)
    }

    fn candidates(&self, host: &Host<'_>, known_document: bool) -> BTreeSet<usize> {
        let mut ids = BTreeSet::new();
        if known_document {
            ids.extend(self.global.iter().copied());
        }
        let mut suffix = host.name;
        loop {
            ids.extend(self.suffixes.get(suffix).into_iter().flatten().copied());
            let Some((_, parent)) = suffix.split_once('.').filter(|_| host.has_suffixes) else {
                break;
            };
            suffix = parent;
        }
        if let Some(wildcard) = host.registrable_label {
            ids.extend(self.wildcards.get(wildcard).into_iter().flatten().copied());
        }
        ids
    }

    fn select(&self, host: &str, known_document: bool) -> Vec<Arc<Compiled>> {
        if self.rules.is_empty() {
            return Vec::new();
        }
        let normalized = if host.bytes().any(|b| b.is_ascii_uppercase()) {
            std::borrow::Cow::Owned(host.to_ascii_lowercase())
        } else {
            std::borrow::Cow::Borrowed(host)
        };
        let host = Host::new(normalized.trim_end_matches('.'));
        let candidates = self.candidates(&host, known_document);
        let applies = |spec: &Spec| {
            !spec
                .domains
                .iter()
                .any(|domain| domain.exclude && host.matches(domain))
        };
        let mut seen: HashSet<_> = candidates
            .iter()
            .map(|&id| &self.rules[id].0)
            .filter(|spec| spec.exception && applies(spec))
            .map(|spec| spec.selector.as_str())
            .collect();
        candidates
            .into_iter()
            .filter_map(|id| {
                let (spec, compiled) = &self.rules[id];
                (!spec.exception && applies(spec) && seen.insert(spec.selector.as_str()))
                    .then(|| Arc::clone(compiled))
            })
            .take(MAX_HTML_RULES_PER_PAGE)
            .collect()
    }
}

struct Host<'a> {
    name: &'a str,
    has_suffixes: bool,
    registrable_label: Option<&'a str>,
}

impl<'a> Host<'a> {
    fn new(name: &'a str) -> Self {
        let dns = Name::parse(name);
        let registrable_label =
            dns.and_then(|dns| dns.root()?.split_once('.').map(|(label, _)| label));
        Self {
            name,
            has_suffixes: dns.is_some(),
            registrable_label,
        }
    }

    fn matches(&self, domain: &Domain) -> bool {
        if domain.wildcard {
            return self.registrable_label == Some(&domain.name);
        }
        self.name == domain.name
            || (self.has_suffixes
                && self
                    .name
                    .strip_suffix(&domain.name)
                    .is_some_and(|prefix| prefix.ends_with('.')))
    }
}

impl super::Filters {
    /// Host selection also determines whether the request needs an unpacked body.
    pub(crate) fn html_rules(
        &self,
        host: &str,
        url: &str,
        method: &str,
        known_document: bool,
    ) -> Vec<Arc<Compiled>> {
        let rules = self.html.select(host, known_document);
        if !rules.is_empty() && self.page_exceptions.skip_html(url, method) {
            return Vec::new();
        }
        rules
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::path::Path;

    use crate::filter::Filters;

    fn filters(text: &str) -> Filters {
        Filters::build(
            [Ok((Path::new("subscription.txt"), text.to_owned()))].into_iter(),
            None,
            false,
            16,
        )
        .unwrap()
    }

    fn cut(filters: &Filters, host: &str, path: &str, html: &'static str) -> Bytes {
        let selected = filters.html_rules(host, &format!("https://{host}/{path}"), "GET", true);
        crate::htmlfilter::apply(
            Bytes::from_static(html.as_bytes()),
            &selected,
            None,
            false,
            &|| true,
        )
        .unwrap()
    }

    #[test]
    fn an_untrusted_list_can_cut_elements_on_the_named_domain_and_its_subdomains() {
        let filters = filters("example.org$$script\n");
        for (host, survives) in [
            ("example.org", false),
            ("sub.example.org", false),
            ("example.org.evil.test", true),
            ("otherexample.org", true),
            ("example.net", true),
        ] {
            assert_eq!(
                !cut(&filters, host, "", "<script>ad</script>").is_empty(),
                survives,
                "{host}"
            );
        }
    }

    #[test]
    fn negative_domains_apply_with_or_without_an_inclusion() {
        let filters = filters("example.org,~keep.example.org$$script\n~keep.example.org$$div\n");
        let body = "<script>ad</script><div>ad</div>";
        assert_eq!(cut(&filters, "example.org", "", body), "");
        assert_eq!(cut(&filters, "sub.keep.example.org", "", body), body);
        assert_eq!(
            cut(&filters, "elsewhere.test", "", body),
            "<script>ad</script>"
        );
    }

    #[test]
    fn wildcard_domains_match_the_registrable_label_with_any_public_suffix() {
        let filters = filters("example.*,~keep.example.co.uk$$script\n");
        for (host, survives) in [
            ("example.org", false),
            ("sub.example.co.uk", false),
            ("example.blogspot.com", false),
            ("keep.example.co.uk", true),
            ("example.other.co.uk", true),
            ("example.evil.com", true),
        ] {
            assert_eq!(
                !cut(&filters, host, "", "<script>ad</script>").is_empty(),
                survives,
                "{host}"
            );
        }
    }

    #[test]
    fn exact_ip_domains_have_no_parent_suffixes() {
        let filters = filters("2.3.4.5$$div\n");
        assert_eq!(cut(&filters, "2.3.4.5", "", "<div>ad</div>"), "");
        assert_eq!(
            cut(&filters, "x.2.3.4.5", "", "<div>ad</div>"),
            "<div>ad</div>"
        );
    }

    #[test]
    fn a_trimmed_selector_exception_disables_a_global_rule_on_its_domain() {
        let filters = filters(
            "$$script[tag-content=\"banner\"]\nexample.com$@$  script[tag-content=\"banner\"]  \n",
        );
        let body = "<script>banner</script>";
        assert_eq!(cut(&filters, "example.org", "", body), "");
        assert_eq!(cut(&filters, "sub.example.com", "", body), body);
    }

    #[test]
    fn a_global_exception_cancels_the_same_selector_in_either_syntax() {
        for exception in [
            "$@$script[tag-content=\"banner\"]",
            "#@#^script[tag-content=\"banner\"]",
        ] {
            let filters = filters(&format!(
                "example.com$$script[tag-content=\"banner\"]\n{exception}\n"
            ));
            assert_eq!(
                cut(&filters, "example.com", "", "<script>banner</script>"),
                "<script>banner</script>"
            );
        }
    }

    #[test]
    fn content_and_document_exceptions_stop_html_filtering_on_the_document_itself() {
        let filters = filters(
            "$$div\n\
            @@||site.test/content$content\n\
            @@||site.test/doc$document\n\
            @@||site.test/element$elemhide\n\
            @@||site.test/generic$generichide\n",
        );
        let body = "<div>ad</div>";
        for path in ["content", "doc"] {
            assert_eq!(cut(&filters, "site.test", path, body), body);
        }
        for path in ["element", "generic", "ordinary"] {
            assert_eq!(cut(&filters, "site.test", path, body), "");
        }
    }

    #[test]
    fn the_page_bound_keeps_the_first_rules_in_lexicographical_order() {
        let text: String = (0..MAX_HTML_RULES_PER_PAGE + 2)
            .rev()
            .map(|i| format!("site.test$$div[id={i:03}]\n"))
            .collect();
        let filters = filters(&text);
        let result = filters.html_rules("site.test", "https://site.test/", "GET", true);
        assert_eq!(result.len(), MAX_HTML_RULES_PER_PAGE);
        for (i, rule) in result.iter().enumerate() {
            assert!(matches!(
                &rule.steps[0].attributes[0].value,
                Some(crate::htmlfilter::AttributeValue::Contains(value)) if value == &format!("{i:03}")
            ));
        }
    }

    #[test]
    fn cached_selectors_rebuild_their_compiled_conditions_and_exceptions() {
        let temp = tempfile::tempdir().unwrap();
        let lists = temp.path().join("lists");
        let cache = temp.path().join("cache");
        std::fs::create_dir(&lists).unwrap();
        std::fs::write(
            lists.join("rules.txt"),
            "example.*$$main > script[src^=/ads/]:contains(/^(?=.{2,8}$)(?=.*ad\\d+).+$/s)\n\
            keep.example.org$@$main > script[src^=/ads/]:contains(/^(?=.{2,8}$)(?=.*ad\\d+).+$/s)\n",
        )
        .unwrap();
        let built = Filters::load_cached(&[&lists], &cache).unwrap();
        let cached = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
        let body =
            "<main><script src=/ads/one>ad23</script><script src=/app/one>ad23</script></main>";
        for filters in [built, cached] {
            assert_eq!(
                cut(&filters, "example.co.uk", "", body),
                "<main><script src=/app/one>ad23</script></main>"
            );
            assert_eq!(cut(&filters, "keep.example.org", "", body), body);
        }
    }
}

#[cfg(test)]
#[test]
fn html_exceptions_displace_removals_at_the_table_limit() {
    let mut rules = HtmlProjections::default();
    let text: Vec<_> = (0..MAX_HTML_RULES)
        .map(|i| format!("site.test$$div[id={i:04}]"))
        .chain(["site.test$@$div[id=0000]".to_owned()])
        .collect();
    for line in &text {
        let crate::rule::Kind::Html(rule) = crate::rule::read(line).kind else {
            panic!("{line}")
        };
        rules.add(line, rule);
    }
    let (kept, _) = rules.prepare().compile();
    assert_eq!(kept.rules.len(), MAX_HTML_RULES);
    let body = bytes::Bytes::from_static(b"<div id=0000>keep</div><div id=0001>remove</div>");
    assert_eq!(
        crate::htmlfilter::apply(body, &kept.select("site.test", true), None, false, &|| true)
            .unwrap(),
        "<div id=0000>keep</div>"
    );
}

#[cfg(test)]
#[test]
fn reviewed_html_expressions_are_charged_to_the_generation_budget() {
    use crate::limits::{MAX_HTML_REGEX_BYTES, MAX_HTML_REGEXES};
    let text: Vec<_> = (0..MAX_HTML_REGEXES)
        .map(|i| format!(r"site{i}.test##^script:has-text(/\w{{10}}a{i}/)"))
        .collect();
    let mut rules = HtmlProjections::default();
    for line in &text {
        let crate::rule::Kind::Html(rule) = crate::rule::read(line).kind else {
            panic!("{line}")
        };
        rules.add(line, rule);
    }
    let (kept, _) = rules.prepare().compile();
    let charged: usize = kept.rules.iter().map(|(_, rule)| rule.regex_cost().1).sum();
    assert!(
        charged > 0 && charged <= MAX_HTML_REGEX_BYTES,
        "{charged} bytes"
    );
    assert_eq!(
        HtmlRules::cached(&kept.serialized().unwrap())
            .unwrap()
            .rules
            .len(),
        kept.rules.len()
    );

    // Expanding the counted repetition still fits individually but makes the shared budget bind.
    let text: Vec<_> = (0..MAX_HTML_REGEXES)
        .map(|i| format!(r"site{i}.test##^script:has-text(/\w{{250}}a{i}/)"))
        .collect();
    let mut rules = HtmlProjections::default();
    for line in &text {
        let crate::rule::Kind::Html(rule) = crate::rule::read(line).kind else {
            panic!("{line}")
        };
        rules.add(line, rule);
    }
    let (kept, _) = rules.prepare().compile();
    let charged: usize = kept.rules.iter().map(|(_, rule)| rule.regex_cost().1).sum();
    assert!(kept.rules.len() < MAX_HTML_REGEXES, "{charged} bytes");
    assert!(charged <= MAX_HTML_REGEX_BYTES, "{charged} bytes");
}
