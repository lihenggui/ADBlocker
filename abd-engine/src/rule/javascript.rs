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

use adblock::resources::{PermissionMask, Resource, ResourceType};
use base64::{Engine as _, prelude::BASE64_STANDARD};
use ring::digest::{SHA256, SHA256_OUTPUT_LEN, digest};

use crate::cosmetic::{JAVASCRIPT_END, JAVASCRIPT_START, safe_raw_text};
use crate::limits::{MAX_JS_RULE, MAX_JS_SCRIPTS, MAX_JS_TABLE_BYTES};

pub(crate) const PERMISSION: PermissionMask = PermissionMask::from_bits(0b10);
const NAME_PREFIX: &str = "abd-js-rule-";
pub(crate) const MAX_CACHED_SCRIPTS: usize =
    SHA256_OUTPUT_LEN + MAX_JS_TABLE_BYTES + MAX_JS_SCRIPTS;

pub(super) fn name(id: usize) -> String {
    format!("{NAME_PREFIX}{id}")
}

pub(super) fn uses_reserved_name(line: &str) -> bool {
    if line.starts_with('|') || line.starts_with("@@|") {
        return false;
    }
    let Some((_, suffix)) = line.split_once('#') else {
        return false;
    };
    let suffix = suffix.strip_prefix('@').unwrap_or(suffix);
    let suffix = suffix.strip_prefix('?').unwrap_or(suffix);
    suffix.strip_prefix("#+js(").is_some_and(|args| {
        let args = args.trim_start();
        // adblock-rust accepts quoted names and appends the .js extension.
        // Its escape normalization cannot create any character of this prefix.
        args.strip_prefix(['\'', '"', '`'])
            .unwrap_or(args)
            .starts_with(NAME_PREFIX)
    })
}

fn wrap(script: &str) -> String {
    [JAVASCRIPT_START, script, JAVASCRIPT_END].concat()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Rejection {
    NoDomains,
    Untrusted,
    Empty,
    Length,
    Template,
    Unsafe,
    Count,
    TableSize,
    ReservedName,
}

pub(super) fn check_body(script: &str) -> Result<(), Rejection> {
    if script.is_empty() {
        return Err(Rejection::Empty);
    }
    if script.len() > MAX_JS_RULE {
        return Err(Rejection::Length);
    }
    if script.contains("{{") {
        return Err(Rejection::Template);
    }
    if script.contains('\n') || !safe_raw_text(&wrap(script)) {
        return Err(Rejection::Unsafe);
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct Scripts<'a> {
    ids: HashMap<&'a str, usize>,
    entries: Vec<&'a str>,
    bytes: usize,
    dropped: usize,
}

impl<'a> Scripts<'a> {
    pub(super) fn number(&self, script: &str) -> usize {
        self.ids.get(script).copied().unwrap_or(self.entries.len())
    }

    pub(super) fn keep(&mut self, script: &'a str, id: usize) -> Result<(), Rejection> {
        if id < self.entries.len() {
            return Ok(());
        }
        let bytes = self.bytes + script.len();
        if id == MAX_JS_SCRIPTS {
            self.dropped += 1;
            return Err(Rejection::Count);
        }
        if bytes > MAX_JS_TABLE_BYTES {
            self.dropped += 1;
            return Err(Rejection::TableSize);
        }
        self.ids.insert(script, id);
        self.entries.push(script);
        self.bytes = bytes;
        Ok(())
    }

    pub(crate) fn finish(self) -> ScriptTable {
        if self.dropped != 0 {
            eprintln!(
                "lists: skipped {} JavaScript rules at the table limit",
                self.dropped
            );
        }
        ScriptTable(self.entries.into_iter().map(str::to_owned).collect())
    }
}

pub(crate) struct ScriptTable(Vec<String>);

impl ScriptTable {
    pub(crate) fn resources(&self) -> impl Iterator<Item = Resource> {
        self.0.iter().enumerate().map(|(id, script)| Resource {
            name: format!("{}.js", name(id)),
            aliases: Vec::new(),
            kind: ResourceType::Template,
            content: BASE64_STANDARD.encode(wrap(script)),
            dependencies: Vec::new(),
            permission: PERMISSION,
        })
    }

    pub(crate) fn serialized(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        for script in &self.0 {
            payload.extend_from_slice(script.as_bytes());
            payload.push(b'\n');
        }
        [digest(&SHA256, &payload).as_ref(), &payload].concat()
    }

    pub(crate) fn cached(data: &[u8]) -> Option<Self> {
        let (checksum, payload) = data.split_at_checked(SHA256_OUTPUT_LEN)?;
        if digest(&SHA256, payload).as_ref() != checksum {
            return None;
        }
        let text = std::str::from_utf8(payload).ok()?;
        let mut entries = Vec::new();
        let mut bytes = 0;
        // split_terminator preserves a carriage return for the safety check.
        for script in text.split_terminator('\n') {
            bytes += script.len();
            if bytes > MAX_JS_TABLE_BYTES || entries.len() == MAX_JS_SCRIPTS {
                return None;
            }
            check_body(script).ok()?;
            entries.push(script.to_owned());
        }
        Some(Self(entries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adblock::resources::{InMemoryResourceStorage, ResourceStorage};

    use crate::rule::{Kind, Reason, adguard::Unsupported, read, read_with_scripts};

    #[test]
    fn javascript_rules_and_exceptions_project_to_the_same_invocation() {
        let mut scripts = Scripts::default();
        for (line, expected) in [
            (
                "EXAMPLE.org, ~skip.example.org#%#window.fixture = 1;",
                "example.org,~skip.example.org##+js(abd-js-rule-0)",
            ),
            (
                "except.example.org#@%# window.fixture = 1;",
                "except.example.org#@#+js(abd-js-rule-0)",
            ),
        ] {
            let rule = read_with_scripts(line, Some(&mut scripts));
            assert!(matches!(rule.kind, Kind::Cosmetic));
            assert_eq!(rule.matcher_input().as_deref(), Some(expected));
        }
        assert_eq!(scripts.finish().resources().count(), 1);
    }

    #[test]
    fn unusable_javascript_rules_keep_their_reason_and_do_not_fill_the_table() {
        use Rejection::*;
        use Unsupported::{JavaScript, Matcher};
        let long = format!("site.test#%#//{}", "x".repeat(MAX_JS_RULE));
        let mut scripts = Scripts::default();
        for (line, reason) in [
            ("#%#window.fixture = 1;", JavaScript(NoDomains)),
            ("#@%#window.fixture = 1;", JavaScript(NoDomains)),
            ("site.test#%#", JavaScript(Empty)),
            (
                "site.test#%#window.fixture = '</ScRiPt>';",
                JavaScript(Unsafe),
            ),
            ("site.test#%#window.fixture = '<!--';", JavaScript(Unsafe)),
            ("site.test#%#window.fixture = '\0';", JavaScript(Unsafe)),
            (
                "site.test#%#window.fixture = '{{1}}';",
                JavaScript(Template),
            ),
            (&long, JavaScript(Length)),
            ("~skip.test#%#window.fixture = 1;", Matcher),
            ("site.test,~/regex/#%#window.fixture = 1;", Matcher),
            ("site.test,~skip.test#@%#window.fixture = 1;", Matcher),
        ] {
            let rule = read_with_scripts(line, Some(&mut scripts));
            assert!(
                matches!(rule.kind, Kind::NotApplied(Reason::AdGuard(actual)) if actual == reason),
                "{line}"
            );
            assert!(rule.matcher_input().is_none());
        }
        assert!(scripts.entries.is_empty());
        assert!(check_body(&format!("//{}", "x".repeat(MAX_JS_RULE - 2))).is_ok());
    }

    #[test]
    fn javascript_rules_and_exceptions_require_permission_to_enter_the_table() {
        for line in [
            "site.test#%#window.fixture = 1;",
            "site.test#@%#window.fixture = 1;",
        ] {
            let rule = read(line);
            assert!(matches!(
                rule.kind,
                Kind::NotApplied(Reason::AdGuard(Unsupported::JavaScript(
                    Rejection::Untrusted
                )))
            ));
            assert!(rule.matcher_input().is_none());
        }
    }

    #[test]
    fn javascript_resources_require_their_own_permission_and_cannot_redirect() {
        let mut scripts = Scripts::default();
        let body = "window.fixture = 1;";
        assert!(matches!(
            read_with_scripts("site.test#%#window.fixture = 1;", Some(&mut scripts)).kind,
            Kind::Cosmetic
        ));
        let resources = scripts.finish();
        let storage = ResourceStorage::from_backend(InMemoryResourceStorage::from_resources(
            resources.resources(),
        ));
        for permission in [PermissionMask::default(), PermissionMask::from_bits(1)] {
            assert!(
                storage
                    .get_scriptlet_resources([("abd-js-rule-0", permission)])
                    .is_empty()
            );
        }
        assert!(
            storage
                .get_scriptlet_resources([("abd-js-rule-0", PERMISSION)])
                .contains(body)
        );
        assert!(storage.get_redirect_resource("abd-js-rule-0.js").is_none());
    }

    #[test]
    fn native_rules_cannot_spell_a_reserved_resource_name() {
        for line in [
            "site.test##+js(abd-js-rule-0)",
            "site.test#?#+js( 'abd-js-rule-0')",
            "site.test#@#+js(\"abd-js-rule-0.js\")",
            "site.test#@?#+js(`abd-js-rule-0`, ignored)",
        ] {
            let rule = read(line);
            assert!(matches!(
                rule.kind,
                Kind::NotApplied(Reason::AdGuard(Unsupported::JavaScript(
                    Rejection::ReservedName
                )))
            ));
            assert!(rule.matcher_input().is_none());
        }
        assert!(
            read("site.test##+js(set-constant, abd-js-rule-0, true)")
                .matcher_input()
                .is_some()
        );
    }

    #[test]
    fn the_count_limit_drops_new_bodies_but_keeps_references_to_existing_ones() {
        let rules: Vec<_> = (0..=MAX_JS_SCRIPTS)
            .map(|n| format!("site.test#%#window.fixture = {n};"))
            .collect();
        let mut scripts = Scripts::default();
        for line in &rules[..MAX_JS_SCRIPTS] {
            assert!(matches!(
                read_with_scripts(line, Some(&mut scripts)).kind,
                Kind::Cosmetic
            ));
        }
        let extra = read_with_scripts(&rules[MAX_JS_SCRIPTS], Some(&mut scripts));
        assert!(matches!(
            extra.kind,
            Kind::NotApplied(Reason::AdGuard(Unsupported::JavaScript(Rejection::Count)))
        ));
        let exception = read_with_scripts(
            "except.site.test#@%#window.fixture = 0;",
            Some(&mut scripts),
        );
        assert!(matches!(exception.kind, Kind::Cosmetic));
        assert_eq!(scripts.dropped, 1);
        assert_eq!(scripts.entries.len(), MAX_JS_SCRIPTS);
    }

    #[test]
    fn the_table_byte_limit_drops_the_rule_that_would_cross_it() {
        let rules: Vec<_> = (0..MAX_JS_TABLE_BYTES / MAX_JS_RULE)
            .map(|n| format!("site.test#%#//{n:04}{}", "x".repeat(MAX_JS_RULE - 6)))
            .collect();
        let mut scripts = Scripts::default();
        for line in &rules {
            assert!(matches!(
                read_with_scripts(line, Some(&mut scripts)).kind,
                Kind::Cosmetic
            ));
        }
        assert_eq!(scripts.bytes, MAX_JS_TABLE_BYTES);
        let extra = read_with_scripts("site.test#%#//extra", Some(&mut scripts));
        assert!(matches!(
            extra.kind,
            Kind::NotApplied(Reason::AdGuard(Unsupported::JavaScript(
                Rejection::TableSize
            )))
        ));
        assert_eq!(scripts.dropped, 1);
    }
}

#[cfg(test)]
#[test]
fn a_script_exception_reserves_its_body_at_the_generation_limit() {
    use std::fmt::Write as _;
    let mut text = String::from("site.test#%#window.keep = 1;\n");
    for i in 0..MAX_JS_SCRIPTS - 1 {
        writeln!(text, "other.test#%#window.filler = {i};").unwrap();
    }
    text.push_str("site.test#%#window.exempt = 1;\nsite.test#@%#window.exempt = 1;\n");
    let directory = tempfile::tempdir().unwrap();
    let rules = directory.path().join("rules");
    std::fs::create_dir(&rules).unwrap();
    std::fs::write(rules.join("user.txt"), &text).unwrap();
    let filters = crate::filter::Filters::load(&[&rules], false).unwrap();
    let script = filters.hiding("https://site.test/").script.unwrap();
    assert!(script.contains("window.keep = 1;"));
    assert!(!script.contains("window.exempt = 1;"));

    let report = crate::rulecheck::check(&text, crate::rule::SourcePermissions::USER);
    assert_eq!(report.issues.len(), 1);
    assert!(report.issues[0].text.starts_with("other.test#%#"));
}
