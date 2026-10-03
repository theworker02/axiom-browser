//! Grid layout (CSS Grid 1): the explicit grid with `repeat(auto-fill | auto-fit)`,
//! line-number, named-line, span and area placement, sparse and dense auto-placement
//! into implicit tracks, track sizing (fixed, percentage, intrinsic, `minmax`,
//! `fit-content` and `fr` tracks), and `justify-*` / `align-*` alignment.

use std::collections::HashSet;
use std::sync::Arc;

use axiom_style::grid::{AutoRepeat, Breadth, GridAreas, GridLine, TrackList, TrackSize};
use axiom_style::{Align, GridAutoFlow, Length};

use crate::block::{self, BlockOut, BoxMetrics, ContainerOut, Containing, Space};
use crate::flex::{distribute, gap, item_contribution, logical};
use crate::tree::{BoxKind, BoxNode};
use crate::Ctx;

/// Line numbers are clamped to this range, as the spec allows (§8.3).
const MAX_LINE: i32 = 1000;

/// One axis of the explicit grid.
struct ExplicitAxis {
    tracks: Vec<TrackSize>,
    names: Vec<Vec<Arc<str>>>,
    /// Explicit track count: the template's or the areas', whichever is larger.
    count: usize,
    /// Tracks repeated by `auto-fit`, which collapse when no item occupies them.
    auto_fit: Option<(usize, usize)>,
}

impl ExplicitAxis {
    fn new(list: Option<&TrackList>, area_tracks: usize, avail: Option<f32>, gap: f32) -> Self {
        let mut tracks = Vec::new();
        let mut names = vec![Vec::new()];
        let mut auto_fit = None;
        if let Some(l) = list {
            tracks = l.tracks.clone();
            names = l.line_names.clone();
            if let Some(r) = &l.auto_repeat {
                let count = repeat_count(l, r, avail, gap);
                let inserted: Vec<TrackSize> =
                    (0..count).flat_map(|_| r.tracks.iter().cloned()).collect();
                let k = inserted.len();
                tracks.splice(r.at..r.at, inserted);
                let at = (r.at + 1).min(names.len());
                names.splice(at..at, std::iter::repeat_with(Vec::new).take(k));
                if !r.fill {
                    auto_fit = Some((r.at, r.at + k));
                }
            }
        }
        let count = tracks.len().max(area_tracks);
        Self {
            tracks,
            names,
            count,
            auto_fit,
        }
    }

    /// Explicit lines named `name`, including the implicit `<area>-start` / `<area>-end`
    /// names of template areas.
    fn named(&self, areas: Option<&GridAreas>, rows: bool, name: &str) -> Vec<i32> {
        let mut lines: Vec<i32> = self
            .names
            .iter()
            .enumerate()
            .filter(|(_, n)| n.iter().any(|x| &**x == name))
            .map(|(i, _)| i as i32)
            .collect();
        for a in areas.map_or(&[][..], |a| &a.areas) {
            let (start, end) = if rows {
                (a.row_start, a.row_end)
            } else {
                (a.column_start, a.column_end)
            };
            if name.strip_suffix("-start") == Some(&*a.name) {
                lines.push(start as i32);
            }
            if name.strip_suffix("-end") == Some(&*a.name) {
                lines.push(end as i32);
            }
        }
        lines.sort_unstable();
        lines.dedup();
        lines
    }
}

/// Repetitions of an `auto-fill` / `auto-fit` repeat (§7.2.3.2): as many as fit without
/// overflowing, at least one.
fn repeat_count(l: &TrackList, r: &AutoRepeat, avail: Option<f32>, gap: f32) -> usize {
    let Some(avail) = avail else {
        return 1;
    };
    let fixed = |b: &Breadth| match b {
        Breadth::Length(l) => l.resolve(avail),
        _ => None,
    };
    let size = |t: &TrackSize| fixed(&t.max).or_else(|| fixed(&t.min));
    let others: f32 = l.tracks.iter().map(|t| size(t).unwrap_or(0.0)).sum();
    let Some(repeated) = r
        .tracks
        .iter()
        .map(size)
        .sum::<Option<f32>>()
        .filter(|v| *v > 0.0)
    else {
        return 1;
    };
    let per = repeated + gap * r.tracks.len() as f32;
    let base = others + gap * (l.tracks.len() as f32 - 1.0);
    ((avail - base) / per).floor().clamp(1.0, MAX_LINE as f32) as usize
}

/// The `index`th line named by `lines` (negative: from the end), where every implicit
/// line counts as having the name when the explicit grid has too few (§8.3).
fn nth_named(lines: &[i32], index: i32, explicit: i32) -> i32 {
    let c = lines.len() as i32;
    if index > 0 {
        if index <= c {
            lines[(index - 1) as usize]
        } else {
            explicit + (index - c)
        }
    } else {
        let k = -index;
        if k <= c {
            lines[(c - k) as usize]
        } else {
            -(k - c)
        }
    }
}

struct Resolver<'s> {
    axis: &'s ExplicitAxis,
    areas: Option<&'s GridAreas>,
    rows: bool,
}

impl Resolver<'_> {
    fn named(&self, name: &str) -> Vec<i32> {
        self.axis.named(self.areas, self.rows, name)
    }

    fn line(&self, l: &GridLine, side: &str) -> Option<i32> {
        let explicit = self.axis.count as i32;
        let line = match l {
            GridLine::Line { index, name: None } => {
                if *index > 0 {
                    index - 1
                } else {
                    explicit + 1 + index
                }
            }
            GridLine::Line {
                index,
                name: Some(n),
            } => nth_named(&self.named(n), *index, explicit),
            GridLine::Name(n) => match self.named(&format!("{n}-{side}")).first() {
                Some(&l) => l,
                None => nth_named(&self.named(n), 1, explicit),
            },
            _ => return None,
        };
        Some(line.clamp(-MAX_LINE, MAX_LINE))
    }

    /// How far a `span` reaches from line `from` (forwards for an end line).
    fn span(&self, l: &GridLine, from: i32, forward: bool) -> i32 {
        let explicit = self.axis.count as i32;
        let reach = match l {
            GridLine::Span {
                count,
                name: Some(n),
            } => {
                let count = *count as i32;
                let lines = self.named(n);
                if forward {
                    let after: Vec<i32> = lines.into_iter().filter(|&x| x > from).collect();
                    let target = match after.get((count - 1) as usize) {
                        Some(&t) => t,
                        None => explicit.max(from) + (count - after.len() as i32),
                    };
                    target - from
                } else {
                    let before: Vec<i32> = lines.into_iter().filter(|&x| x < from).rev().collect();
                    let target = match before.get((count - 1) as usize) {
                        Some(&t) => t,
                        None => from.min(0) - (count - before.len() as i32),
                    };
                    from - target
                }
            }
            GridLine::Span { count, name: None } => *count as i32,
            _ => 1,
        };
        reach.clamp(1, MAX_LINE)
    }

    /// Definite lines in explicit-grid coordinates, or the span of an auto-placed item.
    fn resolve(&self, start: &GridLine, end: &GridLine) -> (Option<(i32, i32)>, usize) {
        match (self.line(start, "start"), self.line(end, "end")) {
            (Some(a), Some(b)) if a == b => (Some((a, a + 1)), 1),
            (Some(a), Some(b)) => (Some((a.min(b), a.max(b))), (a - b).unsigned_abs() as usize),
            (Some(a), None) => {
                let n = self.span(end, a, true);
                (Some((a, a + n)), n as usize)
            }
            (None, Some(b)) => {
                let n = self.span(start, b, false);
                (Some((b - n, b)), n as usize)
            }
            (None, None) => {
                let span = match (start, end) {
                    (GridLine::Span { count, .. }, _) | (_, GridLine::Span { count, .. }) => {
                        (*count as i32).clamp(1, MAX_LINE) as usize
                    }
                    _ => 1,
                };
                (None, span)
            }
        }
    }
}

struct GridItem<'a> {
    bx: &'a BoxNode,
    /// Track ranges `[columns, rows]`, set once placed.
    area: [(usize, usize); 2],
}

#[derive(Clone)]
struct Track {
    size: TrackSize,
    base: f32,
    limit: f32,
    collapsed: bool,
}

struct Grid<'a> {
    items: Vec<GridItem<'a>>,
    columns: Vec<Track>,
    rows: Vec<Track>,
    column_gap: f32,
    row_gap: f32,
}

/// The size of track `i` (explicit-grid coordinates): the template's, else the
/// `grid-auto-*` pattern, which repeats forwards after the explicit grid and backwards
/// before it (§7.6).
fn track_size(i: i32, axis: &ExplicitAxis, auto: Option<&[TrackSize]>) -> TrackSize {
    if i >= 0 && (i as usize) < axis.tracks.len() {
        return axis.tracks[i as usize].clone();
    }
    match auto {
        Some(a) if !a.is_empty() => {
            let n = a.len() as i32;
            let k = if i >= 0 {
                (i - axis.tracks.len() as i32).rem_euclid(n)
            } else {
                i.rem_euclid(n)
            };
            a[k as usize].clone()
        }
        _ => TrackSize::auto(),
    }
}

/// An item's track ranges `[columns, rows]` (`None`: auto-placed) and spans.
type Slot = ([Option<(usize, usize)>; 2], [usize; 2]);

/// Auto-placement (§8.5).
fn place(slots: &mut [Slot], explicit: [usize; 2], flow: GridAutoFlow) -> [usize; 2] {
    // The minor axis is the one the cursor walks along; the major axis grows.
    let (minor, major) = if flow.column { (1, 0) } else { (0, 1) };
    let mut occupied: HashSet<(usize, usize)> = HashSet::new();
    let fits = |occ: &HashSet<(usize, usize)>, maj: (usize, usize), min: (usize, usize)| {
        (maj.0..maj.1).all(|a| (min.0..min.1).all(|b| !occ.contains(&(a, b))))
    };
    let mark = |occ: &mut HashSet<(usize, usize)>, maj: (usize, usize), min: (usize, usize)| {
        for a in maj.0..maj.1 {
            for b in min.0..min.1 {
                occ.insert((a, b));
            }
        }
    };
    for (area, _) in slots.iter() {
        if let (Some(maj), Some(min)) = (area[major], area[minor]) {
            mark(&mut occupied, maj, min);
        }
    }
    // Items locked to a major track.
    let mut cursors: std::collections::HashMap<usize, usize> = Default::default();
    for (area, span) in slots.iter_mut() {
        if let (Some(maj), None) = (area[major], area[minor]) {
            let mut c = if flow.dense {
                0
            } else {
                cursors.get(&maj.0).copied().unwrap_or(0)
            };
            while !fits(&occupied, maj, (c, c + span[minor])) {
                c += 1;
            }
            area[minor] = Some((c, c + span[minor]));
            mark(&mut occupied, maj, (c, c + span[minor]));
            cursors.insert(maj.0, c + span[minor]);
        }
    }
    let minor_count = slots
        .iter()
        .map(|(area, span)| area[minor].map_or(span[minor], |m| m.1))
        .fold(explicit[minor], usize::max);
    let (mut cur_maj, mut cur_min) = (0usize, 0usize);
    for (area, span) in slots.iter_mut() {
        if area[major].is_some() {
            continue;
        }
        let span_maj = span[major];
        if let Some((c0, c1)) = area[minor] {
            if flow.dense {
                cur_maj = 0;
            } else if c0 < cur_min {
                cur_maj += 1;
            }
            cur_min = c0;
            while !fits(&occupied, (cur_maj, cur_maj + span_maj), (c0, c1)) {
                cur_maj += 1;
            }
            area[major] = Some((cur_maj, cur_maj + span_maj));
            mark(&mut occupied, (cur_maj, cur_maj + span_maj), (c0, c1));
        } else {
            let span_min = span[minor];
            if flow.dense {
                cur_maj = 0;
                cur_min = 0;
            }
            loop {
                if cur_min + span_min > minor_count {
                    cur_maj += 1;
                    cur_min = 0;
                    continue;
                }
                if fits(
                    &occupied,
                    (cur_maj, cur_maj + span_maj),
                    (cur_min, cur_min + span_min),
                ) {
                    break;
                }
                cur_min += 1;
            }
            area[major] = Some((cur_maj, cur_maj + span_maj));
            area[minor] = Some((cur_min, cur_min + span_min));
            mark(
                &mut occupied,
                (cur_maj, cur_maj + span_maj),
                (cur_min, cur_min + span_min),
            );
            cur_min += span_min;
        }
    }
    let mut counts = explicit;
    for (area, _) in slots.iter() {
        for axis in 0..2 {
            counts[axis] = counts[axis].max(area[axis].map_or(0, |a| a.1));
        }
    }
    counts
}

/// Resolve the explicit grid and place the in-flow children.
fn build<'a>(bx: &'a BoxNode, width: Option<f32>, height: Option<f32>) -> Grid<'a> {
    let s = &bx.style;
    let column_gap = gap(&s.column_gap, width);
    let row_gap = gap(&s.row_gap, height);
    let areas = s.grid_template_areas.as_deref();
    let cols = ExplicitAxis::new(
        s.grid_template_columns.as_deref(),
        areas.map_or(0, |a| a.columns),
        width,
        column_gap,
    );
    let rows = ExplicitAxis::new(
        s.grid_template_rows.as_deref(),
        areas.map_or(0, |a| a.rows),
        height,
        row_gap,
    );
    let col_resolver = Resolver {
        axis: &cols,
        areas,
        rows: false,
    };
    let row_resolver = Resolver {
        axis: &rows,
        areas,
        rows: true,
    };
    let mut children: Vec<&BoxNode> = bx.children.iter().filter(|c| !c.is_out_of_flow()).collect();
    children.sort_by_key(|c| c.style.order);
    let mut resolved = Vec::with_capacity(children.len());
    let mut lowest = [0i32; 2];
    for c in &children {
        let cs = &c.style;
        let (ca, cspan) = col_resolver.resolve(&cs.grid_column_start, &cs.grid_column_end);
        let (ra, rspan) = row_resolver.resolve(&cs.grid_row_start, &cs.grid_row_end);
        for (axis, a) in [(0, ca), (1, ra)] {
            if let Some((start, _)) = a {
                lowest[axis] = lowest[axis].min(start);
            }
        }
        resolved.push(([ca, ra], [cspan, rspan]));
    }
    // Implicit tracks before the explicit grid shift every line.
    let shift = [-lowest[0], -lowest[1]];
    let mut slots: Vec<Slot> = resolved
        .iter()
        .map(|(area, span)| {
            let at = |axis: usize| {
                area[axis].map(|(a, b)| ((a + shift[axis]) as usize, (b + shift[axis]) as usize))
            };
            ([at(0), at(1)], *span)
        })
        .collect();
    let explicit = [
        cols.count + shift[0] as usize,
        rows.count + shift[1] as usize,
    ];
    let counts = place(&mut slots, explicit, s.grid_auto_flow);
    let tracks = |axis: &ExplicitAxis, n: usize, shift: i32, auto: Option<&[TrackSize]>| {
        (0..n)
            .map(|i| Track {
                size: track_size(i as i32 - shift, axis, auto),
                base: 0.0,
                limit: 0.0,
                collapsed: false,
            })
            .collect::<Vec<_>>()
    };
    let mut columns = tracks(&cols, counts[0], shift[0], s.grid_auto_columns.as_deref());
    let mut row_tracks = tracks(&rows, counts[1], shift[1], s.grid_auto_rows.as_deref());
    let items: Vec<GridItem> = children
        .into_iter()
        .zip(&slots)
        .map(|(bx, (area, _))| GridItem {
            bx,
            area: [area[0].unwrap_or((0, 1)), area[1].unwrap_or((0, 1))],
        })
        .collect();
    for (axis, fit, tracks) in [
        (0, cols.auto_fit, &mut columns),
        (1, rows.auto_fit, &mut row_tracks),
    ] {
        if let Some((a, b)) = fit {
            for i in a..b {
                let idx = i + shift[axis] as usize;
                let used = items
                    .iter()
                    .any(|it| (it.area[axis].0..it.area[axis].1).contains(&idx));
                if !used {
                    if let Some(t) = tracks.get_mut(idx) {
                        t.collapsed = true;
                    }
                }
            }
        }
    }
    Grid {
        items,
        columns,
        rows: row_tracks,
        column_gap,
        row_gap,
    }
}

#[derive(Clone, Copy)]
enum Avail {
    Definite(f32),
    MinContent,
    MaxContent,
}

/// An item's margin-box contributions to the tracks it spans.
struct Contribution {
    start: usize,
    end: usize,
    min: f32,
    max: f32,
}

/// Sum of track sizes and the gaps between tracks that have not collapsed.
fn used(tracks: &[Track], gap: f32) -> f32 {
    let visible = tracks.iter().filter(|t| !t.collapsed).count();
    tracks.iter().map(|t| t.base).sum::<f32>() + gap * visible.saturating_sub(1) as f32
}

fn fr(t: &Track) -> Option<f32> {
    match t.size.max {
        Breadth::Fr(f) if !t.collapsed => Some(f),
        _ => None,
    }
}

/// The used size of one `fr` over `idx` filling `space` (§11.7.1).
fn find_fr_size(tracks: &[Track], idx: &[usize], space: f32) -> f32 {
    let mut inflexible = vec![false; tracks.len()];
    loop {
        let (mut leftover, mut flex_sum) = (space, 0.0f32);
        for &i in idx {
            match fr(&tracks[i]) {
                Some(f) if !inflexible[i] => flex_sum += f,
                _ => leftover -= tracks[i].base,
            }
        }
        let size = leftover / flex_sum.max(1.0);
        let mut changed = false;
        for &i in idx {
            if let Some(f) = fr(&tracks[i]) {
                if !inflexible[i] && size * f < tracks[i].base {
                    inflexible[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return size.max(0.0);
        }
    }
}

/// The track sizing algorithm (§11.3–11.8) for one axis.
fn size_tracks(tracks: &mut [Track], contributions: &[Contribution], avail: Avail, gap: f32) {
    let basis = match avail {
        Avail::Definite(a) => Some(a),
        _ => None,
    };
    let fixed = |b: &Breadth| match b {
        Breadth::Length(Length::Px(v)) => Some(*v),
        Breadth::Length(l) => basis.and_then(|a| l.resolve(a)),
        _ => None,
    };
    // A percentage against an indefinite size behaves as `auto`.
    let min_intrinsic = |t: &Track| {
        !t.collapsed
            && (t.size.min.is_intrinsic()
                || (matches!(t.size.min, Breadth::Length(_)) && fixed(&t.size.min).is_none()))
    };
    let max_intrinsic = |t: &Track| {
        !t.collapsed
            && (t.size.max.is_intrinsic()
                || (matches!(t.size.max, Breadth::Length(_)) && fixed(&t.size.max).is_none()))
    };
    for t in tracks.iter_mut() {
        if t.collapsed {
            t.base = 0.0;
            t.limit = 0.0;
            continue;
        }
        t.base = fixed(&t.size.min).unwrap_or(0.0).max(0.0);
        t.limit = match &t.size.max {
            b @ Breadth::Length(_) => fixed(b).unwrap_or(f32::INFINITY),
            _ => f32::INFINITY,
        };
        if t.limit < t.base {
            t.limit = t.base;
        }
    }
    let gaps_in = |span: usize| gap * span.saturating_sub(1) as f32;

    // Intrinsic contributions, narrowest spans first (§11.5). Items crossing a flexible
    // track only feed the intrinsic minimums of the flexible tracks.
    let mut order: Vec<&Contribution> = contributions.iter().collect();
    order.sort_by_key(|c| c.end - c.start);
    let mut limits: Vec<Option<f32>> = vec![None; tracks.len()];
    for c in &order {
        let range = c.start..c.end.min(tracks.len());
        if range.clone().all(|i| tracks[i].collapsed) {
            continue;
        }
        let span = range.len();
        let crosses_flex = range.clone().any(|i| fr(&tracks[i]).is_some());
        let targets: Vec<usize> = range
            .clone()
            .filter(|&i| min_intrinsic(&tracks[i]) && (!crosses_flex || fr(&tracks[i]).is_some()))
            .collect();
        if !targets.is_empty() {
            let want = if targets
                .iter()
                .all(|&i| matches!(tracks[i].size.min, Breadth::MaxContent))
            {
                c.max
            } else {
                c.min
            };
            let have: f32 = range.clone().map(|i| tracks[i].base).sum::<f32>() + gaps_in(span);
            if want > have {
                let each = (want - have) / targets.len() as f32;
                for &i in &targets {
                    tracks[i].base += each;
                }
            }
        }
        if crosses_flex {
            continue;
        }
        let targets: Vec<usize> = range
            .clone()
            .filter(|&i| max_intrinsic(&tracks[i]))
            .collect();
        if !targets.is_empty() {
            let want = if targets
                .iter()
                .all(|&i| matches!(tracks[i].size.max, Breadth::MinContent))
            {
                c.min
            } else {
                c.max
            };
            let have: f32 = range
                .clone()
                .map(|i| {
                    limits[i].unwrap_or(if tracks[i].limit.is_finite() {
                        tracks[i].limit
                    } else {
                        tracks[i].base
                    })
                })
                .sum::<f32>()
                + gaps_in(span);
            let each = (want - have).max(0.0) / targets.len() as f32;
            for &i in &targets {
                limits[i] = Some(limits[i].unwrap_or(tracks[i].base) + each);
            }
        }
    }
    for (i, t) in tracks.iter_mut().enumerate() {
        if t.collapsed {
            continue;
        }
        if max_intrinsic(t) {
            t.limit = limits[i].unwrap_or(t.base);
        }
        if let Some(arg) = t.size.fit_content.as_ref().and_then(|l| match l {
            Length::Px(v) => Some(*v),
            l => basis.and_then(|a| l.resolve(a)),
        }) {
            t.limit = t.limit.min(t.base.max(arg));
        }
        if t.limit < t.base {
            t.limit = t.base;
        }
    }

    // Maximize tracks (§11.6).
    match avail {
        Avail::Definite(a) => {
            let mut free = a - used(tracks, gap);
            loop {
                let growable: Vec<usize> = (0..tracks.len())
                    .filter(|&i| {
                        let t = &tracks[i];
                        !t.collapsed && fr(t).is_none() && t.base < t.limit - 0.001
                    })
                    .collect();
                if free <= 0.001 || growable.is_empty() {
                    break;
                }
                let each = free / growable.len() as f32;
                for i in growable {
                    let t = &mut tracks[i];
                    let add = each.min(t.limit - t.base);
                    t.base += add;
                    free -= add;
                }
            }
        }
        Avail::MaxContent => {
            for t in tracks.iter_mut() {
                if t.limit.is_finite() && fr(t).is_none() {
                    t.base = t.limit;
                }
            }
        }
        Avail::MinContent => {}
    }

    // Expand flexible tracks (§11.7).
    let flexible: Vec<usize> = (0..tracks.len())
        .filter(|&i| fr(&tracks[i]).is_some())
        .collect();
    if !flexible.is_empty() {
        let size = match avail {
            Avail::Definite(a) => {
                let all: Vec<usize> = (0..tracks.len()).collect();
                let visible = tracks.iter().filter(|t| !t.collapsed).count();
                find_fr_size(tracks, &all, a - gap * visible.saturating_sub(1) as f32)
            }
            Avail::MinContent => 0.0,
            Avail::MaxContent => {
                let mut size = 0.0f32;
                for &i in &flexible {
                    let f = fr(&tracks[i]).unwrap_or(1.0);
                    size = size.max(if f > 1.0 {
                        tracks[i].base / f
                    } else {
                        tracks[i].base
                    });
                }
                for c in contributions {
                    let idx: Vec<usize> = (c.start..c.end.min(tracks.len())).collect();
                    if idx.iter().any(|&i| fr(&tracks[i]).is_some()) {
                        size = size.max(find_fr_size(tracks, &idx, c.max - gaps_in(idx.len())));
                    }
                }
                size
            }
        };
        for i in flexible {
            let f = fr(&tracks[i]).unwrap_or(0.0);
            tracks[i].base = tracks[i].base.max(size * f);
        }
    }
}

/// Stretch `auto` tracks into positive free space (§11.8).
fn stretch_auto(tracks: &mut [Track], free: f32) {
    let autos: Vec<usize> = (0..tracks.len())
        .filter(|&i| {
            let t = &tracks[i];
            !t.collapsed && matches!(t.size.max, Breadth::Auto) && t.size.fit_content.is_none()
        })
        .collect();
    if free > 0.0 && !autos.is_empty() {
        let each = free / autos.len() as f32;
        for i in autos {
            tracks[i].base += each;
        }
    }
}

/// Track start offsets, with `lead` before the first track and `between` added to each
/// gap; collapsed tracks take no gap.
fn positions(tracks: &[Track], gap: f32, lead: f32, between: f32) -> Vec<f32> {
    let last_visible = tracks.iter().rposition(|t| !t.collapsed);
    let mut out = Vec::with_capacity(tracks.len());
    let mut x = lead;
    for (i, t) in tracks.iter().enumerate() {
        out.push(x);
        x += t.base;
        if !t.collapsed && last_visible.is_some_and(|l| i < l) {
            x += gap + between;
        }
    }
    out
}

fn span_extent(pos: &[f32], tracks: &[Track], (a, b): (usize, usize)) -> (f32, f32) {
    let start = pos[a];
    (start, pos[b - 1] + tracks[b - 1].base - start)
}

/// `justify-self` / `align-self` as start, end, center or stretch.
fn self_alignment(a: Align, replaced: bool, reversed: bool) -> Align {
    match logical(a, reversed) {
        Align::Normal if replaced => Align::Start,
        Align::Normal | Align::Stretch => Align::Stretch,
        a @ (Align::End | Align::Center) => a,
        _ => Align::Start,
    }
}

/// Leading space and extra gap for `justify-content` / `align-content` over `n` tracks.
fn content_offsets(a: Align, free: f32, n: usize, reversed: bool) -> (f32, f32) {
    match logical(a, reversed) {
        Align::Normal | Align::Stretch => (0.0, 0.0),
        a => distribute(a, free, n),
    }
}

pub(crate) fn layout<'a>(ctx: &Ctx<'a>, bx: &'a BoxNode, space: &Space) -> ContainerOut {
    let s = &bx.style;
    let mut g = build(bx, Some(space.width), space.height);
    let mut fragments = Vec::new();
    for c in bx.children.iter().filter(|c| c.is_out_of_flow()) {
        let x = if s.rtl {
            space.x + space.width
        } else {
            space.x
        };
        fragments.push(block::placeholder(ctx, c, x, space.y));
    }

    // Columns.
    let col_contrib: Vec<Contribution> = g
        .items
        .iter()
        .map(|it| {
            let (min, max) = item_contribution(ctx, it.bx);
            Contribution {
                start: it.area[0].0,
                end: it.area[0].1,
                min,
                max,
            }
        })
        .collect();
    size_tracks(
        &mut g.columns,
        &col_contrib,
        Avail::Definite(space.width),
        g.column_gap,
    );
    if matches!(s.justify_content, Align::Normal | Align::Stretch) {
        let free = space.width - used(&g.columns, g.column_gap);
        stretch_auto(&mut g.columns, free);
    }
    let visible_cols = g.columns.iter().filter(|t| !t.collapsed).count();
    let (col_lead, col_between) = content_offsets(
        s.justify_content,
        space.width - used(&g.columns, g.column_gap),
        visible_cols,
        s.rtl,
    );
    let col_pos = positions(&g.columns, g.column_gap, col_lead, col_between);

    // Item widths, then the heights rows are sized from.
    struct Sized<'a> {
        bx: &'a BoxNode,
        m: BoxMetrics,
        area: [(usize, usize); 2],
        x: f32,
        area_w: f32,
        width: f32,
        out: BlockOut,
    }
    let mut sized: Vec<Sized> = Vec::with_capacity(g.items.len());
    for it in &g.items {
        let st = &it.bx.style;
        let (x, area_w) = span_extent(&col_pos, &g.columns, it.area[0]);
        let m = BoxMetrics::new(st, area_w);
        let bp_h = m.bp_h();
        let replaced = matches!(it.bx.kind, BoxKind::Replaced(_));
        let justify = self_alignment(st.justify_self.unwrap_or(s.justify_items), replaced, s.rtl);
        let avail = (area_w - m.margin[1] - m.margin[3] - bp_h).max(0.0);
        let cb = Containing {
            width: area_w,
            height: None,
            rtl: s.rtl,
        };
        let width = match (&it.bx.kind, block::specified_width(st, area_w, bp_h)) {
            (_, Some(w)) => w,
            (BoxKind::Replaced(r), None) => block::replaced_size(it.bx, r, cb).0,
            (_, None) if justify == Align::Stretch && !m.auto_left && !m.auto_right => avail,
            (_, None) => match st.width {
                Length::MinContent => block::content_intrinsic(ctx, it.bx).0,
                Length::MaxContent => block::content_intrinsic(ctx, it.bx).1,
                _ => block::shrink_to_fit(ctx, it.bx, avail),
            },
        };
        let width = block::clamp_width(st, width, area_w, bp_h);
        let out = block::layout_item(ctx, it.bx, &m, width, None, cb);
        sized.push(Sized {
            bx: it.bx,
            m,
            area: it.area,
            x,
            area_w,
            width,
            out,
        });
    }

    // Rows.
    let row_contrib: Vec<Contribution> = sized
        .iter()
        .map(|it| {
            let h = it.out.frag.rect.height + it.m.margin[0] + it.m.margin[2];
            Contribution {
                start: it.area[1].0,
                end: it.area[1].1,
                min: h,
                max: h,
            }
        })
        .collect();
    let row_avail = space.height.map_or(Avail::MaxContent, Avail::Definite);
    size_tracks(&mut g.rows, &row_contrib, row_avail, g.row_gap);
    let total_rows = used(&g.rows, g.row_gap);
    let container_h = space
        .height
        .unwrap_or_else(|| block::clamp_height(s, total_rows, space.cb_height, space.bp_v));
    if matches!(s.align_content, Align::Normal | Align::Stretch) {
        stretch_auto(&mut g.rows, container_h - total_rows);
    }
    let total_rows = used(&g.rows, g.row_gap);
    let visible_rows = g.rows.iter().filter(|t| !t.collapsed).count();
    let (row_lead, row_between) = content_offsets(
        s.align_content,
        container_h - total_rows,
        visible_rows,
        false,
    );
    let row_pos = positions(&g.rows, g.row_gap, row_lead, row_between);

    // Place items in their areas.
    let mut first_baseline: Option<(usize, usize, f32)> = None;
    let mut last_baseline: Option<(usize, usize, f32)> = None;
    for it in sized {
        let st = &it.bx.style;
        let (area_y, area_h) = span_extent(&row_pos, &g.rows, it.area[1]);
        let replaced = matches!(it.bx.kind, BoxKind::Replaced(_));
        let bp_v = it.m.bp_v();
        let (auto_top, auto_bottom) = (st.margin.top.is_auto(), st.margin.bottom.is_auto());
        let align = self_alignment(st.align_self.unwrap_or(s.align_items), replaced, false);
        let cb = Containing {
            width: it.area_w,
            height: Some(area_h),
            rtl: s.rtl,
        };
        let stretch_h =
            (align == Align::Stretch && st.height.is_auto() && !auto_top && !auto_bottom).then(
                || {
                    let inner = (area_h - it.m.margin[0] - it.m.margin[2] - bp_v).max(0.0);
                    block::clamp_height(st, inner, Some(area_h), bp_v)
                },
            );
        let mut out = if stretch_h.is_some() || st.height.has_percent() {
            block::layout_item(ctx, it.bx, &it.m, it.width, stretch_h, cb)
        } else {
            it.out
        };

        // Inline axis, in logical coordinates mirrored for right-to-left.
        let justify = self_alignment(st.justify_self.unwrap_or(s.justify_items), replaced, s.rtl);
        let (m_start, m_end, a_start, a_end) = if s.rtl {
            (
                it.m.margin[1],
                it.m.margin[3],
                it.m.auto_right,
                it.m.auto_left,
            )
        } else {
            (
                it.m.margin[3],
                it.m.margin[1],
                it.m.auto_left,
                it.m.auto_right,
            )
        };
        let w = out.frag.rect.width;
        let free = it.area_w - w - m_start - m_end;
        let dx = if (a_start || a_end) && free > 0.0 {
            match (a_start, a_end) {
                (true, true) => free / 2.0,
                (true, false) => free,
                _ => 0.0,
            }
        } else {
            match justify {
                Align::End => free,
                Align::Center => free / 2.0,
                _ => 0.0,
            }
        };
        let logical_x = it.x + dx + m_start;
        let x = if s.rtl {
            space.x + space.width - logical_x - w
        } else {
            space.x + logical_x
        };

        let h = out.frag.rect.height;
        let free = area_h - h - it.m.margin[0] - it.m.margin[2];
        let dy = if (auto_top || auto_bottom) && free > 0.0 {
            match (auto_top, auto_bottom) {
                (true, true) => free / 2.0,
                (true, false) => free,
                _ => 0.0,
            }
        } else {
            match align {
                Align::End => free,
                Align::Center => free / 2.0,
                _ => 0.0,
            }
        };
        let y = space.y + area_y + dy + it.m.margin[0];
        out.frag.translate(x, y);
        block::apply_relative(&mut out.frag, st, cb);
        if let Some(b) = out.first_baseline {
            let key = (it.area[1].0, it.area[0].0, y + b);
            if first_baseline.is_none_or(|f| (key.0, key.1) < (f.0, f.1)) {
                first_baseline = Some(key);
            }
            if last_baseline.is_none_or(|l| (key.0, key.1) >= (l.0, l.1)) {
                last_baseline = Some(key);
            }
        }
        fragments.push(out.frag);
    }

    ContainerOut {
        fragments,
        height: space.height.map_or(container_h, |_| total_rows),
        first_baseline: first_baseline.map(|b| b.2),
        last_baseline: last_baseline.map(|b| b.2),
    }
}

/// Min- and max-content widths of a grid container's content box (§11.1): the column
/// sizes under min- and max-content constraints.
pub(crate) fn intrinsic<'a>(ctx: &Ctx<'a>, bx: &'a BoxNode) -> (f32, f32) {
    let g = build(bx, None, None);
    let contributions: Vec<Contribution> = g
        .items
        .iter()
        .map(|it| {
            let (min, max) = item_contribution(ctx, it.bx);
            Contribution {
                start: it.area[0].0,
                end: it.area[0].1,
                min,
                max,
            }
        })
        .collect();
    let size = |avail: Avail| {
        let mut cols = g.columns.clone();
        size_tracks(&mut cols, &contributions, avail, g.column_gap);
        used(&cols, g.column_gap)
    };
    (size(Avail::MinContent), size(Avail::MaxContent))
}
