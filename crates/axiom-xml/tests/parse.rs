use axiom_dom::{Document, NodeId, NodeKind, SVG_NAMESPACE, XMLNS_NAMESPACE, XML_NAMESPACE};
use axiom_xml::{parse, parse_for_dom_parser, PARSER_ERROR_NAMESPACE};

fn root_element(doc: &Document) -> NodeId {
    let root = doc.document_id.unwrap();
    *doc.get(root)
        .children
        .iter()
        .find(|&&c| doc.is_element(c))
        .expect("a root element")
}

fn ns(doc: &Document, id: NodeId) -> Option<&str> {
    doc.namespace_uri(doc.namespace(id).unwrap())
}

fn elements(doc: &Document, id: NodeId) -> Vec<NodeId> {
    doc.get(id)
        .children
        .iter()
        .copied()
        .filter(|&c| doc.is_element(c))
        .collect()
}

#[test]
fn builds_the_tree_with_prolog_and_character_data() {
    let doc = parse(
        "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<!-- lead -->\n\
         <!DOCTYPE note SYSTEM \"note.dtd\" [ <!ENTITY x \"]\"> ]>\n\
         <note id='n1'>A &amp; B &#x41;&#66; <![CDATA[<raw> & ]]>tail<?pi data?></note>\n<!--after-->",
    )
    .unwrap();
    let root = doc.document_id.unwrap();
    let kinds: Vec<&str> = doc
        .get(root)
        .children
        .iter()
        .map(|&c| match &doc.get(c).kind {
            NodeKind::Comment { .. } => "comment",
            NodeKind::Doctype { .. } => "doctype",
            NodeKind::Element { .. } => "element",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["comment", "doctype", "element", "comment"]);
    let note = root_element(&doc);
    assert_eq!(doc.tag_name(note), Some("note"));
    assert_eq!(ns(&doc, note), None);
    assert_eq!(doc.attr_ns(note, None, "id"), Some("n1"));
    assert_eq!(doc.text_content(note), "A & B AB <raw> & tail");
    let last = *doc.get(note).children.last().unwrap();
    assert!(matches!(
        &doc.get(last).kind,
        NodeKind::ProcessingInstruction { target, data } if target == "pi" && data == "data"
    ));
    let doctype = doc.get(root).children[1];
    assert!(matches!(
        &doc.get(doctype).kind,
        NodeKind::Doctype { name, system_id, .. } if name == "note" && system_id == "note.dtd"
    ));
}

#[test]
fn resolves_namespaces_and_keeps_case() {
    let doc = parse(
        "<svg xmlns='http://www.w3.org/2000/svg' xmlns:x='urn:x' xml:lang='en'>\
           <x:Item x:Attr='1' plain='2'/><foreignObject><div xmlns=''/></foreignObject></svg>",
    )
    .unwrap();
    let svg = root_element(&doc);
    assert_eq!(ns(&doc, svg), Some(SVG_NAMESPACE));
    assert_eq!(
        doc.attr_ns(svg, Some(XMLNS_NAMESPACE), "xmlns"),
        Some(SVG_NAMESPACE)
    );
    assert_eq!(doc.attr_ns(svg, Some(XMLNS_NAMESPACE), "x"), Some("urn:x"));
    assert_eq!(doc.attr_ns(svg, Some(XML_NAMESPACE), "lang"), Some("en"));
    let kids = elements(&doc, svg);
    let item = kids[0];
    assert_eq!(doc.tag_name(item), Some("Item"));
    assert_eq!(doc.element_prefix(item), Some("x"));
    assert_eq!(ns(&doc, item), Some("urn:x"));
    assert_eq!(doc.attr_ns(item, Some("urn:x"), "Attr"), Some("1"));
    assert_eq!(doc.attr_ns(item, None, "plain"), Some("2"));
    let foreign = kids[1];
    assert_eq!(doc.tag_name(foreign), Some("foreignObject"));
    let div = elements(&doc, foreign)[0];
    assert_eq!(ns(&doc, div), None);
}

#[test]
fn nesting_depth_is_not_limited_by_the_stack() {
    let depth = 100_000;
    let xml = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
    let doc = parse(&xml).unwrap();
    let mut id = root_element(&doc);
    let mut seen = 1;
    while let Some(&child) = doc.get(id).children.first() {
        id = child;
        seen += 1;
    }
    assert_eq!(seen, depth);
}

#[test]
fn well_formedness_errors_are_fatal_with_positions() {
    let cases = [
        ("", "no root element"),
        ("<a>", "unclosed element 'a'"),
        ("<a></b>", "does not match"),
        ("<a/><b/>", "extra content"),
        ("text<a/>", "prolog"),
        ("<a b=c/>", "quoted"),
        ("<a b='1' b='2'/>", "repeated"),
        ("<a>&nbsp;</a>", "entity 'nbsp' is not defined"),
        ("<a>&#0;</a>", "invalid character reference"),
        ("<a>]]></a>", "']]>'"),
        ("<a><!-- x -- y --></a>", "'--'"),
        ("<p:a/>", "prefix 'p' is not defined"),
        (
            "<a xmlns:p='urn:p' p:x='1' xmlns:q='urn:p' q:x='2'/>",
            "repeated",
        ),
        ("<a xmlns:p=''/>", "cannot be undeclared"),
        ("<a b='<'/>", "'<'"),
        ("<a>\u{1}</a>", "invalid character"),
        ("<a/>\n<?xml version='1.0'?>", "XML declaration"),
    ];
    for (input, expected) in cases {
        let err = parse(input).expect_err(input);
        assert!(
            err.message.contains(expected),
            "{input:?}: {:?} lacks {expected:?}",
            err.message
        );
    }
    let err = parse("<a>\n  <b></c>\n</a>").unwrap_err();
    assert_eq!((err.line, err.column), (2, 10));
}

#[test]
fn dom_parser_errors_become_a_parsererror_document() {
    let doc = parse_for_dom_parser("<a>\n<b></a>");
    let root = root_element(&doc);
    assert_eq!(doc.tag_name(root), Some("parsererror"));
    assert_eq!(ns(&doc, root), Some(PARSER_ERROR_NAMESPACE));
    let text = doc.text_content(root);
    assert!(text.starts_with("XML Parsing Error: end tag 'a' does not match start tag 'b'"));
    assert!(text.contains("Line Number 2, Column 8:"));
    let sourcetext = elements(&doc, root)[0];
    assert_eq!(doc.tag_name(sourcetext), Some("sourcetext"));
    assert_eq!(doc.text_content(sourcetext), "<b></a>\n-------^");
}
