//! Image layers of `background-*` and `mask-*` (CSS Backgrounds 3 §3, CSS Masking 1 §7):
//! `url()` images and linear, radial and conic gradients, each positioned, sized and
//! tiled on its own. The first layer listed paints on top.

use std::sync::Arc;

use crate::values::{
    parse_color, parse_length, split_components, split_top_level, Color, Length, UnitContext,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Image {
    None,
    Url(Arc<str>),
    Gradient(Arc<Gradient>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    pub stops: Vec<ColorStop>,
    pub repeating: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GradientKind {
    Linear(LinearDirection),
    Radial {
        circle: bool,
        extent: RadialExtent,
        center: [Length; 2],
    },
    /// `from` in degrees clockwise from "to top".
    Conic {
        from: f32,
        center: [Length; 2],
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LinearDirection {
    /// Degrees clockwise from "to top".
    Angle(f32),
    /// Toward a corner: the signs of x and y (`to left top` is (-1, -1)).
    Corner(f32, f32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum RadialExtent {
    ClosestSide,
    ClosestCorner,
    FarthestSide,
    FarthestCorner,
    /// Explicit radii; a circle uses the first.
    Size(Length, Length),
}

/// A color stop. Conic positions are fractions of a turn, stored as percentages.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorStop {
    pub color: Color,
    pub position: Option<Length>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BgSize {
    Cover,
    Contain,
    /// Width and height, either of which may be `auto`.
    Explicit(Length, Length),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    Repeat,
    NoRepeat,
    Space,
    Round,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxArea {
    BorderBox,
    PaddingBox,
    ContentBox,
    /// `background-clip: text`.
    Text,
}

/// The layered longhands of `background` or `mask`. Every list has at least one entry;
/// the number of layers is the number of images and shorter lists repeat.
#[derive(Debug, Clone)]
pub struct ImageLayers {
    pub image: Arc<[Image]>,
    pub position_x: Arc<[Length]>,
    pub position_y: Arc<[Length]>,
    pub size: Arc<[BgSize]>,
    pub repeat: Arc<[[Repeat; 2]]>,
    pub origin: Arc<[BoxArea]>,
    pub clip: Arc<[BoxArea]>,
}

/// One layer of [`ImageLayers`].
#[derive(Debug, Clone, Copy)]
pub struct Layer<'a> {
    pub image: &'a Image,
    pub position: [&'a Length; 2],
    pub size: &'a BgSize,
    pub repeat: [Repeat; 2],
    pub origin: BoxArea,
    pub clip: BoxArea,
}

impl ImageLayers {
    /// Initial values; backgrounds position against the padding box, masks the border box.
    pub fn initial(origin: BoxArea) -> Self {
        Self {
            image: Arc::from([Image::None]),
            position_x: Arc::from([Length::Percent(0.0)]),
            position_y: Arc::from([Length::Percent(0.0)]),
            size: Arc::from([BgSize::Explicit(Length::Auto, Length::Auto)]),
            repeat: Arc::from([[Repeat::Repeat; 2]]),
            origin: Arc::from([origin]),
            clip: Arc::from([BoxArea::BorderBox]),
        }
    }

    pub fn has_images(&self) -> bool {
        self.image.iter().any(|i| *i != Image::None)
    }

    /// Layers from top to bottom.
    pub fn layers(&self) -> impl DoubleEndedIterator<Item = Layer<'_>> + '_ {
        fn at<T>(list: &[T], i: usize) -> &T {
            &list[i % list.len()]
        }
        (0..self.image.len()).map(move |i| Layer {
            image: &self.image[i],
            position: [at(&self.position_x, i), at(&self.position_y, i)],
            size: at(&self.size, i),
            repeat: *at(&self.repeat, i),
            origin: *at(&self.origin, i),
            clip: *at(&self.clip, i),
        })
    }

    /// `url()` references of the layers.
    pub fn urls(&self) -> impl Iterator<Item = &Arc<str>> + '_ {
        self.image.iter().filter_map(|i| match i {
            Image::Url(u) => Some(u),
            _ => None,
        })
    }
}

fn list<T>(raw: &str, item: impl Fn(&str) -> Option<T>) -> Option<Arc<[T]>> {
    let items = split_top_level(raw, ',')
        .into_iter()
        .map(|i| item(i.trim()))
        .collect::<Option<Vec<T>>>()?;
    (!items.is_empty()).then(|| Arc::from(items))
}

/// `background-image` / `mask-image`.
pub fn parse_images(raw: &str, cx: &UnitContext, current: Color) -> Option<Arc<[Image]>> {
    list(raw, |i| parse_image(i, cx, current))
}

/// `background-position-x` (`horizontal`) or `-y`.
pub fn parse_positions(raw: &str, cx: &UnitContext, horizontal: bool) -> Option<Arc<[Length]>> {
    list(raw, |item| {
        let toks = split_components(item);
        let keyword = |t: &str| match (t.to_ascii_lowercase().as_str(), horizontal) {
            ("center", _) => Some(50.0),
            ("left", true) | ("top", false) => Some(0.0),
            ("right", true) | ("bottom", false) => Some(100.0),
            _ => None,
        };
        match toks.as_slice() {
            [t] => keyword(t)
                .map(Length::Percent)
                .or_else(|| parse_length(t, cx)),
            [edge, offset] => match keyword(edge)? {
                0.0 => parse_length(offset, cx),
                100.0 => parse_length(&format!("calc(100% - {offset})"), cx),
                _ => None,
            },
            _ => None,
        }
    })
}

/// `background-size` / `mask-size`.
pub fn parse_sizes(raw: &str, cx: &UnitContext) -> Option<Arc<[BgSize]>> {
    list(raw, |item| {
        let one = |t: &str| -> Option<Length> {
            if t.eq_ignore_ascii_case("auto") {
                return Some(Length::Auto);
            }
            parse_length(t, cx).filter(|l| match l {
                Length::Px(v) | Length::Percent(v) => *v >= 0.0,
                l => l.has_percent(),
            })
        };
        match split_components(item).as_slice() {
            [t] if t.eq_ignore_ascii_case("cover") => Some(BgSize::Cover),
            [t] if t.eq_ignore_ascii_case("contain") => Some(BgSize::Contain),
            [w] => Some(BgSize::Explicit(one(w)?, Length::Auto)),
            [w, h] => Some(BgSize::Explicit(one(w)?, one(h)?)),
            _ => None,
        }
    })
}

/// `background-repeat` / `mask-repeat`.
pub fn parse_repeats(raw: &str) -> Option<Arc<[[Repeat; 2]]>> {
    let one = |t: &str| match t {
        "repeat" => Some(Repeat::Repeat),
        "no-repeat" => Some(Repeat::NoRepeat),
        "space" => Some(Repeat::Space),
        "round" => Some(Repeat::Round),
        _ => None,
    };
    list(&raw.to_ascii_lowercase(), |item| {
        match split_components(item).as_slice() {
            ["repeat-x"] => Some([Repeat::Repeat, Repeat::NoRepeat]),
            ["repeat-y"] => Some([Repeat::NoRepeat, Repeat::Repeat]),
            [a] => one(a).map(|r| [r, r]),
            [a, b] => Some([one(a)?, one(b)?]),
            _ => None,
        }
    })
}

fn box_area(t: &str) -> Option<BoxArea> {
    Some(match t.to_ascii_lowercase().as_str() {
        "border-box" | "border" | "fill-box" | "stroke-box" | "view-box" | "no-clip" => {
            BoxArea::BorderBox
        }
        "padding-box" | "padding" => BoxArea::PaddingBox,
        "content-box" | "content" => BoxArea::ContentBox,
        "text" => BoxArea::Text,
        _ => return None,
    })
}

/// `background-origin` / `-clip`, `mask-origin` / `-clip`.
pub fn parse_areas(raw: &str) -> Option<Arc<[BoxArea]>> {
    list(raw, box_area)
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    let quoted = s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')));
    if quoted {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// A quoted or bare `url()` argument with its CSS escapes (`\"`, `\22 `) resolved.
fn url_arg(s: &str) -> String {
    crate::generated::unescape(unquote(s))
}

/// `name(args)` → (lowercase name, args).
fn function(v: &str) -> Option<(String, &str)> {
    let open = v.find('(')?;
    let args = v[open + 1..].strip_suffix(')')?;
    Some((v[..open].trim().to_ascii_lowercase(), args))
}

fn parse_image(v: &str, cx: &UnitContext, current: Color) -> Option<Image> {
    if v.eq_ignore_ascii_case("none") {
        return Some(Image::None);
    }
    let (full, args) = function(v)?;
    let name = full
        .strip_prefix("-webkit-")
        .or_else(|| full.strip_prefix("-moz-"))
        .unwrap_or(&full);
    // Prefixed gradients use the legacy syntax: a start side and counter-clockwise angles.
    let legacy = name.len() != full.len();
    match name {
        "url" => {
            let url = url_arg(args);
            (!url.is_empty()).then(|| Image::Url(Arc::from(url)))
        }
        // The first candidate stands in for the set.
        "image-set" => {
            let first = split_top_level(args, ',').into_iter().next()?;
            let candidate = split_components(first).into_iter().next()?;
            if candidate.starts_with(['"', '\'']) {
                Some(Image::Url(Arc::from(url_arg(candidate))))
            } else {
                parse_image(candidate, cx, current)
            }
        }
        _ => {
            let (repeating, base) = match name.strip_prefix("repeating-") {
                Some(base) => (true, base),
                None => (false, name),
            };
            let parts = split_top_level(args, ',');
            let (kind, stops) = match base {
                "linear-gradient" => linear(&parts, legacy)?,
                "radial-gradient" => radial(&parts, cx, legacy)?,
                "conic-gradient" => conic(&parts, cx)?,
                _ => return None,
            };
            let stops = color_stops(
                stops,
                cx,
                current,
                matches!(kind, GradientKind::Conic { .. }),
            )?;
            Some(Image::Gradient(Arc::new(Gradient {
                kind,
                stops,
                repeating,
            })))
        }
    }
}

/// Degrees of an `<angle>` (a bare `0` too).
fn parse_angle(t: &str) -> Option<f32> {
    let t = t.trim().to_ascii_lowercase();
    if t == "0" {
        return Some(0.0);
    }
    let units = [
        ("deg", 1.0),
        ("grad", 0.9),
        ("rad", 180.0 / std::f32::consts::PI),
        ("turn", 360.0),
    ];
    units.iter().find_map(|(unit, scale)| {
        t.strip_suffix(unit)
            .and_then(|n| n.parse::<f32>().ok())
            .map(|n| n * scale)
    })
}

/// Direction from side keywords: `to` direction, or for legacy syntax the start side.
fn side_direction(words: &[&str], legacy: bool) -> Option<LinearDirection> {
    let (mut x, mut y) = (0.0f32, 0.0f32);
    for w in words {
        match w.to_ascii_lowercase().as_str() {
            "left" if x == 0.0 => x = -1.0,
            "right" if x == 0.0 => x = 1.0,
            "top" if y == 0.0 => y = -1.0,
            "bottom" if y == 0.0 => y = 1.0,
            "center" if legacy => {}
            _ => return None,
        }
    }
    if legacy {
        (x, y) = (-x, -y);
    }
    Some(match (x, y) {
        (0.0, 0.0) => return None,
        (0.0, y) => LinearDirection::Angle(if y < 0.0 { 0.0 } else { 180.0 }),
        (x, 0.0) => LinearDirection::Angle(if x > 0.0 { 90.0 } else { 270.0 }),
        (x, y) => LinearDirection::Corner(x, y),
    })
}

type Parsed<'a> = (GradientKind, &'a [&'a str]);

fn linear<'a>(parts: &'a [&'a str], legacy: bool) -> Option<Parsed<'a>> {
    let first = parts.first()?.trim();
    let words = split_components(first);
    if let Some(a) = parse_angle(first) {
        // Legacy angles run counter-clockwise from "to right".
        let a = if legacy { 90.0 - a } else { a };
        return Some((GradientKind::Linear(LinearDirection::Angle(a)), &parts[1..]));
    }
    let dir = match words.split_first() {
        Some((to, rest)) if !legacy && to.eq_ignore_ascii_case("to") => side_direction(rest, false),
        _ if legacy => side_direction(&words, true),
        _ => None,
    };
    Some(match dir {
        Some(d) => (GradientKind::Linear(d), &parts[1..]),
        None => (GradientKind::Linear(LinearDirection::Angle(180.0)), parts),
    })
}

/// `[x, y]` lengths of a `<position>`.
fn position(tokens: &[&str], cx: &UnitContext) -> Option<[Length; 2]> {
    let [x, y] = split_position(tokens)?;
    Some([parse_length(&x, cx)?, parse_length(&y, cx)?])
}

fn is_color(t: &str) -> bool {
    parse_color(t, Color::BLACK).is_some()
}

fn radial<'a>(parts: &'a [&'a str], cx: &UnitContext, legacy: bool) -> Option<Parsed<'a>> {
    let mut circle = false;
    let mut extent = None;
    let mut center = [Length::Percent(50.0), Length::Percent(50.0)];
    let mut rest = parts;
    let config = |part: &str| split_components(part).first().is_some_and(|t| !is_color(t));
    if legacy && rest.first().is_some_and(|p| config(p)) {
        // `-webkit-radial-gradient(center, ellipse cover, …)`: position first.
        if let Some(c) = position(&split_components(rest[0]), cx) {
            center = c;
            rest = &rest[1..];
        }
    }
    if rest.first().is_some_and(|p| config(p)) {
        let toks = split_components(rest[0]);
        let at = toks.iter().position(|t| t.eq_ignore_ascii_case("at"));
        let (shape, pos) = match at {
            Some(i) => (&toks[..i], Some(&toks[i + 1..])),
            None => (&toks[..], None),
        };
        let mut sizes = Vec::new();
        for t in shape {
            match t.to_ascii_lowercase().as_str() {
                "circle" => circle = true,
                "ellipse" => {}
                "closest-side" | "contain" => extent = Some(RadialExtent::ClosestSide),
                "closest-corner" => extent = Some(RadialExtent::ClosestCorner),
                "farthest-side" => extent = Some(RadialExtent::FarthestSide),
                "farthest-corner" | "cover" => extent = Some(RadialExtent::FarthestCorner),
                _ => sizes.push(parse_length(t, cx)?),
            }
        }
        match sizes.as_slice() {
            [] => {}
            [r] => {
                circle = true;
                extent = Some(RadialExtent::Size(r.clone(), r.clone()));
            }
            [rx, ry] => extent = Some(RadialExtent::Size(rx.clone(), ry.clone())),
            _ => return None,
        }
        if let Some(pos) = pos {
            center = position(pos, cx)?;
        }
        rest = &rest[1..];
    }
    let extent = extent.unwrap_or(RadialExtent::FarthestCorner);
    Some((
        GradientKind::Radial {
            circle,
            extent,
            center,
        },
        rest,
    ))
}

fn conic<'a>(parts: &'a [&'a str], cx: &UnitContext) -> Option<Parsed<'a>> {
    let mut from = 0.0;
    let mut center = [Length::Percent(50.0), Length::Percent(50.0)];
    let mut rest = parts;
    let toks = split_components(parts.first()?);
    let lead = toks.first().map(|t| t.to_ascii_lowercase());
    if matches!(lead.as_deref(), Some("from" | "at")) {
        let at = toks.iter().position(|t| t.eq_ignore_ascii_case("at"));
        if lead.as_deref() == Some("from") {
            from = parse_angle(toks.get(1)?)?;
        }
        if let Some(i) = at {
            center = position(&toks[i + 1..], cx)?;
        }
        rest = &parts[1..];
    }
    Some((GradientKind::Conic { from, center }, rest))
}

fn color_stops(
    parts: &[&str],
    cx: &UnitContext,
    current: Color,
    angular: bool,
) -> Option<Vec<ColorStop>> {
    let mut stops = Vec::new();
    for part in parts {
        let toks = split_components(part);
        let (first, rest) = toks.split_first()?;
        let Some(color) = parse_color(first, current) else {
            // A transition hint between two stops (or a legacy `pos color` order).
            match (rest, toks.last().and_then(|t| parse_color(t, current))) {
                ([], None) => continue,
                (_, Some(color)) => {
                    let pos = stop_position(first, cx, angular)?;
                    stops.push(ColorStop {
                        color,
                        position: Some(pos),
                    });
                    continue;
                }
                _ => return None,
            }
        };
        if rest.is_empty() {
            stops.push(ColorStop {
                color,
                position: None,
            });
        }
        for p in rest.iter().take(2) {
            stops.push(ColorStop {
                color,
                position: Some(stop_position(p, cx, angular)?),
            });
        }
    }
    (!stops.is_empty()).then_some(stops)
}

fn stop_position(t: &str, cx: &UnitContext, angular: bool) -> Option<Length> {
    if angular {
        if let Some(deg) = parse_angle(t).filter(|_| !t.trim().ends_with('%')) {
            return Some(Length::Percent(deg / 3.6));
        }
    }
    parse_length(t, cx).filter(|l| !l.is_auto())
}

fn keyword_position(t: &str) -> Option<(char, f32)> {
    Some(match t.to_ascii_lowercase().as_str() {
        "left" => ('x', 0.0),
        "right" => ('x', 100.0),
        "top" => ('y', 0.0),
        "bottom" => ('y', 100.0),
        "center" => ('c', 50.0),
        _ => return None,
    })
}

/// `[x, y]` of a `<position>` as CSS text, edge offsets turned into `calc()`.
pub fn split_position(tokens: &[&str]) -> Option<[String; 2]> {
    let pct = |p: f32| format!("{p}%");
    match tokens {
        [t] => Some(match keyword_position(t) {
            Some(('y', p)) => [pct(50.0), pct(p)],
            Some((_, p)) => [pct(p), pct(50.0)],
            None => [t.to_string(), pct(50.0)],
        }),
        [a, b] => {
            let (ka, kb) = (keyword_position(a), keyword_position(b));
            let swapped = matches!(ka, Some(('y', _))) || matches!(kb, Some(('x', _)));
            if swapped {
                let (Some((ax, ap)), Some((bx, bp))) = (ka, kb) else {
                    return None;
                };
                if ax == 'x' || bx == 'y' {
                    return None;
                }
                return Some([pct(bp), pct(ap)]);
            }
            let x = ka.map_or_else(|| a.to_string(), |(_, p)| pct(p));
            let y = kb.map_or_else(|| b.to_string(), |(_, p)| pct(p));
            Some([x, y])
        }
        [_, _, _] | [_, _, _, _] => {
            let mut parts: Vec<(char, String)> = Vec::new();
            let mut i = 0;
            while i < tokens.len() {
                let (axis, p) = keyword_position(tokens[i])?;
                let offset = tokens
                    .get(i + 1)
                    .copied()
                    .filter(|t| keyword_position(t).is_none());
                i += 1 + usize::from(offset.is_some());
                let value = match offset {
                    None => pct(p),
                    Some(_) if axis == 'c' => return None,
                    Some(o) if p == 0.0 => o.to_string(),
                    Some(o) => format!("calc(100% - {o})"),
                };
                parts.push((axis, value));
            }
            let mut x = None;
            let mut y = None;
            for (axis, v) in &parts {
                let slot = match axis {
                    'x' => &mut x,
                    'y' => &mut y,
                    _ => continue,
                };
                if slot.replace(v.clone()).is_some() {
                    return None;
                }
            }
            for (_, v) in parts.iter().filter(|(a, _)| *a == 'c') {
                if x.is_none() {
                    x = Some(v.clone());
                } else if y.is_none() {
                    y = Some(v.clone());
                } else {
                    return None;
                }
            }
            (parts.len() == 2).then_some([x?, y?])
        }
        _ => None,
    }
}

/// Longhand values of a `background` or `mask` shorthand, as comma-separated CSS text.
#[derive(Debug, Default, PartialEq)]
pub struct Shorthand {
    pub image: String,
    pub position_x: String,
    pub position_y: String,
    pub size: String,
    pub repeat: String,
    pub origin: String,
    pub clip: String,
    /// `background` only: the final layer's color (`transparent` if none).
    pub color: String,
}

fn is_image(lower: &str) -> bool {
    lower == "none"
        || lower.starts_with("url(")
        || lower.contains("gradient(")
        || lower.contains("image-set(")
}

fn is_length_like(lower: &str) -> bool {
    lower.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '+')
        || lower.starts_with("calc(")
        || lower.starts_with("min(")
        || lower.starts_with("max(")
        || lower.starts_with("clamp(")
        || lower.starts_with("var(")
}

/// Split `a/b` components (outside functions) around the slash.
fn slash_tokens(layer: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for t in split_components(layer) {
        if t.contains('(') || !t.contains('/') {
            out.push(t);
            continue;
        }
        let mut rest = t;
        while let Some(i) = rest.find('/') {
            if i > 0 {
                out.push(&rest[..i]);
            }
            out.push("/");
            rest = &rest[i + 1..];
        }
        if !rest.is_empty() {
            out.push(rest);
        }
    }
    out
}

/// Expand `background` (`is_mask` false) or `mask`; `None` if the value is invalid.
pub fn expand_shorthand(value: &str, is_mask: bool) -> Option<Shorthand> {
    let layers = split_top_level(value, ',');
    let mut out = Shorthand {
        color: "transparent".into(),
        ..Shorthand::default()
    };
    let default_origin = if is_mask { "border-box" } else { "padding-box" };
    for (i, layer) in layers.iter().enumerate() {
        let (mut image, mut pos, mut size, mut repeat, mut boxes) =
            ("none", Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut after_slash = false;
        for t in slash_tokens(layer) {
            let lower = t.to_ascii_lowercase();
            if t == "/" {
                after_slash = true;
                continue;
            }
            if after_slash {
                let size_token = is_length_like(&lower)
                    || matches!(lower.as_str(), "auto" | "cover" | "contain");
                if size_token && size.len() < 2 {
                    size.push(t);
                    continue;
                }
                after_slash = false;
            }
            match lower.as_str() {
                l if is_image(l) => image = t,
                "repeat" | "no-repeat" | "repeat-x" | "repeat-y" | "space" | "round" => {
                    repeat.push(t)
                }
                "scroll" | "fixed" | "local" => {}
                "alpha" | "luminance" | "match-source" | "add" | "subtract" | "intersect"
                | "exclude" | "source-over"
                    if is_mask => {}
                l if box_area(l).is_some() => boxes.push(t),
                l if keyword_position(l).is_some() || is_length_like(l) => pos.push(t),
                _ if !is_mask && i + 1 == layers.len() && is_color(t) => out.color = t.to_string(),
                _ => return None,
            }
        }
        let [x, y] = if pos.is_empty() {
            ["0%".to_string(), "0%".to_string()]
        } else {
            split_position(&pos)?
        };
        let (origin, clip) = match boxes.as_slice() {
            [] => (default_origin, "border-box"),
            [a] => (*a, *a),
            [a, b] => (*a, *b),
            _ => return None,
        };
        let sep = if i == 0 { "" } else { ", " };
        let push = |field: &mut String, v: &str| {
            field.push_str(sep);
            field.push_str(v);
        };
        push(&mut out.image, image);
        push(&mut out.position_x, &x);
        push(&mut out.position_y, &y);
        push(
            &mut out.size,
            &if size.is_empty() {
                "auto".into()
            } else {
                size.join(" ")
            },
        );
        push(
            &mut out.repeat,
            &if repeat.is_empty() {
                "repeat".into()
            } else {
                repeat.join(" ")
            },
        );
        push(&mut out.origin, origin);
        push(&mut out.clip, clip);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> UnitContext {
        UnitContext {
            em: 16.0,
            rem: 16.0,
            vw: 10.0,
            vh: 6.0,
            ex: 8.0,
            ch: 8.0,
        }
    }

    #[test]
    fn positions_accept_keywords_in_either_order_and_edge_offsets() {
        let s = |t: &str| split_position(&split_components(t));
        assert_eq!(s("center"), Some(["50%".into(), "50%".into()]));
        assert_eq!(s("top"), Some(["50%".into(), "0%".into()]));
        assert_eq!(s("top right"), Some(["100%".into(), "0%".into()]));
        assert_eq!(s("10px bottom"), Some(["10px".into(), "100%".into()]));
        assert_eq!(
            s("right 10px top 5px"),
            Some(["calc(100% - 10px)".into(), "5px".into()])
        );
        assert_eq!(
            s("bottom 4px center"),
            Some(["50%".into(), "calc(100% - 4px)".into()])
        );
        assert_eq!(s("top 10px"), None);
        let x = parse_positions("right 10px, left", &cx(), true).unwrap();
        assert_eq!(x[0].resolve(100.0), Some(90.0));
        assert_eq!(x[1], Length::Percent(0.0));
    }

    #[test]
    fn gradients_parse_directions_stops_and_legacy_syntax() {
        let g = |t: &str| match parse_image(t, &cx(), Color::BLACK) {
            Some(Image::Gradient(g)) => g,
            other => panic!("{t}: {other:?}"),
        };
        let a = g("linear-gradient(to right, red, rgba(0, 0, 255, .5) 40%, lime 50% 60%)");
        assert_eq!(a.kind, GradientKind::Linear(LinearDirection::Angle(90.0)));
        assert_eq!(a.stops.len(), 4);
        assert_eq!(a.stops[1].position, Some(Length::Percent(40.0)));
        assert_eq!(a.stops[3].position, Some(Length::Percent(60.0)));
        let b = g("-webkit-linear-gradient(top, #fff, #000)");
        assert_eq!(b.kind, GradientKind::Linear(LinearDirection::Angle(180.0)));
        let c = g("linear-gradient(to left top, red, blue)");
        assert_eq!(
            c.kind,
            GradientKind::Linear(LinearDirection::Corner(-1.0, -1.0))
        );
        let d = g("repeating-radial-gradient(circle 10px at 20% 30%, red, blue 10px)");
        assert!(d.repeating);
        assert_eq!(
            d.kind,
            GradientKind::Radial {
                circle: true,
                extent: RadialExtent::Size(Length::Px(10.0), Length::Px(10.0)),
                center: [Length::Percent(20.0), Length::Percent(30.0)],
            }
        );
        let e = g("conic-gradient(from 90deg, red, blue 180deg)");
        assert_eq!(e.stops[1].position, Some(Length::Percent(50.0)));
        assert_eq!(g("linear-gradient(45deg, red, blue)").kind, {
            GradientKind::Linear(LinearDirection::Angle(45.0))
        });
        assert!(parse_image("linear-gradient(to nowhere, red)", &cx(), Color::BLACK).is_none());
    }

    #[test]
    fn url_escapes_are_resolved() {
        let img = parse_image(
            r#"url("data:image/svg+xml,<svg xmlns=\"a\" x=\27 b\27/>")"#,
            &cx(),
            Color::BLACK,
        );
        let Some(Image::Url(url)) = img else {
            panic!("{img:?}");
        };
        assert_eq!(&*url, "data:image/svg+xml,<svg xmlns=\"a\" x='b'/>");
    }

    #[test]
    fn shorthand_splits_layers_into_longhands() {
        let s = expand_shorthand(
            "url(\"a.svg\") no-repeat right 4px center / 20px auto, linear-gradient(red, blue) #eee",
            false,
        )
        .unwrap();
        assert_eq!(s.image, "url(\"a.svg\"), linear-gradient(red, blue)");
        assert_eq!(s.position_x, "calc(100% - 4px), 0%");
        assert_eq!(s.position_y, "50%, 0%");
        assert_eq!(s.size, "20px auto, auto");
        assert_eq!(s.repeat, "no-repeat, repeat");
        assert_eq!(s.origin, "padding-box, padding-box");
        assert_eq!(s.color, "#eee");
        let m = expand_shorthand("url(i.svg) center/contain no-repeat", true).unwrap();
        assert_eq!(
            (m.size.as_str(), m.origin.as_str()),
            ("contain", "border-box")
        );
        assert_eq!(expand_shorthand("red", false).unwrap().image, "none");
        assert!(expand_shorthand("bogus", false).is_none());
    }
}
