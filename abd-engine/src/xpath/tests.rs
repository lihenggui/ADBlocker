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

use roxmltree::Document;

use super::budget::Budget;
use super::evaluate::Evaluation;
use super::nodes::{Context, Item};
use super::value::Value;
use super::*;

mod removal;

const XML: &str = concat!(
    "<?outside before?>",
    r#"<r xml:lang="en-US" xmlns:n="urn:n" xmlns:m="urn:n">"#,
    r#"<group><a k="1">01</a><a k="2">2</a><a k="3">bad</a></group>"#,
    r#"<group xml:lang="fr"><a k="4">4</a></group>"#,
    "<div>8</div><and>2</and><foo-bar/><foo>9</foo><bar>1</bar>",
    r#"<n:item m:key="namespace"><child xmlns="urn:default">𝄞é</child><legacy xmlns="relative"/></n:item>"#,
    "<!--note--><?work data?><space>  x&#xD;\n\t y  </space>",
    "<words>so<![CDATA[me]]> text<inner>!</inner></words><空/>",
    "</r><!--after-->"
);

enum Expected<'a> {
    B(bool),
    N(f64),
    S(&'a str),
    Nodes(&'a [&'a str]),
}

use Expected::*;

const CASES: &[(&str, Expected<'_>, &str)] = &[
    ("/", Nodes(&["#document"]), "2"),
    ("r/group/a", Nodes(&["a:01", "a:2", "a:bad", "a:4"]), "2"),
    ("/r/group[1]/a[2]", Nodes(&["a:2"]), "2"),
    ("//a[1]", Nodes(&["a:01", "a:4"]), "2.5"),
    ("/descendant::a[1]", Nodes(&["a:01"]), "2.5"),
    ("(/r/group/a)[1]", Nodes(&["a:01"]), "3.3"),
    ("(/r/group)/a[1]", Nodes(&["a:01", "a:4"]), "3.3"),
    ("(/r)//a[1]", Nodes(&["a:01", "a:4"]), "3.3"),
    ("//a[@k='3'][1]", Nodes(&["a:bad"]), "2.4"),
    ("//a[1][@k='3']", Nodes(&[]), "2.4"),
    ("//a[2.5]", Nodes(&[]), "2.4"),
    ("//a[number('x')]", Nodes(&[]), "2.4"),
    ("//a['0']", Nodes(&["a:01", "a:2", "a:bad", "a:4"]), "2.4"),
    ("//a[@k]", Nodes(&["a:01", "a:2", "a:bad", "a:4"]), "2.4"),
    (
        "//a[@k='2']/ancestor::*[position()=1]",
        Nodes(&["group:012bad"]),
        "2.2",
    ),
    (
        "//a[@k='2']/ancestor::*[position()=last()]",
        Nodes(&["r:012bad48291𝄞é  x\r\n\t y  some text!"]),
        "2.4",
    ),
    (
        "//a[@k='2']/ancestor-or-self::*[position()<=2]",
        Nodes(&["group:012bad", "a:2"]),
        "2.2",
    ),
    (
        "(//a[@k='2']/ancestor::*)[position()=last()]",
        Nodes(&["group:012bad"]),
        "3.3",
    ),
    ("//a[@k='2']/attribute::k", Nodes(&["@k:2"]), "2.2"),
    ("//a[@k='2']/child::text()", Nodes(&["#text:2"]), "2.2"),
    ("/r/descendant::a[last()]", Nodes(&["a:4"]), "2.2"),
    (
        "//a[@k='2']/descendant-or-self::node()",
        Nodes(&["a:2", "#text:2"]),
        "2.2",
    ),
    ("//a[@k='1']/following::a[1]", Nodes(&["a:2"]), "2.2"),
    (
        "//a[@k='1']/following::text()[1]",
        Nodes(&["#text:2"]),
        "2.2",
    ),
    (
        "//a[@k='1']/@k/following::text()[1]",
        Nodes(&["#text:01"]),
        "2.2",
    ),
    (
        "//a[@k='1']/following-sibling::a[last()]",
        Nodes(&["a:bad"]),
        "2.2",
    ),
    (
        "//a[@k='2']/@k/following-sibling::node()",
        Nodes(&[]),
        "2.2",
    ),
    ("//a[@k='2']/@k/parent::a", Nodes(&["a:2"]), "2.2"),
    (
        "//a[@k='2']/@k/ancestor-or-self::node()[1]",
        Nodes(&["@k:2"]),
        "2.2",
    ),
    ("//a[@k='2']/@k/self::*", Nodes(&[]), "2.3"),
    (
        "//a[@k='2']/@k/descendant-or-self::node()",
        Nodes(&["@k:2"]),
        "2.2",
    ),
    (
        "//a[@k='4']/preceding::a[position()=2]",
        Nodes(&["a:2"]),
        "2.2",
    ),
    (
        "//a[@k='4']/preceding::group[1]",
        Nodes(&["group:012bad"]),
        "2.2",
    ),
    ("(//a[@k='4']/preceding::a)[1]", Nodes(&["a:01"]), "3.3"),
    (
        "//a[@k='3']/preceding-sibling::a[1]",
        Nodes(&["a:2"]),
        "2.2",
    ),
    (
        "//a[@k='3']/preceding-sibling::a[position()=last()]",
        Nodes(&["a:01"]),
        "2.4",
    ),
    ("//a[@k='3']/self::a", Nodes(&["a:bad"]), "2.2"),
    ("//a[@k='2']/@k/.", Nodes(&["@k:2"]), "2.5"),
    ("//a[@k='2']/@k/../text()", Nodes(&["#text:2"]), "2.5"),
    ("//a[@k='4']/../@*", Nodes(&["@xml:lang:fr"]), "2.5"),
    ("//group[.//a[@k='4']]/a", Nodes(&["a:4"]), "2.5"),
    (
        "//comment()",
        Nodes(&["#comment:note", "#comment:after"]),
        "2.3",
    ),
    (
        "//processing-instruction()",
        Nodes(&["outside:before", "work:data"]),
        "2.3",
    ),
    (
        "//processing-instruction('work')",
        Nodes(&["work:data"]),
        "2.3",
    ),
    ("//processing-instruction('missing')", Nodes(&[]), "2.3"),
    (
        "//words/node()",
        Nodes(&["#text:some text", "inner:!"]),
        "5.7",
    ),
    (
        "//空|//a[@k='3']|//a[@k='3']|//a[@k='1']",
        Nodes(&["a:01", "a:bad", "空:"]),
        "3.3",
    ),
    ("//item | //child | //key", Nodes(&[]), "2.3"),
    ("/*/div", Nodes(&["div:8"]), "3.7"),
    ("/r/and", Nodes(&["and:2"]), "3.7"),
    ("/r/*[name()='div'] * /r/and", N(16.0), "3.7"),
    ("/r/div div /r/and", N(4.0), "3.7"),
    ("boolean(/r/div and /r/and)", B(true), "3.7"),
    ("/r/foo-bar", Nodes(&["foo-bar:"]), "3.5"),
    ("/r/foo - /r/bar", N(8.0), "3.5"),
    ("2 * 3 + 4", N(10.0), "3.5"),
    ("2 + 3 * 4", N(14.0), "3.5"),
    ("(2 + 3) * 4", N(20.0), "3.1"),
    ("12 div 3 div 2", N(2.0), "3.5"),
    ("5 mod 2", N(1.0), "3.5"),
    ("5 mod -2", N(1.0), "3.5"),
    ("-5 mod 2", N(-1.0), "3.5"),
    ("-5 mod -2", N(-1.0), "3.5"),
    ("--5 - -2", N(7.0), "3.5"),
    ("-//a[@k='2'] | //a[@k='1']", N(-1.0), "3.5"),
    ("2*.5 + 1.", N(2.0), "3.7"),
    ("false() and false() or true()", B(true), "3.4"),
    ("true() or false() and false()", B(true), "3.4"),
    ("true() and false()", B(false), "3.4"),
    ("false() or 0", B(false), "3.4"),
    ("2 < 3", B(true), "3.4"),
    ("3 <= 3", B(true), "3.4"),
    ("3 > 2 > 1", B(false), "3.4"),
    ("2 >= 3", B(false), "3.4"),
    ("2 + 3 = 5", B(true), "3.4"),
    ("2 != 3", B(true), "3.4"),
    ("'02' = '2'", B(false), "3.4"),
    ("'02' = 2", B(true), "3.4"),
    ("false() = ''", B(true), "3.4"),
    ("true() = 'false'", B(true), "3.4"),
    ("2 = true()", B(true), "3.4"),
    ("'10' > '2'", B(true), "3.4"),
    ("true() < 2", B(true), "3.4"),
    ("number('NaN') = number('NaN')", B(false), "3.4"),
    ("number('NaN') != number('NaN')", B(true), "3.4"),
    ("number('NaN') >= 0", B(false), "3.4"),
    ("//a = '01'", B(true), "3.4"),
    ("'1' = //a", B(false), "3.4"),
    ("//a = 1", B(true), "3.4"),
    ("4 = //a", B(true), "3.4"),
    ("//a != '01'", B(true), "3.4"),
    ("not(//a != '01')", B(false), "3.4"),
    ("//missing = false()", B(true), "3.4"),
    ("//missing != false()", B(false), "3.4"),
    ("//missing != ''", B(false), "3.4"),
    ("//missing = //missing", B(false), "3.4"),
    ("//a = //a[@k='1']", B(true), "3.4"),
    ("//a != //a[@k='1']", B(true), "3.4"),
    ("//a[@k='1'] = //a[@k='2']", B(false), "3.4"),
    ("//a[@k='1'] < //a[@k='2']", B(true), "3.4"),
    ("//a < '2'", B(true), "3.4"),
    ("3 < //a", B(true), "3.4"),
    ("//a > false()", B(true), "3.4"),
    ("false() < //a", B(true), "3.4"),
    ("//a > true()", B(false), "3.4"),
    ("//missing >= false()", B(true), "3.4"),
    ("//a[@k='3'] > 0", B(false), "3.4"),
    ("last()", N(1.0), "4.1"),
    ("position()", N(1.0), "4.1"),
    ("count(//a)", N(4.0), "4.1"),
    ("count(//a|//a)", N(4.0), "4.1"),
    ("id('1 2')", Nodes(&[]), "4.1"),
    ("id(//@k)", Nodes(&[]), "4.1"),
    ("local-name(//*[name()='n:item'])", S("item"), "4.1"),
    ("name(//*[local-name()='item'])", S("n:item"), "4.1"),
    ("namespace-uri(//*[local-name()='item'])", S("urn:n"), "4.1"),
    (
        "namespace-uri(//*[local-name()='child'])",
        S("urn:default"),
        "4.1",
    ),
    ("name(//@*[local-name()='key'])", S("m:key"), "4.1"),
    ("namespace-uri(//@*[local-name()='key'])", S("urn:n"), "4.1"),
    ("namespace-uri(//a)", S(""), "4.1"),
    (
        "namespace-uri(//*[local-name()='legacy'])",
        S("relative"),
        "5",
    ),
    ("local-name()", S(""), "4.1"),
    ("name()", S(""), "4.1"),
    ("name(//missing)", S(""), "4.1"),
    (
        "local-name(//processing-instruction()[1])",
        S("outside"),
        "4.1",
    ),
    ("name(//a[4]|//a[1])", S("a"), "4.1"),
    (
        "//a[local-name()='a'][namespace-uri()=''][name()='a'][last()]",
        Nodes(&["a:bad", "a:4"]),
        "4.1",
    ),
    ("string(//a)", S("01"), "4.2"),
    ("string(//a[@k='4']|//a[@k='2'])", S("2"), "4.2"),
    ("string(//missing)", S(""), "4.2"),
    ("string(//words)", S("some text!"), "5.2"),
    ("string(//words/text())", S("some text"), "5.7"),
    ("string(//comment())", S("note"), "5.6"),
    ("string(//processing-instruction())", S("before"), "5.5"),
    ("string(false())", S("false"), "4.2"),
    ("string(true())", S("true"), "4.2"),
    ("string(0 div 0)", S("NaN"), "4.2"),
    ("string(0)", S("0"), "4.2"),
    (
        "string(1000000000000000128)",
        S("1000000000000000128"),
        "4.2",
    ),
    ("string(-0)", S("0"), "4.2"),
    ("string(1 div 0)", S("Infinity"), "4.2"),
    ("string(-1 div 0)", S("-Infinity"), "4.2"),
    ("string(00069.)", S("69"), "4.2"),
    ("string(-000.00125)", S("-0.00125"), "4.2"),
    ("string(.5)", S("0.5"), "4.2"),
    ("string(0.00000000001)", S("0.00000000001"), "4.2"),
    (
        "string(100000000000000000000)",
        S("100000000000000000000"),
        "4.2",
    ),
    ("concat('abc', 2, true(), //a)", S("abc2true01"), "4.2"),
    ("starts-with('abc','ab')", B(true), "4.2"),
    ("starts-with('abc','bc')", B(false), "4.2"),
    ("starts-with('abc','')", B(true), "4.2"),
    ("contains('abc','b')", B(true), "4.2"),
    ("contains('abc','d')", B(false), "4.2"),
    ("contains('abc','')", B(true), "4.2"),
    ("substring-before('1999/04/01','/')", S("1999"), "4.2"),
    ("substring-before('abc','z')", S(""), "4.2"),
    ("substring-before('abc','')", S(""), "4.2"),
    ("substring-after('1999/04/01','/')", S("04/01"), "4.2"),
    ("substring-after('1999/04/01','19')", S("99/04/01"), "4.2"),
    ("substring-after('abc','z')", S(""), "4.2"),
    ("substring-after('abc','')", S("abc"), "4.2"),
    ("substring('12345',2,3)", S("234"), "4.2"),
    ("substring('12345',2)", S("2345"), "4.2"),
    ("substring('12345',1.5,2.6)", S("234"), "4.2"),
    ("substring('12345',0,3)", S("12"), "4.2"),
    ("substring('12345',0 div 0,3)", S(""), "4.2"),
    ("substring('12345',1,0 div 0)", S(""), "4.2"),
    ("substring('12345',-42,1 div 0)", S("12345"), "4.2"),
    ("substring('12345',-1 div 0,1 div 0)", S(""), "4.2"),
    ("substring('12345',-1 div 0)", S("12345"), "4.2"),
    ("substring('12345',2,-1)", S(""), "4.2"),
    ("substring('a𝄞b',2,1)", S("𝄞"), "3.6"),
    ("string-length('𝄞é')", N(2.0), "3.6"),
    ("//a[string-length() = 1]", Nodes(&["a:2", "a:4"]), "4.2"),
    ("//a[string() = 'bad']", Nodes(&["a:bad"]), "4.2"),
    ("normalize-space(//space)", S("x y"), "4.2"),
    ("normalize-space('  a  b  ')", S("a  b"), "4.2"),
    (
        "//space[normalize-space() = 'x y']/self::space",
        Nodes(&["space:  x\r\n\t y  "]),
        "4.2",
    ),
    ("translate('bar','abc','ABC')", S("BAr"), "4.2"),
    ("translate('--aaa--','abc-','ABC')", S("AAA"), "4.2"),
    ("translate('aba','aab','XYZ')", S("XZX"), "4.2"),
    ("translate('𝄞é','𝄞','ab')", S("aé"), "4.2"),
    ("boolean(//a)", B(true), "4.3"),
    ("boolean(//absent)", B(false), "4.3"),
    ("boolean('0')", B(true), "4.3"),
    ("boolean('')", B(false), "4.3"),
    ("boolean(-0)", B(false), "4.3"),
    ("boolean(0 div 0)", B(false), "4.3"),
    ("boolean(-1 div 0)", B(true), "4.3"),
    ("not(false())", B(true), "4.3"),
    ("true()", B(true), "4.3"),
    ("false()", B(false), "4.3"),
    ("lang('en')", B(false), "4.3"),
    ("//a[lang('EN')]", Nodes(&["a:01", "a:2", "a:bad"]), "4.3"),
    ("//a[lang('en-us')][1]", Nodes(&["a:01"]), "4.3"),
    ("//a[lang('e')]", Nodes(&[]), "4.3"),
    ("//a/@k[lang('FR')]", Nodes(&["@k:4"]), "4.3"),
    ("number('  -2.5\t')", N(-2.5), "4.4"),
    ("number('1.')", N(1.0), "4.4"),
    ("number('+.2')", N(f64::NAN), "4.4"),
    ("number('1e2')", N(f64::NAN), "4.4"),
    ("number(' 2')", N(f64::NAN), "4.4"),
    ("number(' ')", N(f64::NAN), "4.4"),
    ("number('--2')", N(f64::NAN), "4.4"),
    ("number(true())", N(1.0), "4.4"),
    ("number(false())", N(0.0), "4.4"),
    ("number(//a)", N(1.0), "4.4"),
    ("number(//absent)", N(f64::NAN), "4.4"),
    ("//a[number() > 2]", Nodes(&["a:4"]), "4.4"),
    ("sum(//@k)", N(10.0), "4.4"),
    ("sum(//a)", N(f64::NAN), "4.4"),
    ("sum(//missing)", N(0.0), "4.4"),
    ("floor(-1.25)", N(-2.0), "4.4"),
    ("ceiling(-1.25)", N(-1.0), "4.4"),
    ("round(-1.5)", N(-1.0), "4.4"),
    ("round(1.5)", N(2.0), "4.4"),
    ("round(0.49999999999999994)", N(0.0), "4.4"),
    ("round(-0.5)", N(-0.0), "4.4"),
    ("round(-0.25)", N(-0.0), "4.4"),
    ("round(-0)", N(-0.0), "4.4"),
    ("round(0)", N(0.0), "4.4"),
    ("round(0 div 0)", N(f64::NAN), "4.4"),
    ("round(1 div 0)", N(f64::INFINITY), "4.4"),
    ("round(-1 div 0)", N(f64::NEG_INFINITY), "4.4"),
];

#[test]
fn xpath_expressions_follow_the_recommendation() {
    let document = Document::parse(XML).unwrap();
    for (expression, expected, section) in CASES {
        let expr = parse::parse(expression)
            .unwrap_or_else(|reason| panic!("{section} {expression}: {reason}"));
        let mut work = Evaluation::new(&|| true);
        let got = work.evaluate(&expr, Context::root(&document)).unwrap();
        assert_result(got, expected, expression, section, &mut work.budget);
    }
}

fn assert_result(
    got: Value<'_, '_>,
    expected: &Expected<'_>,
    expression: &str,
    section: &str,
    work: &mut Budget<'_>,
) {
    match (got, expected) {
        (Value::Boolean(got), B(want)) => assert_eq!(got, *want, "{section} {expression}"),
        (Value::Number(got), N(want)) if want.is_nan() => {
            assert!(got.is_nan(), "{section} {expression}: {got}")
        }
        (Value::Number(got), N(want)) => assert_eq!(
            got.to_bits(),
            want.to_bits(),
            "{section} {expression}: {got} != {want}"
        ),
        (Value::String(got), S(want)) => assert_eq!(got, *want, "{section} {expression}"),
        (Value::Nodes(got), Nodes(want)) => {
            let labels: Vec<_> = got.into_iter().map(|item| label(item, work)).collect();
            assert_eq!(labels, *want, "{section} {expression}");
        }
        _ => panic!("{section} {expression}: unexpected result type"),
    }
}

fn label(item: Item<'_, '_>, work: &mut Budget<'_>) -> String {
    let name = match item {
        Item::Tree(node) if node.is_root() => return "#document".to_owned(),
        Item::Tree(node) if node.is_text() => "#text".to_owned(),
        Item::Tree(node) if node.is_comment() => "#comment".to_owned(),
        Item::Tree(_) => item.qualified_name().to_owned(),
        Item::Attribute(..) => format!("@{}", item.qualified_name()),
    };
    format!("{name}:{}", item.value(work).unwrap())
}

#[test]
fn invalid_xpath_names_the_reason_it_is_rejected() {
    for (input, reason) in [
        ("$test", "XPath variables are unavailable"),
        ("//a[$v]", "XPath variables are unavailable"),
        (
            "/r/namespace::node()",
            "XPath namespace axis is unavailable",
        ),
        ("/n:r", "XPath prefixes have no namespace bindings"),
        ("/r/n:*", "XPath prefixes have no namespace bindings"),
        ("/r['unfinished]", "unterminated XPath literal"),
        ("/r['\0']", "invalid character in XPath literal"),
        ("12.5", "XPath must return a node-set"),
        ("'string'", "XPath must return a node-set"),
        ("true()", "XPath must return a node-set"),
        ("//a[count()]", "wrong number of XPath function arguments"),
        ("//a[count(2)]", "XPath function needs a node-set"),
        ("//a[unknown()]", "unknown XPath function"),
        ("/r/unknown::node()", "unknown XPath axis"),
        ("//a | 1", "XPath union needs node-sets"),
        ("(1)[2]", "XPath filter needs a node-set"),
        ("(1)/a", "XPath path needs a node-set"),
        ("//a[", "expected an XPath expression"),
        ("/r/.[1]", "unexpected XPath token"),
    ] {
        assert_eq!(XPath::parse(input).err(), Some(reason), "{input}");
    }
}

mod properties {
    use bytes::Bytes;
    use proptest::prelude::*;
    use roxmltree::Document;

    use super::*;
    use crate::limits::{MAX_XPATH_DEPTH, MAX_XPATH_VISITS};
    use crate::test_support::{arbitrary_bytes, arbitrary_text, assert_removal, mutate};
    use crate::xpath::{evaluate::Evaluation, nodes::Context};

    fn expression() -> impl Strategy<Value = String> {
        let step = prop::sample::select(vec![
            "/r",
            "//ad",
            "/@enabled",
            "/text()",
            "/comment()",
            "/*",
            "/..",
            "/child::ad[1]",
            "[preceding-sibling::*]",
            "[contains(., 'promo')]",
            "/",
            "//",
            "/@",
        ]);
        prop_oneof![
            arbitrary_text(),
            prop::collection::vec(step, 1..8).prop_map(|steps| steps.concat()),
            (0..=MAX_XPATH_DEPTH + 2)
                .prop_map(|depth| { format!("{}//ad{}", "(".repeat(depth), ")".repeat(depth)) }),
            (1..=MAX_XPATH_DEPTH + 2).prop_map(|depth| format!("//ad[0{}]", " + 1".repeat(depth))),
        ]
    }

    fn document() -> impl Strategy<Value = Bytes> {
        let node = prop_oneof![
            "[a-z&<>]{0,24}".prop_map(|text| format!("<ad><![CDATA[{text}]]></ad>")),
            prop::sample::select(vec![
                "<ad enabled='yes'>promo &amp; skip</ad>",
                "<other a='&quot;'>\u{e9}</other>",
                "<!--keep-->",
                "<?pi data?>",
                "<ad/>",
            ])
            .prop_map(str::to_owned),
        ];
        let structured = (
            prop::collection::vec(node, 0..32),
            prop::option::of((any::<prop::sample::Index>(), prop::option::of(any::<u8>()))),
        )
            .prop_map(|(nodes, mutation)| {
                mutate(
                    format!("<r>\r\n{}</r>", nodes.concat()).into_bytes(),
                    mutation,
                )
            });
        prop_oneof![arbitrary_bytes(), structured].prop_map(Bytes::from)
    }

    proptest! {
        #[test]
        fn parsing_and_evaluation_return_within_the_depth_and_visit_limits(
            expression in expression(), input in document(),
        ) {
            if let Ok(path) = XPath::parse(&expression) {
                prop_assert!(path.0.depth <= MAX_XPATH_DEPTH);
                if let Some(output) = prune(&path, input.clone(), &|| true) {
                    assert_removal(&input, &output)?;
                }
                if let Ok(text) = std::str::from_utf8(&input)
                    && let Ok(doc) = Document::parse(text)
                {
                    let mut evaluation = Evaluation::new(&|| true);
                    let result = evaluation.evaluate(&path.0, Context::root(&doc));
                    prop_assert!(evaluation.budget.visits_used() <= MAX_XPATH_VISITS);
                    if let Ok(nodes) = result.and_then(|value| value.nodes()) {
                        // An empty selection must preserve the bytes for any accepted expression.
                        if nodes.is_empty() {
                            prop_assert_eq!(prune(&path, input.clone(), &|| true), Some(input));
                        }
                    }
                }
            }
        }

        #[test]
        fn selecting_the_protected_document_or_no_nodes_preserves_every_byte(input in document()) {
            for expression in ["/", "//__never_generated__"] {
                let path = XPath::parse(expression).unwrap();
                let result = prune(&path, input.clone(), &|| true);
                prop_assert_eq!(result.as_ref(), Some(&input));
            }
        }
    }

    #[test]
    fn a_large_sibling_query_stops_after_debiting_exactly_the_visit_limit() {
        let input = format!("<r>{}</r>", "<ad/>".repeat(700));
        let doc = Document::parse(&input).unwrap();
        let path = XPath::parse("//ad[preceding-sibling::* and following-sibling::*]").unwrap();
        let mut evaluation = Evaluation::new(&|| true);
        assert!(evaluation.evaluate(&path.0, Context::root(&doc)).is_err());
        assert_eq!(evaluation.budget.visits_used(), MAX_XPATH_VISITS);
    }
}
