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

//! Dependency behavior relied on by the engine's projections and diagnostics.

use std::collections::BTreeSet;

use adblock::Engine;
use adblock::filters::cosmetic::{CosmeticFilter, CosmeticFilterError};
use adblock::filters::network::{NetworkFilterError, NetworkFilterMask as Mask};
use adblock::lists::{FilterParseError, FilterSet, ParseOptions, ParsedLine, parse_filter};
use adblock::request::Request;
use adblock::resources::{MimeType, PermissionMask, Resource, ResourceType};
use adblock::utils::fast_hash;
use base64::{Engine as _, prelude::BASE64_STANDARD};

const URL: &str = "https://ads.example/banner.js";
const PAGE: &str = "https://reader.example/";

fn engine(rules: &str) -> Engine {
    crate::public_suffix::install();
    let mut set = FilterSet::new(false);
    set.add_filter_list(rules.to_owned(), ParseOptions::default());
    Engine::new_with_filter_set(set)
}

fn request() -> Request {
    crate::public_suffix::install();
    Request::new(URL, PAGE, "script", "GET").unwrap()
}

fn resource(name: &str, body: &str, kind: ResourceType, permission: PermissionMask) -> Resource {
    Resource {
        name: name.to_owned(),
        aliases: Vec::new(),
        kind,
        content: BASE64_STANDARD.encode(body),
        dependencies: Vec::new(),
        permission,
    }
}

fn template(name: &str, body: &str) -> Resource {
    resource(
        name,
        body,
        ResourceType::Template,
        PermissionMask::default(),
    )
}

// filter/app_rules.rs and app_patterns.rs append synthetic scope hashes.
// Checking only the first hash would miss app groups and matching app regexes.
#[test]
fn a_positive_source_domain_can_match_any_request_source_hash() {
    let matcher = engine("||ads.example/banner.js$domain=source-one|source-three");
    let mut req = request();
    for hosts in [
        ["source-one", "other"],
        ["other", "source-three"],
        ["source-three", "source-one"],
    ] {
        req.source_hostname_hashes = Some(hosts.into_iter().map(fast_hash).collect());
        assert!(
            matcher.check_network_request(&req).should_block(),
            "{hosts:?}"
        );
    }
    req.source_hostname_hashes = Some(vec![fast_hash("other"), fast_hash("source-two")]);
    assert!(!matcher.check_network_request(&req).should_block());
}

// filter/app_rules.rs and app_patterns.rs also project app exclusions.
// Ignoring a negative hash after the first one would block an excluded app.
#[test]
fn a_negative_source_domain_excludes_a_request_on_any_source_hash() {
    let matcher = engine("||ads.example/banner.js$domain=~source-one|~source-three");
    let mut req = request();
    req.source_hostname_hashes = Some(vec![fast_hash("other"), fast_hash("source-two")]);
    assert!(matcher.check_network_request(&req).should_block());
    for hosts in [
        ["source-one", "other"],
        ["other", "source-three"],
        ["source-three", "source-one"],
    ] {
        req.source_hostname_hashes = Some(hosts.into_iter().map(fast_hash).collect());
        assert!(
            !matcher.check_network_request(&req).should_block(),
            "{hosts:?}"
        );
    }
}

// filter/response_rules.rs encodes every action ID as a distinct CSP value.
// A first-match-only query or a different separator would lose response rules.
#[test]
fn the_csp_query_returns_all_matching_directives_separated_by_commas() {
    let matcher = engine(concat!(
        "||ads.example^$csp=script-src 'none'\n",
        "||ads.example^$csp=img-src 'none'\n",
        "||ads.example^$csp=frame-src 'none'\n",
        "||other.example^$csp=connect-src 'none'\n",
    ));
    for kind in ["document", "subdocument"] {
        let req = Request::new(URL, PAGE, kind, "GET").unwrap();
        let directives = matcher.get_csp_directives(&req).unwrap();
        assert_eq!(
            directives.split(',').collect::<BTreeSet<_>>(),
            BTreeSet::from(["script-src 'none'", "img-src 'none'", "frame-src 'none'"]),
        );
        assert_eq!(directives.split(',').count(), 3);
    }
    assert!(matcher.get_csp_directives(&request()).is_none());
}

// filter.rs combines real CSP rules and their exceptions. Suppressing by
// anything broader than the directive would also remove sibling policies.
#[test]
fn a_csp_exception_removes_only_the_directive_with_the_same_value() {
    let matcher = engine(concat!(
        "||ads.example^$csp=img-src 'none'\n",
        "||ads.example^$csp=script-src 'none'\n",
        "@@||ads.example^$csp=script-src 'none'\n",
    ));
    let req = Request::new(URL, PAGE, "document", "GET").unwrap();
    assert_eq!(
        matcher.get_csp_directives(&req).as_deref(),
        Some("img-src 'none'")
    );
}

// filter.rs relies on a valueless CSP exception to keep an exempt response's
// existing policy intact even when multiple injected policies would match.
#[test]
fn a_valueless_csp_exception_suppresses_every_matching_directive() {
    let matcher = engine(concat!(
        "||ads.example^$csp=img-src 'none'\n",
        "||ads.example^$csp=script-src 'none'\n",
        "@@||ads.example^$csp\n",
    ));
    let req = Request::new(URL, PAGE, "document", "GET").unwrap();
    assert!(matcher.get_csp_directives(&req).is_none());
}

// rule/denyallow.rs rewrites excluded destinations into negated $to entries.
// Substring or exact-only matching would exempt the wrong requests.
#[test]
fn a_negative_destination_excludes_the_host_and_its_subdomains_only() {
    let matcher = engine("banner.js$domain=reader.example,to=~exempt.example");
    for (host, blocked) in [
        ("exempt.example", false),
        ("a.exempt.example", false),
        ("notexempt.example", true),
        ("exempt.example.other", true),
        ("example", true),
        ("unrelated.example", true),
    ] {
        let url = format!("https://{host}/banner.js");
        let req = Request::new(&url, PAGE, "script", "GET").unwrap();
        assert_eq!(
            matcher.check_network_request(&req).should_block(),
            blocked,
            "{host}"
        );
    }
}

// filter.rs adds lowercase host hashes before matching because supplying
// an uppercase URL alone misses lowercase $to and $domain entries.
#[test]
fn request_hostname_hashes_use_the_supplied_ascii_case() {
    let matcher = engine("banner.js$domain=reader.example,to=~exempt.example");
    let mut mixed =
        Request::new("https://EXEMPT.example/banner.js", PAGE, "script", "GET").unwrap();
    assert_eq!(mixed.hostname, "EXEMPT.example");
    assert!(
        mixed
            .hostname_hashes
            .as_ref()
            .unwrap()
            .contains(&fast_hash("EXEMPT.example"))
    );
    assert!(matcher.check_network_request(&mixed).should_block());

    mixed
        .hostname_hashes
        .as_mut()
        .unwrap()
        .push(fast_hash("exempt.example"));
    assert!(!matcher.check_network_request(&mixed).should_block());
    assert!(mixed.url.contains("EXEMPT.example"));
}

// rule/denyallow.rs uses serialized length to detect parsed negative-only
// catch-all rules dropped by the compiler. A different distinction would
// make the rule checker claim such rules were applied.
#[test]
fn negative_only_destination_catchalls_compile_to_an_empty_matcher() {
    let empty = Engine::new_with_parsed_rules(vec![], vec![])
        .serialize()
        .len();
    for (line, discarded) in [
        ("*$script,to=~exempt.example", true),
        ("*$script,domain=reader.example,to=~exempt.example", false),
    ] {
        let ParsedLine::Network(rule) = parse_filter(line, false, ParseOptions::default()).unwrap()
        else {
            panic!("not a network rule: {line}");
        };
        let matcher = Engine::new_with_parsed_rules(vec![rule], vec![]);
        assert_eq!(matcher.serialize().len() == empty, discarded, "{line}");
        assert_eq!(
            matcher.check_network_request(&request()).should_block(),
            !discarded
        );
    }
}

// filter/redirects.rs decodes the MIME and standard base64 from this precise
// data URL shape. A resource name or bare payload would become a lost redirect.
#[test]
fn a_redirect_result_is_a_mime_typed_standard_base64_data_url() {
    let mut matcher = engine("||ads.example^$redirect=fixture.txt");
    matcher.use_resources([resource(
        "fixture.txt",
        "\u{fffd}?",
        ResourceType::Mime(MimeType::TextPlain),
        PermissionMask::default(),
    )]);
    let result = matcher.check_network_request(&request());
    assert_eq!(
        result.redirect.as_deref(),
        Some("data:text/plain;base64,77+9Pw==")
    );
}

// filter/redirects.rs relies on the matcher to arbitrate between app and web
// redirect candidates. A lower priority or an exact named exception winning
// after the fact would send a different response body.
#[test]
fn redirect_priorities_and_named_exceptions_are_resolved_before_resource_lookup() {
    let rules = concat!(
        "||ads.example^$redirect=low.txt:1\n",
        "||ads.example^$redirect-rule=high.txt:5\n",
    );
    for (exception, expected) in [
        ("", "data:text/plain;base64,aGlnaA=="),
        (
            "@@||ads.example^$redirect-rule=high.txt:5",
            "data:text/plain;base64,bG93",
        ),
    ] {
        let mut matcher = engine(&format!("{rules}{exception}"));
        matcher.use_resources([
            resource(
                "low.txt",
                "low",
                ResourceType::Mime(MimeType::TextPlain),
                PermissionMask::default(),
            ),
            resource(
                "high.txt",
                "high",
                ResourceType::Mime(MimeType::TextPlain),
                PermissionMask::default(),
            ),
        ]);
        assert_eq!(
            matcher
                .check_network_request(&request())
                .redirect
                .as_deref(),
            Some(expected)
        );
    }
}

// filter/redirects.rs projects a bare redirect exception to an important
// rule and uses this flag to suppress the remaining redirect candidates.
#[test]
fn an_important_match_is_reported_even_when_a_redirect_candidate_exists() {
    let mut matcher = engine(concat!(
        "||ads.example^$redirect-rule=fixture.txt\n",
        "||ads.example/banner.js$important\n",
    ));
    matcher.use_resources([resource(
        "fixture.txt",
        "mock",
        ResourceType::Mime(MimeType::TextPlain),
        PermissionMask::default(),
    )]);
    let result = matcher.check_network_request(&request());
    assert!(result.important);
    assert!(result.redirect.is_some());
}

// filter/removeparam.rs matches the rewritten query against the original.
// Normalizing case, percent escapes, empty values, or fragments would prevent
// the engine from restoring just the parameters exempted by local rules.
#[test]
fn removeparam_returns_the_absolute_url_and_preserves_the_other_url_bytes() {
    let matcher = engine("||ads.example^$removeparam=ad\n||ads.example^$removeparam=other");
    let url = "https://ads.example/Path?ad=1&keep=%2f&ad=&ad&ad=2&other=3#Part";
    let req = Request::new(url, PAGE, "document", "GET").unwrap();
    assert_eq!(
        matcher.check_network_request(&req).rewritten_url.as_deref(),
        Some("https://ads.example/Path?keep=%2f&ad=&ad#Part"),
    );
    let no_change = Request::new(
        "https://ads.example/Path?ad=&ad#Part",
        PAGE,
        "document",
        "GET",
    )
    .unwrap();
    assert!(
        matcher
            .check_network_request(&no_change)
            .rewritten_url
            .is_none()
    );
}

// rule/javascript.rs puts complete JavaScript bodies in argumentless templates.
// Template expansion with zero arguments must leave the code bytes intact.
#[test]
fn an_argumentless_template_is_injected_with_its_body_verbatim() {
    let mut matcher = engine("ads.example##+js(fixture)");
    let body = "  (() => {\n    window.fixture = 'unchanged';\n  })();\n";
    matcher.use_resources([template("fixture.js", body)]);
    let result = matcher.url_cosmetic_resources(URL);
    assert_eq!(
        result.injected_script,
        format!("try {{\n{body}\n}} catch ( e ) {{ }}\n")
    );
}

// rule/javascript.rs and rule/permissions.rs compile trusted rules separately.
// Ignoring the filter mask or treating another bit as equivalent would let an
// unrelated list invoke trusted JavaScript.
#[test]
fn a_custom_resource_permission_is_checked_against_the_compiled_filter_bit() {
    let required = PermissionMask::from_bits(2);
    let body = "window.trustedFixture = true;";
    for (bits, allowed) in [(0, false), (1, false), (2, true), (3, true)] {
        let mut set = FilterSet::new(false);
        set.add_filter_list(
            "ads.example##+js('trusted-fixture')".to_owned(),
            ParseOptions {
                permissions: PermissionMask::from_bits(bits),
                ..ParseOptions::default()
            },
        );
        let mut matcher = Engine::new_with_filter_set(set);
        // The startup cache must also preserve the permission compiled into the rule.
        let mut restored = Engine::default();
        restored.deserialize(&matcher.serialize()).unwrap();
        for engine in [&mut matcher, &mut restored] {
            engine.use_resources([resource(
                "trusted-fixture.js",
                body,
                ResourceType::Template,
                required,
            )]);
            let script = engine.url_cosmetic_resources(URL).injected_script;
            assert_eq!(script.contains(body), allowed, "filter bits {bits}");
            assert_eq!(script.is_empty(), !allowed, "filter bits {bits}");
        }
    }
}

// rule/javascript.rs shares resource storage with subscription scriptlets.
// One unknown name must not suppress otherwise valid code for the page.
#[test]
fn a_missing_scriptlet_resource_does_not_prevent_the_other_injections() {
    let mut matcher = engine(concat!(
        "ads.example##+js(missing-fixture)\n",
        "ads.example##+js(first-fixture)\n",
        "ads.example##+js(second-fixture)\n",
    ));
    matcher.use_resources([
        template("first-fixture.js", "window.firstFixture = 1;"),
        template("second-fixture.js", "window.secondFixture = 2;"),
    ]);
    let script = matcher.url_cosmetic_resources(URL).injected_script;
    assert!(script.contains("window.firstFixture = 1;"));
    assert!(script.contains("window.secondFixture = 2;"));
    assert!(!script.contains("missing-fixture"));
}

// rule/adguard/arguments.rs and scriptlets.rs rely on the matcher's quoting
// and escape handling to deliver the translated argument values to helpers.
#[test]
fn emitted_scriptlet_argument_forms_round_trip_to_the_individual_arguments() {
    let line = concat!(
        "ads.example##+js(capture, '', 'a,b', \"it's, ok\", ",
        "`single' and double\", with comma`, no'\"`\u{5c},comma, tail\u{5c} , last)",
    );
    let mut matcher = engine(line);
    matcher.use_resources([template("capture.js", "function capture() {}")]);
    let script = matcher.url_cosmetic_resources(URL).injected_script;
    let args = [
        "",
        "a,b",
        "it's, ok",
        "single' and double\", with comma",
        "no'\"`,comma",
        "tail\\",
        "last",
    ];
    let expected = args
        .map(|arg| serde_json::to_string(arg).unwrap())
        .join(", ");
    assert!(
        script.contains(&format!("capture({expected})")),
        "{script:?}"
    );
}

// filter/app_rules.rs checks exceptions in an engine containing no blocks,
// including after a web block was found in a different engine.
#[test]
fn the_previously_matched_subset_can_return_an_exception_from_an_exception_only_engine() {
    let matcher = engine("@@||ads.example^$script");
    let req = request();
    assert!(matcher.check_network_request(&req).exception.is_none());
    for force in [false, true] {
        let result = matcher.check_network_request_subset(&req, true, force);
        assert!(result.filter.is_some());
        assert!(result.exception.is_some());
        assert!(!result.should_block());
    }
}

// filter.rs and filter/page_exceptions.rs consult an exception-only engine
// without a previously matched rule. Losing the exception would disable the guard.
#[test]
fn a_forced_subset_reports_an_exception_even_with_no_previous_match() {
    let matcher = engine("@@||ads.example^$script");
    let result = matcher.check_network_request_subset(&request(), false, true);
    assert!(result.filter.is_none());
    assert!(result.exception.is_some());
    assert!(!result.should_block());
}

// filter/load.rs, host_rules.rs and redirects.rs compare parsed IDs to skip
// disabled projections. The disabling modifier must preserve the rule ID.
#[test]
fn badfilter_and_its_target_share_an_id() {
    let parse = |line| {
        let ParsedLine::Network(rule) = parse_filter(line, false, ParseOptions::default()).unwrap()
        else {
            panic!("not a network rule: {line}");
        };
        (rule.get_id(), rule.is_badfilter())
    };
    let (active, bad) = parse("||ads.example^$script,redirect=fixture.txt");
    assert!(!bad);
    let (disabled, bad) = parse("||ads.example^$script,redirect=fixture.txt,badfilter");
    assert!(bad);
    assert_eq!(disabled, active);
}

// filter/host_rules.rs needs hostname, separator mask and path as separate
// pieces when building conservative guards for encrypted connections.
#[test]
fn parsed_hostname_anchors_expose_the_path_and_its_left_anchor_separately() {
    crate::public_suffix::install();
    let ParsedLine::Network(rule) =
        parse_filter("@@||ads.example/allowed", false, ParseOptions::default()).unwrap()
    else {
        panic!("not a network rule");
    };
    assert_eq!(rule.hostname.as_deref(), Some("ads.example"));
    assert!(rule.mask.contains(Mask::IS_LEFT_ANCHOR));
    assert!(!rule.mask.contains(Mask::IS_HOSTNAME_REGEX));
    assert_eq!(rule.filter.iter().collect::<Vec<_>>(), ["/allowed"]);
}

// filter/host_rules.rs selects HTTPS guards from the parsed destination
// list. Reinterpreting its hashes or exposing discarded TLDs as destinations
// would make those guards miss the hosts or claim the wrong coverage.
#[test]
fn parsed_destination_hashes_keep_specific_hosts_and_drop_mixed_broad_positives() {
    crate::public_suffix::install();
    let line = "@@banner.js$to=exempt.example|com|~other.example";
    let ParsedLine::Network(rule) = parse_filter(line, false, ParseOptions::default()).unwrap()
    else {
        panic!("not a network rule: {line}");
    };
    assert!(rule.mask.contains(Mask::FROM_HTTPS));
    assert_eq!(
        rule.opt_to_domains.as_deref(),
        Some([fast_hash("exempt.example")].as_slice())
    );
    assert_eq!(
        rule.opt_not_to_domains.as_deref(),
        Some([fast_hash("other.example")].as_slice())
    );
}

// filter/matcher.rs stores originals by source index and zero-based list line.
// Compacting away skipped lines would show the wrong original rule in diagnostics.
#[test]
fn unoptimized_debug_matches_preserve_list_indexes_and_uncompacted_line_numbers() {
    crate::public_suffix::install();
    let mut set = FilterSet::new(true);
    let first = set.add_filter_list("||other.example^".to_owned(), ParseOptions::default());
    let second = set.add_filter_list(
        "! comment\n\n||ads.example^$unknown-option\n||ads.example/banner.js\n".to_owned(),
        ParseOptions::default(),
    );
    assert_ne!(first.source_index, second.source_index);
    let matcher = Engine::new_with_filter_set_no_optimize(set);
    let matched = matcher.check_network_request(&request()).filter.unwrap();
    let location = matched.source_location.unwrap();
    assert_eq!(location.source_index as usize, second.source_index);
    assert_eq!(location.line_number, 3);
    assert_eq!(matched.raw_line.as_deref(), Some("||ads.example/banner.js"));
}

// filter.rs sends an empty source hostname for an unnamed page. If it
// became the target or an invented hostname, domain restrictions would shift.
#[test]
fn a_preparsed_request_without_a_source_has_no_hashes_for_named_page_rules() {
    let url = "https://ads.example/banner.js";
    let req = Request::preparsed(url, "ads.example", "", "script", false, "GET");
    assert!(req.source_hostname_hashes.is_none());
    for (condition, blocked) in [("ads.example", false), ("~ads.example", true)] {
        let matcher = engine(&format!("||ads.example/banner.js$domain={condition}"));
        assert_eq!(matcher.check_network_request(&req).should_block(), blocked);
    }
}

// filter/response_rules.rs passes the original method and party relation into
// Request::preparsed. Losing either would apply response actions outside scope.
#[test]
fn preparsed_requests_use_the_supplied_method_and_party_relation() {
    let matcher = engine("||ads.example/banner.js$method=post,third-party");
    for (method, third_party, blocked) in [
        ("POST", true, true),
        ("GET", true, false),
        ("POST", false, false),
    ] {
        let req = Request::preparsed(
            URL,
            "ads.example",
            "reader.example",
            "script",
            third_party,
            method,
        );
        assert_eq!(matcher.check_network_request(&req).should_block(), blocked);
    }
}

// filter/page_exceptions.rs and filter.rs reuse the matcher exception set for
// streamed generic selectors. Ignoring the set hides elements a page exempts.
#[test]
fn url_cosmetic_exceptions_are_understood_by_generic_class_and_id_lookup() {
    let matcher = engine("##.advert\n###banner\nads.example#@#.advert");
    let assets = matcher.url_cosmetic_resources(URL);
    assert!(assets.exceptions.contains(".advert"));
    let selectors = matcher.hidden_class_id_selectors(
        ["advert"].iter().copied(),
        ["banner"].iter().copied(),
        &assets.exceptions,
    );
    assert!(selectors.iter().any(|s| s == "#banner"));
    assert!(!selectors.iter().any(|s| s == ".advert"));
}

// filter.rs passes the matcher JSON to cosmetic/procedural.rs. Field or
// operator spelling changes would stop procedural rules from reaching the page.
#[test]
fn procedural_cosmetic_rules_have_the_selector_and_action_json_the_page_consumes() {
    let matcher = engine("ads.example##div:has-text(sponsor):remove()");
    let assets = matcher.url_cosmetic_resources(URL);
    let mut actions = assets.procedural_actions.iter();
    let action: serde_json::Value = serde_json::from_str(actions.next().unwrap()).unwrap();
    assert_eq!(
        action,
        serde_json::json!({
            "selector": [{"type": "css-selector", "arg": "div"}, {"type": "has-text", "arg": "sponsor"}],
            "action": {"type": "remove"},
        })
    );
    assert!(actions.next().is_none());
}

// rule/check.rs uses these variants to distinguish syntax, scope, and rules
// the matcher does not implement. Each test keeps one diagnostic input stable.
macro_rules! parse_error {
    ($name:ident, $line:expr, $error:pat) => {
        #[test]
        fn $name() {
            crate::public_suffix::install();
            let error = parse_filter($line, false, ParseOptions::default()).err();
            assert!(
                matches!(error, Some($error)),
                "input {:?} gave {error:?}",
                $line
            );
        }
    };
}

parse_error!(
    a_comment_is_unsupported_by_the_rule_parser,
    "! comment",
    FilterParseError::Unsupported
);
parse_error!(
    whitespace_alone_reports_an_empty_rule,
    " \t",
    FilterParseError::Empty
);
parse_error!(
    an_unknown_network_option_reports_unrecognised_option,
    "||ads.example^$unknown-option",
    FilterParseError::Network(NetworkFilterError::UnrecognisedOption)
);
parse_error!(
    regex_only_source_domains_report_no_supported_domains,
    r"banner.js$domain=/example\.org/",
    FilterParseError::Network(NetworkFilterError::NoSupportedDomains)
);
parse_error!(
    match_case_on_a_plain_filter_reports_the_full_regex_requirement,
    "banner.js$match-case",
    FilterParseError::Network(NetworkFilterError::MatchCaseWithoutFullRegex)
);
parse_error!(
    a_valueless_redirect_exception_reports_empty_redirection,
    "@@||ads.example^$redirect",
    FilterParseError::Network(NetworkFilterError::EmptyRedirection)
);
parse_error!(
    a_removeparam_exception_reports_unsupported_exception,
    "@@||ads.example^$removeparam=ad",
    FilterParseError::Network(NetworkFilterError::RemoveparamWithException)
);
parse_error!(
    a_csp_rule_with_content_type_reports_the_incompatible_modifier,
    "||ads.example^$script,csp=img-src 'none'",
    FilterParseError::Network(NetworkFilterError::CspWithContentType)
);
parse_error!(
    adguard_scriptlet_form_reports_unsupported_cosmetic_syntax,
    "ads.example#%#x()",
    FilterParseError::Cosmetic(CosmeticFilterError::UnsupportedSyntax)
);
parse_error!(
    html_filtering_reports_its_own_unsupported_variant,
    "ads.example##^script",
    FilterParseError::Cosmetic(CosmeticFilterError::HtmlFilteringUnsupported)
);
parse_error!(
    cosmetic_location_modifiers_report_their_unsupported_variant,
    "[foo]ads.example##.advert",
    FilterParseError::Cosmetic(CosmeticFilterError::LocationModifiersUnsupported)
);
parse_error!(
    procedural_selector_lists_report_the_multiple_selectors_variant,
    "ads.example##div:has-text(ad), span",
    FilterParseError::Cosmetic(CosmeticFilterError::ProceduralFilterWithMultipleSelectors)
);
parse_error!(
    a_generic_unhide_reports_missing_scope,
    "#@#.advert",
    FilterParseError::Cosmetic(CosmeticFilterError::GenericUnhide)
);
parse_error!(
    a_generic_scriptlet_reports_missing_scope,
    "##+js(fixture)",
    FilterParseError::Cosmetic(CosmeticFilterError::GenericScriptInject)
);
parse_error!(
    a_generic_remove_action_reports_missing_scope,
    "##.advert:remove()",
    FilterParseError::Cosmetic(CosmeticFilterError::GenericAction)
);
parse_error!(
    a_negated_domain_on_an_unhide_reports_double_negation,
    "~ads.example#@#.advert",
    FilterParseError::Cosmetic(CosmeticFilterError::DoubleNegation)
);
parse_error!(
    an_unterminated_scriptlet_argument_reports_invalid_scriptlet_args,
    "ads.example##+js(fixture, 'bad)",
    FilterParseError::Cosmetic(CosmeticFilterError::InvalidScriptletArgs)
);
parse_error!(
    an_invalid_idna_cosmetic_domain_reports_punycode_error,
    "ads.\u{fffd}##.advert",
    FilterParseError::Cosmetic(CosmeticFilterError::PunycodeError)
);
parse_error!(
    an_unclosed_cosmetic_action_reports_invalid_action_specifier,
    "ads.example##.advert:style(color: red",
    FilterParseError::Cosmetic(CosmeticFilterError::InvalidActionSpecifier)
);
parse_error!(
    a_forbidden_style_value_reports_invalid_css_style,
    "ads.example##.advert:style(background: url(x))",
    FilterParseError::Cosmetic(CosmeticFilterError::InvalidCssStyle)
);
parse_error!(
    an_unclosed_attribute_selector_reports_invalid_css_selector,
    "ads.example##.advert[",
    FilterParseError::Cosmetic(CosmeticFilterError::InvalidCssSelector)
);
parse_error!(
    an_empty_cosmetic_body_reports_empty_rule,
    "ads.example##",
    FilterParseError::Cosmetic(CosmeticFilterError::EmptyRule)
);

// rule/check.rs handles this variant, although parse_filter classifies lines
// without a marker as network rules before calling the cosmetic parser.
#[test]
fn a_cosmetic_parse_without_a_marker_reports_missing_sharp() {
    let error = CosmeticFilter::parse(".advert", false, PermissionMask::default()).err();
    assert!(
        matches!(error, Some(CosmeticFilterError::MissingSharp)),
        "{error:?}"
    );
}

// rule/regex.rs prechecks full regexes exposed by stripping the noop modifier.
// The matcher accepts the text at parse time and only rejects it when matching.
#[test]
fn invalid_full_network_regexes_parse_but_never_match() {
    crate::public_suffix::install();
    let line = "/[:broken/";
    assert!(matches!(
        parse_filter(line, false, ParseOptions::default()),
        Ok(ParsedLine::Network(_))
    ));
    assert!(
        !engine(line)
            .check_network_request(&request())
            .should_block()
    );
}
