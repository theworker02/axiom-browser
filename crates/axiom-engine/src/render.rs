//! Style and layout state of one document, shared between the frame loop and the script
//! host. Script queries (`getComputedStyle`, `getBoundingClientRect`, `offsetWidth`, …)
//! flush style and layout synchronously through [`flush_style_and_layout`], the same
//! steps the frame loop runs before paint, so they never observe stale geometry.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use axiom_dom::{Document, NodeId, NodeKind, StyleSource};
use axiom_js::ElementGeometry;
use axiom_layout::{layout_document, Fragment, LayoutInputs, LayoutTree, Rect};
use axiom_paint::CssImages;
use axiom_style::serialize::{self, px};
use axiom_style::{ComputedStyle, Display, Length, Position, StyleEngine, StyleMap};
use axiom_trace::{TraceKind, TraceTimeline};

use crate::csp::element_nonce;
use crate::inline_svg::InlineSvgCache;
use crate::page::{DecodedImage, DocumentShared};

/// Inputs and outputs of style and layout for one document.
#[derive(Default)]
pub struct RenderState {
    pub styles: StyleMap,
    /// The engine that produced `styles`, kept to style elements it skipped
    /// (`display: none` subtrees) when script asks for them.
    pub style_engine: Option<StyleEngine>,
    pub layout: Option<LayoutTree>,
    pub images: HashMap<NodeId, Arc<DecodedImage>>,
    /// Inline `<svg>` bitmaps, rebuilt with every style pass.
    pub inline_svgs: HashMap<NodeId, Arc<DecodedImage>>,
    pub inline_svg_cache: InlineSvgCache,
    /// Loaded `url()` background and mask images, keyed by the URL text of the style.
    pub css_images: CssImages,
    /// `url()` images the last style pass found on rendered elements, for the loader.
    pub css_image_urls: Vec<String>,
    /// Registered `@font-face` families (lowercase) and their document-scoped font keys.
    pub web_fonts: HashMap<String, String>,
    /// Loaded `<link rel=stylesheet>` bodies (with their `@import`s) keyed by the link.
    pub external_css: HashMap<NodeId, String>,
    /// Loaded `@import`s of inline `<style>` elements, keyed by the style element.
    pub inline_imports: HashMap<NodeId, String>,
    /// Viewport size in CSS px that style and layout use.
    pub viewport: (f32, f32),
    /// Style or layout passes forced by script queries (diagnostics).
    pub forced_flushes: u64,
    /// Layout passes so far (observers re-check geometry when it changes).
    pub layout_generation: u64,
}

impl std::fmt::Debug for RenderState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderState")
            .field("styled_nodes", &self.styles.len())
            .field("has_layout", &self.layout.is_some())
            .field("images", &self.images.len())
            .field("viewport", &self.viewport)
            .field("forced_flushes", &self.forced_flushes)
            .finish()
    }
}

/// What a flush did, with timings for the HUD and trace.
#[derive(Debug, Clone, Copy, Default)]
pub struct FlushOutcome {
    pub styled: bool,
    pub laid_out: bool,
    pub style_ms: f64,
    pub layout_ms: f64,
}

/// The author style sheets in document order: allowed inline `<style>` text (after its
/// loaded `@import`s) and loaded, enabled external sheets.
pub fn author_css(doc: &Document, shared: &DocumentShared) -> String {
    sources_css(doc, shared, doc.collect_style_sources())
}

fn sources_css(doc: &Document, shared: &DocumentShared, sources: Vec<StyleSource>) -> String {
    let render = shared.render.borrow();
    let mut css = String::new();
    for source in sources {
        match source {
            StyleSource::Inline { node, text } => {
                let nonce = element_nonce(doc, node);
                if !shared
                    .csp
                    .inline_style_allowed(node, &text, nonce.as_deref())
                {
                    continue;
                }
                if let Some(imported) = render.inline_imports.get(&node) {
                    css.push_str(imported);
                    css.push('\n');
                }
                css.push_str(&text);
            }
            StyleSource::External { node, .. } => {
                if doc.has_attr(node, "disabled") {
                    continue;
                }
                if let Some(text) = render.external_css.get(&node) {
                    css.push_str(text);
                }
            }
        }
        css.push('\n');
    }
    css
}

/// Recompute style and layout if the document's dirty flags (or missing results) ask for
/// it. Consumed `style` / `layout` flags turn into `paint` / `composite`, so the next
/// frame still repaints.
pub fn flush_style_and_layout(
    document: &RefCell<Document>,
    shared: &Rc<DocumentShared>,
    trace: Option<&TraceTimeline>,
) -> FlushOutcome {
    let mut outcome = FlushOutcome::default();
    let dirty = document.borrow().dirty;
    let needs_style = dirty.style || shared.render.borrow().styles.is_empty();
    if needs_style {
        let t0 = Instant::now();
        let _span = trace.map(|t| t.span(TraceKind::Style, "style"));
        let (width, height, fonts) = {
            let r = shared.render.borrow();
            (r.viewport.0, r.viewport.1, r.web_fonts.clone())
        };
        let mut engine = StyleEngine::with_viewport(width, height);
        engine.set_font_aliases(fonts);
        if shared.csp.is_active() {
            // Weak: the engine is stored in `shared.render`.
            let csp_shared = Rc::downgrade(shared);
            engine.set_style_attribute_filter(move |node, value| {
                csp_shared
                    .upgrade()
                    .is_some_and(|s| s.csp.style_attribute_allowed(node, value))
            });
        }
        let styles = {
            let doc = document.borrow();
            {
                let _css = trace.map(|t| t.span(TraceKind::CssParse, "css parse"));
                engine.add_author_css(&author_css(&doc, shared));
                for (_, root) in doc.connected_shadow_roots() {
                    let sources = doc.collect_style_sources_in(root);
                    if !sources.is_empty() {
                        engine.add_shadow_css(root, &sources_css(&doc, shared, sources));
                    }
                }
            }
            engine.compute_document(&doc)
        };
        {
            let mut render = shared.render.borrow_mut();
            let render = &mut *render;
            render.inline_svgs = render
                .inline_svg_cache
                .rasterize(&document.borrow(), &styles);
            render.css_image_urls = css_image_urls(&styles);
            render.styles = styles;
            render.style_engine = Some(engine);
        }
        let mut doc = document.borrow_mut();
        doc.dirty.style = false;
        doc.dirty.layout = true;
        doc.dirty.paint = true;
        outcome.styled = true;
        outcome.style_ms = t0.elapsed().as_secs_f64() * 1000.0;
    }

    let dirty = document.borrow().dirty;
    let needs_layout = dirty.layout || shared.render.borrow().layout.is_none();
    if needs_layout {
        let t0 = Instant::now();
        let _span = trace.map(|t| t.span(TraceKind::Layout, "layout"));
        let mut layout = {
            let render = shared.render.borrow();
            let forms = shared.forms.borrow();
            let inputs = LayoutInputs {
                images: &render.images,
                inline_svgs: &render.inline_svgs,
                form_values: &forms.values,
                form_checked: &forms.checked,
            };
            layout_document(
                &document.borrow(),
                &render.styles,
                &inputs,
                render.viewport.0,
                render.viewport.1,
            )
        };
        {
            let mut render = shared.render.borrow_mut();
            let render = &mut *render;
            render.inline_svg_cache.fit_to_layout(
                &document.borrow(),
                &render.styles,
                &mut layout,
                &mut render.inline_svgs,
            );
            render.layout = Some(layout);
            render.layout_generation += 1;
        }
        let mut doc = document.borrow_mut();
        doc.dirty.layout = false;
        doc.dirty.paint = true;
        doc.dirty.composite = true;
        outcome.laid_out = true;
        outcome.layout_ms = t0.elapsed().as_secs_f64() * 1000.0;
    }
    outcome
}

/// `url()` background and mask images of rendered elements, each once.
pub fn css_image_urls(styles: &StyleMap) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut urls = Vec::new();
    let generated = styles.generated_boxes().map(|(_, _, g)| &g.style);
    for style in styles.iter().map(|(_, s)| s).chain(generated) {
        if style.display == Display::None {
            continue;
        }
        for url in style.background.urls().chain(style.mask.urls()) {
            if seen.insert(url.clone()) {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

/// Elements whose `width` / `height` apply even when they are inline.
const REPLACED: &[&str] = &[
    "img", "input", "textarea", "select", "button", "video", "audio", "canvas", "iframe", "svg",
    "object", "embed",
];

fn is_inline_non_replaced(doc: &Document, node: NodeId, style: &ComputedStyle) -> bool {
    style.display == Display::Inline
        && !doc
            .tag_name(node)
            .is_some_and(|t| REPLACED.iter().any(|r| t.eq_ignore_ascii_case(r)))
}

/// The style of element `node`, computing it when style skipped it (`display: none`
/// subtrees); `None` for non-elements and disconnected elements.
fn element_style<'a>(
    doc: &Document,
    render: &'a RenderState,
    node: NodeId,
) -> Option<Cow<'a, ComputedStyle>> {
    if !matches!(doc.get(node).kind, NodeKind::Element { .. }) {
        return None;
    }
    if let Some(style) = render.styles.get(node) {
        return Some(Cow::Borrowed(&**style));
    }
    let engine = render.style_engine.as_ref()?;
    engine
        .compute_unstyled(doc, &render.styles, node)
        .map(Cow::Owned)
}

/// Content-box width of `node`'s nearest rendered ancestor (its containing block for
/// in-flow boxes), or the viewport width.
fn containing_width(doc: &Document, layout: &LayoutTree, node: NodeId) -> f32 {
    let mut cur = doc.get(node).parent;
    while let Some(p) = cur {
        if let Some(f) = layout.node_fragments(p).first() {
            let pad = f.padding_rect();
            let s = &f.style;
            let inner = pad.width
                - s.padding.left.resolve_or(pad.width, 0.0)
                - s.padding.right.resolve_or(pad.width, 0.0);
            return inner.max(0.0);
        }
        cur = doc.get(p).parent;
    }
    layout.viewport_width
}

/// The resolved value of `property` for element `node` (CSSOM `getComputedStyle`): the
/// used value for `width` / `height` and percentage margins and paddings of rendered
/// boxes, the computed value otherwise.
pub fn resolved_value(
    doc: &Document,
    render: &RenderState,
    node: NodeId,
    property: &str,
) -> Option<String> {
    let style = element_style(doc, render, node)?;
    let rendered = render
        .layout
        .as_ref()
        .and_then(|l| l.node_fragments(node).first().map(|f| (l, f.rect)));
    if let Some((layout, rect)) = rendered {
        let cb = containing_width(doc, layout, node);
        let edge = |l: &Length| px(l.resolve_or(cb, 0.0));
        let used = |l: &Length| matches!(l, Length::Percent(_) | Length::Calc(_));
        match property {
            "width" | "height" if !is_inline_non_replaced(doc, node, &style) => {
                let [bt, br, bb, bl] = style.border_widths();
                let p = &style.padding;
                let size = if property == "width" {
                    let inner = p.left.resolve_or(cb, 0.0) + p.right.resolve_or(cb, 0.0) + bl + br;
                    rect.width - if style.border_box_sizing { 0.0 } else { inner }
                } else {
                    let inner = p.top.resolve_or(cb, 0.0) + p.bottom.resolve_or(cb, 0.0) + bt + bb;
                    rect.height - if style.border_box_sizing { 0.0 } else { inner }
                };
                return Some(px(size.max(0.0)));
            }
            "margin-top" if used(&style.margin.top) => return Some(edge(&style.margin.top)),
            "margin-right" if used(&style.margin.right) => return Some(edge(&style.margin.right)),
            "margin-bottom" if used(&style.margin.bottom) => {
                return Some(edge(&style.margin.bottom))
            }
            "margin-left" if used(&style.margin.left) => return Some(edge(&style.margin.left)),
            "padding-top" if used(&style.padding.top) => return Some(edge(&style.padding.top)),
            "padding-right" if used(&style.padding.right) => {
                return Some(edge(&style.padding.right))
            }
            "padding-bottom" if used(&style.padding.bottom) => {
                return Some(edge(&style.padding.bottom))
            }
            "padding-left" if used(&style.padding.left) => return Some(edge(&style.padding.left)),
            _ => {}
        }
    }
    serialize::computed_value(&style, property)
}

/// Right and bottom edges of `f`'s descendants (its scrollable overflow).
fn overflow_extent(f: &Fragment) -> (f32, f32) {
    let mut right = f32::MIN;
    let mut bottom = f32::MIN;
    for c in &f.children {
        right = right.max(c.rect.right());
        bottom = bottom.max(c.rect.bottom());
        let (r, b) = overflow_extent(c);
        right = right.max(r);
        bottom = bottom.max(b);
    }
    (right, bottom)
}

/// The `offsetParent` of `node` (CSSOM View §7): the nearest positioned ancestor, else the
/// nearest `td` / `th` / `table` (for static elements), else the body.
fn offset_parent(
    doc: &Document,
    render: &RenderState,
    node: NodeId,
    style: &ComputedStyle,
) -> Option<NodeId> {
    let body = doc.body();
    if Some(node) == doc.document_element() || Some(node) == body {
        return None;
    }
    if style.position == Position::Fixed {
        return None;
    }
    let mut cur = doc.get(node).parent;
    while let Some(p) = cur {
        if Some(p) == body {
            return body;
        }
        if let Some(s) = render.styles.get(p) {
            if s.is_positioned() {
                return Some(p);
            }
            let cell = doc.tag_name(p).is_some_and(|t| {
                ["td", "th", "table"]
                    .iter()
                    .any(|n| t.eq_ignore_ascii_case(n))
            });
            if cell && style.position == Position::Static {
                return Some(p);
            }
        }
        cur = doc.get(p).parent;
    }
    None
}

/// Layout geometry of element `node` for CSSOM View; `scroll_y` is the viewport's scroll
/// offset. `None` for non-elements.
pub fn element_geometry(
    doc: &Document,
    render: &RenderState,
    node: NodeId,
    scroll_y: f32,
) -> Option<ElementGeometry> {
    if !matches!(doc.get(node).kind, NodeKind::Element { .. }) {
        return None;
    }
    let none = ElementGeometry {
        offset_parent: -1,
        ..ElementGeometry::default()
    };
    let Some(layout) = render.layout.as_ref() else {
        return Some(none);
    };
    let frags = layout.node_fragments(node);
    let Some(first) = frags.first() else {
        return Some(none);
    };
    let style = &first.style;
    let bounds = frags
        .iter()
        .skip(1)
        .fold(first.rect, |acc, f| acc.union(&f.rect));
    let client_rects = frags
        .iter()
        .map(|f| {
            let r = f.rect;
            [r.x, r.y - scroll_y, r.width, r.height].map(f64::from)
        })
        .collect();

    let parent = offset_parent(doc, render, node, style);
    let origin = match parent {
        Some(p) if Some(p) != doc.body() => {
            layout.node_fragments(p).first().map_or((0.0, 0.0), |f| {
                let pr = f.padding_rect();
                (pr.x, pr.y)
            })
        }
        _ => (0.0, 0.0),
    };
    let offset = [
        first.rect.x - origin.0,
        first.rect.y - origin.1,
        bounds.width,
        bounds.height,
    ]
    .map(f64::from);

    let (vw, vh) = (layout.viewport_width, layout.viewport_height);
    let (client, scroll_size) = if Some(node) == doc.document_element() {
        (
            [0.0, 0.0, vw, vh],
            [vw.max(bounds.right()), vh.max(layout.content_height)],
        )
    } else if is_inline_non_replaced(doc, node, style) {
        ([0.0; 4], [0.0; 2])
    } else {
        let [bt, br, bb, bl] = style.border_widths();
        let pad: Rect = first.padding_rect();
        let (right, bottom) = overflow_extent(first);
        let pr = style.padding.right.resolve_or(pad.width, 0.0);
        let pb = style.padding.bottom.resolve_or(pad.width, 0.0);
        let scroll_w = pad.width.max(right + pr - pad.x);
        let scroll_h = pad.height.max(bottom + pb - pad.y);
        (
            [
                bl,
                bt,
                (first.rect.width - bl - br).max(0.0),
                (first.rect.height - bt - bb).max(0.0),
            ],
            [scroll_w, scroll_h],
        )
    };
    Some(ElementGeometry {
        client_rects,
        offset_parent: parent.map_or(-1, |p| p.0 as i32),
        offset,
        client: client.map(f64::from),
        scroll_size: scroll_size.map(f64::from),
    })
}
