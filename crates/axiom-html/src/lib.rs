//! HTML parsing: the WHATWG tokenizer ([`tokenizer`]) and tree construction, incremental
//! and streaming.
//!
//! [`HtmlParser`] accepts input in chunks ([`HtmlParser::feed`], [`HtmlParser::finish`])
//! and builds the tree into a caller-owned [`Document`], so script can run against the
//! same document while the parser is paused. [`HtmlParser::step`] returns after every
//! `</script>` ([`ParseStep::Script`]) so the caller can execute the script at its
//! parser position. A `<script>`, `<style>`, `<title>`, `<textarea>` (and the other
//! raw-text elements) is not inserted until its end tag has arrived, so script never sees
//! a half-received element ([`ParseStep::NeedData`]). `document.write` is not supported,
//! and parse errors are not reported. [`parse_fragment`] implements the fragment case
//! (`innerHTML`-style parsing against a context element); [`serialize_children`] and
//! [`serialize_outer`] are the matching serializers.

mod entities;
mod serialize;
pub mod tokenizer;
mod tree_builder;

pub use serialize::{serialize_children, serialize_outer};

use axiom_dom::{Document, Namespace, NodeId};

use tokenizer::{Token, Tokenizer};
use tree_builder::TreeBuilder;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parse a complete document (scripts are inserted but not run) with the scripting flag
/// enabled.
pub fn parse_html(input: &str) -> Result<Document, ParseError> {
    parse_html_with_scripting(input, true)
}

/// Parse a complete document. `scripting` is the HTML parser's scripting flag: it decides
/// whether `<noscript>` content is raw text (enabled) or markup (disabled).
pub fn parse_html_with_scripting(input: &str, scripting: bool) -> Result<Document, ParseError> {
    let mut document = Document::new();
    let mut parser = HtmlParser::with_scripting(scripting);
    parser.feed(input);
    parser.finish();
    while parser.step(&mut document) != ParseStep::Done {}
    Ok(document)
}

/// Parse `input` as the contents of a `context_tag` element in `context_namespace`
/// (HTML §13.4). The parsed nodes are the children of the returned `html` element; the
/// document is a scratch container.
pub fn parse_fragment(
    context_tag: &str,
    context_namespace: Namespace,
    input: &str,
    scripting: bool,
) -> (Document, NodeId) {
    let mut doc = Document::new();
    let root = doc.create_element("html");
    let document_node = doc.document_id.expect("document root");
    doc.append_child(document_node, root);
    let context = doc.create_element_ns(context_tag, context_namespace);
    let mut tokenizer = Tokenizer::new();
    let mut builder = TreeBuilder::new(scripting);
    builder.start_fragment(&doc, root, context, &mut tokenizer);
    tokenizer.feed(input);
    tokenizer.finish();
    while let Some(token) = tokenizer.next_token() {
        let eof = token == Token::Eof;
        builder.process(&mut doc, &mut tokenizer, token);
        if eof || builder.stopped {
            break;
        }
    }
    (doc, root)
}

/// Outcome of one [`HtmlParser::step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseStep {
    /// A `<script>` element was just closed; the parser is paused before the markup that
    /// follows it.
    Script(NodeId),
    /// Everything fed so far is parsed; call [`HtmlParser::feed`] or
    /// [`HtmlParser::finish`].
    NeedData,
    /// End of input reached and the tree is complete.
    Done,
}

pub struct HtmlParser {
    tokenizer: Tokenizer,
    builder: TreeBuilder,
    /// A raw-text start tag held back until its end tag has arrived.
    pending: Option<Token>,
    started: bool,
    done: bool,
}

impl Default for HtmlParser {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for HtmlParser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HtmlParser")
            .field("bytes_parsed", &self.bytes_parsed())
            .field("done", &self.done)
            .finish()
    }
}

impl HtmlParser {
    /// A parser with the scripting flag enabled.
    pub fn new() -> Self {
        Self::with_scripting(true)
    }

    pub fn with_scripting(scripting: bool) -> Self {
        Self {
            tokenizer: Tokenizer::new(),
            builder: TreeBuilder::new(scripting),
            pending: None,
            started: false,
            done: false,
        }
    }

    /// Append decoded markup.
    pub fn feed(&mut self, text: &str) {
        self.tokenizer.feed(text);
    }

    /// No more input will arrive.
    pub fn finish(&mut self) {
        self.tokenizer.finish();
    }

    pub fn is_finished(&self) -> bool {
        self.tokenizer.is_finished()
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Markup fed but not parsed yet (what a preload scanner may look ahead into).
    pub fn unparsed(&self) -> &str {
        self.tokenizer.unconsumed()
    }

    /// Bytes of markup parsed so far (after newline normalization).
    pub fn bytes_parsed(&self) -> usize {
        self.tokenizer.consumed_bytes()
    }

    /// `<link>`, `<img>`, `<iframe>`, `<base>`, `<meta>` and (complete) `<style>` elements inserted
    /// since the last call, in insertion order.
    pub fn take_discovered(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.builder.discovered)
    }

    /// Parse until a script element closes, more input is needed, or the end.
    pub fn step(&mut self, doc: &mut Document) -> ParseStep {
        if self.done {
            return ParseStep::Done;
        }
        if !self.started {
            self.builder.start_document(doc);
            self.started = true;
        }
        loop {
            let token = match self.pending.take() {
                Some(t) => t,
                None => match self.tokenizer.next_token() {
                    Some(t) => t,
                    None => return ParseStep::NeedData,
                },
            };
            if let Token::StartTag(t) = &token {
                if holds_until_end_tag(&t.name) && !self.end_tag_arrived(&t.name) {
                    self.pending = Some(token);
                    return ParseStep::NeedData;
                }
            }
            let eof = token == Token::Eof;
            self.builder.process(doc, &mut self.tokenizer, token);
            if let Some(script) = self.builder.script.take() {
                return ParseStep::Script(script);
            }
            if eof || self.builder.stopped {
                self.done = true;
                return ParseStep::Done;
            }
        }
    }

    /// Whether the end tag closing a raw-text element named `name` is in the input
    /// received so far (or no more input is coming).
    fn end_tag_arrived(&self, name: &str) -> bool {
        if self.tokenizer.is_finished() {
            return true;
        }
        let rest = self.tokenizer.unconsumed().as_bytes();
        rawtext_end(rest, name).is_some_and(|(close, n)| rest[close + n..].contains(&b'>'))
    }
}

/// Elements whose content the tokenizer reads as RCDATA / RAWTEXT / script data.
fn holds_until_end_tag(name: &str) -> bool {
    matches!(
        name,
        "script"
            | "style"
            | "title"
            | "textarea"
            | "xmp"
            | "iframe"
            | "noembed"
            | "noframes"
            | "noscript"
    )
}

/// Offset of the `</tag` that closes a rawtext element (case-insensitive, followed by a
/// tag terminator), and the length of that prefix.
fn rawtext_end(bytes: &[u8], tag: &str) -> Option<(usize, usize)> {
    let end = format!("</{tag}");
    let n = end.len();
    (0..bytes.len()).find_map(|i| {
        let slice = &bytes[i..];
        (slice.len() >= n
            && slice[..n].eq_ignore_ascii_case(end.as_bytes())
            && matches!(
                slice.get(n),
                Some(b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\x0c')
            ))
        .then_some((i, n))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_document() {
        let html = r#"<!doctype html><html><head><title>Hi</title></head><body><h1>Hello</h1></body></html>"#;
        let doc = parse_html(html).unwrap();
        assert!(doc.body().is_some());
        assert!(doc.text_content(doc.body().unwrap()).contains("Hello"));
    }

    #[test]
    fn raw_text_with_multibyte_characters_before_the_end_tag() {
        let html = include_str!("../../../tests/html/multibyte-rawtext.html");
        let doc = parse_html(html).unwrap();
        let body = doc.text_content(doc.body().unwrap());
        assert!(body.contains("var s = 'xéééééé';"), "{body}");
        assert!(body.contains("after the script"), "{body}");
        let script = doc.find_descendant(doc.body().unwrap(), "script").unwrap();
        assert_eq!(doc.text_content(script), "var s = 'xéééééé';");
    }

    #[test]
    fn selected_option_is_cloned_into_selectedcontent() {
        let selected = |html: &str| {
            let doc = parse_html(html).unwrap();
            let target = doc
                .find_descendant(doc.body().unwrap(), "selectedcontent")
                .unwrap();
            doc.text_content(target)
        };
        assert_eq!(
            selected("<select><button><selectedcontent></button><option>a<option selected><b>b</b><option selected>c</select>"),
            "c"
        );
        assert_eq!(
            selected("<select><button><selectedcontent>old</button><option disabled>a<option>b<option>c</select>"),
            "b"
        );
        assert_eq!(
            selected(
                "<select multiple><button><selectedcontent></button><option selected>a</select>"
            ),
            ""
        );
    }

    #[test]
    fn processing_instructions_are_inserted_like_comments() {
        let doc = parse_html("<?php echo 1 ?><p>x<?t d?></p>").unwrap();
        let text = doc.to_string();
        assert!(text.contains("<?php echo 1 ?>"), "{text}");
        assert!(text.contains("<?t d?>"), "{text}");
    }

    fn element_count(doc: &Document) -> usize {
        doc.iter_elements().count()
    }

    #[test]
    fn pauses_after_each_script_with_only_the_preceding_dom() {
        let html = r#"<html><body><p id="a">a</p><script>one</script><p id="b">b</p><script src="x.js"></script><p id="c">c</p></body></html>"#;
        let mut doc = Document::new();
        let mut p = HtmlParser::new();
        p.feed(html);
        p.finish();
        let ParseStep::Script(first) = p.step(&mut doc) else {
            panic!("no pause at the first script");
        };
        assert_eq!(doc.text_content(first), "one");
        assert!(doc.get_element_by_id("a").is_some());
        assert!(
            doc.get_element_by_id("b").is_none(),
            "parsed past the script"
        );
        let ParseStep::Script(second) = p.step(&mut doc) else {
            panic!("no pause at the second script");
        };
        assert_eq!(doc.attr(second, "src"), Some("x.js"));
        assert!(doc.get_element_by_id("b").is_some());
        assert!(doc.get_element_by_id("c").is_none());
        assert_eq!(p.step(&mut doc), ParseStep::Done);
        assert!(doc.get_element_by_id("c").is_some());
    }

    #[test]
    fn streaming_in_tiny_chunks_matches_one_shot_parsing() {
        let html = r#"<!doctype html><html><head><title>T</title><style>p{color:red}</style><!-- note --></head><body><p class="x" data-v='a>b'>héllo <b>wörld</b></p><script>if (a < b) { x = "</p>"; }</script><img src="i.png"><p>end</p></body></html>"#;
        let expected = parse_html(html).unwrap().to_string();
        for size in [1, 2, 3, 5, 7, 13] {
            let mut doc = Document::new();
            let mut p = HtmlParser::new();
            let mut scripts = 0;
            let chars: Vec<char> = html.chars().collect();
            for chunk in chars.chunks(size) {
                p.feed(&chunk.iter().collect::<String>());
                loop {
                    match p.step(&mut doc) {
                        ParseStep::Script(_) => scripts += 1,
                        ParseStep::NeedData => break,
                        ParseStep::Done => unreachable!(),
                    }
                }
            }
            p.finish();
            while p.step(&mut doc) != ParseStep::Done {}
            assert_eq!(doc.to_string(), expected, "chunk size {size}");
            assert_eq!(scripts, 1, "chunk size {size}");
        }
    }

    #[test]
    fn incomplete_constructs_wait_for_more_input() {
        let mut doc = Document::new();
        let mut p = HtmlParser::new();
        p.feed("<html><body><scr");
        assert_eq!(p.step(&mut doc), ParseStep::NeedData);
        let before = element_count(&doc);
        p.feed("ipt>var a = 1;</scr");
        assert_eq!(p.step(&mut doc), ParseStep::NeedData);
        assert_eq!(
            element_count(&doc),
            before,
            "script inserted before its end tag"
        );
        p.feed("ipt><p>after</p>");
        assert!(matches!(p.step(&mut doc), ParseStep::Script(_)));
        assert_eq!(p.step(&mut doc), ParseStep::NeedData);
        assert!(p.unparsed().is_empty());
        p.finish();
        assert_eq!(p.step(&mut doc), ParseStep::Done);
        assert!(doc.text_content(doc.body().unwrap()).contains("after"));
    }

    #[test]
    fn discovery_reports_resource_elements_in_order() {
        let mut doc = Document::new();
        let mut p = HtmlParser::new();
        p.feed(r#"<head><meta charset=utf-8><base href="/b/"><link rel=stylesheet href=a.css><style>x{}</style></head><body><img src=i.png>"#);
        p.finish();
        while p.step(&mut doc) != ParseStep::Done {}
        let tags: Vec<_> = p
            .take_discovered()
            .into_iter()
            .map(|n| doc.tag_name(n).unwrap().to_string())
            .collect();
        assert_eq!(tags, ["meta", "base", "link", "style", "img"]);
    }

    #[test]
    fn unparsed_exposes_markup_after_a_paused_script() {
        let mut doc = Document::new();
        let mut p = HtmlParser::new();
        p.feed(r#"<script src="a.js"></script><img src="later.png">"#);
        assert!(matches!(p.step(&mut doc), ParseStep::Script(_)));
        assert_eq!(p.unparsed(), r#"<img src="later.png">"#);
    }

    #[test]
    fn content_without_optional_tags_goes_into_body() {
        let doc =
            parse_html("<!doctype html><title>t</title><p>one<p>two<div>three</div>").unwrap();
        let body = doc.body().expect("implied body");
        let tags: Vec<_> = doc
            .get(body)
            .children
            .iter()
            .filter_map(|&c| doc.tag_name(c))
            .collect();
        assert_eq!(tags, ["p", "p", "div"]);
        let head = doc.head().expect("implied head");
        assert!(doc.find_descendant(head, "title").is_some());
        let root = doc.document_id.unwrap();
        assert!(
            doc.get(root)
                .children
                .iter()
                .all(|&c| !matches!(doc.tag_name(c), Some("p" | "div" | "title"))),
            "content left outside <html>"
        );
    }

    #[test]
    fn unquoted_attribute_values_may_contain_slashes() {
        let doc = parse_html("<script src=/resources/testharness.js></script><a href=/x/y/>z</a>")
            .unwrap();
        let script = doc
            .find_descendant(doc.document_id.unwrap(), "script")
            .unwrap();
        assert_eq!(doc.attr(script, "src"), Some("/resources/testharness.js"));
        let a = doc.find_descendant(doc.document_id.unwrap(), "a").unwrap();
        assert_eq!(doc.attr(a, "href"), Some("/x/y/"));
    }

    #[test]
    fn misnested_formatting_uses_the_adoption_agency() {
        let doc = parse_html("<p><b>1<i>2</b>3</i>4</p>").unwrap();
        let p = doc.find_descendant(doc.body().unwrap(), "p").unwrap();
        let tags: Vec<_> = doc
            .get(p)
            .children
            .iter()
            .map(|&c| doc.tag_name(c).unwrap_or("#text"))
            .collect();
        assert_eq!(tags, ["b", "i", "#text"]);
        assert_eq!(doc.text_content(p), "1234");
    }

    #[test]
    fn table_text_is_foster_parented() {
        let doc = parse_html("<table>x<tr><td>cell</td></tr></table>").unwrap();
        let body = doc.body().unwrap();
        let first = doc.get(body).children[0];
        assert_eq!(doc.text_content(first), "x");
        let table = doc.find_descendant(body, "table").unwrap();
        assert!(doc.find_descendant(table, "tbody").is_some());
    }

    #[test]
    fn svg_content_keeps_its_namespace_and_self_closing_tags() {
        let doc = parse_html(
            "<svg viewbox='0 0 1 1'><path/><foreignobject><p>x</p></foreignobject></svg><p>after",
        )
        .unwrap();
        let svg = doc.find_descendant(doc.body().unwrap(), "svg").unwrap();
        assert_eq!(doc.namespace(svg), Some(Namespace::Svg));
        assert_eq!(doc.attr(svg, "viewBox"), Some("0 0 1 1"));
        let kids: Vec<_> = doc
            .get(svg)
            .children
            .iter()
            .map(|&c| doc.tag_name(c).unwrap())
            .collect();
        assert_eq!(kids, ["path", "foreignObject"]);
        let p = doc.find_descendant(svg, "p").unwrap();
        assert_eq!(doc.namespace(p), Some(Namespace::Html));
    }

    #[test]
    fn fragment_parsing_uses_the_context_element() {
        let (doc, root) = parse_fragment("tr", Namespace::Html, "<td>a<td>b", true);
        let cells: Vec<_> = doc
            .get(root)
            .children
            .iter()
            .map(|&c| doc.tag_name(c).unwrap())
            .collect();
        assert_eq!(cells, ["td", "td"]);
        let (doc, root) = parse_fragment("textarea", Namespace::Html, "<b>not markup</b>", true);
        assert_eq!(doc.text_content(root), "<b>not markup</b>");
    }

    #[test]
    fn template_shadowrootmode_attaches_a_declarative_shadow_root() {
        use axiom_dom::ShadowRootMode;
        let doc = parse_html(
            "<div id=a><template shadowrootmode=closed shadowrootclonable><p>in</p></template>\
             <template shadowrootmode=open><i>second</i></template><b>light</b></div>\
             <span id=b><template shadowrootmode=sideways>t</template></span>",
        )
        .unwrap();
        let host = doc.get_element_by_id("a").unwrap();
        let shadow = doc.shadow_root(host).expect("shadow root");
        assert_eq!(shadow.init.mode, ShadowRootMode::Closed);
        assert!(shadow.init.declarative && shadow.init.clonable);
        assert_eq!(doc.text_content(shadow.root), "in");
        // The host already has a shadow root: the second template is an ordinary one.
        let kids: Vec<_> = doc
            .get(host)
            .children
            .iter()
            .map(|&c| doc.tag_name(c).unwrap())
            .collect();
        assert_eq!(kids, ["template", "b"]);
        // An invalid mode is an ordinary template too.
        let span = doc.get_element_by_id("b").unwrap();
        assert!(doc.shadow_root(span).is_none());
        assert_eq!(doc.tag_name(doc.get(span).children[0]), Some("template"));

        // Fragment parsing (innerHTML) never attaches shadow roots.
        let (doc, root) = parse_fragment(
            "body",
            Namespace::Html,
            "<div><template shadowrootmode=open>x</template></div>",
            true,
        );
        let div = doc.get(root).children[0];
        assert!(doc.shadow_root(div).is_none());
        assert_eq!(doc.tag_name(doc.get(div).children[0]), Some("template"));
    }

    #[test]
    fn quirks_mode_follows_the_doctype() {
        use axiom_dom::QuirksMode;
        assert_eq!(
            parse_html("<!DOCTYPE html>").unwrap().quirks_mode,
            QuirksMode::NoQuirks
        );
        assert_eq!(
            parse_html("<p>no doctype").unwrap().quirks_mode,
            QuirksMode::Quirks
        );
        assert_eq!(
            parse_html(r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" "x">"#)
                .unwrap()
                .quirks_mode,
            QuirksMode::LimitedQuirks
        );
    }
}
