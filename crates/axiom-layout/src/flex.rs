//! Flexible box layout (CSS Flexbox 1 §9): `order`, line breaking, flexible length
//! resolution with min/max violations and automatic minimum sizes, cross sizes with
//! stretching and baseline alignment, `align-content`, main-axis auto margins and
//! `justify-content`, gaps, and reversed or right-to-left axes.

use axiom_style::{Align, ComputedStyle, FlexWrap, Length, Overflow};

use crate::block::{self, BlockOut, BoxMetrics, ContainerOut, Containing, Space};
use crate::tree::{BoxKind, BoxNode};
use crate::Ctx;

const EPS: f32 = 0.01;

/// A `row-gap` / `column-gap` (`normal` is 0 in flex and grid containers); percentages
/// need a definite `basis`.
pub(crate) fn gap(l: &Length, basis: Option<f32>) -> f32 {
    match l {
        Length::Px(v) => v.max(0.0),
        l if l.has_percent() => basis.and_then(|b| l.resolve(b)).unwrap_or(0.0).max(0.0),
        _ => 0.0,
    }
}

/// `left` / `right` as start or end of an axis that runs right-to-left when `reversed`.
pub(crate) fn logical(a: Align, reversed: bool) -> Align {
    match (a, reversed) {
        (Align::Left, false) | (Align::Right, true) => Align::Start,
        (Align::Left, true) | (Align::Right, false) => Align::End,
        (a, _) => a,
    }
}

/// Leading offset and extra spacing between `n` subjects for a content-distribution
/// value with `free` space (CSS Box Alignment 3 §5.3). Distribution falls back to
/// `start` or `center` when there is no positive space to distribute.
pub(crate) fn distribute(a: Align, free: f32, n: usize) -> (f32, f32) {
    let count = n as f32;
    match a {
        Align::End => (free, 0.0),
        Align::Center => (free / 2.0, 0.0),
        Align::SpaceBetween if free > 0.0 && n > 1 => (0.0, free / (count - 1.0)),
        Align::SpaceAround if free > 0.0 && n > 0 => (free / count / 2.0, free / count),
        Align::SpaceEvenly if free > 0.0 => (free / (count + 1.0), free / (count + 1.0)),
        Align::SpaceAround | Align::SpaceEvenly => (free / 2.0, 0.0),
        _ => (0.0, 0.0),
    }
}

/// Min- and max-content contributions (margin box) of a flex or grid item. Replaced
/// boxes sized by percentages are compressible: they contribute nothing to the
/// min-content size (CSS Sizing 3 §5.2.2).
pub(crate) fn item_contribution<'a>(ctx: &Ctx<'a>, bx: &'a BoxNode) -> (f32, f32) {
    let (min, max) = block::outer_intrinsic(ctx, bx);
    let s = &bx.style;
    let compressible = matches!(bx.kind, BoxKind::Replaced(_))
        && (s.width.has_percent() || s.max_width.has_percent());
    if compressible {
        (horizontal_extras(s), max)
    } else {
        (min, max)
    }
}

/// Horizontal border, padding and fixed margins (what `outer_intrinsic` adds).
fn horizontal_extras(s: &ComputedStyle) -> f32 {
    let px = |l: &Length| match l {
        Length::Px(v) => *v,
        _ => 0.0,
    };
    let b = s.border_widths();
    px(&s.padding.left)
        + px(&s.padding.right)
        + b[1]
        + b[3]
        + px(&s.margin.left)
        + px(&s.margin.right)
}

struct Item<'a> {
    bx: &'a BoxNode,
    m: BoxMetrics,
    main_bp: f32,
    /// Main-axis margins with `auto` as 0.
    main_margin: f32,
    cross_bp: f32,
    cross_margin: f32,
    /// Flex base size, hypothetical and target main sizes, and min/max main sizes, all
    /// content-box.
    base: f32,
    hypo: f32,
    target: f32,
    min: f32,
    max: f32,
    frozen: bool,
    /// Content width the item is laid out at (column containers).
    width: f32,
    align: Align,
    stretch: bool,
    out: Option<BlockOut>,
}

impl Item<'_> {
    fn outer_hypo(&self) -> f32 {
        self.hypo + self.main_bp + self.main_margin
    }

    fn outer_target(&self) -> f32 {
        self.target + self.main_bp + self.main_margin
    }

    fn factor(&self, grow: bool) -> f32 {
        let s = &self.bx.style;
        if grow {
            s.flex_grow
        } else {
            s.flex_shrink
        }
    }
}

/// Main-axis quantities of one item, resolved before lines are formed.
fn new_item<'a>(
    ctx: &Ctx<'a>,
    bx: &'a BoxNode,
    container: &ComputedStyle,
    space: &Space,
    single_line: bool,
) -> Item<'a> {
    let s = &bx.style;
    let row = container.flex_direction.is_row();
    let m = BoxMetrics::new(s, space.width);
    let (bp_h, bp_v) = (m.bp_h(), m.bp_v());
    let cb = Containing {
        width: space.width,
        height: space.height,
        rtl: container.rtl,
    };
    let align = match s.align_self.unwrap_or(container.align_items) {
        Align::Normal => Align::Stretch,
        Align::Baseline if !row => Align::Start,
        a => logical(a, false),
    };
    let cross_auto_margin = if row {
        s.margin.top.is_auto() || s.margin.bottom.is_auto()
    } else {
        m.auto_left || m.auto_right
    };
    let cross_size_auto = if row {
        s.height.is_auto()
    } else {
        s.width.is_auto()
    };
    let stretch = align == Align::Stretch && cross_size_auto && !cross_auto_margin;
    let replaced = match &bx.kind {
        BoxKind::Replaced(r) => Some(r),
        _ => None,
    };

    // Column containers know each item's width (the cross size) before its height.
    let width = if row {
        0.0
    } else {
        let avail = (space.width - m.margin[1] - m.margin[3] - bp_h).max(0.0);
        let w = match (replaced, block::specified_width(s, space.width, bp_h)) {
            (_, Some(w)) => w,
            (_, None) if stretch && single_line => avail,
            (Some(r), None) => block::replaced_size(bx, r, cb).0,
            (None, None) => match s.width {
                Length::MinContent => block::content_intrinsic(ctx, bx).0,
                Length::MaxContent => block::content_intrinsic(ctx, bx).1,
                _ => block::shrink_to_fit(ctx, bx, avail),
            },
        };
        block::clamp_width(s, w, space.width, bp_h)
    };

    let (main_bp, main_margin, cross_bp, cross_margin) = if row {
        (
            bp_h,
            m.margin[1] + m.margin[3],
            bp_v,
            m.margin[0] + m.margin[2],
        )
    } else {
        (
            bp_v,
            m.margin[0] + m.margin[2],
            bp_h,
            m.margin[1] + m.margin[3],
        )
    };
    let container_main = if row { Some(space.width) } else { space.height };
    let to_content = |v: f32| {
        if s.border_box_sizing {
            (v - main_bp).max(0.0)
        } else {
            v.max(0.0)
        }
    };
    let definite = |l: &Length| match l {
        Length::Px(v) => Some(*v),
        l if l.has_percent() => container_main.and_then(|c| l.resolve(c)),
        _ => None,
    };
    let main_prop = if row { &s.width } else { &s.height };
    let basis = if s.flex_basis.is_auto() {
        main_prop
    } else {
        &s.flex_basis
    };

    // The content size in the main axis: max-content width, or the height the item
    // needs at its width.
    let mut content_cache: Option<f32> = None;
    let mut content_main = |keyword: &Length| -> f32 {
        if let Some(v) = content_cache {
            return v;
        }
        let v = match (row, replaced) {
            (true, Some(r)) => block::replaced_size(bx, r, cb).0,
            (true, None) => match keyword {
                Length::MinContent => block::content_intrinsic(ctx, bx).0,
                Length::FitContent => {
                    block::shrink_to_fit(ctx, bx, space.width - main_margin - main_bp)
                }
                _ => block::content_intrinsic(ctx, bx).1,
            },
            (false, Some(r)) => block::replaced_height_at(bx, r, width, cb),
            (false, None) => {
                let out = block::layout_item(ctx, bx, &m, width, None, cb);
                (out.auto_height - bp_v).max(0.0)
            }
        };
        content_cache = Some(v);
        v
    };
    let base = match definite(basis) {
        Some(v) => to_content(v),
        None => content_main(basis),
    };

    let max_prop = if row { &s.max_width } else { &s.max_height };
    let max = definite(max_prop).map_or(f32::INFINITY, to_content);
    let min_prop = if row { &s.min_width } else { &s.min_height };
    let min = match min_prop {
        Length::Auto => {
            let overflow = if row { s.overflow_x } else { s.overflow_y };
            if !matches!(overflow, Overflow::Visible | Overflow::Clip) {
                0.0
            } else {
                // Automatic minimum size (§4.5): the content size suggestion, capped by
                // a definite specified size.
                let content = if row {
                    let (min_content, _) = item_contribution(ctx, bx);
                    (min_content - main_bp - main_margin).max(0.0)
                } else {
                    content_main(&Length::Auto)
                };
                let content = content.min(max);
                match definite(main_prop) {
                    Some(v) => content.min(to_content(v)),
                    None => content,
                }
            }
        }
        l => definite(l).map_or(0.0, to_content),
    };
    let hypo = base.min(max).max(min);
    Item {
        bx,
        m,
        main_bp,
        main_margin,
        cross_bp,
        cross_margin,
        base,
        hypo,
        target: hypo,
        min,
        max,
        frozen: false,
        width,
        align,
        stretch,
        out: None,
    }
}

/// Outer main size of a line's items (hypothetical or target sizes) with the gaps.
fn line_used(items: &[Item], gap: f32, target: bool) -> f32 {
    let sizes: f32 = items
        .iter()
        .map(|i| {
            if target {
                i.outer_target()
            } else {
                i.outer_hypo()
            }
        })
        .sum();
    sizes + gap * items.len().saturating_sub(1) as f32
}

/// Resolve the flexible lengths of one line's items into `target` (§9.7).
fn resolve_flexible(items: &mut [Item], avail: f32, gap: f32) {
    let gaps = gap * items.len().saturating_sub(1) as f32;
    let used: f32 = items.iter().map(Item::outer_hypo).sum::<f32>() + gaps;
    let grow = used < avail;
    for it in items.iter_mut() {
        it.target = it.hypo;
        it.frozen =
            it.factor(grow) == 0.0 || (grow && it.base > it.hypo) || (!grow && it.base < it.hypo);
    }
    let free = |items: &[Item]| {
        avail
            - gaps
            - items
                .iter()
                .map(|i| i.main_bp + i.main_margin + if i.frozen { i.target } else { i.base })
                .sum::<f32>()
    };
    let initial = free(items);
    while items.iter().any(|i| !i.frozen) {
        let mut remaining = free(items);
        let factors: f32 = items
            .iter()
            .filter(|i| !i.frozen)
            .map(|i| i.factor(grow))
            .sum();
        if factors < 1.0 && (initial * factors).abs() < remaining.abs() {
            remaining = initial * factors;
        }
        let scaled: f32 = items
            .iter()
            .filter(|i| !i.frozen)
            .map(|i| i.factor(false) * i.base)
            .sum();
        for it in items.iter_mut().filter(|i| !i.frozen) {
            it.target = if grow {
                it.base + remaining * it.factor(true) / factors
            } else if scaled > 0.0 {
                it.base - remaining.abs() * it.factor(false) * it.base / scaled
            } else {
                it.base
            };
        }
        let mut total = 0.0;
        let mut violation = vec![0.0f32; items.len()];
        for (i, it) in items.iter_mut().enumerate().filter(|(_, i)| !i.frozen) {
            let clamped = it.target.min(it.max).max(it.min).max(0.0);
            violation[i] = clamped - it.target;
            total += violation[i];
            it.target = clamped;
        }
        for (i, it) in items.iter_mut().enumerate().filter(|(_, i)| !i.frozen) {
            if total.abs() < 1e-3
                || (total > 0.0 && violation[i] > 0.0)
                || (total < 0.0 && violation[i] < 0.0)
            {
                it.frozen = true;
            }
        }
    }
}

pub(crate) fn layout<'a>(ctx: &Ctx<'a>, bx: &'a BoxNode, space: &Space) -> ContainerOut {
    let s = &bx.style;
    let row = s.flex_direction.is_row();
    let wrap = s.flex_wrap != FlexWrap::NoWrap;
    // Physical direction of the main and cross axes.
    let main_rev = s.flex_direction.is_reverse() != (row && s.rtl);
    let cross_rev = (s.flex_wrap == FlexWrap::WrapReverse) != (!row && s.rtl);
    let item_cb = Containing {
        width: space.width,
        height: space.height,
        rtl: s.rtl,
    };
    let (main_gap, cross_gap) = if row {
        (
            gap(&s.column_gap, Some(space.width)),
            gap(&s.row_gap, space.height),
        )
    } else {
        (
            gap(&s.row_gap, space.height),
            gap(&s.column_gap, Some(space.width)),
        )
    };
    let clamp_container_h = |h: f32| block::clamp_height(s, h, space.cb_height, space.bp_v);

    let mut fragments = Vec::new();
    let mut in_flow: Vec<&BoxNode> = Vec::new();
    for c in &bx.children {
        if c.is_out_of_flow() {
            let x = if s.rtl {
                space.x + space.width
            } else {
                space.x
            };
            fragments.push(block::placeholder(ctx, c, x, space.y));
        } else {
            in_flow.push(c);
        }
    }
    in_flow.sort_by_key(|c| c.style.order);
    let mut items: Vec<Item> = in_flow
        .into_iter()
        .map(|c| new_item(ctx, c, s, space, !wrap))
        .collect();

    // Lines (§9.3).
    let avail_main = if row { Some(space.width) } else { space.height };
    let break_at = avail_main.unwrap_or_else(|| clamp_container_h(f32::INFINITY));
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let (mut start, mut used) = (0, 0.0f32);
    for (i, item) in items.iter().enumerate() {
        let outer = item.outer_hypo();
        if wrap && i > start && used + main_gap + outer > break_at + EPS {
            lines.push((start, i));
            start = i;
            used = outer;
        } else {
            used += if i > start { main_gap } else { 0.0 } + outer;
        }
    }
    if start < items.len() {
        lines.push((start, items.len()));
    }
    let main_size = avail_main.unwrap_or_else(|| {
        let longest = lines
            .iter()
            .map(|&(a, b)| line_used(&items[a..b], main_gap, false))
            .fold(0.0, f32::max);
        clamp_container_h(longest)
    });

    for &(a, b) in &lines {
        resolve_flexible(&mut items[a..b], main_size, main_gap);
    }

    // Hypothetical cross sizes (§9.4).
    for it in &mut items {
        it.out = Some(if row {
            block::layout_item(ctx, it.bx, &it.m, it.target, None, item_cb)
        } else {
            block::layout_item(ctx, it.bx, &it.m, it.width, Some(it.target), item_cb)
        });
    }
    let cross_of = |out: &BlockOut| {
        if row {
            out.frag.rect.height
        } else {
            out.frag.rect.width
        }
    };
    let baseline_of = |out: &BlockOut| out.first_baseline.unwrap_or(out.frag.rect.height);
    let baseline_item = |it: &Item| {
        row && it.align == Align::Baseline
            && !it.bx.style.margin.top.is_auto()
            && !it.bx.style.margin.bottom.is_auto()
    };
    let mut line_cross: Vec<f32> = lines
        .iter()
        .map(|&(a, b)| {
            let (mut outer_max, mut ascent, mut descent) = (0.0f32, 0.0f32, 0.0f32);
            for it in &items[a..b] {
                let out = it.out.as_ref().expect("laid out");
                let outer = cross_of(out) + it.cross_margin;
                if baseline_item(it) {
                    let asc = it.m.margin[0] + baseline_of(out);
                    ascent = ascent.max(asc);
                    descent = descent.max(outer - asc);
                } else {
                    outer_max = outer_max.max(outer);
                }
            }
            outer_max.max(ascent + descent)
        })
        .collect();
    let definite_cross = if row { space.height } else { Some(space.width) };
    if !wrap {
        if let Some(first) = line_cross.first_mut() {
            *first = match definite_cross {
                Some(c) => c,
                None => clamp_container_h(*first),
            };
        }
    }
    let lines_total =
        line_cross.iter().sum::<f32>() + cross_gap * lines.len().saturating_sub(1) as f32;
    let container_cross = definite_cross.unwrap_or_else(|| clamp_container_h(lines_total));

    // align-content (§9.4 step 15, §9.6).
    let free_cross = container_cross - lines_total;
    let (mut cross_lead, mut cross_between) = (0.0, 0.0);
    if wrap && !lines.is_empty() {
        match s.align_content {
            Align::Normal | Align::Stretch => {
                if free_cross > 0.0 {
                    let each = free_cross / lines.len() as f32;
                    line_cross.iter_mut().for_each(|c| *c += each);
                }
            }
            a => {
                (cross_lead, cross_between) = distribute(logical(a, false), free_cross, lines.len())
            }
        }
    }

    // Stretched items (§9.4 step 11) get a definite cross size and are laid out again.
    for (li, &(a, b)) in lines.iter().enumerate() {
        for it in &mut items[a..b] {
            if !it.stretch {
                continue;
            }
            let st = &it.bx.style;
            let inner = (line_cross[li] - it.cross_margin - it.cross_bp).max(0.0);
            if row {
                let h = block::clamp_height(st, inner, space.height, it.cross_bp);
                let current = it.out.as_ref().map_or(0.0, |o| o.frag.rect.height);
                if (h + it.cross_bp - current).abs() > EPS
                    || !matches!(it.bx.kind, BoxKind::Replaced(_))
                {
                    it.out = Some(block::layout_item(
                        ctx,
                        it.bx,
                        &it.m,
                        it.target,
                        Some(h),
                        item_cb,
                    ));
                }
            } else {
                let w = block::clamp_width(st, inner, space.width, it.cross_bp);
                if (w - it.width).abs() > EPS {
                    it.width = w;
                    it.out = Some(block::layout_item(
                        ctx,
                        it.bx,
                        &it.m,
                        w,
                        Some(it.target),
                        item_cb,
                    ));
                }
            }
        }
    }

    // Main-axis and cross-axis alignment (§9.5, §9.6), in logical coordinates that are
    // mirrored for reversed axes.
    let justify = logical(s.justify_content, main_rev && row);
    let mut first_baseline = None;
    let mut last_baseline = None;
    let mut cross_cursor = cross_lead;
    for (li, &(a, b)) in lines.iter().enumerate() {
        let lc = line_cross[li];
        let line_pos = if cross_rev {
            container_cross - cross_cursor - lc
        } else {
            cross_cursor
        };
        cross_cursor += lc + cross_gap + cross_between;
        let line = &mut items[a..b];
        let free = main_size - line_used(line, main_gap, true);
        let main_auto = |it: &Item| {
            let st = &it.bx.style;
            let (start, end) = if row {
                (it.m.auto_left, it.m.auto_right)
            } else {
                (st.margin.top.is_auto(), st.margin.bottom.is_auto())
            };
            if main_rev {
                (end, start)
            } else {
                (start, end)
            }
        };
        let auto_count: usize = line
            .iter()
            .map(|it| {
                let (s0, s1) = main_auto(it);
                s0 as usize + s1 as usize
            })
            .sum();
        let (lead, between, auto_each) = if free > 0.0 && auto_count > 0 {
            (0.0, 0.0, free / auto_count as f32)
        } else {
            let (l, b) = distribute(justify, free, line.len());
            (l, b, 0.0)
        };
        let max_ascent = line
            .iter()
            .filter(|it| baseline_item(it))
            .map(|it| it.m.margin[0] + baseline_of(it.out.as_ref().expect("laid out")))
            .fold(0.0f32, f32::max);
        let mut cursor = lead;
        let mut line_baseline: Option<f32> = None;
        let mut line_first: Option<f32> = None;
        for it in line.iter_mut() {
            let mut out = it.out.take().expect("laid out");
            let st = &it.bx.style;
            let (m_top, m_right, m_bottom, m_left) = (
                it.m.margin[0],
                it.m.margin[1],
                it.m.margin[2],
                it.m.margin[3],
            );
            let (main_start, main_end) = match (row, main_rev) {
                (true, false) => (m_left, m_right),
                (true, true) => (m_right, m_left),
                (false, false) => (m_top, m_bottom),
                (false, true) => (m_bottom, m_top),
            };
            let (auto_start, auto_end) = main_auto(it);
            let size = if row {
                out.frag.rect.width
            } else {
                out.frag.rect.height
            };
            cursor += main_start + if auto_start { auto_each } else { 0.0 };
            let main_pos = if main_rev {
                main_size - cursor - size
            } else {
                cursor
            };
            cursor += size + main_end + if auto_end { auto_each } else { 0.0 } + between + main_gap;

            let cross = cross_of(&out);
            let (cross_start, cross_end, c_auto_start, c_auto_end) = {
                let (s0, e0, as0, ae0) = if row {
                    (
                        m_top,
                        m_bottom,
                        st.margin.top.is_auto(),
                        st.margin.bottom.is_auto(),
                    )
                } else {
                    (m_left, m_right, it.m.auto_left, it.m.auto_right)
                };
                if cross_rev {
                    (e0, s0, ae0, as0)
                } else {
                    (s0, e0, as0, ae0)
                }
            };
            let cfree = lc - cross - cross_start - cross_end;
            let offset = if (c_auto_start || c_auto_end) && cfree > 0.0 {
                match (c_auto_start, c_auto_end) {
                    (true, true) => cfree / 2.0,
                    (true, false) => cfree,
                    _ => 0.0,
                }
            } else if baseline_item(it) {
                max_ascent - (m_top + baseline_of(&out))
            } else {
                match it.align {
                    Align::End => cfree,
                    Align::Center => cfree / 2.0,
                    _ => 0.0,
                }
            };
            let logical_cross = offset + cross_start;
            let cross_pos = if cross_rev {
                lc - logical_cross - cross
            } else {
                logical_cross
            };
            let (x, y) = if row {
                (space.x + main_pos, space.y + line_pos + cross_pos)
            } else {
                (space.x + line_pos + cross_pos, space.y + main_pos)
            };
            out.frag.translate(x, y);
            block::apply_relative(&mut out.frag, st, item_cb);
            let baseline = y + baseline_of(&out);
            if baseline_item(it) && line_baseline.is_none() {
                line_baseline = Some(baseline);
            }
            if line_first.is_none() {
                line_first = Some(out.first_baseline.map_or(baseline, |b| y + b));
            }
            fragments.push(out.frag);
        }
        let b = line_baseline.or(line_first);
        if li == 0 {
            first_baseline = b;
        }
        last_baseline = b;
    }

    ContainerOut {
        fragments,
        height: if row { container_cross } else { main_size },
        first_baseline,
        last_baseline,
    }
}

/// Min- and max-content widths of a flex container's content box (§9.9.1).
pub(crate) fn intrinsic<'a>(ctx: &Ctx<'a>, bx: &'a BoxNode) -> (f32, f32) {
    let s = &bx.style;
    let contributions: Vec<(f32, f32)> = bx
        .children
        .iter()
        .filter(|c| !c.is_out_of_flow())
        .map(|c| item_contribution(ctx, c))
        .collect();
    let widest = |f: fn(&(f32, f32)) -> f32| contributions.iter().map(f).fold(0.0f32, f32::max);
    if s.flex_direction.is_row() {
        let gaps = gap(&s.column_gap, None) * contributions.len().saturating_sub(1) as f32;
        let max = contributions.iter().map(|c| c.1).sum::<f32>() + gaps;
        let min = if s.flex_wrap == FlexWrap::NoWrap {
            contributions.iter().map(|c| c.0).sum::<f32>() + gaps
        } else {
            widest(|c| c.0)
        };
        (min, max)
    } else {
        (widest(|c| c.0), widest(|c| c.1))
    }
}
