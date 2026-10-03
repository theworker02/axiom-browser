//! Box tree construction: DOM + computed styles → boxes (CSS Display §2).
//!
//! `display: contents` splices children into the parent, block containers with mixed
//! children wrap inline runs in anonymous blocks (runs of collapsible white space are
//! dropped), inline boxes that contain block-level boxes become blocks, and replaced
//! elements and form controls become atomic boxes with intrinsic sizes.

use std::cell::OnceCell;
use std::sync::Arc;

use axiom_dom::{Document, NodeId, NodeKind};
use axiom_style::{
    ComputedStyle, Display, Float, GeneratedBox, ListStyleType, PseudoElement, StyleMap,
};

use crate::{LayoutInputs, RasterImage};

pub(crate) struct BoxNode {
    pub node: Option<NodeId>,
    pub style: Arc<ComputedStyle>,
    pub kind: BoxKind,
    pub children: Vec<BoxNode>,
    /// Outside list marker (`list-style-position: outside`).
    pub marker: Option<Marker>,
    /// (min-content, max-content) border-box widths.
    pub intrinsic: OnceCell<(f32, f32)>,
    /// Table cells: columns and rows spanned.
    pub colspan: u16,
    pub rowspan: u16,
    /// In-flow child of a flex or grid container (an independent formatting context).
    pub item: bool,
}

pub(crate) enum BoxKind {
    /// Block-level box; the formatting algorithm follows `style.display`.
    Block,
    /// Non-atomic inline box.
    Inline,
    /// Raw DOM text (white space is processed by the inline formatting context).
    Text(String),
    /// Inline-level block container (`inline-block`, `inline-flex`, `button`, …).
    Atomic,
    Replaced(Replaced),
    LineBreak,
    WordBreak,
}

pub(crate) enum Replaced {
    Image(Arc<RasterImage>),
    /// An inline `<svg>` root. Only fixed `width` / `height` attributes are natural
    /// dimensions; without them the viewBox ratio sizes it against the available width.
    Svg {
        image: Arc<RasterImage>,
        width: Option<f32>,
        height: Option<f32>,
        ratio: Option<f32>,
    },
    /// Unloaded image / `svg` / `video` / `iframe` …: a sized empty box.
    Empty {
        width: f32,
        height: f32,
    },
    Widget(Widget),
}

#[derive(Clone)]
pub(crate) enum Widget {
    TextField {
        text: String,
        placeholder: bool,
        size: u32,
    },
    Button {
        label: String,
    },
    Check {
        checked: bool,
        radio: bool,
    },
    Select {
        label: String,
        widest: String,
    },
    TextArea {
        text: String,
        cols: u32,
        rows: u32,
    },
    Range,
    Color,
}

#[derive(Clone)]
pub(crate) enum Marker {
    Text(String),
    Shape(crate::Shape),
}

impl BoxNode {
    pub fn new(node: Option<NodeId>, style: Arc<ComputedStyle>, kind: BoxKind) -> Self {
        Self {
            node,
            style,
            kind,
            children: Vec::new(),
            marker: None,
            intrinsic: OnceCell::new(),
            colspan: 1,
            rowspan: 1,
            item: false,
        }
    }

    /// Inline-level (participates in an inline formatting context).
    pub fn is_inline_level(&self) -> bool {
        match self.kind {
            BoxKind::Inline
            | BoxKind::Text(_)
            | BoxKind::Atomic
            | BoxKind::LineBreak
            | BoxKind::WordBreak => true,
            BoxKind::Replaced(_) => self.style.display.is_inline_level(),
            BoxKind::Block => false,
        }
    }

    /// Whether the box has its own style (text and breaks share their parent's).
    fn is_element_box(&self) -> bool {
        !matches!(
            self.kind,
            BoxKind::Text(_) | BoxKind::LineBreak | BoxKind::WordBreak
        )
    }

    pub fn is_out_of_flow(&self) -> bool {
        self.is_element_box() && self.style.position.is_out_of_flow()
    }

    pub fn is_float(&self) -> bool {
        self.is_element_box() && self.style.float != Float::None && !self.is_out_of_flow()
    }

    fn is_collapsible_whitespace(&self) -> bool {
        match &self.kind {
            BoxKind::Text(t) => {
                !self.style.preserves_spaces()
                    && (!self.style.preserves_newlines() || !t.contains('\n'))
                    && t.chars()
                        .all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
            }
            _ => false,
        }
    }
}

pub(crate) struct Builder<'a> {
    pub doc: &'a Document,
    pub styles: &'a StyleMap,
    pub inputs: &'a LayoutInputs<'a>,
}

impl<'a> Builder<'a> {
    /// The root box (the document element).
    pub fn build_root(&self) -> Option<BoxNode> {
        let root = self.doc.document_element()?;
        let style = self.styles.get(root)?.clone();
        if style.display == Display::None {
            return None;
        }
        let mut bx = BoxNode::new(Some(root), style, BoxKind::Block);
        let children = self.children_of(root, &bx.style);
        bx.children = fix_children(children, &bx.style);
        Some(bx)
    }

    fn children_of(&self, parent: NodeId, parent_style: &Arc<ComputedStyle>) -> Vec<BoxNode> {
        let mut out = Vec::new();
        let is_ol = self
            .doc
            .tag_name(parent)
            .is_some_and(|t| t.eq_ignore_ascii_case("ol"));
        let reversed = is_ol && self.doc.has_attr(parent, "reversed");
        let mut counter: i64 = if is_ol {
            self.doc
                .attr(parent, "start")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or_else(|| {
                    if reversed {
                        self.count_list_items(parent) as i64
                    } else {
                        1
                    }
                })
        } else {
            1
        };
        let flat = self.doc.flat_children(parent);
        let children = &*flat;
        if self
            .doc
            .tag_name(parent)
            .is_some_and(|t| t.eq_ignore_ascii_case("details"))
        {
            // A details element renders its first summary child, then (only when open)
            // everything else; without a summary it gets a default "Details" legend.
            let summary = children.iter().copied().find(|&c| {
                self.doc
                    .tag_name(c)
                    .is_some_and(|t| t.eq_ignore_ascii_case("summary"))
            });
            let open = self.doc.has_attr(parent, "open");
            match summary {
                Some(s) => self.build_node(s, parent_style, &mut counter, reversed, &mut out),
                None => {
                    let mut style = ComputedStyle::inherit_from(parent_style);
                    style.display = Display::ListItem;
                    style.list_style_type = if open {
                        ListStyleType::DisclosureOpen
                    } else {
                        ListStyleType::DisclosureClosed
                    };
                    let style = Arc::new(style);
                    let mut bx = BoxNode::new(None, style.clone(), BoxKind::Block);
                    bx.marker = marker_for(&style.list_style_type, 1);
                    bx.children
                        .push(BoxNode::new(None, style, BoxKind::Text("Details".into())));
                    out.push(bx);
                }
            }
            if open {
                for &child in children.iter().filter(|&&c| Some(c) != summary) {
                    self.build_node(child, parent_style, &mut counter, reversed, &mut out);
                }
            }
            return out;
        }
        for &child in children {
            self.build_node(child, parent_style, &mut counter, reversed, &mut out);
        }
        out
    }

    fn count_list_items(&self, parent: NodeId) -> usize {
        self.doc
            .get(parent)
            .children
            .iter()
            .filter(|&&c| {
                self.styles
                    .get(c)
                    .is_some_and(|s| s.display == Display::ListItem)
            })
            .count()
    }

    fn build_node(
        &self,
        id: NodeId,
        parent_style: &Arc<ComputedStyle>,
        counter: &mut i64,
        reversed: bool,
        out: &mut Vec<BoxNode>,
    ) {
        let node = self.doc.get(id);
        match &node.kind {
            NodeKind::Text { data } | NodeKind::CData { data } => {
                if data.is_empty() {
                    return;
                }
                let style = self
                    .styles
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| parent_style.clone());
                out.push(BoxNode::new(Some(id), style, BoxKind::Text(data.clone())));
            }
            NodeKind::Element { tag, .. } => {
                let Some(style) = self.styles.get(id).cloned() else {
                    return;
                };
                if style.display == Display::None {
                    return;
                }
                if style.display == Display::Contents {
                    for &c in self.doc.flat_children(id).iter() {
                        self.build_node(c, &style, counter, reversed, out);
                    }
                    return;
                }
                let tag = tag.to_ascii_lowercase();
                let mut bx = self.build_element(id, &tag, style);
                if bx.style.display == Display::ListItem {
                    if let Some(v) = self
                        .doc
                        .attr(id, "value")
                        .and_then(|v| v.trim().parse().ok())
                    {
                        *counter = v;
                    }
                    let ordinal = *counter;
                    *counter += if reversed { -1 } else { 1 };
                    if let Some(marker) = marker_for(&bx.style.list_style_type, ordinal) {
                        if bx.style.list_style_inside {
                            let text = match marker {
                                Marker::Text(t) => t,
                                Marker::Shape(s) => format!("{} ", s.as_char()),
                            };
                            let t = BoxNode::new(None, bx.style.clone(), BoxKind::Text(text));
                            bx.children.insert(0, t);
                            if bx.children.len() > 1 && !bx.children[1].is_inline_level() {
                                bx.children = fix_block_children(bx.children, &bx.style);
                            }
                        } else {
                            bx.marker = Some(marker);
                        }
                    }
                }
                out.push(bx);
            }
            _ => {}
        }
    }

    fn build_element(&self, id: NodeId, tag: &str, style: Arc<ComputedStyle>) -> BoxNode {
        if let Some(replaced) = self.replaced(id, tag, &style) {
            return BoxNode::new(Some(id), style, BoxKind::Replaced(replaced));
        }
        match tag {
            "br" => return BoxNode::new(Some(id), style, BoxKind::LineBreak),
            "wbr" => return BoxNode::new(Some(id), style, BoxKind::WordBreak),
            "img" => {
                // No image and no size: the alt text renders inline.
                let alt = self.doc.attr(id, "alt").unwrap_or("").to_string();
                let mut bx = BoxNode::new(Some(id), style.clone(), BoxKind::Inline);
                if !alt.is_empty() {
                    bx.children
                        .push(BoxNode::new(None, style, BoxKind::Text(alt)));
                }
                return bx;
            }
            _ => {}
        }
        let mut children = self.children_of(id, &style);
        if let Some(g) = self.styles.generated(id, PseudoElement::Before) {
            children.insert(0, self.generated_box(g));
        }
        if let Some(g) = self.styles.generated(id, PseudoElement::After) {
            children.push(self.generated_box(g));
        }
        let mut bx = self.container(Some(id), tag, style, children);
        if bx.style.display == Display::TableCell && matches!(tag, "td" | "th") {
            let span = |name: &str, max: u16| {
                self.doc
                    .attr(id, name)
                    .and_then(|v| v.trim().parse::<u16>().ok())
                    .map_or(1, |n| n.clamp(1, max))
            };
            bx.colspan = span("colspan", 1000);
            bx.rowspan = span("rowspan", 65534);
        }
        bx
    }

    /// The box of a `::before` / `::after` pseudo-element.
    fn generated_box(&self, g: &GeneratedBox) -> BoxNode {
        let mut children = Vec::new();
        if !g.text.is_empty() {
            children.push(BoxNode::new(
                None,
                g.style.clone(),
                BoxKind::Text(g.text.clone()),
            ));
        }
        self.container(None, "", g.style.clone(), children)
    }

    /// A non-replaced box of `style` around `children`, fixed up for its display type.
    fn container(
        &self,
        node: Option<NodeId>,
        tag: &str,
        style: Arc<ComputedStyle>,
        children: Vec<BoxNode>,
    ) -> BoxNode {
        let display = style.display;
        let kind = if display.is_inline_level() || display == Display::Contents {
            if matches!(display, Display::Inline | Display::Contents) && tag != "button" {
                BoxKind::Inline
            } else {
                BoxKind::Atomic
            }
        } else {
            BoxKind::Block
        };
        let mut bx = BoxNode::new(node, style, kind);
        let table_level = match display {
            Display::Table | Display::InlineTable => Some(TableLevel::Table),
            Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup => {
                Some(TableLevel::Group)
            }
            Display::TableRow => Some(TableLevel::Row),
            _ => None,
        };
        if let Some(level) = table_level {
            bx.children = fix_table_children(children, &bx.style, level);
            return bx;
        }
        match bx.kind {
            BoxKind::Inline => {
                if children
                    .iter()
                    .any(|c| !c.is_inline_level() && !c.is_out_of_flow() && !c.is_float())
                {
                    // Block-in-inline: treat the inline box as a block container. The
                    // inline's own box does not wrap the block (CSS 2 §9.2.1.1), so it
                    // has no decorations and its vertical margins are ignored.
                    bx.kind = BoxKind::Block;
                    let mut s = (*bx.style).clone();
                    let zero = axiom_style::Length::Px(0.0);
                    s.margin.top = zero.clone();
                    s.margin.bottom = zero.clone();
                    s.padding = axiom_style::Edges {
                        top: zero.clone(),
                        right: zero.clone(),
                        bottom: zero.clone(),
                        left: zero,
                    };
                    for side in &mut s.border {
                        side.width = 0.0;
                    }
                    s.background_color = axiom_style::Color::TRANSPARENT;
                    bx.style = Arc::new(s);
                    bx.children = fix_block_children(children, &bx.style);
                } else {
                    bx.children = children;
                }
            }
            _ => bx.children = fix_children(children, &bx.style),
        }
        bx
    }

    fn replaced(&self, id: NodeId, tag: &str, style: &ComputedStyle) -> Option<Replaced> {
        let attr_px = |name: &str| -> Option<f32> {
            self.doc
                .attr(id, name)
                .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
        };
        match tag {
            "img" => {
                if let Some(img) = self.inputs.images.get(&id) {
                    return Some(Replaced::Image(img.clone()));
                }
                let (w, h) = (attr_px("width"), attr_px("height"));
                let css_sized = !style.width.is_auto() || !style.height.is_auto();
                if w.is_some() || h.is_some() || css_sized {
                    return Some(Replaced::Empty {
                        width: w.unwrap_or(0.0),
                        height: h.unwrap_or(0.0),
                    });
                }
                None
            }
            "svg" | "video" | "canvas" | "iframe" | "embed" | "object" | "audio" => {
                if let Some(img) = self.inputs.inline_svgs.get(&id).filter(|_| tag == "svg") {
                    let ratio = self
                        .doc
                        .attr(id, "viewBox")
                        .and_then(view_box_ratio)
                        .or_else(|| (img.height > 0).then(|| img.width as f32 / img.height as f32));
                    return Some(Replaced::Svg {
                        image: img.clone(),
                        width: attr_px("width"),
                        height: attr_px("height"),
                        ratio,
                    });
                }
                if let Some(img) = self.inputs.images.get(&id) {
                    return Some(Replaced::Image(img.clone()));
                }
                let (dw, dh) = match tag {
                    "audio" => (300.0, 54.0),
                    _ => (300.0, 150.0),
                };
                Some(Replaced::Empty {
                    width: attr_px("width").unwrap_or(dw),
                    height: attr_px("height").unwrap_or(dh),
                })
            }
            "input" => Some(self.input_widget(id)),
            "select" => {
                let mut label = String::new();
                let mut widest = String::new();
                let mut first = None;
                let mut selected = None;
                for opt in self.doc.descendants(id) {
                    if self
                        .doc
                        .tag_name(opt)
                        .is_some_and(|t| t.eq_ignore_ascii_case("option"))
                    {
                        let text = self
                            .doc
                            .attr(opt, "label")
                            .map(str::to_string)
                            .unwrap_or_else(|| collapse_ws(&self.doc.text_content(opt)));
                        if text.chars().count() > widest.chars().count() {
                            widest = text.clone();
                        }
                        if first.is_none() {
                            first = Some(text.clone());
                        }
                        let chosen = self
                            .inputs
                            .form_values
                            .get(&id)
                            .map(|v| {
                                self.doc
                                    .attr(opt, "value")
                                    .map(str::to_string)
                                    .unwrap_or(text.clone())
                                    == *v
                            })
                            .unwrap_or_else(|| self.doc.has_attr(opt, "selected"));
                        if chosen && selected.is_none() {
                            selected = Some(text);
                        }
                    }
                }
                if let Some(s) = selected.or(first) {
                    label = s;
                }
                Some(Replaced::Widget(Widget::Select { label, widest }))
            }
            "textarea" => {
                let text = self
                    .inputs
                    .form_values
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| self.doc.text_content(id));
                let (text, _) = if text.is_empty() {
                    (
                        self.doc.attr(id, "placeholder").unwrap_or("").to_string(),
                        true,
                    )
                } else {
                    (text, false)
                };
                Some(Replaced::Widget(Widget::TextArea {
                    text,
                    cols: attr_px("cols").map(|v| v as u32).unwrap_or(20).max(1),
                    rows: attr_px("rows").map(|v| v as u32).unwrap_or(2).max(1),
                }))
            }
            _ => None,
        }
    }

    fn input_widget(&self, id: NodeId) -> Replaced {
        let ty = self
            .doc
            .attr(id, "type")
            .unwrap_or("text")
            .trim()
            .to_ascii_lowercase();
        let value = self
            .inputs
            .form_values
            .get(&id)
            .cloned()
            .or_else(|| self.doc.attr(id, "value").map(str::to_string));
        let w = match ty.as_str() {
            "checkbox" | "radio" => Widget::Check {
                checked: self
                    .inputs
                    .form_checked
                    .get(&id)
                    .copied()
                    .unwrap_or_else(|| self.doc.has_attr(id, "checked")),
                radio: ty == "radio",
            },
            "submit" | "reset" | "button" => Widget::Button {
                label: value.unwrap_or_else(|| {
                    match ty.as_str() {
                        "submit" => "Submit",
                        "reset" => "Reset",
                        _ => "",
                    }
                    .to_string()
                }),
            },
            "image" => {
                if let Some(img) = self.inputs.images.get(&id) {
                    return Replaced::Image(img.clone());
                }
                Widget::Button {
                    label: self.doc.attr(id, "alt").unwrap_or("Submit").to_string(),
                }
            }
            "file" => Widget::Button {
                label: "Choose File".to_string(),
            },
            "range" => Widget::Range,
            "color" => Widget::Color,
            _ => {
                let size = self
                    .doc
                    .attr(id, "size")
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(20u32)
                    .max(1);
                match value.filter(|v| !v.is_empty()) {
                    Some(v) => Widget::TextField {
                        text: if ty == "password" {
                            "\u{2022}".repeat(v.chars().count())
                        } else {
                            v
                        },
                        placeholder: false,
                        size,
                    },
                    None => Widget::TextField {
                        text: self.doc.attr(id, "placeholder").unwrap_or("").to_string(),
                        placeholder: true,
                        size,
                    },
                }
            }
        };
        Replaced::Widget(w)
    }
}

/// Width over height of an SVG `viewBox="min-x min-y width height"`.
fn view_box_ratio(v: &str) -> Option<f32> {
    let n: Vec<f32> = v
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    match n[..] {
        [_, _, w, h] if w > 0.0 && h > 0.0 => Some(w / h),
        _ => None,
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Children of a block, flex or grid container.
fn fix_children(children: Vec<BoxNode>, parent_style: &Arc<ComputedStyle>) -> Vec<BoxNode> {
    if parent_style.display.is_flex() || parent_style.display.is_grid() {
        fix_item_children(children, parent_style)
    } else {
        fix_block_children(children, parent_style)
    }
}

/// Flex and grid items (CSS Flexbox 1 §4, CSS Grid 1 §6): every in-flow child is an item,
/// and each contiguous run of text becomes an anonymous item unless it is only
/// collapsible white space.
fn fix_item_children(children: Vec<BoxNode>, parent_style: &Arc<ComputedStyle>) -> Vec<BoxNode> {
    let mut out = Vec::new();
    let mut run: Vec<BoxNode> = Vec::new();
    let flush = |run: &mut Vec<BoxNode>, out: &mut Vec<BoxNode>| {
        if run.iter().all(BoxNode::is_collapsible_whitespace) {
            run.clear();
            return;
        }
        let mut style = ComputedStyle::inherit_from(parent_style);
        style.display = Display::Block;
        let mut anon = BoxNode::new(None, Arc::new(style), BoxKind::Block);
        anon.children = std::mem::take(run);
        anon.item = true;
        out.push(anon);
    };
    for mut c in children {
        if c.is_element_box() {
            flush(&mut run, &mut out);
            c.item = !c.is_out_of_flow();
            out.push(c);
        } else {
            run.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Anonymous block wrapping (CSS 2 §9.2.1.1): a block container holds either only
/// block-level or only inline-level boxes.
pub(crate) fn fix_block_children(
    children: Vec<BoxNode>,
    parent_style: &Arc<ComputedStyle>,
) -> Vec<BoxNode> {
    let neutral = |c: &BoxNode| c.is_out_of_flow() || c.is_float();
    let has_block = children.iter().any(|c| !c.is_inline_level() && !neutral(c));
    if !has_block {
        return children;
    }
    let mut out = Vec::new();
    let mut run: Vec<BoxNode> = Vec::new();
    let flush = |run: &mut Vec<BoxNode>, out: &mut Vec<BoxNode>| {
        if run.is_empty() {
            return;
        }
        let items = std::mem::take(run);
        if items
            .iter()
            .all(|c| c.is_collapsible_whitespace() || neutral(c))
        {
            // Out-of-flow boxes survive; white space between blocks disappears.
            out.extend(items.into_iter().filter(neutral));
            return;
        }
        let mut style = ComputedStyle::inherit_from(parent_style);
        style.display = Display::Block;
        let mut anon = BoxNode::new(None, Arc::new(style), BoxKind::Block);
        anon.children = items;
        out.push(anon);
    };
    for c in children {
        if c.is_inline_level() || (neutral(&c) && !run.is_empty()) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TableLevel {
    Table,
    Group,
    Row,
}

fn is_row_group(d: Display) -> bool {
    matches!(
        d,
        Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup
    )
}

/// Missing table wrappers (CSS 2 §17.2.1): inside a table, runs of boxes that are not
/// captions, row groups or rows get an anonymous row; inside a row group they get an
/// anonymous row; inside a row, runs of non-cells get an anonymous cell. Columns and
/// white space between table parts are dropped.
fn fix_table_children(
    children: Vec<BoxNode>,
    parent_style: &Arc<ComputedStyle>,
    level: TableLevel,
) -> Vec<BoxNode> {
    let keeps = |c: &BoxNode| {
        let d = c.style.display;
        c.is_out_of_flow()
            || match level {
                TableLevel::Table => {
                    d == Display::TableCaption || d == Display::TableRow || is_row_group(d)
                }
                TableLevel::Group => d == Display::TableRow,
                TableLevel::Row => d == Display::TableCell,
            }
    };
    let mut out = Vec::new();
    let mut run: Vec<BoxNode> = Vec::new();
    let flush = |run: &mut Vec<BoxNode>, out: &mut Vec<BoxNode>| {
        if run.iter().all(BoxNode::is_collapsible_whitespace) {
            run.clear();
            return;
        }
        let items = std::mem::take(run);
        let mut style = ComputedStyle::inherit_from(parent_style);
        style.display = if level == TableLevel::Row {
            Display::TableCell
        } else {
            Display::TableRow
        };
        let style = Arc::new(style);
        let mut anon = BoxNode::new(None, style.clone(), BoxKind::Block);
        anon.children = if level == TableLevel::Row {
            fix_block_children(items, &style)
        } else {
            fix_table_children(items, &style, TableLevel::Row)
        };
        out.push(anon);
    };
    for c in children {
        let column = matches!(
            c.style.display,
            Display::TableColumn | Display::TableColumnGroup
        ) && c.is_element_box();
        if column {
            continue;
        }
        if keeps(&c) && c.is_element_box() {
            flush(&mut run, &mut out);
            out.push(c);
        } else {
            run.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn marker_for(ty: &ListStyleType, ordinal: i64) -> Option<Marker> {
    use crate::Shape;
    Some(match ty {
        ListStyleType::None => return None,
        ListStyleType::Disc => Marker::Shape(Shape::Disc),
        ListStyleType::Circle => Marker::Shape(Shape::Circle),
        ListStyleType::Square => Marker::Shape(Shape::Square),
        ListStyleType::DisclosureClosed => Marker::Shape(Shape::DisclosureClosed),
        ListStyleType::DisclosureOpen => Marker::Shape(Shape::DisclosureOpen),
        ListStyleType::Decimal => Marker::Text(format!("{ordinal}. ")),
        ListStyleType::DecimalLeadingZero => Marker::Text(format!("{ordinal:02}. ")),
        ListStyleType::LowerAlpha => Marker::Text(format!("{}. ", alpha(ordinal, b'a'))),
        ListStyleType::UpperAlpha => Marker::Text(format!("{}. ", alpha(ordinal, b'A'))),
        ListStyleType::LowerRoman => Marker::Text(format!("{}. ", roman(ordinal).to_lowercase())),
        ListStyleType::UpperRoman => Marker::Text(format!("{}. ", roman(ordinal))),
        ListStyleType::String(s) => Marker::Text(s.to_string()),
    })
}

fn alpha(mut n: i64, base: u8) -> String {
    if n <= 0 {
        return n.to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push((base + (n % 26) as u8) as char);
        n /= 26;
    }
    out.iter().rev().collect()
}

fn roman(n: i64) -> String {
    if !(1..4000).contains(&n) {
        return n.to_string();
    }
    let table = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut n = n;
    let mut out = String::new();
    for (v, s) in table {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    out
}
