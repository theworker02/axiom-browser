//! Engine integration smoke tests.

use axiom_engine::{Engine, NavigateOptions};

#[test]
fn renders_example_like_fixture() {
    let html = include_str!("../../../tests/html/example-like.html");
    let engine = Engine::new();
    let page = engine
        .render_html(
            html,
            NavigateOptions {
                viewport_width: 800,
                viewport_height: 600,
                ..NavigateOptions::default()
            },
        )
        .expect("render");
    assert_eq!(page.title, "Axiom Fixture");
    assert_eq!(page.framebuffer.width, 800);
    assert_eq!(page.framebuffer.height, 600);
    assert!(!page.display_list.commands.is_empty());
    // Expect non-white pixels somewhere (grey background / text).
    let non_white = page
        .framebuffer
        .pixels
        .iter()
        .filter(|&&p| p != 0xff_ff_ff_ff)
        .count();
    assert!(
        non_white > 100,
        "expected painted content, got {non_white} non-white pixels"
    );
}

/// Ink extent `(top, bottom)` of each run of inked columns, left to right.
fn glyph_extents(fb: &axiom_paint::Framebuffer) -> Vec<(u32, u32)> {
    let ink = |x: u32, y: u32| (fb.pixels[(y * fb.width + x) as usize] & 0xff) < 128;
    let mut out = Vec::new();
    let mut current: Option<(u32, u32)> = None;
    for x in 0..fb.width {
        let rows: Vec<u32> = (0..fb.height).filter(|&y| ink(x, y)).collect();
        match (rows.first(), rows.last(), current) {
            (Some(&t), Some(&b), None) => current = Some((t, b)),
            (Some(&t), Some(&b), Some((ct, cb))) => current = Some((ct.min(t), cb.max(b))),
            (None, _, Some(ext)) => {
                out.push(ext);
                current = None;
            }
            _ => {}
        }
    }
    out.extend(current);
    out
}

/// Glyphs sit on a shared baseline: a descender (`g`) extends below the bottom of `x`,
/// and both start at about the x-height. Regression test for bitmaps placed at
/// `baseline + ymin`, which drew descenders raised and everything else lowered.
#[test]
fn glyphs_share_a_baseline_and_descenders_hang_below_it() {
    let html = include_str!("../../../tests/rendering/glyph-baseline.html");
    let page = Engine::new()
        .render_html(
            html,
            NavigateOptions {
                viewport_width: 200,
                viewport_height: 100,
                ..NavigateOptions::default()
            },
        )
        .expect("render");
    let glyphs = glyph_extents(&page.framebuffer);
    assert_eq!(
        glyphs.len(),
        2,
        "expected the ink of `x` and `g`: {glyphs:?}"
    );
    let (x_top, x_bottom) = glyphs[0];
    let (g_top, g_bottom) = glyphs[1];
    assert!(
        g_bottom >= x_bottom + 7,
        "descender must hang below the baseline: x {glyphs:?}"
    );
    assert!(
        x_top.abs_diff(g_top) <= 3,
        "x and g must both start near the x-height: {glyphs:?}"
    );
}

/// Floats sit beside block formatting contexts, table cells line up in columns, and an
/// inline box split around a block paints nothing of its own.
#[test]
fn floats_tables_and_block_in_inline_paint_where_expected() {
    let html = include_str!("../../../tests/rendering/floats-tables-block-in-inline.html");
    let page = Engine::new()
        .render_html(
            html,
            NavigateOptions {
                viewport_width: 200,
                viewport_height: 100,
                ..NavigateOptions::default()
            },
        )
        .expect("render");
    let fb = &page.framebuffer;
    // Pixels are 0xAABBGGRR.
    let at = |x: u32, y: u32| fb.pixels[(y * fb.width + x) as usize];
    assert_eq!(at(25, 25), 0xff_ff_00_00, "float");
    assert_eq!(at(100, 25), 0xff_00_80_00, "BFC beside the float");
    assert_eq!(at(20, 60), 0xff_ff_ff_00, "first cell");
    assert_eq!(at(60, 60), 0xff_ff_00_ff, "second cell");
    assert_eq!(
        at(100, 60),
        0xff_ff_ff_ff,
        "table is only as wide as its cells"
    );
    assert_eq!(
        at(100, 80),
        0xff_00_ff_ff,
        "block inside the inline, without margins"
    );
    assert!(
        !fb.pixels.contains(&0xff_00_00_ff),
        "the inline box's background must not paint"
    );
}

/// Flex items share free space by their grow factors and stretch across the line; grid
/// items land in named areas across a fixed track, a gap and an `fr` track, and
/// self-alignment centers a box in its area.
#[test]
fn flex_and_grid_containers_paint_where_expected() {
    let html = include_str!("../../../tests/rendering/flex-grid.html");
    let page = Engine::new()
        .render_html(
            html,
            NavigateOptions {
                viewport_width: 200,
                viewport_height: 100,
                ..NavigateOptions::default()
            },
        )
        .expect("render");
    let fb = &page.framebuffer;
    // Pixels are 0xAABBGGRR.
    let at = |x: u32, y: u32| fb.pixels[(y * fb.width + x) as usize];
    assert_eq!(at(25, 20), 0xff_ff_00_00, "flex: 1 item is 50px");
    assert_eq!(at(55, 20), 0xff_00_80_00, "flex: 2 item starts at 50px");
    assert_eq!(at(145, 20), 0xff_00_80_00, "and is 100px wide");
    assert_eq!(at(155, 38), 0xff_00_00_ff, "height:auto item stretches");
    assert_eq!(at(5, 45), 0xff_ff_ff_00, "side area: the 60px column");
    assert_eq!(at(70, 70), 0xff_ff_ff_ff, "column gap");
    assert_eq!(at(81, 45), 0xff_ff_00_ff, "main area: the fr column");
    assert_eq!(at(199, 99), 0xff_ff_00_ff, "fr column fills the width");
    // The 10px dot is centered in the 60x60 side area: x 25..35, y 65..75.
    for (x, y) in [(25, 65), (34, 74), (30, 70)] {
        assert_eq!(at(x, y), 0xff_00_ff_ff, "centered dot at ({x}, {y})");
    }
    for (x, y) in [(24, 70), (35, 70), (30, 64), (30, 75)] {
        assert_eq!(
            at(x, y),
            0xff_ff_ff_00,
            "side area around the dot at ({x}, {y})"
        );
    }
}

/// A declarative shadow root renders its own content and slots in the flat tree: shadow
/// styles stay scoped, slotted children take document rules over `::slotted`, a slot with
/// nothing assigned shows its fallback, and unassigned light children are not rendered.
#[test]
fn declarative_shadow_roots_render_the_flat_tree() {
    let html = include_str!("../../../tests/rendering/declarative-shadow-dom.html");
    let page = Engine::new()
        .render_html(
            html,
            NavigateOptions {
                viewport_width: 200,
                viewport_height: 100,
                ..NavigateOptions::default()
            },
        )
        .expect("render");
    let fb = &page.framebuffer;
    // Pixels are 0xAABBGGRR.
    let at = |x: u32, y: u32| fb.pixels[(y * fb.width + x) as usize];
    assert_eq!(
        at(50, 10),
        0xff_00_80_00,
        "shadow content with shadow styles"
    );
    assert_eq!(
        at(50, 30),
        0xff_ff_00_00,
        "slotted child, document rule wins"
    );
    assert_eq!(at(50, 50), 0xff_ff_ff_00, "slot fallback content");
    assert_eq!(at(50, 70), 0xff_00_00_00, "the box after the host");
    assert_eq!(at(150, 10), 0xff_ff_ff_ff, "the host is 100px wide");
    for (color, what) in [(0xff_00_00_ff, "red"), (0xff_ff_00_ff, "magenta")] {
        assert!(!fb.pixels.contains(&color), "no {what} pixels");
    }
}

/// Inline `<svg>` paints with the cascade's paint: `fill: currentColor` from author CSS
/// follows each icon's color, presentation attributes and `style` apply, and hidden icons
/// stay unpainted.
#[test]
fn inline_svg_paints_with_css_fill_and_stroke() {
    let html = include_str!("../../../tests/rendering/inline-svg.html");
    let page = Engine::new()
        .render_html(
            html,
            NavigateOptions {
                viewport_width: 100,
                viewport_height: 100,
                ..NavigateOptions::default()
            },
        )
        .expect("render");
    let fb = &page.framebuffer;
    // Pixels are 0xAABBGGRR.
    let at = |x: u32, y: u32| fb.pixels[(y * fb.width + x) as usize];
    assert_eq!(at(10, 10), 0xff_00_80_00, "icon filled with the page color");
    assert_eq!(at(10, 30), 0xff_ff_00_00, "icon filled with its own color");
    assert_eq!(at(1, 41), 0xff_00_00_00, "stroked frame");
    assert_eq!(at(10, 50), 0xff_ff_ff_ff, "the frame is not filled");
    assert_eq!(at(30, 50), 0xff_00_ff_ff, "style attribute fill");
    assert_eq!(at(10, 70), 0xff_ff_ff_ff, "nothing below the last icon");
    assert!(!fb.pixels.contains(&0xff_00_00_ff), "no red pixels");
}

/// `<center>` centers a table; percentage columns widen it so the auto column gets the
/// share of the width left over by them.
#[test]
fn centered_table_gives_percentage_columns_their_share() {
    let html = include_str!("../../../tests/rendering/center-percent-table.html");
    let page = Engine::new()
        .render_html(
            html,
            NavigateOptions {
                viewport_width: 200,
                viewport_height: 20,
                ..NavigateOptions::default()
            },
        )
        .expect("render");
    let fb = &page.framebuffer;
    // Pixels are 0xAABBGGRR. The 80px table spans x 60..140.
    let at = |x: u32| fb.pixels[(5 * fb.width + x) as usize];
    assert_eq!(at(55), 0xff_ff_ff_ff, "left of the table");
    assert_eq!(at(70), 0xff_00_00_ff, "left 25% column");
    assert_eq!(at(100), 0xff_ff_00_00, "middle column");
    assert_eq!(at(130), 0xff_00_ff_00, "right 25% column");
    assert_eq!(at(145), 0xff_ff_ff_ff, "right of the table");
}

/// Background layers: a gradient, an SVG mask that shows half of its box, a tiled image
/// over a color, a positioned gradient layer and `background-clip: content-box`.
#[test]
fn background_and_mask_layers_paint_where_expected() {
    let html = include_str!("../../../tests/rendering/background-layers.html");
    let page = Engine::new()
        .render_html(
            html,
            NavigateOptions {
                viewport_width: 200,
                viewport_height: 100,
                ..NavigateOptions::default()
            },
        )
        .expect("render");
    let fb = &page.framebuffer;
    // Pixels are 0xAABBGGRR.
    let at = |x: u32, y: u32| fb.pixels[(y * fb.width + x) as usize];
    let red_blue = |p: u32| (p & 0xff, (p >> 16) & 0xff);
    let (r, b) = red_blue(at(1, 10));
    assert!(r > 240 && b < 16, "gradient starts red: {:08x}", at(1, 10));
    let (r, b) = red_blue(at(98, 10));
    assert!(r < 16 && b > 240, "gradient ends blue: {:08x}", at(98, 10));
    assert_eq!(at(5, 30), 0xff_00_80_00, "mask keeps the left half");
    assert_eq!(at(15, 30), 0xff_ff_ff_ff, "mask hides the right half");
    assert_eq!(at(2, 45), 0xff_00_00_ff, "tile");
    assert_eq!(at(7, 45), 0xff_00_ff_ff, "color under the tile");
    assert_eq!(at(12, 55), 0xff_00_00_ff, "next tile");
    assert_eq!(at(35, 70), 0xff_ff_00_00, "gradient layer on the right");
    assert_eq!(at(5, 70), 0xff_00_ff_00, "color beside it");
    assert_eq!(at(2, 90), 0xff_ff_ff_ff, "padding outside the clip");
    assert_eq!(at(20, 90), 0xff_ff_ff_00, "content box");
}

/// `::before` / `::after` generate boxes: a clearfix `::after` contains a float, block
/// pseudo-elements stack around the element's content, and `content: none` / `normal`
/// generate nothing.
#[test]
fn before_and_after_generate_boxes() {
    let html = include_str!("../../../tests/rendering/generated-content.html");
    let page = Engine::new()
        .render_html(
            html,
            NavigateOptions {
                viewport_width: 100,
                viewport_height: 100,
                ..NavigateOptions::default()
            },
        )
        .expect("render");
    let fb = &page.framebuffer;
    // Pixels are 0xAABBGGRR.
    let at = |x: u32, y: u32| fb.pixels[(y * fb.width + x) as usize];
    assert_eq!(at(10, 15), 0xff_ff_00_00, "float");
    assert_eq!(at(50, 35), 0xff_00_ff_00, "the clearfix pushes the next box below it");
    assert_eq!(at(5, 45), 0xff_00_00_ff, "::before");
    assert_eq!(at(25, 55), 0xff_00_80_00, ":after");
    assert_eq!(at(5, 55), 0xff_ff_ff_ff, "beside the :after box");
    assert!(
        !fb.pixels.contains(&0xff_00_00_00),
        "content: none / normal generate no box"
    );
}

/// Markup without `<html>`, `<head>` or `<body>` tags still renders: the tree builder
/// implies those elements, so the title, the style sheet and the box land where they
/// would with the tags written out.
#[test]
fn renders_content_when_html_head_and_body_tags_are_omitted() {
    let html = include_str!("../../../tests/html/implied-body.html");
    let page = Engine::new()
        .render_html(
            html,
            NavigateOptions {
                viewport_width: 200,
                viewport_height: 200,
                ..NavigateOptions::default()
            },
        )
        .expect("render");
    assert_eq!(page.title, "Implied body");
    let green = page
        .framebuffer
        .pixels
        .iter()
        .filter(|&&p| p == 0xff_00_80_00)
        .count();
    assert_eq!(green, 100 * 100, "expected the 100x100 green box");
}
