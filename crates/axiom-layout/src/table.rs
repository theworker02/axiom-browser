//! Table layout (CSS 2 §17) in the separated borders model: a slot grid with column
//! and row spans, auto (or fixed) column widths, baseline / `vertical-align` placement
//! of cell content, row and row-group background boxes, and top captions.
//! `border-collapse: collapse` only removes the spacing between cells.

use std::sync::Arc;

use axiom_style::{BorderStyle, ComputedStyle, Display, Length, VerticalAlign};

use crate::block::{self, BoxMetrics, Containing};
use crate::tree::BoxNode;
use crate::{BoxEdges, Ctx, Fragment, FragmentKind, Rect};

pub(crate) fn is_table(d: Display) -> bool {
    matches!(d, Display::Table | Display::InlineTable)
}

struct Cell<'a> {
    bx: &'a BoxNode,
    row: usize,
    col: usize,
    colspan: usize,
    rowspan: usize,
}

struct Row<'a> {
    bx: &'a BoxNode,
    /// Index into `Grid::groups`, if the row is inside a row group box.
    group: Option<usize>,
}

struct Grid<'a> {
    captions: Vec<&'a BoxNode>,
    groups: Vec<&'a BoxNode>,
    rows: Vec<Row<'a>>,
    cells: Vec<Cell<'a>>,
    cols: usize,
    out_of_flow: Vec<&'a BoxNode>,
}

fn build_grid(table: &BoxNode) -> Grid<'_> {
    let mut grid = Grid {
        captions: Vec::new(),
        groups: Vec::new(),
        rows: Vec::new(),
        cells: Vec::new(),
        cols: 0,
        out_of_flow: Vec::new(),
    };
    // Header groups render first and footer groups last, whatever their source order.
    let rank = |c: &BoxNode| match c.style.display {
        Display::TableHeaderGroup => 0,
        Display::TableFooterGroup => 2,
        _ => 1,
    };
    let mut parts: Vec<&BoxNode> = Vec::new();
    for c in &table.children {
        if c.is_out_of_flow() {
            grid.out_of_flow.push(c);
        } else if c.style.display == Display::TableCaption {
            grid.captions.push(c);
        } else {
            parts.push(c);
        }
    }
    parts.sort_by_key(|c| rank(c));
    // Occupied slots of rows still spanned from above, per row group.
    for part in parts {
        let (group, rows): (Option<usize>, Vec<&BoxNode>) =
            if part.style.display == Display::TableRow {
                (None, vec![part])
            } else {
                grid.groups.push(part);
                let rows = part
                    .children
                    .iter()
                    .filter(|r| {
                        if r.is_out_of_flow() {
                            grid.out_of_flow.push(r);
                            false
                        } else {
                            true
                        }
                    })
                    .collect();
                (Some(grid.groups.len() - 1), rows)
            };
        let first = grid.rows.len();
        let mut occupied: Vec<Vec<bool>> = vec![Vec::new(); rows.len()];
        for (ri, row) in rows.iter().enumerate() {
            grid.rows.push(Row { bx: row, group });
            let mut col = 0;
            for cell in &row.children {
                if cell.is_out_of_flow() {
                    grid.out_of_flow.push(cell);
                    continue;
                }
                while occupied[ri].get(col).copied().unwrap_or(false) {
                    col += 1;
                }
                let colspan = cell.colspan.max(1) as usize;
                let rowspan = (cell.rowspan.max(1) as usize).min(rows.len() - ri);
                for occ in occupied.iter_mut().skip(ri).take(rowspan) {
                    if occ.len() < col + colspan {
                        occ.resize(col + colspan, false);
                    }
                    for slot in &mut occ[col..col + colspan] {
                        *slot = true;
                    }
                }
                grid.cells.push(Cell {
                    bx: cell,
                    row: first + ri,
                    col,
                    colspan,
                    rowspan,
                });
                col += colspan;
                grid.cols = grid.cols.max(col);
            }
        }
    }
    grid
}

fn spacing(s: &ComputedStyle) -> f32 {
    if s.border_collapse {
        0.0
    } else {
        s.border_spacing
    }
}

/// Cells ignore margins.
fn cell_metrics(cell: &BoxNode, table_w: f32) -> BoxMetrics {
    let mut m = BoxMetrics::new(&cell.style, table_w);
    m.margin = [0.0; 4];
    m.auto_left = false;
    m.auto_right = false;
    m
}

/// Border-box min/max widths of a cell (a fixed `width` is also its maximum).
fn cell_widths<'a>(ctx: &Ctx<'a>, cell: &'a BoxNode) -> (f32, f32) {
    let m = cell_metrics(cell, 0.0);
    let bp = m.bp_h();
    let (mn, mx) = block::content_intrinsic(ctx, cell);
    let (mut mn, mut mx) = (mn + bp, mx + bp);
    if let Length::Px(w) = cell.style.width {
        let w = if cell.style.border_box_sizing {
            w
        } else {
            w + bp
        };
        mn = mn.max(w);
        mx = w.max(mn);
    }
    (mn, mx)
}

/// Border-box min/max widths of each column.
fn column_widths<'a>(ctx: &Ctx<'a>, grid: &Grid<'a>, sp: f32) -> (Vec<f32>, Vec<f32>) {
    let mut min = vec![0.0f32; grid.cols];
    let mut max = vec![0.0f32; grid.cols];
    let mut spanning: Vec<&Cell> = grid.cells.iter().filter(|c| c.colspan > 1).collect();
    for c in grid.cells.iter().filter(|c| c.colspan == 1) {
        let (mn, mx) = cell_widths(ctx, c.bx);
        min[c.col] = min[c.col].max(mn);
        max[c.col] = max[c.col].max(mx);
    }
    spanning.sort_by_key(|c| c.colspan);
    for c in spanning {
        let (mn, mx) = cell_widths(ctx, c.bx);
        let cols = c.col..c.col + c.colspan;
        let gaps = sp * (c.colspan - 1) as f32;
        for (widths, need) in [(&mut min, mn), (&mut max, mx)] {
            let have: f32 = widths[cols.clone()].iter().sum::<f32>() + gaps;
            if need > have {
                let extra = (need - have) / c.colspan as f32;
                for w in &mut widths[cols.clone()] {
                    *w += extra;
                }
            }
        }
    }
    for (mn, mx) in min.iter().zip(max.iter_mut()) {
        *mx = mx.max(*mn);
    }
    (min, max)
}

const TABLE_MAX_WIDTH: f32 = 1_000_000.0;

/// Percentage widths of columns, from their single-column cells. Percentages past a
/// running total of 100% are cut back, as browsers do.
fn column_percents(grid: &Grid) -> Vec<Option<f32>> {
    let mut pct = vec![None::<f32>; grid.cols];
    for c in grid.cells.iter().filter(|c| c.colspan == 1) {
        if let Length::Percent(p) = c.bx.style.width {
            let slot = pct[c.col].get_or_insert(0.0);
            *slot = slot.max(p);
        }
    }
    let mut total = 0.0;
    for p in pct.iter_mut().flatten() {
        *p = p.min(100.0 - total).max(0.0);
        total += *p;
    }
    pct
}

/// Max-content width of the columns once percentage columns get their share: a 25%
/// column with a 100px maximum needs a 400px table, and the other columns need the
/// rest of 100% to hold their own maximums.
fn percent_max_width(max: &[f32], pct: &[Option<f32>]) -> f32 {
    let sum: f32 = max.iter().sum();
    let pct_total: f32 = pct.iter().flatten().sum();
    if pct_total <= 0.0 {
        return sum;
    }
    let mut width = sum;
    for (m, p) in max.iter().zip(pct) {
        if let Some(p) = p.filter(|p| *p > 0.0) {
            width = width.max(m * 100.0 / p);
        }
    }
    let rest: f32 = max
        .iter()
        .zip(pct)
        .filter(|(_, p)| p.is_none())
        .map(|(m, _)| m)
        .sum();
    if pct_total < 100.0 {
        width = width.max(rest * 100.0 / (100.0 - pct_total));
    } else if rest > 0.0 {
        // No share is left for the auto columns: browsers take all the width they can.
        width = TABLE_MAX_WIDTH;
    }
    width
}

fn grid_spacing(cols: usize, sp: f32) -> f32 {
    if cols == 0 {
        0.0
    } else {
        sp * (cols + 1) as f32
    }
}

/// Min- and max-content widths of a table's content box.
pub(crate) fn intrinsic<'a>(ctx: &Ctx<'a>, table: &'a BoxNode) -> (f32, f32) {
    let grid = build_grid(table);
    let sp = spacing(&table.style);
    let (min, max) = column_widths(ctx, &grid, sp);
    let gaps = grid_spacing(grid.cols, sp);
    let mut mn = min.iter().sum::<f32>() + gaps;
    let mx = percent_max_width(&max, &column_percents(&grid)) + gaps;
    for cap in &grid.captions {
        mn = mn.max(block::outer_intrinsic(ctx, cap).0);
    }
    (mn, mx.max(mn))
}

/// Column widths for a table content box `width` wide.
fn distribute<'a>(
    ctx: &Ctx<'a>,
    table: &'a BoxNode,
    grid: &Grid<'a>,
    width: f32,
    sp: f32,
) -> Vec<f32> {
    let avail = (width - grid_spacing(grid.cols, sp)).max(0.0);
    let n = grid.cols;
    if table.style.table_layout_fixed && !table.style.width.is_auto() {
        // Fixed layout: widths come from the first row's cells.
        let mut widths: Vec<Option<f32>> = vec![None; n];
        for c in grid.cells.iter().filter(|c| c.row == 0) {
            if let Length::Px(w) = c.bx.style.width {
                let bp = cell_metrics(c.bx, width).bp_h();
                let w = if c.bx.style.border_box_sizing {
                    w
                } else {
                    w + bp
                };
                let per = (w - sp * (c.colspan - 1) as f32) / c.colspan as f32;
                for slot in &mut widths[c.col..c.col + c.colspan] {
                    *slot = Some(per.max(0.0));
                }
            }
        }
        let assigned: f32 = widths.iter().flatten().sum();
        let free = widths.iter().filter(|w| w.is_none()).count();
        let rest = (avail - assigned).max(0.0);
        let mut out: Vec<f32> = widths
            .iter()
            .map(|w| w.unwrap_or(if free > 0 { rest / free as f32 } else { 0.0 }))
            .collect();
        if free == 0 && n > 0 && avail > assigned {
            let extra = (avail - assigned) / n as f32;
            out.iter_mut().for_each(|w| *w += extra);
        }
        return out;
    }
    let (min, max) = column_widths(ctx, grid, sp);
    let pct = column_percents(grid);
    if pct.iter().any(Option::is_some) {
        return distribute_percent(&min, &max, &pct, avail);
    }
    let (sum_min, sum_max): (f32, f32) = (min.iter().sum(), max.iter().sum());
    if avail >= sum_max {
        let extra = avail - sum_max;
        max.iter()
            .map(|&w| {
                if sum_max > 0.0 {
                    w + extra * w / sum_max
                } else {
                    extra / n.max(1) as f32
                }
            })
            .collect()
    } else if avail > sum_min {
        let t = (avail - sum_min) / (sum_max - sum_min);
        min.iter().zip(&max).map(|(a, b)| a + (b - a) * t).collect()
    } else {
        min
    }
}

/// Auto layout with percentage columns: they take their share of `avail` (never below
/// their minimum) first; the other columns grow from minimum toward maximum with what
/// is left, and any surplus goes to them, or to the percentage columns if there are none.
fn distribute_percent(min: &[f32], max: &[f32], pct: &[Option<f32>], avail: f32) -> Vec<f32> {
    let mut out: Vec<f32> = min
        .iter()
        .zip(pct)
        .map(|(&mn, p)| p.map_or(mn, |p| mn.max(avail * p / 100.0)))
        .collect();
    let pct_used: f32 = out
        .iter()
        .zip(pct)
        .filter(|(_, p)| p.is_some())
        .map(|(w, _)| w)
        .sum();
    let auto: Vec<usize> = (0..out.len()).filter(|&i| pct[i].is_none()).collect();
    let auto_min: f32 = auto.iter().map(|&i| min[i]).sum();
    let auto_max: f32 = auto.iter().map(|&i| max[i]).sum();
    let left = avail - pct_used;
    if left < auto_min {
        // Too narrow: shrink the percentage columns toward their minimums.
        let over = auto_min - left;
        let give: f32 = out
            .iter()
            .zip(min)
            .zip(pct)
            .filter(|(_, p)| p.is_some())
            .map(|((w, m), _)| w - m)
            .sum();
        if give > 0.0 {
            let t = (over / give).min(1.0);
            for ((w, &m), p) in out.iter_mut().zip(min).zip(pct) {
                if p.is_some() {
                    *w -= (*w - m) * t;
                }
            }
        }
    } else if left <= auto_max {
        let t = if auto_max > auto_min {
            (left - auto_min) / (auto_max - auto_min)
        } else {
            0.0
        };
        for &i in &auto {
            out[i] = min[i] + (max[i] - min[i]) * t;
        }
    } else if !auto.is_empty() {
        let extra = left - auto_max;
        for &i in &auto {
            out[i] = max[i]
                + if auto_max > 0.0 {
                    extra * max[i] / auto_max
                } else {
                    extra / auto.len() as f32
                };
        }
    } else if pct_used > 0.0 {
        let extra = left;
        for w in &mut out {
            *w += extra * *w / pct_used;
        }
    }
    out
}

/// A row or row-group background box: its borders, padding and margins do not apply.
fn background_style(s: &Arc<ComputedStyle>) -> Arc<ComputedStyle> {
    if s.border_widths().iter().all(|&w| w == 0.0) {
        return s.clone();
    }
    let mut st = (**s).clone();
    for side in &mut st.border {
        side.style = BorderStyle::None;
    }
    Arc::new(st)
}

pub(crate) struct TableOut {
    pub fragments: Vec<Fragment>,
    pub height: f32,
    pub first_baseline: Option<f32>,
}

/// Lays out the table grid inside the content box at (`content_x`, `content_y`).
/// `min_height` is the table's specified content height, if any.
pub(crate) fn layout<'a>(
    ctx: &Ctx<'a>,
    table: &'a BoxNode,
    content_x: f32,
    content_y: f32,
    width: f32,
    min_height: Option<f32>,
) -> TableOut {
    let grid = build_grid(table);
    let sp = spacing(&table.style);
    let mut fragments = Vec::new();

    // Captions (all on top).
    let mut y = content_y;
    for cap in &grid.captions {
        let out = block::layout_block_level(
            ctx,
            cap,
            Containing {
                width,
                height: None,
                rtl: table.style.rtl,
            },
            content_x,
            false,
            None,
        );
        let mut frag = out.frag;
        frag.translate(0.0, y + out.margin[0]);
        y += out.margin[0] + frag.rect.height + out.margin[2];
        fragments.push(frag);
    }
    let grid_top = y;

    let cols = distribute(ctx, table, &grid, width, sp);
    let mut col_x = Vec::with_capacity(grid.cols + 1);
    let mut x = content_x + sp;
    for w in &cols {
        col_x.push(x);
        x += w + sp;
    }
    col_x.push(x);
    let span_w =
        |c: &Cell| cols[c.col..c.col + c.colspan].iter().sum::<f32>() + sp * (c.colspan - 1) as f32;

    // Lay out every cell at its width, then size rows.
    struct Laid {
        frag: Fragment,
        height: f32,
        baseline: f32,
    }
    let laid: Vec<Laid> = grid
        .cells
        .iter()
        .map(|c| {
            let m = cell_metrics(c.bx, width);
            let content_w = (span_w(c) - m.bp_h()).max(0.0);
            let out = block::layout_box(ctx, c.bx, &m, content_w, None, col_x[c.col], false, None);
            let height = out.frag.rect.height.max(out.auto_height);
            // Without line boxes the baseline is the bottom of the content box.
            let baseline = out
                .first_baseline
                .unwrap_or(height - m.padding[2] - m.border[2]);
            Laid {
                frag: out.frag,
                height,
                baseline,
            }
        })
        .collect();

    let n_rows = grid.rows.len();
    let mut row_h = vec![0.0f32; n_rows];
    let mut row_baseline = vec![None::<f32>; n_rows];
    for (r, row) in grid.rows.iter().enumerate() {
        if let Length::Px(h) = row.bx.style.height {
            row_h[r] = h;
        }
    }
    let baseline_aligned =
        |c: &Cell| c.rowspan == 1 && c.bx.style.vertical_align == VerticalAlign::Baseline;
    for (c, l) in grid.cells.iter().zip(&laid) {
        if baseline_aligned(c) {
            let b = row_baseline[c.row].get_or_insert(l.baseline);
            *b = b.max(l.baseline);
        }
    }
    for (c, l) in grid.cells.iter().zip(&laid) {
        if c.rowspan != 1 {
            continue;
        }
        let need = match row_baseline[c.row] {
            Some(b) if baseline_aligned(c) => b - l.baseline + l.height,
            _ => l.height,
        };
        row_h[c.row] = row_h[c.row].max(need);
    }
    for (c, l) in grid.cells.iter().zip(&laid) {
        if c.rowspan > 1 {
            let rows = c.row..c.row + c.rowspan;
            let have = row_h[rows.clone()].iter().sum::<f32>() + sp * (c.rowspan - 1) as f32;
            if l.height > have {
                row_h[rows.end - 1] += l.height - have;
            }
        }
    }
    let rows_total = row_h.iter().sum::<f32>() + grid_spacing(n_rows, sp);
    if let Some(h) = min_height {
        let grid_h = h - (grid_top - content_y);
        if grid_h > rows_total && n_rows > 0 {
            let extra = (grid_h - rows_total) / n_rows as f32;
            row_h.iter_mut().for_each(|h| *h += extra);
        }
    }
    let mut row_y = Vec::with_capacity(n_rows + 1);
    let mut y = grid_top + if n_rows > 0 { sp } else { 0.0 };
    for h in &row_h {
        row_y.push(y);
        y += h + sp;
    }
    let grid_bottom = if n_rows > 0 { y } else { grid_top };

    // Cells into their rows.
    let mut row_cells: Vec<Vec<Fragment>> = (0..n_rows).map(|_| Vec::new()).collect();
    for (c, l) in grid.cells.iter().zip(laid) {
        let h: f32 =
            row_h[c.row..c.row + c.rowspan].iter().sum::<f32>() + sp * (c.rowspan - 1) as f32;
        let offset = match c.bx.style.vertical_align {
            VerticalAlign::Baseline if c.rowspan == 1 => {
                row_baseline[c.row].map_or(0.0, |b| b - l.baseline)
            }
            VerticalAlign::Middle => (h - l.height) / 2.0,
            VerticalAlign::Bottom => h - l.height,
            _ => 0.0,
        }
        .max(0.0);
        let mut frag = l.frag;
        if offset != 0.0 {
            for child in &mut frag.children {
                child.translate(0.0, offset);
            }
        }
        frag.rect.height = h;
        frag.translate(0.0, row_y[c.row]);
        row_cells[c.row].push(frag);
    }

    let (left, right) = (
        col_x[0],
        col_x[grid.cols] - if grid.cols > 0 { sp } else { 0.0 },
    );
    let mut rows: Vec<(Option<usize>, Fragment)> = Vec::with_capacity(n_rows);
    for (r, (row, cells)) in grid.rows.iter().zip(row_cells).enumerate() {
        rows.push((
            row.group,
            Fragment {
                node: row.bx.node,
                style: background_style(&row.bx.style),
                rect: Rect::new(left, row_y[r], (right - left).max(0.0), row_h[r]),
                kind: FragmentKind::Box(BoxEdges::BOTH),
                children: cells,
            },
        ));
    }
    // Rows into their groups (a group's rows are contiguous).
    let mut group_frags: Vec<Option<Fragment>> = grid
        .groups
        .iter()
        .map(|g| {
            Some(Fragment {
                node: g.node,
                style: background_style(&g.style),
                rect: Rect::new(left, 0.0, (right - left).max(0.0), 0.0),
                kind: FragmentKind::Box(BoxEdges::BOTH),
                children: Vec::new(),
            })
        })
        .collect();
    let mut order: Vec<Result<usize, Fragment>> = Vec::new();
    for (group, frag) in rows {
        match group {
            Some(g) => {
                let gf = group_frags[g].as_mut().unwrap();
                if gf.children.is_empty() {
                    gf.rect.y = frag.rect.y;
                    order.push(Ok(g));
                }
                gf.rect.height = frag.rect.bottom() - gf.rect.y;
                gf.children.push(frag);
            }
            None => order.push(Err(frag)),
        }
    }
    for item in order {
        match item {
            Ok(g) => fragments.extend(group_frags[g].take()),
            Err(frag) => fragments.push(frag),
        }
    }
    for bx in grid.out_of_flow {
        fragments.push(block::placeholder(ctx, bx, content_x, grid_top));
    }

    let first_baseline = row_baseline
        .first()
        .copied()
        .flatten()
        .zip(row_y.first())
        .map(|(b, y)| y + b);
    TableOut {
        fragments,
        height: grid_bottom - content_y,
        first_baseline,
    }
}
