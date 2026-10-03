//! Layout: box tree construction and CSS visual formatting into a fragment tree.
//!
//! Block formatting (CSS 2 §9–10) handles widths and auto margins, `box-sizing`,
//! min/max sizes, margin collapsing between siblings and through parents, relative
//! positioning, floats and clearance, tables, and absolutely positioned boxes. Inline
//! formatting (CSS Text 3 / CSS Inline 3) processes white space across element
//! boundaries, shortens line boxes next to floats, breaks lines at UAX #14
//! opportunities, reorders bidi runs (UAX #9), aligns and justifies lines and places
//! inline boxes, atomic inlines and replaced elements on font-metric line boxes. Text is
//! shaped by `axiom-text`, and paint draws exactly the glyphs layout measured.
//!
//! Fragments carry absolute page coordinates.

mod block;
mod flex;
mod float;
mod grid;
mod inline;
mod table;
mod tree;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use axiom_dom::{Document, NodeId};
use axiom_style::{Color, ComputedStyle, Position, StyleMap};
use axiom_text::ShapedRun;

pub use axiom_text;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Rect::new(
            x,
            y,
            (self.right().min(other.right()) - x).max(0.0),
            (self.bottom().min(other.bottom()) - y).max(0.0),
        )
    }
}

/// Decoded RGBA8 pixels (unpremultiplied).
#[derive(Debug, Clone, PartialEq)]
pub struct RasterImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Page state layout reads besides the DOM and styles.
pub struct LayoutInputs<'a> {
    pub images: &'a HashMap<NodeId, Arc<RasterImage>>,
    /// Rasterized inline `<svg>` roots.
    pub inline_svgs: &'a HashMap<NodeId, Arc<RasterImage>>,
    pub form_values: &'a HashMap<NodeId, String>,
    /// Checkedness script or the user set; absent entries fall back to `checked`.
    pub form_checked: &'a HashMap<NodeId, bool>,
}

impl LayoutInputs<'static> {
    pub fn empty() -> Self {
        static IMAGES: OnceLock<HashMap<NodeId, Arc<RasterImage>>> = OnceLock::new();
        static VALUES: OnceLock<HashMap<NodeId, String>> = OnceLock::new();
        static CHECKED: OnceLock<HashMap<NodeId, bool>> = OnceLock::new();
        Self {
            images: IMAGES.get_or_init(HashMap::new),
            inline_svgs: IMAGES.get_or_init(HashMap::new),
            form_values: VALUES.get_or_init(HashMap::new),
            form_checked: CHECKED.get_or_init(HashMap::new),
        }
    }
}

/// Simple vector shapes (list markers and form control glyphs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Disc,
    Circle,
    Square,
    DisclosureClosed,
    DisclosureOpen,
    Check,
    RadioDot,
    DropdownArrow,
}

impl Shape {
    pub(crate) fn as_char(self) -> char {
        match self {
            Shape::Disc | Shape::RadioDot => '\u{2022}',
            Shape::Circle => '\u{25E6}',
            Shape::Square => '\u{25AA}',
            Shape::DisclosureClosed => '\u{25B8}',
            Shape::DisclosureOpen | Shape::DropdownArrow => '\u{25BE}',
            Shape::Check => '\u{2713}',
        }
    }
}

/// Which horizontal edges of a box fragment are real (inline boxes split across lines
/// only draw the start edge on their first fragment and the end edge on their last).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxEdges {
    pub left: bool,
    pub right: bool,
    /// Inline-level box (inline box, inline-block, inline replaced element): it paints
    /// with the inline content of its block, after block backgrounds (CSS 2 Appendix E).
    pub inline: bool,
}

impl BoxEdges {
    pub const BOTH: BoxEdges = BoxEdges {
        left: true,
        right: true,
        inline: false,
    };
}

#[derive(Debug, Clone)]
pub struct TextFragment {
    pub text: String,
    pub run: Arc<ShapedRun>,
    /// Absolute baseline y; glyph x positions are relative to the fragment's `rect.x`.
    pub baseline: f32,
    pub font_size: f32,
    pub color: Color,
    pub decorations: u8,
    pub decoration_color: Color,
}

#[derive(Debug, Clone)]
pub enum FragmentKind {
    /// A box with background and borders: block, atomic inline, replaced element, or
    /// one line's piece of an inline box.
    Box(BoxEdges),
    Text(TextFragment),
    Image(Arc<RasterImage>),
    Shape(Shape, Color),
    /// An absolutely positioned box before the positioning pass replaces it.
    AbsPlaceholder(usize),
}

#[derive(Debug, Clone)]
pub struct Fragment {
    /// The element this fragment belongs to (for text: the parent element).
    pub node: Option<NodeId>,
    pub style: Arc<ComputedStyle>,
    /// Border box (text: the glyphs' content area).
    pub rect: Rect,
    pub kind: FragmentKind,
    pub children: Vec<Fragment>,
}

impl Fragment {
    pub(crate) fn translate(&mut self, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.rect.x += dx;
        self.rect.y += dy;
        if let FragmentKind::Text(t) = &mut self.kind {
            t.baseline += dy;
        }
        for c in &mut self.children {
            c.translate(dx, dy);
        }
    }

    /// The padding box (border box minus borders).
    pub fn padding_rect(&self) -> Rect {
        let [t, r, b, l] = self.style.border_widths();
        Rect::new(
            self.rect.x + l,
            self.rect.y + t,
            (self.rect.width - l - r).max(0.0),
            (self.rect.height - t - b).max(0.0),
        )
    }

    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Fragment::count).sum::<usize>()
    }
}

#[derive(Debug, Clone)]
pub struct LayoutTree {
    pub root: Fragment,
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// The canvas background (the root's, or the body's when the root has none).
    pub canvas_background: Color,
    /// Bottom of the laid-out content (scrollable height).
    pub content_height: f32,
}

impl LayoutTree {
    /// The frontmost element at page coordinates (`x`, `y`).
    pub fn hit_test(&self, x: f32, y: f32) -> Option<NodeId> {
        fn walk(f: &Fragment, x: f32, y: f32) -> Option<NodeId> {
            let clips = f.style.overflow_x.clips() || f.style.overflow_y.clips();
            if clips && matches!(f.kind, FragmentKind::Box(_)) && !f.rect.contains(x, y) {
                return None;
            }
            // Positioned children paint on top; test them first.
            for c in f.children.iter().rev().filter(|c| c.style.is_positioned()) {
                if let Some(n) = walk(c, x, y) {
                    return Some(n);
                }
            }
            for c in f.children.iter().rev().filter(|c| !c.style.is_positioned()) {
                if let Some(n) = walk(c, x, y) {
                    return Some(n);
                }
            }
            if f.rect.contains(x, y) && !f.style.visibility_hidden {
                return f.node;
            }
            None
        }
        walk(&self.root, x, y).or(self.root.node)
    }

    /// Union of the border boxes of `node`'s fragments.
    pub fn node_rect(&self, node: NodeId) -> Option<Rect> {
        fn walk(f: &Fragment, node: NodeId, acc: &mut Option<Rect>) {
            if f.node == Some(node) && !matches!(f.kind, FragmentKind::Text(_)) {
                *acc = Some(match acc {
                    Some(r) => r.union(&f.rect),
                    None => f.rect,
                });
            }
            for c in &f.children {
                walk(c, node, acc);
            }
        }
        let mut acc = None;
        walk(&self.root, node, &mut acc);
        acc
    }

    /// `node`'s box, image and shape fragments in tree order (one per line for inline
    /// boxes); text fragments are covered by their inline box.
    pub fn node_fragments(&self, node: NodeId) -> Vec<&Fragment> {
        fn walk<'a>(f: &'a Fragment, node: NodeId, out: &mut Vec<&'a Fragment>) {
            if f.node == Some(node)
                && !matches!(
                    f.kind,
                    FragmentKind::Text(_) | FragmentKind::AbsPlaceholder(_)
                )
            {
                out.push(f);
            }
            for c in &f.children {
                walk(c, node, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.root, node, &mut out);
        out
    }

    pub fn fragment_count(&self) -> usize {
        self.root.count()
    }
}

pub(crate) struct Ctx<'a> {
    /// Absolutely positioned boxes found during flow layout, by placeholder index.
    pub abs_boxes: std::cell::RefCell<Vec<&'a tree::BoxNode>>,
}

pub fn layout_document(
    doc: &Document,
    styles: &StyleMap,
    inputs: &LayoutInputs,
    viewport_width: f32,
    viewport_height: f32,
) -> LayoutTree {
    let builder = tree::Builder {
        doc,
        styles,
        inputs,
    };
    let Some(root_box) = builder.build_root() else {
        return LayoutTree {
            root: Fragment {
                node: None,
                style: Arc::new(ComputedStyle::initial()),
                rect: Rect::new(0.0, 0.0, viewport_width, viewport_height),
                kind: FragmentKind::Box(BoxEdges::BOTH),
                children: Vec::new(),
            },
            viewport_width,
            viewport_height,
            canvas_background: Color::WHITE,
            content_height: viewport_height,
        };
    };
    let ctx = Ctx {
        abs_boxes: std::cell::RefCell::new(Vec::new()),
    };
    let icb = block::Containing {
        width: viewport_width,
        height: Some(viewport_height),
        rtl: false,
    };
    let out = block::layout_block_level(&ctx, &root_box, icb, 0.0, true, None);
    let mut root = out.frag;
    let margin_top = out.top_margin.value();
    root.translate(0.0, margin_top);

    let viewport = Rect::new(0.0, 0.0, viewport_width, viewport_height);
    block::position_absolutes(&ctx, &mut root, viewport, viewport);

    // Canvas background: the root's, else the body's (whose own is then not painted).
    let mut canvas_background = root.style.background_color;
    if canvas_background.a == 0 {
        let body = doc.body();
        if let Some(body_frag) = root
            .children
            .iter_mut()
            .find(|c| c.node.is_some() && c.node == body)
        {
            canvas_background = body_frag.style.background_color;
            if canvas_background.a > 0 {
                let mut s = (*body_frag.style).clone();
                s.background_color = Color::TRANSPARENT;
                body_frag.style = Arc::new(s);
            }
        }
    } else {
        let mut s = (*root.style).clone();
        s.background_color = Color::TRANSPARENT;
        root.style = Arc::new(s);
    }
    if canvas_background.a == 0 {
        canvas_background = Color::WHITE;
    }

    let content_height = content_bottom(&root).max(viewport_height);
    LayoutTree {
        root,
        viewport_width,
        viewport_height,
        canvas_background,
        content_height,
    }
}

fn content_bottom(f: &Fragment) -> f32 {
    let mut bottom = f.rect.bottom();
    let clips = f.style.overflow_y.clips() && matches!(f.kind, FragmentKind::Box(_));
    if !clips {
        for c in &f.children {
            if c.style.position == Position::Fixed {
                continue;
            }
            bottom = bottom.max(content_bottom(c));
        }
    }
    bottom
}

#[cfg(test)]
mod tests;
