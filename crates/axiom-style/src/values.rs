//! CSS values: lengths (with `calc()` / `min()` / `max()` / `clamp()`) and colors.

use std::sync::Arc;

/// Context for resolving relative units to CSS pixels.
#[derive(Debug, Clone, Copy)]
pub struct UnitContext {
    /// Font size `em` refers to (the parent's for `font-size`, the element's otherwise).
    pub em: f32,
    pub rem: f32,
    pub vw: f32,
    pub vh: f32,
    /// x-height and `0` advance of the font `em` belongs to.
    pub ex: f32,
    pub ch: f32,
}

/// Whether `value` uses `ex` or `ch` (which need font metrics to resolve).
pub fn has_font_relative_unit(value: &str) -> bool {
    let b = value.as_bytes();
    (1..b.len().saturating_sub(1)).any(|i| {
        (b[i - 1].is_ascii_digit() || b[i - 1] == b'.')
            && matches!(
                &[b[i].to_ascii_lowercase(), b[i + 1].to_ascii_lowercase()],
                b"ex" | b"ch"
            )
            && b.get(i + 2).is_none_or(|c| !c.is_ascii_alphanumeric())
    })
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Length {
    #[default]
    Auto,
    Px(f32),
    Percent(f32),
    /// A `calc()` expression that mixes percentages with absolute lengths.
    Calc(Arc<CalcNode>),
    MinContent,
    MaxContent,
    FitContent,
    /// `none` (for `max-width` / `max-height`).
    None,
}

impl Length {
    pub const ZERO: Length = Length::Px(0.0);

    /// Pixels against a containing-block size; `None` for `auto`, `none` and the
    /// intrinsic keywords.
    pub fn resolve(&self, containing: f32) -> Option<f32> {
        match self {
            Length::Px(v) => Some(*v),
            Length::Percent(p) => Some(containing * p / 100.0),
            Length::Calc(c) => Some(c.eval(containing)),
            _ => None,
        }
    }

    /// Like [`Length::resolve`], with `default` for the non-numeric cases.
    pub fn resolve_or(&self, containing: f32, default: f32) -> f32 {
        self.resolve(containing).unwrap_or(default)
    }

    pub fn is_auto(&self) -> bool {
        matches!(self, Length::Auto)
    }

    /// Whether resolving needs a definite containing-block size.
    pub fn has_percent(&self) -> bool {
        matches!(self, Length::Percent(_) | Length::Calc(_))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CalcNode {
    Px(f32),
    Percent(f32),
    Number(f32),
    Add(Box<CalcNode>, Box<CalcNode>),
    Sub(Box<CalcNode>, Box<CalcNode>),
    Mul(Box<CalcNode>, Box<CalcNode>),
    Div(Box<CalcNode>, Box<CalcNode>),
    Min(Vec<CalcNode>),
    Max(Vec<CalcNode>),
    Clamp(Box<CalcNode>, Box<CalcNode>, Box<CalcNode>),
}

impl CalcNode {
    pub fn eval(&self, pct_base: f32) -> f32 {
        match self {
            CalcNode::Px(v) | CalcNode::Number(v) => *v,
            CalcNode::Percent(p) => pct_base * p / 100.0,
            CalcNode::Add(a, b) => a.eval(pct_base) + b.eval(pct_base),
            CalcNode::Sub(a, b) => a.eval(pct_base) - b.eval(pct_base),
            CalcNode::Mul(a, b) => a.eval(pct_base) * b.eval(pct_base),
            CalcNode::Div(a, b) => {
                let d = b.eval(pct_base);
                if d == 0.0 {
                    0.0
                } else {
                    a.eval(pct_base) / d
                }
            }
            CalcNode::Min(v) => v
                .iter()
                .map(|n| n.eval(pct_base))
                .fold(f32::INFINITY, f32::min),
            CalcNode::Max(v) => v
                .iter()
                .map(|n| n.eval(pct_base))
                .fold(f32::NEG_INFINITY, f32::max),
            CalcNode::Clamp(lo, v, hi) => {
                let lo = lo.eval(pct_base);
                v.eval(pct_base).min(hi.eval(pct_base)).max(lo)
            }
        }
    }

    fn has_percent(&self) -> bool {
        match self {
            CalcNode::Percent(_) => true,
            CalcNode::Px(_) | CalcNode::Number(_) => false,
            CalcNode::Add(a, b)
            | CalcNode::Sub(a, b)
            | CalcNode::Mul(a, b)
            | CalcNode::Div(a, b) => a.has_percent() || b.has_percent(),
            CalcNode::Min(v) | CalcNode::Max(v) => v.iter().any(CalcNode::has_percent),
            CalcNode::Clamp(a, b, c) => a.has_percent() || b.has_percent() || c.has_percent(),
        }
    }
}

/// Parse a dimension token (`12px`, `1.5em`, `50%`, `0`) into pixels or a percentage.
fn dimension(token: &str, cx: &UnitContext) -> Option<CalcNode> {
    let t = token.trim();
    if let Some(num) = t.strip_suffix('%') {
        return num.parse::<f32>().ok().map(CalcNode::Percent);
    }
    let split = t
        .find(|c: char| {
            !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E')
        })
        .unwrap_or(t.len());
    // `1e3px` vs `1em`: an `e` directly followed by a letter starts the unit.
    let (mut num, mut unit) = t.split_at(split);
    if let Some(pos) = num.find(['e', 'E']) {
        if !num[pos + 1..].starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+') {
            unit = &t[pos..];
            num = &t[..pos];
        }
    }
    let v: f32 = num.parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    let px = match unit.to_ascii_lowercase().as_str() {
        "" => return Some(CalcNode::Number(v)),
        "px" => v,
        "em" => v * cx.em,
        "rem" => v * cx.rem,
        "ex" => v * cx.ex,
        "ch" => v * cx.ch,
        "cap" | "ic" => v * cx.em * 0.5,
        "rex" | "rch" => v * cx.rem * 0.5,
        "lh" => v * cx.em * 1.2,
        "rlh" => v * cx.rem * 1.2,
        "vw" | "svw" | "lvw" | "dvw" | "cqw" | "cqi" => v * cx.vw / 100.0,
        "vh" | "svh" | "lvh" | "dvh" | "cqh" | "cqb" => v * cx.vh / 100.0,
        "vmin" | "svmin" | "lvmin" | "dvmin" | "cqmin" => v * cx.vw.min(cx.vh) / 100.0,
        "vmax" | "svmax" | "lvmax" | "dvmax" | "cqmax" => v * cx.vw.max(cx.vh) / 100.0,
        "vi" => v * cx.vw / 100.0,
        "vb" => v * cx.vh / 100.0,
        "pt" => v * 96.0 / 72.0,
        "pc" => v * 16.0,
        "in" => v * 96.0,
        "cm" => v * 96.0 / 2.54,
        "mm" => v * 96.0 / 25.4,
        "q" => v * 96.0 / 101.6,
        _ => return None,
    };
    Some(CalcNode::Px(px))
}

/// Parse a length / percentage (`auto`, `12px`, `50%`, `calc(100% - 2em)`, …).
pub fn parse_length(value: &str, cx: &UnitContext) -> Option<Length> {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    match lower.as_str() {
        "auto" => return Some(Length::Auto),
        "none" => return Some(Length::None),
        "min-content" | "-webkit-min-content" | "-moz-min-content" => {
            return Some(Length::MinContent)
        }
        "max-content" | "-webkit-max-content" | "-moz-max-content" | "intrinsic" => {
            return Some(Length::MaxContent)
        }
        "fit-content"
        | "-webkit-fit-content"
        | "-moz-fit-content"
        | "-moz-available"
        | "-webkit-fill-available"
        | "stretch"
        | "fill-available" => return Some(Length::FitContent),
        _ => {}
    }
    if lower.starts_with("fit-content(") {
        return Some(Length::FitContent);
    }
    let node = if is_math_function(&lower) {
        let mut p = CalcParser::new(v, cx);
        let n = p.term_list()?;
        p.skip_ws();
        if p.i != p.s.len() {
            return None;
        }
        n
    } else {
        dimension(v, cx)?
    };
    Some(match node {
        CalcNode::Px(px) => Length::Px(px),
        CalcNode::Percent(p) => Length::Percent(p),
        // Unitless: only `0` is a length.
        CalcNode::Number(0.0) => Length::Px(0.0),
        CalcNode::Number(_) => return None,
        other if other.has_percent() => Length::Calc(Arc::new(other)),
        other => Length::Px(other.eval(0.0)),
    })
}

/// A unitless or dimension number (`line-height: 1.5`, `flex-grow: 2`).
pub fn parse_number(value: &str, cx: &UnitContext) -> Option<f32> {
    let v = value.trim();
    if is_math_function(&v.to_ascii_lowercase()) {
        let mut p = CalcParser::new(v, cx);
        return p.term_list().map(|n| n.eval(0.0));
    }
    v.parse::<f32>().ok().filter(|n| n.is_finite())
}

fn is_math_function(lower: &str) -> bool {
    [
        "calc(",
        "-webkit-calc(",
        "-moz-calc(",
        "min(",
        "max(",
        "clamp(",
    ]
    .iter()
    .any(|f| lower.starts_with(f))
}

struct CalcParser<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
    cx: &'a UnitContext,
}

impl<'a> CalcParser<'a> {
    fn new(src: &'a str, cx: &'a UnitContext) -> Self {
        Self {
            s: src.as_bytes(),
            src,
            i: 0,
            cx,
        }
    }

    fn skip_ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    /// `a + b - c` at the top of a function argument.
    fn term_list(&mut self) -> Option<CalcNode> {
        self.skip_ws();
        let mut left = self.product()?;
        loop {
            self.skip_ws();
            match self.s.get(self.i) {
                Some(b'+') => {
                    self.i += 1;
                    let r = self.product()?;
                    left = CalcNode::Add(Box::new(left), Box::new(r));
                }
                Some(b'-') => {
                    self.i += 1;
                    let r = self.product()?;
                    left = CalcNode::Sub(Box::new(left), Box::new(r));
                }
                _ => return Some(left),
            }
        }
    }

    fn product(&mut self) -> Option<CalcNode> {
        self.skip_ws();
        let mut left = self.atom()?;
        loop {
            self.skip_ws();
            match self.s.get(self.i) {
                Some(b'*') => {
                    self.i += 1;
                    let r = self.atom()?;
                    left = CalcNode::Mul(Box::new(left), Box::new(r));
                }
                Some(b'/') => {
                    self.i += 1;
                    let r = self.atom()?;
                    left = CalcNode::Div(Box::new(left), Box::new(r));
                }
                _ => return Some(left),
            }
        }
    }

    fn args(&mut self) -> Option<Vec<CalcNode>> {
        let mut out = Vec::new();
        loop {
            out.push(self.term_list()?);
            self.skip_ws();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b')') => {
                    self.i += 1;
                    return Some(out);
                }
                _ => return None,
            }
        }
    }

    fn atom(&mut self) -> Option<CalcNode> {
        self.skip_ws();
        if self.s.get(self.i) == Some(&b'(') {
            self.i += 1;
            let n = self.term_list()?;
            self.skip_ws();
            if self.s.get(self.i) != Some(&b')') {
                return None;
            }
            self.i += 1;
            return Some(n);
        }
        let start = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphanumeric()
                || matches!(self.s[self.i], b'.' | b'%' | b'-' | b'_')
                || (self.i == start && self.s[self.i] == b'+'))
        {
            // A `-` after the first character of a number is a unit character only
            // inside identifiers (`-webkit-calc`); `1-2` is not valid CSS anyway.
            self.i += 1;
        }
        let word = &self.src[start..self.i];
        if self.s.get(self.i) == Some(&b'(') {
            self.i += 1;
            let name = word.to_ascii_lowercase();
            let args = self.args()?;
            return match name.as_str() {
                "calc" | "-webkit-calc" | "-moz-calc" if args.len() == 1 => args.into_iter().next(),
                "min" => Some(CalcNode::Min(args)),
                "max" => Some(CalcNode::Max(args)),
                "clamp" if args.len() == 3 => {
                    let mut it = args.into_iter();
                    let (a, b, c) = (it.next()?, it.next()?, it.next()?);
                    Some(CalcNode::Clamp(Box::new(a), Box::new(b), Box::new(c)))
                }
                _ => None,
            };
        }
        dimension(word, self.cx)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub fn to_rgba_u32(self) -> u32 {
        ((self.a as u32) << 24) | ((self.b as u32) << 16) | ((self.g as u32) << 8) | self.r as u32
    }

    pub fn with_alpha(self, alpha: f32) -> Self {
        Self {
            a: (self.a as f32 * alpha.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }
}

/// Parse a color; `current` is the value `currentcolor` refers to.
pub fn parse_color(value: &str, current: Color) -> Option<Color> {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(open) = lower.find('(') {
        let name = &lower[..open];
        let inner = lower[open + 1..].strip_suffix(')')?;
        return match name {
            "rgb" | "rgba" => parse_rgb(inner),
            "hsl" | "hsla" => parse_hsl(inner),
            "hwb" => parse_hwb(inner),
            "light-dark" => {
                let args = split_top_level(inner, ',');
                parse_color(args.first()?, current)
            }
            "color-mix" => parse_color_mix(inner, current),
            "var" => None,
            _ => None,
        };
    }
    match lower.as_str() {
        "currentcolor" => return Some(current),
        "transparent" => return Some(Color::TRANSPARENT),
        _ => {}
    }
    named_color(&lower).map(|(r, g, b)| Color::rgb(r, g, b))
}

/// Split at top-level `sep` bytes (not inside parentheses).
pub fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                c if c == sep && depth == 0 => {
                    out.push(s[start..i].trim());
                    start = i + c.len_utf8();
                }
                _ => {}
            },
        }
    }
    out.push(s[start..].trim());
    out
}

/// Split on top-level whitespace (component values).
pub fn split_components(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            start.get_or_insert(i);
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                start.get_or_insert(i);
            }
            '(' => {
                depth += 1;
                start.get_or_insert(i);
            }
            ')' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => {
                if let Some(st) = start.take() {
                    out.push(&s[st..i]);
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(st) = start {
        out.push(&s[st..]);
    }
    out
}

fn parse_hex(hex: &str) -> Option<Color> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let d = |i: usize, n: usize| u8::from_str_radix(&hex[i..i + n], 16).ok();
    let dd = |i: usize| d(i, 1).map(|v| v * 17);
    match hex.len() {
        3 => Some(Color::rgb(dd(0)?, dd(1)?, dd(2)?)),
        4 => Some(Color {
            a: dd(3)?,
            ..Color::rgb(dd(0)?, dd(1)?, dd(2)?)
        }),
        6 => Some(Color::rgb(d(0, 2)?, d(2, 2)?, d(4, 2)?)),
        8 => Some(Color {
            a: d(6, 2)?,
            ..Color::rgb(d(0, 2)?, d(2, 2)?, d(4, 2)?)
        }),
        _ => None,
    }
}

/// Channel arguments of a color function (legacy commas or modern `a b c / alpha`).
fn channels(inner: &str) -> Option<(Vec<&str>, Option<&str>)> {
    let (main, alpha) = match inner.split_once('/') {
        Some((m, a)) => (m, Some(a.trim())),
        None => (inner, None),
    };
    let parts: Vec<&str> = if main.contains(',') {
        main.split(',').map(str::trim).collect()
    } else {
        main.split_whitespace().collect()
    };
    match (parts.len(), alpha) {
        (3, a) => Some((parts, a)),
        (4, None) => Some((parts[..3].to_vec(), Some(parts[3]))),
        _ => None,
    }
}

fn alpha_value(a: Option<&str>) -> Option<u8> {
    let Some(a) = a else { return Some(255) };
    if a == "none" {
        return Some(0);
    }
    let v = match a.strip_suffix('%') {
        Some(p) => p.trim().parse::<f32>().ok()? / 100.0,
        None => a.parse::<f32>().ok()?,
    };
    Some((v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn parse_rgb(inner: &str) -> Option<Color> {
    let (parts, alpha) = channels(inner)?;
    let ch = |s: &str| -> Option<u8> {
        if s == "none" {
            return Some(0);
        }
        let v = match s.strip_suffix('%') {
            Some(p) => p.trim().parse::<f32>().ok()? * 2.55,
            None => s.parse::<f32>().ok()?,
        };
        Some(v.clamp(0.0, 255.0).round() as u8)
    };
    Some(Color {
        r: ch(parts[0])?,
        g: ch(parts[1])?,
        b: ch(parts[2])?,
        a: alpha_value(alpha)?,
    })
}

fn hue(s: &str) -> Option<f32> {
    let s = s.trim();
    if s == "none" {
        return Some(0.0);
    }
    let (num, scale) = if let Some(n) = s.strip_suffix("deg") {
        (n, 1.0)
    } else if let Some(n) = s.strip_suffix("grad") {
        (n, 0.9)
    } else if let Some(n) = s.strip_suffix("rad") {
        (n, 180.0 / std::f32::consts::PI)
    } else if let Some(n) = s.strip_suffix("turn") {
        (n, 360.0)
    } else {
        (s, 1.0)
    };
    Some(num.trim().parse::<f32>().ok()? * scale)
}

fn percent(s: &str) -> Option<f32> {
    let s = s.trim();
    if s == "none" {
        return Some(0.0);
    }
    let v: f32 = s.strip_suffix('%').unwrap_or(s).trim().parse().ok()?;
    Some((v / 100.0).clamp(0.0, 1.0))
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0) / 360.0;
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

fn parse_hsl(inner: &str) -> Option<Color> {
    let (parts, alpha) = channels(inner)?;
    let (r, g, b) = hsl_to_rgb(hue(parts[0])?, percent(parts[1])?, percent(parts[2])?);
    Some(Color {
        r,
        g,
        b,
        a: alpha_value(alpha)?,
    })
}

fn parse_hwb(inner: &str) -> Option<Color> {
    let (parts, alpha) = channels(inner)?;
    let (h, mut w, mut bl) = (hue(parts[0])?, percent(parts[1])?, percent(parts[2])?);
    if w + bl > 1.0 {
        let sum = w + bl;
        w /= sum;
        bl /= sum;
    }
    let (r, g, b) = hsl_to_rgb(h, 1.0, 0.5);
    let f = |c: u8| ((c as f32 / 255.0 * (1.0 - w - bl) + w) * 255.0).round() as u8;
    Some(Color {
        r: f(r),
        g: f(g),
        b: f(b),
        a: alpha_value(alpha)?,
    })
}

/// `color-mix(in srgb, a p%, b q%)`, mixed in sRGB whatever the interpolation space.
fn parse_color_mix(inner: &str, current: Color) -> Option<Color> {
    let args = split_top_level(inner, ',');
    if args.len() != 3 {
        return None;
    }
    let part = |s: &str| -> Option<(Color, Option<f32>)> {
        let comps = split_components(s);
        let mut color = None;
        let mut pct = None;
        for c in comps {
            if let Some(p) = c.strip_suffix('%') {
                pct = Some(p.parse::<f32>().ok()? / 100.0);
            } else {
                color = Some(parse_color(c, current)?);
            }
        }
        Some((color?, pct))
    };
    let (a, pa) = part(args[1])?;
    let (b, pb) = part(args[2])?;
    let wa = match (pa, pb) {
        (Some(x), _) => x,
        (None, Some(y)) => 1.0 - y,
        _ => 0.5,
    }
    .clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 * wa + y as f32 * (1.0 - wa)).round() as u8;
    Some(Color {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
        a: mix(a.a, b.a),
    })
}

fn named_color(name: &str) -> Option<(u8, u8, u8)> {
    NAMED_COLORS
        .binary_search_by(|(n, _)| n.cmp(&name))
        .ok()
        .map(|i| {
            let c = NAMED_COLORS[i].1;
            ((c >> 16) as u8, (c >> 8) as u8, c as u8)
        })
}

/// CSS named colors plus the system colors used by form controls, sorted by name.
static NAMED_COLORS: &[(&str, u32)] = &[
    ("accentcolor", 0x0075ff),
    ("accentcolortext", 0xffffff),
    ("activetext", 0xff0000),
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("buttonborder", 0x767676),
    ("buttonface", 0xefefef),
    ("buttontext", 0x000000),
    ("cadetblue", 0x5f9ea0),
    ("canvas", 0xffffff),
    ("canvastext", 0x000000),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("field", 0xffffff),
    ("fieldtext", 0x000000),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("graytext", 0x6d6d6d),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("highlight", 0x3399ff),
    ("highlighttext", 0xffffff),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("linktext", 0x0000ee),
    ("magenta", 0xff00ff),
    ("mark", 0xffff00),
    ("marktext", 0x000000),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("selecteditem", 0x3399ff),
    ("selecteditemtext", 0xffffff),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("visitedtext", 0x551a8b),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;

    const CX: UnitContext = UnitContext {
        em: 20.0,
        rem: 16.0,
        vw: 1000.0,
        vh: 800.0,
        ex: 9.0,
        ch: 11.0,
    };

    #[test]
    fn font_relative_units_use_font_metrics() {
        assert_eq!(parse_length("2ex", &CX), Some(Length::Px(18.0)));
        assert_eq!(parse_length("calc(1ch + 1px)", &CX), Some(Length::Px(12.0)));
        assert!(has_font_relative_unit("calc(1ch + 1px)"));
        assert!(has_font_relative_unit("0.5EX"));
        assert!(!has_font_relative_unit("each 1px"));
        assert!(!has_font_relative_unit("italic 20px/1 Ahem"));
    }

    #[test]
    fn named_colors_are_sorted() {
        assert!(NAMED_COLORS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn lengths_resolve_units() {
        assert_eq!(parse_length("2em", &CX), Some(Length::Px(40.0)));
        assert_eq!(parse_length("1.5rem", &CX), Some(Length::Px(24.0)));
        assert_eq!(parse_length("10vw", &CX), Some(Length::Px(100.0)));
        assert_eq!(parse_length("12pt", &CX), Some(Length::Px(16.0)));
        assert_eq!(parse_length("50%", &CX), Some(Length::Percent(50.0)));
        assert_eq!(parse_length("0", &CX), Some(Length::Px(0.0)));
        assert_eq!(parse_length("12", &CX), None);
        assert_eq!(parse_length("1e1px", &CX), Some(Length::Px(10.0)));
    }

    #[test]
    fn calc_min_max_clamp() {
        assert_eq!(
            parse_length("calc(1em + 10px)", &CX),
            Some(Length::Px(30.0))
        );
        let l = parse_length("calc(100% - 2 * 1rem)", &CX).unwrap();
        assert_eq!(l.resolve(200.0), Some(168.0));
        let l = parse_length("min(100%, 600px)", &CX).unwrap();
        assert_eq!(l.resolve(1000.0), Some(600.0));
        assert_eq!(l.resolve(300.0), Some(300.0));
        assert_eq!(
            parse_length("clamp(1rem, 2.5vw, 2rem)", &CX),
            Some(Length::Px(25.0))
        );
        assert_eq!(
            parse_length("calc((10px + 5px) / 3)", &CX),
            Some(Length::Px(5.0))
        );
    }

    #[test]
    fn colors() {
        let k = Color::BLACK;
        assert_eq!(parse_color("#fff", k), Some(Color::WHITE));
        assert_eq!(parse_color("#11223380", k).unwrap().a, 0x80);
        assert_eq!(
            parse_color("rgb(255 0 0 / 50%)", k).unwrap(),
            Color {
                a: 128,
                ..Color::rgb(255, 0, 0)
            }
        );
        assert_eq!(parse_color("rgba(0, 128, 0, 0.5)", k).unwrap().g, 128);
        assert_eq!(
            parse_color("hsl(120, 100%, 25%)", k),
            Some(Color::rgb(0, 128, 0))
        );
        assert_eq!(
            parse_color("RebeccaPurple", k),
            Some(Color::rgb(0x66, 0x33, 0x99))
        );
        assert_eq!(
            parse_color("currentColor", Color::WHITE),
            Some(Color::WHITE)
        );
        assert_eq!(parse_color("light-dark(#000, #fff)", k), Some(Color::BLACK));
        assert_eq!(
            parse_color("color-mix(in srgb, #000 50%, #fff)", k),
            Some(Color::rgb(128, 128, 128))
        );
        assert_eq!(parse_color("nonsense", k), None);
    }

    #[test]
    fn component_splitting() {
        assert_eq!(
            split_components("1px solid rgb(0, 0, 0)"),
            ["1px", "solid", "rgb(0, 0, 0)"]
        );
        assert_eq!(split_top_level("a, f(b, c), d", ','), ["a", "f(b, c)", "d"]);
    }
}
