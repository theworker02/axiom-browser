//! html5lib tree-construction tests (`.dat` files, maintained in WPT under
//! `html/syntax/parsing/resources`).
//!
//! Each test parses `#data` with Axiom's HTML parser and compares the resulting tree, in
//! html5lib's `| `-prefixed dump format, with `#document`. Parse errors (`#errors`) are
//! not compared: Axiom does not report parse errors. `#document-fragment` tests use the
//! fragment parsing algorithm with the given context element. Tests marked `#script-off`
//! run with the scripting flag disabled; all others run with it enabled.

use std::path::{Path, PathBuf};

use axiom_dom::{
    Document, Namespace, NodeId, NodeKind, XLINK_NAMESPACE, XMLNS_NAMESPACE, XML_NAMESPACE,
};

use crate::expectations::{Outcome, Status};
use crate::runner::{panic_message, parallel_map};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeTest {
    pub file: String,
    /// 1-based position within the file.
    pub index: usize,
    pub data: String,
    pub document: String,
    pub fragment_context: Option<String>,
    /// `Some(true)` for `#script-on`, `Some(false)` for `#script-off`.
    pub scripting: Option<bool>,
}

impl TreeTest {
    pub fn id(&self) -> String {
        format!("{}:{}", self.file, self.index)
    }
}

/// Split a `.dat` file into tests, with the same section and newline rules as html5lib's
/// reference `TestData` reader.
pub fn parse_dat(file: &str, text: &str) -> Vec<TreeTest> {
    let mut tests = Vec::new();
    let mut sections: Vec<(String, String)> = Vec::new();
    let finish =
        |sections: &mut Vec<(String, String)>, tests: &mut Vec<TreeTest>, trim_extra: bool| {
            if sections.is_empty() {
                return;
            }
            if trim_extra {
                if let Some((_, v)) = sections.last_mut() {
                    v.pop();
                }
            }
            let mut t = TreeTest {
                file: file.to_string(),
                index: tests.len() + 1,
                ..TreeTest::default()
            };
            for (key, mut value) in sections.drain(..) {
                if value.ends_with('\n') {
                    value.pop();
                }
                match key.as_str() {
                    "data" => t.data = value,
                    "document" => t.document = value,
                    "document-fragment" => t.fragment_context = Some(value),
                    "script-on" => t.scripting = Some(true),
                    "script-off" => t.scripting = Some(false),
                    _ => {}
                }
            }
            tests.push(t);
        };
    for line in text.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix('#') {
            let heading = rest.trim().to_string();
            if heading == "data" && !sections.is_empty() {
                finish(&mut sections, &mut tests, true);
            }
            sections.push((heading, String::new()));
        } else if let Some((_, v)) = sections.last_mut() {
            v.push_str(line);
        }
    }
    // A blank line after the last test is a separator, not part of its `#document`.
    finish(&mut sections, &mut tests, text.ends_with("\n\n"));
    tests
}

/// html5lib tree dump of `doc` (children of the document node, `| ` prefixed).
pub fn serialize(doc: &Document) -> String {
    match doc.document_id {
        Some(root) => serialize_children(doc, root),
        None => String::new(),
    }
}

/// html5lib tree dump of the children of `parent`.
pub fn serialize_children(doc: &Document, parent: NodeId) -> String {
    let mut lines = Vec::new();
    for &child in &doc.get(parent).children {
        dump(doc, child, 0, &mut lines);
    }
    lines.join("\n")
}

fn dump(doc: &Document, id: NodeId, depth: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(depth);
    let node = doc.get(id);
    match &node.kind {
        NodeKind::Doctype {
            name,
            public_id,
            system_id,
        } => {
            if public_id.is_empty() && system_id.is_empty() {
                out.push(format!("| {pad}<!DOCTYPE {name}>"));
            } else {
                out.push(format!(
                    "| {pad}<!DOCTYPE {name} \"{public_id}\" \"{system_id}\">"
                ));
            }
        }
        NodeKind::Element { tag, namespace } => {
            let prefix = match namespace {
                Namespace::Svg => "svg ",
                Namespace::MathMl => "math ",
                Namespace::Html | Namespace::Null | Namespace::Other(_) => "",
            };
            out.push(format!("| {pad}<{prefix}{tag}>"));
            let mut attrs: Vec<_> = node
                .attrs
                .iter()
                .map(|a| {
                    let name = match a.namespace.as_deref() {
                        Some(XLINK_NAMESPACE) => format!("xlink {}", a.local_name),
                        Some(XML_NAMESPACE) => format!("xml {}", a.local_name),
                        Some(XMLNS_NAMESPACE) => format!("xmlns {}", a.local_name),
                        _ => a.qualified_name(),
                    };
                    (name, &a.value)
                })
                .collect();
            attrs.sort();
            for (k, v) in attrs {
                out.push(format!("| {pad}  {k}=\"{v}\""));
            }
            if let Some(contents) = doc.template_contents(id) {
                out.push(format!("| {pad}  content"));
                for &child in &doc.get(contents).children {
                    dump(doc, child, depth + 2, out);
                }
            }
        }
        // html5lib's HTML-tree dump does not distinguish CDATA nodes from
        // character data. XML support can therefore reuse this serializer
        // without making the HTML conformance runner non-exhaustive.
        NodeKind::Text { data } | NodeKind::CData { data } => {
            out.push(format!("| {pad}\"{data}\""));
        }
        NodeKind::Comment { data } => out.push(format!("| {pad}<!-- {data} -->")),
        NodeKind::ProcessingInstruction { target, data } => {
            out.push(format!("| {pad}<?{target} {data}?>"))
        }
        NodeKind::Document => out.push(format!("| {pad}#document")),
        NodeKind::DocumentFragment => out.push(format!("| {pad}#document-fragment")),
    }
    for &child in &node.children {
        dump(doc, child, depth + 1, out);
    }
}

/// `svg path` / `math mi` / `td` → (namespace, tag).
fn fragment_context(ctx: &str) -> (Namespace, &str) {
    match ctx.split_once(' ') {
        Some(("svg", tag)) => (Namespace::Svg, tag),
        Some(("math", tag)) => (Namespace::MathMl, tag),
        _ => (Namespace::Html, ctx),
    }
}

pub fn run_one(t: &TreeTest) -> Outcome {
    let outcome = |status| Outcome::new(t.id(), t.file.clone(), status);
    let scripting = t.scripting.unwrap_or(true);
    let parsed = std::panic::catch_unwind(|| match &t.fragment_context {
        Some(ctx) => {
            let (ns, tag) = fragment_context(ctx);
            let (doc, root) = axiom_html::parse_fragment(tag, ns, &t.data, scripting);
            serialize_children(&doc, root)
        }
        None => match axiom_html::parse_html_with_scripting(&t.data, scripting) {
            Ok(doc) => serialize(&doc),
            Err(e) => format!("parse error: {e}"),
        },
    });
    let got = match parsed {
        Ok(got) => got,
        Err(p) => return outcome(Status::Crash).with_message(panic_message(&*p)),
    };
    if got == t.document {
        return outcome(Status::Pass);
    }
    outcome(Status::Fail).with_message(first_difference(&t.document, &got))
}

/// `line N: expected `…`, got `…`` for the first line that differs.
pub fn first_difference(expected: &str, got: &str) -> String {
    let (e, g): (Vec<_>, Vec<_>) = (expected.lines().collect(), got.lines().collect());
    let n = e.len().max(g.len());
    for i in 0..n {
        let (el, gl) = (e.get(i).copied(), g.get(i).copied());
        if el != gl {
            return format!(
                "line {}: expected `{}`, got `{}`",
                i + 1,
                el.unwrap_or("<end>"),
                gl.unwrap_or("<end>")
            );
        }
    }
    "trees differ".to_string()
}

pub fn dat_dir(root: &Path) -> PathBuf {
    root.join("tests/wpt/html/syntax/parsing/resources")
}

pub fn load(root: &Path) -> anyhow::Result<Vec<TreeTest>> {
    let dir = dat_dir(root);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "dat"))
        .collect();
    files.sort();
    let mut tests = Vec::new();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path)?;
        tests.extend(parse_dat(&name, &text));
    }
    Ok(tests)
}

pub fn run(root: &Path, jobs: usize, filter: Option<&str>) -> anyhow::Result<Vec<Outcome>> {
    let tests: Vec<_> = load(root)?
        .into_iter()
        .filter(|t| filter.is_none_or(|f| t.id().contains(f)))
        .collect();
    Ok(parallel_map(&tests, jobs, run_one))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "#data\n<p>One<p>Two\n#errors\n(1,3): expected-doctype-but-got-start-tag\n#document\n| <html>\n|   <head>\n|   <body>\n|     <p>\n|       \"One\"\n|     <p>\n|       \"Two\"\n\n#data\n\n#errors\n#document-fragment\ntd\n#document\n\n#data\n<pre>\n\nx</pre>\n#errors\n#script-off\n#document\n| <html>\n|   <head>\n|   <body>\n|     <pre>\n|       \"\nx\"\n";

    #[test]
    fn dat_sections_follow_the_reference_reader() {
        let tests = parse_dat("s.dat", SAMPLE);
        assert_eq!(tests.len(), 3);
        assert_eq!(tests[0].data, "<p>One<p>Two");
        assert!(tests[0].document.ends_with("|       \"Two\""));
        assert_eq!(tests[0].id(), "s.dat:1");
        assert_eq!(tests[1].data, "");
        assert_eq!(tests[1].fragment_context.as_deref(), Some("td"));
        assert_eq!(tests[2].data, "<pre>\n\nx</pre>");
        assert_eq!(tests[2].scripting, Some(false));
        assert!(
            tests[2].document.ends_with("\"\nx\""),
            "{:?}",
            tests[2].document
        );
    }

    #[test]
    fn serializer_uses_the_html5lib_dump_format() {
        let doc = axiom_html::parse_html(
            "<!DOCTYPE html><html><head></head><body><p class=b id=a>x<!--c--></p></body></html>",
        )
        .unwrap();
        let dump = serialize(&doc);
        assert_eq!(
            dump,
            "| <!DOCTYPE html>\n| <html>\n|   <head>\n|   <body>\n|     <p>\n|       class=\"b\"\n|       id=\"a\"\n|       \"x\"\n|       <!-- c -->"
        );
    }

    #[test]
    fn serializer_treats_cdata_as_character_data() {
        let mut doc = Document::new();
        let root = doc.document_id.expect("document root");
        let cdata = doc.create_cdata("x<y");
        doc.append_child(root, cdata);

        assert_eq!(serialize(&doc), "| \"x<y\"");
    }

    #[test]
    fn a_passing_and_a_failing_case() {
        let tests = parse_dat("s.dat", SAMPLE);
        let pass = TreeTest {
            data: "<!DOCTYPE html><html><head></head><body><p>x</p></body></html>".into(),
            document:
                "| <!DOCTYPE html>\n| <html>\n|   <head>\n|   <body>\n|     <p>\n|       \"x\""
                    .into(),
            ..tests[0].clone()
        };
        assert_eq!(run_one(&pass).status, Status::Pass);
        assert_eq!(run_one(&tests[0]).status, Status::Pass, "implied </p>");
        let wrong = TreeTest {
            document: tests[0].document.replace("\"Two\"", "\"Three\""),
            ..tests[0].clone()
        };
        let fail = run_one(&wrong);
        assert_eq!(fail.status, Status::Fail);
        assert!(fail.message.starts_with("line 7:"), "{}", fail.message);
        assert_eq!(
            run_one(&tests[1]).status,
            Status::Pass,
            "empty <td> fragment"
        );
    }

    #[test]
    fn foreign_elements_template_contents_and_doctype_ids_use_html5lib_names() {
        let doc = axiom_html::parse_html(
            "<!DOCTYPE html PUBLIC \"p\" \"s\"><svg xlink:href=a viewbox=b></svg><template><i></i></template>",
        )
        .unwrap();
        let dump = serialize(&doc);
        assert!(dump.starts_with("| <!DOCTYPE html \"p\" \"s\">"), "{dump}");
        assert!(
            dump.contains("|     <svg svg>\n|       viewBox=\"b\"\n|       xlink href=\"a\""),
            "{dump}"
        );
        assert!(
            dump.contains("<template>\n|       content\n|         <i>"),
            "{dump}"
        );
    }
}
