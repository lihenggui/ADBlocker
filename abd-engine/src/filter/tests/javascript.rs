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

use std::{fs, path::Path};

use super::*;
use crate::list_url::{MOVED, file_name};

const BODY: &str = "window.adbJavaScript = 42;";
const URL: &str = "https://site.test/";

fn user_rules(dir: &Path, text: &str) -> std::path::PathBuf {
    let rules = dir.join("rules");
    fs::create_dir(&rules).unwrap();
    fs::write(rules.join("user.txt"), text).unwrap();
    rules
}

#[test]
fn only_marked_catalog_lists_and_user_rules_can_supply_javascript() {
    let text = format!("site.test#%#{BODY}\n");
    let temp = tempfile::tempdir().unwrap();
    let lists = temp.path().join("lists");
    fs::create_dir(&lists).unwrap();
    let rules = user_rules(temp.path(), &text);
    assert!(
        Filters::load(&[&rules], true)
            .unwrap()
            .hiding(URL)
            .script
            .is_some()
    );

    for &(previous, current) in MOVED {
        let old = lists.join(file_name(previous));
        let new = lists.join(file_name(current));
        fs::write(&old, &text).unwrap();
        assert_eq!(
            crate::sources::active_copy(&lists, current),
            (previous, old.clone())
        );
        let load = || Filters::load(&[&lists], true).unwrap();
        let untrusted = load();
        assert!(untrusted.hiding(URL).script.is_none());
        assert_eq!(untrusted.scripts.resources().count(), 0);

        fs::write(&new, &text).unwrap();
        assert_eq!(
            crate::sources::active_copy(&lists, current),
            (current, new.clone())
        );
        let trusted = load();
        assert!(trusted.hiding(URL).script.unwrap().contains(BODY));
        fs::remove_file(old).unwrap();
        fs::remove_file(new).unwrap();
    }
    for url in [
        "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/filters.txt",
        "https://example.org/custom.txt",
    ] {
        let list = lists.join(file_name(url));
        fs::write(&list, &text).unwrap();
        let untrusted = Filters::load(&[&lists], true).unwrap();
        assert!(untrusted.hiding(URL).script.is_none(), "{url}");
        assert_eq!(untrusted.scripts.resources().count(), 0);
        fs::remove_file(list).unwrap();
    }
}

#[test]
fn only_the_shared_script_is_excepted_and_native_rules_cannot_reach_it() {
    let dir = tempfile::tempdir().unwrap();
    let rules = user_rules(
        dir.path(),
        &format!(
            "except.site.test#@%#{BODY}\n\
         site.test,~skip.site.test#%#{BODY}\n\
         other.test#%#{BODY}\n\
         site.test#%#window.other = true;\n\
         site.test#@#+js(abd-js-rule-0)\n\
         forged.test##+js(abd-js-rule-0)\n"
        ),
    );
    let filters = Filters::load(&[&rules], true).unwrap();
    assert_eq!(filters.scripts.resources().count(), 2);
    for (host, first, second) in [
        ("site.test", true, true),
        ("skip.site.test", false, true),
        ("except.site.test", false, true),
        ("other.test", true, false),
        ("forged.test", false, false),
    ] {
        let script = filters
            .hiding(&format!("https://{host}/"))
            .script
            .unwrap_or_default();
        assert_eq!(script.matches(BODY).count(), usize::from(first), "{host}");
        assert_eq!(script.contains("window.other = true;"), second, "{host}");
    }
}

#[test]
fn cached_javascript_keeps_its_wrapper_and_each_call_has_its_own_try() {
    let dir = tempfile::tempdir().unwrap();
    let thrown = "throw new Error('fixture');";
    let commented = "window.adbJavaScript = 42; // trailing comment";
    let rules = user_rules(
        dir.path(),
        &format!("site.test,one.test#%#{thrown}\nsite.test#%#{commented}\n"),
    );
    let cache = dir.path().join("cache");
    let load = || Filters::load_cached(&[&rules], &cache).unwrap();
    let built = load();
    let key = built.generation.unwrap();
    let kept = Filters::read_cache(&cache, key).unwrap();
    assert_eq!(
        built.hiding("https://one.test/").script,
        kept.hiding("https://one.test/").script
    );
    for filters in [built, kept] {
        let script = filters.hiding(URL).script.unwrap();
        for body in [thrown, commented] {
            let call = format!("try {{\n(function(){{\n{body}\n}})();\n}} catch ( e ) {{ }}\n");
            assert!(script.contains(&call), "{script}");
        }
        assert_eq!(script.matches("try {").count(), 2);
    }
    let script_file = cache.join("scripts");
    let mut corrupt = fs::read(&script_file).unwrap();
    corrupt.truncate(corrupt.len() - commented.len() - 1);
    fs::write(&script_file, corrupt).unwrap();
    assert!(Filters::read_cache(&cache, key).is_none());
    fs::remove_file(script_file).unwrap();
    assert!(Filters::read_cache(&cache, key).is_none());
    assert!(load().hiding(URL).script.unwrap().contains(commented));
    assert!(Filters::read_cache(&cache, key).is_some());
}

#[test]
fn page_script_exceptions_keep_element_hiding_unless_it_is_also_excepted() {
    let dir = tempfile::tempdir().unwrap();
    let rules = user_rules(
        dir.path(),
        &format!(
            "site.test#%#{BODY}\n\
         site.test##+js(set-constant, adbScriptlet, 42)\n\
         site.test##.ad\n\
         site.test##div:has-text(Advert)\n\
         @@||site.test/scripts^$jsinject\n\
         @@||site.test/both^$elemhide,jsinject,extension\n\
         @@||site.test/content^$content,jsinject\n"
        ),
    );
    let cache = dir.path().join("cache");
    let built = Filters::load_cached(&[&rules], &cache).unwrap();
    let kept = Filters::read_cache(&cache, built.generation.unwrap()).unwrap();
    for filters in [built, kept] {
        let original = filters.hiding(URL).script.unwrap();
        assert!(original.contains(BODY) && original.contains("adbScriptlet"));

        for path in ["scripts", "content"] {
            let hiding = filters.hiding(&format!("{URL}{path}"));
            assert!(hiding.css.unwrap().contains(".ad"));
            let script = hiding.script.unwrap();
            assert!(script.contains("Advert"));
            assert!(!script.contains(BODY) && !script.contains("adbScriptlet"));
        }
        assert!(
            filters
                .page_exceptions
                .response(&format!("{URL}content"), Page::Itself, "GET")
                .all
        );
        let hiding = filters.hiding(&format!("{URL}both"));
        assert!(hiding.css.is_none() && hiding.script.is_none());
    }
}

#[test]
fn oversized_pages_keep_complete_javascript_calls_that_fit() {
    use crate::limits::MAX_JS_RULE;
    use std::fmt::Write as _;

    let dir = tempfile::tempdir().unwrap();
    let mut text = String::new();
    let value = "x".repeat(MAX_JS_RULE - 128);
    for index in 0..18 {
        writeln!(
            text,
            "site.test#%#window.large{index:02} = '{value}'; // comment"
        )
        .unwrap();
    }
    let small = "window.small = true;";
    writeln!(text, "site.test#%#{small}").unwrap();
    let rules = user_rules(dir.path(), &text);
    let filters = Filters::load(&[&rules], true).unwrap();
    let script = filters.hiding(URL).script.unwrap();
    assert!(script.len() <= 256 << 10);
    assert!(script.contains(small));
    for index in 0..18 {
        let call = format!(
            "try {{\n(function(){{\nwindow.large{index:02} = '{value}'; // comment\n}})();\n}} catch ( e ) {{ }}\n"
        );
        assert_eq!(script.contains(&call), index < 16, "{index}");
    }
    assert_eq!(script.matches("try {").count(), 17);
    assert_eq!(script.matches("} catch ( e ) { }").count(), 17);
    assert!(script.ends_with("\n})();\n"));
}
