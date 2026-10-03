//! DOM tree for Axiom.
//!
//! Arena-style storage with `NodeId` handles. Phase 2 adds mutable DOM APIs,
//! selector queries, and mutation invalidation hooks.

mod attributes;
mod names;
mod selector;
mod shadow;
mod tree;

pub use attributes::{Attr, Attributes};
pub use names::{
    is_valid_attribute_local_name, is_valid_doctype_name, is_valid_element_local_name,
    is_valid_namespace_prefix, validate_and_extract, DomError, NameContext, QualifiedName,
    HTML_NAMESPACE, MATHML_NAMESPACE, SVG_NAMESPACE, XLINK_NAMESPACE, XMLNS_NAMESPACE,
    XML_NAMESPACE,
};
pub use selector::{
    closest, matches, matches_in_shadow, matches_pseudo, parse_selector_list, query_selector,
    query_selector_all, AttrCase, AttrOp, Combinator, ComplexSelector, Compound, NsMatch, Pseudo,
    PseudoElement, SelectorList, Simple,
};
pub use shadow::{ShadowRoot, ShadowRootInit, ShadowRootMode, SlotAssignmentMode};

use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DirtyFlags {
    pub style: bool,
    pub layout: bool,
    pub paint: bool,
    pub composite: bool,
}

impl DirtyFlags {
    pub fn all() -> Self {
        Self {
            style: true,
            layout: true,
            paint: true,
            composite: true,
        }
    }

    pub fn paint_only() -> Self {
        Self {
            paint: true,
            composite: true,
            ..Self::default()
        }
    }

    pub fn merge(&mut self, other: DirtyFlags) {
        self.style |= other.style;
        self.layout |= other.layout;
        self.paint |= other.paint;
        self.composite |= other.composite;
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn any(&self) -> bool {
        self.style || self.layout || self.paint || self.composite
    }
}

/// Element namespace. Tag names of HTML elements are lowercase; SVG and MathML names keep
/// the case the HTML parser gives them (`foreignObject`, `definitionURL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Namespace {
    #[default]
    Html,
    Svg,
    MathMl,
    /// No namespace (`namespaceURI` is null).
    Null,
    /// Any other namespace URI, interned by its document (`Document::namespace_uri`).
    Other(u32),
}

/// Document mode (HTML §13.2.6.4.1), set by the parser from the doctype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum QuirksMode {
    #[default]
    NoQuirks,
    LimitedQuirks,
    Quirks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Document,
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },
    Element {
        tag: String,
        namespace: Namespace,
    },
    Text {
        data: String,
    },
    /// XML CDATA section. It is character data like [`NodeKind::Text`], but preserves
    /// its node type so XML/XHTML DOM consumers do not lose XML syntax information.
    CData {
        data: String,
    },
    Comment {
        data: String,
    },
    ProcessingInstruction {
        target: String,
        data: String,
    },
    DocumentFragment,
}

/// A stylesheet contributing to the cascade, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleSource {
    Inline { node: NodeId, text: String },
    External { node: NodeId, href: String },
}

/// A `<script>` element as found by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSource {
    pub node: NodeId,
    /// Raw `src` attribute (unresolved); `None` for inline scripts.
    pub src: Option<String>,
    pub text: String,
    pub is_async: bool,
    pub defer: bool,
    pub type_attr: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub attrs: Attributes,
}

#[derive(Debug, Default)]
pub struct Document {
    nodes: Vec<Node>,
    pub document_id: Option<NodeId>,
    pub dirty: DirtyFlags,
    pub hover: Option<NodeId>,
    pub focus: Option<NodeId>,
    pub active: Option<NodeId>,
    pub quirks_mode: QuirksMode,
    /// `<template>` element → its contents DocumentFragment.
    template_contents: HashMap<NodeId, NodeId>,
    /// Shadow host → its shadow root.
    shadow_roots: HashMap<NodeId, ShadowRoot>,
    /// Shadow root → its host.
    shadow_hosts: HashMap<NodeId, NodeId>,
    /// Slot → the nodes `assign()` gave it (manual slot assignment).
    manual_slots: HashMap<NodeId, Vec<NodeId>>,
    /// Autonomous custom elements that script has upgraded (`:defined`).
    defined_custom_elements: HashSet<NodeId>,
    /// URIs of `Namespace::Other` values.
    namespaces: Vec<String>,
    /// Namespace prefixes of the (rare) prefixed elements.
    prefixes: HashMap<NodeId, String>,
    /// Bumped by every `mark_dirty`, so script can cache tree queries per version.
    version: u64,
}

impl Document {
    pub fn new() -> Self {
        let mut doc = Self {
            nodes: Vec::new(),
            document_id: None,
            dirty: DirtyFlags::all(),
            hover: None,
            focus: None,
            active: None,
            quirks_mode: QuirksMode::NoQuirks,
            template_contents: HashMap::new(),
            shadow_roots: HashMap::new(),
            shadow_hosts: HashMap::new(),
            manual_slots: HashMap::new(),
            defined_custom_elements: HashSet::new(),
            namespaces: Vec::new(),
            prefixes: HashMap::new(),
            version: 0,
        };
        let id = doc.alloc(Node {
            kind: NodeKind::Document,
            parent: None,
            children: Vec::new(),
            attrs: Attributes::default(),
        });
        doc.document_id = Some(id);
        doc
    }

    pub fn mark_dirty(&mut self, flags: DirtyFlags) {
        self.dirty.merge(flags);
        self.version = self.version.wrapping_add(1);
    }

    /// Changes whenever the tree, an attribute or character data changes.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn alloc(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(node);
        id
    }

    pub fn get(&self, id: NodeId) -> &Node {
        &self.nodes[id.0]
    }

    pub fn get_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.0]
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        if self.get(child).parent.is_some() {
            let old = self.get(child).parent.unwrap();
            self.remove_child(old, child);
        }
        self.get_mut(child).parent = Some(parent);
        self.get_mut(parent).children.push(child);
        self.mark_dirty(DirtyFlags::all());
    }

    pub fn remove_child(&mut self, parent: NodeId, child: NodeId) -> bool {
        let children = &mut self.get_mut(parent).children;
        let Some(idx) = children.iter().position(|&c| c == child) else {
            return false;
        };
        children.remove(idx);
        self.get_mut(child).parent = None;
        self.mark_dirty(DirtyFlags::all());
        true
    }

    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
        if self.get(child).parent.is_some() {
            let old = self.get(child).parent.unwrap();
            self.remove_child(old, child);
        }
        self.get_mut(child).parent = Some(parent);
        match reference {
            Some(r) => {
                let children = &mut self.get_mut(parent).children;
                if let Some(idx) = children.iter().position(|&c| c == r) {
                    children.insert(idx, child);
                } else {
                    children.push(child);
                }
            }
            None => self.get_mut(parent).children.push(child),
        }
        self.mark_dirty(DirtyFlags::all());
    }

    pub fn replace_child(&mut self, parent: NodeId, new_child: NodeId, old_child: NodeId) -> bool {
        let idx = {
            let children = &self.get(parent).children;
            match children.iter().position(|&c| c == old_child) {
                Some(i) => i,
                None => return false,
            }
        };
        if let Some(prev) = self.get(new_child).parent {
            self.remove_child(prev, new_child);
        }
        self.get_mut(old_child).parent = None;
        self.get_mut(new_child).parent = Some(parent);
        self.get_mut(parent).children[idx] = new_child;
        self.mark_dirty(DirtyFlags::all());
        true
    }

    pub fn clone_node(&mut self, id: NodeId, deep: bool) -> NodeId {
        let kind = self.get(id).kind.clone();
        let attrs = self.get(id).attrs.clone();
        let children = self.get(id).children.clone();
        let new_id = self.alloc(Node {
            kind,
            parent: None,
            children: Vec::new(),
            attrs,
        });
        if let Some(prefix) = self.prefixes.get(&id).cloned() {
            self.prefixes.insert(new_id, prefix);
        }
        if deep {
            for child in children {
                let cloned = self.clone_node(child, true);
                self.append_child(new_id, cloned);
            }
            if let Some(contents) = self.template_contents(id) {
                let copy = self.template_contents_or_create(new_id);
                for child in self.get(contents).children.clone() {
                    let cloned = self.clone_node(child, true);
                    self.append_child(copy, cloned);
                }
            }
        }
        self.clone_shadow_root(id, new_id);
        new_id
    }

    /// A deep copy of node `id` of `src` (another document) owned by this document,
    /// including prefixes, namespaces and template contents.
    pub fn import_node(&mut self, src: &Document, id: NodeId) -> NodeId {
        let mut kind = src.get(id).kind.clone();
        if let NodeKind::Element { namespace, .. } = &mut kind {
            *namespace = self.namespace_for_uri(src.namespace_uri(*namespace));
        }
        let new_id = self.alloc(Node {
            kind,
            parent: None,
            children: Vec::new(),
            attrs: src.get(id).attrs.clone(),
        });
        if let Some(prefix) = src.element_prefix(id) {
            self.prefixes.insert(new_id, prefix.to_string());
        }
        for &child in &src.get(id).children {
            let copy = self.import_node(src, child);
            self.append_child(new_id, copy);
        }
        if let Some(contents) = src.template_contents(id) {
            let copy = self.template_contents_or_create(new_id);
            for &child in &src.get(contents).children {
                let imported = self.import_node(src, child);
                self.append_child(copy, imported);
            }
        }
        new_id
    }

    /// The namespace value for `uri` (`None` or empty: no namespace), interning URIs
    /// other than HTML, SVG and MathML.
    pub fn namespace_for_uri(&mut self, uri: Option<&str>) -> Namespace {
        match uri.filter(|u| !u.is_empty()) {
            None => Namespace::Null,
            Some(HTML_NAMESPACE) => Namespace::Html,
            Some(SVG_NAMESPACE) => Namespace::Svg,
            Some(MATHML_NAMESPACE) => Namespace::MathMl,
            Some(other) => {
                let i = match self.namespaces.iter().position(|n| n == other) {
                    Some(i) => i,
                    None => {
                        self.namespaces.push(other.to_string());
                        self.namespaces.len() - 1
                    }
                };
                Namespace::Other(i as u32)
            }
        }
    }

    pub fn namespace_uri(&self, namespace: Namespace) -> Option<&str> {
        match namespace {
            Namespace::Html => Some(HTML_NAMESPACE),
            Namespace::Svg => Some(SVG_NAMESPACE),
            Namespace::MathMl => Some(MATHML_NAMESPACE),
            Namespace::Null => None,
            Namespace::Other(i) => self.namespaces.get(i as usize).map(String::as_str),
        }
    }

    /// An element from a validated namespace, prefix and local name.
    pub fn create_element_qualified(&mut self, name: &QualifiedName) -> NodeId {
        let namespace = self.namespace_for_uri(name.namespace.as_deref());
        let id = self.create_element_ns(&name.local_name, namespace);
        if let Some(prefix) = &name.prefix {
            self.prefixes.insert(id, prefix.clone());
        }
        id
    }

    pub fn element_prefix(&self, id: NodeId) -> Option<&str> {
        self.prefixes.get(&id).map(String::as_str)
    }

    /// `prefix:localName` of an element, or its local name.
    pub fn qualified_name(&self, id: NodeId) -> Option<String> {
        let local = self.tag_name(id)?;
        Some(match self.element_prefix(id) {
            Some(p) => format!("{p}:{local}"),
            None => local.to_string(),
        })
    }

    pub fn attributes(&self, id: NodeId) -> &Attributes {
        &self.get(id).attrs
    }

    pub fn attr_ns(&self, id: NodeId, namespace: Option<&str>, local_name: &str) -> Option<&str> {
        self.get(id).attrs.get_ns(namespace, local_name)
    }

    pub fn set_attr_ns(
        &mut self,
        id: NodeId,
        namespace: Option<&str>,
        prefix: Option<&str>,
        local_name: &str,
        value: &str,
    ) {
        self.get_mut(id)
            .attrs
            .set_ns(namespace, prefix, local_name, value);
        self.mark_dirty(DirtyFlags::all());
    }

    /// Replaces the attribute `(namespace, local_name)` in place (prefix included) or
    /// appends it.
    pub fn replace_attr_ns(
        &mut self,
        id: NodeId,
        namespace: Option<&str>,
        prefix: Option<&str>,
        local_name: &str,
        value: &str,
    ) {
        self.get_mut(id)
            .attrs
            .replace_ns(namespace, prefix, local_name, value);
        self.mark_dirty(DirtyFlags::all());
    }

    pub fn remove_attr_ns(
        &mut self,
        id: NodeId,
        namespace: Option<&str>,
        local_name: &str,
    ) -> bool {
        let removed = self
            .get_mut(id)
            .attrs
            .remove_ns(namespace, local_name)
            .is_some();
        if removed {
            self.mark_dirty(DirtyFlags::all());
        }
        removed
    }

    /// Removes the first attribute whose qualified name is exactly `name`.
    pub fn remove_attr_exact(&mut self, id: NodeId, name: &str) -> bool {
        let removed = self.get_mut(id).attrs.remove(name).is_some();
        if removed {
            self.mark_dirty(DirtyFlags::all());
        }
        removed
    }

    pub fn contains(&self, root: NodeId, other: NodeId) -> bool {
        let mut cur = Some(other);
        while let Some(id) = cur {
            if id == root {
                return true;
            }
            cur = self.get(id).parent;
        }
        false
    }

    pub fn create_element(&mut self, tag: &str) -> NodeId {
        self.create_element_ns(&tag.to_ascii_lowercase(), Namespace::Html)
    }

    /// An element with `tag` exactly as given.
    pub fn create_element_ns(&mut self, tag: &str, namespace: Namespace) -> NodeId {
        self.alloc(Node {
            kind: NodeKind::Element {
                tag: tag.to_string(),
                namespace,
            },
            parent: None,
            children: Vec::new(),
            attrs: Attributes::default(),
        })
    }

    pub fn namespace(&self, id: NodeId) -> Option<Namespace> {
        match &self.get(id).kind {
            NodeKind::Element { namespace, .. } => Some(*namespace),
            _ => None,
        }
    }

    /// The contents fragment of a `<template>` element, created on first use.
    pub fn template_contents_or_create(&mut self, template: NodeId) -> NodeId {
        if let Some(&fragment) = self.template_contents.get(&template) {
            return fragment;
        }
        let fragment = self.create_document_fragment();
        self.template_contents.insert(template, fragment);
        fragment
    }

    pub fn template_contents(&self, template: NodeId) -> Option<NodeId> {
        self.template_contents.get(&template).copied()
    }

    /// Records that script upgraded custom element `id` (its custom element state is
    /// "custom"), so `:defined` matches it.
    pub fn set_custom_element_defined(&mut self, id: NodeId) {
        if self.defined_custom_elements.insert(id) {
            self.mark_dirty(DirtyFlags::all());
        }
    }

    pub fn is_custom_element_defined(&self, id: NodeId) -> bool {
        self.defined_custom_elements.contains(&id)
    }

    pub fn create_text(&mut self, data: &str) -> NodeId {
        self.alloc(Node {
            kind: NodeKind::Text {
                data: data.to_string(),
            },
            parent: None,
            children: Vec::new(),
            attrs: Attributes::default(),
        })
    }

    pub fn create_cdata(&mut self, data: &str) -> NodeId {
        self.alloc(Node {
            kind: NodeKind::CData {
                data: data.to_string(),
            },
            parent: None,
            children: Vec::new(),
            attrs: Attributes::default(),
        })
    }

    pub fn create_comment(&mut self, data: &str) -> NodeId {
        self.alloc(Node {
            kind: NodeKind::Comment {
                data: data.to_string(),
            },
            parent: None,
            children: Vec::new(),
            attrs: Attributes::default(),
        })
    }

    pub fn create_processing_instruction(&mut self, target: &str, data: &str) -> NodeId {
        self.alloc(Node {
            kind: NodeKind::ProcessingInstruction {
                target: target.to_string(),
                data: data.to_string(),
            },
            parent: None,
            children: Vec::new(),
            attrs: Attributes::default(),
        })
    }

    pub fn create_doctype(&mut self, name: &str) -> NodeId {
        self.create_doctype_with_ids(name, "", "")
    }

    pub fn create_doctype_with_ids(
        &mut self,
        name: &str,
        public_id: &str,
        system_id: &str,
    ) -> NodeId {
        self.alloc(Node {
            kind: NodeKind::Doctype {
                name: name.to_string(),
                public_id: public_id.to_string(),
                system_id: system_id.to_string(),
            },
            parent: None,
            children: Vec::new(),
            attrs: Attributes::default(),
        })
    }

    /// A new, empty Document node other than [`Document::document_id`]: the root of an
    /// inert document script creates (`createHTMLDocument`, `DOMParser`, …). Nothing under
    /// it is connected, so it is never styled, laid out or scripted.
    pub fn create_document_node(&mut self) -> NodeId {
        self.alloc(Node {
            kind: NodeKind::Document,
            parent: None,
            children: Vec::new(),
            attrs: Attributes::default(),
        })
    }

    pub fn create_document_fragment(&mut self) -> NodeId {
        self.alloc(Node {
            kind: NodeKind::DocumentFragment,
            parent: None,
            children: Vec::new(),
            attrs: Attributes::default(),
        })
    }

    pub fn set_attr(&mut self, id: NodeId, name: &str, value: &str) {
        self.set_attr_exact(id, &name.to_ascii_lowercase(), value);
    }

    /// Sets the first attribute whose qualified name is `name`, without case folding
    /// (the parser's adjusted SVG / MathML names such as `viewBox`), or appends one.
    pub fn set_attr_exact(&mut self, id: NodeId, name: &str, value: &str) {
        self.get_mut(id).attrs.set(name, value);
        self.mark_dirty(DirtyFlags::all());
    }

    pub fn remove_attr(&mut self, id: NodeId, name: &str) {
        let attrs = &mut self.get_mut(id).attrs;
        if attrs.remove(name).is_none() {
            attrs.remove(&name.to_ascii_lowercase());
        }
        self.mark_dirty(DirtyFlags::all());
    }

    pub fn has_attr(&self, id: NodeId, name: &str) -> bool {
        self.attr(id, name).is_some()
    }

    /// Exact-name match first, then the ASCII-lowercased name.
    pub fn attr(&self, id: NodeId, name: &str) -> Option<&str> {
        let attrs = &self.get(id).attrs;
        attrs
            .get(name)
            .or_else(|| attrs.get(&name.to_ascii_lowercase()))
    }

    pub fn class_name(&self, id: NodeId) -> String {
        self.attr(id, "class").unwrap_or("").to_string()
    }

    pub fn set_class_name(&mut self, id: NodeId, value: &str) {
        self.set_attr(id, "class", value);
    }

    pub fn tag_name(&self, id: NodeId) -> Option<&str> {
        match &self.get(id).kind {
            NodeKind::Element { tag, .. } => Some(tag.as_str()),
            _ => None,
        }
    }

    pub fn is_element(&self, id: NodeId) -> bool {
        matches!(self.get(id).kind, NodeKind::Element { .. })
    }

    pub fn set_text_content(&mut self, id: NodeId, text: &str) {
        let children = self.get(id).children.clone();
        for child in children {
            self.remove_child(id, child);
        }
        let is_character_data = matches!(
            self.get(id).kind,
            NodeKind::Text { .. }
                | NodeKind::CData { .. }
                | NodeKind::Comment { .. }
                | NodeKind::ProcessingInstruction { .. }
        );
        if is_character_data {
            if let NodeKind::Text { data }
            | NodeKind::CData { data }
            | NodeKind::Comment { data }
            | NodeKind::ProcessingInstruction { data, .. } = &mut self.get_mut(id).kind
            {
                *data = text.to_string();
            }
            self.mark_dirty(DirtyFlags {
                layout: true,
                paint: true,
                composite: true,
                style: false,
            });
        } else if !text.is_empty()
            && matches!(
                self.get(id).kind,
                NodeKind::Element { .. } | NodeKind::Document | NodeKind::DocumentFragment
            )
        {
            let t = self.create_text(text);
            self.append_child(id, t);
        }
    }

    pub fn text_content(&self, id: NodeId) -> String {
        match &self.get(id).kind {
            NodeKind::Text { data } | NodeKind::CData { data } => data.clone(),
            _ => {
                let mut out = String::new();
                for &child in &self.get(id).children {
                    out.push_str(&self.text_content(child));
                }
                out
            }
        }
    }

    pub fn document_element(&self) -> Option<NodeId> {
        let doc = self.document_id?;
        self.get(doc)
            .children
            .iter()
            .copied()
            .find(|&id| self.tag_name(id) == Some("html"))
    }

    pub fn body(&self) -> Option<NodeId> {
        let html = self.document_element()?;
        self.find_descendant(html, "body")
    }

    pub fn head(&self) -> Option<NodeId> {
        let html = self.document_element()?;
        self.find_descendant(html, "head")
    }

    pub fn find_descendant(&self, root: NodeId, tag: &str) -> Option<NodeId> {
        if self.tag_name(root) == Some(tag) {
            return Some(root);
        }
        for &child in &self.get(root).children {
            if let Some(found) = self.find_descendant(child, tag) {
                return Some(found);
            }
        }
        None
    }

    pub fn get_element_by_id(&self, id_value: &str) -> Option<NodeId> {
        self.iter_elements()
            .find(|&id| self.attr(id, "id") == Some(id_value))
    }

    pub fn get_elements_by_tag_name(&self, tag: &str) -> Vec<NodeId> {
        let tag = tag.to_ascii_lowercase();
        if tag == "*" {
            return self.iter_elements().collect();
        }
        self.iter_elements()
            .filter(|&id| self.tag_name(id) == Some(tag.as_str()))
            .collect()
    }

    pub fn get_elements_by_class_name(&self, class: &str) -> Vec<NodeId> {
        self.iter_elements()
            .filter(|&id| {
                self.attr(id, "class")
                    .map(|c| c.split_whitespace().any(|x| x == class))
                    .unwrap_or(false)
            })
            .collect()
    }

    pub fn query_selector(&self, selector: &str) -> Option<NodeId> {
        let root = self.document_id?;
        query_selector(self, root, selector)
    }

    pub fn query_selector_all(&self, selector: &str) -> Vec<NodeId> {
        let Some(root) = self.document_id else {
            return Vec::new();
        };
        query_selector_all(self, root, selector)
    }

    pub fn parent_chain(&self, node: NodeId) -> Vec<NodeId> {
        let mut chain = Vec::new();
        let mut cur = Some(node);
        while let Some(id) = cur {
            chain.push(id);
            cur = self.get(id).parent;
        }
        chain.reverse();
        chain
    }

    pub fn collect_style_text(&self) -> String {
        let mut css = String::new();
        if let Some(doc) = self.document_id {
            self.walk_styles(doc, &mut css);
        }
        css
    }

    pub fn collect_script_texts(&self) -> Vec<String> {
        let mut scripts = Vec::new();
        if let Some(doc) = self.document_id {
            self.walk_scripts(doc, &mut scripts);
        }
        scripts
    }

    pub fn collect_image_srcs(&self) -> Vec<(NodeId, String)> {
        let mut out = Vec::new();
        for id in self.iter_elements() {
            if self.tag_name(id) == Some("img") {
                if let Some(src) = self.attr(id, "src") {
                    out.push((id, src.to_string()));
                }
            }
        }
        out
    }

    /// `<style>` and `<link rel=stylesheet>` in tree order (cascade order).
    pub fn collect_style_sources(&self) -> Vec<StyleSource> {
        let mut out = Vec::new();
        self.walk_preorder(|id| out.extend(self.style_source(id)));
        out
    }

    /// The stylesheet `id` contributes, if it is a `<style>` or `<link rel=stylesheet>`.
    fn style_source(&self, id: NodeId) -> Option<StyleSource> {
        match self.tag_name(id)? {
            "style" => Some(StyleSource::Inline {
                node: id,
                text: self.text_content(id),
            }),
            "link" => {
                let rel = self.attr(id, "rel").unwrap_or("").to_ascii_lowercase();
                let mut tokens = rel.split_ascii_whitespace();
                let is_sheet = tokens.clone().any(|t| t == "stylesheet");
                let alternate = tokens.any(|t| t == "alternate");
                let href = self.attr(id, "href").map(str::trim)?;
                (is_sheet && !alternate && !href.is_empty()).then(|| StyleSource::External {
                    node: id,
                    href: href.to_string(),
                })
            }
            _ => None,
        }
    }

    /// Every `<script>` in tree order, inline and external.
    pub fn collect_script_sources(&self) -> Vec<ScriptSource> {
        let mut out = Vec::new();
        self.walk_preorder(|id| {
            if self.tag_name(id) == Some("script") {
                out.push(ScriptSource {
                    node: id,
                    src: self
                        .attr(id, "src")
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()),
                    text: self.text_content(id),
                    is_async: self.has_attr(id, "async"),
                    defer: self.has_attr(id, "defer"),
                    type_attr: self.attr(id, "type").map(str::to_string),
                });
            }
        });
        out
    }

    fn walk_preorder(&self, mut f: impl FnMut(NodeId)) {
        let Some(doc) = self.document_id else {
            return;
        };
        let mut stack = vec![doc];
        while let Some(id) = stack.pop() {
            f(id);
            stack.extend(self.get(id).children.iter().rev().copied());
        }
    }

    fn walk_styles(&self, id: NodeId, out: &mut String) {
        if self.tag_name(id) == Some("style") {
            out.push_str(&self.text_content(id));
            out.push('\n');
        }
        for &child in &self.get(id).children {
            self.walk_styles(child, out);
        }
    }

    fn walk_scripts(&self, id: NodeId, out: &mut Vec<String>) {
        if self.tag_name(id) == Some("script") && self.attr(id, "src").is_none() {
            let text = self.text_content(id);
            if !text.trim().is_empty() {
                out.push(text);
            }
        }
        for &child in &self.get(id).children {
            self.walk_scripts(child, out);
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Descendants of `root` (excluding `root`) in tree order.
    pub fn descendants(&self, root: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.get(root).children.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            out.push(id);
            stack.extend(self.get(id).children.iter().rev().copied());
        }
        out
    }

    /// Every node in the arena that is an element, attached or not, in allocation order.
    pub fn iter_elements(&self) -> impl Iterator<Item = NodeId> + '_ {
        (0..self.nodes.len())
            .map(NodeId)
            .filter(|&id| self.is_element(id))
    }
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(doc) = self.document_id {
            self.fmt_node(f, doc, 0)?;
        }
        Ok(())
    }
}

impl Document {
    fn fmt_node(&self, f: &mut fmt::Formatter<'_>, id: NodeId, depth: usize) -> fmt::Result {
        let indent = "  ".repeat(depth);
        match &self.get(id).kind {
            NodeKind::Document => writeln!(f, "{indent}#document")?,
            NodeKind::DocumentFragment => writeln!(f, "{indent}#document-fragment")?,
            NodeKind::Doctype { name, .. } => writeln!(f, "{indent}<!DOCTYPE {name}>")?,
            NodeKind::Element { .. } => {
                write!(
                    f,
                    "{indent}<{}",
                    self.qualified_name(id).unwrap_or_default()
                )?;
                let mut attrs: Vec<_> = self
                    .get(id)
                    .attrs
                    .iter()
                    .map(|a| (a.qualified_name(), &a.value))
                    .collect();
                attrs.sort();
                for (k, v) in attrs {
                    write!(f, " {k}=\"{v}\"")?;
                }
                writeln!(f, ">")?;
            }
            NodeKind::Text { data } => {
                let t = data.trim();
                if !t.is_empty() {
                    writeln!(f, "{indent}\"{t}\"")?;
                }
            }
            NodeKind::CData { data } => writeln!(f, "{indent}<![CDATA[{data}]]>")?,
            NodeKind::Comment { data } => writeln!(f, "{indent}<!-- {data} -->")?,
            NodeKind::ProcessingInstruction { target, data } => {
                writeln!(f, "{indent}<?{target} {data}?>")?
            }
        }
        for &child in &self.get(id).children {
            self.fmt_node(f, child, depth + 1)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_and_query() {
        let mut doc = Document::new();
        let html = doc.create_element("html");
        let body = doc.create_element("body");
        let div = doc.create_element("div");
        doc.set_attr(div, "id", "main");
        doc.set_attr(div, "class", "box");
        let root = doc.document_id.unwrap();
        doc.append_child(root, html);
        doc.append_child(html, body);
        doc.append_child(body, div);
        assert_eq!(doc.get_element_by_id("main"), Some(div));
        assert_eq!(doc.query_selector("#main.box"), Some(div));
        doc.set_text_content(div, "hi");
        assert!(doc.text_content(div).contains("hi"));
    }
}
