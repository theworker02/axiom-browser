//! HTML fragment serialization (HTML § "Serializing HTML fragments"): `innerHTML` /
//! `outerHTML` getters. Shadow roots are not serialized (Axiom has none).

use axiom_dom::{
    Document, Namespace, NodeId, NodeKind, XLINK_NAMESPACE, XMLNS_NAMESPACE, XML_NAMESPACE,
};

const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
    "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// The serialization of the children of `node` (a template's contents for `<template>`).
/// `scripting` decides whether `<noscript>` text is raw.
pub fn serialize_children(doc: &Document, node: NodeId, scripting: bool) -> String {
    let mut out = String::new();
    let parent = match doc.tag_name(node) {
        Some("template") if doc.namespace(node) == Some(Namespace::Html) => {
            doc.template_contents(node).unwrap_or(node)
        }
        _ => node,
    };
    for &child in &doc.get(parent).children {
        serialize_node(doc, child, node, scripting, &mut out);
    }
    out
}

/// The serialization of `node` itself (`outerHTML`).
pub fn serialize_outer(doc: &Document, node: NodeId, scripting: bool) -> String {
    let mut out = String::new();
    let parent = doc.get(node).parent.unwrap_or(node);
    serialize_node(doc, node, parent, scripting, &mut out);
    out
}

fn raw_text_parent(doc: &Document, parent: NodeId, scripting: bool) -> bool {
    doc.namespace(parent) == Some(Namespace::Html)
        && match doc.tag_name(parent) {
            Some("style" | "script" | "xmp" | "iframe" | "noembed" | "noframes" | "plaintext") => {
                true
            }
            Some("noscript") => scripting,
            _ => false,
        }
}

fn serialize_node(doc: &Document, node: NodeId, parent: NodeId, scripting: bool, out: &mut String) {
    match &doc.get(node).kind {
        NodeKind::Element { tag, namespace } => {
            let name = match namespace {
                Namespace::Html | Namespace::Svg | Namespace::MathMl => tag.clone(),
                _ => doc.qualified_name(node).unwrap_or_else(|| tag.clone()),
            };
            out.push('<');
            out.push_str(&name);
            for a in doc.attributes(node).iter() {
                out.push(' ');
                match a.namespace.as_deref() {
                    None => out.push_str(&a.local_name),
                    Some(XML_NAMESPACE) => {
                        out.push_str("xml:");
                        out.push_str(&a.local_name);
                    }
                    Some(XMLNS_NAMESPACE) if a.local_name == "xmlns" => out.push_str("xmlns"),
                    Some(XMLNS_NAMESPACE) => {
                        out.push_str("xmlns:");
                        out.push_str(&a.local_name);
                    }
                    Some(XLINK_NAMESPACE) => {
                        out.push_str("xlink:");
                        out.push_str(&a.local_name);
                    }
                    Some(_) => out.push_str(&a.qualified_name()),
                }
                out.push_str("=\"");
                escape(&a.value, true, out);
                out.push('"');
            }
            out.push('>');
            if *namespace == Namespace::Html && VOID_ELEMENTS.contains(&tag.as_str()) {
                return;
            }
            out.push_str(&serialize_children(doc, node, scripting));
            out.push_str("</");
            out.push_str(&name);
            out.push('>');
        }
        NodeKind::Text { data } => {
            if raw_text_parent(doc, parent, scripting) {
                out.push_str(data);
            } else {
                escape(data, false, out);
            }
        }
        NodeKind::CData { data } => {
            out.push_str("<![CDATA[");
            out.push_str(data);
            out.push_str("]]>");
        }
        NodeKind::Comment { data } => {
            out.push_str("<!--");
            out.push_str(data);
            out.push_str("-->");
        }
        NodeKind::ProcessingInstruction { target, data } => {
            out.push_str("<?");
            out.push_str(target);
            out.push(' ');
            out.push_str(data);
            out.push('>');
        }
        NodeKind::Doctype { name, .. } => {
            out.push_str("<!DOCTYPE ");
            out.push_str(name);
            out.push('>');
        }
        NodeKind::Document | NodeKind::DocumentFragment => {
            for &child in &doc.get(node).children {
                serialize_node(doc, child, node, scripting, out);
            }
        }
    }
}

fn escape(s: &str, attribute: bool, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '\u{A0}' => out.push_str("&nbsp;"),
            '"' if attribute => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_markup() {
        let doc = crate::parse_html(
            "<div id=a title='x\"<y>'>t&amp;<br><b>b</b><!--c--><script>1<2</script><svg><path/></svg><template><i>x</i></template></div>",
        )
        .unwrap();
        let div = doc.query_selector("#a").unwrap();
        assert_eq!(
            serialize_children(&doc, div, true),
            "t&amp;<br><b>b</b><!--c--><script>1<2</script><svg><path></path></svg><template><i>x</i></template>"
        );
        assert!(serialize_outer(&doc, div, true)
            .starts_with("<div id=\"a\" title=\"x&quot;&lt;y&gt;\">"));
    }
}
