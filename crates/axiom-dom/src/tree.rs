//! Checked tree mutation (DOM §4.2.3): pre-insert, replace and pre-remove, with the
//! "ensure pre-insert validity" constraints. The unchecked `append_child`,
//! `insert_before` and `remove_child` stay available to the parser.

use crate::{Document, DomError, NodeId, NodeKind};

impl Document {
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.get(id).parent?;
        let siblings = &self.get(parent).children;
        let i = siblings.iter().position(|&c| c == id)?;
        siblings.get(i + 1).copied()
    }

    pub fn previous_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.get(id).parent?;
        let siblings = &self.get(parent).children;
        let i = siblings.iter().position(|&c| c == id)?;
        i.checked_sub(1).map(|j| siblings[j])
    }

    /// Text, comment and processing-instruction nodes.
    pub fn is_character_data(&self, id: NodeId) -> bool {
        matches!(
            self.get(id).kind,
            NodeKind::Text { .. }
                | NodeKind::CData { .. }
                | NodeKind::Comment { .. }
                | NodeKind::ProcessingInstruction { .. }
        )
    }

    /// The `<template>` whose contents are `fragment`.
    pub fn template_host(&self, fragment: NodeId) -> Option<NodeId> {
        self.template_contents
            .iter()
            .find(|(_, &contents)| contents == fragment)
            .map(|(&template, _)| template)
    }

    /// Whether `ancestor` is `node` or one of its ancestors, crossing from template
    /// contents to their template.
    pub fn is_host_including_inclusive_ancestor(&self, ancestor: NodeId, node: NodeId) -> bool {
        let mut cur = Some(node);
        while let Some(n) = cur {
            if n == ancestor {
                return true;
            }
            cur = self
                .get(n)
                .parent
                .or_else(|| self.template_host(n))
                .or_else(|| self.shadow_host(n));
        }
        false
    }

    /// "Ensure pre-insert validity" of inserting `node` into `parent` before `child`,
    /// ignoring `exclude` (the child being replaced).
    pub fn ensure_pre_insert_validity(
        &self,
        node: NodeId,
        parent: NodeId,
        child: Option<NodeId>,
        exclude: Option<NodeId>,
    ) -> Result<(), DomError> {
        let parent_kind = &self.get(parent).kind;
        if !matches!(
            parent_kind,
            NodeKind::Document | NodeKind::DocumentFragment | NodeKind::Element { .. }
        ) {
            return Err(DomError::HierarchyRequest);
        }
        if self.is_host_including_inclusive_ancestor(node, parent) {
            return Err(DomError::HierarchyRequest);
        }
        if child.is_some_and(|c| self.get(c).parent != Some(parent)) {
            return Err(DomError::NotFound);
        }
        let node_kind = &self.get(node).kind;
        if matches!(node_kind, NodeKind::Document) {
            return Err(DomError::HierarchyRequest);
        }
        if !matches!(parent_kind, NodeKind::Document) {
            return match node_kind {
                NodeKind::Doctype { .. } => Err(DomError::HierarchyRequest),
                _ => Ok(()),
            };
        }
        if matches!(node_kind, NodeKind::Text { .. } | NodeKind::CData { .. }) {
            return Err(DomError::HierarchyRequest);
        }
        if self.is_character_data(node) {
            return Ok(());
        }
        let is_element = |n: NodeId| matches!(self.get(n).kind, NodeKind::Element { .. });
        let is_doctype = |n: NodeId| matches!(self.get(n).kind, NodeKind::Doctype { .. });
        let others = || {
            self.get(parent)
                .children
                .iter()
                .copied()
                .filter(move |&c| Some(c) != exclude)
        };
        let doctype_follows = |c: NodeId| {
            let siblings = &self.get(parent).children;
            let i = siblings
                .iter()
                .position(|&s| s == c)
                .unwrap_or(siblings.len());
            siblings[i + 1..].iter().any(|&s| is_doctype(s))
        };
        let element_precedes = |c: NodeId| {
            let siblings = &self.get(parent).children;
            let i = siblings.iter().position(|&s| s == c).unwrap_or(0);
            siblings[..i].iter().any(|&s| is_element(s))
        };
        if matches!(node_kind, NodeKind::DocumentFragment) {
            let children = &self.get(node).children;
            let elements = children.iter().filter(|&&c| is_element(c)).count();
            if elements > 1
                || children.iter().any(|&c| {
                    matches!(
                        self.get(c).kind,
                        NodeKind::Text { .. } | NodeKind::CData { .. }
                    )
                })
            {
                return Err(DomError::HierarchyRequest);
            }
            if elements == 0 {
                return Ok(());
            }
        }
        if matches!(
            node_kind,
            NodeKind::DocumentFragment | NodeKind::Element { .. }
        ) {
            let conflict = others().any(is_element)
                || child.is_some_and(doctype_follows)
                || child.is_some_and(|c| is_doctype(c) && Some(c) != exclude);
            return if conflict {
                Err(DomError::HierarchyRequest)
            } else {
                Ok(())
            };
        }
        let conflict = others().any(is_doctype)
            || child.is_some_and(element_precedes)
            || (child.is_none() && others().any(is_element));
        if conflict {
            Err(DomError::HierarchyRequest)
        } else {
            Ok(())
        }
    }

    /// "Pre-insert" `node` into `parent` before `child` (append when `None`).
    pub fn pre_insert(
        &mut self,
        node: NodeId,
        parent: NodeId,
        child: Option<NodeId>,
    ) -> Result<(), DomError> {
        self.ensure_pre_insert_validity(node, parent, child, None)?;
        let reference = if child == Some(node) {
            self.next_sibling(node)
        } else {
            child
        };
        self.insert_node(node, parent, reference);
        Ok(())
    }

    /// "Replace" `child` with `node` within `parent`.
    pub fn replace_child_checked(
        &mut self,
        parent: NodeId,
        node: NodeId,
        child: NodeId,
    ) -> Result<(), DomError> {
        self.ensure_pre_insert_validity(node, parent, Some(child), Some(child))?;
        let mut reference = self.next_sibling(child);
        if reference == Some(node) {
            reference = self.next_sibling(node);
        }
        if self.get(child).parent.is_some() {
            self.remove_child(parent, child);
        }
        self.insert_node(node, parent, reference);
        Ok(())
    }

    /// "Pre-remove" `child` from `parent`.
    pub fn pre_remove(&mut self, parent: NodeId, child: NodeId) -> Result<(), DomError> {
        if self.get(child).parent != Some(parent) {
            return Err(DomError::NotFound);
        }
        self.remove_child(parent, child);
        Ok(())
    }

    /// "Insert": a fragment contributes its children, in order.
    fn insert_node(&mut self, node: NodeId, parent: NodeId, before: Option<NodeId>) {
        let nodes = if matches!(self.get(node).kind, NodeKind::DocumentFragment) {
            let children = self.get(node).children.clone();
            for &c in &children {
                self.remove_child(node, c);
            }
            children
        } else {
            vec![node]
        };
        for n in nodes {
            self.insert_before(parent, n, before);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Document, DomError};

    #[test]
    fn document_children_are_constrained() {
        let mut doc = Document::new();
        let root = doc.document_id.unwrap();
        let html = doc.create_element("html");
        let text = doc.create_text("x");
        let doctype = doc.create_doctype("html");
        assert_eq!(
            doc.pre_insert(text, root, None),
            Err(DomError::HierarchyRequest)
        );
        doc.pre_insert(html, root, None).unwrap();
        let second = doc.create_element("body");
        assert_eq!(
            doc.pre_insert(second, root, None),
            Err(DomError::HierarchyRequest)
        );
        assert_eq!(
            doc.pre_insert(doctype, root, None),
            Err(DomError::HierarchyRequest)
        );
        doc.pre_insert(doctype, root, Some(html)).unwrap();
        assert_eq!(doc.get(root).children, vec![doctype, html]);
        assert_eq!(doc.replace_child_checked(root, second, html), Ok(()));
        assert_eq!(doc.get(root).children, vec![doctype, second]);
    }

    #[test]
    fn cycles_and_foreign_children_are_rejected() {
        let mut doc = Document::new();
        let a = doc.create_element("div");
        let b = doc.create_element("span");
        let stray = doc.create_element("p");
        doc.pre_insert(b, a, None).unwrap();
        assert_eq!(doc.pre_insert(a, b, None), Err(DomError::HierarchyRequest));
        assert_eq!(doc.pre_insert(a, a, None), Err(DomError::HierarchyRequest));
        assert_eq!(
            doc.pre_insert(stray, a, Some(stray)),
            Err(DomError::NotFound)
        );
        let text = doc.create_text("t");
        assert_eq!(
            doc.pre_insert(stray, text, None),
            Err(DomError::HierarchyRequest)
        );
        assert_eq!(doc.pre_remove(b, a), Err(DomError::NotFound));
    }

    #[test]
    fn fragments_insert_their_children() {
        let mut doc = Document::new();
        let parent = doc.create_element("div");
        let last = doc.create_element("b");
        doc.pre_insert(last, parent, None).unwrap();
        let fragment = doc.create_document_fragment();
        let x = doc.create_text("x");
        let y = doc.create_element("i");
        doc.pre_insert(x, fragment, None).unwrap();
        doc.pre_insert(y, fragment, None).unwrap();
        doc.pre_insert(fragment, parent, Some(last)).unwrap();
        assert_eq!(doc.get(parent).children, vec![x, y, last]);
        assert!(doc.get(fragment).children.is_empty());
        assert_eq!(doc.get(x).parent, Some(parent));
    }
}
