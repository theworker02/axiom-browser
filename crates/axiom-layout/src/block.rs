//! Block formatting: block-level boxes, replaced elements, form controls, intrinsic
//! sizes and absolutely positioned boxes.

use std::sync::Arc;

use axiom_style::{Clear, Color, ComputedStyle, Display, Float, Length, Overflow, Position};
use axiom_text::FontSpec;

use crate::float::Floats;
use crate::table;
use crate::tree::{BoxKind, BoxNode, Marker, Replaced, Widget};
use crate::{flex, grid, inline};
use crate::{BoxEdges, Ctx, Fragment, FragmentKind, Rect, Shape, TextFragment};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Containing {
    pub width: f32,
    /// Definite height for percentage heights, if any.
    pub height: Option<f32>,
    /// `direction: rtl` on the containing block (decides over-constrained margins).
    pub rtl: bool,
}

/// A set of adjoining margins being collapsed (CSS 2 §8.3.1).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Margin {
    pos: f32,
    neg: f32,
}

impl Margin {
    pub fn from(v: f32) -> Self {
        let mut m = Margin::default();
        m.add(v);
        m
    }

    pub fn add(&mut self, v: f32) {
        if v > 0.0 {
            self.pos = self.pos.max(v);
        } else {
            self.neg = self.neg.min(v);
        }
    }

    pub fn merge(&mut self, o: Margin) {
        self.pos = self.pos.max(o.pos);
        self.neg = self.neg.min(o.neg);
    }

    pub fn value(&self) -> f32 {
        self.pos + self.neg
    }
}

pub(crate) struct BoxMetrics {
    /// Top, right, bottom, left.
    pub margin: [f32; 4],
    pub auto_left: bool,
    pub auto_right: bool,
    pub padding: [f32; 4],
    pub border: [f32; 4],
}

impl BoxMetrics {
    pub fn new(s: &ComputedStyle, cb_width: f32) -> Self {
        let m = |l: &Length| l.resolve(cb_width).unwrap_or(0.0);
        let p = |l: &Length| l.resolve(cb_width).unwrap_or(0.0).max(0.0);
        Self {
            margin: [
                m(&s.margin.top),
                m(&s.margin.right),
                m(&s.margin.bottom),
                m(&s.margin.left),
            ],
            auto_left: s.margin.left.is_auto(),
            auto_right: s.margin.right.is_auto(),
            padding: [
                p(&s.padding.top),
                p(&s.padding.right),
                p(&s.padding.bottom),
                p(&s.padding.left),
            ],
            border: s.border_widths(),
        }
    }

    pub fn bp_h(&self) -> f32 {
        self.padding[1] + self.padding[3] + self.border[1] + self.border[3]
    }

    pub fn bp_v(&self) -> f32 {
        self.padding[0] + self.padding[2] + self.border[0] + self.border[2]
    }
}

pub(crate) struct BlockOut {
    /// Border box with its top at y = 0.
    pub frag: Fragment,
    /// The box's top margin collapsed with any margins escaping from its first child.
    pub top_margin: Margin,
    pub bottom_margin: Margin,
    /// Empty box whose top and bottom margins collapse together.
    pub collapse_through: bool,
    /// Baselines relative to the border-box top.
    pub first_baseline: Option<f32>,
    pub last_baseline: Option<f32>,
    pub margin: [f32; 4],
    /// Border-box height the content needs (the height with `height: auto`).
    pub auto_height: f32,
}

/// The block formatting context's floats a box takes part in, with the offset of the
/// box's border-box top from the formatting context's origin.
pub(crate) type FloatCx<'f> = Option<(&'f mut Floats, f32)>;

fn establishes_bfc(bx: &BoxNode, is_root: bool) -> bool {
    let s = &bx.style;
    is_root
        || bx.item
        || bx.is_float()
        || bx.is_out_of_flow()
        || !matches!(bx.kind, BoxKind::Block)
        || s.overflow_x != Overflow::Visible
        || s.overflow_y != Overflow::Visible
        || matches!(
            s.display,
            Display::FlowRoot
                | Display::Flex
                | Display::InlineFlex
                | Display::Grid
                | Display::InlineGrid
                | Display::Table
                | Display::InlineTable
                | Display::TableCell
                | Display::TableCaption
                | Display::InlineBlock
        )
}

/// Content-box width from the `width` property, if it is definite.
pub(crate) fn specified_width(s: &ComputedStyle, cb_width: f32, bp_h: f32) -> Option<f32> {
    let w = s.width.resolve(cb_width)?;
    Some(if s.border_box_sizing {
        (w - bp_h).max(0.0)
    } else {
        w.max(0.0)
    })
}

pub(crate) fn clamp_width(s: &ComputedStyle, w: f32, cb_width: f32, bp_h: f32) -> f32 {
    let adj = |v: f32| if s.border_box_sizing { v - bp_h } else { v };
    let mut w = w;
    if let Some(max) = s.max_width.resolve(cb_width) {
        w = w.min(adj(max));
    }
    if let Some(min) = s.min_width.resolve(cb_width) {
        w = w.max(adj(min));
    }
    w.max(0.0)
}

pub(crate) fn clamp_height(s: &ComputedStyle, h: f32, cb_height: Option<f32>, bp_v: f32) -> f32 {
    let adj = |v: f32| if s.border_box_sizing { v - bp_v } else { v };
    let resolve = |l: &Length| match (l, cb_height) {
        (Length::Px(v), _) => Some(*v),
        (l, Some(cb)) if l.has_percent() => l.resolve(cb),
        _ => None,
    };
    let mut h = h;
    if let Some(max) = resolve(&s.max_height) {
        h = h.min(adj(max));
    }
    if let Some(min) = resolve(&s.min_height) {
        h = h.max(adj(min));
    }
    h.max(0.0)
}

/// Content-box height from the `height` property, if it is definite.
pub(crate) fn specified_height(
    s: &ComputedStyle,
    cb_height: Option<f32>,
    bp_v: f32,
) -> Option<f32> {
    let h = match (&s.height, cb_height) {
        (Length::Px(v), _) => *v,
        (l, Some(cb)) if l.has_percent() => l.resolve(cb)?,
        _ => return None,
    };
    Some(if s.border_box_sizing {
        (h - bp_v).max(0.0)
    } else {
        h.max(0.0)
    })
}

/// Lay out a block-level (or atomic inline) box whose containing block's content box
/// starts at `cb_x`. Returns the border box at y = 0.
pub(crate) fn layout_block_level<'a>(
    ctx: &Ctx<'a>,
    bx: &'a BoxNode,
    cb: Containing,
    cb_x: f32,
    is_root: bool,
    floats: FloatCx,
) -> BlockOut {
    let s = &bx.style;
    let mut m = BoxMetrics::new(s, cb.width);
    let bp_h = m.bp_h();
    let is_table = table::is_table(s.display);
    let shrink = bx.is_float() || matches!(bx.kind, BoxKind::Atomic) || is_table;

    if let BoxKind::Replaced(r) = &bx.kind {
        let (w, h) = replaced_size(bx, r, cb);
        if !shrink && !bx.style.display.is_inline_level() {
            resolve_auto_margins(&mut m, cb.width, w + bp_h, cb.rtl);
        }
        let border_x = cb_x + m.margin[3];
        let mut out = layout_replaced(bx, r, &m, w, h, border_x);
        apply_relative(&mut out.frag, s, cb);
        return out;
    }

    let content_w = match &s.width {
        Length::MinContent => content_intrinsic(ctx, bx).0,
        Length::MaxContent => content_intrinsic(ctx, bx).1,
        Length::FitContent => shrink_to_fit(ctx, bx, cb.width - m.margin[1] - m.margin[3] - bp_h),
        _ => match specified_width(s, cb.width, bp_h) {
            Some(w) => w,
            None if shrink => shrink_to_fit(ctx, bx, cb.width - m.margin[1] - m.margin[3] - bp_h),
            None => (cb.width - m.margin[1] - m.margin[3] - bp_h).max(0.0),
        },
    };
    let mut content_w = clamp_width(s, content_w, cb.width, bp_h);
    if is_table {
        // A table is never narrower than its columns' minimum widths.
        content_w = content_w.max(content_intrinsic(ctx, bx).0);
    }
    let block_level_table = s.display == Display::Table;
    if (!shrink || block_level_table) && !bx.is_float() {
        resolve_auto_margins(&mut m, cb.width, content_w + bp_h, cb.rtl);
    }
    let border_x = cb_x + m.margin[3];
    let mut out = layout_box(ctx, bx, &m, content_w, cb.height, border_x, is_root, floats);
    apply_relative(&mut out.frag, s, cb);
    out
}

/// Auto margins (CSS 2 §10.3.3). An over-constrained right-to-left box gives up its
/// left margin; a left-to-right one its right margin, which never moves the box.
fn resolve_auto_margins(m: &mut BoxMetrics, cb_width: f32, border_w: f32, rtl: bool) {
    let remaining = cb_width - border_w - m.margin[1] - m.margin[3];
    match (m.auto_left, m.auto_right) {
        (true, true) => {
            let half = (remaining / 2.0).max(0.0);
            m.margin[3] = half;
            m.margin[1] = half;
        }
        (true, false) => m.margin[3] += remaining.max(0.0),
        (false, true) => m.margin[1] += remaining,
        (false, false) if rtl => m.margin[3] += remaining,
        (false, false) => {}
    }
}

pub(crate) fn apply_relative(frag: &mut Fragment, s: &ComputedStyle, cb: Containing) {
    if !matches!(s.position, Position::Relative | Position::Sticky) {
        return;
    }
    let dx = match (
        s.inset.left.resolve(cb.width),
        s.inset.right.resolve(cb.width),
    ) {
        (Some(l), _) => l,
        (None, Some(r)) => -r,
        _ => 0.0,
    };
    let v = |l: &Length| match (l, cb.height) {
        (Length::Px(v), _) => Some(*v),
        (l, Some(h)) if l.has_percent() => l.resolve(h),
        _ => None,
    };
    let dy = match (v(&s.inset.top), v(&s.inset.bottom)) {
        (Some(t), _) => t,
        (None, Some(b)) => -b,
        _ => 0.0,
    };
    frag.translate(dx, dy);
}

/// Lay out a non-replaced box with a known content width. The border box starts at
/// (`border_x`, 0).
#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_box<'a>(
    ctx: &Ctx<'a>,
    bx: &'a BoxNode,
    m: &BoxMetrics,
    content_w: f32,
    cb_height: Option<f32>,
    border_x: f32,
    is_root: bool,
    floats: FloatCx,
) -> BlockOut {
    layout_box_sized(
        ctx, bx, m, content_w, cb_height, border_x, is_root, floats, None,
    )
}

/// [`layout_box`] with a used content height decided by the parent's formatting context
/// (a flexed or stretched flex item, a stretched grid item); `None` uses `height`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_box_sized<'a>(
    ctx: &Ctx<'a>,
    bx: &'a BoxNode,
    m: &BoxMetrics,
    content_w: f32,
    cb_height: Option<f32>,
    border_x: f32,
    is_root: bool,
    floats: FloatCx,
    used_height: Option<f32>,
) -> BlockOut {
    let s = &bx.style;
    let bfc = establishes_bfc(bx, is_root);
    let mut own_floats = Floats::default();
    let (floats, oy): (&mut Floats, f32) = match floats {
        Some((f, oy)) if !bfc => (f, oy),
        _ => (&mut own_floats, 0.0),
    };
    let bp_v = m.bp_v();
    let spec_h = used_height.or_else(|| specified_height(s, cb_height, bp_v));
    let content_x = border_x + m.border[3] + m.padding[3];
    let content_y = m.border[0] + m.padding[0];
    let child_cb_h = spec_h.or(if is_root { cb_height } else { None });

    let collapse_top = !bfc && m.border[0] == 0.0 && m.padding[0] == 0.0;
    let bottom_open = !bfc
        && m.border[2] == 0.0
        && m.padding[2] == 0.0
        && s.min_height.resolve(0.0).unwrap_or(0.0) <= 0.0;
    let collapse_bottom = bottom_open && spec_h.is_none();

    let inline_ctx = !bx.children.is_empty()
        && bx
            .children
            .iter()
            .all(|c| c.is_inline_level() || c.is_out_of_flow() || c.is_float());

    let mut children = Vec::new();
    let mut top_escape = Margin::default();
    let mut bottom_escape = Margin::default();
    let mut first_baseline = None;
    let mut last_baseline = None;
    let mut empty = true;
    let is_table = table::is_table(s.display);
    let mut content_h = if is_table {
        let out = table::layout(ctx, bx, content_x, content_y, content_w, spec_h);
        children = out.fragments;
        first_baseline = out.first_baseline;
        last_baseline = out.first_baseline;
        empty = false;
        out.height
    } else if s.display.is_flex() || s.display.is_grid() {
        let space = Space {
            x: content_x,
            y: content_y,
            width: content_w,
            height: spec_h.map(|h| clamp_height(s, h, cb_height, bp_v)),
            cb_height,
            bp_v,
        };
        let out = if s.display.is_flex() {
            flex::layout(ctx, bx, &space)
        } else {
            grid::layout(ctx, bx, &space)
        };
        children = out.fragments;
        first_baseline = out.first_baseline;
        last_baseline = out.last_baseline;
        empty = children.is_empty() && out.height == 0.0;
        out.height
    } else if bx.children.is_empty() {
        0.0
    } else if inline_ctx {
        let out = inline::layout_inline(ctx, bx, content_x, content_y, content_w, floats, oy);
        children = out.fragments;
        first_baseline = out.first_baseline;
        last_baseline = out.last_baseline;
        empty = out.height == 0.0;
        out.height
    } else {
        let out = layout_block_children(
            ctx,
            &bx.children,
            content_x,
            content_y,
            Containing {
                width: content_w,
                height: child_cb_h,
                rtl: s.rtl,
            },
            s.text_align.block_alignment(),
            collapse_top,
            collapse_bottom,
            floats,
            oy,
        );
        children = out.fragments;
        top_escape = out.top_escape;
        bottom_escape = out.bottom_escape;
        first_baseline = out.first_baseline;
        last_baseline = out.last_baseline;
        empty = out.empty;
        out.height
    };
    if bfc {
        // A formatting context root grows to contain its floats (CSS 2 §10.6.7).
        content_h = content_h.max(own_floats.bottom() - content_y);
    }

    // A table's height is a minimum: rows never shrink below their content.
    let used_h = match spec_h {
        Some(h) if is_table => h.max(content_h),
        Some(h) => h,
        None => content_h,
    };
    let h = clamp_height(s, used_h, cb_height, bp_v);
    let border_h = h + bp_v;

    let collapse_through =
        empty && border_h == 0.0 && collapse_top && bottom_open && spec_h.is_none_or(|h| h == 0.0);
    let mut top_margin = Margin::from(m.margin[0]);
    let mut bottom_margin = Margin::from(m.margin[2]);
    if collapse_top {
        top_margin.merge(top_escape);
    }
    if collapse_bottom {
        bottom_margin.merge(bottom_escape);
    }

    let mut frag = Fragment {
        node: bx.node,
        style: bx.style.clone(),
        rect: Rect::new(border_x, 0.0, content_w + m.bp_h(), border_h),
        kind: FragmentKind::Box(BoxEdges::BOTH),
        children,
    };
    if let Some(marker) = &bx.marker {
        let baseline = first_baseline.unwrap_or_else(|| {
            let fm = axiom_text::font_metrics(&font_spec(s));
            content_y + fm.ascent
        });
        // The marker sits outside the principal box (CSS 2 §12.5.1).
        frag.children
            .push(marker_fragment(bx, marker, border_x, baseline));
    }
    BlockOut {
        frag,
        top_margin,
        bottom_margin,
        collapse_through,
        first_baseline,
        last_baseline,
        margin: m.margin,
        auto_height: content_h + bp_v,
    }
}

pub(crate) fn font_spec(s: &ComputedStyle) -> FontSpec {
    FontSpec {
        families: Arc::clone(&s.font_family),
        weight: s.font_weight,
        italic: s.italic,
        size: s.font_size,
    }
}

fn marker_fragment(bx: &BoxNode, marker: &Marker, border_x: f32, baseline: f32) -> Fragment {
    let s = &bx.style;
    let fs = s.font_size;
    match marker {
        Marker::Shape(shape) => {
            let size = (fs * 0.35).max(3.0);
            let x = border_x - fs * 0.55 - size;
            let y = baseline - fs * 0.3 - size / 2.0;
            Fragment {
                node: bx.node,
                style: s.clone(),
                rect: Rect::new(x, y, size, size),
                kind: FragmentKind::Shape(*shape, s.color),
                children: Vec::new(),
            }
        }
        Marker::Text(text) => {
            let spec = font_spec(s);
            let run = axiom_text::shape(text, &spec, false);
            let fm = axiom_text::font_metrics(&spec);
            let x = border_x - run.width;
            Fragment {
                node: bx.node,
                style: s.clone(),
                rect: Rect::new(x, baseline - fm.ascent, run.width, fm.ascent + fm.descent),
                kind: FragmentKind::Text(TextFragment {
                    text: text.clone(),
                    run,
                    baseline,
                    font_size: fs,
                    color: s.color,
                    decorations: 0,
                    decoration_color: s.color,
                }),
                children: Vec::new(),
            }
        }
    }
}

/// A flex or grid container's content box: `x` is absolute, `y` relative to the
/// container's border-box top (like the fragments inside it).
pub(crate) struct Space {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    /// Definite content height, if any.
    pub height: Option<f32>,
    /// The container's own containing block height (for its `min-height`/`max-height`).
    pub cb_height: Option<f32>,
    pub bp_v: f32,
}

/// Fragments a flex or grid container lays out, with the content height they need.
pub(crate) struct ContainerOut {
    pub fragments: Vec<Fragment>,
    pub height: f32,
    pub first_baseline: Option<f32>,
    pub last_baseline: Option<f32>,
}

struct ChildrenOut {
    fragments: Vec<Fragment>,
    height: f32,
    top_escape: Margin,
    bottom_escape: Margin,
    first_baseline: Option<f32>,
    last_baseline: Option<f32>,
    empty: bool,
}

/// Lays out a float and places it among `floats`, no higher than `y_min` (formatting
/// context coordinates). The fragment is positioned for a parent whose border-box top is
/// at `oy`.
pub(crate) fn layout_float<'a>(
    ctx: &Ctx<'a>,
    bx: &'a BoxNode,
    cb: Containing,
    content_x: f32,
    floats: &mut Floats,
    y_min: f32,
    oy: f32,
) -> Fragment {
    let out = layout_block_level(ctx, bx, cb, 0.0, false, None);
    let [mt, mr, mb, ml] = out.margin;
    let w = out.frag.rect.width + ml + mr;
    let h = out.frag.rect.height + mt + mb;
    let y_min = floats
        .clearance(bx.style.clear)
        .map_or(y_min, |c| y_min.max(c));
    let left = bx.style.float == Float::Left;
    let (x, y) = floats.place(left, w, h, y_min, content_x, content_x + cb.width);
    let mut frag = out.frag;
    frag.translate(x, y - oy + mt);
    frag
}

#[allow(clippy::too_many_arguments)]
fn layout_block_children<'a>(
    ctx: &Ctx<'a>,
    children: &'a [BoxNode],
    content_x: f32,
    content_y: f32,
    cb: Containing,
    align: Option<f32>,
    collapse_top: bool,
    collapse_bottom: bool,
    floats: &mut Floats,
    oy: f32,
) -> ChildrenOut {
    let mut fragments = Vec::new();
    let mut cursor = 0.0f32;
    let mut margin = Margin::default();
    let mut at_start = true;
    let mut top_escape = Margin::default();
    let mut first_baseline = None;
    let mut last_baseline = None;
    // Formatting context y of a local y.
    let bfc_y = |y: f32| oy + content_y + y;
    for child in children {
        let pending = if at_start && collapse_top {
            cursor
        } else {
            cursor + margin.value()
        };
        if child.is_out_of_flow() {
            // The static position is at the start edge: the right one in rtl.
            let x = if child.style.rtl {
                content_x + cb.width
            } else {
                content_x
            };
            fragments.push(placeholder(ctx, child, x, content_y + pending));
            continue;
        }
        if child.is_float() {
            let frag = layout_float(ctx, child, cb, content_x, floats, bfc_y(pending), oy);
            fragments.push(frag);
            continue;
        }
        // Where the border edge lands if the child's own top margin collapses in.
        let mut est = margin;
        est.add(child.style.margin.top.resolve(cb.width).unwrap_or(0.0));
        let est_y = if at_start && collapse_top {
            cursor
        } else {
            cursor + est.value()
        };
        // Clearance puts the border edge below the cleared floats (CSS 2 §9.5.2).
        let mut forced_y = floats
            .clearance(child.style.clear)
            .map(|b| b - oy - content_y)
            .filter(|&b| child.style.clear != Clear::None && b > est_y);
        let child_bfc = establishes_bfc(child, false);
        let mut attempts = 0;
        let mut out = loop {
            let y_try = forced_y.unwrap_or(est_y);
            if !child_bfc || floats.is_empty() {
                let fl = (!child_bfc).then_some((&mut *floats, bfc_y(y_try)));
                break layout_block_level(ctx, child, cb, content_x, false, fl);
            }
            // A formatting context root may not overlap floats: it sits beside them,
            // or moves down until it fits.
            let (l, r) = floats.band(bfc_y(y_try), 0.0, content_x, content_x + cb.width);
            let narrowed = l > content_x || r < content_x + cb.width;
            let sub = Containing {
                width: (r - l).max(0.0),
                height: cb.height,
                rtl: cb.rtl,
            };
            let out =
                layout_block_level(ctx, child, if narrowed { sub } else { cb }, l, false, None);
            let outer_w = out.frag.rect.width + out.margin[1].max(0.0) + out.margin[3].max(0.0);
            let outer_h = out.frag.rect.height + out.margin[0] + out.margin[2];
            let (l2, r2) = floats.band(bfc_y(y_try), outer_h, content_x, content_x + cb.width);
            let blocked = (l2 > content_x || r2 < content_x + cb.width) && r2 - l2 < outer_w - 0.01;
            attempts += 1;
            match floats.next_bottom(bfc_y(y_try)) {
                Some(b) if blocked && attempts < 64 => forced_y = Some(b - oy - content_y),
                _ => break out,
            }
        };
        let [_, mr, _, ml] = out.margin;
        let free = cb.width - (out.frag.rect.width + ml + mr);
        let fixed_margins =
            !child.style.margin.left.is_auto() && !child.style.margin.right.is_auto();
        if let Some(t) = align.filter(|_| free > 0.0 && fixed_margins) {
            out.frag.translate(free * t, 0.0);
        }
        let mut m = margin;
        m.merge(out.top_margin);
        let mut frag = out.frag;
        if let Some(y) = forced_y {
            // Clearance separates the margins above from the child's.
            if at_start && collapse_top {
                top_escape = margin;
            }
            frag.translate(0.0, content_y + y);
            if first_baseline.is_none() {
                first_baseline = out.first_baseline.map(|b| content_y + y + b);
            }
            if let Some(b) = out.last_baseline {
                last_baseline = Some(content_y + y + b);
            }
            cursor = y + frag.rect.height;
            fragments.push(frag);
            margin = out.bottom_margin;
            at_start = false;
            continue;
        }
        if out.collapse_through {
            m.merge(out.bottom_margin);
            let y = if at_start && collapse_top {
                cursor
            } else {
                cursor + m.value()
            };
            frag.translate(0.0, content_y + y);
            fragments.push(frag);
            margin = m;
            continue;
        }
        let y = if at_start && collapse_top {
            top_escape = m;
            cursor
        } else {
            cursor + m.value()
        };
        frag.translate(0.0, content_y + y);
        if first_baseline.is_none() {
            first_baseline = out.first_baseline.map(|b| content_y + y + b);
        }
        if let Some(b) = out.last_baseline {
            last_baseline = Some(content_y + y + b);
        }
        cursor = y + frag.rect.height;
        fragments.push(frag);
        margin = out.bottom_margin;
        at_start = false;
    }
    let empty = at_start;
    if at_start && collapse_top {
        top_escape = margin;
        margin = Margin::default();
    }
    let (height, bottom_escape) = if collapse_bottom && !at_start {
        (cursor, margin)
    } else {
        ((cursor + margin.value()).max(cursor), Margin::default())
    };
    ChildrenOut {
        fragments,
        height,
        top_escape,
        bottom_escape,
        first_baseline,
        last_baseline,
        empty,
    }
}

/// A zero-size stand-in at the static position of an absolutely positioned box.
pub(crate) fn placeholder<'a>(ctx: &Ctx<'a>, bx: &'a BoxNode, x: f32, y: f32) -> Fragment {
    let mut boxes = ctx.abs_boxes.borrow_mut();
    let index = boxes.len();
    boxes.push(bx);
    Fragment {
        node: bx.node,
        style: bx.style.clone(),
        rect: Rect::new(x, y, 0.0, 0.0),
        kind: FragmentKind::AbsPlaceholder(index),
        children: Vec::new(),
    }
}

/// Replace placeholders with laid-out absolutely positioned boxes. `cb` is the padding
/// box of the nearest positioned ancestor.
pub(crate) fn position_absolutes<'a>(ctx: &Ctx<'a>, frag: &mut Fragment, cb: Rect, viewport: Rect) {
    for child in &mut frag.children {
        if let FragmentKind::AbsPlaceholder(i) = child.kind {
            let bx = ctx.abs_boxes.borrow()[i];
            let child_cb = if bx.style.position == Position::Fixed {
                viewport
            } else {
                cb
            };
            *child = layout_absolute(ctx, bx, child_cb, (child.rect.x, child.rect.y));
            let pr = child.padding_rect();
            position_absolutes(ctx, child, pr, viewport);
        } else {
            let next = if child.style.is_positioned() && matches!(child.kind, FragmentKind::Box(_))
            {
                child.padding_rect()
            } else {
                cb
            };
            position_absolutes(ctx, child, next, viewport);
        }
    }
}

fn layout_absolute<'a>(
    ctx: &Ctx<'a>,
    bx: &'a BoxNode,
    cb: Rect,
    static_pos: (f32, f32),
) -> Fragment {
    let s = &bx.style;
    let mut m = BoxMetrics::new(s, cb.width);
    let bp_h = m.bp_h();
    let bp_v = m.bp_v();
    let h_len = |l: &Length| l.resolve(cb.width);
    let v_len = |l: &Length| l.resolve(cb.height);
    let (left, right) = (h_len(&s.inset.left), h_len(&s.inset.right));
    let (top, bottom) = (v_len(&s.inset.top), v_len(&s.inset.bottom));
    let containing = Containing {
        width: cb.width,
        height: Some(cb.height),
        rtl: s.rtl,
    };

    if let BoxKind::Replaced(r) = &bx.kind {
        let (w, h) = replaced_size(bx, r, containing);
        let x = match (left, right) {
            (Some(l), _) => cb.x + l + m.margin[3],
            (None, Some(rt)) => cb.right() - rt - m.margin[1] - (w + bp_h),
            _ if s.rtl => static_pos.0 - m.margin[1] - (w + bp_h),
            _ => static_pos.0 + m.margin[3],
        };
        let y = match (top, bottom) {
            (Some(t), _) => cb.y + t + m.margin[0],
            (None, Some(b)) => cb.bottom() - b - m.margin[2] - (h + bp_v),
            _ => static_pos.1 + m.margin[0],
        };
        let mut out = layout_replaced(bx, r, &m, w, h, 0.0);
        out.frag.translate(x, y);
        return out.frag;
    }

    let margins_h = m.margin[1] + m.margin[3];
    let width = match specified_width(s, cb.width, bp_h) {
        Some(w) => w,
        None => match (left, right) {
            (Some(l), Some(r)) => (cb.width - l - r - margins_h - bp_h).max(0.0),
            _ => {
                let avail = match (left, right) {
                    (Some(l), None) => cb.width - l,
                    (None, Some(r)) => cb.width - r,
                    _ if s.rtl => static_pos.0 - cb.x,
                    _ => cb.right() - static_pos.0,
                } - margins_h
                    - bp_h;
                shrink_to_fit(ctx, bx, avail.max(0.0))
            }
        },
    };
    let width = clamp_width(s, width, cb.width, bp_h);
    if let (Some(l), Some(r)) = (left, right) {
        if m.auto_left || m.auto_right {
            let remaining = cb.width - l - r - width - bp_h - margins_h;
            if m.auto_left && m.auto_right {
                m.margin[3] = (remaining / 2.0).max(0.0);
                m.margin[1] = m.margin[3];
            } else if m.auto_left {
                m.margin[3] = remaining;
            }
        }
    }
    let x = match (left, right) {
        (Some(l), _) => cb.x + l + m.margin[3],
        (None, Some(r)) => cb.right() - r - m.margin[1] - (width + bp_h),
        _ if s.rtl => static_pos.0 - m.margin[1] - (width + bp_h),
        _ => static_pos.0 + m.margin[3],
    };
    let spec_h = specified_height(s, Some(cb.height), bp_v);
    let stretch_h = match (top, bottom, spec_h) {
        (Some(t), Some(b), None) => {
            Some((cb.height - t - b - m.margin[0] - m.margin[2] - bp_v).max(0.0))
        }
        _ => None,
    };
    let mut out = layout_box(ctx, bx, &m, width, Some(cb.height), 0.0, false, None);
    if let Some(h) = stretch_h {
        let h = clamp_height(s, h, Some(cb.height), bp_v);
        out.frag.rect.height = out.frag.rect.height.max(h + bp_v);
    }
    let border_h = out.frag.rect.height;
    let y = match (top, bottom) {
        (Some(t), _) => cb.y + t + m.margin[0],
        (None, Some(b)) => cb.bottom() - b - m.margin[2] - border_h,
        _ => static_pos.1 + m.margin[0],
    };
    out.frag.translate(x, y);
    out.frag
}

/// Lay out a flex or grid item at a used content width (and, if the container decided
/// it, content height), with its border box at (0, 0).
pub(crate) fn layout_item<'a>(
    ctx: &Ctx<'a>,
    bx: &'a BoxNode,
    m: &BoxMetrics,
    content_w: f32,
    used_h: Option<f32>,
    cb: Containing,
) -> BlockOut {
    if let BoxKind::Replaced(r) = &bx.kind {
        let h = used_h.unwrap_or_else(|| replaced_height_at(bx, r, content_w, cb));
        return layout_replaced(bx, r, m, content_w, h, 0.0);
    }
    layout_box_sized(ctx, bx, m, content_w, cb.height, 0.0, false, None, used_h)
}

/// Content height of a replaced box whose used content width is `w`: the specified
/// height, else `w` through the box's aspect ratio.
pub(crate) fn replaced_height_at(bx: &BoxNode, r: &Replaced, w: f32, cb: Containing) -> f32 {
    let s = &bx.style;
    let (rw, rh) = replaced_size(bx, r, cb);
    let bp_v = BoxMetrics::new(s, cb.width).bp_v();
    if specified_height(s, cb.height, bp_v).is_some() || rw <= 0.0 {
        return rh;
    }
    clamp_height(s, w * rh / rw, cb.height, bp_v)
}

/// Shrink-to-fit width (CSS 2 §10.3.5) of the content box.
pub(crate) fn shrink_to_fit<'a>(ctx: &Ctx<'a>, bx: &'a BoxNode, available: f32) -> f32 {
    let (min, max) = content_intrinsic(ctx, bx);
    max.min(available.max(min)).max(0.0)
}

/// Min- and max-content widths of the content box.
pub(crate) fn content_intrinsic<'a>(ctx: &Ctx<'a>, bx: &'a BoxNode) -> (f32, f32) {
    *bx.intrinsic.get_or_init(|| match &bx.kind {
        BoxKind::Replaced(r) => {
            let (w, _) = replaced_size(
                bx,
                r,
                Containing {
                    width: 0.0,
                    height: None,
                    rtl: false,
                },
            );
            (w, w)
        }
        _ if table::is_table(bx.style.display) => table::intrinsic(ctx, bx),
        _ if bx.style.display.is_flex() => flex::intrinsic(ctx, bx),
        _ if bx.style.display.is_grid() => grid::intrinsic(ctx, bx),
        _ if bx.children.is_empty() => (0.0, 0.0),
        _ if bx
            .children
            .iter()
            .all(|c| c.is_inline_level() || c.is_out_of_flow() || c.is_float()) =>
        {
            inline::intrinsic(ctx, bx)
        }
        _ => {
            // Consecutive floats sit side by side at the max-content width.
            let (mut mn, mut mx, mut floats) = (0.0f32, 0.0f32, 0.0f32);
            for c in bx.children.iter().filter(|c| !c.is_out_of_flow()) {
                let (a, b) = outer_intrinsic(ctx, c);
                mn = mn.max(a);
                if c.is_float() {
                    floats += b;
                    mx = mx.max(floats);
                } else {
                    floats = 0.0;
                    mx = mx.max(b);
                }
            }
            (mn, mx)
        }
    })
}

/// Margin-box min/max-content contribution of a box.
pub(crate) fn outer_intrinsic<'a>(ctx: &Ctx<'a>, bx: &'a BoxNode) -> (f32, f32) {
    let s = &bx.style;
    let px = |l: &Length| match l {
        Length::Px(v) => *v,
        _ => 0.0,
    };
    let bp =
        px(&s.padding.left) + px(&s.padding.right) + s.border_widths()[1] + s.border_widths()[3];
    let margins = px(&s.margin.left) + px(&s.margin.right);
    let (mut mn, mut mx) = match (&s.width, &bx.kind) {
        (Length::Px(w), k) if !matches!(k, BoxKind::Replaced(_)) => {
            let c = if s.border_box_sizing {
                (w - bp).max(0.0)
            } else {
                *w
            };
            (c, c)
        }
        _ => content_intrinsic(ctx, bx),
    };
    let adj = |v: f32| if s.border_box_sizing { v - bp } else { v };
    if let Length::Px(max) = s.max_width {
        mn = mn.min(adj(max));
        mx = mx.min(adj(max));
    }
    if let Length::Px(min) = s.min_width {
        mn = mn.max(adj(min));
        mx = mx.max(adj(min));
    }
    (mn.max(0.0) + bp + margins, mx.max(0.0) + bp + margins)
}

/// Used content size of a replaced element (CSS 2 §10.3.2 / §10.6.2).
pub(crate) fn replaced_size(bx: &BoxNode, r: &Replaced, cb: Containing) -> (f32, f32) {
    let s = &bx.style;
    let m = BoxMetrics::new(s, cb.width);
    let (bp_h, bp_v) = (m.bp_h(), m.bp_v());
    let (nat_w, nat_h, nat_ratio) = match r {
        Replaced::Image(img) => {
            let (w, h) = (img.width as f32, img.height as f32);
            (w, h, (h > 0.0).then(|| w / h))
        }
        Replaced::Svg {
            image,
            width,
            height,
            ratio,
        } => {
            let stretch = cb.width - bp_h - m.margin[1] - m.margin[3];
            let (w, h) = match (*width, *height, *ratio) {
                (Some(w), Some(h), _) => (w, h),
                (Some(w), None, r) => (w, r.map_or(150.0, |r| w / r)),
                (None, Some(h), r) => (r.map_or(300.0, |r| h * r), h),
                (None, None, Some(r)) if stretch > 0.0 => (stretch, stretch / r),
                (None, None, _) => (image.width as f32, image.height as f32),
            };
            (w, h, *ratio)
        }
        Replaced::Empty { width, height } => (
            *width,
            *height,
            (*width > 0.0 && *height > 0.0).then(|| width / height),
        ),
        Replaced::Widget(w) => {
            let (w, h) = widget_size(s, w);
            (w, h, None)
        }
    };
    let ratio = s.aspect_ratio.or(nat_ratio);
    let spec_w = if cb.width > 0.0 || !s.width.has_percent() {
        specified_width(s, cb.width, bp_h)
    } else {
        None
    };
    let spec_h = specified_height(s, cb.height, bp_v);
    let (mut w, mut h) = match (spec_w, spec_h) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, ratio.map(|r| w / r).unwrap_or(nat_h)),
        (None, Some(h)) => (ratio.map(|r| h * r).unwrap_or(nat_w), h),
        (None, None) => (nat_w, nat_h),
    };
    if cb.width > 0.0 || !s.max_width.has_percent() {
        let clamped = clamp_width(s, w, cb.width, bp_h);
        if clamped != w && spec_h.is_none() {
            if let Some(r) = ratio {
                h = clamped / r;
            }
        }
        w = clamped;
    }
    let clamped_h = clamp_height(s, h, cb.height, bp_v);
    if clamped_h != h && spec_w.is_none() {
        if let Some(r) = ratio {
            w = clamped_h * r;
        }
    }
    (w.max(0.0), clamped_h)
}

fn widget_font(s: &ComputedStyle) -> (FontSpec, f32, f32) {
    let spec = font_spec(s);
    let fm = axiom_text::font_metrics(&spec);
    let line = s.line_height_px(fm.normal_line_height() / s.font_size.max(1.0));
    (spec, line, fm.ascent + fm.descent)
}

fn widget_size(s: &ComputedStyle, w: &Widget) -> (f32, f32) {
    let (spec, line, _) = widget_font(s);
    let char_w = s.font_size * 0.55;
    match w {
        Widget::TextField { size, .. } => (*size as f32 * char_w, line),
        Widget::Button { label } => (axiom_text::measure(label, &spec), line),
        Widget::Check { .. } => (13.0, 13.0),
        Widget::Select { widest, .. } => (axiom_text::measure(widest, &spec) + 20.0, line),
        Widget::TextArea { cols, rows, .. } => (*cols as f32 * char_w, *rows as f32 * line),
        Widget::Range => (129.0, 16.0),
        Widget::Color => (44.0, 23.0),
    }
}

pub(crate) fn layout_replaced(
    bx: &BoxNode,
    r: &Replaced,
    m: &BoxMetrics,
    w: f32,
    h: f32,
    border_x: f32,
) -> BlockOut {
    let s = &bx.style;
    let content = Rect::new(
        border_x + m.border[3] + m.padding[3],
        m.border[0] + m.padding[0],
        w,
        h,
    );
    let mut children = Vec::new();
    let mut baseline = None;
    let mut style = bx.style.clone();
    match r {
        Replaced::Image(img) | Replaced::Svg { image: img, .. } => children.push(Fragment {
            node: bx.node,
            style: bx.style.clone(),
            rect: content,
            kind: FragmentKind::Image(img.clone()),
            children: Vec::new(),
        }),
        Replaced::Empty { .. } => {}
        Replaced::Widget(widget) => {
            let mut clipped = (**s).clone();
            clipped.overflow_x = Overflow::Clip;
            clipped.overflow_y = Overflow::Clip;
            style = Arc::new(clipped);
            baseline = widget_children(bx, widget, content, &mut children);
        }
    }
    let frag = Fragment {
        node: bx.node,
        style,
        rect: Rect::new(border_x, 0.0, w + m.bp_h(), h + m.bp_v()),
        kind: FragmentKind::Box(BoxEdges::BOTH),
        children,
    };
    let bottom = frag.rect.height;
    BlockOut {
        frag,
        top_margin: Margin::from(m.margin[0]),
        bottom_margin: Margin::from(m.margin[2]),
        collapse_through: false,
        first_baseline: baseline.or(Some(bottom)),
        last_baseline: baseline.or(Some(bottom)),
        margin: m.margin,
        auto_height: bottom,
    }
}

/// Text and glyph fragments inside a form control; returns the text baseline.
fn widget_children(
    bx: &BoxNode,
    widget: &Widget,
    content: Rect,
    out: &mut Vec<Fragment>,
) -> Option<f32> {
    let s = &bx.style;
    let (spec, line, text_h) = widget_font(s);
    let fm = axiom_text::font_metrics(&spec);
    let text = |t: &str, x_align: f32, top: f32, color: Color, out: &mut Vec<Fragment>| -> f32 {
        let run = axiom_text::shape(t, &spec, false);
        let x = content.x + (content.width - run.width) * x_align;
        let baseline = top + (line - text_h) / 2.0 + fm.ascent;
        out.push(Fragment {
            node: bx.node,
            style: bx.style.clone(),
            rect: Rect::new(x, baseline - fm.ascent, run.width, text_h),
            kind: FragmentKind::Text(TextFragment {
                text: t.to_string(),
                run,
                baseline,
                font_size: s.font_size,
                color,
                decorations: 0,
                decoration_color: color,
            }),
            children: Vec::new(),
        });
        baseline
    };
    let middle = content.y + (content.height - line) / 2.0;
    match widget {
        Widget::TextField {
            text: t,
            placeholder,
            ..
        } => {
            let color = if *placeholder {
                Color::rgb(0x75, 0x75, 0x75)
            } else {
                s.color
            };
            Some(text(t, 0.0, middle, color, out))
        }
        Widget::Button { label } => Some(text(label, 0.5, middle, s.color, out)),
        Widget::Select { label, .. } => {
            let b = text(label, 0.0, middle, s.color, out);
            let size = (s.font_size * 0.6).max(6.0);
            out.push(Fragment {
                node: bx.node,
                style: bx.style.clone(),
                rect: Rect::new(
                    content.right() - size - 2.0,
                    content.y + (content.height - size) / 2.0,
                    size,
                    size,
                ),
                kind: FragmentKind::Shape(Shape::DropdownArrow, s.color),
                children: Vec::new(),
            });
            Some(b)
        }
        Widget::TextArea { text: t, .. } => {
            let mut first = None;
            for (i, l) in t.split('\n').enumerate() {
                let b = text(l, 0.0, content.y + i as f32 * line, s.color, out);
                first.get_or_insert(b);
            }
            first
        }
        Widget::Check { checked, radio } => {
            if *checked {
                let inset = 3.0;
                out.push(Fragment {
                    node: bx.node,
                    style: bx.style.clone(),
                    rect: Rect::new(
                        content.x + inset - 2.0,
                        content.y + inset - 2.0,
                        content.width - 2.0 * inset + 4.0,
                        content.height - 2.0 * inset + 4.0,
                    ),
                    kind: FragmentKind::Shape(
                        if *radio {
                            Shape::RadioDot
                        } else {
                            Shape::Check
                        },
                        Color::rgb(0x00, 0x75, 0xff),
                    ),
                    children: Vec::new(),
                });
            }
            None
        }
        Widget::Range => {
            out.push(Fragment {
                node: bx.node,
                style: bx.style.clone(),
                rect: Rect::new(
                    content.x,
                    content.y + content.height / 2.0 - 2.0,
                    content.width,
                    4.0,
                ),
                kind: FragmentKind::Shape(Shape::Square, Color::rgb(0x00, 0x75, 0xff)),
                children: Vec::new(),
            });
            None
        }
        Widget::Color => {
            out.push(Fragment {
                node: bx.node,
                style: bx.style.clone(),
                rect: Rect::new(
                    content.x + 4.0,
                    content.y + 4.0,
                    content.width - 8.0,
                    content.height - 8.0,
                ),
                kind: FragmentKind::Shape(Shape::Square, Color::BLACK),
                children: Vec::new(),
            });
            None
        }
    }
}
