//! SVG images (`<img src="*.svg">`): rasterized with resvg at their intrinsic size.
//!
//! Images run in "secure static mode": no scripts, no animation, and nothing outside the
//! document loads (only `data:` URLs resolve).

use std::sync::{Arc, OnceLock};

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{self, fontdb, roxmltree, ImageHrefResolver, Options, Size};

/// Bitmaps larger than this are rasterized at a proportionally smaller size.
const MAX_PIXELS: f32 = 16.0 * 1024.0 * 1024.0;
/// CSS default object size for replaced content without an intrinsic size.
const DEFAULT_SIZE: (f32, f32) = (300.0, 150.0);

/// Whether `bytes` are an SVG document (plain or gzip-compressed).
pub fn looks_like_svg(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.starts_with('<') && head.contains("<svg")
}

/// System fonts for `<text>`, loaded once and only by an SVG that has text.
fn system_fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// A length attribute that fixes a dimension (percentages and `auto` do not).
fn fixed_length(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        let v = v.trim();
        !v.is_empty() && !v.ends_with('%') && !v.eq_ignore_ascii_case("auto")
    })
}

fn view_box_ratio(value: Option<&str>) -> Option<f32> {
    let nums: Vec<f32> = value?
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    match nums[..] {
        [_, _, w, h] if w > 0.0 && h > 0.0 => Some(w / h),
        _ => None,
    }
}

/// CSS "default sizing algorithm" for an image with no specified size: fixed
/// `width`/`height` win; a missing one follows the `viewBox` ratio; with neither the
/// image fits the 300×150 default object size.
fn intrinsic_size(width: Option<f32>, height: Option<f32>, ratio: Option<f32>) -> (f32, f32) {
    let (dw, dh) = DEFAULT_SIZE;
    match (width, height, ratio) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some(r)) => (w, w / r),
        (Some(w), None, None) => (w, dh),
        (None, Some(h), Some(r)) => (h * r, h),
        (None, Some(h), None) => (dw, h),
        (None, None, Some(r)) if r >= dw / dh => (dw, dw / r),
        (None, None, Some(r)) => (dh * r, dh),
        (None, None, None) => (dw, dh),
    }
}

fn parse_options<'a>() -> roxmltree::ParsingOptions<'a> {
    roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    }
}

/// `text` with the root's `width` / `height` set to pixels where `[(replace, px); 2]` asks.
fn with_root_size(text: &str, root: roxmltree::Node, dims: [(bool, f32); 2]) -> String {
    let tag_end = root.range().start + 1 + root.tag_name().name().len();
    let tag_end = text[tag_end..]
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .map_or(tag_end, |i| tag_end + i);
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for ((replace, px), name) in dims.into_iter().zip(["width", "height"]) {
        if !replace {
            continue;
        }
        match root.attribute_node(name) {
            Some(attr) => edits.push((attr.range_value(), px.to_string())),
            None => edits.push((tag_end..tag_end, format!(" {name}=\"{px}\""))),
        }
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut out = text.to_string();
    for (range, value) in edits {
        out.replace_range(range, &value);
    }
    out
}

/// Decode an SVG document to straight-alpha RGBA at its intrinsic size.
pub fn decode_svg(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let data = if bytes.starts_with(&[0x1f, 0x8b]) {
        usvg::decompress_svgz(bytes).map_err(|e| e.to_string())?
    } else {
        bytes.to_vec()
    };
    let text = std::str::from_utf8(&data).map_err(|_| "SVG is not UTF-8".to_string())?;
    let doc = roxmltree::Document::parse_with_options(text, parse_options())
        .map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err("root element is not <svg>".into());
    }

    let mut opt = Options {
        resources_dir: None,
        default_size: Size::from_wh(DEFAULT_SIZE.0, DEFAULT_SIZE.1).expect("valid size"),
        image_href_resolver: ImageHrefResolver {
            resolve_data: ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..Options::default()
    };
    if doc.descendants().any(|n| n.has_tag_name("text")) {
        opt.fontdb = system_fonts();
    }
    let mut tree = usvg::Tree::from_xmltree(&doc, &opt).map_err(|e| e.to_string())?;
    let size = tree.size();

    let width = fixed_length(root.attribute("width")).then(|| size.width());
    let height = fixed_length(root.attribute("height")).then(|| size.height());
    let ratio = view_box_ratio(root.attribute("viewBox")).or_else(|| match (width, height) {
        (Some(w), Some(h)) if h > 0.0 => Some(w / h),
        _ => None,
    });
    let (mut w, mut h) = intrinsic_size(width, height, ratio);
    if width.is_none() || height.is_none() {
        // usvg resolves percentages against the viewBox, not the default object size:
        // pin the viewport to the intrinsic size so the viewBox keeps its aspect ratio.
        let sized = with_root_size(text, root, [(width.is_none(), w), (height.is_none(), h)]);
        let doc = roxmltree::Document::parse_with_options(&sized, parse_options())
            .map_err(|e| e.to_string())?;
        tree = usvg::Tree::from_xmltree(&doc, &opt).map_err(|e| e.to_string())?;
    }
    if w * h > MAX_PIXELS {
        let scale = (MAX_PIXELS / (w * h)).sqrt();
        log::warn!("SVG image {w}x{h} rasterized at {scale:.2}x");
        w *= scale;
        h *= scale;
    }
    let (pw, ph) = ((w.round() as u32).max(1), (h.round() as u32).max(1));
    let mut pixmap = Pixmap::new(pw, ph).ok_or("invalid SVG image size")?;
    let size = tree.size();
    let transform = Transform::from_scale(pw as f32 / size.width(), ph as f32 / size.height());
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut rgba = Vec::with_capacity(pw as usize * ph as usize * 4);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Ok((pw, ph, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(img: &(u32, u32, Vec<u8>), x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.0 + x) * 4) as usize;
        img.2[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn sniffs_svg_documents() {
        assert!(looks_like_svg(b"<svg xmlns='http://www.w3.org/2000/svg'/>"));
        assert!(looks_like_svg(
            b"\xef\xbb\xbf<?xml version='1.0'?>\n<!-- c -->\n<svg/>"
        ));
        assert!(!looks_like_svg(b"\x89PNG\r\n\x1a\n"));
        assert!(!looks_like_svg(b"<html><body>svg</body></html>"));
    }

    #[test]
    fn renders_at_the_declared_size() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
            <rect width="20" height="20" fill="#008000"/>
            <rect x="20" width="20" height="20" fill="red" fill-opacity="0.5"/>
        </svg>"##;
        let img = decode_svg(svg).unwrap();
        assert_eq!((img.0, img.1), (40, 20));
        assert_eq!(pixel(&img, 5, 5), [0, 128, 0, 255]);
        let half = pixel(&img, 30, 5);
        assert_eq!((half[0], half[1], half[2]), (255, 0, 0));
        assert!((126..=129).contains(&half[3]), "{half:?}");
    }

    #[test]
    fn intrinsic_size_follows_css_default_sizing() {
        let only_view_box = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"/>"#;
        let img = decode_svg(only_view_box).unwrap();
        assert_eq!((img.0, img.1), (150, 150));
        let wide = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 400 100"/>"#;
        assert_eq!(decode_svg(wide).map(|i| (i.0, i.1)).unwrap(), (300, 75));
        let width_only =
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="64" viewBox="0 0 2 1"/>"#;
        assert_eq!(
            decode_svg(width_only).map(|i| (i.0, i.1)).unwrap(),
            (64, 32)
        );
        let percent = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="50%"/>"#;
        assert_eq!(decode_svg(percent).map(|i| (i.0, i.1)).unwrap(), (300, 150));
    }

    #[test]
    fn percentage_sizes_keep_the_view_box_aspect_ratio() {
        // A centered circle filling the height: stretched unevenly, its left and right
        // edges would move toward the middle.
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="112" height="100%" viewBox="-10 -5 20 10">
            <circle r="5"/></svg>"#;
        let img = decode_svg(svg).unwrap();
        assert_eq!((img.0, img.1), (112, 56));
        assert_eq!(pixel(&img, 56, 28)[3], 255);
        assert_eq!(pixel(&img, 56 - 26, 28)[3], 255, "circle keeps its width");
        assert_eq!(pixel(&img, 56 - 30, 28)[3], 0);
    }

    #[test]
    fn external_references_do_not_load() {
        let dir = std::env::temp_dir();
        let png = dir.join("axiom-svg-external.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]))
            .save(&png)
            .unwrap();
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4">
                <image width="4" height="4" xlink:href="{}"/></svg>"#,
            png.display()
        );
        let img = decode_svg(svg.as_bytes()).unwrap();
        assert_eq!(pixel(&img, 1, 1)[3], 0, "local file must not be read");
        let _ = std::fs::remove_file(png);
    }

    #[test]
    fn rejects_non_svg_roots() {
        assert!(decode_svg(b"<html/>").is_err());
        assert!(decode_svg(b"not xml").is_err());
    }
}
