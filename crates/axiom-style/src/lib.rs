//! Style cascade and computed values.
//!
//! Selectors are compiled with the Selectors Level 4 engine in `axiom-dom` and bucketed
//! by the rightmost compound's ID, class or type. The cascade sorts declarations by
//! origin and importance (user-agent < presentational hints < author, with `!important`
//! reversing the origins), then specificity, then order. Custom properties and `var()`
//! are resolved per element, shorthands are expanded into longhands, and relative units
//! are computed against the element's font size, the root font size and the viewport.
//! `@media` and `@supports` are evaluated when a sheet is added, so the viewport must be
//! set first.

pub mod background;
pub mod generated;
pub mod grid;
mod media;
pub mod serialize;
mod values;

pub use axiom_dom::PseudoElement;
pub use background::{BgSize, BoxArea, ImageLayers, Repeat};
pub use generated::{Content, ContentItem, GeneratedBox};
pub use grid::{GridAreas, GridAutoFlow, GridLine, TrackList, TrackSize};
pub use media::{media_matches, supports_matches, MediaEnvironment};
pub use values::{
    parse_color, parse_length, parse_number, split_components, split_top_level, CalcNode, Color,
    Length, UnitContext,
};

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use axiom_css::{parse_declarations, parse_stylesheet, CssRule, Declaration, Stylesheet};
use axiom_dom::{ComplexSelector, Document, NodeId, NodeKind, Pseudo, SelectorList, Simple};
use axiom_text::FontSpec;
use values::has_font_relative_unit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    None,
    Contents,
    Block,
    Inline,
    InlineBlock,
    FlowRoot,
    ListItem,
    Flex,
    InlineFlex,
    Grid,
    InlineGrid,
    Table,
    InlineTable,
    TableRowGroup,
    TableHeaderGroup,
    TableFooterGroup,
    TableRow,
    TableCell,
    TableColumn,
    TableColumnGroup,
    TableCaption,
}

impl Display {
    pub fn is_inline_level(self) -> bool {
        matches!(
            self,
            Display::Inline
                | Display::InlineBlock
                | Display::InlineFlex
                | Display::InlineGrid
                | Display::InlineTable
        )
    }

    pub fn is_flex(self) -> bool {
        matches!(self, Display::Flex | Display::InlineFlex)
    }

    pub fn is_grid(self) -> bool {
        matches!(self, Display::Grid | Display::InlineGrid)
    }

    /// The block-level equivalent (for floats, absolutely positioned boxes, flex and
    /// grid items, and the root element).
    pub fn blockified(self) -> Display {
        match self {
            Display::Inline
            | Display::InlineBlock
            | Display::TableRowGroup
            | Display::TableHeaderGroup
            | Display::TableFooterGroup
            | Display::TableRow
            | Display::TableCell
            | Display::TableColumn
            | Display::TableColumnGroup
            | Display::TableCaption => Display::Block,
            Display::InlineFlex => Display::Flex,
            Display::InlineGrid => Display::Grid,
            Display::InlineTable => Display::Table,
            other => other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

impl Position {
    pub fn is_out_of_flow(self) -> bool {
        matches!(self, Position::Absolute | Position::Fixed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Float {
    None,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clear {
    None,
    Left,
    Right,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    Visible,
    Hidden,
    Clip,
    Scroll,
    Auto,
}

impl Overflow {
    pub fn clips(self) -> bool {
        !matches!(self, Overflow::Visible)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Start,
    End,
    Left,
    Right,
    Center,
    Justify,
    /// `-webkit-left` / `-webkit-right` / `-webkit-center`: like `left` / `right` /
    /// `center`, and also aligning block children whose side margins are not `auto`
    /// (the rendering of `<center>` and the HTML `align` attribute).
    WebkitLeft,
    WebkitRight,
    WebkitCenter,
}

impl TextAlign {
    /// Where a block child with non-`auto` side margins goes: 0 left, 0.5 centered, 1 right.
    pub fn block_alignment(self) -> Option<f32> {
        match self {
            TextAlign::WebkitLeft => Some(0.0),
            TextAlign::WebkitCenter => Some(0.5),
            TextAlign::WebkitRight => Some(1.0),
            _ => None,
        }
    }
}

/// `white-space-collapse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceCollapse {
    Collapse,
    Preserve,
    PreserveBreaks,
    BreakSpaces,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextTransform {
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VerticalAlign {
    Baseline,
    Sub,
    Super,
    Top,
    Bottom,
    Middle,
    TextTop,
    TextBottom,
    /// Raise (positive) or lower the baseline by pixels.
    Length(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ListStyleType {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
    DecimalLeadingZero,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
    DisclosureClosed,
    DisclosureOpen,
    String(Arc<str>),
}

/// SVG `fill` / `stroke` (SVG 2 §13.2). `currentColor` stays a keyword so each element
/// resolves it against its own `color`.
#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    None,
    Color(Color),
    CurrentColor,
    /// `url(#id) [fallback]`, kept verbatim for the SVG rasterizer.
    Url(Arc<str>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    None,
    Hidden,
    Solid,
    Dashed,
    Dotted,
    Double,
    Groove,
    Ridge,
    Inset,
    Outset,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderSide {
    /// Used width: zero when the style is `none` or `hidden`.
    pub width: f32,
    pub style: BorderStyle,
    pub color: Color,
}

impl Default for BorderSide {
    fn default() -> Self {
        Self {
            width: 3.0,
            style: BorderStyle::None,
            color: Color::BLACK,
        }
    }
}

impl BorderSide {
    pub fn used_width(&self) -> f32 {
        match self.style {
            BorderStyle::None | BorderStyle::Hidden => 0.0,
            _ => self.width,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHeight {
    Normal,
    Number(f32),
    Px(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

impl FlexDirection {
    pub fn is_row(self) -> bool {
        matches!(self, FlexDirection::Row | FlexDirection::RowReverse)
    }

    pub fn is_reverse(self) -> bool {
        matches!(
            self,
            FlexDirection::RowReverse | FlexDirection::ColumnReverse
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexWrap {
    NoWrap,
    Wrap,
    WrapReverse,
}

/// Box alignment values for `justify-*` / `align-*` / `place-*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Normal,
    Stretch,
    Start,
    End,
    Center,
    Baseline,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
    Left,
    Right,
}

pub const UNDERLINE: u8 = 1;
pub const OVERLINE: u8 = 2;
pub const LINE_THROUGH: u8 = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct Edges {
    pub top: Length,
    pub right: Length,
    pub bottom: Length,
    pub left: Length,
}

impl Edges {
    pub fn all(v: Length) -> Self {
        Self {
            top: v.clone(),
            right: v.clone(),
            bottom: v.clone(),
            left: v,
        }
    }
}

impl Default for Edges {
    fn default() -> Self {
        Self::all(Length::ZERO)
    }
}

#[derive(Debug, Clone)]
pub struct ComputedStyle {
    pub display: Display,
    pub position: Position,
    pub float: Float,
    pub clear: Clear,
    pub border_box_sizing: bool,
    /// `top` / `right` / `bottom` / `left`.
    pub inset: Edges,
    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    pub min_height: Length,
    pub max_width: Length,
    pub max_height: Length,
    pub aspect_ratio: Option<f32>,
    pub margin: Edges,
    pub padding: Edges,
    /// Top, right, bottom, left.
    pub border: [BorderSide; 4],
    pub border_radius: f32,
    pub color: Color,
    pub background_color: Color,
    pub background: ImageLayers,
    pub mask: ImageLayers,
    /// Only `::before` / `::after` use it.
    pub content: Content,
    pub opacity: f32,
    pub visibility_hidden: bool,
    pub overflow_x: Overflow,
    pub overflow_y: Overflow,
    pub z_index: Option<i32>,
    /// Raw `font-family` list, resolved by `axiom-text`.
    pub font_family: Arc<str>,
    pub font_size: f32,
    pub font_weight: u16,
    pub italic: bool,
    pub line_height: LineHeight,
    pub text_align: TextAlign,
    pub rtl: bool,
    /// Own `text-decoration-line` flags.
    pub text_decoration_line: u8,
    pub text_decoration_color: Option<Color>,
    /// Decorations propagated from this box and its ancestors (CSS Text Decoration §3).
    pub decorations_in_effect: u8,
    pub decoration_color: Color,
    pub text_transform: TextTransform,
    pub text_indent: Length,
    pub space_collapse: SpaceCollapse,
    pub nowrap: bool,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub vertical_align: VerticalAlign,
    pub list_style_type: ListStyleType,
    pub list_style_inside: bool,
    pub break_anywhere: bool,
    pub word_break_all: bool,
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Length,
    pub order: i32,
    pub justify_content: Align,
    pub align_items: Align,
    pub align_self: Option<Align>,
    pub align_content: Align,
    pub justify_items: Align,
    pub justify_self: Option<Align>,
    pub row_gap: Length,
    pub column_gap: Length,
    /// `None` for `none`.
    pub grid_template_columns: Option<Arc<TrackList>>,
    pub grid_template_rows: Option<Arc<TrackList>>,
    pub grid_template_areas: Option<Arc<GridAreas>>,
    /// `None` for `auto`.
    pub grid_auto_columns: Option<Arc<[TrackSize]>>,
    pub grid_auto_rows: Option<Arc<[TrackSize]>>,
    pub grid_auto_flow: GridAutoFlow,
    pub grid_row_start: GridLine,
    pub grid_row_end: GridLine,
    pub grid_column_start: GridLine,
    pub grid_column_end: GridLine,
    pub border_spacing: f32,
    pub border_collapse: bool,
    pub table_layout_fixed: bool,
    pub fill: Paint,
    pub stroke: Paint,
    pub stroke_width: Length,
    pub fill_opacity: f32,
    pub stroke_opacity: f32,
    /// Custom properties in effect (inherited).
    pub custom: Arc<HashMap<String, String>>,
}

impl ComputedStyle {
    pub fn initial() -> Self {
        Self {
            display: Display::Inline,
            position: Position::Static,
            float: Float::None,
            clear: Clear::None,
            border_box_sizing: false,
            inset: Edges::all(Length::Auto),
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            min_height: Length::Auto,
            max_width: Length::None,
            max_height: Length::None,
            aspect_ratio: None,
            margin: Edges::default(),
            padding: Edges::default(),
            border: [BorderSide::default(); 4],
            border_radius: 0.0,
            color: Color::BLACK,
            background_color: Color::TRANSPARENT,
            background: ImageLayers::initial(BoxArea::PaddingBox),
            mask: ImageLayers::initial(BoxArea::BorderBox),
            content: Content::Normal,
            opacity: 1.0,
            visibility_hidden: false,
            overflow_x: Overflow::Visible,
            overflow_y: Overflow::Visible,
            z_index: None,
            font_family: Arc::from("sans-serif"),
            font_size: 16.0,
            font_weight: 400,
            italic: false,
            line_height: LineHeight::Normal,
            text_align: TextAlign::Start,
            rtl: false,
            text_decoration_line: 0,
            text_decoration_color: None,
            decorations_in_effect: 0,
            decoration_color: Color::BLACK,
            text_transform: TextTransform::None,
            text_indent: Length::ZERO,
            space_collapse: SpaceCollapse::Collapse,
            nowrap: false,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            vertical_align: VerticalAlign::Baseline,
            list_style_type: ListStyleType::Disc,
            list_style_inside: false,
            break_anywhere: false,
            word_break_all: false,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            order: 0,
            justify_content: Align::Normal,
            align_items: Align::Normal,
            align_self: None,
            align_content: Align::Normal,
            justify_items: Align::Normal,
            justify_self: None,
            row_gap: Length::Auto,
            column_gap: Length::Auto,
            grid_template_columns: None,
            grid_template_rows: None,
            grid_template_areas: None,
            grid_auto_columns: None,
            grid_auto_rows: None,
            grid_auto_flow: GridAutoFlow::default(),
            grid_row_start: GridLine::Auto,
            grid_row_end: GridLine::Auto,
            grid_column_start: GridLine::Auto,
            grid_column_end: GridLine::Auto,
            border_spacing: 0.0,
            border_collapse: false,
            table_layout_fixed: false,
            fill: Paint::Color(Color::BLACK),
            stroke: Paint::None,
            stroke_width: Length::Px(1.0),
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            custom: Arc::new(HashMap::new()),
        }
    }

    /// Initial values with the inherited properties taken from `parent`.
    pub fn inherit_from(parent: &ComputedStyle) -> Self {
        let mut s = Self::initial();
        for prop in INHERITED {
            copy_property(&mut s, parent, prop);
        }
        s.custom = Arc::clone(&parent.custom);
        s.decorations_in_effect = parent.decorations_in_effect;
        s.decoration_color = parent.decoration_color;
        s
    }

    /// Used line height in pixels; `normal` uses the primary font's metrics factor.
    pub fn line_height_px(&self, normal_factor: f32) -> f32 {
        match self.line_height {
            LineHeight::Normal => self.font_size * normal_factor,
            LineHeight::Number(n) => self.font_size * n,
            LineHeight::Px(px) => px,
        }
    }

    pub fn preserves_spaces(&self) -> bool {
        matches!(
            self.space_collapse,
            SpaceCollapse::Preserve | SpaceCollapse::BreakSpaces
        )
    }

    pub fn preserves_newlines(&self) -> bool {
        !matches!(self.space_collapse, SpaceCollapse::Collapse)
    }

    pub fn wraps(&self) -> bool {
        !self.nowrap
    }

    pub fn border_widths(&self) -> [f32; 4] {
        [
            self.border[0].used_width(),
            self.border[1].used_width(),
            self.border[2].used_width(),
            self.border[3].used_width(),
        ]
    }

    pub fn is_positioned(&self) -> bool {
        self.position != Position::Static
    }
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self::initial()
    }
}

/// Computed styles of the rendered elements and text nodes (text nodes share their
/// parent's style). Nodes inside `display: none` subtrees have no entry.
#[derive(Debug, Clone, Default)]
pub struct StyleMap {
    styles: Vec<Option<Arc<ComputedStyle>>>,
    count: usize,
    generated: HashMap<(NodeId, PseudoElement), GeneratedBox>,
}

impl StyleMap {
    /// The `::before` / `::after` box of element `id`, when its `content` generates one.
    pub fn generated(&self, id: NodeId, pe: PseudoElement) -> Option<&GeneratedBox> {
        self.generated.get(&(id, pe))
    }

    pub fn insert_generated(&mut self, id: NodeId, pe: PseudoElement, g: GeneratedBox) {
        self.generated.insert((id, pe), g);
    }

    /// Every generated box, in no particular order.
    pub fn generated_boxes(&self) -> impl Iterator<Item = (NodeId, PseudoElement, &GeneratedBox)> {
        self.generated.iter().map(|(&(id, pe), g)| (id, pe, g))
    }

    pub fn get(&self, id: NodeId) -> Option<&Arc<ComputedStyle>> {
        self.styles.get(id.0).and_then(Option::as_ref)
    }

    pub fn insert(&mut self, id: NodeId, style: Arc<ComputedStyle>) {
        if self.styles.len() <= id.0 {
            self.styles.resize(id.0 + 1, None);
        }
        if self.styles[id.0].replace(style).is_none() {
            self.count += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The styled nodes in `NodeId` order.
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Arc<ComputedStyle>)> {
        self.styles
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (NodeId(i), s)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    UserAgent,
    Author,
}

struct CompiledRule {
    selector: SelectorList,
    specificity: u32,
    origin: Origin,
    order: u32,
    declarations: Arc<[Declaration]>,
    /// `::before` / `::after` rules style that pseudo-element of the subject.
    pseudo: Option<PseudoElement>,
}

/// Style rules bucketed by a key of their subject compound.
#[derive(Default)]
struct RuleSet {
    /// `::before` / `::after` rules, bucketed the same way.
    generated: Option<Box<RuleSet>>,
    rules: Vec<CompiledRule>,
    by_id: HashMap<String, Vec<u32>>,
    by_class: HashMap<String, Vec<u32>>,
    by_tag: HashMap<String, Vec<u32>>,
    universal: Vec<u32>,
    /// Subjects with `:host` / `:host-context()`: only the shadow host can match.
    host: Vec<u32>,
    /// Subjects with `::slotted()`: they match the host's slotted children.
    slotted: Vec<u32>,
}

impl RuleSet {
    fn add(&mut self, rules: &[CssRule], origin: Origin, env: &MediaEnvironment) {
        for rule in rules {
            match rule {
                CssRule::Style(style) => {
                    let Some(list) = axiom_dom::parse_selector_list(&style.selectors) else {
                        continue;
                    };
                    let declarations: Arc<[Declaration]> = style.declarations.clone().into();
                    for complex in list.0 {
                        match complex.pseudo_element {
                            None => self.push(complex, origin, &declarations),
                            Some(PseudoElement::Other) => {}
                            Some(_) => self.generated.get_or_insert_with(Default::default).push(
                                complex,
                                origin,
                                &declarations,
                            ),
                        }
                    }
                }
                CssRule::Media { query, rules } => {
                    if media_matches(query, env) {
                        self.add(rules, origin, env);
                    }
                }
                CssRule::Supports { condition, rules } => {
                    if supports_matches(condition) {
                        self.add(rules, origin, env);
                    }
                }
                CssRule::Group { rules, .. } => self.add(rules, origin, env),
                CssRule::Import { .. } | CssRule::Other { .. } => {}
            }
        }
    }

    fn push(
        &mut self,
        complex: ComplexSelector,
        origin: Origin,
        declarations: &Arc<[Declaration]>,
    ) {
        let pseudo = complex.pseudo_element;
        // A pseudo-element counts like a type selector.
        let specificity =
            complex_specificity(&complex.compounds) + u32::from(pseudo.is_some()).min(255);
        let key = bucket_key(complex.compounds.last());
        let index = self.rules.len() as u32;
        self.rules.push(CompiledRule {
            selector: SelectorList(vec![complex]),
            specificity,
            origin,
            order: index,
            declarations: Arc::clone(declarations),
            pseudo,
        });
        match key {
            Bucket::Id(id) => self.by_id.entry(id).or_default().push(index),
            Bucket::Class(c) => self.by_class.entry(c).or_default().push(index),
            Bucket::Tag(t) => self.by_tag.entry(t).or_default().push(index),
            Bucket::Universal => self.universal.push(index),
            Bucket::Host => self.host.push(index),
            Bucket::Slotted => self.slotted.push(index),
        }
    }

    /// Indices of the rules whose bucket fits element `id`, sorted and unique.
    fn candidates(&self, doc: &Document, id: NodeId, tag: &str) -> Vec<u32> {
        let mut candidates: Vec<u32> = self.universal.clone();
        if let Some(v) = self.by_tag.get(tag) {
            candidates.extend_from_slice(v);
        }
        if let Some(idv) = doc.attr(id, "id") {
            if let Some(v) = self.by_id.get(&idv.to_ascii_lowercase()) {
                candidates.extend_from_slice(v);
            }
        }
        if let Some(class) = doc.attr(id, "class") {
            for c in class.split_ascii_whitespace() {
                if let Some(v) = self.by_class.get(&c.to_ascii_lowercase()) {
                    candidates.extend_from_slice(v);
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        candidates
    }

    /// Adds the declarations of rules `indices` that match `id` (or its pseudo-element
    /// `want`) to `decls`. With `host` the rules are that host's shadow tree's; `inner`
    /// marks rules reaching `id` from a shadow tree it hosts or is slotted into.
    #[allow(clippy::too_many_arguments)]
    fn cascade<'s>(
        &'s self,
        doc: &Document,
        id: NodeId,
        host: Option<NodeId>,
        indices: &[u32],
        inner: bool,
        want: Option<PseudoElement>,
        decls: &mut Vec<((u8, u32, u32), &'s Declaration)>,
    ) {
        for &i in indices {
            let rule = &self.rules[i as usize];
            if rule.pseudo != want {
                continue;
            }
            let matched = match (rule.pseudo, host) {
                (Some(pe), host) => axiom_dom::matches_pseudo(doc, id, &rule.selector, pe, host),
                (None, Some(h)) => axiom_dom::matches_in_shadow(doc, id, &rule.selector, h),
                (None, None) => axiom_dom::matches(doc, id, &rule.selector, None),
            };
            if !matched {
                continue;
            }
            for d in rule.declarations.iter() {
                let rank = cascade_rank(rule.origin, d.important, inner);
                decls.push(((rank, rule.specificity, rule.order), d));
            }
        }
    }
}

/// Cascade rank of a declaration: origin and importance, with CSS Scoping's context
/// step. For normal declarations the outer tree (the element's own) beats rules from a
/// shadow tree it hosts or is slotted into (`:host`, `::slotted()`); `!important`
/// reverses that.
fn cascade_rank(origin: Origin, important: bool, inner: bool) -> u8 {
    match (origin, important) {
        (Origin::UserAgent, false) => 0,
        (Origin::Author, false) if inner => 2,
        (Origin::Author, false) => 3,
        (Origin::Author, true) if inner => 5,
        (Origin::Author, true) => 4,
        (Origin::UserAgent, true) => 6,
    }
}

pub struct StyleEngine {
    env: MediaEnvironment,
    ua: RuleSet,
    author: RuleSet,
    /// Shadow root → the rules of its `<style>` sheets.
    shadow: HashMap<NodeId, RuleSet>,
    font_aliases: HashMap<String, String>,
    style_attribute_filter: Option<Box<StyleAttributeFilter>>,
}

/// Decides whether an element's `style` attribute value applies.
pub type StyleAttributeFilter = dyn Fn(NodeId, &str) -> bool;

fn ua_sheet() -> &'static Stylesheet {
    static UA: OnceLock<Stylesheet> = OnceLock::new();
    UA.get_or_init(axiom_css::user_agent_stylesheet)
}

impl StyleEngine {
    /// An engine with the user-agent sheet for a 1024×768 viewport.
    pub fn new() -> Self {
        Self::with_viewport(1024.0, 768.0)
    }

    pub fn with_viewport(width: f32, height: f32) -> Self {
        let env = MediaEnvironment { width, height };
        let mut ua = RuleSet::default();
        ua.add(&ua_sheet().rules, Origin::UserAgent, &env);
        Self {
            env,
            ua,
            author: RuleSet::default(),
            shadow: HashMap::new(),
            font_aliases: HashMap::new(),
            style_attribute_filter: None,
        }
    }

    /// `style` attributes apply only where `filter` returns `true` (the document's
    /// Content Security Policy). Without a filter they all apply.
    pub fn set_style_attribute_filter(&mut self, filter: impl Fn(NodeId, &str) -> bool + 'static) {
        self.style_attribute_filter = Some(Box::new(filter));
    }

    /// Maps `font-family` names (lowercase) to the keys their web fonts were registered
    /// under with `axiom_text::register_web_font`.
    pub fn set_font_aliases(&mut self, aliases: HashMap<String, String>) {
        self.font_aliases = aliases;
    }

    fn alias_families(&self, list: &str) -> Arc<str> {
        let mut changed = false;
        let names: Vec<&str> = list
            .split(',')
            .map(|f| {
                let name = f
                    .trim()
                    .trim_matches(['"', '\''])
                    .trim()
                    .to_ascii_lowercase();
                match self.font_aliases.get(&name) {
                    Some(key) => {
                        changed = true;
                        key.as_str()
                    }
                    None => f.trim(),
                }
            })
            .collect();
        if changed {
            Arc::from(names.join(", "))
        } else {
            Arc::from(list)
        }
    }

    pub fn viewport(&self) -> MediaEnvironment {
        self.env
    }

    pub fn add_author_css(&mut self, css: &str) {
        let sheet = parse_stylesheet(css);
        self.add_stylesheet(sheet);
    }

    pub fn add_stylesheet(&mut self, sheet: Stylesheet) {
        self.author.add(&sheet.rules, Origin::Author, &self.env);
    }

    /// Adds `css` to the rules of shadow root `root`'s tree.
    pub fn add_shadow_css(&mut self, root: NodeId, css: &str) {
        let sheet = parse_stylesheet(css);
        self.shadow
            .entry(root)
            .or_default()
            .add(&sheet.rules, Origin::Author, &self.env);
    }

    pub fn rule_count(&self) -> usize {
        self.ua.rules.len()
            + self.author.rules.len()
            + self.shadow.values().map(|s| s.rules.len()).sum::<usize>()
    }

    /// Styles every node in the flat tree. Light-tree children of a shadow host that no
    /// slot takes are not rendered and get no style.
    pub fn compute_document(&self, doc: &Document) -> StyleMap {
        let mut map = StyleMap::default();
        let Some(root) = doc.document_id else {
            return map;
        };
        let initial = Arc::new(ComputedStyle::initial());
        let mut root_font_size = 16.0;
        // (node, parent style, is the root element, host of the shadow tree it is in)
        let mut stack: Vec<(NodeId, Arc<ComputedStyle>, bool, Option<NodeId>)> = doc
            .get(root)
            .children
            .iter()
            .rev()
            .map(|&c| (c, Arc::clone(&initial), true, None))
            .collect();
        // Shadow host → the host of the tree the host itself is in.
        let mut host_scopes: HashMap<NodeId, Option<NodeId>> = HashMap::new();
        while let Some((id, parent, is_root, scope)) = stack.pop() {
            let node = doc.get(id);
            let style = match &node.kind {
                NodeKind::Element { tag, .. } => {
                    let tag = tag.to_ascii_lowercase();
                    let rem = if is_root { 16.0 } else { root_font_size };
                    let cx = ElementContext {
                        is_root,
                        rem,
                        scope,
                    };
                    let style = self.compute_element(doc, id, &tag, &parent, cx);
                    if is_root {
                        root_font_size = style.font_size;
                    }
                    if style.display != Display::None {
                        for pe in [PseudoElement::Before, PseudoElement::After] {
                            if let Some(g) = self.compute_generated(doc, id, &tag, &style, cx, pe) {
                                map.insert_generated(id, pe, g);
                            }
                        }
                    }
                    Arc::new(style)
                }
                NodeKind::Text { .. } | NodeKind::CData { .. } => {
                    map.insert(id, parent);
                    continue;
                }
                _ => continue,
            };
            map.insert(id, Arc::clone(&style));
            if style.display == Display::None {
                continue;
            }
            let (children, child_scope): (Cow<'_, [NodeId]>, Option<NodeId>) =
                if let Some(shadow) = doc.shadow_root(id) {
                    host_scopes.insert(id, scope);
                    (Cow::Borrowed(&doc.get(shadow.root).children), Some(id))
                } else {
                    let assigned = match scope {
                        Some(_) => doc.assigned_nodes(id),
                        None => Vec::new(),
                    };
                    if assigned.is_empty() {
                        (Cow::Borrowed(&node.children), scope)
                    } else {
                        let outer = scope.and_then(|h| host_scopes.get(&h).copied().flatten());
                        (Cow::Owned(assigned), outer)
                    }
                };
            for &child in children.iter().rev() {
                stack.push((child, Arc::clone(&style), false, child_scope));
            }
        }
        map
    }

    /// Style of element `node` that `compute_document` skipped (inside a `display: none`
    /// subtree), cascaded down from its nearest styled ancestor. `None` when `node` is not
    /// an element connected to the document.
    pub fn compute_unstyled(
        &self,
        doc: &Document,
        map: &StyleMap,
        node: NodeId,
    ) -> Option<ComputedStyle> {
        let root = doc.document_id?;
        if !matches!(doc.get(node).kind, NodeKind::Element { .. }) {
            return None;
        }
        if let Some(style) = map.get(node) {
            return Some((**style).clone());
        }
        let mut chain = Vec::new();
        let mut cur = node;
        let (mut parent, mut is_root) = loop {
            if let Some(style) = map.get(cur) {
                break ((**style).clone(), false);
            }
            if cur == root {
                break (ComputedStyle::initial(), true);
            }
            chain.push(cur);
            cur = match doc.flat_tree_parent(cur) {
                Some(p) => p,
                // An unassigned child of a shadow host: styled from the host.
                None => doc
                    .get(cur)
                    .parent
                    .filter(|&p| doc.shadow_root(p).is_some())?,
            };
        };
        let rem = doc
            .get(root)
            .children
            .iter()
            .find_map(|&c| map.get(c))
            .map_or(16.0, |s| s.font_size);
        let mut result = None;
        for &id in chain.iter().rev() {
            let NodeKind::Element { tag, .. } = &doc.get(id).kind else {
                continue;
            };
            let tag = tag.to_ascii_lowercase();
            let cx = ElementContext {
                is_root,
                rem,
                scope: doc.shadow_host(doc.tree_root(id)),
            };
            let style = self.compute_element(doc, id, &tag, &parent, cx);
            is_root = false;
            parent = style.clone();
            result = Some(style);
        }
        result
    }

    fn compute_element(
        &self,
        doc: &Document,
        id: NodeId,
        tag: &str,
        parent: &ComputedStyle,
        cx: ElementContext,
    ) -> ComputedStyle {
        let ElementContext {
            is_root,
            rem,
            scope,
        } = cx;
        let mut decls: Vec<((u8, u32, u32), &Declaration)> = Vec::new();
        let ua = &self.ua;
        let candidates = ua.candidates(doc, id, tag);
        ua.cascade(doc, id, None, &candidates, false, None, &mut decls);
        // The element's own tree: the document's sheets, or its shadow root's.
        if let Some(set) = self.own_rules(doc, scope) {
            let candidates = set.candidates(doc, id, tag);
            set.cascade(doc, id, scope, &candidates, false, None, &mut decls);
        }
        // `:host` rules of the shadow tree this element hosts.
        if let Some(set) = doc.shadow_root(id).and_then(|s| self.shadow.get(&s.root)) {
            set.cascade(doc, id, Some(id), &set.host, true, None, &mut decls);
        }
        // `::slotted()` rules of the shadow tree this element is slotted into.
        if let Some(host) = doc.get(id).parent {
            if let Some(set) = doc.shadow_root(host).and_then(|s| self.shadow.get(&s.root)) {
                set.cascade(doc, id, Some(host), &set.slotted, true, None, &mut decls);
            }
        }
        let hints = presentational_hints(doc, id, tag);
        for d in &hints {
            decls.push(((1, 0, 0), d));
        }
        let inline = doc
            .attr(id, "style")
            .filter(|v| {
                self.style_attribute_filter
                    .as_ref()
                    .is_none_or(|f| f(id, v))
            })
            .map(parse_declarations);
        if let Some(inline) = &inline {
            for (n, d) in inline.iter().enumerate() {
                let rank = cascade_rank(Origin::Author, d.important, false);
                decls.push(((rank, u32::MAX, n as u32), d));
            }
        }
        decls.sort_by_key(|(k, _)| *k);
        self.resolve(&decls, parent, is_root, rem)
    }

    /// The rules of the tree an element is in: the document's sheets, or the sheets of
    /// the shadow root of `scope`.
    fn own_rules(&self, doc: &Document, scope: Option<NodeId>) -> Option<&RuleSet> {
        match scope {
            None => Some(&self.author),
            Some(host) => doc.shadow_root(host).and_then(|s| self.shadow.get(&s.root)),
        }
    }

    /// Style of pseudo-element `pe` of element `id` (styled `style`), and its text, when
    /// its `content` generates a box.
    fn compute_generated(
        &self,
        doc: &Document,
        id: NodeId,
        tag: &str,
        style: &ComputedStyle,
        cx: ElementContext,
        pe: PseudoElement,
    ) -> Option<GeneratedBox> {
        let mut decls: Vec<((u8, u32, u32), &Declaration)> = Vec::new();
        let sets = [Some(&self.ua), self.own_rules(doc, cx.scope)];
        for (n, set) in sets.into_iter().enumerate() {
            let Some(set) = set.and_then(|s| s.generated.as_deref()) else {
                continue;
            };
            let host = if n == 0 { None } else { cx.scope };
            let candidates = set.candidates(doc, id, tag);
            set.cascade(doc, id, host, &candidates, false, Some(pe), &mut decls);
        }
        if decls.is_empty() {
            return None;
        }
        decls.sort_by_key(|(k, _)| *k);
        let s = self.resolve(&decls, style, false, cx.rem);
        let Content::Items(items) = &s.content else {
            return None;
        };
        if s.display == Display::None {
            return None;
        }
        let mut text = String::new();
        for item in items.iter() {
            match item {
                ContentItem::Text(t) => text.push_str(t),
                ContentItem::Attr(name) => text.push_str(doc.attr(id, name).unwrap_or("")),
                ContentItem::OpenQuote => text.push('\u{201C}'),
                ContentItem::CloseQuote => text.push('\u{201D}'),
            }
        }
        Some(GeneratedBox {
            style: Arc::new(s),
            text,
        })
    }

    /// The computed style from the sorted declarations of an element (or pseudo-element)
    /// whose parent is styled `parent`.
    fn resolve(
        &self,
        decls: &[((u8, u32, u32), &Declaration)],
        parent: &ComputedStyle,
        is_root: bool,
        rem: f32,
    ) -> ComputedStyle {
        // Custom properties.
        let mut custom = Arc::clone(&parent.custom);
        if decls.iter().any(|(_, d)| d.property.starts_with("--")) {
            let map = Arc::make_mut(&mut custom);
            for (_, d) in decls {
                if d.property.starts_with("--") {
                    let v = d.value.trim();
                    match v.to_ascii_lowercase().as_str() {
                        "initial" => {
                            map.remove(&d.property);
                        }
                        "inherit" | "unset" | "revert" | "revert-layer" => {
                            match parent.custom.get(&d.property) {
                                Some(pv) => {
                                    map.insert(d.property.clone(), pv.clone());
                                }
                                None => {
                                    map.remove(&d.property);
                                }
                            }
                        }
                        _ => {
                            map.insert(d.property.clone(), v.to_string());
                        }
                    }
                }
            }
            let snapshot = map.clone();
            for (name, value) in map.iter_mut() {
                if value.contains("var(") {
                    *value = substitute_vars(value, &snapshot, 0, name).unwrap_or_default();
                }
            }
        }

        // Longhands in cascade order.
        let mut longhands = Longhands::default();
        for (_, d) in decls {
            if d.property.starts_with("--") {
                continue;
            }
            let substituted;
            let value = if d.value.contains("var(") {
                substituted = substitute_vars(&d.value, &custom, 0, "")
                    .unwrap_or_else(|| "unset".to_string());
                substituted.as_str()
            } else {
                d.value.as_str()
            };
            expand(&d.property, value, &mut longhands);
        }

        let mut s = ComputedStyle::inherit_from(parent);
        s.custom = custom;
        // `ex`/`ch` need the font's metrics; only look them up when a value uses them.
        let font_units = longhands
            .entries
            .iter()
            .any(|(_, v)| has_font_relative_unit(v));
        let font_cx = |st: &ComputedStyle, cx: &mut UnitContext| {
            if font_units {
                let spec = FontSpec::new(&st.font_family, st.font_weight, st.italic, st.font_size);
                let m = axiom_text::font_metrics(&spec);
                (cx.ex, cx.ch) = (m.x_height, m.ch_width);
            } else {
                (cx.ex, cx.ch) = (st.font_size * 0.5, st.font_size * 0.5);
            }
        };
        let mut cx = UnitContext {
            em: parent.font_size,
            rem,
            vw: self.env.width,
            vh: self.env.height,
            ex: 0.0,
            ch: 0.0,
        };
        font_cx(parent, &mut cx);
        // The font first (other lengths depend on it), then `color` (for currentcolor).
        if let Some(v) = longhands.get("font-size") {
            apply(&mut s, "font-size", v, parent, &cx);
        }
        cx.em = s.font_size;
        if is_root {
            cx.rem = s.font_size;
        }
        for prop in ["font-family", "font-weight", "font-style"] {
            if let Some(v) = longhands.get(prop) {
                apply(&mut s, prop, v, parent, &cx);
            }
        }
        if longhands.get("font-family").is_some() && !self.font_aliases.is_empty() {
            s.font_family = self.alias_families(&s.font_family);
        }
        font_cx(&s, &mut cx);
        if let Some(v) = longhands.get("color") {
            apply(&mut s, "color", v, parent, &cx);
        }
        for side in &mut s.border {
            side.color = s.color;
        }
        for (prop, value) in &longhands.entries {
            if !matches!(
                *prop,
                "font-size" | "color" | "font-family" | "font-weight" | "font-style"
            ) {
                apply(&mut s, prop, value, parent, &cx);
            }
        }

        // Blockification.
        let parent_display = if is_root { None } else { Some(parent.display) };
        if is_root
            || s.float != Float::None
            || s.position.is_out_of_flow()
            || parent_display.is_some_and(|d| d.is_flex() || d.is_grid())
        {
            s.display = s.display.blockified();
        }
        if s.position.is_out_of_flow() {
            s.float = Float::None;
        }
        if s.text_decoration_line != 0 {
            s.decorations_in_effect |= s.text_decoration_line;
            s.decoration_color = s.text_decoration_color.unwrap_or(s.color);
        }
        s
    }
}

impl Default for StyleEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-element inputs to the cascade besides the element and its parent's style.
#[derive(Clone, Copy)]
struct ElementContext {
    is_root: bool,
    /// Root element font size, for `rem`.
    rem: f32,
    /// Host of the shadow tree the element is in; `None` in the document tree.
    scope: Option<NodeId>,
}

enum Bucket {
    Id(String),
    Class(String),
    Tag(String),
    Universal,
    Host,
    Slotted,
}

fn bucket_key(compound: Option<&axiom_dom::Compound>) -> Bucket {
    let Some(c) = compound else {
        return Bucket::Universal;
    };
    for p in &c.parts {
        match p {
            Simple::Pseudo(Pseudo::Slotted(_)) => return Bucket::Slotted,
            Simple::Pseudo(Pseudo::Host(_) | Pseudo::HostContext(_)) => return Bucket::Host,
            _ => {}
        }
    }
    let mut class = None;
    let mut tag = None;
    // ID and class keys are ASCII-lowercased (quirks mode matches them
    // case-insensitively); the selector match afterwards is exact.
    for p in &c.parts {
        match p {
            Simple::Id(id) => return Bucket::Id(id.to_ascii_lowercase()),
            Simple::Class(cl) if class.is_none() => class = Some(cl.to_ascii_lowercase()),
            Simple::Type { lower, .. } if tag.is_none() => tag = Some(lower.clone()),
            _ => {}
        }
    }
    if let Some(c) = class {
        return Bucket::Class(c);
    }
    match tag {
        Some(t) => Bucket::Tag(t),
        None => Bucket::Universal,
    }
}

fn complex_specificity(compounds: &[axiom_dom::Compound]) -> u32 {
    let (mut a, mut b, mut c) = (0u32, 0u32, 0u32);
    for compound in compounds {
        let (x, y, z) = compound_specificity(&compound.parts);
        a += x;
        b += y;
        c += z;
    }
    (a.min(255) << 16) | (b.min(255) << 8) | c.min(255)
}

fn list_specificity(list: &SelectorList) -> (u32, u32, u32) {
    list.0
        .iter()
        .map(|cs| {
            let s = complex_specificity(&cs.compounds);
            (s >> 16, (s >> 8) & 255, s & 255)
        })
        .max()
        .unwrap_or((0, 0, 0))
}

fn compound_specificity(parts: &[Simple]) -> (u32, u32, u32) {
    let (mut a, mut b, mut c) = (0, 0, 0);
    for p in parts {
        match p {
            Simple::Id(_) => a += 1,
            Simple::Class(_) | Simple::Attr { .. } => b += 1,
            Simple::Type { .. } => c += 1,
            Simple::Universal(_) | Simple::HasAnchor => {}
            Simple::Pseudo(ps) => {
                let (x, y, z) = match ps {
                    Pseudo::Where(_) => (0, 0, 0),
                    Pseudo::Is(l) | Pseudo::Not(l) | Pseudo::Has(l) => list_specificity(l),
                    Pseudo::Nth { of: Some(l), .. }
                    | Pseudo::Host(Some(l))
                    | Pseudo::HostContext(l) => {
                        let (x, y, z) = list_specificity(l);
                        (x, y + 1, z)
                    }
                    Pseudo::Slotted(l) => {
                        let (x, y, z) = list_specificity(l);
                        (x, y, z + 1)
                    }
                    _ => (0, 1, 0),
                };
                a += x;
                b += y;
                c += z;
            }
        }
    }
    (a, b, c)
}

/// Replace `var(--name[, fallback])` references; `None` when a reference is invalid
/// (the declaration is then invalid at computed-value time).
fn substitute_vars(
    value: &str,
    custom: &HashMap<String, String>,
    depth: u32,
    self_name: &str,
) -> Option<String> {
    if depth > 16 {
        return None;
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(pos) = find_var(rest) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 4..];
        let close = matching_paren(after)?;
        let inner = &after[..close];
        let (name, fallback) = match inner.find(',') {
            Some(c) => (inner[..c].trim(), Some(inner[c + 1..].trim())),
            None => (inner.trim(), None),
        };
        let resolved = match custom.get(name).filter(|_| name != self_name) {
            Some(v) if !v.trim().is_empty() => {
                if v.contains("var(") {
                    substitute_vars(v, custom, depth + 1, self_name)
                } else {
                    Some(v.clone())
                }
            }
            _ => None,
        };
        let resolved = match (resolved, fallback) {
            (Some(v), _) => v,
            (None, Some(fb)) => {
                if fb.contains("var(") {
                    substitute_vars(fb, custom, depth + 1, self_name)?
                } else {
                    fb.to_string()
                }
            }
            (None, None) => return None,
        };
        out.push_str(&resolved);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn find_var(s: &str) -> Option<usize> {
    let lower = s.as_bytes();
    let mut i = 0;
    while i + 4 <= lower.len() {
        if lower[i..i + 4].eq_ignore_ascii_case(b"var(")
            && (i == 0 || !(lower[i - 1].is_ascii_alphanumeric() || lower[i - 1] == b'-'))
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(i),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Longhand declarations in cascade order (later entries override earlier ones).
#[derive(Default)]
struct Longhands {
    entries: Vec<(&'static str, String)>,
}

impl Longhands {
    fn set(&mut self, prop: &'static str, value: impl Into<String>) {
        let value = value.into();
        if let Some(e) = self.entries.iter_mut().find(|(p, _)| *p == prop) {
            e.1 = value;
        } else {
            self.entries.push((prop, value));
        }
    }

    fn get(&self, prop: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(p, _)| *p == prop)
            .map(|(_, v)| v.as_str())
    }
}

static LONGHANDS: &[&str] = &[
    "display",
    "position",
    "float",
    "clear",
    "box-sizing",
    "top",
    "right",
    "bottom",
    "left",
    "width",
    "height",
    "min-width",
    "min-height",
    "max-width",
    "max-height",
    "aspect-ratio",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "border-top-width",
    "border-right-width",
    "border-bottom-width",
    "border-left-width",
    "border-top-style",
    "border-right-style",
    "border-bottom-style",
    "border-left-style",
    "border-top-color",
    "border-right-color",
    "border-bottom-color",
    "border-left-color",
    "border-top-left-radius",
    "color",
    "content",
    "background-color",
    "background-image",
    "background-position-x",
    "background-position-y",
    "background-size",
    "background-repeat",
    "background-origin",
    "background-clip",
    "mask-image",
    "mask-position-x",
    "mask-position-y",
    "mask-size",
    "mask-repeat",
    "mask-origin",
    "mask-clip",
    "opacity",
    "visibility",
    "overflow-x",
    "overflow-y",
    "z-index",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "line-height",
    "text-align",
    "direction",
    "text-decoration-line",
    "text-decoration-color",
    "text-transform",
    "text-indent",
    "white-space-collapse",
    "text-wrap-mode",
    "letter-spacing",
    "word-spacing",
    "vertical-align",
    "list-style-type",
    "list-style-position",
    "overflow-wrap",
    "word-break",
    "flex-direction",
    "flex-wrap",
    "flex-grow",
    "flex-shrink",
    "flex-basis",
    "order",
    "justify-content",
    "align-items",
    "align-self",
    "align-content",
    "justify-items",
    "justify-self",
    "row-gap",
    "column-gap",
    "grid-template-columns",
    "grid-template-rows",
    "grid-template-areas",
    "grid-auto-columns",
    "grid-auto-rows",
    "grid-auto-flow",
    "grid-row-start",
    "grid-row-end",
    "grid-column-start",
    "grid-column-end",
    "border-spacing",
    "border-collapse",
    "table-layout",
    "fill",
    "stroke",
    "stroke-width",
    "fill-opacity",
    "stroke-opacity",
];

static SHORTHANDS: &[&str] = &[
    "margin",
    "padding",
    "inset",
    "border",
    "border-top",
    "border-right",
    "border-bottom",
    "border-left",
    "border-width",
    "border-style",
    "border-color",
    "border-radius",
    "border-block",
    "border-inline",
    "border-block-start",
    "border-block-end",
    "border-inline-start",
    "border-inline-end",
    "margin-block",
    "margin-inline",
    "padding-block",
    "padding-inline",
    "inset-block",
    "inset-inline",
    "background",
    "background-position",
    "mask",
    "mask-position",
    "font",
    "list-style",
    "flex",
    "flex-flow",
    "gap",
    "grid-gap",
    "place-items",
    "place-content",
    "place-self",
    "text-decoration",
    "overflow",
    "white-space",
    "text-wrap",
    "grid-row",
    "grid-column",
    "grid-area",
    "grid-template",
    "word-wrap",
];

static INHERITED: &[&str] = &[
    "color",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "line-height",
    "text-align",
    "direction",
    "text-transform",
    "text-indent",
    "white-space-collapse",
    "text-wrap-mode",
    "letter-spacing",
    "word-spacing",
    "list-style-type",
    "list-style-position",
    "visibility",
    "overflow-wrap",
    "word-break",
    "border-spacing",
    "border-collapse",
    "fill",
    "stroke",
    "stroke-width",
    "fill-opacity",
    "stroke-opacity",
];

/// Whether Axiom implements `property` (for `@supports`).
pub fn is_supported_property(property: &str) -> bool {
    let p = canonical_name(property);
    LONGHANDS.contains(&p.as_str())
        || SHORTHANDS.contains(&p.as_str())
        || logical_longhand(&p).is_some()
}

/// Lowercase and strip vendor prefixes (`-webkit-box-sizing` → `box-sizing`).
fn canonical_name(property: &str) -> String {
    let p = property.to_ascii_lowercase();
    for prefix in ["-webkit-", "-moz-", "-ms-", "-o-"] {
        if let Some(rest) = p.strip_prefix(prefix) {
            if LONGHANDS.contains(&rest) || SHORTHANDS.contains(&rest) {
                return rest.to_string();
            }
        }
    }
    p
}

/// Logical properties for a horizontal, left-to-right writing mode.
fn logical_longhand(p: &str) -> Option<&'static str> {
    Some(match p {
        "inline-size" => "width",
        "block-size" => "height",
        "min-inline-size" => "min-width",
        "min-block-size" => "min-height",
        "max-inline-size" => "max-width",
        "max-block-size" => "max-height",
        "margin-block-start" => "margin-top",
        "margin-block-end" => "margin-bottom",
        "margin-inline-start" => "margin-left",
        "margin-inline-end" => "margin-right",
        "padding-block-start" => "padding-top",
        "padding-block-end" => "padding-bottom",
        "padding-inline-start" => "padding-left",
        "padding-inline-end" => "padding-right",
        "inset-block-start" => "top",
        "inset-block-end" => "bottom",
        "inset-inline-start" => "left",
        "inset-inline-end" => "right",
        "border-block-start-width" => "border-top-width",
        "border-block-end-width" => "border-bottom-width",
        "border-inline-start-width" => "border-left-width",
        "border-inline-end-width" => "border-right-width",
        "border-block-start-color" => "border-top-color",
        "border-block-end-color" => "border-bottom-color",
        "border-inline-start-color" => "border-left-color",
        "border-inline-end-color" => "border-right-color",
        "border-block-start-style" => "border-top-style",
        "border-block-end-style" => "border-bottom-style",
        "border-inline-start-style" => "border-left-style",
        "border-inline-end-style" => "border-right-style",
        "grid-column-gap" => "column-gap",
        "grid-row-gap" => "row-gap",
        "border-radius" => "border-top-left-radius",
        _ => return None,
    })
}

fn is_global_keyword(v: &str) -> bool {
    matches!(
        v.to_ascii_lowercase().as_str(),
        "inherit" | "initial" | "unset" | "revert" | "revert-layer"
    )
}

/// Expand a declaration into longhands; unknown properties are dropped.
fn expand(property: &str, value: &str, out: &mut Longhands) {
    let name = canonical_name(property);
    let value = value.trim();
    if let Some(&long) = LONGHANDS.iter().find(|&&l| l == name) {
        out.set(long, value);
        return;
    }
    if let Some(long) = logical_longhand(&name) {
        out.set(long, value);
        return;
    }
    let global = is_global_keyword(value);
    let sides = |prefix: &str, suffix: &str| -> [&'static str; 4] {
        let find = |side: &str| -> &'static str {
            let full = format!("{prefix}{side}{suffix}");
            LONGHANDS
                .iter()
                .find(|&&l| l == full)
                .copied()
                .unwrap_or("")
        };
        [find("top"), find("right"), find("bottom"), find("left")]
    };
    let four = |props: [&'static str; 4], out: &mut Longhands| {
        if global {
            for p in props {
                out.set(p, value);
            }
            return;
        }
        let parts = split_components(value);
        let (t, r, b, l) = match parts.as_slice() {
            [a] => (*a, *a, *a, *a),
            [a, b] => (*a, *b, *a, *b),
            [a, b, c] => (*a, *b, *c, *b),
            [a, b, c, d, ..] => (*a, *b, *c, *d),
            [] => return,
        };
        out.set(props[0], t);
        out.set(props[1], r);
        out.set(props[2], b);
        out.set(props[3], l);
    };
    let two = |a_prop: &'static str, b_prop: &'static str, out: &mut Longhands| {
        if global {
            out.set(a_prop, value);
            out.set(b_prop, value);
            return;
        }
        let parts = split_components(value);
        match parts.as_slice() {
            [a] => {
                out.set(a_prop, *a);
                out.set(b_prop, *a);
            }
            [a, b, ..] => {
                out.set(a_prop, *a);
                out.set(b_prop, *b);
            }
            [] => {}
        }
    };
    match name.as_str() {
        "margin" => four(sides("margin-", ""), out),
        "padding" => four(sides("padding-", ""), out),
        "inset" => four(["top", "right", "bottom", "left"], out),
        "border-width" => four(sides("border-", "-width"), out),
        "border-style" => four(sides("border-", "-style"), out),
        "border-color" => four(sides("border-", "-color"), out),
        "margin-block" => two("margin-top", "margin-bottom", out),
        "margin-inline" => two("margin-left", "margin-right", out),
        "padding-block" => two("padding-top", "padding-bottom", out),
        "padding-inline" => two("padding-left", "padding-right", out),
        "inset-block" => two("top", "bottom", out),
        "inset-inline" => two("left", "right", out),
        "border" => {
            for side in ["top", "right", "bottom", "left"] {
                border_side(side, value, global, out);
            }
        }
        "border-top" | "border-block-start" => border_side("top", value, global, out),
        "border-right" | "border-inline-end" => border_side("right", value, global, out),
        "border-bottom" | "border-block-end" => border_side("bottom", value, global, out),
        "border-left" | "border-inline-start" => border_side("left", value, global, out),
        "border-block" => {
            border_side("top", value, global, out);
            border_side("bottom", value, global, out);
        }
        "border-inline" => {
            border_side("left", value, global, out);
            border_side("right", value, global, out);
        }
        "background" | "mask" => {
            let longhands = |p: &str| {
                [
                    "image",
                    "position-x",
                    "position-y",
                    "size",
                    "repeat",
                    "origin",
                    "clip",
                ]
                .map(|l| LONGHANDS.iter().copied().find(|n| *n == format!("{p}-{l}")))
            };
            let names = longhands(&name).map(|n| n.unwrap_or(""));
            let is_mask = name == "mask";
            if global {
                for n in names {
                    out.set(n, value);
                }
                if !is_mask {
                    out.set("background-color", value);
                }
                return;
            }
            let Some(s) = background::expand_shorthand(value, is_mask) else {
                return;
            };
            let values = [
                s.image,
                s.position_x,
                s.position_y,
                s.size,
                s.repeat,
                s.origin,
                s.clip,
            ];
            for (n, v) in names.into_iter().zip(values) {
                out.set(n, v);
            }
            if !is_mask {
                out.set("background-color", s.color);
            }
        }
        "background-position" | "mask-position" => {
            let x = if name == "mask-position" {
                "mask-position-x"
            } else {
                "background-position-x"
            };
            let y = if name == "mask-position" {
                "mask-position-y"
            } else {
                "background-position-y"
            };
            if global {
                out.set(x, value);
                out.set(y, value);
                return;
            }
            let mut xs = Vec::new();
            let mut ys = Vec::new();
            for layer in split_top_level(value, ',') {
                let Some([px, py]) = background::split_position(&split_components(layer)) else {
                    return;
                };
                xs.push(px);
                ys.push(py);
            }
            out.set(x, xs.join(", "));
            out.set(y, ys.join(", "));
        }
        "font" => expand_font(value, global, out),
        "list-style" => {
            if global {
                out.set("list-style-type", value);
                out.set("list-style-position", value);
                return;
            }
            let mut ty = None;
            let mut pos = "outside";
            let mut none_count = 0;
            for c in split_components(value) {
                let lc = c.to_ascii_lowercase();
                match lc.as_str() {
                    "inside" | "outside" => pos = if lc == "inside" { "inside" } else { "outside" },
                    "none" => none_count += 1,
                    _ if lc.starts_with("url(") || lc.contains("gradient(") => {}
                    _ => ty = Some(c.to_string()),
                }
            }
            let ty = match ty {
                Some(t) => t,
                None if none_count > 0 => "none".into(),
                None => "disc".into(),
            };
            out.set("list-style-type", ty);
            out.set("list-style-position", pos);
        }
        "flex" => {
            if global {
                for p in ["flex-grow", "flex-shrink", "flex-basis"] {
                    out.set(p, value);
                }
                return;
            }
            let (g, s, b) = match value.to_ascii_lowercase().as_str() {
                "none" => ("0".to_string(), "0".to_string(), "auto".to_string()),
                "auto" => ("1".into(), "1".into(), "auto".into()),
                _ => {
                    let parts = split_components(value);
                    let is_num = |s: &str| s.parse::<f32>().is_ok();
                    match parts.as_slice() {
                        [a] if is_num(a) => (a.to_string(), "1".into(), "0%".into()),
                        [a] => ("1".into(), "1".into(), a.to_string()),
                        [a, b] if is_num(b) => (a.to_string(), b.to_string(), "0%".into()),
                        [a, b] => (a.to_string(), "1".into(), b.to_string()),
                        [a, b, c, ..] => (a.to_string(), b.to_string(), c.to_string()),
                        [] => return,
                    }
                }
            };
            out.set("flex-grow", g);
            out.set("flex-shrink", s);
            out.set("flex-basis", b);
        }
        "flex-flow" => {
            if global {
                out.set("flex-direction", value);
                out.set("flex-wrap", value);
                return;
            }
            for c in split_components(value) {
                if c.contains("wrap") {
                    out.set("flex-wrap", c);
                } else {
                    out.set("flex-direction", c);
                }
            }
        }
        "gap" | "grid-gap" => two("row-gap", "column-gap", out),
        "place-items" => two("align-items", "justify-items", out),
        "place-content" => two("align-content", "justify-content", out),
        "place-self" => two("align-self", "justify-self", out),
        "overflow" => two("overflow-x", "overflow-y", out),
        "text-decoration" => {
            if global {
                out.set("text-decoration-line", value);
                out.set("text-decoration-color", value);
                return;
            }
            let mut lines = Vec::new();
            let mut color = "currentcolor".to_string();
            for c in split_components(value) {
                let lc = c.to_ascii_lowercase();
                match lc.as_str() {
                    "underline" | "overline" | "line-through" | "none" | "blink" => lines.push(lc),
                    "solid" | "double" | "dotted" | "dashed" | "wavy" | "auto" | "from-font" => {}
                    _ if parse_color(c, Color::BLACK).is_some() => color = c.to_string(),
                    _ => {}
                }
            }
            out.set(
                "text-decoration-line",
                if lines.is_empty() {
                    "none".to_string()
                } else {
                    lines.join(" ")
                },
            );
            out.set("text-decoration-color", color);
        }
        "white-space" => {
            if global {
                out.set("white-space-collapse", value);
                out.set("text-wrap-mode", value);
                return;
            }
            let (collapse, wrap) = match value.to_ascii_lowercase().as_str() {
                "normal" => ("collapse", "wrap"),
                "pre" => ("preserve", "nowrap"),
                "nowrap" => ("collapse", "nowrap"),
                "pre-wrap" => ("preserve", "wrap"),
                "pre-line" => ("preserve-breaks", "wrap"),
                "break-spaces" => ("break-spaces", "wrap"),
                _ => return,
            };
            out.set("white-space-collapse", collapse);
            out.set("text-wrap-mode", wrap);
        }
        "text-wrap" => {
            for c in split_components(value) {
                let lc = c.to_ascii_lowercase();
                if matches!(lc.as_str(), "wrap" | "nowrap") || global {
                    out.set("text-wrap-mode", c);
                }
            }
        }
        "word-wrap" => out.set("overflow-wrap", value),
        "grid-row" | "grid-column" => {
            let (start, end) = if name == "grid-row" {
                ("grid-row-start", "grid-row-end")
            } else {
                ("grid-column-start", "grid-column-end")
            };
            // An omitted end copies a lone <custom-ident> start (else `auto`).
            let parts = split_top_level(value, '/');
            let copy = grid_line_default;
            out.set(start, parts[0]);
            out.set(end, parts.get(1).copied().unwrap_or_else(|| copy(parts[0])));
        }
        "grid-area" => {
            let parts = split_top_level(value, '/');
            let copy = grid_line_default;
            let row_start = parts[0];
            let column_start = parts.get(1).copied().unwrap_or_else(|| copy(row_start));
            out.set("grid-row-start", row_start);
            out.set("grid-column-start", column_start);
            out.set(
                "grid-row-end",
                parts.get(2).copied().unwrap_or_else(|| copy(row_start)),
            );
            out.set(
                "grid-column-end",
                parts.get(3).copied().unwrap_or_else(|| copy(column_start)),
            );
        }
        "grid-template" => {
            if global || value.eq_ignore_ascii_case("none") {
                out.set("grid-template-rows", value);
                out.set("grid-template-columns", value);
                out.set("grid-template-areas", value);
                return;
            }
            let parts = split_top_level(value, '/');
            let rows = parts[0];
            if rows.contains('"') || rows.contains('\'') {
                // `"a a" 40px "b c" 1fr / auto 1fr`: strings are the areas, the rest rows.
                let mut areas = Vec::new();
                let mut sizes = Vec::new();
                for c in split_components(rows) {
                    if c.starts_with('"') || c.starts_with('\'') {
                        areas.push(c);
                        if sizes.len() < areas.len() - 1 {
                            sizes.push("auto");
                        }
                    } else {
                        sizes.push(c);
                    }
                }
                while sizes.len() < areas.len() {
                    sizes.push("auto");
                }
                out.set("grid-template-areas", areas.join(" "));
                out.set("grid-template-rows", sizes.join(" "));
            } else {
                out.set("grid-template-rows", rows);
                out.set("grid-template-areas", "none");
            }
            out.set(
                "grid-template-columns",
                parts.get(1).copied().unwrap_or("none"),
            );
        }
        _ => {}
    }
}

fn border_side(side: &str, value: &str, global: bool, out: &mut Longhands) {
    let find = |suffix: &str| -> &'static str {
        let full = format!("border-{side}-{suffix}");
        LONGHANDS
            .iter()
            .find(|&&l| l == full)
            .copied()
            .unwrap_or("")
    };
    let (w, s, c) = (find("width"), find("style"), find("color"));
    if global {
        out.set(w, value);
        out.set(s, value);
        out.set(c, value);
        return;
    }
    let mut width = "medium".to_string();
    let mut style = "none".to_string();
    let mut color = "currentcolor".to_string();
    for part in split_components(value) {
        let lp = part.to_ascii_lowercase();
        if parse_border_style(&lp).is_some() {
            style = lp;
        } else if matches!(lp.as_str(), "thin" | "medium" | "thick")
            || lp.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '-')
            || lp.starts_with("calc(")
        {
            width = part.to_string();
        } else {
            color = part.to_string();
        }
    }
    out.set(w, width);
    out.set(s, style);
    out.set(c, color);
}

fn expand_font(value: &str, global: bool, out: &mut Longhands) {
    let props = [
        "font-style",
        "font-weight",
        "font-size",
        "line-height",
        "font-family",
    ];
    if global {
        for p in props {
            out.set(p, value);
        }
        return;
    }
    let lower = value.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "caption" | "icon" | "menu" | "message-box" | "small-caption" | "status-bar"
    ) || lower.starts_with("-apple-system")
        || lower.starts_with("-webkit-")
    {
        out.set("font-style", "normal");
        out.set("font-weight", "normal");
        out.set("font-size", "13.333px");
        out.set("line-height", "normal");
        out.set("font-family", "system-ui");
        return;
    }
    let mut style = "normal";
    let mut weight = "normal";
    let base = value.as_ptr() as usize;
    for part in split_components(value) {
        let lp = part.to_ascii_lowercase();
        match lp.as_str() {
            "normal" | "small-caps" | "condensed" | "expanded" | "semi-condensed"
            | "semi-expanded" | "ultra-condensed" | "extra-condensed" | "ultra-expanded"
            | "extra-expanded" => {}
            "italic" | "oblique" => style = part,
            "bold" | "bolder" | "lighter" => weight = part,
            _ if part.parse::<f32>().is_ok() && !part.contains('/') => weight = part,
            _ => {
                // The size (with optional `/line-height`), then the family list.
                let offset = part.as_ptr() as usize - base;
                let rest = &value[offset..];
                let (size_lh, family) = split_size_and_family(rest);
                let (size, lh) = match size_lh.split_once('/') {
                    Some((s, l)) => (s.trim(), l.trim()),
                    None => (size_lh.trim(), "normal"),
                };
                out.set("font-style", style);
                out.set("font-weight", weight);
                out.set("font-size", size);
                out.set("line-height", if lh.is_empty() { "normal" } else { lh });
                out.set("font-family", family.trim());
                return;
            }
        }
    }
}

/// `12px/1.5 "Helvetica Neue", Arial` → (`12px/1.5`, `"Helvetica Neue", Arial`).
fn split_size_and_family(rest: &str) -> (String, &str) {
    let comps = split_components(rest);
    let base = rest.as_ptr() as usize;
    let mut size = String::new();
    let mut i = 0;
    while i < comps.len() {
        let c = comps[i];
        size.push_str(c);
        i += 1;
        let ends_with_slash = c.ends_with('/');
        let next_is_slash = comps.get(i).is_some_and(|n| n.starts_with('/'));
        if !(ends_with_slash || next_is_slash) {
            break;
        }
        if next_is_slash {
            let n = comps[i];
            size.push_str(n);
            i += 1;
            if n == "/" {
                if let Some(lh) = comps.get(i) {
                    size.push_str(lh);
                    i += 1;
                }
            }
            break;
        }
    }
    let family = comps
        .get(i)
        .map(|c| &rest[c.as_ptr() as usize - base..])
        .unwrap_or("");
    (size, family)
}

/// Copy one longhand's computed value from `src`.
fn copy_property(dst: &mut ComputedStyle, src: &ComputedStyle, prop: &str) {
    match prop {
        "display" => dst.display = src.display,
        "position" => dst.position = src.position,
        "float" => dst.float = src.float,
        "clear" => dst.clear = src.clear,
        "box-sizing" => dst.border_box_sizing = src.border_box_sizing,
        "top" => dst.inset.top = src.inset.top.clone(),
        "right" => dst.inset.right = src.inset.right.clone(),
        "bottom" => dst.inset.bottom = src.inset.bottom.clone(),
        "left" => dst.inset.left = src.inset.left.clone(),
        "width" => dst.width = src.width.clone(),
        "height" => dst.height = src.height.clone(),
        "min-width" => dst.min_width = src.min_width.clone(),
        "min-height" => dst.min_height = src.min_height.clone(),
        "max-width" => dst.max_width = src.max_width.clone(),
        "max-height" => dst.max_height = src.max_height.clone(),
        "aspect-ratio" => dst.aspect_ratio = src.aspect_ratio,
        "margin-top" => dst.margin.top = src.margin.top.clone(),
        "margin-right" => dst.margin.right = src.margin.right.clone(),
        "margin-bottom" => dst.margin.bottom = src.margin.bottom.clone(),
        "margin-left" => dst.margin.left = src.margin.left.clone(),
        "padding-top" => dst.padding.top = src.padding.top.clone(),
        "padding-right" => dst.padding.right = src.padding.right.clone(),
        "padding-bottom" => dst.padding.bottom = src.padding.bottom.clone(),
        "padding-left" => dst.padding.left = src.padding.left.clone(),
        "border-top-width" => dst.border[0].width = src.border[0].width,
        "border-right-width" => dst.border[1].width = src.border[1].width,
        "border-bottom-width" => dst.border[2].width = src.border[2].width,
        "border-left-width" => dst.border[3].width = src.border[3].width,
        "border-top-style" => dst.border[0].style = src.border[0].style,
        "border-right-style" => dst.border[1].style = src.border[1].style,
        "border-bottom-style" => dst.border[2].style = src.border[2].style,
        "border-left-style" => dst.border[3].style = src.border[3].style,
        "border-top-color" => dst.border[0].color = src.border[0].color,
        "border-right-color" => dst.border[1].color = src.border[1].color,
        "border-bottom-color" => dst.border[2].color = src.border[2].color,
        "border-left-color" => dst.border[3].color = src.border[3].color,
        "border-top-left-radius" => dst.border_radius = src.border_radius,
        "color" => dst.color = src.color,
        "background-color" => dst.background_color = src.background_color,
        "content" => dst.content = src.content.clone(),
        "opacity" => dst.opacity = src.opacity,
        "visibility" => dst.visibility_hidden = src.visibility_hidden,
        "overflow-x" => dst.overflow_x = src.overflow_x,
        "overflow-y" => dst.overflow_y = src.overflow_y,
        "z-index" => dst.z_index = src.z_index,
        "font-family" => dst.font_family = Arc::clone(&src.font_family),
        "font-size" => dst.font_size = src.font_size,
        "font-weight" => dst.font_weight = src.font_weight,
        "font-style" => dst.italic = src.italic,
        "line-height" => dst.line_height = src.line_height,
        "text-align" => dst.text_align = src.text_align,
        "direction" => dst.rtl = src.rtl,
        "text-decoration-line" => dst.text_decoration_line = src.text_decoration_line,
        "text-decoration-color" => dst.text_decoration_color = src.text_decoration_color,
        "text-transform" => dst.text_transform = src.text_transform,
        "text-indent" => dst.text_indent = src.text_indent.clone(),
        "white-space-collapse" => dst.space_collapse = src.space_collapse,
        "text-wrap-mode" => dst.nowrap = src.nowrap,
        "letter-spacing" => dst.letter_spacing = src.letter_spacing,
        "word-spacing" => dst.word_spacing = src.word_spacing,
        "vertical-align" => dst.vertical_align = src.vertical_align,
        "list-style-type" => dst.list_style_type = src.list_style_type.clone(),
        "list-style-position" => dst.list_style_inside = src.list_style_inside,
        "overflow-wrap" => dst.break_anywhere = src.break_anywhere,
        "word-break" => dst.word_break_all = src.word_break_all,
        "flex-direction" => dst.flex_direction = src.flex_direction,
        "flex-wrap" => dst.flex_wrap = src.flex_wrap,
        "flex-grow" => dst.flex_grow = src.flex_grow,
        "flex-shrink" => dst.flex_shrink = src.flex_shrink,
        "flex-basis" => dst.flex_basis = src.flex_basis.clone(),
        "order" => dst.order = src.order,
        "justify-content" => dst.justify_content = src.justify_content,
        "align-items" => dst.align_items = src.align_items,
        "align-self" => dst.align_self = src.align_self,
        "align-content" => dst.align_content = src.align_content,
        "justify-items" => dst.justify_items = src.justify_items,
        "justify-self" => dst.justify_self = src.justify_self,
        "row-gap" => dst.row_gap = src.row_gap.clone(),
        "column-gap" => dst.column_gap = src.column_gap.clone(),
        "grid-template-columns" => dst.grid_template_columns = src.grid_template_columns.clone(),
        "grid-template-rows" => dst.grid_template_rows = src.grid_template_rows.clone(),
        "grid-template-areas" => dst.grid_template_areas = src.grid_template_areas.clone(),
        "grid-auto-columns" => dst.grid_auto_columns = src.grid_auto_columns.clone(),
        "grid-auto-rows" => dst.grid_auto_rows = src.grid_auto_rows.clone(),
        "grid-auto-flow" => dst.grid_auto_flow = src.grid_auto_flow,
        "grid-row-start" => dst.grid_row_start = src.grid_row_start.clone(),
        "grid-row-end" => dst.grid_row_end = src.grid_row_end.clone(),
        "grid-column-start" => dst.grid_column_start = src.grid_column_start.clone(),
        "grid-column-end" => dst.grid_column_end = src.grid_column_end.clone(),
        "border-spacing" => dst.border_spacing = src.border_spacing,
        "border-collapse" => dst.border_collapse = src.border_collapse,
        "table-layout" => dst.table_layout_fixed = src.table_layout_fixed,
        "fill" => dst.fill = src.fill.clone(),
        "stroke" => dst.stroke = src.stroke.clone(),
        "stroke-width" => dst.stroke_width = src.stroke_width.clone(),
        "fill-opacity" => dst.fill_opacity = src.fill_opacity,
        "stroke-opacity" => dst.stroke_opacity = src.stroke_opacity,
        p => {
            let (dst, src, rest) = match p.strip_prefix("background-") {
                Some(rest) => (&mut dst.background, &src.background, rest),
                None => match p.strip_prefix("mask-") {
                    Some(rest) => (&mut dst.mask, &src.mask, rest),
                    None => return,
                },
            };
            match rest {
                "image" => dst.image = src.image.clone(),
                "position-x" => dst.position_x = src.position_x.clone(),
                "position-y" => dst.position_y = src.position_y.clone(),
                "size" => dst.size = src.size.clone(),
                "repeat" => dst.repeat = src.repeat.clone(),
                "origin" => dst.origin = src.origin.clone(),
                "clip" => dst.clip = src.clip.clone(),
                _ => {}
            }
        }
    }
}

fn parse_paint(raw: &str, v: &str, color: Color) -> Option<Paint> {
    Some(match v {
        "none" => Paint::None,
        "currentcolor" => Paint::CurrentColor,
        _ if v.starts_with("url(") => Paint::Url(Arc::from(raw)),
        _ => Paint::Color(parse_color(raw, color)?),
    })
}

fn initial_style() -> &'static ComputedStyle {
    static INITIAL: OnceLock<ComputedStyle> = OnceLock::new();
    INITIAL.get_or_init(ComputedStyle::initial)
}

fn parse_border_style(v: &str) -> Option<BorderStyle> {
    Some(match v {
        "none" => BorderStyle::None,
        "hidden" => BorderStyle::Hidden,
        "solid" => BorderStyle::Solid,
        "dashed" => BorderStyle::Dashed,
        "dotted" => BorderStyle::Dotted,
        "double" => BorderStyle::Double,
        "groove" => BorderStyle::Groove,
        "ridge" => BorderStyle::Ridge,
        "inset" => BorderStyle::Inset,
        "outset" => BorderStyle::Outset,
        _ => return None,
    })
}

fn parse_align(v: &str) -> Option<Align> {
    let v = v
        .trim()
        .trim_start_matches("safe ")
        .trim_start_matches("unsafe ")
        .trim_start_matches("legacy ")
        .trim();
    Some(match v {
        "normal" => Align::Normal,
        "stretch" => Align::Stretch,
        "start" | "flex-start" | "self-start" => Align::Start,
        "end" | "flex-end" | "self-end" => Align::End,
        "center" => Align::Center,
        "baseline" | "first baseline" | "last baseline" => Align::Baseline,
        "space-between" => Align::SpaceBetween,
        "space-around" => Align::SpaceAround,
        "space-evenly" => Align::SpaceEvenly,
        "left" => Align::Left,
        "right" => Align::Right,
        "legacy" => Align::Normal,
        _ => return None,
    })
}

fn parse_display(v: &str) -> Option<Display> {
    Some(match v {
        "none" => Display::None,
        "contents" => Display::Contents,
        "block" | "block flow" | "run-in" | "block math" => Display::Block,
        "inline" | "inline flow" | "ruby" | "ruby-text" | "ruby-base" | "math" => Display::Inline,
        "inline-block" | "inline flow-root" | "-webkit-inline-box" | "-moz-inline-box" => {
            Display::InlineBlock
        }
        "flow-root" | "block flow-root" | "-webkit-box" | "-moz-box" => Display::FlowRoot,
        "list-item" | "block list-item" | "list-item block" | "flow list-item" => Display::ListItem,
        "flex" | "block flex" | "-webkit-flex" | "-ms-flexbox" => Display::Flex,
        "inline-flex" | "inline flex" | "-webkit-inline-flex" | "-ms-inline-flexbox" => {
            Display::InlineFlex
        }
        "grid" | "block grid" | "-ms-grid" => Display::Grid,
        "inline-grid" | "inline grid" | "-ms-inline-grid" => Display::InlineGrid,
        "table" | "block table" => Display::Table,
        "inline-table" | "inline table" => Display::InlineTable,
        "table-row-group" => Display::TableRowGroup,
        "table-header-group" => Display::TableHeaderGroup,
        "table-footer-group" => Display::TableFooterGroup,
        "table-row" => Display::TableRow,
        "table-cell" => Display::TableCell,
        "table-column" => Display::TableColumn,
        "table-column-group" => Display::TableColumnGroup,
        "table-caption" => Display::TableCaption,
        _ => return None,
    })
}

fn parse_list_style_type(raw: &str) -> Option<ListStyleType> {
    let v = raw.to_ascii_lowercase();
    Some(match v.as_str() {
        "none" => ListStyleType::None,
        "disc" => ListStyleType::Disc,
        "circle" => ListStyleType::Circle,
        "square" => ListStyleType::Square,
        "decimal" | "cjk-decimal" | "arabic-indic" | "persian" | "devanagari" | "bengali"
        | "thai" | "hebrew" | "armenian" | "georgian" => ListStyleType::Decimal,
        "decimal-leading-zero" => ListStyleType::DecimalLeadingZero,
        "lower-alpha" | "lower-latin" | "lower-greek" => ListStyleType::LowerAlpha,
        "upper-alpha" | "upper-latin" => ListStyleType::UpperAlpha,
        "lower-roman" => ListStyleType::LowerRoman,
        "upper-roman" => ListStyleType::UpperRoman,
        "disclosure-closed" => ListStyleType::DisclosureClosed,
        "disclosure-open" => ListStyleType::DisclosureOpen,
        _ if (raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2)
            || (raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2) =>
        {
            ListStyleType::String(Arc::from(&raw[1..raw.len() - 1]))
        }
        _ => ListStyleType::Disc,
    })
}

/// Border widths: keywords and lengths (percentages are invalid).
fn parse_border_width(v: &str, cx: &UnitContext) -> Option<f32> {
    match v {
        "thin" => Some(1.0),
        "medium" => Some(3.0),
        "thick" => Some(5.0),
        _ => match parse_length(v, cx)? {
            Length::Px(px) => Some(px.max(0.0)),
            _ => None,
        },
    }
}

fn parse_font_size(v: &str, parent: f32, cx: &UnitContext) -> Option<f32> {
    let size = match v {
        "xx-small" => 9.0,
        "x-small" => 10.0,
        "small" => 13.0,
        "medium" => 16.0,
        "large" => 18.0,
        "x-large" => 24.0,
        "xx-large" => 32.0,
        "xxx-large" => 48.0,
        "smaller" => parent / 1.2,
        "larger" => parent * 1.2,
        "math" => parent,
        _ => parse_length(v, cx)?.resolve(parent)?,
    };
    (size.is_finite() && size >= 0.0).then_some(size)
}

/// The value an omitted grid line in a shorthand takes: a lone `<custom-ident>` is copied,
/// anything else becomes `auto`.
fn grid_line_default(part: &str) -> &str {
    if grid::is_grid_ident(part) {
        part
    } else {
        "auto"
    }
}

/// Apply one longhand's specified value to `s`. Invalid values leave the property as
/// it was (inherited or initial), per the cascade's "invalid declaration" handling.
fn apply(s: &mut ComputedStyle, prop: &str, raw: &str, parent: &ComputedStyle, cx: &UnitContext) {
    let raw = raw.trim();
    let lower = raw.to_ascii_lowercase();
    let v = lower.as_str();
    if is_global_keyword(v) {
        let inherited = INHERITED.contains(&prop);
        let inherit = v == "inherit" || (v != "initial" && inherited);
        if inherit {
            copy_property(s, parent, prop);
        } else {
            copy_property(s, initial_style(), prop);
            if prop.ends_with("-color") && prop.starts_with("border-") {
                let side = ["top", "right", "bottom", "left"]
                    .iter()
                    .position(|side| prop == format!("border-{side}-color"))
                    .unwrap_or(0);
                s.border[side].color = s.color;
            }
        }
        return;
    }
    let len = |v: &str| parse_length(v, cx);
    let side_index = |p: &str| -> usize {
        if p.contains("top") {
            0
        } else if p.contains("right") {
            1
        } else if p.contains("bottom") {
            2
        } else {
            3
        }
    };
    match prop {
        "display" => {
            if let Some(d) = parse_display(v) {
                s.display = d;
            }
        }
        "position" => {
            s.position = match v {
                "static" => Position::Static,
                "relative" => Position::Relative,
                "absolute" => Position::Absolute,
                "fixed" => Position::Fixed,
                "sticky" | "-webkit-sticky" => Position::Sticky,
                _ => s.position,
            }
        }
        "float" => {
            s.float = match v {
                "left" | "inline-start" => Float::Left,
                "right" | "inline-end" => Float::Right,
                "none" => Float::None,
                _ => s.float,
            }
        }
        "clear" => {
            s.clear = match v {
                "left" | "inline-start" => Clear::Left,
                "right" | "inline-end" => Clear::Right,
                "both" => Clear::Both,
                "none" => Clear::None,
                _ => s.clear,
            }
        }
        "box-sizing" => {
            if v == "border-box" || v == "content-box" {
                s.border_box_sizing = v == "border-box";
            }
        }
        "top" | "right" | "bottom" | "left" => {
            if let Some(l) = len(v) {
                match prop {
                    "top" => s.inset.top = l,
                    "right" => s.inset.right = l,
                    "bottom" => s.inset.bottom = l,
                    _ => s.inset.left = l,
                }
            }
        }
        "width" | "height" | "min-width" | "min-height" | "max-width" | "max-height" => {
            if let Some(l) = len(v) {
                let valid = match (&l, prop) {
                    (Length::None, "max-width" | "max-height") => true,
                    (Length::None, _) => false,
                    (Length::Auto, "max-width" | "max-height") => false,
                    (Length::Px(px), _) => *px >= 0.0,
                    (Length::Percent(p), _) => *p >= 0.0,
                    _ => true,
                };
                if valid {
                    match prop {
                        "width" => s.width = l,
                        "height" => s.height = l,
                        "min-width" => s.min_width = l,
                        "min-height" => s.min_height = l,
                        "max-width" => s.max_width = l,
                        _ => s.max_height = l,
                    }
                }
            }
        }
        "aspect-ratio" => {
            let ratio_part = v.trim_start_matches("auto").trim();
            s.aspect_ratio = if ratio_part.is_empty() {
                None
            } else if let Some((a, b)) = ratio_part.split_once('/') {
                match (a.trim().parse::<f32>(), b.trim().parse::<f32>()) {
                    (Ok(a), Ok(b)) if a > 0.0 && b > 0.0 => Some(a / b),
                    _ => s.aspect_ratio,
                }
            } else {
                ratio_part.parse::<f32>().ok().filter(|r| *r > 0.0)
            };
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            if let Some(l) = len(v).filter(|l| !matches!(l, Length::None)) {
                match side_index(prop) {
                    0 => s.margin.top = l,
                    1 => s.margin.right = l,
                    2 => s.margin.bottom = l,
                    _ => s.margin.left = l,
                }
            }
        }
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            if let Some(l) = len(v).filter(|l| match l {
                Length::Px(px) => *px >= 0.0,
                Length::Percent(p) => *p >= 0.0,
                Length::Calc(_) => true,
                _ => false,
            }) {
                match side_index(prop) {
                    0 => s.padding.top = l,
                    1 => s.padding.right = l,
                    2 => s.padding.bottom = l,
                    _ => s.padding.left = l,
                }
            }
        }
        "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" => {
            if let Some(w) = parse_border_width(v, cx) {
                s.border[side_index(prop)].width = w;
            }
        }
        "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => {
            if let Some(st) = parse_border_style(v) {
                s.border[side_index(prop)].style = st;
            }
        }
        "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color" => {
            if let Some(c) = parse_color(raw, s.color) {
                s.border[side_index(prop)].color = c;
            }
        }
        "border-top-left-radius" => {
            if let Some(first) = split_components(v).first() {
                if let Some(r) = len(first).and_then(|l| l.resolve(0.0)) {
                    s.border_radius = r.max(0.0);
                }
            }
        }
        "color" => {
            if let Some(c) = parse_color(raw, parent.color) {
                s.color = c;
            }
        }
        "content" => {
            if let Some(c) = generated::parse_content(raw) {
                s.content = c;
            }
        }
        "background-color" => {
            if let Some(c) = parse_color(raw, s.color) {
                s.background_color = c;
            }
        }
        p if p.starts_with("background-") || p.starts_with("mask-") => {
            let color = s.color;
            let (layers, rest) = match p.strip_prefix("background-") {
                Some(rest) => (&mut s.background, rest),
                None => (&mut s.mask, &p["mask-".len()..]),
            };
            match rest {
                "image" => {
                    if let Some(v) = background::parse_images(raw, cx, color) {
                        layers.image = v;
                    }
                }
                "position-x" | "position-y" => {
                    let horizontal = rest == "position-x";
                    if let Some(v) = background::parse_positions(raw, cx, horizontal) {
                        if horizontal {
                            layers.position_x = v;
                        } else {
                            layers.position_y = v;
                        }
                    }
                }
                "size" => {
                    if let Some(v) = background::parse_sizes(raw, cx) {
                        layers.size = v;
                    }
                }
                "repeat" => {
                    if let Some(v) = background::parse_repeats(raw) {
                        layers.repeat = v;
                    }
                }
                "origin" | "clip" => {
                    if let Some(v) = background::parse_areas(raw) {
                        if rest == "origin" {
                            layers.origin = v;
                        } else {
                            layers.clip = v;
                        }
                    }
                }
                _ => {}
            }
        }
        "opacity" => {
            let o = match v.strip_suffix('%') {
                Some(p) => p.trim().parse::<f32>().ok().map(|p| p / 100.0),
                None => parse_number(v, cx),
            };
            if let Some(o) = o {
                s.opacity = o.clamp(0.0, 1.0);
            }
        }
        "visibility" => match v {
            "hidden" | "collapse" => s.visibility_hidden = true,
            "visible" => s.visibility_hidden = false,
            _ => {}
        },
        "fill" | "stroke" => {
            if let Some(p) = parse_paint(raw, v, s.color) {
                if prop == "fill" {
                    s.fill = p;
                } else {
                    s.stroke = p;
                }
            }
        }
        "stroke-width" => {
            let unitless = parse_number(v, cx).map(Length::Px);
            if let Some(l) = unitless.or_else(|| len(v)).filter(|l| match l {
                Length::Px(px) => *px >= 0.0,
                Length::Percent(p) => *p >= 0.0,
                Length::Calc(_) => true,
                _ => false,
            }) {
                s.stroke_width = l;
            }
        }
        "fill-opacity" | "stroke-opacity" => {
            let o = match v.strip_suffix('%') {
                Some(p) => p.trim().parse::<f32>().ok().map(|p| p / 100.0),
                None => parse_number(v, cx),
            };
            if let Some(o) = o.map(|o| o.clamp(0.0, 1.0)) {
                if prop == "fill-opacity" {
                    s.fill_opacity = o;
                } else {
                    s.stroke_opacity = o;
                }
            }
        }
        "overflow-x" | "overflow-y" => {
            let o = match v {
                "visible" => Overflow::Visible,
                "hidden" => Overflow::Hidden,
                "clip" => Overflow::Clip,
                "scroll" => Overflow::Scroll,
                "auto" | "overlay" => Overflow::Auto,
                _ => return,
            };
            if prop == "overflow-x" {
                s.overflow_x = o;
            } else {
                s.overflow_y = o;
            }
        }
        "z-index" => {
            if v == "auto" {
                s.z_index = None;
            } else if let Ok(z) = v.parse::<i32>() {
                s.z_index = Some(z);
            }
        }
        "font-family" => {
            if !raw.is_empty() {
                s.font_family = Arc::from(raw);
            }
        }
        "font-size" => {
            if let Some(px) = parse_font_size(v, parent.font_size, cx) {
                s.font_size = px;
            }
        }
        "font-weight" => {
            let p = parent.font_weight;
            s.font_weight = match v {
                "normal" => 400,
                "bold" => 700,
                "bolder" => match p {
                    0..=349 => 400,
                    350..=549 => 700,
                    550..=899 => 900,
                    _ => p,
                },
                "lighter" => match p {
                    0..=549 => 100,
                    550..=749 => 400,
                    _ => 700,
                },
                _ => match parse_number(v, cx) {
                    Some(n) if (1.0..=1000.0).contains(&n) => n as u16,
                    _ => s.font_weight,
                },
            };
        }
        "font-style" => {
            if v == "normal" {
                s.italic = false;
            } else if v.starts_with("italic") || v.starts_with("oblique") {
                s.italic = true;
            }
        }
        "line-height" => {
            if v == "normal" {
                s.line_height = LineHeight::Normal;
            } else if let Some(n) = v.parse::<f32>().ok().filter(|n| *n >= 0.0) {
                s.line_height = LineHeight::Number(n);
            } else if let Some(l) = len(v) {
                if let Some(px) = l.resolve(s.font_size) {
                    s.line_height = LineHeight::Px(px.max(0.0));
                }
            } else if let Some(n) = parse_number(v, cx) {
                s.line_height = LineHeight::Number(n.max(0.0));
            }
        }
        "text-align" => {
            s.text_align = match v {
                "start" => TextAlign::Start,
                "end" => TextAlign::End,
                "left" => TextAlign::Left,
                "right" => TextAlign::Right,
                "center" => TextAlign::Center,
                "-webkit-left" | "-moz-left" => TextAlign::WebkitLeft,
                "-webkit-right" | "-moz-right" => TextAlign::WebkitRight,
                "-webkit-center" | "-moz-center" => TextAlign::WebkitCenter,
                "justify" | "justify-all" => TextAlign::Justify,
                "match-parent" => parent.text_align,
                _ => s.text_align,
            }
        }
        "direction" => match v {
            "rtl" => s.rtl = true,
            "ltr" => s.rtl = false,
            _ => {}
        },
        "text-decoration-line" => {
            let mut flags = 0;
            for w in v.split_whitespace() {
                flags |= match w {
                    "underline" => UNDERLINE,
                    "overline" => OVERLINE,
                    "line-through" => LINE_THROUGH,
                    "none" | "blink" => 0,
                    _ => return,
                };
            }
            s.text_decoration_line = flags;
        }
        "text-decoration-color" => {
            if v == "currentcolor" {
                s.text_decoration_color = None;
            } else if let Some(c) = parse_color(raw, s.color) {
                s.text_decoration_color = Some(c);
            }
        }
        "text-transform" => {
            s.text_transform = match v {
                "uppercase" => TextTransform::Uppercase,
                "lowercase" => TextTransform::Lowercase,
                "capitalize" => TextTransform::Capitalize,
                "none" | "full-width" | "full-size-kana" => TextTransform::None,
                _ => s.text_transform,
            }
        }
        "text-indent" => {
            if let Some(l) = split_components(v).first().and_then(|t| len(t)) {
                s.text_indent = l;
            }
        }
        "white-space-collapse" => {
            s.space_collapse = match v {
                "collapse" | "discard" => SpaceCollapse::Collapse,
                "preserve" | "preserve-spaces" => SpaceCollapse::Preserve,
                "preserve-breaks" => SpaceCollapse::PreserveBreaks,
                "break-spaces" => SpaceCollapse::BreakSpaces,
                _ => s.space_collapse,
            }
        }
        "text-wrap-mode" => match v {
            "nowrap" => s.nowrap = true,
            "wrap" => s.nowrap = false,
            _ => {}
        },
        "letter-spacing" | "word-spacing" => {
            let px = if v == "normal" {
                Some(0.0)
            } else {
                len(v).and_then(|l| l.resolve(s.font_size))
            };
            if let Some(px) = px {
                if prop == "letter-spacing" {
                    s.letter_spacing = px;
                } else {
                    s.word_spacing = px;
                }
            }
        }
        "vertical-align" => {
            s.vertical_align = match v {
                "baseline" => VerticalAlign::Baseline,
                "sub" => VerticalAlign::Sub,
                "super" => VerticalAlign::Super,
                "top" => VerticalAlign::Top,
                "bottom" => VerticalAlign::Bottom,
                "middle" => VerticalAlign::Middle,
                "text-top" => VerticalAlign::TextTop,
                "text-bottom" => VerticalAlign::TextBottom,
                _ => match len(v).and_then(|l| l.resolve(s.line_height_px(1.2))) {
                    Some(px) => VerticalAlign::Length(px),
                    None => s.vertical_align,
                },
            }
        }
        "list-style-type" => {
            if let Some(t) = parse_list_style_type(raw) {
                s.list_style_type = t;
            }
        }
        "list-style-position" => match v {
            "inside" => s.list_style_inside = true,
            "outside" => s.list_style_inside = false,
            _ => {}
        },
        "overflow-wrap" => match v {
            "break-word" | "anywhere" => s.break_anywhere = true,
            "normal" => s.break_anywhere = false,
            _ => {}
        },
        "word-break" => match v {
            "break-all" => s.word_break_all = true,
            "break-word" => s.break_anywhere = true,
            "normal" | "keep-all" | "auto-phrase" => s.word_break_all = false,
            _ => {}
        },
        "flex-direction" => {
            s.flex_direction = match v {
                "row" => FlexDirection::Row,
                "row-reverse" => FlexDirection::RowReverse,
                "column" => FlexDirection::Column,
                "column-reverse" => FlexDirection::ColumnReverse,
                _ => s.flex_direction,
            }
        }
        "flex-wrap" => {
            s.flex_wrap = match v {
                "nowrap" => FlexWrap::NoWrap,
                "wrap" => FlexWrap::Wrap,
                "wrap-reverse" => FlexWrap::WrapReverse,
                _ => s.flex_wrap,
            }
        }
        "flex-grow" | "flex-shrink" => {
            if let Some(n) = parse_number(v, cx).filter(|n| *n >= 0.0) {
                if prop == "flex-grow" {
                    s.flex_grow = n;
                } else {
                    s.flex_shrink = n;
                }
            }
        }
        "flex-basis" => {
            if v == "content" {
                s.flex_basis = Length::MaxContent;
            } else if let Some(l) = len(v).filter(|l| !matches!(l, Length::None)) {
                s.flex_basis = l;
            }
        }
        "order" => {
            if let Ok(n) = v.parse::<i32>() {
                s.order = n;
            }
        }
        "justify-content" | "align-items" | "align-content" | "justify-items" => {
            if let Some(a) = parse_align(v) {
                match prop {
                    "justify-content" => s.justify_content = a,
                    "align-items" => s.align_items = a,
                    "align-content" => s.align_content = a,
                    _ => s.justify_items = a,
                }
            }
        }
        "align-self" | "justify-self" => {
            let a = if v == "auto" {
                None
            } else if let Some(a) = parse_align(v) {
                Some(a)
            } else {
                return;
            };
            if prop == "align-self" {
                s.align_self = a;
            } else {
                s.justify_self = a;
            }
        }
        "row-gap" | "column-gap" => {
            let l = if v == "normal" {
                Some(Length::Auto)
            } else {
                len(v)
            };
            if let Some(l) = l {
                if prop == "row-gap" {
                    s.row_gap = l;
                } else {
                    s.column_gap = l;
                }
            }
        }
        "grid-template-columns" => {
            s.grid_template_columns = grid::parse_track_list(raw, cx).map(Arc::new)
        }
        "grid-template-rows" => {
            s.grid_template_rows = grid::parse_track_list(raw, cx).map(Arc::new)
        }
        "grid-template-areas" => s.grid_template_areas = grid::parse_areas(raw).map(Arc::new),
        "grid-auto-columns" => s.grid_auto_columns = grid::parse_track_sizes(raw, cx),
        "grid-auto-rows" => s.grid_auto_rows = grid::parse_track_sizes(raw, cx),
        "grid-auto-flow" => {
            if let Some(flow) = grid::parse_auto_flow(raw) {
                s.grid_auto_flow = flow;
            }
        }
        "grid-row-start" => s.grid_row_start = grid::parse_grid_line(raw),
        "grid-row-end" => s.grid_row_end = grid::parse_grid_line(raw),
        "grid-column-start" => s.grid_column_start = grid::parse_grid_line(raw),
        "grid-column-end" => s.grid_column_end = grid::parse_grid_line(raw),
        "border-spacing" => {
            if let Some(px) = split_components(v)
                .first()
                .and_then(|t| len(t))
                .and_then(|l| l.resolve(0.0))
            {
                s.border_spacing = px.max(0.0);
            }
        }
        "border-collapse" => match v {
            "collapse" => s.border_collapse = true,
            "separate" => s.border_collapse = false,
            _ => {}
        },
        "table-layout" => match v {
            "fixed" => s.table_layout_fixed = true,
            "auto" => s.table_layout_fixed = false,
            _ => {}
        },
        _ => {}
    }
}

/// HTML presentational attributes mapped to CSS (HTML Standard §15.3).
fn presentational_hints(doc: &Document, id: NodeId, tag: &str) -> Vec<Declaration> {
    let mut out = Vec::new();
    let mut push = |p: &str, v: String| {
        out.push(Declaration {
            property: p.to_string(),
            value: v,
            important: false,
        })
    };
    let dimension = |v: &str| -> Option<String> {
        let v = v.trim();
        let digits = v
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(v.len());
        if digits == 0 {
            return None;
        }
        let n: f32 = v[..digits].parse().ok()?;
        Some(if v[digits..].starts_with('%') {
            format!("{n}%")
        } else {
            format!("{n}px")
        })
    };
    if doc.namespace(id) == Some(axiom_dom::Namespace::Svg) {
        // SVG presentation attributes (SVG 2 §6.6) that style and the rasterizer use.
        for attr in [
            "fill",
            "stroke",
            "stroke-width",
            "fill-opacity",
            "stroke-opacity",
            "color",
            "display",
            "visibility",
        ] {
            if let Some(v) = doc.attr(id, attr) {
                push(attr, v.to_string());
            }
        }
    }
    let has_dims = matches!(
        tag,
        "img"
            | "video"
            | "canvas"
            | "iframe"
            | "embed"
            | "object"
            | "table"
            | "td"
            | "th"
            | "col"
            | "hr"
            | "svg"
            | "input"
    );
    if tag == "svg" && doc.namespace(id) == Some(axiom_dom::Namespace::Svg) {
        // SVG `width` / `height` are presentation attributes taking any CSS length
        // (`1.33em`, `100%`); a bare number is in px.
        for name in ["width", "height"] {
            if let Some(v) = doc.attr(id, name).map(str::trim).filter(|v| !v.is_empty()) {
                let css = match v.parse::<f32>() {
                    Ok(n) => format!("{n}px"),
                    Err(_) => v.to_string(),
                };
                push(name, css);
            }
        }
    } else if has_dims {
        let image_input = tag != "input"
            || doc
                .attr(id, "type")
                .is_some_and(|t| t.eq_ignore_ascii_case("image"));
        if image_input {
            if let Some(w) = doc.attr(id, "width").and_then(dimension) {
                push("width", w);
            }
            if tag != "hr" && tag != "col" {
                if let Some(h) = doc.attr(id, "height").and_then(dimension) {
                    push("height", h);
                }
            }
        }
    }
    if matches!(
        tag,
        "body" | "table" | "tr" | "td" | "th" | "thead" | "tbody" | "tfoot"
    ) {
        if let Some(c) = doc.attr(id, "bgcolor") {
            push("background-color", legacy_color(c));
        }
    }
    if tag == "body" {
        if let Some(c) = doc.attr(id, "text") {
            push("color", legacy_color(c));
        }
        for (attr, prop) in [
            ("marginwidth", "margin-left"),
            ("marginwidth", "margin-right"),
            ("leftmargin", "margin-left"),
            ("rightmargin", "margin-right"),
            ("marginheight", "margin-top"),
            ("marginheight", "margin-bottom"),
            ("topmargin", "margin-top"),
            ("bottommargin", "margin-bottom"),
        ] {
            if let Some(v) = doc.attr(id, attr).and_then(dimension) {
                push(prop, v);
            }
        }
    }
    if tag == "font" {
        if let Some(c) = doc.attr(id, "color") {
            push("color", legacy_color(c));
        }
        if let Some(face) = doc.attr(id, "face") {
            push("font-family", face.to_string());
        }
        if let Some(size) = doc.attr(id, "size") {
            let s = size.trim();
            let n = if let Some(r) = s.strip_prefix('+') {
                r.parse::<i32>().map(|v| 3 + v).ok()
            } else if let Some(r) = s.strip_prefix('-') {
                r.parse::<i32>().map(|v| 3 - v).ok()
            } else {
                s.parse::<i32>().ok()
            };
            if let Some(n) = n {
                let kw = match n.clamp(1, 7) {
                    1 => "x-small",
                    2 => "small",
                    3 => "medium",
                    4 => "large",
                    5 => "x-large",
                    6 => "xx-large",
                    _ => "xxx-large",
                };
                push("font-size", kw.to_string());
            }
        }
    }
    if let Some(align) = doc.attr(id, "align") {
        let a = align.trim().to_ascii_lowercase();
        match tag {
            "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "td" | "th" | "tr"
            | "thead" | "tbody" | "tfoot" | "caption" | "legend" => {
                let v = match a.as_str() {
                    "center" | "middle" => "-webkit-center",
                    "left" => "-webkit-left",
                    "right" => "-webkit-right",
                    "justify" => "justify",
                    _ => "",
                };
                if !v.is_empty() {
                    push("text-align", v.to_string());
                }
            }
            "table" => match a.as_str() {
                "center" => {
                    push("margin-left", "auto".into());
                    push("margin-right", "auto".into());
                }
                "left" | "right" => push("float", a.clone()),
                _ => {}
            },
            "img" | "iframe" | "object" | "embed" | "input" => match a.as_str() {
                "left" | "right" => push("float", a.clone()),
                "middle" | "center" => push("vertical-align", "middle".into()),
                "top" => push("vertical-align", "top".into()),
                "bottom" | "baseline" => push("vertical-align", "baseline".into()),
                _ => {}
            },
            "hr" => match a.as_str() {
                "left" => {
                    push("margin-left", "0".into());
                    push("margin-right", "auto".into());
                }
                "right" => {
                    push("margin-left", "auto".into());
                    push("margin-right", "0".into());
                }
                _ => {}
            },
            _ => {}
        }
    }
    if matches!(tag, "td" | "th" | "tr" | "thead" | "tbody" | "tfoot") {
        if let Some(v) = doc.attr(id, "valign") {
            push("vertical-align", v.trim().to_ascii_lowercase());
        }
        if tag != "tr" && doc.has_attr(id, "nowrap") {
            push("white-space", "nowrap".into());
        }
    }
    if tag == "table" {
        if let Some(b) = doc.attr(id, "border") {
            let w = b.trim().parse::<f32>().unwrap_or(1.0);
            if w > 0.0 {
                push("border", format!("{w}px outset gray"));
            }
        }
        if let Some(sp) = doc.attr(id, "cellspacing").and_then(dimension) {
            push("border-spacing", sp);
        }
    }
    if matches!(tag, "td" | "th") {
        if let Some(table) = ancestor_table(doc, id) {
            if let Some(p) = doc.attr(table, "cellpadding").and_then(dimension) {
                push("padding", p);
            }
            if doc
                .attr(table, "border")
                .and_then(|b| b.trim().parse::<f32>().ok())
                .is_some_and(|w| w > 0.0)
            {
                push("border", "1px inset gray".into());
            }
        }
    }
    if tag == "img" {
        if let Some(b) = doc.attr(id, "border").and_then(dimension) {
            push("border", format!("{b} solid"));
        }
        for (attr, props) in [
            ("hspace", ["margin-left", "margin-right"]),
            ("vspace", ["margin-top", "margin-bottom"]),
        ] {
            if let Some(v) = doc.attr(id, attr).and_then(dimension) {
                for p in props {
                    push(p, v.clone());
                }
            }
        }
    }
    if tag == "hr" {
        if doc.has_attr(id, "noshade") {
            push("border-style", "solid".into());
        }
        if let Some(c) = doc.attr(id, "color") {
            push("border-color", legacy_color(c));
            push("background-color", legacy_color(c));
        }
        if let Some(s) = doc.attr(id, "size").and_then(dimension) {
            push("height", s);
        }
    }
    if let Some(dir) = doc.attr(id, "dir") {
        match dir.trim().to_ascii_lowercase().as_str() {
            "rtl" => push("direction", "rtl".into()),
            "ltr" => push("direction", "ltr".into()),
            _ => {}
        }
    }
    if tag == "bdo" {
        if let Some(dir) = doc.attr(id, "dir") {
            push("direction", dir.trim().to_ascii_lowercase());
        }
    }
    if matches!(tag, "ol" | "ul" | "li") {
        if let Some(t) = doc.attr(id, "type") {
            let v = match t.trim() {
                "1" => "decimal",
                "a" => "lower-alpha",
                "A" => "upper-alpha",
                "i" => "lower-roman",
                "I" => "upper-roman",
                other => match other.to_ascii_lowercase().as_str() {
                    "disc" => "disc",
                    "circle" => "circle",
                    "square" => "square",
                    "none" => "none",
                    _ => "",
                },
            };
            if !v.is_empty() {
                push("list-style-type", v.to_string());
            }
        }
    }
    out
}

fn ancestor_table(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut cur = doc.get(id).parent;
    while let Some(p) = cur {
        if doc
            .tag_name(p)
            .is_some_and(|t| t.eq_ignore_ascii_case("table"))
        {
            return Some(p);
        }
        cur = doc.get(p).parent;
    }
    None
}

/// Legacy color attribute values (`bgcolor="ccc"` is `#cccccc` in the HTML parsing
/// rules; named colors pass through).
fn legacy_color(v: &str) -> String {
    let v = v.trim();
    if parse_color(v, Color::BLACK).is_some() {
        return v.to_string();
    }
    let hex: String = v
        .trim_start_matches('#')
        .chars()
        .map(|c| if c.is_ascii_hexdigit() { c } else { '0' })
        .collect();
    match hex.len() {
        3 | 6 => format!("#{hex}"),
        n if n > 0 => {
            let mut h = hex;
            while !h.len().is_multiple_of(3) {
                h.push('0');
            }
            let part = h.len() / 3;
            let take = |i: usize| {
                let seg = &h[i * part..(i + 1) * part];
                seg[..2.min(seg.len())].to_string()
            };
            format!("#{}{}{}", take(0), take(1), take(2))
        }
        _ => "transparent".to_string(),
    }
}

#[cfg(test)]
mod tests;
