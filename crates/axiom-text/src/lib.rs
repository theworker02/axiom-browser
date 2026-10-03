//! Text for Axiom: system fonts, per-character fallback, shaping (rustybuzz), line break
//! opportunities (UAX #14), bidi levels (UAX #9) and cached glyph rasterization
//! (fontdue).
//!
//! Layout shapes each breakable segment once through [`shape`] and keeps the positioned
//! glyphs; paint draws exactly those glyphs with [`glyph_bitmap`], so measurement and
//! rendering can never disagree.

mod fonts;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use fonts::{ChainFont, Registry};

pub use fonts::parse_family_list;

/// Font request: a CSS `font-family` value, weight, style and size in px.
#[derive(Debug, Clone, PartialEq)]
pub struct FontSpec {
    pub families: Arc<str>,
    pub weight: u16,
    pub italic: bool,
    pub size: f32,
}

impl FontSpec {
    pub fn new(families: &str, weight: u16, italic: bool, size: f32) -> Self {
        Self {
            families: Arc::from(families),
            weight,
            italic,
            size,
        }
    }

    fn bold(&self) -> bool {
        self.weight >= 600
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontId(pub u16);

/// A positioned glyph. `x` is the pen position of the glyph origin relative to the start
/// of its run (offsets included); `y` is the vertical offset from the baseline, positive
/// downwards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub font: FontId,
    pub id: u16,
    pub x: f32,
    pub y: f32,
    pub embolden: bool,
}

/// Glyphs of one shaped string, in visual (left-to-right drawing) order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShapedRun {
    pub glyphs: Vec<Glyph>,
    pub width: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    /// Lowercase `x` height (CSS `ex`) and `0` advance (CSS `ch`).
    pub x_height: f32,
    pub ch_width: f32,
}

impl FontMetrics {
    /// `line-height: normal`.
    pub fn normal_line_height(&self) -> f32 {
        self.ascent + self.descent + self.line_gap
    }
}

/// Coverage bitmap of one glyph. `left` is the offset of the left edge from the glyph
/// origin; `top` is the distance from the baseline up to the top edge.
#[derive(Debug, Clone, Default)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub coverage: Vec<u8>,
}

const SHAPE_CACHE_LIMIT: usize = 200_000;
const GLYPH_CACHE_LIMIT: usize = 100_000;

#[derive(PartialEq, Eq, Hash)]
struct ShapeKey {
    text: String,
    families: Arc<str>,
    bold: bool,
    italic: bool,
    size_bits: u32,
    rtl: bool,
}

fn shape_cache() -> &'static Mutex<HashMap<ShapeKey, Arc<ShapedRun>>> {
    static CACHE: OnceLock<Mutex<HashMap<ShapeKey, Arc<ShapedRun>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

type GlyphKey = (u16, u16, u32, bool);

fn glyph_cache() -> &'static Mutex<HashMap<GlyphKey, Arc<GlyphBitmap>>> {
    static CACHE: OnceLock<Mutex<HashMap<GlyphKey, Arc<GlyphBitmap>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Metrics of the first available font for `spec`.
pub fn font_metrics(spec: &FontSpec) -> FontMetrics {
    let reg = Registry::global();
    let chain = reg.chain(&spec.families, spec.bold(), spec.italic);
    let font = chain.iter().find_map(|c| reg.font(c.entry));
    match font {
        Some(f) => FontMetrics {
            ascent: f.ascent * spec.size,
            descent: f.descent * spec.size,
            line_gap: f.line_gap * spec.size,
            x_height: f.x_height * spec.size,
            ch_width: f.ch_width * spec.size,
        },
        None => FontMetrics {
            ascent: spec.size * 0.8,
            descent: spec.size * 0.2,
            line_gap: 0.0,
            x_height: spec.size * 0.5,
            ch_width: spec.size * 0.5,
        },
    }
}

/// Makes a downloaded font (TrueType, OpenType or WOFF 1.0) available as a face of the
/// family `key` in `font-family` lists. Callers scope `key` per document so one page's
/// web fonts never become visible to another.
pub fn register_web_font(
    key: &str,
    bytes: Vec<u8>,
    bold: bool,
    italic: bool,
) -> Result<(), String> {
    Registry::global().register_web_font(&key.to_ascii_lowercase(), bytes, bold, italic)?;
    // Runs shaped for this family before the face existed used fallback fonts.
    shape_cache().lock().unwrap().clear();
    Ok(())
}

/// Width of `text` shaped left to right.
pub fn measure(text: &str, spec: &FontSpec) -> f32 {
    shape(text, spec, false).width
}

/// Shape `text` as one run in direction `rtl`, falling back per character through the
/// family chain and the system fallback fonts.
pub fn shape(text: &str, spec: &FontSpec, rtl: bool) -> Arc<ShapedRun> {
    let key = ShapeKey {
        text: text.to_string(),
        families: spec.families.clone(),
        bold: spec.bold(),
        italic: spec.italic,
        size_bits: spec.size.to_bits(),
        rtl,
    };
    if let Some(hit) = shape_cache().lock().unwrap().get(&key) {
        return hit.clone();
    }
    let run = Arc::new(shape_uncached(text, spec, rtl));
    let mut cache = shape_cache().lock().unwrap();
    if cache.len() >= SHAPE_CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, run.clone());
    run
}

fn is_cluster_extender(c: char) -> bool {
    let u = c as u32;
    // Combining marks, variation selectors, ZWJ/ZWNJ and emoji modifiers stay with the
    // font of the preceding character.
    matches!(u,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A
        | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06ED | 0x0900..=0x0903 | 0x093A..=0x094F
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C | 0x200D | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x1F3FB..=0x1F3FF | 0xE0100..=0xE01EF)
}

fn shape_uncached(text: &str, spec: &FontSpec, rtl: bool) -> ShapedRun {
    let reg = Registry::global();
    let chain = reg.chain(&spec.families, spec.bold(), spec.italic);
    type Slot = (ChainFont, Option<Arc<fonts::LoadedFont>>);
    let mut loaded: Vec<Slot> = chain.iter().map(|c| (*c, None)).collect();
    let font_of = |i: usize, loaded: &mut Vec<Slot>| {
        if loaded[i].1.is_none() {
            loaded[i].1 = reg.font(loaded[i].0.entry);
        }
        loaded[i].1.clone()
    };
    let primary = (0..loaded.len()).find(|&i| font_of(i, &mut loaded).is_some());
    let Some(primary) = primary else {
        return ShapedRun::default();
    };

    // Itemize by font.
    let mut runs: Vec<(usize, usize, usize)> = Vec::new();
    let mut current: Option<usize> = None;
    for (idx, ch) in text.char_indices() {
        let choice = if ch.is_whitespace() || ch.is_control() || is_cluster_extender(ch) {
            current.unwrap_or(primary)
        } else {
            let covered = |i: usize, loaded: &mut Vec<_>| {
                font_of(i, loaded).is_some_and(|f| f.shaper.glyph_index(ch).is_some())
            };
            if current.is_some_and(|c| c != primary && covered(c, &mut loaded))
                && !covered(primary, &mut loaded)
            {
                current.unwrap()
            } else {
                (0..loaded.len())
                    .find(|&i| covered(i, &mut loaded))
                    .unwrap_or(primary)
            }
        };
        match runs.last_mut() {
            Some((font, _, end)) if *font == choice => *end = idx + ch.len_utf8(),
            _ => runs.push((choice, idx, idx + ch.len_utf8())),
        }
        current = Some(choice);
    }
    if rtl {
        runs.reverse();
    }

    let mut out = ShapedRun::default();
    let mut pen = 0.0f32;
    for (font_i, start, end) in runs {
        let Some(font) = font_of(font_i, &mut loaded) else {
            continue;
        };
        let cf = loaded[font_i].0;
        let scale = spec.size / font.units_per_em;
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(&text[start..end]);
        buffer.set_direction(if rtl {
            rustybuzz::Direction::RightToLeft
        } else {
            rustybuzz::Direction::LeftToRight
        });
        buffer.guess_segment_properties();
        let shaped = rustybuzz::shape(&font.shaper, &[], buffer);
        for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            out.glyphs.push(Glyph {
                font: FontId(cf.entry),
                id: info.glyph_id as u16,
                x: pen + pos.x_offset as f32 * scale,
                y: -(pos.y_offset as f32) * scale,
                embolden: cf.embolden,
            });
            pen += pos.x_advance as f32 * scale;
        }
    }
    out.width = pen;
    out
}

/// Rasterized coverage of `glyph` at `size` px (cached).
pub fn glyph_bitmap(glyph: &Glyph, size: f32) -> Arc<GlyphBitmap> {
    let size_q = (size * 4.0).round().max(1.0) as u32;
    let key = (glyph.font.0, glyph.id, size_q, glyph.embolden);
    if let Some(hit) = glyph_cache().lock().unwrap().get(&key) {
        return hit.clone();
    }
    let bitmap = Arc::new(rasterize_glyph(glyph, size_q as f32 / 4.0));
    let mut cache = glyph_cache().lock().unwrap();
    if cache.len() >= GLYPH_CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, bitmap.clone());
    bitmap
}

fn rasterize_glyph(glyph: &Glyph, size: f32) -> GlyphBitmap {
    let Some(font) = Registry::global().font(glyph.font.0) else {
        return GlyphBitmap::default();
    };
    let (m, coverage) = font.raster.rasterize_indexed(glyph.id, size);
    let mut bitmap = GlyphBitmap {
        width: m.width as u32,
        height: m.height as u32,
        left: m.xmin,
        top: m.height as i32 + m.ymin,
        coverage,
    };
    if glyph.embolden && bitmap.width > 0 {
        // Synthetic bold: widen strokes by one pixel.
        let (w, h) = (bitmap.width as usize, bitmap.height as usize);
        let mut bold = vec![0u8; (w + 1) * h];
        for y in 0..h {
            for x in 0..=w {
                let a = if x < w { bitmap.coverage[y * w + x] } else { 0 };
                let b = if x > 0 {
                    bitmap.coverage[y * w + x - 1]
                } else {
                    0
                };
                bold[y * (w + 1) + x] = a.max(b);
            }
        }
        bitmap.width += 1;
        bitmap.coverage = bold;
    }
    bitmap
}

/// A line break opportunity after byte `offset`; `mandatory` for hard breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakOpportunity {
    pub offset: usize,
    pub mandatory: bool,
}

/// UAX #14 break opportunities of `text` (the end of the text is always included).
pub fn break_opportunities(text: &str) -> Vec<BreakOpportunity> {
    unicode_linebreak::linebreaks(text)
        .map(|(offset, op)| BreakOpportunity {
            offset,
            mandatory: op == unicode_linebreak::BreakOpportunity::Mandatory,
        })
        .collect()
}

/// UAX #9 embedding level of every byte of `text` and the paragraph's base level
/// (`base_rtl` forces a direction; `None` detects it from the first strong character).
pub fn bidi_levels(text: &str, base_rtl: Option<bool>) -> (Vec<u8>, u8) {
    let base = base_rtl.map(|rtl| {
        if rtl {
            unicode_bidi::Level::rtl()
        } else {
            unicode_bidi::Level::ltr()
        }
    });
    let info = unicode_bidi::BidiInfo::new(text, base);
    let para = info
        .paragraphs
        .first()
        .map(|p| p.level.number())
        .unwrap_or(0);
    (info.levels.iter().map(|l| l.number()).collect(), para)
}

/// Whether any character of `text` is strongly right-to-left (fast path for layout).
pub fn has_rtl(text: &str) -> bool {
    text.chars().any(|c| {
        let u = c as u32;
        (0x0590..=0x08FF).contains(&u)
            || (0xFB1D..=0xFDFF).contains(&u)
            || (0xFE70..=0xFEFF).contains(&u)
            || (0x10800..=0x10FFF).contains(&u)
            || (0x1E800..=0x1EFFF).contains(&u)
    })
}

/// Stable hash of a font spec (for callers that key caches on it).
pub fn spec_hash(spec: &FontSpec) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    spec.families.hash(&mut h);
    spec.weight.hash(&mut h);
    spec.italic.hash(&mut h);
    spec.size.to_bits().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sans(size: f32) -> FontSpec {
        FontSpec::new("sans-serif", 400, false, size)
    }

    #[test]
    fn latin_text_shapes_with_positive_advances() {
        let run = shape("Hello, world", &sans(16.0), false);
        assert_eq!(run.glyphs.len(), 12);
        assert!(run.width > 50.0 && run.width < 150.0, "{}", run.width);
        assert!(run.glyphs.windows(2).all(|w| w[1].x >= w[0].x));
        assert_eq!(measure("", &sans(16.0)), 0.0);
    }

    #[test]
    fn width_scales_with_size_and_bold_is_wider_or_equal() {
        let small = measure("Axiom browser", &sans(10.0));
        let big = measure("Axiom browser", &sans(20.0));
        assert!((big / small - 2.0).abs() < 0.05, "{small} {big}");
        let bold = measure(
            "Axiom browser",
            &FontSpec::new("sans-serif", 700, false, 20.0),
        );
        assert!(bold >= big * 0.98);
    }

    #[test]
    fn unknown_families_fall_back_to_sans() {
        let a = measure(
            "fallback",
            &FontSpec::new("\"Nonexistent Face\", sans-serif", 400, false, 16.0),
        );
        let b = measure("fallback", &sans(16.0));
        assert_eq!(a, b);
    }

    #[test]
    fn cjk_uses_a_fallback_font_when_available() {
        let run = shape("A漢", &sans(16.0), false);
        assert_eq!(run.glyphs.len(), 2);
        if run.glyphs[1].id != 0 {
            assert_ne!(
                run.glyphs[0].font, run.glyphs[1].font,
                "CJK from a fallback font"
            );
        }
    }

    #[test]
    fn glyph_bitmaps_have_ink_and_descenders_extend_below_the_baseline() {
        let spec = sans(32.0);
        let x = shape("x", &spec, false).glyphs[0];
        let g = shape("g", &spec, false).glyphs[0];
        let bx = glyph_bitmap(&x, spec.size);
        let bg = glyph_bitmap(&g, spec.size);
        assert!(bx.coverage.iter().any(|&c| c > 0));
        let x_bottom = bx.top - bx.height as i32;
        let g_bottom = bg.top - bg.height as i32;
        assert!(g_bottom < x_bottom - 3, "g {g_bottom} x {x_bottom}");
    }

    #[test]
    fn break_opportunities_follow_uax14() {
        let ops: Vec<usize> = break_opportunities("hello world")
            .iter()
            .map(|b| b.offset)
            .collect();
        assert_eq!(ops, [6, 11]);
        let cjk = break_opportunities("漢字漢字");
        assert!(cjk.len() >= 3, "CJK breaks between ideographs: {cjk:?}");
        assert!(break_opportunities("a\nb")[0].mandatory);
    }

    #[test]
    fn bidi_levels_mark_arabic_as_rtl() {
        let text = "abc مرحبا";
        let (levels, base) = bidi_levels(text, None);
        assert_eq!(base, 0);
        assert_eq!(levels[0], 0);
        assert_eq!(*levels.last().unwrap(), 1);
        assert!(has_rtl(text));
        assert!(!has_rtl("plain"));
    }

    #[test]
    fn metrics_are_positive() {
        let m = font_metrics(&sans(16.0));
        assert!(m.ascent > 10.0 && m.descent > 2.0);
        assert!(m.normal_line_height() > 16.0);
        assert!(m.x_height > 5.0 && m.x_height < 12.0, "{}", m.x_height);
        assert!(m.ch_width > 5.0 && m.ch_width < 12.0, "{}", m.ch_width);
    }

    fn ahem() -> Vec<u8> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/wpt/fonts/Ahem.ttf"
        );
        std::fs::read(path).expect("tests/wpt/fonts/Ahem.ttf")
    }

    /// Wraps an sfnt in WOFF 1.0, compressing every table that shrinks.
    fn to_woff(sfnt: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let be16 = |o: usize| u16::from_be_bytes([sfnt[o], sfnt[o + 1]]);
        let be32 = |o: usize| u32::from_be_bytes([sfnt[o], sfnt[o + 1], sfnt[o + 2], sfnt[o + 3]]);
        let n = be16(4) as usize;
        let mut dir = Vec::new();
        let mut data = Vec::new();
        let data_start = 44 + 20 * n;
        for i in 0..n {
            let r = 12 + 16 * i;
            let (tag, checksum, offset, len) = (
                be32(r),
                be32(r + 4),
                be32(r + 8) as usize,
                be32(r + 12) as usize,
            );
            let table = &sfnt[offset..offset + len];
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
            z.write_all(table).unwrap();
            let z = z.finish().unwrap();
            let stored: &[u8] = if z.len() < len { &z } else { table };
            for v in [
                tag,
                (data_start + data.len()) as u32,
                stored.len() as u32,
                len as u32,
                checksum,
            ] {
                dir.extend_from_slice(&v.to_be_bytes());
            }
            data.extend_from_slice(stored);
            data.resize(data.len().div_ceil(4) * 4, 0);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"wOFF");
        out.extend_from_slice(&sfnt[..4]);
        out.extend_from_slice(&((data_start + data.len()) as u32).to_be_bytes());
        out.extend_from_slice(&(n as u16).to_be_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&(sfnt.len() as u32).to_be_bytes());
        out.resize(44, 0);
        out.extend_from_slice(&dir);
        out.extend_from_slice(&data);
        out
    }

    /// Wraps an sfnt in WOFF 2.0 with every table untransformed, in one Brotli stream.
    fn to_woff2(sfnt: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let be16 = |o: usize| u16::from_be_bytes([sfnt[o], sfnt[o + 1]]);
        let be32 = |o: usize| u32::from_be_bytes([sfnt[o], sfnt[o + 1], sfnt[o + 2], sfnt[o + 3]]);
        let base128 = |out: &mut Vec<u8>, mut v: u32| {
            let mut bytes = vec![(v & 0x7f) as u8];
            v >>= 7;
            while v > 0 {
                bytes.push((v & 0x7f) as u8 | 0x80);
                v >>= 7;
            }
            out.extend(bytes.iter().rev());
        };
        let n = be16(4) as usize;
        let mut dir = Vec::new();
        let mut stream = Vec::new();
        for i in 0..n {
            let r = 12 + 16 * i;
            let (tag, offset, len) = (be32(r), be32(r + 8) as usize, be32(r + 12) as usize);
            // Arbitrary tag (63); glyf/loca need transform version 3 to be stored as-is.
            let null = if matches!(&tag.to_be_bytes(), b"glyf" | b"loca") {
                3 << 6
            } else {
                0
            };
            dir.push(63 | null);
            dir.extend_from_slice(&tag.to_be_bytes());
            base128(&mut dir, len as u32);
            stream.extend_from_slice(&sfnt[offset..offset + len]);
        }
        let mut compressed = Vec::new();
        {
            let mut w = brotli::CompressorWriter::new(&mut compressed, 4096, 11, 22);
            w.write_all(&stream).unwrap();
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"wOF2");
        out.extend_from_slice(&sfnt[..4]);
        let length_at = out.len();
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(n as u16).to_be_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&(sfnt.len() as u32).to_be_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
        out.resize(48, 0);
        out.extend_from_slice(&dir);
        out.extend_from_slice(&compressed);
        out.resize(out.len().div_ceil(4) * 4, 0);
        let total = (out.len() as u32).to_be_bytes();
        out[length_at..length_at + 4].copy_from_slice(&total);
        out
    }

    #[test]
    fn web_fonts_register_from_ttf_and_woff() {
        let ttf = ahem();
        register_web_font("Test Ahem\u{1}ttf", ttf.clone(), false, false).unwrap();
        register_web_font("test ahem\u{1}woff", to_woff(&ttf), false, false).unwrap();
        register_web_font("test ahem\u{1}woff2", to_woff2(&ttf), false, false).unwrap();
        for family in [
            "\"Test Ahem\u{1}ttf\", serif",
            "'test ahem\u{1}woff'",
            "'test ahem\u{1}woff2'",
        ] {
            let spec = FontSpec::new(family, 400, false, 20.0);
            let m = font_metrics(&spec);
            assert_eq!((m.ascent, m.descent), (16.0, 4.0), "{family}");
            assert_eq!((m.x_height, m.ch_width), (16.0, 20.0), "{family}");
            assert_eq!(measure("xxxx", &spec), 80.0, "{family}");
        }
        // Unregistered keys (another document's scope) never resolve to the web font.
        assert_ne!(
            measure(
                "xxxx",
                &FontSpec::new("test ahem\u{1}other", 400, false, 20.0)
            ),
            80.0
        );
        assert!(register_web_font("x", b"wOF2....".to_vec(), false, false)
            .unwrap_err()
            .contains("WOFF2"));
        let mut truncated = to_woff2(&ttf);
        truncated.truncate(truncated.len() / 2);
        assert!(register_web_font("x", truncated, false, false).is_err());
        assert!(register_web_font("x", b"<html>".to_vec(), false, false).is_err());
    }
}
