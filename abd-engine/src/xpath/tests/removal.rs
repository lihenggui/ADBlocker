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

use bytes::Bytes;

use super::*;
use crate::limits::*;

fn edit(expression: &str, input: &'static str) -> Bytes {
    prune(
        &XPath::parse(expression).unwrap(),
        Bytes::from_static(input.as_bytes()),
        &|| true,
    )
    .unwrap()
}

#[test]
fn only_selected_source_ranges_are_removed() {
    let input = concat!(
        "\u{feff}<?xml\tversion = '1.0' encoding = \"utf-8\"?>\r\n",
        "<?before keep?><r foo=\"keep\"\n  drop = 'a > b'   keep='x'><!--remove-->",
        "<discard own='x'><inner>a</inner>z</discard>",
        "<text>remove<![CDATA[<c>]]>&amp;<!--keep-->TAIL</text>",
        "<text><!--also keep--><![CDATA[remove]]>&lt;remove</text>",
        "<?remove data?></r>\r\n"
    );
    let output = edit(
        concat!(
            "/r/@drop | /r/discard | /r/discard/@* | /r/discard/inner/text() | ",
            "/r/comment() | /r/text/text()[starts-with(., 'remove')] | /r/processing-instruction('remove')"
        ),
        input,
    );
    assert_eq!(
        output,
        concat!(
            "\u{feff}<?xml\tversion = '1.0' encoding = \"utf-8\"?>\r\n",
            "<?before keep?><r foo=\"keep\"   keep='x'>",
            "<text><!--keep-->TAIL</text><text><!--also keep--></text></r>\r\n"
        )
    );
    assert!(
        Document::parse(
            &std::str::from_utf8(&output)
                .unwrap()
                .replace("<?xml\t", "<?xml ")
        )
        .is_ok()
    );
}

#[test]
fn root_and_document_are_protected_and_namespace_declarations_are_not_attributes() {
    let input = "<p:r xmlns:p='urn:p' xmlns='urn:d' x='1'><ad p:x='2'/><keep/></p:r>";
    assert_eq!(edit("/ | /* | /r | //ad", input), input);
    assert_eq!(
        edit("//*[name()='ad'] | //@*", input),
        "<p:r xmlns:p='urn:p' xmlns='urn:d'><keep/></p:r>"
    );
    assert_eq!(
        edit("//@*[name()='p:x']", input),
        "<p:r xmlns:p='urn:p' xmlns='urn:d' x='1'><ad/><keep/></p:r>"
    );
    assert_eq!(
        edit("/ | /* | //@x", input),
        "<p:r xmlns:p='urn:p' xmlns='urn:d'><ad p:x='2'/><keep/></p:r>"
    );
}

#[test]
fn malformed_non_utf8_and_unedited_documents_keep_the_original_allocation() {
    for input in [
        b"<r><t></r>".as_slice(),
        b"<?xml version='1.0' encoding='ISO-8859-1'?><r><t/></r>",
        b"<?xml version='1.0' encoding='UTF8'?><r><t/></r>",
        b"<?xml version='1.0' standalone='maybe'?><r><t/></r>",
        b"<?xml version='a.b'?><r><t/></r>",
        b"<?XML version='1.0'?><r><t/></r>",
        b"<?xml version='1.0'?>\xef\xbb\xbf<r><t/></r>",
        b"<!DOCTYPE r><r><t/></r>",
        b"<!DOCTYPE r [<!ENTITY x 'ad'>]><r><t>&x;</t></r>",
        b"<r>\xe9<t/></r>",
        b"<r><t/><?pi!?></r>",
        b"<r><t/><?xml?></r>",
        b"<r/>",
        b"<r>]]<t/>></r>",
    ] {
        assert_same_allocation(Bytes::copy_from_slice(input));
    }
}

fn assert_same_allocation(input: Bytes) {
    let output = prune(&XPath::parse("//t").unwrap(), input.clone(), &|| true).unwrap();
    assert_eq!(output.as_ptr(), input.as_ptr());
    assert_eq!(output, input);
}

#[test]
fn document_limits_are_checked_before_selecting_from_the_tree() {
    let nested = "<r>".repeat(MAX_XML_DEPTH) + "<t/>" + &"</r>".repeat(MAX_XML_DEPTH);
    let too_many = format!("<r>{}</r>", "<t/>".repeat(MAX_XML_NODES as usize));
    let attrs = (0..=MAX_XML_ATTRIBUTES)
        .map(|i| format!(" a{i}='x'"))
        .collect::<String>();
    let too_many_attrs = format!("<r{attrs}><t/></r>");
    let combined = format!("<r>{}</r>", "<t a='x'/>".repeat(MAX_XML_NODES as usize / 2));
    let namespaces = (0..=MAX_XML_NAMESPACE_DECLARATIONS)
        .map(|i| format!("<n xmlns:p='urn:{i}'/>"))
        .collect::<String>();
    let too_many_namespaces = format!("<r><t/>{namespaces}</r>");
    let too_many_markers = format!(
        "<r><t><![CDATA[{}]]></t></r>",
        "<".repeat(MAX_XML_PARSE_MARKERS)
    );
    let too_long = format!("<r><t>{}</t></r>", " ".repeat(MAX_REWRITE_BODY));
    for input in [
        nested,
        too_many,
        too_many_attrs,
        combined,
        too_many_namespaces,
        too_many_markers,
        too_long,
    ] {
        assert_same_allocation(Bytes::from(input));
    }
}

#[test]
fn expression_length_parse_nesting_and_syntax_tree_depth_are_bounded() {
    let too_long = format!("//t['{}']", "a".repeat(MAX_XPATH_EXPRESSION));
    let deeply_nested = format!(
        "{}//t{}",
        "(".repeat(MAX_XPATH_DEPTH + 1),
        ")".repeat(MAX_XPATH_DEPTH + 1)
    );
    let deep_tree = format!("//t[0{}]", " + 1".repeat(MAX_XPATH_DEPTH));
    for (expression, error) in [
        (too_long, "XPath expression is too long"),
        (deeply_nested, "XPath syntax is nested too deeply"),
        (deep_tree, "XPath syntax tree is too deep"),
    ] {
        assert_eq!(XPath::parse(&expression).err(), Some(error));
    }
}

#[test]
fn visits_text_work_and_cancelled_evaluation_abandon_the_rewrite() {
    let input = Bytes::from(format!("<r>{}</r>", "<t/>".repeat(700)));
    let traversal = XPath::parse("//t[preceding-sibling::* and following-sibling::*]").unwrap();
    assert!(prune(&traversal, input, &|| true).is_none());

    let input = Bytes::from(format!(
        "<r><t>{}</t></r>",
        "a".repeat(MAX_REWRITE_BODY / 2)
    ));
    let expression = format!(
        "//t[false(){}]",
        " or contains(., 'x')".repeat(MAX_XPATH_TEXT_WORK / (MAX_REWRITE_BODY / 2))
    );
    let text_work = XPath::parse(&expression).unwrap();
    assert!(prune(&text_work, input, &|| true).is_none());

    // The XML preflight and tree walk finish before cancellation starts.
    let calls = std::cell::Cell::new(0);
    let deadline = || {
        calls.set(calls.get() + 1);
        calls.get() < 12
    };
    let path = XPath::parse("//t[preceding::r]").unwrap();
    assert!(prune(&path, Bytes::from_static(b"<r><t/></r>"), &deadline).is_none());
}
