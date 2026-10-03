use super::*;
use axiom_layout::{layout_document, LayoutInputs};
use axiom_style::StyleEngine;

fn render(html: &str, w: u32, h: u32) -> (DisplayList, Framebuffer) {
    let doc = axiom_html::parse_html(html).expect("parse");
    let mut engine = StyleEngine::with_viewport(w as f32, h as f32);
    engine.add_author_css(&doc.collect_style_text());
    let styles = engine.compute_document(&doc);
    let tree = layout_document(&doc, &styles, &LayoutInputs::empty(), w as f32, h as f32);
    let list = build_display_list(&tree);
    let fb = rasterize(&list, w, h);
    (list, fb)
}

fn px(fb: &Framebuffer, x: u32, y: u32) -> (u8, u8, u8) {
    let p = fb.pixels[(y * fb.width + x) as usize];
    (p as u8, (p >> 8) as u8, (p >> 16) as u8)
}

const BODY0: &str = "<style>body{margin:0}</style>";

#[test]
fn backgrounds_and_borders() {
    let (_, fb) = render(
        &format!("{BODY0}<div style='width:50px;height:40px;background:#f00;border:5px solid #00f'></div>"),
        100,
        100,
    );
    assert_eq!(px(&fb, 2, 2), (0, 0, 255));
    assert_eq!(px(&fb, 30, 25), (255, 0, 0));
    assert_eq!(px(&fb, 58, 48), (0, 0, 255));
    assert_eq!(px(&fb, 80, 80), (255, 255, 255));
}

#[test]
fn canvas_background_from_body() {
    let (_, fb) = render("<body style='background:#0f0'>x</body>", 60, 60);
    assert_eq!(px(&fb, 59, 59), (0, 255, 0));
}

#[test]
fn text_is_drawn_on_its_baseline() {
    let (list, fb) = render(
        &format!("{BODY0}<p style='margin:0;font-size:20px'>Hxg</p>"),
        120,
        60,
    );
    let text = list
        .commands
        .iter()
        .find_map(|c| match c {
            DisplayCommand::Text { text, baseline, .. } => Some((text.clone(), *baseline)),
            _ => None,
        })
        .expect("text command");
    assert_eq!(text.0, "Hxg");
    let dark_rows: Vec<u32> = (0..fb.height)
        .filter(|&y| (0..fb.width).any(|x| px(&fb, x, y).0 < 128))
        .collect();
    let (top, bottom) = (
        *dark_rows.first().unwrap() as f32,
        *dark_rows.last().unwrap() as f32,
    );
    assert!(
        top < text.1 && bottom > text.1,
        "ink spans the baseline: {top}..{bottom} vs {}",
        text.1
    );
    assert!(text.1 - top > 8.0, "cap height above baseline");
}

#[test]
fn z_index_orders_positioned_boxes() {
    let (_, fb) = render(
        &format!(
            "{BODY0}<div style='position:relative'>
             <div style='position:absolute;left:0;top:0;width:40px;height:40px;background:#f00;z-index:2'></div>
             <div style='position:absolute;left:0;top:0;width:40px;height:40px;background:#00f;z-index:1'></div>
             </div>"
        ),
        60,
        60,
    );
    assert_eq!(px(&fb, 10, 10), (255, 0, 0));
}

#[test]
fn positioned_paints_over_later_flow_content() {
    let (_, fb) = render(
        &format!(
            "{BODY0}<div style='position:relative;left:0;top:0;height:20px'><div style='position:absolute;width:40px;height:40px;background:#f00'></div></div>
             <div style='height:40px;background:#00f'></div>"
        ),
        60,
        80,
    );
    assert_eq!(px(&fb, 10, 30), (255, 0, 0));
    assert_eq!(px(&fb, 50, 30), (0, 0, 255));
}

#[test]
fn overflow_hidden_clips_children() {
    let (_, fb) = render(
        &format!("{BODY0}<div style='width:20px;height:20px;overflow:hidden'><div style='width:50px;height:50px;background:#f00'></div></div>"),
        60,
        60,
    );
    assert_eq!(px(&fb, 10, 10), (255, 0, 0));
    assert_eq!(px(&fb, 30, 30), (255, 255, 255));
}

#[test]
fn opacity_blends() {
    let (_, fb) = render(
        &format!("{BODY0}<div style='width:20px;height:20px;background:#000;opacity:0.5'></div>"),
        30,
        30,
    );
    let (r, _, _) = px(&fb, 10, 10);
    assert!((120..=135).contains(&r), "{r}");
}

#[test]
fn region_raster_offsets_content() {
    let (list, _) = render(
        &format!("{BODY0}<div style='height:500px'></div><div style='height:10px;background:#f00'></div>"),
        50,
        100,
    );
    let fb = rasterize_region(&list, 0.0, 480.0, 50, 50);
    assert_eq!(px(&fb, 5, 25), (255, 0, 0));
    assert_eq!(px(&fb, 5, 5), (255, 255, 255));
}

#[test]
fn rounded_corners_are_transparent() {
    let (_, fb) = render(
        &format!(
            "{BODY0}<div style='width:40px;height:40px;background:#000;border-radius:20px'></div>"
        ),
        50,
        50,
    );
    assert_eq!(px(&fb, 0, 0), (255, 255, 255));
    assert_eq!(px(&fb, 20, 20), (0, 0, 0));
}
