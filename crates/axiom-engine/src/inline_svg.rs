//! Inline `<svg>` (HTML §4.8.16). Every rendered SVG root whose parent is not itself an SVG
//! element is serialized as a standalone SVG document, with the cascade's `fill` /
//! `stroke` values written in (author CSS such as `.octicon { fill: currentColor }` never
//! reaches the rasterizer otherwise), and rasterized like an SVG image in secure static
//! mode. Style gives each root a first bitmap at its CSS or attribute size (its intrinsic
//! size for layout); after layout, roots whose box came out a different size are drawn
//! again at that size, so `preserveAspectRatio` applies instead of the bitmap stretching.
//! Identical markup shares one bitmap.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use axiom_dom::{Document, Namespace, NodeId, NodeKind};
use axiom_layout::{Fragment, FragmentKind, LayoutTree, RasterImage};
use axiom_style::serialize::{self, number};
use axiom_style::{ComputedStyle, Display, Length, StyleMap};

/// Rasterized inline SVG documents keyed by their serialized markup.
#[derive(Default)]
pub struct InlineSvgCache {
    entries: HashMap<String, Option<Arc<RasterImage>>>,
    /// Markup used since the last style pass began; everything else is evicted by the next.
    live: HashSet<String>,
}

impl InlineSvgCache {
    /// Bitmaps for the rendered SVG roots in `styles`, at their CSS or attribute size.
    pub fn rasterize(
        &mut self,
        doc: &Document,
        styles: &StyleMap,
    ) -> HashMap<NodeId, Arc<RasterImage>> {
        let live = std::mem::take(&mut self.live);
        self.entries.retain(|markup, _| live.contains(markup));
        let mut out = HashMap::new();
        for (id, style) in styles.iter() {
            if !is_svg_root(doc, id) {
                continue;
            }
            let size = [style.width.clone(), style.height.clone()].map(|l| match l {
                Length::Px(px) => Some(px),
                _ => None,
            });
            if let Some(image) = self.image(serialize_root(doc, styles, id, style, size)) {
                out.insert(id, image);
            }
        }
        out
    }

    /// Redraw inline SVG roots whose laid-out content box differs from their bitmap at
    /// the box's size, swapping the new bitmap into `layout` and `images`.
    pub fn fit_to_layout(
        &mut self,
        doc: &Document,
        styles: &StyleMap,
        layout: &mut LayoutTree,
        images: &mut HashMap<NodeId, Arc<RasterImage>>,
    ) {
        fn walk(
            cache: &mut InlineSvgCache,
            doc: &Document,
            styles: &StyleMap,
            f: &mut Fragment,
            images: &mut HashMap<NodeId, Arc<RasterImage>>,
        ) {
            if let (FragmentKind::Image(image), Some(id)) = (&mut f.kind, f.node) {
                let (w, h) = (f.rect.width.round(), f.rect.height.round());
                let differs = image.width as f32 != w || image.height as f32 != h;
                if differs && w >= 1.0 && h >= 1.0 && images.contains_key(&id) {
                    if let Some(style) = styles.get(id) {
                        let markup = serialize_root(doc, styles, id, style, [Some(w), Some(h)]);
                        if let Some(fitted) = cache.image(markup) {
                            *image = Arc::clone(&fitted);
                            images.insert(id, fitted);
                        }
                    }
                }
            }
            for c in &mut f.children {
                walk(cache, doc, styles, c, images);
            }
        }
        walk(self, doc, styles, &mut layout.root, images);
    }

    fn image(&mut self, markup: String) -> Option<Arc<RasterImage>> {
        self.live.insert(markup.clone());
        self.entries
            .entry(markup)
            .or_insert_with_key(
                |markup| match axiom_loader::decode_image(markup.as_bytes()) {
                    Ok((width, height, rgba)) => Some(Arc::new(RasterImage {
                        width,
                        height,
                        rgba,
                    })),
                    Err(e) => {
                        log::debug!("inline <svg> not rendered: {e}");
                        None
                    }
                },
            )
            .clone()
    }
}

fn is_svg(doc: &Document, id: NodeId) -> bool {
    doc.namespace(id) == Some(Namespace::Svg)
}

fn is_svg_root(doc: &Document, id: NodeId) -> bool {
    is_svg(doc, id)
        && doc.tag_name(id) == Some("svg")
        && !doc.get(id).parent.is_some_and(|p| is_svg(doc, p))
}

/// The properties written onto elements, as `(name, value)` with `currentColor` resolved.
fn paint_properties(s: &ComputedStyle) -> [(&'static str, String); 5] {
    let stroke_width = match &s.stroke_width {
        Length::Px(px) => number(*px),
        other => serialize::length(other),
    };
    [
        ("fill", serialize::paint(&s.fill, s.color)),
        ("stroke", serialize::paint(&s.stroke, s.color)),
        ("stroke-width", stroke_width),
        ("fill-opacity", number(s.fill_opacity)),
        ("stroke-opacity", number(s.stroke_opacity)),
    ]
}

/// The root's markup; `size` (px) replaces its `width` / `height` attributes.
fn serialize_root(
    doc: &Document,
    styles: &StyleMap,
    root: NodeId,
    style: &ComputedStyle,
    size: [Option<f32>; 2],
) -> String {
    let mut out = String::new();
    write_element(doc, styles, root, None, Some(style), size, &mut out);
    out
}

fn write_element(
    doc: &Document,
    styles: &StyleMap,
    id: NodeId,
    parent: Option<&ComputedStyle>,
    style: Option<&ComputedStyle>,
    size: [Option<f32>; 2],
    out: &mut String,
) {
    let Some(tag) = doc.tag_name(id) else {
        return;
    };
    // Scripts never run in SVG rendered this way, and HTML content is not drawn.
    if matches!(tag, "script" | "foreignObject") {
        return;
    }
    let is_root = parent.is_none();
    out.push('<');
    out.push_str(tag);
    if is_root {
        out.push_str(
            " xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"",
        );
    }
    let mut declarations: Vec<String> = Vec::new();
    for attr in doc.attributes(id).iter() {
        let name = attr.qualified_name();
        if name == "xmlns" || name.starts_with("xmlns:") || !is_xml_name(&name) {
            continue;
        }
        if name == "style" {
            declarations.extend(
                attr.value
                    .split(';')
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                    .map(str::to_string),
            );
            continue;
        }
        let replaced = match name.as_str() {
            "width" => size[0].is_some(),
            "height" => size[1].is_some(),
            _ => false,
        };
        if is_root && replaced {
            continue;
        }
        write_attr(out, &name, &attr.value);
    }
    if is_root {
        for (name, px) in ["width", "height"].into_iter().zip(size) {
            if let Some(px) = px {
                write_attr(out, name, &number(px));
            }
        }
    }
    match style {
        Some(s) if s.display != Display::None => {
            let own = paint_properties(s);
            let inherited = parent.map(paint_properties);
            for (i, (name, value)) in own.iter().enumerate() {
                if inherited.as_ref().is_none_or(|p| p[i].1 != *value) {
                    declarations.push(format!("{name}:{value}"));
                }
            }
        }
        // Without a style of its own, a child of a rendered element is `display: none`.
        _ => declarations.push("display:none".into()),
    }
    if !declarations.is_empty() {
        write_attr(out, "style", &declarations.join(";"));
    }
    out.push('>');
    for &child in &doc.get(id).children {
        match &doc.get(child).kind {
            NodeKind::Element { .. } if is_svg(doc, child) => {
                let child_style = styles.get(child).map(|s| &**s);
                write_element(doc, styles, child, style, child_style, [None; 2], out);
            }
            NodeKind::Text { data } | NodeKind::CData { data } => escape_into(out, data, false),
            _ => {}
        }
    }
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
}

fn write_attr(out: &mut String, name: &str, value: &str) {
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    escape_into(out, value, true);
    out.push('"');
}

fn escape_into(out: &mut String, text: &str, attr: bool) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            '\n' | '\t' | '\r' if attr => {
                let _ = write!(out, "&#{};", c as u32);
            }
            c if (c as u32) < 0x20 && !matches!(c, '\n' | '\t' | '\r') => {}
            c => out.push(c),
        }
    }
}

/// A conservative XML `Name` check: attributes the HTML parser accepted but XML would
/// reject are dropped instead of failing the whole document.
fn is_xml_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_' || first == ':')
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '-' | '.'))
        && name.matches(':').count() <= 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_style::StyleEngine;

    fn render(html: &str) -> (Document, StyleMap) {
        let doc = axiom_html::parse_html(html).expect("parses");
        let mut engine = StyleEngine::with_viewport(200.0, 200.0);
        engine.add_author_css(&doc.collect_style_text());
        let styles = engine.compute_document(&doc);
        (doc, styles)
    }

    fn root(doc: &Document) -> NodeId {
        doc.find_descendant(doc.body().unwrap(), "svg").unwrap()
    }

    #[test]
    fn author_css_paint_reaches_the_markup() {
        let (doc, styles) = render(
            "<style>body { color: rgb(0, 128, 0) } .i { fill: currentColor }</style>\
             <svg class=i width=16 height=16 viewBox='0 0 16 16'>\
             <path d='M0 0h16v16H0z'/><path fill=none stroke=red d='M0 0L16 16'/></svg>",
        );
        let id = root(&doc);
        let markup = serialize_root(&doc, &styles, id, styles.get(id).unwrap(), [None; 2]);
        assert!(
            markup.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""),
            "{markup}"
        );
        assert!(markup.contains("fill:rgb(0, 128, 0)"), "{markup}");
        assert!(
            markup.contains("fill:none;stroke:rgb(255, 0, 0)"),
            "{markup}"
        );
        let images = InlineSvgCache::default().rasterize(&doc, &styles);
        let img = &images[&id];
        assert_eq!((img.width, img.height), (16, 16));
        let px = &img.rgba[(8 * 16 + 2) * 4..(8 * 16 + 2) * 4 + 4];
        assert_eq!(px, [0, 128, 0, 255], "{markup}");
    }

    #[test]
    fn scripts_foreign_objects_and_hidden_children_are_left_out() {
        let (doc, styles) = render(
            "<style>.gone { display: none }</style>\
             <svg width=4 height=4><script>alert(1)</script>\
             <foreignObject><p>html</p></foreignObject><rect class=gone width=4 height=4 /></svg>",
        );
        let id = root(&doc);
        let markup = serialize_root(&doc, &styles, id, styles.get(id).unwrap(), [None; 2]);
        assert!(
            !markup.contains("script") && !markup.contains("foreignObject"),
            "{markup}"
        );
        assert!(markup.contains("display:none"), "{markup}");
    }

    #[test]
    fn identical_icons_share_one_bitmap() {
        let (doc, styles) = render(
            "<svg width=8 height=8><rect width=8 height=8 /></svg>\
             <svg width=8 height=8><rect width=8 height=8 /></svg>",
        );
        let mut cache = InlineSvgCache::default();
        let images = cache.rasterize(&doc, &styles);
        let bitmaps: Vec<_> = images.values().collect();
        assert_eq!(bitmaps.len(), 2);
        assert!(Arc::ptr_eq(bitmaps[0], bitmaps[1]));
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn em_and_percent_sizes_redraw_at_the_laid_out_size() {
        let (doc, styles) = render(
            "<style>body { margin: 0; font-size: 10px } div { width: 60px; height: 20px }</style>\
             <svg id=em width=2em height=2em viewBox='0 0 1 1'><rect width=1 height=1 /></svg>\
             <div><svg id=pct width=100% height=100% viewBox='0 0 1 1' style='display: block'>\
             <rect width=1 height=1 /></svg></div>",
        );
        let mut cache = InlineSvgCache::default();
        let mut images = cache.rasterize(&doc, &styles);
        let em = doc.get_element_by_id("em").unwrap();
        let pct = doc.get_element_by_id("pct").unwrap();
        assert_eq!((images[&em].width, images[&em].height), (20, 20));
        let mut layout = axiom_layout::layout_document(
            &doc,
            &styles,
            &axiom_layout::LayoutInputs {
                inline_svgs: &images,
                ..axiom_layout::LayoutInputs::empty()
            },
            200.0,
            200.0,
        );
        cache.fit_to_layout(&doc, &styles, &mut layout, &mut images);
        let fitted = &images[&pct];
        assert_eq!((fitted.width, fitted.height), (60, 20));
        // `xMidYMid meet`: the square sits centered, not stretched across the box.
        let alpha = |x: u32| fitted.rgba[((10 * 60 + x) * 4 + 3) as usize];
        assert_eq!((alpha(5), alpha(30), alpha(55)), (0, 255, 0));
    }

    #[test]
    fn view_box_only_icons_fill_the_available_width() {
        let (doc, styles) = render(
            "<style>body { margin: 0 } a { display: block; width: 40px; padding: 8px }\
             b { display: block; position: relative; width: 48px; height: 48px }\
             #abs { position: absolute; left: 0; top: 0 }</style>\
             <a><svg id=icon viewBox='0 0 24 24'><rect width=24 height=24 /></svg></a>\
             <b><svg id=abs viewBox='0 0 2 1'><rect width=2 height=1 /></svg></b>",
        );
        let mut cache = InlineSvgCache::default();
        let mut images = cache.rasterize(&doc, &styles);
        let mut layout = axiom_layout::layout_document(
            &doc,
            &styles,
            &axiom_layout::LayoutInputs {
                inline_svgs: &images,
                ..axiom_layout::LayoutInputs::empty()
            },
            200.0,
            200.0,
        );
        cache.fit_to_layout(&doc, &styles, &mut layout, &mut images);
        let size = |id: &str| {
            let img = &images[&doc.get_element_by_id(id).unwrap()];
            (img.width, img.height)
        };
        assert_eq!(size("icon"), (40, 40));
        assert_eq!(size("abs"), (48, 24));
    }
}
