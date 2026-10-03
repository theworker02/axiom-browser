//! HTML tree construction (§13.2.6).
//!
//! Insertion modes, the stack of open elements, the list of active formatting elements
//! (with the adoption agency algorithm), foster parenting, `<template>` contents, foreign
//! (SVG / MathML) content and the fragment case. `<select>` follows the current
//! specification, which has no "in select" insertion modes. Not implemented: parse error
//! reporting, form-owner association and `document.write` insertion points. Processing
//! instructions are inserted wherever comments are. When a selected `<option>` is popped,
//! its children are cloned into the select's `<selectedcontent>`. Documents (not
//! fragments) attach declarative shadow roots for `<template shadowrootmode>`.

use std::collections::HashMap;

use axiom_dom::{
    Document, Namespace, NodeId, NodeKind, QuirksMode, ShadowRootInit, ShadowRootMode,
    XLINK_NAMESPACE, XMLNS_NAMESPACE, XML_NAMESPACE,
};

use crate::tokenizer::{Doctype, State, Tag, Token, Tokenizer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InHeadNoscript,
    AfterHead,
    InBody,
    Text,
    InTable,
    InTableText,
    InCaption,
    InColumnGroup,
    InTableBody,
    InRow,
    InCell,
    InTemplate,
    AfterBody,
    InFrameset,
    AfterFrameset,
    AfterAfterBody,
    AfterAfterFrameset,
}

#[derive(Debug, Clone)]
enum Entry {
    Marker,
    Element(NodeId, Tag),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Default,
    ListItem,
    Button,
    Table,
}

const WS: [char; 5] = ['\t', '\n', '\u{c}', '\r', ' '];

fn is_ws(c: char) -> bool {
    WS.contains(&c)
}

fn is_all_ws(s: &str) -> bool {
    s.chars().all(is_ws)
}

/// Leading ASCII whitespace of `s`, and the rest.
fn split_ws(s: &str) -> (&str, &str) {
    let n = s.find(|c| !is_ws(c)).unwrap_or(s.len());
    s.split_at(n)
}

fn tag(name: &str) -> Tag {
    Tag {
        name: name.to_string(),
        ..Tag::default()
    }
}

/// The stack of open elements. Every removal is recorded so that the steps that run "when
/// an element is popped off the stack of open elements" can see it.
#[derive(Debug, Default)]
struct OpenElements {
    stack: Vec<NodeId>,
    removed: Vec<NodeId>,
}

impl std::ops::Deref for OpenElements {
    type Target = [NodeId];

    fn deref(&self) -> &[NodeId] {
        &self.stack
    }
}

impl std::ops::Index<usize> for OpenElements {
    type Output = NodeId;

    fn index(&self, i: usize) -> &NodeId {
        &self.stack[i]
    }
}

impl std::ops::IndexMut<usize> for OpenElements {
    fn index_mut(&mut self, i: usize) -> &mut NodeId {
        &mut self.stack[i]
    }
}

impl OpenElements {
    fn push(&mut self, node: NodeId) {
        self.stack.push(node);
    }

    fn insert(&mut self, i: usize, node: NodeId) {
        self.stack.insert(i, node);
    }

    fn pop(&mut self) -> Option<NodeId> {
        let node = self.stack.pop();
        self.removed.extend(node);
        node
    }

    fn remove(&mut self, i: usize) -> NodeId {
        let node = self.stack.remove(i);
        self.removed.push(node);
        node
    }

    fn truncate(&mut self, len: usize) {
        while self.stack.len() > len {
            self.pop();
        }
    }

    fn clear(&mut self) {
        self.truncate(0);
    }

    fn take_removed(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.removed)
    }
}

pub(crate) struct TreeBuilder {
    mode: Mode,
    original_mode: Mode,
    template_modes: Vec<Mode>,
    open: OpenElements,
    formatting: Vec<Entry>,
    head: Option<NodeId>,
    form: Option<NodeId>,
    frameset_ok: bool,
    scripting: bool,
    foster_parenting: bool,
    table_text: String,
    /// Fragment case: the context element (detached, in the same document).
    context: Option<NodeId>,
    skip_newline: bool,
    document: Option<NodeId>,
    /// A `</script>` was just processed: the script element to run.
    pub(crate) script: Option<NodeId>,
    /// `<link>`, `<img>`, `<base>`, `<meta>` and complete `<style>` elements, in insertion
    /// order.
    pub(crate) discovered: Vec<NodeId>,
    pub(crate) stopped: bool,
    /// The document's "allow declarative shadow roots" (false for fragment parsing).
    allow_declarative_shadow_roots: bool,
    /// `<template shadowrootmode>` elements (never inserted) and the shadow roots their
    /// contents go into.
    shadow_templates: HashMap<NodeId, NodeId>,
}

impl TreeBuilder {
    pub(crate) fn new(scripting: bool) -> Self {
        Self {
            mode: Mode::Initial,
            original_mode: Mode::Initial,
            template_modes: Vec::new(),
            open: OpenElements::default(),
            formatting: Vec::new(),
            head: None,
            form: None,
            frameset_ok: true,
            scripting,
            foster_parenting: false,
            table_text: String::new(),
            context: None,
            skip_newline: false,
            document: None,
            script: None,
            discovered: Vec::new(),
            stopped: false,
            allow_declarative_shadow_roots: true,
            shadow_templates: HashMap::new(),
        }
    }

    pub(crate) fn start_document(&mut self, doc: &Document) {
        self.document = doc.document_id;
    }

    /// Set up the fragment case (§13.4 "parsing HTML fragments") for `root` (an `html`
    /// element already appended to the document) and `context`.
    pub(crate) fn start_fragment(
        &mut self,
        doc: &Document,
        root: NodeId,
        context: NodeId,
        tok: &mut Tokenizer,
    ) {
        self.document = doc.document_id;
        self.context = Some(context);
        self.allow_declarative_shadow_roots = false;
        self.open = OpenElements::default();
        self.open.push(root);
        if doc.namespace(context) == Some(Namespace::Html) {
            let name = doc.tag_name(context).unwrap_or_default();
            let state = match name {
                "title" | "textarea" => Some(State::Rcdata),
                "style" | "xmp" | "iframe" | "noembed" | "noframes" => Some(State::Rawtext),
                "noscript" if self.scripting => Some(State::Rawtext),
                "script" => Some(State::ScriptData),
                "plaintext" => Some(State::Plaintext),
                _ => None,
            };
            if let Some(state) = state {
                tok.set_state(state);
            }
            if name == "template" {
                self.template_modes.push(Mode::InTemplate);
            }
        }
        self.reset_insertion_mode(doc);
        self.update_cdata(doc, tok);
    }

    // ----- node helpers -------------------------------------------------------------

    fn current(&self) -> Option<NodeId> {
        self.open.last().copied()
    }

    fn adjusted_current(&self) -> Option<NodeId> {
        match (self.context, self.open.len()) {
            (Some(ctx), 1) => Some(ctx),
            _ => self.current(),
        }
    }

    fn update_cdata(&self, doc: &Document, tok: &mut Tokenizer) {
        let foreign = self
            .adjusted_current()
            .is_some_and(|n| doc.namespace(n) != Some(Namespace::Html));
        tok.set_allow_cdata(foreign);
    }

    fn is_html(doc: &Document, node: NodeId, names: &[&str]) -> bool {
        match &doc.get(node).kind {
            NodeKind::Element {
                tag,
                namespace: Namespace::Html,
            } => names.contains(&tag.as_str()),
            _ => false,
        }
    }

    fn html_name(doc: &Document, node: NodeId) -> Option<&str> {
        match &doc.get(node).kind {
            NodeKind::Element {
                tag,
                namespace: Namespace::Html,
            } => Some(tag.as_str()),
            _ => None,
        }
    }

    fn current_is(&self, doc: &Document, names: &[&str]) -> bool {
        self.current().is_some_and(|n| Self::is_html(doc, n, names))
    }

    fn is_special(doc: &Document, node: NodeId) -> bool {
        match &doc.get(node).kind {
            NodeKind::Element { tag, namespace } => match namespace {
                Namespace::Html => matches!(
                    tag.as_str(),
                    "address"
                        | "applet"
                        | "area"
                        | "article"
                        | "aside"
                        | "base"
                        | "basefont"
                        | "bgsound"
                        | "blockquote"
                        | "body"
                        | "br"
                        | "button"
                        | "caption"
                        | "center"
                        | "col"
                        | "colgroup"
                        | "dd"
                        | "details"
                        | "dir"
                        | "div"
                        | "dl"
                        | "dt"
                        | "embed"
                        | "fieldset"
                        | "figcaption"
                        | "figure"
                        | "footer"
                        | "form"
                        | "frame"
                        | "frameset"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "head"
                        | "header"
                        | "hgroup"
                        | "hr"
                        | "html"
                        | "iframe"
                        | "img"
                        | "input"
                        | "keygen"
                        | "li"
                        | "link"
                        | "listing"
                        | "main"
                        | "marquee"
                        | "menu"
                        | "meta"
                        | "nav"
                        | "noembed"
                        | "noframes"
                        | "noscript"
                        | "object"
                        | "ol"
                        | "p"
                        | "param"
                        | "plaintext"
                        | "pre"
                        | "script"
                        | "search"
                        | "section"
                        | "select"
                        | "source"
                        | "style"
                        | "summary"
                        | "table"
                        | "tbody"
                        | "td"
                        | "template"
                        | "textarea"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "title"
                        | "tr"
                        | "track"
                        | "ul"
                        | "wbr"
                        | "xmp"
                ),
                Namespace::MathMl => {
                    matches!(
                        tag.as_str(),
                        "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml"
                    )
                }
                Namespace::Svg => matches!(tag.as_str(), "foreignObject" | "desc" | "title"),
                Namespace::Null | Namespace::Other(_) => false,
            },
            _ => false,
        }
    }

    fn is_scope_boundary(doc: &Document, node: NodeId, scope: Scope) -> bool {
        let NodeKind::Element { tag, namespace } = &doc.get(node).kind else {
            return false;
        };
        let t = tag.as_str();
        match scope {
            Scope::Table => {
                *namespace == Namespace::Html && matches!(t, "html" | "table" | "template")
            }
            _ => {
                let base = match namespace {
                    Namespace::Html => matches!(
                        t,
                        "applet"
                            | "caption"
                            | "html"
                            | "table"
                            | "td"
                            | "th"
                            | "marquee"
                            | "object"
                            | "select"
                            | "template"
                    ),
                    Namespace::MathMl => {
                        matches!(t, "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml")
                    }
                    Namespace::Svg => matches!(t, "foreignObject" | "desc" | "title"),
                    Namespace::Null | Namespace::Other(_) => false,
                };
                base || (*namespace == Namespace::Html
                    && match scope {
                        Scope::ListItem => matches!(t, "ol" | "ul"),
                        Scope::Button => t == "button",
                        _ => false,
                    })
            }
        }
    }

    fn in_scope(&self, doc: &Document, names: &[&str], scope: Scope) -> bool {
        for &node in self.open.iter().rev() {
            if Self::is_html(doc, node, names) {
                return true;
            }
            if Self::is_scope_boundary(doc, node, scope) {
                return false;
            }
        }
        false
    }

    fn node_in_scope(&self, doc: &Document, target: NodeId) -> bool {
        for &node in self.open.iter().rev() {
            if node == target {
                return true;
            }
            if Self::is_scope_boundary(doc, node, Scope::Default) {
                return false;
            }
        }
        false
    }

    fn template_on_stack(&self, doc: &Document) -> bool {
        self.open
            .iter()
            .any(|&n| Self::is_html(doc, n, &["template"]))
    }

    /// The form rules' "template element on the stack", where a `template` fragment
    /// context counts too (forms inside template contents never set the form pointer).
    fn inside_template(&self, doc: &Document) -> bool {
        self.template_on_stack(doc)
            || self
                .context
                .is_some_and(|c| Self::is_html(doc, c, &["template"]))
    }

    fn pop_until(&mut self, doc: &Document, names: &[&str]) {
        while let Some(node) = self.open.pop() {
            if Self::is_html(doc, node, names) {
                break;
            }
        }
    }

    fn pop_until_node(&mut self, target: NodeId) {
        while let Some(node) = self.open.pop() {
            if node == target {
                break;
            }
        }
    }

    fn remove_from_stack(&mut self, node: NodeId) {
        if let Some(i) = self.open.iter().rposition(|&n| n == node) {
            self.open.remove(i);
        }
    }

    fn generate_implied_end_tags(&mut self, doc: &Document, except: Option<&str>) {
        const IMPLIED: [&str; 10] = [
            "dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc",
        ];
        while let Some(node) = self.current() {
            match Self::html_name(doc, node) {
                Some(name) if IMPLIED.contains(&name) && Some(name) != except => {
                    self.open.pop();
                }
                _ => break,
            }
        }
    }

    fn generate_all_implied_end_tags_thoroughly(&mut self, doc: &Document) {
        const IMPLIED: [&str; 18] = [
            "caption", "colgroup", "dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt",
            "rtc", "tbody", "td", "tfoot", "th", "thead", "tr",
        ];
        while self
            .current()
            .is_some_and(|n| Self::is_html(doc, n, &IMPLIED))
        {
            self.open.pop();
        }
    }

    fn close_p(&mut self, doc: &Document) {
        self.generate_implied_end_tags(doc, Some("p"));
        self.pop_until(doc, &["p"]);
    }

    fn close_p_in_button_scope(&mut self, doc: &Document) {
        if self.in_scope(doc, &["p"], Scope::Button) {
            self.close_p(doc);
        }
    }

    fn clear_stack_back_to(&mut self, doc: &Document, names: &[&str]) {
        while self
            .current()
            .is_some_and(|n| !Self::is_html(doc, n, names))
        {
            self.open.pop();
        }
    }

    // ----- insertion ----------------------------------------------------------------

    /// Appropriate place for inserting a node (§13.2.6.1): `(parent, insert before)`.
    fn appropriate_place(
        &self,
        doc: &mut Document,
        override_target: Option<NodeId>,
    ) -> (NodeId, Option<NodeId>) {
        let target = override_target
            .or(self.current())
            .or(self.document)
            .expect("insertion target");
        let (parent, before) = if self.foster_parenting
            && Self::is_html(doc, target, &["table", "tbody", "tfoot", "thead", "tr"])
        {
            let last_template = self
                .open
                .iter()
                .rposition(|&n| Self::is_html(doc, n, &["template"]));
            let last_table = self
                .open
                .iter()
                .rposition(|&n| Self::is_html(doc, n, &["table"]));
            match (last_template, last_table) {
                (Some(t), tb) if tb.is_none_or(|tb| t > tb) => (self.open[t], None),
                (_, None) => (self.open[0], None),
                (_, Some(tb)) => {
                    let table = self.open[tb];
                    match doc.get(table).parent {
                        Some(p) => (p, Some(table)),
                        None => (self.open[tb - 1], None),
                    }
                }
            }
        } else {
            (target, None)
        };
        if let Some(&shadow) = self.shadow_templates.get(&parent) {
            return (shadow, None);
        }
        if Self::is_html(doc, parent, &["template"]) {
            return (doc.template_contents_or_create(parent), None);
        }
        (parent, before)
    }

    fn insert_at(doc: &mut Document, (parent, before): (NodeId, Option<NodeId>), node: NodeId) {
        match before {
            Some(b) => doc.insert_before(parent, node, Some(b)),
            None => doc.append_child(parent, node),
        }
    }

    fn create_element(doc: &mut Document, t: &Tag, namespace: Namespace) -> NodeId {
        let el = doc.create_element_ns(&t.name, namespace);
        for a in &t.attrs {
            match foreign_attribute(&a.name).filter(|_| namespace != Namespace::Html) {
                Some((ns, prefix, local)) => doc.set_attr_ns(el, Some(ns), prefix, local, &a.value),
                None => doc.set_attr_exact(el, &a.name, &a.value),
            }
        }
        el
    }

    fn insert_element(&mut self, doc: &mut Document, t: &Tag, namespace: Namespace) -> NodeId {
        self.run_popped_steps(doc);
        let place = self.appropriate_place(doc, None);
        let el = Self::create_element(doc, t, namespace);
        Self::insert_at(doc, place, el);
        self.open.push(el);
        if namespace == Namespace::Html {
            match t.name.as_str() {
                "link" | "img" | "base" | "meta" | "iframe" => self.discovered.push(el),
                "template" => {
                    doc.template_contents_or_create(el);
                }
                _ => {}
            }
        }
        el
    }

    fn insert_html(&mut self, doc: &mut Document, t: &Tag) -> NodeId {
        self.insert_element(doc, t, Namespace::Html)
    }

    /// A `template` start tag in head (§13.2.6.4.4): with a valid `shadowrootmode`, the
    /// template stays out of the tree and its contents become a declarative shadow root
    /// of the adjusted current node. Otherwise, or when that node cannot take one, it is
    /// an ordinary template.
    fn insert_template(&mut self, doc: &mut Document, t: &Tag) {
        let attr = |name: &str| t.attrs.iter().find(|a| a.name == name);
        let mode = match attr("shadowrootmode").map(|a| a.value.to_ascii_lowercase()) {
            Some(m) if m == "open" => Some(ShadowRootMode::Open),
            Some(m) if m == "closed" => Some(ShadowRootMode::Closed),
            _ => None,
        };
        let host = self.adjusted_current();
        let (Some(mode), Some(host), true) = (mode, host, self.allow_declarative_shadow_roots)
        else {
            self.insert_html(doc, t);
            return;
        };
        if self.open.first().copied() == Some(host) {
            self.insert_html(doc, t);
            return;
        }
        self.run_popped_steps(doc);
        let place = self.appropriate_place(doc, None);
        let template = Self::create_element(doc, t, Namespace::Html);
        self.open.push(template);
        let init = ShadowRootInit {
            delegates_focus: attr("shadowrootdelegatesfocus").is_some(),
            clonable: attr("shadowrootclonable").is_some(),
            serializable: attr("shadowrootserializable").is_some(),
            declarative: true,
            ..ShadowRootInit::new(mode)
        };
        let shadow = if doc.shadow_root(host).is_some() {
            None
        } else {
            doc.attach_shadow(host, init)
        };
        match shadow {
            Some(root) => {
                self.shadow_templates.insert(template, root);
            }
            None => {
                Self::insert_at(doc, place, template);
                doc.template_contents_or_create(template);
            }
        }
    }

    /// Insert and immediately pop (void elements).
    fn insert_void(&mut self, doc: &mut Document, t: &Tag) {
        self.insert_html(doc, t);
        self.open.pop();
    }

    fn insert_text(&mut self, doc: &mut Document, s: &str) {
        if s.is_empty() {
            return;
        }
        let (parent, before) = self.appropriate_place(doc, None);
        if matches!(doc.get(parent).kind, NodeKind::Document) {
            return;
        }
        let siblings = &doc.get(parent).children;
        let prev = match before {
            Some(b) => siblings
                .iter()
                .position(|&c| c == b)
                .and_then(|i| i.checked_sub(1))
                .map(|i| siblings[i]),
            None => siblings.last().copied(),
        };
        if let Some(prev) = prev {
            if let NodeKind::Text { data } = &mut doc.get_mut(prev).kind {
                data.push_str(s);
                doc.mark_dirty(axiom_dom::DirtyFlags::all());
                return;
            }
        }
        let text = doc.create_text(s);
        Self::insert_at(doc, (parent, before), text);
    }

    /// "Insert a comment" / "insert a processing instruction" at the appropriate place.
    fn insert_comment(&mut self, doc: &mut Document, token: &Token) {
        let place = self.appropriate_place(doc, None);
        let c = Self::create_comment_like(doc, token);
        Self::insert_at(doc, place, c);
    }

    fn insert_comment_in(doc: &mut Document, parent: NodeId, token: &Token) {
        let c = Self::create_comment_like(doc, token);
        doc.append_child(parent, c);
    }

    /// The tree construction rules treat processing instructions exactly like comments.
    fn create_comment_like(doc: &mut Document, token: &Token) -> NodeId {
        match token {
            Token::ProcessingInstruction { target, data } => {
                doc.create_processing_instruction(target, data)
            }
            Token::Comment(data) => doc.create_comment(data),
            other => unreachable!("not a comment or processing instruction: {other:?}"),
        }
    }

    fn add_missing_attrs(doc: &mut Document, node: NodeId, t: &Tag) {
        for a in &t.attrs {
            if !doc.get(node).attrs.contains_key(&a.name) {
                doc.set_attr_exact(node, &a.name, &a.value);
            }
        }
    }

    fn generic_text(&mut self, doc: &mut Document, tok: &mut Tokenizer, t: &Tag, state: State) {
        self.insert_html(doc, t);
        tok.set_state(state);
        self.original_mode = self.mode;
        self.mode = Mode::Text;
    }

    // ----- active formatting elements -----------------------------------------------

    fn push_formatting(&mut self, doc: &Document, node: NodeId, t: &Tag) {
        let mut same = Vec::new();
        for (i, e) in self.formatting.iter().enumerate().rev() {
            match e {
                Entry::Marker => break,
                Entry::Element(n, et) => {
                    if Self::html_name(doc, *n) == Some(t.name.as_str()) && same_attrs(et, t) {
                        same.push(i);
                    }
                }
            }
        }
        if same.len() >= 3 {
            self.formatting.remove(*same.last().unwrap());
        }
        self.formatting.push(Entry::Element(node, t.clone()));
    }

    fn formatting_index(&self, node: NodeId) -> Option<usize> {
        self.formatting
            .iter()
            .rposition(|e| matches!(e, Entry::Element(n, _) if *n == node))
    }

    fn clear_formatting_to_marker(&mut self) {
        while let Some(e) = self.formatting.pop() {
            if matches!(e, Entry::Marker) {
                break;
            }
        }
    }

    fn reconstruct_formatting(&mut self, doc: &mut Document) {
        let Some(last) = self.formatting.last() else {
            return;
        };
        match last {
            Entry::Marker => return,
            Entry::Element(n, _) if self.open.contains(n) => return,
            _ => {}
        }
        let mut i = self.formatting.len() - 1;
        while i > 0 {
            match &self.formatting[i - 1] {
                Entry::Marker => break,
                Entry::Element(n, _) if self.open.contains(n) => break,
                _ => i -= 1,
            }
        }
        while i < self.formatting.len() {
            let Entry::Element(_, t) = self.formatting[i].clone() else {
                unreachable!("markers stop the rewind");
            };
            let el = self.insert_html(doc, &t);
            self.formatting[i] = Entry::Element(el, t);
            i += 1;
        }
    }

    /// Adoption agency algorithm; `false` means "act as any other end tag".
    fn adoption_agency(&mut self, doc: &mut Document, subject: &str) -> bool {
        if let Some(cur) = self.current() {
            if Self::html_name(doc, cur) == Some(subject) && self.formatting_index(cur).is_none() {
                self.open.pop();
                return true;
            }
        }
        for _ in 0..8 {
            let mut found = None;
            for (i, e) in self.formatting.iter().enumerate().rev() {
                match e {
                    Entry::Marker => break,
                    Entry::Element(n, _) if Self::html_name(doc, *n) == Some(subject) => {
                        found = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(fe_list) = found else {
                return false;
            };
            let Entry::Element(fe, fe_tag) = self.formatting[fe_list].clone() else {
                unreachable!()
            };
            let Some(fe_stack) = self.open.iter().rposition(|&n| n == fe) else {
                self.formatting.remove(fe_list);
                return true;
            };
            if !self.node_in_scope(doc, fe) {
                return true;
            }
            let Some(fb_stack) =
                (fe_stack + 1..self.open.len()).find(|&i| Self::is_special(doc, self.open[i]))
            else {
                self.open.truncate(fe_stack);
                self.formatting.remove(fe_list);
                return true;
            };
            let furthest_block = self.open[fb_stack];
            let common_ancestor = self.open[fe_stack - 1];
            let mut bookmark = fe_list;
            let mut node_stack = fb_stack;
            let mut last_node = furthest_block;
            let mut inner = 0;
            loop {
                inner += 1;
                node_stack -= 1;
                let node = self.open[node_stack];
                if node == fe {
                    break;
                }
                let mut in_list = self.formatting_index(node);
                if inner > 3 {
                    if let Some(i) = in_list {
                        self.formatting.remove(i);
                        if i < bookmark {
                            bookmark -= 1;
                        }
                        in_list = None;
                    }
                }
                let Some(list_i) = in_list else {
                    self.open.remove(node_stack);
                    continue;
                };
                let Entry::Element(_, node_tag) = self.formatting[list_i].clone() else {
                    unreachable!()
                };
                let new = Self::create_element(doc, &node_tag, Namespace::Html);
                self.formatting[list_i] = Entry::Element(new, node_tag);
                self.open[node_stack] = new;
                if last_node == furthest_block {
                    bookmark = list_i + 1;
                }
                doc.append_child(new, last_node);
                last_node = new;
            }
            let place = self.appropriate_place(doc, Some(common_ancestor));
            Self::insert_at(doc, place, last_node);
            let new = Self::create_element(doc, &fe_tag, Namespace::Html);
            let children = doc.get(furthest_block).children.clone();
            for c in children {
                doc.append_child(new, c);
            }
            doc.append_child(furthest_block, new);
            if let Some(i) = self.formatting_index(fe) {
                self.formatting.remove(i);
                if i < bookmark {
                    bookmark -= 1;
                }
            }
            let bookmark = bookmark.min(self.formatting.len());
            self.formatting
                .insert(bookmark, Entry::Element(new, fe_tag));
            self.remove_from_stack(fe);
            let fb = self
                .open
                .iter()
                .rposition(|&n| n == furthest_block)
                .expect("furthest block on stack");
            self.open.insert(fb + 1, new);
        }
        true
    }

    fn reset_insertion_mode(&mut self, doc: &Document) {
        for i in (0..self.open.len()).rev() {
            let last = i == 0;
            let node = match (last, self.context) {
                (true, Some(ctx)) => ctx,
                _ => self.open[i],
            };
            let mode = match Self::html_name(doc, node) {
                Some("td" | "th") if !last => Some(Mode::InCell),
                Some("tr") => Some(Mode::InRow),
                Some("tbody" | "thead" | "tfoot") => Some(Mode::InTableBody),
                Some("caption") => Some(Mode::InCaption),
                Some("colgroup") => Some(Mode::InColumnGroup),
                Some("table") => Some(Mode::InTable),
                Some("template") => self.template_modes.last().copied(),
                Some("head") if !last => Some(Mode::InHead),
                Some("body") => Some(Mode::InBody),
                Some("frameset") => Some(Mode::InFrameset),
                Some("html") => Some(if self.head.is_none() {
                    Mode::BeforeHead
                } else {
                    Mode::AfterHead
                }),
                _ => None,
            };
            if let Some(mode) = mode {
                self.mode = mode;
                return;
            }
            if last {
                self.mode = Mode::InBody;
                return;
            }
        }
        self.mode = Mode::InBody;
    }

    // ----- dispatch -----------------------------------------------------------------

    pub(crate) fn process(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        let token = if std::mem::take(&mut self.skip_newline) {
            match token {
                Token::Character(s) => match s.strip_prefix('\n') {
                    Some("") => return,
                    Some(rest) => Token::Character(rest.to_string()),
                    None => Token::Character(s),
                },
                other => other,
            }
        } else {
            token
        };
        if self.use_html_rules(doc, &token) {
            self.process_in(doc, tok, self.mode, token);
        } else {
            self.foreign_content(doc, tok, token);
        }
        self.run_popped_steps(doc);
        self.update_cdata(doc, tok);
    }

    /// Steps for elements popped off the stack of open elements since the last call. They
    /// run before the next element is inserted, so no later option can change which option
    /// is selected in between.
    fn run_popped_steps(&mut self, doc: &mut Document) {
        for node in self.open.take_removed() {
            if Self::is_html(doc, node, &["option"]) {
                Self::maybe_clone_option_into_selectedcontent(doc, node);
            }
        }
    }

    /// "Maybe clone an option into selectedcontent" (HTML §4.10.10). A `selectedcontent`
    /// element's disabled flag is only ever set by DOM insertion steps, so it is false for
    /// parser-built trees.
    fn maybe_clone_option_into_selectedcontent(doc: &mut Document, option: NodeId) {
        let Some(select) = Self::owning_select(doc, option) else {
            return;
        };
        if doc.has_attr(select, "multiple") || Self::selected_option(doc, select) != Some(option) {
            return;
        }
        let Some(target) = doc
            .descendants(select)
            .into_iter()
            .find(|&n| Self::is_html(doc, n, &["selectedcontent"]))
        else {
            return;
        };
        for child in doc.get(target).children.clone() {
            doc.remove_child(target, child);
        }
        for child in doc.get(option).children.clone() {
            let clone = doc.clone_node(child, true);
            doc.append_child(target, clone);
        }
    }

    /// The option whose selectedness is true in a single-select `select` built by the
    /// parser: the last option with a `selected` attribute, otherwise (display size 1) the
    /// first option that is not disabled.
    fn selected_option(doc: &Document, select: NodeId) -> Option<NodeId> {
        let options: Vec<NodeId> = doc
            .descendants(select)
            .into_iter()
            .filter(|&n| {
                Self::is_html(doc, n, &["option"]) && Self::owning_select(doc, n) == Some(select)
            })
            .collect();
        if let Some(&last) = options.iter().rev().find(|&&o| doc.has_attr(o, "selected")) {
            return Some(last);
        }
        let display_size = doc
            .attr(select, "size")
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(1);
        if display_size != 1 {
            return None;
        }
        options.into_iter().find(|&o| {
            let in_disabled_group = doc.get(o).parent.is_some_and(|p| {
                Self::is_html(doc, p, &["optgroup"]) && doc.has_attr(p, "disabled")
            });
            !doc.has_attr(o, "disabled") && !in_disabled_group
        })
    }

    fn owning_select(doc: &Document, node: NodeId) -> Option<NodeId> {
        let mut cur = doc.get(node).parent;
        while let Some(n) = cur {
            if Self::is_html(doc, n, &["select"]) {
                return Some(n);
            }
            cur = doc.get(n).parent;
        }
        None
    }

    fn use_html_rules(&self, doc: &Document, token: &Token) -> bool {
        let Some(node) = self.adjusted_current() else {
            return true;
        };
        let NodeKind::Element { tag, namespace } = &doc.get(node).kind else {
            return true;
        };
        if *namespace == Namespace::Html {
            return true;
        }
        let mathml_text_ip = *namespace == Namespace::MathMl
            && matches!(tag.as_str(), "mi" | "mo" | "mn" | "ms" | "mtext");
        match token {
            Token::StartTag(t)
                if mathml_text_ip && t.name != "mglyph" && t.name != "malignmark" =>
            {
                return true
            }
            Token::Character(_) if mathml_text_ip => return true,
            Token::StartTag(t)
                if *namespace == Namespace::MathMl
                    && tag == "annotation-xml"
                    && t.name == "svg" =>
            {
                return true
            }
            Token::Eof => return true,
            _ => {}
        }
        let html_ip = Self::is_html_integration_point(doc, node);
        html_ip && matches!(token, Token::StartTag(_) | Token::Character(_))
    }

    fn is_html_integration_point(doc: &Document, node: NodeId) -> bool {
        match &doc.get(node).kind {
            NodeKind::Element {
                tag,
                namespace: Namespace::MathMl,
            } if tag == "annotation-xml" => doc.attr(node, "encoding").is_some_and(|e| {
                e.eq_ignore_ascii_case("text/html")
                    || e.eq_ignore_ascii_case("application/xhtml+xml")
            }),
            NodeKind::Element {
                tag,
                namespace: Namespace::Svg,
            } => matches!(tag.as_str(), "foreignObject" | "desc" | "title"),
            _ => false,
        }
    }

    fn process_in(&mut self, doc: &mut Document, tok: &mut Tokenizer, mode: Mode, token: Token) {
        match mode {
            Mode::Initial => self.initial(doc, tok, token),
            Mode::BeforeHtml => self.before_html(doc, tok, token),
            Mode::BeforeHead => self.before_head(doc, tok, token),
            Mode::InHead => self.in_head(doc, tok, token),
            Mode::InHeadNoscript => self.in_head_noscript(doc, tok, token),
            Mode::AfterHead => self.after_head(doc, tok, token),
            Mode::InBody => self.in_body(doc, tok, token),
            Mode::Text => self.text(doc, tok, token),
            Mode::InTable => self.in_table(doc, tok, token),
            Mode::InTableText => self.in_table_text(doc, tok, token),
            Mode::InCaption => self.in_caption(doc, tok, token),
            Mode::InColumnGroup => self.in_column_group(doc, tok, token),
            Mode::InTableBody => self.in_table_body(doc, tok, token),
            Mode::InRow => self.in_row(doc, tok, token),
            Mode::InCell => self.in_cell(doc, tok, token),
            Mode::InTemplate => self.in_template(doc, tok, token),
            Mode::AfterBody => self.after_body(doc, tok, token),
            Mode::InFrameset => self.in_frameset(doc, tok, token),
            Mode::AfterFrameset => self.after_frameset(doc, tok, token),
            Mode::AfterAfterBody => self.after_after_body(doc, tok, token),
            Mode::AfterAfterFrameset => self.after_after_frameset(doc, tok, token),
        }
    }

    fn reprocess(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        self.process_in(doc, tok, self.mode, token);
    }

    fn stop(&mut self) {
        self.open.clear();
        self.stopped = true;
    }

    // ----- insertion modes ----------------------------------------------------------

    fn initial(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(s) => {
                let (_, rest) = split_ws(&s);
                if !rest.is_empty() {
                    let rest = rest.to_string();
                    doc.quirks_mode = QuirksMode::Quirks;
                    self.mode = Mode::BeforeHtml;
                    self.reprocess(doc, tok, Token::Character(rest));
                }
            }
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                let root = self.document.expect("document");
                Self::insert_comment_in(doc, root, &t);
            }
            Token::Doctype(d) => {
                let root = self.document.expect("document");
                let node = doc.create_doctype_with_ids(
                    d.name.as_deref().unwrap_or(""),
                    d.public_id.as_deref().unwrap_or(""),
                    d.system_id.as_deref().unwrap_or(""),
                );
                doc.append_child(root, node);
                doc.quirks_mode = quirks_mode_for(&d);
                self.mode = Mode::BeforeHtml;
            }
            other => {
                doc.quirks_mode = QuirksMode::Quirks;
                self.mode = Mode::BeforeHtml;
                self.reprocess(doc, tok, other);
            }
        }
    }

    fn before_html(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Doctype(_) => {}
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                let root = self.document.expect("document");
                Self::insert_comment_in(doc, root, &t);
            }
            Token::Character(s) => {
                let (_, rest) = split_ws(&s);
                if !rest.is_empty() {
                    let rest = rest.to_string();
                    self.implied_html(doc);
                    self.reprocess(doc, tok, Token::Character(rest));
                }
            }
            Token::StartTag(t) if t.name == "html" => {
                let el = Self::create_element(doc, &t, Namespace::Html);
                doc.append_child(self.document.expect("document"), el);
                self.open.push(el);
                self.mode = Mode::BeforeHead;
            }
            Token::EndTag(t) if !matches!(t.name.as_str(), "head" | "body" | "html" | "br") => {}
            other => {
                self.implied_html(doc);
                self.reprocess(doc, tok, other);
            }
        }
    }

    fn implied_html(&mut self, doc: &mut Document) {
        let el = doc.create_element("html");
        doc.append_child(self.document.expect("document"), el);
        self.open.push(el);
        self.mode = Mode::BeforeHead;
    }

    fn before_head(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(s) => {
                let (_, rest) = split_ws(&s);
                if !rest.is_empty() {
                    let rest = rest.to_string();
                    self.implied_head(doc);
                    self.reprocess(doc, tok, Token::Character(rest));
                }
            }
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                self.insert_comment(doc, &t)
            }
            Token::Doctype(_) => {}
            Token::StartTag(t) if t.name == "html" => self.in_body(doc, tok, Token::StartTag(t)),
            Token::StartTag(t) if t.name == "head" => {
                let head = self.insert_html(doc, &t);
                self.head = Some(head);
                self.mode = Mode::InHead;
            }
            Token::EndTag(t) if !matches!(t.name.as_str(), "head" | "body" | "html" | "br") => {}
            other => {
                self.implied_head(doc);
                self.reprocess(doc, tok, other);
            }
        }
    }

    fn implied_head(&mut self, doc: &mut Document) {
        let head = self.insert_html(doc, &tag("head"));
        self.head = Some(head);
        self.mode = Mode::InHead;
    }

    fn in_head(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(s) => {
                let (ws, rest) = split_ws(&s);
                self.insert_text(doc, ws);
                if !rest.is_empty() {
                    let rest = rest.to_string();
                    self.open.pop();
                    self.mode = Mode::AfterHead;
                    self.reprocess(doc, tok, Token::Character(rest));
                }
            }
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                self.insert_comment(doc, &t)
            }
            Token::Doctype(_) => {}
            Token::StartTag(t) => match t.name.as_str() {
                "html" => self.in_body(doc, tok, Token::StartTag(t)),
                "base" | "basefont" | "bgsound" | "link" | "meta" => self.insert_void(doc, &t),
                "title" => self.generic_text(doc, tok, &t, State::Rcdata),
                "noscript" if self.scripting => self.generic_text(doc, tok, &t, State::Rawtext),
                "noframes" | "style" => self.generic_text(doc, tok, &t, State::Rawtext),
                "noscript" => {
                    self.insert_html(doc, &t);
                    self.mode = Mode::InHeadNoscript;
                }
                "script" => self.generic_text(doc, tok, &t, State::ScriptData),
                "template" => {
                    self.formatting.push(Entry::Marker);
                    self.frameset_ok = false;
                    self.mode = Mode::InTemplate;
                    self.template_modes.push(Mode::InTemplate);
                    self.insert_template(doc, &t);
                }
                "head" => {}
                _ => self.in_head_anything_else(doc, tok, Token::StartTag(t)),
            },
            Token::EndTag(t) => match t.name.as_str() {
                "head" => {
                    self.open.pop();
                    self.mode = Mode::AfterHead;
                }
                "body" | "html" | "br" => self.in_head_anything_else(doc, tok, Token::EndTag(t)),
                "template" => {
                    if !self.template_on_stack(doc) {
                        return;
                    }
                    self.generate_all_implied_end_tags_thoroughly(doc);
                    self.pop_until(doc, &["template"]);
                    self.clear_formatting_to_marker();
                    self.template_modes.pop();
                    self.reset_insertion_mode(doc);
                }
                _ => {}
            },
            Token::Eof => self.in_head_anything_else(doc, tok, Token::Eof),
        }
    }

    fn in_head_anything_else(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        self.open.pop();
        self.mode = Mode::AfterHead;
        self.reprocess(doc, tok, token);
    }

    fn in_head_noscript(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Doctype(_) => {}
            Token::StartTag(t) if t.name == "html" => self.in_body(doc, tok, Token::StartTag(t)),
            Token::EndTag(t) if t.name == "noscript" => {
                self.open.pop();
                self.mode = Mode::InHead;
            }
            Token::Character(s) => {
                let (ws, rest) = split_ws(&s);
                self.insert_text(doc, ws);
                if !rest.is_empty() {
                    let rest = rest.to_string();
                    self.open.pop();
                    self.mode = Mode::InHead;
                    self.reprocess(doc, tok, Token::Character(rest));
                }
            }
            Token::Comment(_) | Token::ProcessingInstruction { .. } => {
                self.in_head(doc, tok, token)
            }
            Token::StartTag(ref t)
                if matches!(
                    t.name.as_str(),
                    "basefont" | "bgsound" | "link" | "meta" | "noframes" | "style"
                ) =>
            {
                self.in_head(doc, tok, token)
            }
            Token::StartTag(ref t) if matches!(t.name.as_str(), "head" | "noscript") => {}
            Token::EndTag(ref t) if t.name != "br" => {}
            other => {
                self.open.pop();
                self.mode = Mode::InHead;
                self.reprocess(doc, tok, other);
            }
        }
    }

    fn after_head(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(s) => {
                let (ws, rest) = split_ws(&s);
                self.insert_text(doc, ws);
                if !rest.is_empty() {
                    let rest = rest.to_string();
                    self.implied_body(doc);
                    self.reprocess(doc, tok, Token::Character(rest));
                }
            }
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                self.insert_comment(doc, &t)
            }
            Token::Doctype(_) => {}
            Token::StartTag(t) => match t.name.as_str() {
                "html" => self.in_body(doc, tok, Token::StartTag(t)),
                "body" => {
                    self.insert_html(doc, &t);
                    self.frameset_ok = false;
                    self.mode = Mode::InBody;
                }
                "frameset" => {
                    self.insert_html(doc, &t);
                    self.mode = Mode::InFrameset;
                }
                "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script"
                | "style" | "template" | "title" => {
                    let head = self.head.expect("head element pointer");
                    self.open.push(head);
                    self.in_head(doc, tok, Token::StartTag(t));
                    self.remove_from_stack(head);
                }
                "head" => {}
                _ => {
                    self.implied_body(doc);
                    self.reprocess(doc, tok, Token::StartTag(t));
                }
            },
            Token::EndTag(t) => match t.name.as_str() {
                "template" => self.in_head(doc, tok, Token::EndTag(t)),
                "body" | "html" | "br" => {
                    self.implied_body(doc);
                    self.reprocess(doc, tok, Token::EndTag(t));
                }
                _ => {}
            },
            Token::Eof => {
                self.implied_body(doc);
                self.reprocess(doc, tok, Token::Eof);
            }
        }
    }

    /// Template contents in `<head>` do not stop a later `<frameset>` from replacing the
    /// implied body (html5lib template.dat 44–45).
    fn implied_body(&mut self, doc: &mut Document) {
        self.insert_html(doc, &tag("body"));
        self.frameset_ok = true;
        self.mode = Mode::InBody;
    }

    fn in_body(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(s) => {
                let s: String = s.chars().filter(|&c| c != '\0').collect();
                if s.is_empty() {
                    return;
                }
                self.reconstruct_formatting(doc);
                self.insert_text(doc, &s);
                if !is_all_ws(&s) {
                    self.frameset_ok = false;
                }
            }
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                self.insert_comment(doc, &t)
            }
            Token::Doctype(_) => {}
            Token::StartTag(t) => self.in_body_start(doc, tok, t),
            Token::EndTag(t) => self.in_body_end(doc, tok, t),
            Token::Eof => {
                if !self.template_modes.is_empty() {
                    self.in_template(doc, tok, Token::Eof);
                } else {
                    self.stop();
                }
            }
        }
    }

    fn in_body_start(&mut self, doc: &mut Document, tok: &mut Tokenizer, mut t: Tag) {
        match t.name.as_str() {
            "html" => {
                if !self.template_on_stack(doc) {
                    let html = self.open[0];
                    Self::add_missing_attrs(doc, html, &t);
                }
            }
            "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script" | "style"
            | "template" | "title" => self.in_head(doc, tok, Token::StartTag(t)),
            "body" => {
                if self.open.len() == 1
                    || !Self::is_html(doc, self.open[1], &["body"])
                    || self.template_on_stack(doc)
                {
                    return;
                }
                self.frameset_ok = false;
                let body = self.open[1];
                Self::add_missing_attrs(doc, body, &t);
            }
            "frameset" => {
                if self.open.len() == 1
                    || !Self::is_html(doc, self.open[1], &["body"])
                    || !self.frameset_ok
                {
                    return;
                }
                let body = self.open[1];
                if let Some(parent) = doc.get(body).parent {
                    doc.remove_child(parent, body);
                }
                self.open.truncate(1);
                self.insert_html(doc, &t);
                self.mode = Mode::InFrameset;
            }
            "address" | "article" | "aside" | "blockquote" | "center" | "details" | "dialog"
            | "dir" | "div" | "dl" | "fieldset" | "figcaption" | "figure" | "footer" | "header"
            | "hgroup" | "main" | "menu" | "nav" | "ol" | "p" | "search" | "section"
            | "summary" | "ul" => {
                self.close_p_in_button_scope(doc);
                self.insert_html(doc, &t);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.close_p_in_button_scope(doc);
                if self.current_is(doc, &["h1", "h2", "h3", "h4", "h5", "h6"]) {
                    self.open.pop();
                }
                self.insert_html(doc, &t);
            }
            "pre" | "listing" => {
                self.close_p_in_button_scope(doc);
                self.insert_html(doc, &t);
                self.skip_newline = true;
                self.frameset_ok = false;
            }
            "form" => {
                let template = self.inside_template(doc);
                if self.form.is_some() && !template {
                    return;
                }
                self.close_p_in_button_scope(doc);
                let form = self.insert_html(doc, &t);
                if !template {
                    self.form = Some(form);
                }
            }
            "li" | "dd" | "dt" => {
                self.frameset_ok = false;
                let names: &[&str] = if t.name == "li" {
                    &["li"]
                } else {
                    &["dd", "dt"]
                };
                for i in (0..self.open.len()).rev() {
                    let node = self.open[i];
                    if let Some(name) = Self::html_name(doc, node).filter(|n| names.contains(n)) {
                        let name = name.to_string();
                        self.generate_implied_end_tags(doc, Some(&name));
                        self.pop_until(doc, &[name.as_str()]);
                        break;
                    }
                    if Self::is_special(doc, node)
                        && !Self::is_html(doc, node, &["address", "div", "p"])
                    {
                        break;
                    }
                }
                self.close_p_in_button_scope(doc);
                self.insert_html(doc, &t);
            }
            "plaintext" => {
                self.close_p_in_button_scope(doc);
                self.insert_html(doc, &t);
                tok.set_state(State::Plaintext);
            }
            "button" => {
                if self.in_scope(doc, &["button"], Scope::Default) {
                    self.generate_implied_end_tags(doc, None);
                    self.pop_until(doc, &["button"]);
                }
                self.reconstruct_formatting(doc);
                self.insert_html(doc, &t);
                self.frameset_ok = false;
            }
            "a" => {
                let existing = self.formatting.iter().rev().find_map(|e| match e {
                    Entry::Marker => Some(None),
                    Entry::Element(n, _) if Self::html_name(doc, *n) == Some("a") => Some(Some(*n)),
                    _ => None,
                });
                if let Some(Some(a)) = existing {
                    self.adoption_agency(doc, "a");
                    if let Some(i) = self.formatting_index(a) {
                        self.formatting.remove(i);
                    }
                    self.remove_from_stack(a);
                }
                self.reconstruct_formatting(doc);
                let el = self.insert_html(doc, &t);
                self.push_formatting(doc, el, &t);
            }
            "b" | "big" | "code" | "em" | "font" | "i" | "s" | "small" | "strike" | "strong"
            | "tt" | "u" => {
                self.reconstruct_formatting(doc);
                let el = self.insert_html(doc, &t);
                self.push_formatting(doc, el, &t);
            }
            "nobr" => {
                self.reconstruct_formatting(doc);
                if self.in_scope(doc, &["nobr"], Scope::Default) {
                    if !self.adoption_agency(doc, "nobr") {
                        self.any_other_end_tag(doc, "nobr");
                    }
                    self.reconstruct_formatting(doc);
                }
                let el = self.insert_html(doc, &t);
                self.push_formatting(doc, el, &t);
            }
            "applet" | "marquee" | "object" => {
                self.reconstruct_formatting(doc);
                self.insert_html(doc, &t);
                self.formatting.push(Entry::Marker);
                self.frameset_ok = false;
            }
            "table" => {
                if doc.quirks_mode != QuirksMode::Quirks {
                    self.close_p_in_button_scope(doc);
                }
                self.insert_html(doc, &t);
                self.frameset_ok = false;
                self.mode = Mode::InTable;
            }
            "area" | "br" | "embed" | "img" | "keygen" | "wbr" => {
                self.reconstruct_formatting(doc);
                self.insert_void(doc, &t);
                self.frameset_ok = false;
            }
            "input" => {
                if self.fragment_select_context(doc) {
                    return;
                }
                if self.in_scope(doc, &["select"], Scope::Default) {
                    self.pop_until(doc, &["select"]);
                }
                self.reconstruct_formatting(doc);
                self.insert_void(doc, &t);
                if !t
                    .attr("type")
                    .is_some_and(|v| v.eq_ignore_ascii_case("hidden"))
                {
                    self.frameset_ok = false;
                }
            }
            "param" | "source" | "track" => self.insert_void(doc, &t),
            "hr" => {
                self.close_p_in_button_scope(doc);
                if self.in_scope(doc, &["select"], Scope::Default) {
                    self.generate_implied_end_tags(doc, None);
                }
                self.insert_void(doc, &t);
                self.frameset_ok = false;
            }
            "image" => {
                t.name = "img".into();
                self.in_body_start(doc, tok, t);
            }
            "textarea" => {
                self.insert_html(doc, &t);
                self.skip_newline = true;
                tok.set_state(State::Rcdata);
                self.original_mode = self.mode;
                self.frameset_ok = false;
                self.mode = Mode::Text;
            }
            "xmp" => {
                self.close_p_in_button_scope(doc);
                self.reconstruct_formatting(doc);
                self.frameset_ok = false;
                self.generic_text(doc, tok, &t, State::Rawtext);
            }
            "iframe" => {
                self.frameset_ok = false;
                self.generic_text(doc, tok, &t, State::Rawtext);
            }
            "noembed" => self.generic_text(doc, tok, &t, State::Rawtext),
            "noscript" if self.scripting => self.generic_text(doc, tok, &t, State::Rawtext),
            "select" => {
                if self.fragment_select_context(doc) {
                    return;
                }
                if self.in_scope(doc, &["select"], Scope::Default) {
                    self.pop_until(doc, &["select"]);
                    return;
                }
                self.reconstruct_formatting(doc);
                self.insert_html(doc, &t);
                self.frameset_ok = false;
            }
            "option" => {
                if self.in_scope(doc, &["select"], Scope::Default) {
                    self.generate_implied_end_tags(doc, Some("optgroup"));
                } else if self.current_is(doc, &["option"]) {
                    self.open.pop();
                }
                self.reconstruct_formatting(doc);
                self.insert_html(doc, &t);
            }
            "optgroup" => {
                if self.in_scope(doc, &["select"], Scope::Default) {
                    self.generate_implied_end_tags(doc, None);
                } else if self.current_is(doc, &["option"]) {
                    self.open.pop();
                }
                self.reconstruct_formatting(doc);
                self.insert_html(doc, &t);
            }
            "rb" | "rtc" => {
                if self.in_scope(doc, &["ruby"], Scope::Default) {
                    self.generate_implied_end_tags(doc, None);
                }
                self.insert_html(doc, &t);
            }
            "rp" | "rt" => {
                if self.in_scope(doc, &["ruby"], Scope::Default) {
                    self.generate_implied_end_tags(doc, Some("rtc"));
                }
                self.insert_html(doc, &t);
            }
            "math" | "svg" => {
                self.reconstruct_formatting(doc);
                let ns = if t.name == "math" {
                    Namespace::MathMl
                } else {
                    Namespace::Svg
                };
                adjust_foreign_tag(&mut t, ns);
                self.insert_element(doc, &t, ns);
                if t.self_closing {
                    self.open.pop();
                }
            }
            "caption" | "col" | "colgroup" | "frame" | "head" | "tbody" | "td" | "tfoot" | "th"
            | "thead" | "tr" => {}
            _ => {
                self.reconstruct_formatting(doc);
                self.insert_html(doc, &t);
            }
        }
    }

    fn fragment_select_context(&self, doc: &Document) -> bool {
        self.context
            .is_some_and(|c| Self::is_html(doc, c, &["select"]))
    }

    fn in_body_end(&mut self, doc: &mut Document, tok: &mut Tokenizer, t: Tag) {
        match t.name.as_str() {
            "template" => self.in_head(doc, tok, Token::EndTag(t)),
            "body" => {
                if self.in_scope(doc, &["body"], Scope::Default) {
                    self.mode = Mode::AfterBody;
                }
            }
            "html" => {
                if self.in_scope(doc, &["body"], Scope::Default) {
                    self.mode = Mode::AfterBody;
                    self.reprocess(doc, tok, Token::EndTag(t));
                }
            }
            "address" | "article" | "aside" | "blockquote" | "button" | "center" | "details"
            | "dialog" | "dir" | "div" | "dl" | "fieldset" | "figcaption" | "figure" | "footer"
            | "header" | "hgroup" | "listing" | "main" | "menu" | "nav" | "ol" | "pre"
            | "search" | "section" | "select" | "summary" | "ul" => {
                if !self.in_scope(doc, &[t.name.as_str()], Scope::Default) {
                    return;
                }
                self.generate_implied_end_tags(doc, None);
                self.pop_until(doc, &[t.name.as_str()]);
            }
            "form" => {
                if !self.inside_template(doc) {
                    let node = self.form.take();
                    let Some(node) = node.filter(|&n| self.node_in_scope(doc, n)) else {
                        return;
                    };
                    self.generate_implied_end_tags(doc, None);
                    self.remove_from_stack(node);
                } else {
                    if !self.in_scope(doc, &["form"], Scope::Default) {
                        return;
                    }
                    self.generate_implied_end_tags(doc, None);
                    self.pop_until(doc, &["form"]);
                }
            }
            "p" => {
                if !self.in_scope(doc, &["p"], Scope::Button) {
                    self.insert_html(doc, &tag("p"));
                }
                self.close_p(doc);
            }
            "li" => {
                if !self.in_scope(doc, &["li"], Scope::ListItem) {
                    return;
                }
                self.generate_implied_end_tags(doc, Some("li"));
                self.pop_until(doc, &["li"]);
            }
            "dd" | "dt" => {
                if !self.in_scope(doc, &[t.name.as_str()], Scope::Default) {
                    return;
                }
                self.generate_implied_end_tags(doc, Some(t.name.as_str()));
                self.pop_until(doc, &[t.name.as_str()]);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                const H: [&str; 6] = ["h1", "h2", "h3", "h4", "h5", "h6"];
                if !self.in_scope(doc, &H, Scope::Default) {
                    return;
                }
                self.generate_implied_end_tags(doc, None);
                self.pop_until(doc, &H);
            }
            "a" | "b" | "big" | "code" | "em" | "font" | "i" | "nobr" | "s" | "small"
            | "strike" | "strong" | "tt" | "u" => {
                if !self.adoption_agency(doc, &t.name) {
                    self.any_other_end_tag(doc, &t.name);
                }
            }
            "applet" | "marquee" | "object" => {
                if !self.in_scope(doc, &[t.name.as_str()], Scope::Default) {
                    return;
                }
                self.generate_implied_end_tags(doc, None);
                self.pop_until(doc, &[t.name.as_str()]);
                self.clear_formatting_to_marker();
            }
            "br" => self.in_body_start(doc, tok, tag("br")),
            _ => self.any_other_end_tag(doc, &t.name),
        }
    }

    fn any_other_end_tag(&mut self, doc: &Document, name: &str) {
        for i in (0..self.open.len()).rev() {
            let node = self.open[i];
            if Self::html_name(doc, node) == Some(name) {
                self.generate_implied_end_tags(doc, Some(name));
                self.pop_until_node(node);
                return;
            }
            if Self::is_special(doc, node) {
                return;
            }
        }
    }

    fn text(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(s) => self.insert_text(doc, &s),
            Token::Eof => {
                self.pop_text_element(doc);
                self.mode = self.original_mode;
                self.reprocess(doc, tok, Token::Eof);
            }
            Token::EndTag(t) => {
                let node = self.pop_text_element(doc);
                self.mode = self.original_mode;
                if t.name == "script" && node.is_some_and(|n| Self::is_html(doc, n, &["script"])) {
                    self.script = node;
                }
            }
            _ => {}
        }
    }

    fn pop_text_element(&mut self, doc: &Document) -> Option<NodeId> {
        let node = self.open.pop();
        if let Some(n) = node.filter(|&n| Self::is_html(doc, n, &["style"])) {
            self.discovered.push(n);
        }
        node
    }

    fn in_table(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(_)
                if self
                    .current_is(doc, &["table", "tbody", "template", "tfoot", "thead", "tr"]) =>
            {
                self.table_text.clear();
                self.original_mode = self.mode;
                self.mode = Mode::InTableText;
                self.reprocess(doc, tok, token);
            }
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                self.insert_comment(doc, &t)
            }
            Token::Doctype(_) => {}
            Token::StartTag(t) => match t.name.as_str() {
                "caption" => {
                    self.clear_stack_back_to(doc, &["table", "template", "html"]);
                    self.formatting.push(Entry::Marker);
                    self.insert_html(doc, &t);
                    self.mode = Mode::InCaption;
                }
                "colgroup" => {
                    self.clear_stack_back_to(doc, &["table", "template", "html"]);
                    self.insert_html(doc, &t);
                    self.mode = Mode::InColumnGroup;
                }
                "col" => {
                    self.clear_stack_back_to(doc, &["table", "template", "html"]);
                    self.insert_html(doc, &tag("colgroup"));
                    self.mode = Mode::InColumnGroup;
                    self.reprocess(doc, tok, Token::StartTag(t));
                }
                "tbody" | "tfoot" | "thead" => {
                    self.clear_stack_back_to(doc, &["table", "template", "html"]);
                    self.insert_html(doc, &t);
                    self.mode = Mode::InTableBody;
                }
                "td" | "th" | "tr" => {
                    self.clear_stack_back_to(doc, &["table", "template", "html"]);
                    self.insert_html(doc, &tag("tbody"));
                    self.mode = Mode::InTableBody;
                    self.reprocess(doc, tok, Token::StartTag(t));
                }
                "table" => {
                    if !self.in_scope(doc, &["table"], Scope::Table) {
                        return;
                    }
                    self.pop_until(doc, &["table"]);
                    self.reset_insertion_mode(doc);
                    self.reprocess(doc, tok, Token::StartTag(t));
                }
                "style" | "script" | "template" => self.in_head(doc, tok, Token::StartTag(t)),
                "input"
                    if t.attr("type")
                        .is_some_and(|v| v.eq_ignore_ascii_case("hidden")) =>
                {
                    self.insert_void(doc, &t);
                }
                "form" => {
                    let template = self.inside_template(doc);
                    if self.form.is_some() && !template {
                        return;
                    }
                    let form = self.insert_html(doc, &t);
                    if !template {
                        self.form = Some(form);
                    }
                    self.open.pop();
                }
                _ => self.in_table_anything_else(doc, tok, Token::StartTag(t)),
            },
            Token::EndTag(t) => match t.name.as_str() {
                "table" => {
                    if !self.in_scope(doc, &["table"], Scope::Table) {
                        return;
                    }
                    self.pop_until(doc, &["table"]);
                    self.reset_insertion_mode(doc);
                }
                "body" | "caption" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot"
                | "th" | "thead" | "tr" => {}
                "template" => self.in_head(doc, tok, Token::EndTag(t)),
                _ => self.in_table_anything_else(doc, tok, Token::EndTag(t)),
            },
            Token::Eof => self.in_body(doc, tok, Token::Eof),
            other => self.in_table_anything_else(doc, tok, other),
        }
    }

    fn in_table_anything_else(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        self.foster_parenting = true;
        self.in_body(doc, tok, token);
        self.foster_parenting = false;
    }

    fn in_table_text(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(s) => self.table_text.extend(s.chars().filter(|&c| c != '\0')),
            other => {
                let text = std::mem::take(&mut self.table_text);
                if !is_all_ws(&text) {
                    self.in_table_anything_else(doc, tok, Token::Character(text));
                } else {
                    self.insert_text(doc, &text);
                }
                self.mode = self.original_mode;
                self.reprocess(doc, tok, other);
            }
        }
    }

    fn in_caption(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        let closes_caption = match &token {
            Token::EndTag(t) if t.name == "caption" => Some(false),
            Token::StartTag(t)
                if matches!(
                    t.name.as_str(),
                    "caption"
                        | "col"
                        | "colgroup"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) =>
            {
                Some(true)
            }
            Token::EndTag(t) if t.name == "table" => Some(true),
            _ => None,
        };
        if let Some(reprocess) = closes_caption {
            if !self.in_scope(doc, &["caption"], Scope::Table) {
                return;
            }
            self.generate_implied_end_tags(doc, None);
            self.pop_until(doc, &["caption"]);
            self.clear_formatting_to_marker();
            self.mode = Mode::InTable;
            if reprocess {
                self.reprocess(doc, tok, token);
            }
            return;
        }
        match token {
            Token::EndTag(ref t)
                if matches!(
                    t.name.as_str(),
                    "body"
                        | "col"
                        | "colgroup"
                        | "html"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) => {}
            other => self.in_body(doc, tok, other),
        }
    }

    fn in_column_group(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(s) => {
                let (ws, rest) = split_ws(&s);
                self.insert_text(doc, ws);
                if !rest.is_empty() {
                    let rest = rest.to_string();
                    self.column_group_anything_else(doc, tok, Token::Character(rest));
                }
            }
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                self.insert_comment(doc, &t)
            }
            Token::Doctype(_) => {}
            Token::StartTag(t) if t.name == "html" => self.in_body(doc, tok, Token::StartTag(t)),
            Token::StartTag(t) if t.name == "col" => self.insert_void(doc, &t),
            Token::EndTag(t) if t.name == "colgroup" => {
                if self.current_is(doc, &["colgroup"]) {
                    self.open.pop();
                    self.mode = Mode::InTable;
                }
            }
            Token::EndTag(t) if t.name == "col" => {}
            Token::StartTag(t) if t.name == "template" => {
                self.in_head(doc, tok, Token::StartTag(t))
            }
            Token::EndTag(t) if t.name == "template" => self.in_head(doc, tok, Token::EndTag(t)),
            Token::Eof => self.in_body(doc, tok, Token::Eof),
            other => self.column_group_anything_else(doc, tok, other),
        }
    }

    fn column_group_anything_else(
        &mut self,
        doc: &mut Document,
        tok: &mut Tokenizer,
        token: Token,
    ) {
        if !self.current_is(doc, &["colgroup"]) {
            return;
        }
        self.open.pop();
        self.mode = Mode::InTable;
        self.reprocess(doc, tok, token);
    }

    fn in_table_body(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        const CONTEXT: [&str; 5] = ["tbody", "tfoot", "thead", "template", "html"];
        match token {
            Token::StartTag(t) if t.name == "tr" => {
                self.clear_stack_back_to(doc, &CONTEXT);
                self.insert_html(doc, &t);
                self.mode = Mode::InRow;
            }
            Token::StartTag(t) if t.name == "th" || t.name == "td" => {
                self.clear_stack_back_to(doc, &CONTEXT);
                self.insert_html(doc, &tag("tr"));
                self.mode = Mode::InRow;
                self.reprocess(doc, tok, Token::StartTag(t));
            }
            Token::EndTag(t) if matches!(t.name.as_str(), "tbody" | "tfoot" | "thead") => {
                if !self.in_scope(doc, &[t.name.as_str()], Scope::Table) {
                    return;
                }
                self.clear_stack_back_to(doc, &CONTEXT);
                self.open.pop();
                self.mode = Mode::InTable;
            }
            Token::StartTag(ref t)
                if matches!(
                    t.name.as_str(),
                    "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead"
                ) =>
            {
                self.table_body_close_and_reprocess(doc, tok, token)
            }
            Token::EndTag(ref t) if t.name == "table" => {
                self.table_body_close_and_reprocess(doc, tok, token)
            }
            Token::EndTag(ref t)
                if matches!(
                    t.name.as_str(),
                    "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th" | "tr"
                ) => {}
            other => self.in_table(doc, tok, other),
        }
    }

    fn table_body_close_and_reprocess(
        &mut self,
        doc: &mut Document,
        tok: &mut Tokenizer,
        token: Token,
    ) {
        if !self.in_scope(doc, &["tbody", "thead", "tfoot"], Scope::Table) {
            return;
        }
        self.clear_stack_back_to(doc, &["tbody", "tfoot", "thead", "template", "html"]);
        self.open.pop();
        self.mode = Mode::InTable;
        self.reprocess(doc, tok, token);
    }

    fn in_row(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        const CONTEXT: [&str; 3] = ["tr", "template", "html"];
        match token {
            Token::StartTag(t) if t.name == "th" || t.name == "td" => {
                self.clear_stack_back_to(doc, &CONTEXT);
                self.insert_html(doc, &t);
                self.mode = Mode::InCell;
                self.formatting.push(Entry::Marker);
            }
            Token::EndTag(t) if t.name == "tr" => {
                if !self.in_scope(doc, &["tr"], Scope::Table) {
                    return;
                }
                self.clear_stack_back_to(doc, &CONTEXT);
                self.open.pop();
                self.mode = Mode::InTableBody;
            }
            Token::StartTag(ref t)
                if matches!(
                    t.name.as_str(),
                    "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead" | "tr"
                ) =>
            {
                self.row_close_and_reprocess(doc, tok, token)
            }
            Token::EndTag(ref t) if t.name == "table" => {
                self.row_close_and_reprocess(doc, tok, token)
            }
            Token::EndTag(ref t) if matches!(t.name.as_str(), "tbody" | "tfoot" | "thead") => {
                if !self.in_scope(doc, &[t.name.as_str()], Scope::Table) {
                    return;
                }
                self.row_close_and_reprocess(doc, tok, token)
            }
            Token::EndTag(ref t)
                if matches!(
                    t.name.as_str(),
                    "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th"
                ) => {}
            other => self.in_table(doc, tok, other),
        }
    }

    fn row_close_and_reprocess(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        if !self.in_scope(doc, &["tr"], Scope::Table) {
            return;
        }
        self.clear_stack_back_to(doc, &["tr", "template", "html"]);
        self.open.pop();
        self.mode = Mode::InTableBody;
        self.reprocess(doc, tok, token);
    }

    fn in_cell(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::EndTag(t) if t.name == "td" || t.name == "th" => {
                if !self.in_scope(doc, &[t.name.as_str()], Scope::Table) {
                    return;
                }
                self.generate_implied_end_tags(doc, None);
                self.pop_until(doc, &[t.name.as_str()]);
                self.clear_formatting_to_marker();
                self.mode = Mode::InRow;
            }
            Token::StartTag(ref t)
                if matches!(
                    t.name.as_str(),
                    "caption"
                        | "col"
                        | "colgroup"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) =>
            {
                if !self.in_scope(doc, &["td", "th"], Scope::Table) {
                    return;
                }
                self.close_cell(doc);
                self.reprocess(doc, tok, token);
            }
            Token::EndTag(ref t)
                if matches!(
                    t.name.as_str(),
                    "body" | "caption" | "col" | "colgroup" | "html"
                ) => {}
            Token::EndTag(ref t)
                if matches!(
                    t.name.as_str(),
                    "table" | "tbody" | "tfoot" | "thead" | "tr"
                ) =>
            {
                if !self.in_scope(doc, &[t.name.as_str()], Scope::Table) {
                    return;
                }
                self.close_cell(doc);
                self.reprocess(doc, tok, token);
            }
            other => self.in_body(doc, tok, other),
        }
    }

    fn close_cell(&mut self, doc: &Document) {
        self.generate_implied_end_tags(doc, None);
        self.pop_until(doc, &["td", "th"]);
        self.clear_formatting_to_marker();
        self.mode = Mode::InRow;
    }

    fn in_template(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(_)
            | Token::Comment(_)
            | Token::ProcessingInstruction { .. }
            | Token::Doctype(_) => self.in_body(doc, tok, token),
            Token::StartTag(ref t) => {
                let next = match t.name.as_str() {
                    "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script"
                    | "style" | "template" | "title" => return self.in_head(doc, tok, token),
                    "caption" | "colgroup" | "tbody" | "tfoot" | "thead" => Mode::InTable,
                    "col" => Mode::InColumnGroup,
                    "tr" => Mode::InTableBody,
                    "td" | "th" => Mode::InRow,
                    _ => Mode::InBody,
                };
                self.template_modes.pop();
                self.template_modes.push(next);
                self.mode = next;
                self.reprocess(doc, tok, token);
            }
            Token::EndTag(ref t) if t.name == "template" => self.in_head(doc, tok, token),
            Token::EndTag(_) => {}
            Token::Eof => {
                if !self.template_on_stack(doc) {
                    self.stop();
                    return;
                }
                self.pop_until(doc, &["template"]);
                self.clear_formatting_to_marker();
                self.template_modes.pop();
                self.reset_insertion_mode(doc);
                self.reprocess(doc, tok, Token::Eof);
            }
        }
    }

    fn after_body(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(ref s) if is_all_ws(s) => self.in_body(doc, tok, token),
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                let html = self.open[0];
                Self::insert_comment_in(doc, html, &t);
            }
            Token::Doctype(_) => {}
            Token::StartTag(ref t) if t.name == "html" => self.in_body(doc, tok, token),
            Token::EndTag(ref t) if t.name == "html" => {
                if self.context.is_none() {
                    self.mode = Mode::AfterAfterBody;
                }
            }
            Token::Eof => self.stop(),
            Token::Character(s) => {
                let (ws, rest) = split_ws(&s);
                let rest = rest.to_string();
                if !ws.is_empty() {
                    self.in_body(doc, tok, Token::Character(ws.to_string()));
                }
                self.mode = Mode::InBody;
                self.reprocess(doc, tok, Token::Character(rest));
            }
            other => {
                self.mode = Mode::InBody;
                self.reprocess(doc, tok, other);
            }
        }
    }

    fn in_frameset(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(s) => {
                let ws: String = s.chars().filter(|&c| is_ws(c)).collect();
                self.insert_text(doc, &ws);
            }
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                self.insert_comment(doc, &t)
            }
            Token::Doctype(_) => {}
            Token::StartTag(t) => match t.name.as_str() {
                "html" => self.in_body(doc, tok, Token::StartTag(t)),
                "frameset" => {
                    self.insert_html(doc, &t);
                }
                "frame" => self.insert_void(doc, &t),
                "noframes" => self.in_head(doc, tok, Token::StartTag(t)),
                _ => {}
            },
            Token::EndTag(t) if t.name == "frameset" => {
                if self.open.len() <= 1 {
                    return;
                }
                self.open.pop();
                if self.context.is_none() && !self.current_is(doc, &["frameset"]) {
                    self.mode = Mode::AfterFrameset;
                }
            }
            Token::EndTag(_) => {}
            Token::Eof => self.stop(),
        }
    }

    fn after_frameset(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(s) => {
                let ws: String = s.chars().filter(|&c| is_ws(c)).collect();
                self.insert_text(doc, &ws);
            }
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                self.insert_comment(doc, &t)
            }
            Token::Doctype(_) => {}
            Token::StartTag(ref t) if t.name == "html" => self.in_body(doc, tok, token),
            Token::EndTag(ref t) if t.name == "html" => self.mode = Mode::AfterAfterFrameset,
            Token::StartTag(ref t) if t.name == "noframes" => self.in_head(doc, tok, token),
            Token::Eof => self.stop(),
            _ => {}
        }
    }

    fn after_after_body(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                let root = self.document.expect("document");
                Self::insert_comment_in(doc, root, &t);
            }
            Token::Doctype(_) => self.in_body(doc, tok, token),
            Token::Character(ref s) if is_all_ws(s) => self.in_body(doc, tok, token),
            Token::StartTag(ref t) if t.name == "html" => self.in_body(doc, tok, token),
            Token::Eof => self.stop(),
            Token::Character(s) => {
                let (ws, rest) = split_ws(&s);
                let rest = rest.to_string();
                if !ws.is_empty() {
                    self.in_body(doc, tok, Token::Character(ws.to_string()));
                }
                self.mode = Mode::InBody;
                self.reprocess(doc, tok, Token::Character(rest));
            }
            other => {
                self.mode = Mode::InBody;
                self.reprocess(doc, tok, other);
            }
        }
    }

    fn after_after_frameset(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                let root = self.document.expect("document");
                Self::insert_comment_in(doc, root, &t);
            }
            Token::Doctype(_) => self.in_body(doc, tok, token),
            Token::Character(s) => {
                let ws: String = s.chars().filter(|&c| is_ws(c)).collect();
                if !ws.is_empty() {
                    self.in_body(doc, tok, Token::Character(ws));
                }
            }
            Token::StartTag(ref t) if t.name == "html" => self.in_body(doc, tok, token),
            Token::StartTag(ref t) if t.name == "noframes" => self.in_head(doc, tok, token),
            Token::Eof => self.stop(),
            _ => {}
        }
    }

    // ----- foreign content ----------------------------------------------------------

    fn foreign_content(&mut self, doc: &mut Document, tok: &mut Tokenizer, token: Token) {
        match token {
            Token::Character(s) => {
                if s.chars().any(|c| !is_ws(c) && c != '\0') {
                    self.frameset_ok = false;
                }
                let s: String = s
                    .chars()
                    .map(|c| if c == '\0' { '\u{FFFD}' } else { c })
                    .collect();
                self.insert_text(doc, &s);
            }
            t @ (Token::Comment(_) | Token::ProcessingInstruction { .. }) => {
                self.insert_comment(doc, &t)
            }
            Token::Doctype(_) => {}
            Token::StartTag(ref t) if breaks_out_of_foreign_content(t) => {
                self.pop_to_html_or_integration_point(doc);
                self.reprocess(doc, tok, token);
            }
            Token::EndTag(ref t) if t.name == "br" || t.name == "p" => {
                self.pop_to_html_or_integration_point(doc);
                self.reprocess(doc, tok, token);
            }
            Token::StartTag(mut t) => {
                let ns = self
                    .adjusted_current()
                    .and_then(|n| doc.namespace(n))
                    .unwrap_or(Namespace::Html);
                adjust_foreign_tag(&mut t, ns);
                self.insert_element(doc, &t, ns);
                if t.self_closing {
                    self.open.pop();
                }
            }
            Token::EndTag(t) => {
                let mut i = self.open.len();
                while i > 0 {
                    i -= 1;
                    let node = self.open[i];
                    if i == 0 {
                        return;
                    }
                    if doc
                        .tag_name(node)
                        .is_some_and(|n| n.eq_ignore_ascii_case(&t.name))
                        && doc.namespace(node) != Some(Namespace::Html)
                    {
                        self.pop_until_node(node);
                        return;
                    }
                    if i > 0 && doc.namespace(self.open[i - 1]) == Some(Namespace::Html) {
                        self.process_in(doc, tok, self.mode, Token::EndTag(t));
                        return;
                    }
                }
            }
            Token::Eof => self.reprocess(doc, tok, Token::Eof),
        }
    }

    fn pop_to_html_or_integration_point(&mut self, doc: &Document) {
        while let Some(node) = self.current() {
            let html = doc.namespace(node) == Some(Namespace::Html);
            let mathml_text_ip = doc.namespace(node) == Some(Namespace::MathMl)
                && matches!(
                    doc.tag_name(node),
                    Some("mi" | "mo" | "mn" | "ms" | "mtext")
                );
            if html || mathml_text_ip || Self::is_html_integration_point(doc, node) {
                break;
            }
            self.open.pop();
        }
    }
}

fn same_attrs(a: &Tag, b: &Tag) -> bool {
    a.attrs.len() == b.attrs.len()
        && a.attrs.iter().all(|x| {
            b.attrs
                .iter()
                .any(|y| y.name == x.name && y.value == x.value)
        })
}

fn breaks_out_of_foreign_content(t: &Tag) -> bool {
    match t.name.as_str() {
        "b" | "big" | "blockquote" | "body" | "br" | "center" | "code" | "dd" | "div" | "dl"
        | "dt" | "em" | "embed" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "head" | "hr" | "i"
        | "img" | "li" | "listing" | "menu" | "meta" | "nobr" | "ol" | "p" | "pre" | "ruby"
        | "s" | "small" | "span" | "strong" | "strike" | "sub" | "sup" | "table" | "tt" | "u"
        | "ul" | "var" => true,
        "font" => t
            .attrs
            .iter()
            .any(|a| matches!(a.name.as_str(), "color" | "face" | "size")),
        _ => false,
    }
}

/// SVG / MathML tag-name and attribute-name case adjustments, and qualified names for
/// the `xlink:` / `xml:` / `xmlns` attributes (§13.2.6.1 "adjust ... attributes").
fn adjust_foreign_tag(t: &mut Tag, ns: Namespace) {
    match ns {
        Namespace::Svg => {
            if let Some(&(_, adjusted)) = SVG_TAGS.iter().find(|(lower, _)| *lower == t.name) {
                t.name = adjusted.to_string();
            }
            for a in &mut t.attrs {
                if let Some(&(_, adjusted)) = SVG_ATTRS.iter().find(|(lower, _)| *lower == a.name) {
                    a.name = adjusted.to_string();
                }
            }
        }
        Namespace::MathMl => {
            for a in &mut t.attrs {
                if a.name == "definitionurl" {
                    a.name = "definitionURL".to_string();
                }
            }
        }
        Namespace::Html | Namespace::Null | Namespace::Other(_) => {}
    }
}

/// The namespace, prefix and local name "adjust foreign attributes" gives an attribute of
/// a foreign element.
fn foreign_attribute(name: &str) -> Option<(&'static str, Option<&str>, &str)> {
    const XLINK: [&str; 7] = [
        "actuate", "arcrole", "href", "role", "show", "title", "type",
    ];
    match name.split_once(':') {
        Some(("xlink", local)) if XLINK.contains(&local) => {
            Some((XLINK_NAMESPACE, Some("xlink"), local))
        }
        Some(("xml", local @ ("lang" | "space"))) => Some((XML_NAMESPACE, Some("xml"), local)),
        Some(("xmlns", "xlink")) => Some((XMLNS_NAMESPACE, Some("xmlns"), "xlink")),
        None if name == "xmlns" => Some((XMLNS_NAMESPACE, None, "xmlns")),
        _ => None,
    }
}

const SVG_TAGS: [(&str, &str); 37] = [
    ("altglyph", "altGlyph"),
    ("altglyphdef", "altGlyphDef"),
    ("altglyphitem", "altGlyphItem"),
    ("animatecolor", "animateColor"),
    ("animatemotion", "animateMotion"),
    ("animatetransform", "animateTransform"),
    ("clippath", "clipPath"),
    ("feblend", "feBlend"),
    ("fecolormatrix", "feColorMatrix"),
    ("fecomponenttransfer", "feComponentTransfer"),
    ("fecomposite", "feComposite"),
    ("feconvolvematrix", "feConvolveMatrix"),
    ("fediffuselighting", "feDiffuseLighting"),
    ("fedisplacementmap", "feDisplacementMap"),
    ("fedistantlight", "feDistantLight"),
    ("fedropshadow", "feDropShadow"),
    ("feflood", "feFlood"),
    ("fefunca", "feFuncA"),
    ("fefuncb", "feFuncB"),
    ("fefuncg", "feFuncG"),
    ("fefuncr", "feFuncR"),
    ("fegaussianblur", "feGaussianBlur"),
    ("feimage", "feImage"),
    ("femerge", "feMerge"),
    ("femergenode", "feMergeNode"),
    ("femorphology", "feMorphology"),
    ("feoffset", "feOffset"),
    ("fepointlight", "fePointLight"),
    ("fespecularlighting", "feSpecularLighting"),
    ("fespotlight", "feSpotLight"),
    ("fetile", "feTile"),
    ("feturbulence", "feTurbulence"),
    ("foreignobject", "foreignObject"),
    ("glyphref", "glyphRef"),
    ("lineargradient", "linearGradient"),
    ("radialgradient", "radialGradient"),
    ("textpath", "textPath"),
];

const SVG_ATTRS: [(&str, &str); 58] = [
    ("attributename", "attributeName"),
    ("attributetype", "attributeType"),
    ("basefrequency", "baseFrequency"),
    ("baseprofile", "baseProfile"),
    ("calcmode", "calcMode"),
    ("clippathunits", "clipPathUnits"),
    ("diffuseconstant", "diffuseConstant"),
    ("edgemode", "edgeMode"),
    ("filterunits", "filterUnits"),
    ("glyphref", "glyphRef"),
    ("gradienttransform", "gradientTransform"),
    ("gradientunits", "gradientUnits"),
    ("kernelmatrix", "kernelMatrix"),
    ("kernelunitlength", "kernelUnitLength"),
    ("keypoints", "keyPoints"),
    ("keysplines", "keySplines"),
    ("keytimes", "keyTimes"),
    ("lengthadjust", "lengthAdjust"),
    ("limitingconeangle", "limitingConeAngle"),
    ("markerheight", "markerHeight"),
    ("markerunits", "markerUnits"),
    ("markerwidth", "markerWidth"),
    ("maskcontentunits", "maskContentUnits"),
    ("maskunits", "maskUnits"),
    ("numoctaves", "numOctaves"),
    ("pathlength", "pathLength"),
    ("patterncontentunits", "patternContentUnits"),
    ("patterntransform", "patternTransform"),
    ("patternunits", "patternUnits"),
    ("pointsatx", "pointsAtX"),
    ("pointsaty", "pointsAtY"),
    ("pointsatz", "pointsAtZ"),
    ("preservealpha", "preserveAlpha"),
    ("preserveaspectratio", "preserveAspectRatio"),
    ("primitiveunits", "primitiveUnits"),
    ("refx", "refX"),
    ("refy", "refY"),
    ("repeatcount", "repeatCount"),
    ("repeatdur", "repeatDur"),
    ("requiredextensions", "requiredExtensions"),
    ("requiredfeatures", "requiredFeatures"),
    ("specularconstant", "specularConstant"),
    ("specularexponent", "specularExponent"),
    ("spreadmethod", "spreadMethod"),
    ("startoffset", "startOffset"),
    ("stddeviation", "stdDeviation"),
    ("stitchtiles", "stitchTiles"),
    ("surfacescale", "surfaceScale"),
    ("systemlanguage", "systemLanguage"),
    ("tablevalues", "tableValues"),
    ("targetx", "targetX"),
    ("targety", "targetY"),
    ("textlength", "textLength"),
    ("viewbox", "viewBox"),
    ("viewtarget", "viewTarget"),
    ("xchannelselector", "xChannelSelector"),
    ("ychannelselector", "yChannelSelector"),
    ("zoomandpan", "zoomAndPan"),
];

/// Document mode for a DOCTYPE token (§13.2.6.4.1).
fn quirks_mode_for(d: &Doctype) -> QuirksMode {
    const QUIRKS_PREFIXES: [&str; 55] = [
        "+//silmaril//dtd html pro v0r11 19970101//",
        "-//as//dtd html 3.0 aswedit + extensions//",
        "-//advasoft ltd//dtd html 3.0 aswedit + extensions//",
        "-//ietf//dtd html 2.0 level 1//",
        "-//ietf//dtd html 2.0 level 2//",
        "-//ietf//dtd html 2.0 strict level 1//",
        "-//ietf//dtd html 2.0 strict level 2//",
        "-//ietf//dtd html 2.0 strict//",
        "-//ietf//dtd html 2.0//",
        "-//ietf//dtd html 2.1e//",
        "-//ietf//dtd html 3.0//",
        "-//ietf//dtd html 3.2 final//",
        "-//ietf//dtd html 3.2//",
        "-//ietf//dtd html 3//",
        "-//ietf//dtd html level 0//",
        "-//ietf//dtd html level 1//",
        "-//ietf//dtd html level 2//",
        "-//ietf//dtd html level 3//",
        "-//ietf//dtd html strict level 0//",
        "-//ietf//dtd html strict level 1//",
        "-//ietf//dtd html strict level 2//",
        "-//ietf//dtd html strict level 3//",
        "-//ietf//dtd html strict//",
        "-//ietf//dtd html//",
        "-//metrius//dtd metrius presentational//",
        "-//microsoft//dtd internet explorer 2.0 html strict//",
        "-//microsoft//dtd internet explorer 2.0 html//",
        "-//microsoft//dtd internet explorer 2.0 tables//",
        "-//microsoft//dtd internet explorer 3.0 html strict//",
        "-//microsoft//dtd internet explorer 3.0 html//",
        "-//microsoft//dtd internet explorer 3.0 tables//",
        "-//netscape comm. corp.//dtd html//",
        "-//netscape comm. corp.//dtd strict html//",
        "-//o'reilly and associates//dtd html 2.0//",
        "-//o'reilly and associates//dtd html extended 1.0//",
        "-//o'reilly and associates//dtd html extended relaxed 1.0//",
        "-//sq//dtd html 2.0 hotmetal + extensions//",
        "-//softquad software//dtd hotmetal pro 6.0::19990601::extensions to html 4.0//",
        "-//softquad//dtd hotmetal pro 4.0::19971010::extensions to html 4.0//",
        "-//spyglass//dtd html 2.0 extended//",
        "-//sun microsystems corp.//dtd hotjava html//",
        "-//sun microsystems corp.//dtd hotjava strict html//",
        "-//w3c//dtd html 3 1995-03-24//",
        "-//w3c//dtd html 3.2 draft//",
        "-//w3c//dtd html 3.2 final//",
        "-//w3c//dtd html 3.2//",
        "-//w3c//dtd html 3.2s draft//",
        "-//w3c//dtd html 4.0 frameset//",
        "-//w3c//dtd html 4.0 transitional//",
        "-//w3c//dtd html experimental 19960712//",
        "-//w3c//dtd html experimental 970421//",
        "-//w3c//dtd w3 html//",
        "-//w3o//dtd w3 html 3.0//",
        "-//webtechs//dtd mozilla html 2.0//",
        "-//webtechs//dtd mozilla html//",
    ];
    let public = d.public_id.as_deref().map(str::to_ascii_lowercase);
    let system = d.system_id.as_deref().map(str::to_ascii_lowercase);
    let public_starts = |p: &str| public.as_deref().is_some_and(|id| id.starts_with(p));
    let html401 = public_starts("-//w3c//dtd html 4.01 frameset//")
        || public_starts("-//w3c//dtd html 4.01 transitional//");
    if d.force_quirks
        || d.name.as_deref() != Some("html")
        || matches!(
            public.as_deref(),
            Some(
                "-//w3o//dtd w3 html strict 3.0//en//"
                    | "-/w3c/dtd html 4.0 transitional/en"
                    | "html"
            )
        )
        || system.as_deref() == Some("http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd")
        || QUIRKS_PREFIXES.iter().any(|p| public_starts(p))
        || (system.is_none() && html401)
    {
        return QuirksMode::Quirks;
    }
    if public_starts("-//w3c//dtd xhtml 1.0 frameset//")
        || public_starts("-//w3c//dtd xhtml 1.0 transitional//")
        || (system.is_some() && html401)
    {
        return QuirksMode::LimitedQuirks;
    }
    QuirksMode::NoQuirks
}
