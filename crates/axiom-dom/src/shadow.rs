//! Shadow trees (DOM §4.2.2, §4.8 "attach a shadow root"): shadow roots attached to
//! hosts, slot assignment (named and manual) and the flat tree that style and layout
//! walk.
//!
//! A shadow root is a parentless `DocumentFragment` node recorded against its host, the
//! same way `<template>` contents are. Ordinary tree walks (selectors, `textContent`,
//! serialization) therefore never enter a shadow tree; the flat tree and the
//! shadow-including helpers here do.

use std::borrow::Cow;

use crate::{Document, Namespace, NodeId, NodeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowRootMode {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SlotAssignmentMode {
    #[default]
    Named,
    Manual,
}

/// `ShadowRootInit`, plus whether the parser attached the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowRootInit {
    pub mode: ShadowRootMode,
    pub delegates_focus: bool,
    pub slot_assignment: SlotAssignmentMode,
    pub clonable: bool,
    pub serializable: bool,
    /// Attached by the parser for `<template shadowrootmode>`.
    pub declarative: bool,
}

impl ShadowRootInit {
    pub fn new(mode: ShadowRootMode) -> Self {
        Self {
            mode,
            delegates_focus: false,
            slot_assignment: SlotAssignmentMode::Named,
            clonable: false,
            serializable: false,
            declarative: false,
        }
    }
}

/// A shadow root and the options it was attached with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowRoot {
    pub root: NodeId,
    pub init: ShadowRootInit,
}

/// HTML elements other than autonomous custom elements that may host a shadow root.
const VALID_HOSTS: &[&str] = &[
    "article",
    "aside",
    "blockquote",
    "body",
    "div",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "main",
    "nav",
    "p",
    "section",
    "span",
];

const RESERVED_CUSTOM_NAMES: &[&str] = &[
    "annotation-xml",
    "color-profile",
    "font-face",
    "font-face-src",
    "font-face-uri",
    "font-face-format",
    "font-face-name",
    "missing-glyph",
];

/// A valid custom element name (HTML §4.13.2), without the full PCENChar table: a
/// lowercase ASCII letter, then no ASCII uppercase, containing a hyphen.
fn is_custom_element_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.contains('-')
        && !name.chars().any(|c| c.is_ascii_uppercase())
        && !RESERVED_CUSTOM_NAMES.contains(&name)
}

impl Document {
    /// "Attach a shadow root" to `host`, returning the root; `None` where the DOM throws a
    /// `NotSupportedError` (a host that cannot have one, or one that already has a
    /// non-declarative root). Re-attaching over a declarative root of the same mode
    /// empties and returns it.
    pub fn attach_shadow(&mut self, host: NodeId, init: ShadowRootInit) -> Option<NodeId> {
        let NodeKind::Element { tag, namespace } = &self.get(host).kind else {
            return None;
        };
        if *namespace != Namespace::Html
            || !(VALID_HOSTS.contains(&tag.as_str()) || is_custom_element_name(tag))
        {
            return None;
        }
        if let Some(existing) = self.shadow_roots.get_mut(&host) {
            if !existing.init.declarative || existing.init.mode != init.mode {
                return None;
            }
            existing.init.declarative = false;
            let root = existing.root;
            for child in self.get(root).children.clone() {
                self.remove_child(root, child);
            }
            return Some(root);
        }
        let root = self.create_document_fragment();
        self.shadow_roots.insert(host, ShadowRoot { root, init });
        self.shadow_hosts.insert(root, host);
        self.mark_dirty(crate::DirtyFlags::all());
        Some(root)
    }

    /// The shadow root attached to `host`, open or closed.
    pub fn shadow_root(&self, host: NodeId) -> Option<&ShadowRoot> {
        self.shadow_roots.get(&host)
    }

    /// The host of shadow root `root`.
    pub fn shadow_host(&self, root: NodeId) -> Option<NodeId> {
        self.shadow_hosts.get(&root).copied()
    }

    pub fn is_shadow_root(&self, id: NodeId) -> bool {
        self.shadow_hosts.contains_key(&id)
    }

    pub fn has_shadow_trees(&self) -> bool {
        !self.shadow_roots.is_empty()
    }

    /// Every (host, shadow root) pair whose host is connected, hosts in tree order.
    pub fn connected_shadow_roots(&self) -> Vec<(NodeId, NodeId)> {
        let mut out = Vec::new();
        if self.shadow_roots.is_empty() {
            return out;
        }
        let Some(doc) = self.document_id else {
            return out;
        };
        let mut stack = vec![doc];
        while let Some(id) = stack.pop() {
            let mut children: Vec<NodeId> = self.get(id).children.clone();
            if let Some(shadow) = self.shadow_roots.get(&id) {
                out.push((id, shadow.root));
                children.insert(0, shadow.root);
            }
            stack.extend(children.into_iter().rev());
        }
        out
    }

    /// The root of `id`'s tree (following parents only).
    pub fn tree_root(&self, id: NodeId) -> NodeId {
        let mut cur = id;
        while let Some(p) = self.get(cur).parent {
            cur = p;
        }
        cur
    }

    /// The shadow-including root: the tree root, continuing through shadow hosts.
    pub fn shadow_including_root(&self, id: NodeId) -> NodeId {
        let mut cur = self.tree_root(id);
        while let Some(host) = self.shadow_host(cur) {
            cur = self.tree_root(host);
        }
        cur
    }

    /// Whether `id` is connected: its shadow-including root is the document.
    pub fn is_connected(&self, id: NodeId) -> bool {
        Some(self.shadow_including_root(id)) == self.document_id
    }

    /// The parent in the event path: the parent node, or a shadow root's host.
    pub fn parent_or_host(&self, id: NodeId) -> Option<NodeId> {
        self.get(id).parent.or_else(|| self.shadow_host(id))
    }

    /// Whether `ancestor` is a shadow-including inclusive ancestor of `node`.
    pub fn shadow_including_contains(&self, ancestor: NodeId, node: NodeId) -> bool {
        let mut cur = Some(node);
        while let Some(id) = cur {
            if id == ancestor {
                return true;
            }
            cur = self.parent_or_host(id);
        }
        false
    }

    /// Children in the flat tree: a host's shadow root children, a slot's assigned nodes
    /// (its own children when nothing is assigned), otherwise the DOM children.
    pub fn flat_children(&self, id: NodeId) -> Cow<'_, [NodeId]> {
        let children = &self.get(id).children;
        if self.shadow_roots.is_empty() {
            return Cow::Borrowed(children);
        }
        if let Some(shadow) = self.shadow_roots.get(&id) {
            return Cow::Borrowed(&self.get(shadow.root).children);
        }
        if self.is_slot(id) {
            let assigned = self.assigned_nodes(id);
            if !assigned.is_empty() {
                return Cow::Owned(assigned);
            }
        }
        Cow::Borrowed(children)
    }

    /// The parent in the flat tree: a slotted node's slot, a shadow root child's host,
    /// otherwise the parent. `None` for a shadow host's child that no slot takes.
    pub fn flat_tree_parent(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.get(id).parent?;
        if self.shadow_roots.contains_key(&parent) {
            return self.assigned_slot(id);
        }
        Some(self.shadow_host(parent).unwrap_or(parent))
    }

    fn is_slot(&self, id: NodeId) -> bool {
        matches!(&self.get(id).kind, NodeKind::Element { tag, namespace: Namespace::Html } if tag == "slot")
    }

    fn slot_name(&self, slot: NodeId) -> &str {
        self.attr_ns(slot, None, "name").unwrap_or("")
    }

    fn slottable_name(&self, node: NodeId) -> Option<&str> {
        match &self.get(node).kind {
            NodeKind::Element { .. } => Some(self.attr_ns(node, None, "slot").unwrap_or("")),
            NodeKind::Text { .. } | NodeKind::CData { .. } => Some(""),
            _ => None,
        }
    }

    /// The first slot in shadow tree `root` (tree order) named `name`.
    fn find_slot(&self, root: NodeId, name: &str) -> Option<NodeId> {
        let mut stack: Vec<NodeId> = self.get(root).children.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            if self.is_slot(id) && self.slot_name(id) == name {
                return Some(id);
            }
            stack.extend(self.get(id).children.iter().rev().copied());
        }
        None
    }

    /// The shadow root a slot belongs to, with its options.
    fn slot_shadow(&self, slot: NodeId) -> Option<(NodeId, &ShadowRoot)> {
        let root = self.tree_root(slot);
        let host = self.shadow_host(root)?;
        Some((host, self.shadow_roots.get(&host)?))
    }

    /// "Find slottables": the host children assigned to `slot`, in tree order. Empty for
    /// a slot outside a shadow tree.
    pub fn assigned_nodes(&self, slot: NodeId) -> Vec<NodeId> {
        if !self.is_slot(slot) {
            return Vec::new();
        }
        let Some((host, shadow)) = self.slot_shadow(slot) else {
            return Vec::new();
        };
        let host_children = &self.get(host).children;
        match shadow.init.slot_assignment {
            SlotAssignmentMode::Manual => {
                let Some(manual) = self.manual_slots.get(&slot) else {
                    return Vec::new();
                };
                manual
                    .iter()
                    .copied()
                    .filter(|n| host_children.contains(n) && self.slottable_name(*n).is_some())
                    .filter(|&n| self.assigned_slot(n) == Some(slot))
                    .collect()
            }
            SlotAssignmentMode::Named => {
                let name = self.slot_name(slot);
                if self.find_slot(shadow.root, name) != Some(slot) {
                    return Vec::new();
                }
                host_children
                    .iter()
                    .copied()
                    .filter(|&c| self.slottable_name(c) == Some(name))
                    .collect()
            }
        }
    }

    /// "Find flattened slottables": assigned nodes with nested slots replaced by theirs.
    pub fn flattened_assigned_nodes(&self, slot: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let assigned = self.assigned_nodes(slot);
        let list = if assigned.is_empty() {
            self.get(slot).children.clone()
        } else {
            assigned
        };
        for node in list {
            if self.is_slot(node) && self.slot_shadow(node).is_some() {
                out.extend(self.flattened_assigned_nodes(node));
            } else if self.slottable_name(node).is_some() {
                out.push(node);
            }
        }
        out
    }

    /// The slot `node` is assigned to (`assignedSlot`, including closed shadow roots).
    pub fn assigned_slot(&self, node: NodeId) -> Option<NodeId> {
        let name = self.slottable_name(node)?;
        let parent = self.get(node).parent?;
        let shadow = self.shadow_roots.get(&parent)?;
        match shadow.init.slot_assignment {
            SlotAssignmentMode::Named => self.find_slot(shadow.root, name),
            SlotAssignmentMode::Manual => {
                let mut stack: Vec<NodeId> = self
                    .get(shadow.root)
                    .children
                    .iter()
                    .rev()
                    .copied()
                    .collect();
                while let Some(id) = stack.pop() {
                    if self.is_slot(id)
                        && self
                            .manual_slots
                            .get(&id)
                            .is_some_and(|nodes| nodes.contains(&node))
                    {
                        return Some(id);
                    }
                    stack.extend(self.get(id).children.iter().rev().copied());
                }
                None
            }
        }
    }

    /// `HTMLSlotElement.assign()`: the manually assigned nodes of `slot`. A node moves
    /// out of any other slot it was manually assigned to.
    pub fn assign_slot(&mut self, slot: NodeId, nodes: Vec<NodeId>) {
        for list in self.manual_slots.values_mut() {
            list.retain(|n| !nodes.contains(n));
        }
        let mut unique = Vec::new();
        for n in nodes {
            if !unique.contains(&n) {
                unique.push(n);
            }
        }
        self.manual_slots.insert(slot, unique);
        self.mark_dirty(crate::DirtyFlags::all());
    }

    /// `<style>` / `<link rel=stylesheet>` sources inside shadow tree `root`, in tree
    /// order (not descending into nested shadow trees).
    pub fn collect_style_sources_in(&self, root: NodeId) -> Vec<crate::StyleSource> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.get(root).children.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            if let Some(source) = self.style_source(id) {
                out.push(source);
            }
            stack.extend(self.get(id).children.iter().rev().copied());
        }
        out
    }

    /// Copies a clonable shadow root of `from` onto `to` (DOM "clone a node" step 7).
    pub(crate) fn clone_shadow_root(&mut self, from: NodeId, to: NodeId) {
        let Some(shadow) = self.shadow_roots.get(&from).copied() else {
            return;
        };
        if !shadow.init.clonable {
            return;
        }
        let Some(root) = self.attach_shadow(to, shadow.init) else {
            return;
        };
        for child in self.get(shadow.root).children.clone() {
            let copy = self.clone_node(child, true);
            self.append_child(root, copy);
        }
    }
}
