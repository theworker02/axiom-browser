//! Inline formatting context (CSS Inline 3 / CSS Text 3).
//!
//! The container's inline-level descendants are flattened into one paragraph string
//! (white space processed across element boundaries, U+FFFC for atomic inlines), which
//! is split at UAX #14 break opportunities into unbreakable segments. Segments are
//! shaped per style run and bidi level, filled greedily into lines, reordered per line
//! (UAX #9 rule L2), aligned, and placed on line boxes built from font metrics.

use std::collections::HashMap;
use std::sync::Arc;

use axiom_dom::NodeId;
use axiom_style::{ComputedStyle, Float, SpaceCollapse, TextAlign, TextTransform, VerticalAlign};
use axiom_text::{Glyph, ShapedRun};

use crate::block::{self, font_spec, Containing};
use crate::float::Floats;
use crate::tree::{BoxKind, BoxNode, Replaced};
use crate::{BoxEdges, Ctx, Fragment, FragmentKind, Rect, TextFragment};

pub(crate) struct InlineOut {
    pub fragments: Vec<Fragment>,
    pub height: f32,
    pub first_baseline: Option<f32>,
    pub last_baseline: Option<f32>,
}

struct InlineBox<'a> {
    bx: &'a BoxNode,
    parent: Option<usize>,
    /// Margin + border + padding on the start / end side.
    start_edge: f32,
    end_edge: f32,
    margin_left: f32,
    /// Baseline raise relative to the container's baseline.
    shift: f32,
}

#[derive(Clone, Copy)]
enum ItemKind<'a> {
    Text,
    Open(usize),
    Close(usize),
    Atomic(&'a BoxNode),
    Break,
    Wbr,
    Abs(&'a BoxNode),
    /// Takes no inline space; placed beside the line it occurs on (or below it).
    Float(&'a BoxNode),
}

struct Item<'a> {
    kind: ItemKind<'a>,
    style: Arc<ComputedStyle>,
    /// Element for hit testing (text: the enclosing element).
    node: Option<NodeId>,
    start: usize,
    end: usize,
    inline_box: Option<usize>,
    shift: f32,
    nowrap: bool,
}

struct Piece {
    item: usize,
    start: usize,
    end: usize,
    run: Option<Arc<ShapedRun>>,
    width: f32,
    min_width: f32,
    level: u8,
    /// Atomic layout index.
    atomic: Option<usize>,
}

#[derive(Default)]
struct Segment {
    pieces: std::ops::Range<usize>,
    width: f32,
    min_width: f32,
    trailing: f32,
    forced: bool,
}

struct AtomicLayout {
    frag: Fragment,
    /// Margin box.
    width: f32,
    height: f32,
    /// Baseline from the margin-box top.
    baseline: f32,
    margin_top: f32,
}

struct Prepared<'a> {
    para: String,
    items: Vec<Item<'a>>,
    boxes: Vec<InlineBox<'a>>,
    pieces: Vec<Piece>,
    segments: Vec<Segment>,
    atomics: Vec<AtomicLayout>,
    rtl: bool,
    bidi: bool,
}

struct Flattener<'a> {
    width: f32,
    para: String,
    items: Vec<Item<'a>>,
    boxes: Vec<InlineBox<'a>>,
    prev_space: bool,
}

impl<'a> Flattener<'a> {
    fn run(
        &mut self,
        children: &'a [BoxNode],
        parent_style: &Arc<ComputedStyle>,
        enclosing: Option<usize>,
        shift: f32,
        elem: Option<NodeId>,
    ) {
        for c in children {
            let pos = self.para.len();
            if c.is_out_of_flow() {
                self.push(
                    ItemKind::Abs(c),
                    c.style.clone(),
                    c.node,
                    pos,
                    pos,
                    enclosing,
                    shift,
                    parent_style.nowrap,
                );
                continue;
            }
            if c.is_float() {
                self.push(
                    ItemKind::Float(c),
                    c.style.clone(),
                    c.node,
                    pos,
                    pos,
                    enclosing,
                    shift,
                    parent_style.nowrap,
                );
                continue;
            }
            match &c.kind {
                BoxKind::Text(t) => {
                    push_text(&mut self.para, t, &c.style, &mut self.prev_space);
                    let end = self.para.len();
                    if end > pos {
                        let nowrap = c.style.nowrap;
                        self.push(
                            ItemKind::Text,
                            c.style.clone(),
                            elem,
                            pos,
                            end,
                            enclosing,
                            shift,
                            nowrap,
                        );
                    }
                }
                BoxKind::Inline => {
                    let s = &c.style;
                    let m = block::BoxMetrics::new(s, self.width);
                    let idx = self.boxes.len();
                    let box_shift = shift + baseline_shift(s, parent_style);
                    self.boxes.push(InlineBox {
                        bx: c,
                        parent: enclosing,
                        start_edge: m.margin[3] + m.border[3] + m.padding[3],
                        end_edge: m.margin[1] + m.border[1] + m.padding[1],
                        margin_left: m.margin[3],
                        shift: box_shift,
                    });
                    self.push(
                        ItemKind::Open(idx),
                        s.clone(),
                        c.node,
                        pos,
                        pos,
                        Some(idx),
                        box_shift,
                        s.nowrap,
                    );
                    self.run(&c.children, s, Some(idx), box_shift, c.node);
                    let end = self.para.len();
                    self.push(
                        ItemKind::Close(idx),
                        s.clone(),
                        c.node,
                        end,
                        end,
                        Some(idx),
                        box_shift,
                        s.nowrap,
                    );
                }
                BoxKind::Atomic | BoxKind::Replaced(_) | BoxKind::Block => {
                    self.para.push('\u{FFFC}');
                    self.prev_space = false;
                    let end = self.para.len();
                    self.push(
                        ItemKind::Atomic(c),
                        c.style.clone(),
                        c.node,
                        pos,
                        end,
                        enclosing,
                        shift,
                        parent_style.nowrap,
                    );
                }
                BoxKind::LineBreak => {
                    // Collapsible spaces before a forced break are removed.
                    if self.prev_space
                        && self.para.ends_with(' ')
                        && !parent_style.preserves_spaces()
                    {
                        self.para.pop();
                        if let Some(last) = self.items.last_mut() {
                            if matches!(last.kind, ItemKind::Text) && last.end > self.para.len() {
                                last.end = self.para.len();
                            }
                        }
                    }
                    let pos = self.para.len();
                    self.para.push('\n');
                    self.prev_space = true;
                    self.push(
                        ItemKind::Break,
                        c.style.clone(),
                        elem,
                        pos,
                        pos + 1,
                        enclosing,
                        shift,
                        false,
                    );
                }
                BoxKind::WordBreak => {
                    self.push(
                        ItemKind::Wbr,
                        c.style.clone(),
                        elem,
                        pos,
                        pos,
                        enclosing,
                        shift,
                        false,
                    );
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        kind: ItemKind<'a>,
        style: Arc<ComputedStyle>,
        node: Option<NodeId>,
        start: usize,
        end: usize,
        inline_box: Option<usize>,
        shift: f32,
        nowrap: bool,
    ) {
        self.items.push(Item {
            kind,
            style,
            node,
            start,
            end,
            inline_box,
            shift,
            nowrap,
        });
    }
}

/// Raise of an inline box's baseline relative to its parent's.
fn baseline_shift(s: &ComputedStyle, parent: &ComputedStyle) -> f32 {
    match s.vertical_align {
        VerticalAlign::Sub => -parent.font_size * 0.2,
        VerticalAlign::Super => parent.font_size * 0.34,
        VerticalAlign::Length(px) => px,
        _ => 0.0,
    }
}

/// White space processing (CSS Text 3 §4.1) and `text-transform`.
fn push_text(para: &mut String, text: &str, s: &ComputedStyle, prev_space: &mut bool) {
    let transformed;
    let text = match s.text_transform {
        TextTransform::None => text,
        TextTransform::Uppercase => {
            transformed = text.to_uppercase();
            &transformed
        }
        TextTransform::Lowercase => {
            transformed = text.to_lowercase();
            &transformed
        }
        TextTransform::Capitalize => {
            let mut out = String::with_capacity(text.len());
            let mut at_word_start = para.chars().last().is_none_or(|c| c.is_whitespace());
            for ch in text.chars() {
                if at_word_start && ch.is_alphabetic() {
                    out.extend(ch.to_uppercase());
                } else {
                    out.push(ch);
                }
                at_word_start = ch.is_whitespace() || ch == '-';
            }
            transformed = out;
            &transformed
        }
    };
    match s.space_collapse {
        SpaceCollapse::Collapse => {
            for ch in text.chars() {
                if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0c') {
                    if !*prev_space {
                        para.push(' ');
                        *prev_space = true;
                    }
                } else {
                    para.push(ch);
                    *prev_space = false;
                }
            }
        }
        SpaceCollapse::PreserveBreaks => {
            for ch in text.chars() {
                match ch {
                    '\n' => {
                        if *prev_space && para.ends_with(' ') {
                            para.pop();
                        }
                        para.push('\n');
                        *prev_space = true;
                    }
                    ' ' | '\t' | '\r' | '\x0c' => {
                        if !*prev_space {
                            para.push(' ');
                            *prev_space = true;
                        }
                    }
                    _ => {
                        para.push(ch);
                        *prev_space = false;
                    }
                }
            }
        }
        SpaceCollapse::Preserve | SpaceCollapse::BreakSpaces => {
            let mut chars = text.chars().peekable();
            while let Some(ch) = chars.next() {
                match ch {
                    '\r' => {
                        if chars.peek() != Some(&'\n') {
                            para.push('\n');
                        }
                    }
                    '\t' => {
                        let col = para.rsplit('\n').next().map_or(0, |l| l.chars().count());
                        for _ in 0..(8 - col % 8) {
                            para.push(' ');
                        }
                    }
                    _ => para.push(ch),
                }
            }
            *prev_space = false;
        }
    }
}

fn apply_letter_spacing(run: Arc<ShapedRun>, spacing: f32) -> Arc<ShapedRun> {
    if spacing == 0.0 || run.glyphs.is_empty() {
        return run;
    }
    let mut r = (*run).clone();
    for (i, g) in r.glyphs.iter_mut().enumerate() {
        g.x += spacing * i as f32;
    }
    r.width += spacing * r.glyphs.len() as f32;
    Arc::new(r)
}

fn prepare<'a>(ctx: &Ctx<'a>, container: &'a BoxNode, width: f32, measure: bool) -> Prepared<'a> {
    let mut fl = Flattener {
        width,
        para: String::new(),
        items: Vec::new(),
        boxes: Vec::new(),
        prev_space: true,
    };
    fl.run(
        &container.children,
        &container.style,
        None,
        0.0,
        container.node,
    );
    let Flattener {
        para, items, boxes, ..
    } = fl;
    let rtl = container.style.rtl;
    let bidi = rtl || axiom_text::has_rtl(&para);
    let levels = if bidi {
        Some(axiom_text::bidi_levels(&para, Some(rtl)).0)
    } else {
        None
    };

    // Byte → item owner (for text, atomics and breaks).
    let mut owner = vec![u32::MAX; para.len()];
    for (i, it) in items.iter().enumerate() {
        for b in &mut owner[it.start..it.end] {
            *b = i as u32;
        }
    }
    let nowrap_at = |b: usize| -> bool {
        owner
            .get(b)
            .and_then(|&o| items.get(o as usize))
            .is_some_and(|it| it.nowrap)
    };
    let mut opps: Vec<(usize, bool)> = axiom_text::break_opportunities(&para)
        .into_iter()
        .filter(|o| o.offset > 0 && o.offset < para.len())
        .filter(|o| o.mandatory || (!nowrap_at(o.offset - 1) && !nowrap_at(o.offset)))
        .map(|o| (o.offset, o.mandatory))
        .collect();
    for it in &items {
        match it.kind {
            ItemKind::Wbr if it.start > 0 && it.start < para.len() => opps.push((it.start, false)),
            ItemKind::Text if it.style.word_break_all && !it.nowrap => {
                for (i, _) in para[it.start..it.end].char_indices().skip(1) {
                    opps.push((it.start + i, false));
                }
            }
            _ => {}
        }
    }
    opps.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    opps.dedup_by_key(|o| o.0);

    // Segments of pieces.
    let mut pieces: Vec<Piece> = Vec::new();
    let mut segments: Vec<Segment> = vec![Segment::default()];
    let mut next_opp = 0usize;
    let mut atomics = Vec::new();
    fn close_segment(pieces: &[Piece], segments: &mut Vec<Segment>, forced: bool) {
        let seg = segments.last_mut().unwrap();
        seg.pieces.end = pieces.len();
        seg.forced |= forced;
        segments.push(Segment {
            pieces: pieces.len()..pieces.len(),
            ..Segment::default()
        });
    }
    let level_at = |b: usize| {
        levels
            .as_ref()
            .map_or(0, |l| l.get(b).copied().unwrap_or(0))
    };
    for (i, it) in items.iter().enumerate() {
        while next_opp < opps.len() && opps[next_opp].0 <= it.start && it.end > it.start {
            let (_, mandatory) = opps[next_opp];
            if pieces.len() > segments.last().unwrap().pieces.start {
                close_segment(&pieces, &mut segments, mandatory);
            }
            next_opp += 1;
        }
        let zero = |pieces: &mut Vec<Piece>, atomic: Option<usize>| {
            pieces.push(Piece {
                item: i,
                start: it.start,
                end: it.end,
                run: None,
                width: 0.0,
                min_width: 0.0,
                level: level_at(it.start.min(para.len().saturating_sub(1))),
                atomic,
            });
        };
        match it.kind {
            ItemKind::Text => {
                let mut cur = it.start;
                loop {
                    let limit = if next_opp < opps.len() && opps[next_opp].0 <= it.end {
                        opps[next_opp].0
                    } else {
                        it.end
                    };
                    // Split further at bidi level changes.
                    let mut s = cur;
                    while s < limit {
                        let lvl = level_at(s);
                        let mut e = s;
                        while e < limit && level_at(e) == lvl {
                            e += para[e..].chars().next().map_or(1, char::len_utf8);
                        }
                        pieces.push(Piece {
                            item: i,
                            start: s,
                            end: e,
                            run: None,
                            width: 0.0,
                            min_width: 0.0,
                            level: lvl,
                            atomic: None,
                        });
                        s = e;
                    }
                    cur = limit;
                    if next_opp < opps.len() && opps[next_opp].0 == limit && limit <= it.end {
                        let mandatory = opps[next_opp].1;
                        next_opp += 1;
                        close_segment(&pieces, &mut segments, mandatory);
                        if limit == it.end {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
            ItemKind::Open(_) | ItemKind::Abs(_) | ItemKind::Wbr => zero(&mut pieces, None),
            ItemKind::Close(_) => {
                // A close edge right after a break opportunity belongs to the previous
                // segment (it ends the word before the break).
                let cur_empty = segments.last().unwrap().pieces.start == pieces.len();
                zero(&mut pieces, None);
                if cur_empty && segments.len() > 1 {
                    let n = segments.len();
                    let moved = pieces.len();
                    segments[n - 2].pieces.end = moved;
                    segments[n - 1].pieces.start = moved;
                }
            }
            ItemKind::Atomic(bx) | ItemKind::Float(bx) => {
                let index = atomics.len();
                if !measure {
                    atomics.push(layout_atomic(ctx, bx, width));
                }
                zero(&mut pieces, Some(index));
                if measure {
                    let (mn, mx) = block::outer_intrinsic(ctx, bx);
                    let p = pieces.last_mut().unwrap();
                    p.width = mx;
                    p.min_width = mn;
                }
            }
            ItemKind::Break => {
                zero(&mut pieces, None);
                if next_opp < opps.len() && opps[next_opp].0 == it.end {
                    next_opp += 1;
                }
                close_segment(&pieces, &mut segments, true);
            }
        }
    }
    segments.last_mut().unwrap().pieces.end = pieces.len();
    if segments.last().is_some_and(|s| s.pieces.is_empty()) && segments.len() > 1 {
        segments.pop();
    }

    // Measure.
    let mut space_cache: HashMap<usize, f32> = HashMap::new();
    for p in &mut pieces {
        let it = &items[p.item];
        match it.kind {
            ItemKind::Text => {
                let spec = font_spec(&it.style);
                let text = &para[p.start..p.end];
                let run = axiom_text::shape(text, &spec, p.level % 2 == 1);
                let run = apply_letter_spacing(run, it.style.letter_spacing);
                let spaces = text.matches(' ').count() as f32;
                p.width = run.width + it.style.word_spacing * spaces;
                p.min_width = p.width;
                p.run = Some(run);
            }
            ItemKind::Open(b) => {
                p.width = boxes[b].start_edge;
                p.min_width = p.width;
            }
            ItemKind::Close(b) => {
                p.width = boxes[b].end_edge;
                p.min_width = p.width;
            }
            ItemKind::Atomic(_) if !measure => {
                let a = &atomics[p.atomic.unwrap()];
                p.width = a.width;
                p.min_width = a.width;
            }
            _ => {}
        }
    }
    for seg in &mut segments {
        seg.width = pieces[seg.pieces.clone()].iter().map(|p| p.width).sum();
        seg.min_width = pieces[seg.pieces.clone()].iter().map(|p| p.min_width).sum();
        // Trailing spaces of the last text piece hang at the end of a line.
        if let Some(p) = pieces[seg.pieces.clone()]
            .iter()
            .rev()
            .find(|p| !matches!(items[p.item].kind, ItemKind::Close(_)))
        {
            let it = &items[p.item];
            if matches!(it.kind, ItemKind::Text)
                && it.style.space_collapse != SpaceCollapse::BreakSpaces
            {
                let text = &para[p.start..p.end];
                let n = text.len() - text.trim_end_matches(' ').len();
                if n > 0 {
                    let key = Arc::as_ptr(&it.style) as usize;
                    let sw = *space_cache.entry(key).or_insert_with(|| {
                        axiom_text::measure(" ", &font_spec(&it.style))
                            + it.style.letter_spacing
                            + it.style.word_spacing
                    });
                    seg.trailing = (sw * n as f32).min(p.width);
                }
            }
        }
    }
    Prepared {
        para,
        items,
        boxes,
        pieces,
        segments,
        atomics,
        rtl,
        bidi,
    }
}

fn layout_atomic<'a>(ctx: &Ctx<'a>, bx: &'a BoxNode, width: f32) -> AtomicLayout {
    let out = block::layout_block_level(
        ctx,
        bx,
        Containing {
            width,
            height: None,
            rtl: false,
        },
        0.0,
        false,
        None,
    );
    let [mt, mr, mb, ml] = out.margin;
    let frag = out.frag;
    let height = frag.rect.height + mt + mb;
    let uses_text_baseline = match &bx.kind {
        BoxKind::Replaced(Replaced::Widget(_)) => true,
        BoxKind::Replaced(_) => false,
        // An inline-block that clips its overflow sits on its bottom margin edge.
        _ => !bx.style.overflow_y.clips(),
    };
    let baseline = match out.last_baseline {
        Some(b) if uses_text_baseline => b + mt,
        _ => height,
    };
    AtomicLayout {
        width: frag.rect.width + ml + mr,
        frag,
        height,
        baseline,
        margin_top: mt,
    }
}

/// Min- and max-content widths of an inline formatting context.
pub(crate) fn intrinsic<'a>(ctx: &Ctx<'a>, container: &'a BoxNode) -> (f32, f32) {
    let p = prepare(ctx, container, 0.0, true);
    let indent = container
        .style
        .text_indent
        .resolve(0.0)
        .unwrap_or(0.0)
        .max(0.0);
    let mut min = 0.0f32;
    let mut max = 0.0f32;
    let mut acc = indent;
    for (i, seg) in p.segments.iter().enumerate() {
        min = min.max(seg.min_width - seg.trailing);
        acc += seg.width;
        if seg.forced || i + 1 == p.segments.len() {
            max = max.max(acc - seg.trailing);
            acc = 0.0;
        }
    }
    (min.max(0.0), max.max(min))
}

#[derive(Clone, Copy)]
struct VMetrics {
    ascent: f32,
    descent: f32,
    /// Half-leading-adjusted ascent/descent (the inline box's line-height contribution).
    lh_ascent: f32,
    lh_descent: f32,
}

fn vmetrics(cache: &mut HashMap<usize, VMetrics>, s: &Arc<ComputedStyle>) -> VMetrics {
    *cache.entry(Arc::as_ptr(s) as usize).or_insert_with(|| {
        let fm = axiom_text::font_metrics(&font_spec(s));
        let normal = fm.normal_line_height() / s.font_size.max(0.01);
        let l = s.line_height_px(normal);
        let half = (l - (fm.ascent + fm.descent)) / 2.0;
        VMetrics {
            ascent: fm.ascent,
            descent: fm.descent,
            lh_ascent: fm.ascent + half,
            lh_descent: fm.descent + half,
        }
    })
}

/// UAX #9 rule L2 over pieces: reverse runs at each level from the highest down to the
/// lowest odd level.
fn visual_order(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let Some(&max) = levels.iter().max() else {
        return order;
    };
    let min_odd = levels
        .iter()
        .copied()
        .filter(|l| l % 2 == 1)
        .min()
        .unwrap_or(max + 1);
    let mut level = max;
    while level >= min_odd && level > 0 {
        let mut i = 0;
        while i < order.len() {
            if levels[order[i]] >= level {
                let start = i;
                while i < order.len() && levels[order[i]] >= level {
                    i += 1;
                }
                order[start..i].reverse();
            } else {
                i += 1;
            }
        }
        level -= 1;
    }
    order
}

/// Places a float that occurs in the paragraph among the formatting context's floats.
fn place_float(
    a: AtomicLayout,
    bx: &BoxNode,
    floats: &mut Floats,
    y_min: f32,
    x0: f32,
    x1: f32,
    oy: f32,
) -> Fragment {
    let y_min = floats
        .clearance(bx.style.clear)
        .map_or(y_min, |c| y_min.max(c));
    let (x, y) = floats.place(
        bx.style.float == Float::Left,
        a.width,
        a.height,
        y_min,
        x0,
        x1,
    );
    let mut frag = a.frag;
    frag.translate(x, y - oy + a.margin_top);
    frag
}

/// Lays out the container's inline content from (`content_x`, `content_y`) in a
/// content box `width` wide. Line boxes are shortened next to `floats`; `oy` is the
/// container's border-box top in the floats' coordinates.
pub(crate) fn layout_inline<'a>(
    ctx: &Ctx<'a>,
    container: &'a BoxNode,
    content_x: f32,
    content_y: f32,
    width: f32,
    floats: &mut Floats,
    oy: f32,
) -> InlineOut {
    let p = prepare(ctx, container, width, false);
    let cs = &container.style;
    let indent = cs.text_indent.resolve(width).unwrap_or(0.0);

    let mut vcache: HashMap<usize, VMetrics> = HashMap::new();
    let strut = vmetrics(&mut vcache, cs);
    let strut_h = strut.lh_ascent + strut.lh_descent;
    let mut fragments_boxes: Vec<Fragment> = Vec::new();
    let mut fragments_content: Vec<Fragment> = Vec::new();
    let mut atomics: Vec<Option<AtomicLayout>> = p.atomics.into_iter().map(Some).collect();
    let mut y = content_y;
    let mut first_baseline = None;
    let mut last_baseline = None;
    let n_segs = p.segments.len();
    let (x0, x1) = (content_x, content_x + width);
    let narrowed = |l: f32, r: f32| l > x0 + 0.01 || r < x1 - 0.01;
    let mut next = 0usize;
    let mut li = 0usize;

    while next < n_segs {
        // Greedy filling of one line in the space the floats leave at its height.
        let line_indent = if li == 0 { indent } else { 0.0 };
        let (mut left, mut right) = floats.band(oy + y, strut_h, x0, x1);
        let first = &p.segments[next];
        if narrowed(left, right) && first.width - first.trailing > right - left - line_indent + 0.01
        {
            if let Some(b) = floats.next_bottom(oy + y) {
                y = b - oy;
                continue;
            }
        }
        let start = next;
        let mut w = 0.0f32;
        let mut deferred: Vec<usize> = Vec::new();
        while next < n_segs {
            let seg = &p.segments[next];
            let avail = right - left - line_indent;
            if next > start && w + seg.width - seg.trailing > avail + 0.01 {
                break;
            }
            for pi in seg.pieces.clone() {
                let pc = &p.pieces[pi];
                let ItemKind::Float(bx) = p.items[pc.item].kind else {
                    continue;
                };
                let fits = atomics[pc.atomic.unwrap()]
                    .as_ref()
                    .is_some_and(|a| w + a.width <= avail + 0.01);
                if deferred.is_empty() && fits {
                    let a = atomics[pc.atomic.unwrap()].take().unwrap();
                    fragments_content.push(place_float(a, bx, floats, oy + y, x0, x1, oy));
                    (left, right) = floats.band(oy + y, strut_h, x0, x1);
                } else {
                    deferred.push(pi);
                }
            }
            w += seg.width;
            next += 1;
            if seg.forced {
                break;
            }
        }
        let line_x = left;
        let line_width = (right - left).max(0.0);
        let range = start..next;
        let is_last = next >= n_segs;
        li += 1;

        let segs = &p.segments[range.clone()];
        let piece_ids: Vec<usize> = segs.iter().flat_map(|s| s.pieces.clone()).collect();
        if piece_ids.is_empty() {
            continue;
        }
        let last_seg = segs.last().unwrap();
        let trailing = last_seg.trailing;
        let trailing_piece = last_seg
            .pieces
            .clone()
            .rev()
            .find(|&i| matches!(p.items[p.pieces[i].item].kind, ItemKind::Text));
        let line_w: f32 = segs.iter().map(|s| s.width).sum::<f32>() - trailing;
        let forced = last_seg.forced;

        // Does the line box have content (CSS 2 §9.4.2 zero-height lines)?
        let has_content = piece_ids.iter().any(|&i| {
            let pc = &p.pieces[i];
            let it = &p.items[pc.item];
            match it.kind {
                ItemKind::Text => p.para[pc.start..pc.end]
                    .chars()
                    .any(|c| c != ' ' || it.style.preserves_spaces()),
                ItemKind::Atomic(_) | ItemKind::Break => true,
                ItemKind::Open(b) | ItemKind::Close(b) => {
                    let bx = &p.boxes[b];
                    bx.start_edge != 0.0 || bx.end_edge != 0.0
                }
                _ => false,
            }
        });

        // Horizontal placement.
        let avail = line_width - line_indent;
        let free = avail - line_w;
        let align = match cs.text_align {
            TextAlign::Justify if is_last || forced => TextAlign::Start,
            a => a,
        };
        let (offset, justify_gap) = match (align, p.rtl) {
            (TextAlign::Left | TextAlign::WebkitLeft, _)
            | (TextAlign::Start, false)
            | (TextAlign::End, true) => (0.0, 0.0),
            (TextAlign::Right | TextAlign::WebkitRight, _)
            | (TextAlign::End, false)
            | (TextAlign::Start, true) => (free, 0.0),
            (TextAlign::Center | TextAlign::WebkitCenter, _) => (free / 2.0, 0.0),
            (TextAlign::Justify, rtl) => {
                if segs.len() > 1 && free > 0.0 && !p.bidi {
                    (0.0, free / (segs.len() - 1) as f32)
                } else if rtl {
                    (free, 0.0)
                } else {
                    (0.0, 0.0)
                }
            }
        };
        let start_x = if p.rtl {
            line_x + offset
        } else {
            line_x + line_indent + offset
        };
        let order: Vec<usize> = if p.bidi {
            let levels: Vec<u8> = piece_ids.iter().map(|&i| p.pieces[i].level).collect();
            visual_order(&levels)
                .into_iter()
                .map(|k| piece_ids[k])
                .collect()
        } else {
            piece_ids.clone()
        };
        let seg_end: std::collections::HashSet<usize> = segs
            .iter()
            .take(segs.len().saturating_sub(1))
            .filter_map(|s| s.pieces.clone().last())
            .collect();
        let mut xs: HashMap<usize, f32> = HashMap::with_capacity(order.len());
        let mut cursor = start_x;
        for &i in &order {
            let pc = &p.pieces[i];
            let mut adv = pc.width;
            let mut x = cursor;
            if Some(i) == trailing_piece && trailing > 0.0 {
                adv -= trailing;
                if pc.level % 2 == 1 {
                    x -= trailing;
                }
            }
            xs.insert(i, x);
            cursor += adv;
            if justify_gap > 0.0 && seg_end.contains(&i) {
                cursor += justify_gap;
            }
        }

        // Vertical metrics.
        let (mut max_asc, mut max_desc) = if has_content {
            (strut.lh_ascent, strut.lh_descent)
        } else {
            (0.0, 0.0)
        };
        let mut edge_aligned: Vec<(usize, f32)> = Vec::new();
        if has_content {
            for &i in &piece_ids {
                let pc = &p.pieces[i];
                let it = &p.items[pc.item];
                match it.kind {
                    ItemKind::Text | ItemKind::Open(_) | ItemKind::Close(_) => {
                        let vm = vmetrics(&mut vcache, &it.style);
                        max_asc = max_asc.max(vm.lh_ascent + it.shift);
                        max_desc = max_desc.max(vm.lh_descent - it.shift);
                    }
                    ItemKind::Atomic(bx) => {
                        let a = atomics[pc.atomic.unwrap()].as_ref().unwrap();
                        let parent = it
                            .inline_box
                            .map(|b| p.boxes[b].bx.style.clone())
                            .unwrap_or_else(|| cs.clone());
                        let pvm = vmetrics(&mut vcache, &parent);
                        let asc = match bx.style.vertical_align {
                            VerticalAlign::Top | VerticalAlign::Bottom => {
                                edge_aligned.push((i, a.height));
                                continue;
                            }
                            VerticalAlign::Middle => a.height / 2.0 + parent.font_size * 0.25,
                            VerticalAlign::TextTop => pvm.ascent,
                            VerticalAlign::TextBottom => a.height - pvm.descent,
                            VerticalAlign::Sub => a.baseline - parent.font_size * 0.2,
                            VerticalAlign::Super => a.baseline + parent.font_size * 0.34,
                            VerticalAlign::Length(px) => a.baseline + px,
                            VerticalAlign::Baseline => a.baseline,
                        } + it.shift;
                        max_asc = max_asc.max(asc);
                        max_desc = max_desc.max(a.height - asc);
                    }
                    _ => {}
                }
            }
            for &(_, h) in &edge_aligned {
                if h > max_asc + max_desc {
                    max_desc = h - max_asc;
                }
            }
        }
        let line_h = max_asc + max_desc;
        let baseline = y + max_asc;
        if has_content {
            if first_baseline.is_none() {
                first_baseline = Some(baseline);
            }
            last_baseline = Some(baseline);
        }

        // Inline box fragments for this line.
        let mut extents: HashMap<usize, (f32, f32, bool, bool)> = HashMap::new();
        for &i in &piece_ids {
            let pc = &p.pieces[i];
            let it = &p.items[pc.item];
            let x = xs[&i];
            if matches!(it.kind, ItemKind::Float(_)) {
                continue;
            }
            let (own, lo, hi) = match it.kind {
                ItemKind::Open(b) => (Some(b), x + p.boxes[b].margin_left, x + pc.width),
                ItemKind::Close(b) => {
                    let bx = &p.boxes[b];
                    let m = block::BoxMetrics::new(&bx.bx.style, width);
                    (Some(b), x, x + m.padding[1] + m.border[1])
                }
                _ => (None, x, x + pc.width),
            };
            if let Some(b) = own {
                let e = extents.entry(b).or_insert((lo, hi, false, false));
                e.0 = e.0.min(lo);
                e.1 = e.1.max(hi);
                match it.kind {
                    ItemKind::Open(_) => {
                        e.0 = lo;
                        e.2 = true;
                    }
                    _ => {
                        e.1 = hi;
                        e.3 = true;
                    }
                }
            }
            let mut anc = match it.kind {
                ItemKind::Open(b) | ItemKind::Close(b) => p.boxes[b].parent,
                _ => it.inline_box,
            };
            while let Some(b) = anc {
                let e = extents.entry(b).or_insert((lo, hi, false, false));
                if !e.2 {
                    e.0 = e.0.min(lo);
                }
                if !e.3 {
                    e.1 = e.1.max(hi);
                }
                anc = p.boxes[b].parent;
            }
        }
        let mut box_ids: Vec<usize> = extents.keys().copied().collect();
        box_ids.sort_unstable();
        for b in box_ids {
            let (lo, hi, has_open, has_close) = extents[&b];
            let ib = &p.boxes[b];
            let s = &ib.bx.style;
            let m = block::BoxMetrics::new(s, width);
            let vm = vmetrics(&mut vcache, s);
            let top = baseline - ib.shift - vm.ascent - m.padding[0] - m.border[0];
            let bottom = baseline - ib.shift + vm.descent + m.padding[2] + m.border[2];
            fragments_boxes.push(Fragment {
                node: ib.bx.node,
                style: s.clone(),
                rect: Rect::new(lo, top, (hi - lo).max(0.0), bottom - top),
                kind: FragmentKind::Box(BoxEdges {
                    left: has_open,
                    right: has_close,
                    inline: true,
                }),
                children: Vec::new(),
            });
        }

        // Text, atomics and placeholders in visual order. Adjacent pieces of one text
        // item join into a single fragment: (item, byte end, fragment index, ink right).
        let mut tail: Option<(usize, usize, usize, f32)> = None;
        for &i in &order {
            let pc = &p.pieces[i];
            let it = &p.items[pc.item];
            let x = xs[&i];
            if !matches!(it.kind, ItemKind::Text) {
                tail = None;
            }
            match it.kind {
                ItemKind::Text => {
                    let text = &p.para[pc.start..pc.end];
                    let blank = text.trim().is_empty() && !it.style.preserves_spaces();
                    if let Some((item, end, idx, ink)) = tail {
                        let f = &mut fragments_content[idx];
                        if item == pc.item && end == pc.start && (f.rect.right() - x).abs() < 0.5 {
                            let dx = x - f.rect.x;
                            if let FragmentKind::Text(t) = &mut f.kind {
                                t.text.push_str(text);
                                let run = Arc::make_mut(&mut t.run);
                                if let Some(r) = &pc.run {
                                    run.glyphs.extend(
                                        r.glyphs.iter().map(|g| Glyph { x: g.x + dx, ..*g }),
                                    );
                                }
                                run.width = dx + pc.width;
                            }
                            f.rect.width += pc.width;
                            let ink = if blank { ink } else { f.rect.right() };
                            tail = Some((pc.item, pc.end, idx, ink));
                            continue;
                        }
                    }
                    if blank {
                        tail = None;
                        continue;
                    }
                    tail = Some((pc.item, pc.end, fragments_content.len(), x + pc.width));
                    let vm = vmetrics(&mut vcache, &it.style);
                    let b = baseline - it.shift;
                    fragments_content.push(Fragment {
                        node: it.node,
                        style: it.style.clone(),
                        rect: Rect::new(x, b - vm.ascent, pc.width, vm.ascent + vm.descent),
                        kind: FragmentKind::Text(TextFragment {
                            text: text.to_string(),
                            run: pc.run.clone().unwrap_or_default(),
                            baseline: b,
                            font_size: it.style.font_size,
                            color: it.style.color,
                            decorations: it.style.decorations_in_effect,
                            decoration_color: it.style.decoration_color,
                        }),
                        children: Vec::new(),
                    });
                }
                ItemKind::Atomic(bx) => {
                    let a = atomics[pc.atomic.unwrap()].take().unwrap();
                    let margin_top_y = match bx.style.vertical_align {
                        VerticalAlign::Top => y,
                        VerticalAlign::Bottom => y + line_h - a.height,
                        _ => {
                            let parent_fs = it
                                .inline_box
                                .map(|b| p.boxes[b].bx.style.font_size)
                                .unwrap_or(cs.font_size);
                            let pvm = {
                                let st = it
                                    .inline_box
                                    .map(|b| p.boxes[b].bx.style.clone())
                                    .unwrap_or_else(|| cs.clone());
                                vmetrics(&mut vcache, &st)
                            };
                            let asc = match bx.style.vertical_align {
                                VerticalAlign::Middle => a.height / 2.0 + parent_fs * 0.25,
                                VerticalAlign::TextTop => pvm.ascent,
                                VerticalAlign::TextBottom => a.height - pvm.descent,
                                VerticalAlign::Sub => a.baseline - parent_fs * 0.2,
                                VerticalAlign::Super => a.baseline + parent_fs * 0.34,
                                VerticalAlign::Length(px) => a.baseline + px,
                                _ => a.baseline,
                            } + it.shift;
                            baseline - asc
                        }
                    };
                    let mut frag = a.frag;
                    frag.translate(x, margin_top_y + a.margin_top);
                    if let FragmentKind::Box(edges) = &mut frag.kind {
                        edges.inline = true;
                    }
                    fragments_content.push(frag);
                }
                ItemKind::Abs(bx) => {
                    fragments_content.push(block::placeholder(ctx, bx, x, y));
                }
                _ => {}
            }
        }
        // Spaces hanging at the end of the line are not part of the ink.
        if let Some((_, _, idx, ink)) = tail {
            let f = &mut fragments_content[idx];
            f.rect.width = (ink - f.rect.x).max(0.0);
        }
        y += line_h;
        // Floats that did not fit beside this line go below it.
        for pi in deferred {
            let pc = &p.pieces[pi];
            if let (ItemKind::Float(bx), Some(a)) =
                (p.items[pc.item].kind, atomics[pc.atomic.unwrap()].take())
            {
                fragments_content.push(place_float(a, bx, floats, oy + y, x0, x1, oy));
            }
        }
    }

    let mut fragments = fragments_boxes;
    fragments.extend(fragments_content);
    InlineOut {
        fragments,
        height: y - content_y,
        first_baseline,
        last_baseline,
    }
}
