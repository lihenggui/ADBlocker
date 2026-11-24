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
use super::{Path, prune};
use crate::limits::{MAX_JSON_DEPTH, MAX_JSON_PATH, MAX_JSON_PATH_NESTING, MAX_JSON_VISITS};

fn changed(path: &str, input: &str) -> String {
    let path = Path::parse(path).expect(path);
    String::from_utf8(prune(&path, input.as_bytes(), &|| true).unwrap()).unwrap()
}

#[test]
fn quoted_names_accept_escapes_and_keep_their_whitespace() {
    let input = r#"{ "a'b":1,"c\"d":2,"e\\f":3," g ":4,"g":5}"#;
    assert_eq!(
        changed(r#" $ . [ 'a\'b' , "c\"d", 'e\\f', ' g ' ] "#, input),
        r#"{"g":5}"#
    );
}

#[test]
fn malformed_paths_are_rejected() {
    for path in [
        "",
        "a",
        "$..",
        "$.",
        "$...",
        "$[(@.length-1)]",
        "$[?(@.x)]",
        "$[?(exists x)]",
        "$[?(key-substr x 5)]",
        "$[?(key-eq x \"unterminated)]",
        "$[?(has '$..')]",
        "$[?(has x) extra]",
        "$[?(has a/b)]",
        "$[a,]",
        "$[]",
        "$.foo@",
        "$.foo bar",
        "$['x'",
        "$[1:x]",
        "$[1:2:3:4]",
        "$[?(key-eq x [])]",
    ] {
        assert!(Path::parse(path).is_none(), "{path}");
    }
}

#[test]
fn expressions_and_nested_filter_paths_are_bounded() {
    assert!(Path::parse(&format!("$.{}", "a".repeat(MAX_JSON_PATH))).is_none());
    let nested = |count| format!("${}{}", "[?(has $".repeat(count), ")]".repeat(count));
    assert!(Path::parse(&nested(MAX_JSON_PATH_NESTING)).is_some());
    assert!(Path::parse(&nested(MAX_JSON_PATH_NESTING + 1)).is_none());
}

#[test]
fn unions_remove_original_indexes_without_shifting() {
    assert_eq!(changed("$[1,3,1,-1]", "[0,1,2,3,4,5]"), "[0,2,4]");
    assert_eq!(
        changed(
            "$.*[name,0,'0']",
            r#"{"a":["x","y"],"b":{"name":1,"0":2,"n":3}}"#
        ),
        r#"{"a":["y"],"b":{"n":3}}"#
    );
}

#[test]
fn slices_use_python_bounds_and_allow_multiple_selections() {
    for (path, expected) in [
        ("$[:2,4::2]", "[2,3,5]"),
        ("$[-2:0:-2]", "[0,1,3,5]"),
        ("$[::-2]", "[0,2,4]"),
        ("$[:-1:-1]", "[0,1,2,3,4,5]"),
        ("$[-100:100:2]", "[1,3,5]"),
        ("$[::0]", "[0,1,2,3,4,5]"),
        ("$[::]", "[]"),
        (
            "$[10000000000000000000000::-10000000000000000000000]",
            "[0,1,2,3,4]",
        ),
    ] {
        assert_eq!(changed(path, "[0,1,2,3,4,5]"), expected, "{path}");
    }
}

#[test]
fn recursive_descent_applies_the_filter_at_each_level() {
    assert_eq!(
        changed(
            "$..[?(has ad)]",
            r#"{"a":[{"ad":null},[{"ad":true},{"ok":1}]],"b":{"ad":false},"c":true}"#
        ),
        r#"{"a":[[{"ok":1}]],"c":true}"#
    );
}

#[test]
fn overlapping_recursive_steps_select_against_the_original_tree() {
    assert_eq!(
        changed("$..a..a", r#"{"a":{"a":{"a":1},"ok":2},"ok":3}"#),
        r#"{"a":{"ok":2},"ok":3}"#
    );
}

#[test]
fn has_matches_null_members_and_paths_on_array_children() {
    assert_eq!(
        changed("$[?(has a)]", r#"[{"a":null},["a"],{}]"#),
        r#"[["a"],{}]"#
    );
    assert_eq!(
        changed("$[?(has $[0])]", r#"[[],[null],{"0":0}]"#),
        r#"[[],{"0":0}]"#
    );
}

#[test]
fn equality_is_type_strict_and_compares_numbers_by_value() {
    let cases = [
        ("1", r#"[{"v":true},{"v":"1"},{"v":null},{"v":1.01}]"#),
        (
            "true",
            r#"[{"v":1.0},{"v":10e-1},{"v":"1"},{"v":null},{"v":1.01}]"#,
        ),
        (
            "null",
            r#"[{"v":1.0},{"v":10e-1},{"v":true},{"v":"1"},{"v":1.01}]"#,
        ),
        (
            "'1'",
            r#"[{"v":1.0},{"v":10e-1},{"v":true},{"v":null},{"v":1.01}]"#,
        ),
    ];
    for (literal, expected) in cases {
        assert_eq!(
            changed(
                &format!("$[?(key-eq v {literal})]"),
                r#"[{"v":1.0},{"v":10e-1},{"v":true},{"v":"1"},{"v":null},{"v":1.01}]"#
            ),
            expected,
        );
    }
    assert_eq!(
        changed(
            "$[?(key-eq '$' 9007199254740993)]",
            "[9007199254740992.0,9007199254740993]"
        ),
        "[9007199254740992.0]"
    );
    assert_eq!(changed("$[?(key-eq '$' -0)]", "[-0,0,0e123]"), "[]");
}

#[test]
fn nested_equality_can_match_any_of_the_selected_values() {
    assert_eq!(
        changed(
            r#"$[?(key-eq "$.x[*]" true)]"#,
            r#"[{"x":[false,true]},{"x":[1,"true"]}]"#
        ),
        r#"[{"x":[1,"true"]}]"#
    );
}

#[test]
fn substrings_match_strings_at_named_keys_or_nested_paths() {
    assert_eq!(
        changed(
            "$[?(key-substr type 'nativead')]",
            r#"[{"type":1},{"type":"nativeads"},{"type":"text"}]"#
        ),
        r#"[{"type":1},{"type":"text"}]"#
    );
    assert_eq!(
        changed(
            r#"$[?(key-substr "$.x[*]" 'nativead')]"#,
            r#"[{"x":[null,"a nativead"]},{"x":["text"]}]"#
        ),
        r#"[{"x":["text"]}]"#
    );
    assert_eq!(
        changed(
            "$[?(key-substr '$' 'NativeAd')]",
            r#"["bothNativeAd",null,3,"nativead"]"#
        ),
        r#"[null,3,"nativead"]"#
    );
}

#[test]
fn root_scalars_and_out_of_range_or_absent_members_are_left_alone() {
    let input = " [ 1, 2 ] ";
    for path in ["$", "$.missing", "$[5]", "$[-3]", "$[0].a"] {
        assert_eq!(changed(path, input), input);
    }
    assert_eq!(changed("$.*", " true "), " true ");
}

#[test]
fn document_depth_counts_from_the_original_root_even_in_nested_filters() {
    let wide = |body: &str| {
        format!(
            "{}{}{}",
            "[".repeat(MAX_JSON_DEPTH),
            body,
            "]".repeat(MAX_JSON_DEPTH)
        )
    };
    let input = format!(r#"{{"drop":1,"deep":{}}}"#, wide(r#"{"drop":2}"#));
    let expected = format!(r#"{{"deep":{}}}"#, wide(r#"{"drop":2}"#));
    assert_eq!(changed("$..drop", &input), expected);
    assert_eq!(changed("$[?(has '$..drop')]", &input), input);
}

#[test]
fn exhausting_visits_abandons_the_whole_removal_plan() {
    let path = Path::parse("$..drop").unwrap();
    let input = format!(
        r#"{{"drop":1,"items":[{}0]}}"#,
        "0,".repeat(MAX_JSON_VISITS)
    );
    assert!(prune(&path, input.as_bytes(), &|| true).is_none());
}

#[test]
fn original_member_order_and_number_spellings_survive_pruning() {
    assert_eq!(
        changed(
            "$.drop",
            r#"{"b":1.0,"drop":true,"a":1e3,"big":123456789012345678901234567890,"E":1E+003,"zero":-0}"#
        ),
        r#"{"b":1.0,"a":1e3,"big":123456789012345678901234567890,"E":1E+003,"zero":-0}"#
    );
}

#[test]
fn jsonp_and_bom_keep_their_original_wrappers() {
    for (input, expected) in [
        (
            r#"window.$cb({"drop":1,"keep":2});"#,
            r#"window.$cb({"keep":2});"#,
        ),
        (
            r#"cb({"drop":1,"keep":"fake);("})"#,
            r#"cb({"keep":"fake);("})"#,
        ),
        (
            " \twindow.cb( { \"drop\": 1 } ) ; \n",
            " \twindow.cb( {} ) ; \n",
        ),
        (
            "\u{feff}jQuery123_456({\"drop\":1,\"ok\":2});",
            "\u{feff}jQuery123_456({\"ok\":2});",
        ),
        ("\u{feff}\n{\"drop\":1,\"ok\":2}", "\u{feff}\n{\"ok\":2}"),
    ] {
        assert_eq!(changed("$.drop", input), expected);
    }
}

#[test]
fn invalid_json_and_unsupported_padding_are_passed_on_byte_for_byte() {
    for input in [
        r#"{ "drop": }"#,
        r#"{"drop":1,}"#,
        r#"{"drop":01}"#,
        r#"cb({"drop":1});run();"#,
        r#"a..cb({"drop":1})"#,
        r#"cb["x"]({"drop":1})"#,
        r#"0cb({"drop":1})"#,
        r#"cb({"drop":1}, 2)"#,
    ] {
        assert_eq!(changed("$.drop", input), input);
    }
    let invalid_utf8 = b"\xff";
    assert_eq!(
        prune(&Path::parse("$.drop").unwrap(), invalid_utf8, &|| true).unwrap(),
        invalid_utf8
    );
}

#[test]
fn a_non_matching_path_never_reserializes_the_document() {
    let input = " cb({ \"keep\":  1e3 }) ; ";
    assert_eq!(changed("$.missing", input), input);
}

#[test]
fn object_removal_uses_decoded_keys_and_preserves_source_order() {
    assert_eq!(
        changed(
            "$.parent.drop",
            r#"{"z":1,"parent":{"stay":2,"drop":3,"all":4},"a":5}"#
        ),
        r#"{"z":1,"parent":{"stay":2,"all":4},"a":5}"#
    );
}

#[test]
fn duplicate_keys_in_a_container_with_removals_leave_the_response_intact() {
    for input in [
        r#"{"drop":1,"drop":2,"keep":3}"#,
        r#"{"outer":{"drop":1,"keep":2,"keep":3}}"#,
    ] {
        assert_eq!(changed("$..drop", input), input);
    }
}

#[test]
fn integer_literals_outside_serde_jsons_integer_range_do_not_match() {
    let input = "[123456789012345678901234567890,123456789012345678901234567891]";
    assert_eq!(
        changed("$[?(key-eq '$' 123456789012345678901234567890)]", input),
        input
    );
    assert_eq!(
        changed(
            "$[?(key-eq '$' -9223372036854775809)]",
            "[-9223372036854775808,-9223372036854775809]"
        ),
        "[-9223372036854775808,-9223372036854775809]"
    );
}
