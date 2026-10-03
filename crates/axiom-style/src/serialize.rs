//! Serialization of computed values for `getComputedStyle()` (CSSOM §6.7 "resolved
//! values"). Layout-dependent properties (`width`, `height`, and percentage insets,
//! margins and paddings of rendered boxes) are resolved by the caller from layout; this
//! module serializes the computed value.

use crate::background::{
    BgSize, BoxArea, GradientKind, Image, ImageLayers, LinearDirection, Repeat,
};
use crate::grid::GridLine;
use crate::values::{CalcNode, Color, Length};
use crate::{
    Align, BorderStyle, Clear, ComputedStyle, Display, FlexDirection, FlexWrap, Float, LineHeight,
    ListStyleType, Overflow, Paint, Position, SpaceCollapse, TextAlign, TextTransform,
    VerticalAlign, LINE_THROUGH, OVERLINE, UNDERLINE,
};

/// Longhands `getComputedStyle()` enumerates, in the order it lists them.
pub const COMPUTED_PROPERTIES: &[&str] = &[
    "align-content",
    "align-items",
    "align-self",
    "aspect-ratio",
    "background-clip",
    "background-color",
    "background-image",
    "background-origin",
    "background-position-x",
    "background-position-y",
    "background-repeat",
    "background-size",
    "border-bottom-color",
    "border-bottom-left-radius",
    "border-bottom-right-radius",
    "border-bottom-style",
    "border-bottom-width",
    "border-collapse",
    "border-left-color",
    "border-left-style",
    "border-left-width",
    "border-right-color",
    "border-right-style",
    "border-right-width",
    "border-spacing",
    "border-top-color",
    "border-top-left-radius",
    "border-top-right-radius",
    "border-top-style",
    "border-top-width",
    "bottom",
    "box-sizing",
    "clear",
    "color",
    "column-gap",
    "direction",
    "display",
    "flex-basis",
    "flex-direction",
    "flex-grow",
    "flex-shrink",
    "flex-wrap",
    "float",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "grid-auto-flow",
    "grid-column-end",
    "grid-column-start",
    "grid-row-end",
    "grid-row-start",
    "height",
    "justify-content",
    "justify-items",
    "justify-self",
    "left",
    "letter-spacing",
    "line-height",
    "list-style-position",
    "list-style-type",
    "margin-bottom",
    "margin-left",
    "margin-right",
    "margin-top",
    "max-height",
    "max-width",
    "min-height",
    "min-width",
    "opacity",
    "order",
    "overflow-x",
    "overflow-y",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "padding-top",
    "position",
    "right",
    "row-gap",
    "table-layout",
    "text-align",
    "text-decoration-color",
    "text-decoration-line",
    "text-indent",
    "text-transform",
    "top",
    "vertical-align",
    "visibility",
    "white-space",
    "width",
    "word-break",
    "word-spacing",
    "z-index",
];

/// A CSS number as browsers print it: integers without a fraction, others with at most
/// four decimals and no trailing zeros.
pub fn number(n: f32) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    let s = format!("{n:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

pub fn px(n: f32) -> String {
    format!("{}px", number(n))
}

pub fn color(c: Color) -> String {
    if c.a == 255 {
        format!("rgb({}, {}, {})", c.r, c.g, c.b)
    } else {
        let alpha = (f32::from(c.a) / 255.0 * 1000.0).round() / 1000.0;
        format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, number(alpha))
    }
}

/// A `fill` / `stroke` value with `currentColor` resolved against `current`.
pub fn paint(p: &Paint, current: Color) -> String {
    match p {
        Paint::None => "none".into(),
        Paint::Color(c) => color(*c),
        Paint::CurrentColor => color(current),
        Paint::Url(u) => u.to_string(),
    }
}

fn joined<T>(items: &[T], f: impl Fn(&T) -> String) -> String {
    items.iter().map(f).collect::<Vec<_>>().join(", ")
}

fn image(i: &Image) -> String {
    let g = match i {
        Image::None => return "none".into(),
        Image::Url(u) => {
            let escaped = u.replace('\\', "\\\\").replace('"', "\\\"");
            return format!("url(\"{escaped}\")");
        }
        Image::Gradient(g) => g,
    };
    let pair = |c: &[Length; 2]| format!("{} {}", length(&c[0]), length(&c[1]));
    let (name, lead) = match &g.kind {
        GradientKind::Linear(LinearDirection::Angle(a)) => (
            "linear-gradient",
            (*a != 180.0).then(|| format!("{}deg", number(*a))),
        ),
        GradientKind::Linear(LinearDirection::Corner(x, y)) => {
            let h = if *x < 0.0 { "left" } else { "right" };
            let v = if *y < 0.0 { "top" } else { "bottom" };
            ("linear-gradient", Some(format!("to {h} {v}")))
        }
        GradientKind::Radial { circle, center, .. } => (
            "radial-gradient",
            Some(format!(
                "{} at {}",
                if *circle { "circle" } else { "ellipse" },
                pair(center)
            )),
        ),
        GradientKind::Conic { from, center } => (
            "conic-gradient",
            Some(format!("from {}deg at {}", number(*from), pair(center))),
        ),
    };
    let stops = joined(&g.stops, |s| match &s.position {
        Some(p) => format!("{} {}", color(s.color), length(p)),
        None => color(s.color),
    });
    let prefix = if g.repeating { "repeating-" } else { "" };
    match lead {
        Some(lead) => format!("{prefix}{name}({lead}, {stops})"),
        None => format!("{prefix}{name}({stops})"),
    }
}

fn layers_value(l: &ImageLayers, part: &str) -> Option<String> {
    let area = |a: &BoxArea| {
        match a {
            BoxArea::BorderBox => "border-box",
            BoxArea::PaddingBox => "padding-box",
            BoxArea::ContentBox => "content-box",
            BoxArea::Text => "text",
        }
        .to_string()
    };
    let repeat = |r: &Repeat| {
        match r {
            Repeat::Repeat => "repeat",
            Repeat::NoRepeat => "no-repeat",
            Repeat::Space => "space",
            Repeat::Round => "round",
        }
        .to_string()
    };
    Some(match part {
        "image" => joined(&l.image, image),
        "position-x" => joined(&l.position_x, length),
        "position-y" => joined(&l.position_y, length),
        "size" => joined(&l.size, |s| match s {
            BgSize::Cover => "cover".into(),
            BgSize::Contain => "contain".into(),
            BgSize::Explicit(w, Length::Auto) => length(w),
            BgSize::Explicit(w, h) => format!("{} {}", length(w), length(h)),
        }),
        "repeat" => joined(&l.repeat, |[x, y]| match (x, y) {
            (Repeat::Repeat, Repeat::NoRepeat) => "repeat-x".into(),
            (Repeat::NoRepeat, Repeat::Repeat) => "repeat-y".into(),
            (x, y) if x == y => repeat(x),
            (x, y) => format!("{} {}", repeat(x), repeat(y)),
        }),
        "origin" => joined(&l.origin, area),
        "clip" => joined(&l.clip, area),
        _ => return None,
    })
}

pub fn length(l: &Length) -> String {
    match l {
        Length::Auto => "auto".into(),
        Length::Px(v) => px(*v),
        Length::Percent(p) => format!("{}%", number(*p)),
        Length::Calc(node) => format!("calc({})", calc(node)),
        Length::MinContent => "min-content".into(),
        Length::MaxContent => "max-content".into(),
        Length::FitContent => "fit-content".into(),
        Length::None => "none".into(),
    }
}

fn calc(node: &CalcNode) -> String {
    let list = |items: &[CalcNode]| items.iter().map(calc).collect::<Vec<_>>().join(", ");
    match node {
        CalcNode::Px(v) => px(*v),
        CalcNode::Percent(p) => format!("{}%", number(*p)),
        CalcNode::Number(n) => number(*n),
        CalcNode::Add(a, b) => format!("{} + {}", calc(a), calc(b)),
        CalcNode::Sub(a, b) => format!("{} - {}", calc(a), calc(b)),
        CalcNode::Mul(a, b) => format!("{} * {}", calc(a), calc(b)),
        CalcNode::Div(a, b) => format!("{} / {}", calc(a), calc(b)),
        CalcNode::Min(items) => format!("min({})", list(items)),
        CalcNode::Max(items) => format!("max({})", list(items)),
        CalcNode::Clamp(a, b, c) => format!("clamp({}, {}, {})", calc(a), calc(b), calc(c)),
    }
}

fn display(d: Display) -> &'static str {
    match d {
        Display::None => "none",
        Display::Contents => "contents",
        Display::Block => "block",
        Display::Inline => "inline",
        Display::InlineBlock => "inline-block",
        Display::FlowRoot => "flow-root",
        Display::ListItem => "list-item",
        Display::Flex => "flex",
        Display::InlineFlex => "inline-flex",
        Display::Grid => "grid",
        Display::InlineGrid => "inline-grid",
        Display::Table => "table",
        Display::InlineTable => "inline-table",
        Display::TableRowGroup => "table-row-group",
        Display::TableHeaderGroup => "table-header-group",
        Display::TableFooterGroup => "table-footer-group",
        Display::TableRow => "table-row",
        Display::TableCell => "table-cell",
        Display::TableColumn => "table-column",
        Display::TableColumnGroup => "table-column-group",
        Display::TableCaption => "table-caption",
    }
}

fn border_style(s: BorderStyle) -> &'static str {
    match s {
        BorderStyle::None => "none",
        BorderStyle::Hidden => "hidden",
        BorderStyle::Solid => "solid",
        BorderStyle::Dashed => "dashed",
        BorderStyle::Dotted => "dotted",
        BorderStyle::Double => "double",
        BorderStyle::Groove => "groove",
        BorderStyle::Ridge => "ridge",
        BorderStyle::Inset => "inset",
        BorderStyle::Outset => "outset",
    }
}

fn overflow(o: Overflow) -> &'static str {
    match o {
        Overflow::Visible => "visible",
        Overflow::Hidden => "hidden",
        Overflow::Clip => "clip",
        Overflow::Scroll => "scroll",
        Overflow::Auto => "auto",
    }
}

fn align(a: Align) -> &'static str {
    match a {
        Align::Normal => "normal",
        Align::Stretch => "stretch",
        Align::Start => "start",
        Align::End => "end",
        Align::Center => "center",
        Align::Baseline => "baseline",
        Align::SpaceBetween => "space-between",
        Align::SpaceAround => "space-around",
        Align::SpaceEvenly => "space-evenly",
        Align::Left => "left",
        Align::Right => "right",
    }
}

fn grid_line(l: &GridLine) -> String {
    match l {
        GridLine::Auto => "auto".into(),
        GridLine::Line { index, name } => match name {
            Some(n) => format!("{index} {n}"),
            None => index.to_string(),
        },
        GridLine::Span { count, name } => match name {
            Some(n) if *count == 1 => format!("span {n}"),
            Some(n) => format!("span {count} {n}"),
            None => format!("span {count}"),
        },
        GridLine::Name(n) => n.to_string(),
    }
}

fn list_style_type(t: &ListStyleType) -> String {
    match t {
        ListStyleType::None => "none".into(),
        ListStyleType::Disc => "disc".into(),
        ListStyleType::Circle => "circle".into(),
        ListStyleType::Square => "square".into(),
        ListStyleType::Decimal => "decimal".into(),
        ListStyleType::DecimalLeadingZero => "decimal-leading-zero".into(),
        ListStyleType::LowerAlpha => "lower-alpha".into(),
        ListStyleType::UpperAlpha => "upper-alpha".into(),
        ListStyleType::LowerRoman => "lower-roman".into(),
        ListStyleType::UpperRoman => "upper-roman".into(),
        ListStyleType::DisclosureClosed => "disclosure-closed".into(),
        ListStyleType::DisclosureOpen => "disclosure-open".into(),
        ListStyleType::String(s) => format!("\"{s}\""),
    }
}

fn white_space(s: &ComputedStyle) -> &'static str {
    match (s.space_collapse, s.nowrap) {
        (SpaceCollapse::Collapse, false) => "normal",
        (SpaceCollapse::Collapse, true) => "nowrap",
        (SpaceCollapse::Preserve, true) => "pre",
        (SpaceCollapse::Preserve, false) => "pre-wrap",
        (SpaceCollapse::PreserveBreaks, _) => "pre-line",
        (SpaceCollapse::BreakSpaces, _) => "break-spaces",
    }
}

fn decoration_line(flags: u8) -> String {
    let names: Vec<&str> = [
        (UNDERLINE, "underline"),
        (OVERLINE, "overline"),
        (LINE_THROUGH, "line-through"),
    ]
    .iter()
    .filter(|(f, _)| flags & f != 0)
    .map(|(_, n)| *n)
    .collect();
    if names.is_empty() {
        "none".into()
    } else {
        names.join(" ")
    }
}

/// The computed value of longhand `property`, or of custom property `--name`; `None` for
/// properties Axiom does not compute.
pub fn computed_value(s: &ComputedStyle, property: &str) -> Option<String> {
    if property.starts_with("--") {
        return s.custom.get(property).map(|v| v.trim().to_string());
    }
    let side = |i: usize| &s.border[i];
    let v = match property {
        "display" => display(s.display).into(),
        "position" => match s.position {
            Position::Static => "static",
            Position::Relative => "relative",
            Position::Absolute => "absolute",
            Position::Fixed => "fixed",
            Position::Sticky => "sticky",
        }
        .into(),
        "float" => match s.float {
            Float::None => "none",
            Float::Left => "left",
            Float::Right => "right",
        }
        .into(),
        "clear" => match s.clear {
            Clear::None => "none",
            Clear::Left => "left",
            Clear::Right => "right",
            Clear::Both => "both",
        }
        .into(),
        "box-sizing" => if s.border_box_sizing {
            "border-box"
        } else {
            "content-box"
        }
        .into(),
        "top" => length(&s.inset.top),
        "right" => length(&s.inset.right),
        "bottom" => length(&s.inset.bottom),
        "left" => length(&s.inset.left),
        "width" => length(&s.width),
        "height" => length(&s.height),
        "min-width" => length(&s.min_width),
        "min-height" => length(&s.min_height),
        "max-width" => length(&s.max_width),
        "max-height" => length(&s.max_height),
        "aspect-ratio" => match s.aspect_ratio {
            Some(r) => format!("{} / 1", number(r)),
            None => "auto".into(),
        },
        "margin-top" => length(&s.margin.top),
        "margin-right" => length(&s.margin.right),
        "margin-bottom" => length(&s.margin.bottom),
        "margin-left" => length(&s.margin.left),
        "padding-top" => length(&s.padding.top),
        "padding-right" => length(&s.padding.right),
        "padding-bottom" => length(&s.padding.bottom),
        "padding-left" => length(&s.padding.left),
        "border-top-width" => px(side(0).used_width()),
        "border-right-width" => px(side(1).used_width()),
        "border-bottom-width" => px(side(2).used_width()),
        "border-left-width" => px(side(3).used_width()),
        "border-top-style" => border_style(side(0).style).into(),
        "border-right-style" => border_style(side(1).style).into(),
        "border-bottom-style" => border_style(side(2).style).into(),
        "border-left-style" => border_style(side(3).style).into(),
        "border-top-color" => color(side(0).color),
        "border-right-color" => color(side(1).color),
        "border-bottom-color" => color(side(2).color),
        "border-left-color" => color(side(3).color),
        "border-top-left-radius"
        | "border-top-right-radius"
        | "border-bottom-right-radius"
        | "border-bottom-left-radius" => px(s.border_radius),
        "color" => color(s.color),
        "fill" => paint(&s.fill, s.color),
        "stroke" => paint(&s.stroke, s.color),
        "stroke-width" => length(&s.stroke_width),
        "fill-opacity" => number(s.fill_opacity),
        "stroke-opacity" => number(s.stroke_opacity),
        "background-color" => color(s.background_color),
        p if p.starts_with("background-") || p.starts_with("mask-") => {
            return match p.strip_prefix("background-") {
                Some(rest) => layers_value(&s.background, rest),
                None => layers_value(&s.mask, &p["mask-".len()..]),
            };
        }
        "opacity" => number(s.opacity),
        "visibility" => if s.visibility_hidden {
            "hidden"
        } else {
            "visible"
        }
        .into(),
        "overflow-x" => overflow(s.overflow_x).into(),
        "overflow-y" => overflow(s.overflow_y).into(),
        "overflow" if s.overflow_x == s.overflow_y => overflow(s.overflow_x).into(),
        "overflow" => format!("{} {}", overflow(s.overflow_x), overflow(s.overflow_y)),
        "z-index" => s.z_index.map_or("auto".into(), |z| z.to_string()),
        "font-family" => s.font_family.to_string(),
        "font-size" => px(s.font_size),
        "font-weight" => s.font_weight.to_string(),
        "font-style" => if s.italic { "italic" } else { "normal" }.into(),
        "line-height" => match s.line_height {
            LineHeight::Normal => "normal".into(),
            LineHeight::Number(n) => px(n * s.font_size),
            LineHeight::Px(v) => px(v),
        },
        "text-align" => match s.text_align {
            TextAlign::Start => "start",
            TextAlign::End => "end",
            TextAlign::Left => "left",
            TextAlign::Right => "right",
            TextAlign::Center => "center",
            TextAlign::Justify => "justify",
            TextAlign::WebkitLeft => "-webkit-left",
            TextAlign::WebkitRight => "-webkit-right",
            TextAlign::WebkitCenter => "-webkit-center",
        }
        .into(),
        "direction" => if s.rtl { "rtl" } else { "ltr" }.into(),
        "text-decoration-line" => decoration_line(s.text_decoration_line),
        "text-decoration-color" => color(s.text_decoration_color.unwrap_or(s.color)),
        "text-transform" => match s.text_transform {
            TextTransform::None => "none",
            TextTransform::Uppercase => "uppercase",
            TextTransform::Lowercase => "lowercase",
            TextTransform::Capitalize => "capitalize",
        }
        .into(),
        "text-indent" => length(&s.text_indent),
        "white-space" => white_space(s).into(),
        "letter-spacing" if s.letter_spacing == 0.0 => "normal".into(),
        "letter-spacing" => px(s.letter_spacing),
        "word-spacing" => px(s.word_spacing),
        "vertical-align" => match s.vertical_align {
            VerticalAlign::Baseline => "baseline".into(),
            VerticalAlign::Sub => "sub".into(),
            VerticalAlign::Super => "super".into(),
            VerticalAlign::Top => "top".into(),
            VerticalAlign::Bottom => "bottom".into(),
            VerticalAlign::Middle => "middle".into(),
            VerticalAlign::TextTop => "text-top".into(),
            VerticalAlign::TextBottom => "text-bottom".into(),
            VerticalAlign::Length(v) => px(v),
        },
        "list-style-type" => list_style_type(&s.list_style_type),
        "list-style-position" => if s.list_style_inside {
            "inside"
        } else {
            "outside"
        }
        .into(),
        "word-break" => if s.word_break_all {
            "break-all"
        } else {
            "normal"
        }
        .into(),
        "flex-direction" => match s.flex_direction {
            FlexDirection::Row => "row",
            FlexDirection::RowReverse => "row-reverse",
            FlexDirection::Column => "column",
            FlexDirection::ColumnReverse => "column-reverse",
        }
        .into(),
        "flex-wrap" => match s.flex_wrap {
            FlexWrap::NoWrap => "nowrap",
            FlexWrap::Wrap => "wrap",
            FlexWrap::WrapReverse => "wrap-reverse",
        }
        .into(),
        "flex-grow" => number(s.flex_grow),
        "flex-shrink" => number(s.flex_shrink),
        "flex-basis" => length(&s.flex_basis),
        "order" => s.order.to_string(),
        "justify-content" => align(s.justify_content).into(),
        "align-items" => align(s.align_items).into(),
        "align-self" => s.align_self.map_or("auto", align).into(),
        "align-content" => align(s.align_content).into(),
        "justify-items" => align(s.justify_items).into(),
        "justify-self" => s.justify_self.map_or("auto", align).into(),
        "row-gap" => match &s.row_gap {
            Length::Auto => "normal".into(),
            l => length(l),
        },
        "column-gap" => match &s.column_gap {
            Length::Auto => "normal".into(),
            l => length(l),
        },
        "grid-auto-flow" => {
            let axis = if s.grid_auto_flow.column {
                "column"
            } else {
                "row"
            };
            if s.grid_auto_flow.dense {
                format!("{axis} dense")
            } else {
                axis.into()
            }
        }
        "grid-row-start" => grid_line(&s.grid_row_start),
        "grid-row-end" => grid_line(&s.grid_row_end),
        "grid-column-start" => grid_line(&s.grid_column_start),
        "grid-column-end" => grid_line(&s.grid_column_end),
        "border-collapse" => if s.border_collapse {
            "collapse"
        } else {
            "separate"
        }
        .into(),
        "border-spacing" => format!("{} {}", px(s.border_spacing), px(s.border_spacing)),
        "table-layout" => if s.table_layout_fixed {
            "fixed"
        } else {
            "auto"
        }
        .into(),
        _ => return shorthand(s, property).or_else(|| unrendered(s, property)),
    };
    Some(v)
}

/// `[top, right, bottom, left]` collapsed the way box shorthands serialize.
fn four(values: [String; 4]) -> String {
    let [t, r, b, l] = values;
    if r != l {
        format!("{t} {r} {b} {l}")
    } else if t != b {
        format!("{t} {r} {b}")
    } else if t != r {
        format!("{t} {r}")
    } else {
        t
    }
}

/// Shorthands serialized from their longhands; `""` when the longhands have no common
/// shorthand value (as in browsers).
fn shorthand(s: &ComputedStyle, property: &str) -> Option<String> {
    let get = |p: &str| computed_value(s, p).unwrap_or_default();
    let sides = |prefix: &str, suffix: &str| {
        four(["top", "right", "bottom", "left"].map(|side| get(&format!("{prefix}{side}{suffix}"))))
    };
    let v = match property {
        "margin" => sides("margin-", ""),
        "padding" => sides("padding-", ""),
        "inset" => four(["top", "right", "bottom", "left"].map(get)),
        "border-width" => sides("border-", "-width"),
        "border-style" => sides("border-", "-style"),
        "border-color" => sides("border-", "-color"),
        "border-radius" => get("border-top-left-radius"),
        "border-top" | "border-right" | "border-bottom" | "border-left" => format!(
            "{} {} {}",
            get(&format!("{property}-width")),
            get(&format!("{property}-style")),
            get(&format!("{property}-color"))
        ),
        "border" => {
            let top = shorthand(s, "border-top")?;
            let uniform = ["border-right", "border-bottom", "border-left"]
                .iter()
                .all(|side| shorthand(s, side).as_deref() == Some(top.as_str()));
            if uniform {
                top
            } else {
                String::new()
            }
        }
        "gap" => {
            let (row, column) = (get("row-gap"), get("column-gap"));
            if row == column {
                row
            } else {
                format!("{row} {column}")
            }
        }
        "flex" => format!(
            "{} {} {}",
            get("flex-grow"),
            get("flex-shrink"),
            get("flex-basis")
        ),
        "flex-flow" => format!("{} {}", get("flex-direction"), get("flex-wrap")),
        "text-decoration" => format!(
            "{} solid {}",
            get("text-decoration-line"),
            get("text-decoration-color")
        ),
        "grid-row" => format!("{} / {}", get("grid-row-start"), get("grid-row-end")),
        "grid-column" => format!("{} / {}", get("grid-column-start"), get("grid-column-end")),
        _ => return None,
    };
    Some(v)
}

/// Properties Axiom parses or ignores but does not render: the value in effect, which is
/// the initial one (no transforms, transitions, shadows, background images, …).
fn unrendered(s: &ComputedStyle, property: &str) -> Option<String> {
    let v = match property {
        "outline-color" | "caret-color" | "column-rule-color" | "-webkit-text-fill-color" => {
            return Some(color(s.color))
        }
        "transform" | "translate" | "rotate" | "scale" | "filter" | "backdrop-filter"
        | "box-shadow" | "text-shadow" | "clip-path" | "mask-image" | "perspective"
        | "animation-name" | "background-image" | "list-style-image" | "counter-reset"
        | "counter-increment" | "contain" | "resize" | "outline-style" | "max-lines" => "none",
        "transition-property" => "all",
        "transition-duration" | "transition-delay" | "animation-duration" | "animation-delay" => {
            "0s"
        }
        "transition-timing-function" | "animation-timing-function" => "ease",
        "transition" => "all 0s ease 0s",
        "animation-iteration-count" => "1",
        "animation-play-state" => "running",
        "animation-fill-mode" => "none",
        "animation-direction" => "normal",
        "cursor"
        | "pointer-events"
        | "user-select"
        | "will-change"
        | "touch-action"
        | "isolation"
        | "scroll-behavior"
        | "quotes"
        | "text-decoration-thickness" => "auto",
        "content"
        | "mix-blend-mode"
        | "font-variant"
        | "overflow-wrap"
        | "word-wrap"
        | "unicode-bidi"
        | "font-kerning"
        | "font-feature-settings"
        | "line-break" => "normal",
        "text-overflow" => "clip",
        "object-fit" => "fill",
        "object-position" | "perspective-origin" => "50% 50%",
        "backface-visibility" => "visible",
        "outline-width" | "outline-offset" => "0px",
        "writing-mode" => "horizontal-tb",
        "font-stretch" => "100%",
        "tab-size" => "8",
        "empty-cells" => "show",
        "caption-side" => "top",
        "text-decoration-style" => "solid",
        "background-repeat" => "repeat",
        "background-position" => "0% 0%",
        "background-size" => "auto",
        "background-attachment" => "scroll",
        "background-clip" => "border-box",
        "background-origin" => "padding-box",
        _ => return None,
    };
    Some(v.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_colors_print_like_browsers() {
        assert_eq!(number(10.0), "10");
        assert_eq!(number(10.5), "10.5");
        assert_eq!(number(1.0 / 3.0), "0.3333");
        assert_eq!(number(-0.0), "0");
        assert_eq!(color(Color::rgb(1, 2, 3)), "rgb(1, 2, 3)");
        assert_eq!(color(Color::TRANSPARENT), "rgba(0, 0, 0, 0)");
    }

    #[test]
    fn initial_values_serialize() {
        let s = ComputedStyle::initial();
        let get = |p: &str| computed_value(&s, p).unwrap();
        assert_eq!(get("display"), "inline");
        assert_eq!(get("margin-top"), "0px");
        assert_eq!(get("width"), "auto");
        assert_eq!(get("color"), "rgb(0, 0, 0)");
        assert_eq!(get("background-color"), "rgba(0, 0, 0, 0)");
        assert_eq!(get("font-size"), "16px");
        assert_eq!(get("border-top-width"), "0px");
        assert_eq!(get("white-space"), "normal");
        assert_eq!(get("z-index"), "auto");
        for p in COMPUTED_PROPERTIES {
            assert!(computed_value(&s, p).is_some(), "{p}");
        }
        assert_eq!(computed_value(&s, "no-such-property"), None);
    }

    #[test]
    fn shorthands_collapse_their_longhands() {
        let mut s = ComputedStyle::initial();
        s.margin.top = Length::Px(1.0);
        s.margin.bottom = Length::Px(1.0);
        s.margin.left = Length::Px(2.0);
        s.margin.right = Length::Px(2.0);
        assert_eq!(computed_value(&s, "margin").unwrap(), "1px 2px");
        s.margin.left = Length::Px(3.0);
        assert_eq!(computed_value(&s, "margin").unwrap(), "1px 2px 1px 3px");
        assert_eq!(computed_value(&s, "padding").unwrap(), "0px");
        assert_eq!(
            computed_value(&s, "border").unwrap(),
            "0px none rgb(0, 0, 0)"
        );
        assert_eq!(computed_value(&s, "flex").unwrap(), "0 1 auto");
        assert_eq!(computed_value(&s, "gap").unwrap(), "normal");
        assert_eq!(computed_value(&s, "transform").unwrap(), "none");
        assert_eq!(computed_value(&s, "transition-duration").unwrap(), "0s");
    }
}
