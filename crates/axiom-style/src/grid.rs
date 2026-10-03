//! Grid track lists, template areas and line placement (CSS Grid 1 §7–8), parsed when
//! the value is computed so lengths inside them are already resolved against font and
//! viewport units. Layout expands auto-repeats once it knows the container size.

use std::sync::Arc;

use crate::values::{parse_length, Length, UnitContext};

/// One end of a track sizing function.
#[derive(Debug, Clone, PartialEq)]
pub enum Breadth {
    /// A length or percentage (`Px`, `Percent` or `Calc`).
    Length(Length),
    Fr(f32),
    Auto,
    MinContent,
    MaxContent,
}

impl Breadth {
    pub fn is_intrinsic(&self) -> bool {
        matches!(
            self,
            Breadth::Auto | Breadth::MinContent | Breadth::MaxContent
        )
    }
}

/// A track sizing function: `minmax(min, max)`, with a single breadth `b` meaning
/// `minmax(b, b)` (`minmax(auto, b)` for flexible `b`).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackSize {
    pub min: Breadth,
    pub max: Breadth,
    /// `fit-content(limit)`: `minmax(auto, max-content)` clamped to the limit.
    pub fit_content: Option<Length>,
}

impl TrackSize {
    pub fn auto() -> Self {
        Self {
            min: Breadth::Auto,
            max: Breadth::Auto,
            fit_content: None,
        }
    }

    fn single(b: Breadth) -> Self {
        let min = match b {
            Breadth::Fr(_) => Breadth::Auto,
            ref other => other.clone(),
        };
        Self {
            min,
            max: b,
            fit_content: None,
        }
    }
}

/// `repeat(auto-fill | auto-fit, …)`.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoRepeat {
    /// `auto-fill`; `false` for `auto-fit`, which collapses repetitions left empty.
    pub fill: bool,
    pub tracks: Vec<TrackSize>,
    /// Index in [`TrackList::tracks`] the repetitions are inserted at.
    pub at: usize,
}

/// `grid-template-columns` / `grid-template-rows`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackList {
    pub tracks: Vec<TrackSize>,
    /// Names of the lines of `tracks` (`tracks.len() + 1` entries). Lines inside an
    /// auto-repeat are unnamed.
    pub line_names: Vec<Vec<Arc<str>>>,
    pub auto_repeat: Option<AutoRepeat>,
}

/// `grid-template-areas`: each named area as 0-based line numbers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GridAreas {
    pub rows: usize,
    pub columns: usize,
    pub areas: Vec<GridArea>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GridArea {
    pub name: Arc<str>,
    pub row_start: usize,
    pub row_end: usize,
    pub column_start: usize,
    pub column_end: usize,
}

/// `grid-row-start` / `grid-row-end` / `grid-column-start` / `grid-column-end`.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum GridLine {
    #[default]
    Auto,
    /// `<integer> <custom-ident>?` (never 0).
    Line { index: i32, name: Option<Arc<str>> },
    /// `span <integer>? <custom-ident>?`.
    Span { count: u32, name: Option<Arc<str>> },
    /// A bare `<custom-ident>`: an area's implicit line or a named line.
    Name(Arc<str>),
}

/// `grid-auto-flow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GridAutoFlow {
    pub column: bool,
    pub dense: bool,
}

/// Whitespace-separated components, keeping `[names]`, `"strings"` and `f(…)` whole.
fn components(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut quote, mut start) = (0i32, None::<char>, None::<usize>);
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                c if c.is_whitespace() && depth == 0 => {
                    if let Some(st) = start.take() {
                        out.push(&s[st..i]);
                    }
                    continue;
                }
                _ => {}
            },
        }
        start.get_or_insert(i);
    }
    if let Some(st) = start {
        out.push(&s[st..]);
    }
    out
}

fn function_args<'a>(token: &'a str, name: &str) -> Option<&'a str> {
    let lower = token.to_ascii_lowercase();
    lower
        .starts_with(name)
        .then(|| token[name.len()..].trim_start())
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
}

/// Split on top-level commas.
fn split_args(s: &str) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0i32, 0, Vec::new());
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out
}

fn parse_breadth(token: &str, cx: &UnitContext) -> Option<Breadth> {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "auto" => return Some(Breadth::Auto),
        "min-content" => return Some(Breadth::MinContent),
        "max-content" => return Some(Breadth::MaxContent),
        _ => {}
    }
    if let Some(n) = lower.strip_suffix("fr") {
        return n
            .parse::<f32>()
            .ok()
            .filter(|n| *n >= 0.0 && n.is_finite())
            .map(Breadth::Fr);
    }
    match parse_length(token, cx)? {
        l @ (Length::Px(_) | Length::Percent(_) | Length::Calc(_)) => Some(Breadth::Length(l)),
        _ => None,
    }
}

fn parse_track_size(token: &str, cx: &UnitContext) -> Option<TrackSize> {
    if let Some(args) = function_args(token, "minmax") {
        let args = split_args(args);
        let [min, max] = args.as_slice() else {
            return None;
        };
        let min = match parse_breadth(min, cx)? {
            Breadth::Fr(_) => return None,
            b => b,
        };
        return Some(TrackSize {
            min,
            max: parse_breadth(max, cx)?,
            fit_content: None,
        });
    }
    if let Some(arg) = function_args(token, "fit-content") {
        let limit = match parse_length(arg, cx)? {
            l @ (Length::Px(_) | Length::Percent(_) | Length::Calc(_)) => l,
            _ => return None,
        };
        return Some(TrackSize {
            min: Breadth::Auto,
            max: Breadth::MaxContent,
            fit_content: Some(limit),
        });
    }
    parse_breadth(token, cx).map(TrackSize::single)
}

fn line_names(token: &str) -> Option<Vec<Arc<str>>> {
    let inner = token.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner.split_whitespace().map(Arc::from).collect())
}

/// Parse a `<track-list>` / `<auto-track-list>`; `None` for `none`, `subgrid`,
/// `masonry` and invalid values.
pub fn parse_track_list(raw: &str, cx: &UnitContext) -> Option<TrackList> {
    let mut list = TrackList {
        line_names: vec![Vec::new()],
        ..TrackList::default()
    };
    for token in components(raw.trim()) {
        if let Some(names) = line_names(token) {
            list.line_names.last_mut()?.extend(names);
            continue;
        }
        if let Some(args) = function_args(token, "repeat") {
            let (count, inner) = args.split_once(',')?;
            let count = count.trim().to_ascii_lowercase();
            let inner = parse_track_list(inner, cx)?;
            if inner.auto_repeat.is_some() || inner.tracks.is_empty() {
                return None;
            }
            match count.as_str() {
                "auto-fill" | "auto-fit" => {
                    if list.auto_repeat.is_some() {
                        return None;
                    }
                    list.auto_repeat = Some(AutoRepeat {
                        fill: count == "auto-fill",
                        tracks: inner.tracks,
                        at: list.tracks.len(),
                    });
                }
                n => {
                    let n: usize = n.parse().ok().filter(|n| (1..=10_000).contains(n))?;
                    for _ in 0..n {
                        let names = &inner.line_names;
                        list.line_names.last_mut()?.extend(names[0].iter().cloned());
                        for (i, t) in inner.tracks.iter().enumerate() {
                            list.tracks.push(t.clone());
                            list.line_names.push(names[i + 1].clone());
                        }
                    }
                }
            }
            continue;
        }
        list.tracks.push(parse_track_size(token, cx)?);
        list.line_names.push(Vec::new());
    }
    (!list.tracks.is_empty() || list.auto_repeat.is_some()).then_some(list)
}

/// Parse `grid-auto-rows` / `grid-auto-columns` (a list of track sizes).
pub fn parse_track_sizes(raw: &str, cx: &UnitContext) -> Option<Arc<[TrackSize]>> {
    let sizes: Option<Vec<TrackSize>> = components(raw.trim())
        .into_iter()
        .map(|t| parse_track_size(t, cx))
        .collect();
    sizes.filter(|s| !s.is_empty()).map(Arc::from)
}

/// Parse `grid-template-areas`; `None` for `none` and invalid (non-rectangular) values.
pub fn parse_areas(raw: &str) -> Option<GridAreas> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for token in components(raw.trim()) {
        let inner = token
            .strip_prefix('"')
            .and_then(|t| t.strip_suffix('"'))
            .or_else(|| token.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')))?;
        // A run of dots is one null cell token, even without spaces around it.
        let cells: Vec<String> = inner
            .split_whitespace()
            .flat_map(|w| {
                let mut out = Vec::new();
                let mut cur = String::new();
                for c in w.chars() {
                    let dot = c == '.';
                    if !cur.is_empty() && cur.ends_with('.') != dot {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.push(c);
                }
                out.push(cur);
                out
            })
            .collect();
        if cells.is_empty() {
            return None;
        }
        rows.push(cells);
    }
    let columns = rows.first()?.len();
    if rows.iter().any(|r| r.len() != columns) {
        return None;
    }
    let mut areas: Vec<GridArea> = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            if cell.starts_with('.') {
                continue;
            }
            match areas.iter_mut().find(|a| &*a.name == cell.as_str()) {
                Some(a) => {
                    a.row_start = a.row_start.min(r);
                    a.row_end = a.row_end.max(r + 1);
                    a.column_start = a.column_start.min(c);
                    a.column_end = a.column_end.max(c + 1);
                }
                None => areas.push(GridArea {
                    name: Arc::from(cell.as_str()),
                    row_start: r,
                    row_end: r + 1,
                    column_start: c,
                    column_end: c + 1,
                }),
            }
        }
    }
    // Every area must be a filled rectangle.
    for a in &areas {
        for row in &rows[a.row_start..a.row_end] {
            if row[a.column_start..a.column_end]
                .iter()
                .any(|cell| cell.as_str() != &*a.name)
            {
                return None;
            }
        }
    }
    Some(GridAreas {
        rows: rows.len(),
        columns,
        areas,
    })
}

fn is_custom_ident(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    !matches!(
        lower.as_str(),
        "auto" | "span" | "inherit" | "initial" | "unset" | "default"
    ) && s
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '-' || !c.is_ascii())
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || !c.is_ascii())
}

/// Parse a `<grid-line>`; invalid values are `auto`.
pub fn parse_grid_line(raw: &str) -> GridLine {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let mut span = false;
    let mut index: Option<i32> = None;
    let mut name: Option<Arc<str>> = None;
    for t in &tokens {
        if t.eq_ignore_ascii_case("span") && !span {
            span = true;
        } else if let Ok(n) = t.parse::<i32>() {
            if index.is_some() {
                return GridLine::Auto;
            }
            index = Some(n);
        } else if is_custom_ident(t) && name.is_none() {
            name = Some(Arc::from(*t));
        } else {
            // Includes `auto`, which is only valid alone and means the same.
            return GridLine::Auto;
        }
    }
    match (span, index, name) {
        (true, Some(n), name) if n > 0 => GridLine::Span {
            count: n as u32,
            name,
        },
        (true, None, Some(name)) => GridLine::Span {
            count: 1,
            name: Some(name),
        },
        (false, Some(n), name) if n != 0 => GridLine::Line { index: n, name },
        (false, None, Some(name)) => GridLine::Name(name),
        _ => GridLine::Auto,
    }
}

/// Whether a `grid-row` / `grid-column` / `grid-area` part is a lone `<custom-ident>`
/// (omitted parts then copy it).
pub fn is_grid_ident(part: &str) -> bool {
    let t = part.trim();
    !t.contains(char::is_whitespace) && is_custom_ident(t)
}

pub fn parse_auto_flow(raw: &str) -> Option<GridAutoFlow> {
    let mut flow = GridAutoFlow::default();
    let mut seen_axis = false;
    for t in raw.split_whitespace() {
        match t.to_ascii_lowercase().as_str() {
            "row" if !seen_axis => seen_axis = true,
            "column" if !seen_axis => {
                seen_axis = true;
                flow.column = true;
            }
            "dense" if !flow.dense => flow.dense = true,
            _ => return None,
        }
    }
    (seen_axis || flow.dense).then_some(flow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> UnitContext {
        UnitContext {
            em: 10.0,
            rem: 16.0,
            vw: 800.0,
            vh: 600.0,
            ex: 5.0,
            ch: 5.0,
        }
    }

    #[test]
    fn track_lists_resolve_units_and_expand_integer_repeats() {
        let l =
            parse_track_list("[a] 2em repeat(2, [b] 1fr) minmax(10px, auto) [end]", &cx()).unwrap();
        assert_eq!(l.tracks.len(), 4);
        assert_eq!(l.tracks[0].max, Breadth::Length(Length::Px(20.0)));
        assert_eq!(l.tracks[1].min, Breadth::Auto);
        assert_eq!(l.tracks[1].max, Breadth::Fr(1.0));
        assert_eq!(l.tracks[3].min, Breadth::Length(Length::Px(10.0)));
        let names: Vec<Vec<&str>> = l
            .line_names
            .iter()
            .map(|n| n.iter().map(|s| &**s).collect())
            .collect();
        assert_eq!(
            names,
            vec![vec!["a"], vec!["b"], vec!["b"], vec![], vec!["end"]]
        );
        let auto = parse_track_list("100px repeat(auto-fill, minmax(8vw, 1fr))", &cx()).unwrap();
        let rep = auto.auto_repeat.unwrap();
        assert!(rep.fill);
        assert_eq!(rep.at, 1);
        assert_eq!(rep.tracks[0].min, Breadth::Length(Length::Px(64.0)));
        assert!(parse_track_list("none", &cx()).is_none());
        assert!(parse_track_list("repeat(0, 1fr)", &cx()).is_none());
        assert!(parse_track_list("minmax(1fr, 2fr)", &cx()).is_none());
    }

    #[test]
    fn areas_must_be_rectangles() {
        let a = parse_areas("\"head head\" \"nav main\" '. foot'").unwrap();
        assert_eq!((a.rows, a.columns), (3, 2));
        let head = a.areas.iter().find(|x| &*x.name == "head").unwrap();
        assert_eq!((head.column_start, head.column_end), (0, 2));
        assert!(!a.areas.iter().any(|x| x.name.starts_with('.')));
        assert!(parse_areas("\"a b\" \"b a\"").is_none());
        assert!(parse_areas("\"a b\" \"c\"").is_none());
        assert_eq!(parse_areas("\"a...b\"").unwrap().columns, 3);
    }

    #[test]
    fn grid_lines() {
        assert_eq!(parse_grid_line("auto"), GridLine::Auto);
        assert_eq!(
            parse_grid_line("-1"),
            GridLine::Line {
                index: -1,
                name: None
            }
        );
        assert_eq!(
            parse_grid_line("span 2"),
            GridLine::Span {
                count: 2,
                name: None
            }
        );
        assert_eq!(parse_grid_line("main"), GridLine::Name(Arc::from("main")));
        assert_eq!(parse_grid_line("0"), GridLine::Auto);
        assert_eq!(parse_grid_line("span 0"), GridLine::Auto);
        assert_eq!(
            parse_auto_flow("column dense"),
            Some(GridAutoFlow {
                column: true,
                dense: true
            })
        );
    }
}
