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

use super::*;

#[test]
fn matching_parameter_removals_from_web_and_app_rules_survive_cache_reload() {
    let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        dir.path().join("list.txt"),
        "$removeparam=param\n\
        ||example.org^$removeparam=app,app=com.reader\n\
        @@||example.org/except^$removeparam=param",
    )
    .unwrap();
    let built = Filters::load_cached(&[dir.path()], cache.path()).unwrap();
    let cached = Filters::read_cache(cache.path(), built.generation.unwrap()).unwrap();
    for filters in [built, cached] {
        let url = "https://example.org/except?param=1&kept=2%20a&app=3&param=&app";
        for (package, expected) in [
            (
                "com.reader",
                "https://example.org/except?kept=2%20a&param=&app",
            ),
            (
                "com.other",
                "https://example.org/except?kept=2%20a&app=3&param=&app",
            ),
        ] {
            let filters = filters.for_app(package);
            let check = filters.check_with_page(url, Page::Itself, "document", "GET");
            assert!(!check.blocked);
            assert_eq!(check.rewritten_url.as_deref(), Some(expected));
            assert_eq!(
                filters
                    .parameters_for_replacement(url, Page::Itself, "document", "GET")
                    .as_deref(),
                Some(expected),
            );
            let actions = filters.response_actions(url, Page::Itself, "document", "GET");
            assert!(matches!(actions.as_slice(), [action]
                if matches!(action.as_ref(),
                    crate::rule::action::ResponseAction::RemoveParamException(name) if name == "param")));
        }
    }
}
