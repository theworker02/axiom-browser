use super::*;
use axiom_style::StyleEngine;

fn layout(html: &str, width: f32) -> (Document, LayoutTree) {
    let doc = axiom_html::parse_html(html).expect("parse");
    let mut engine = StyleEngine::with_viewport(width, 600.0);
    engine.add_author_css(&doc.collect_style_text());
    let styles = engine.compute_document(&doc);
    let tree = layout_document(&doc, &styles, &LayoutInputs::empty(), width, 600.0);
    (doc, tree)
}

fn rect(doc: &Document, tree: &LayoutTree, id: &str) -> Rect {
    let node = doc.get_element_by_id(id).expect("element");
    tree.node_rect(node).expect("fragment")
}

fn texts(f: &Fragment, out: &mut Vec<(String, Rect)>) {
    if let FragmentKind::Text(t) = &f.kind {
        out.push((t.text.clone(), f.rect));
    }
    for c in &f.children {
        texts(c, out);
    }
}

fn all_texts(tree: &LayoutTree) -> Vec<(String, Rect)> {
    let mut v = Vec::new();
    texts(&tree.root, &mut v);
    v
}

#[test]
fn body_margin_and_block_widths() {
    let (doc, tree) = layout("<body><div id=a style='height:10px'></div></body>", 800.0);
    let a = rect(&doc, &tree, "a");
    assert_eq!((a.x, a.y, a.width, a.height), (8.0, 8.0, 784.0, 10.0));
}

#[test]
fn sibling_and_parent_margins_collapse() {
    let (doc, tree) = layout(
        "<style>body{margin:0} div{height:10px}</style>
         <section style='margin-top:30px'><div id=a style='margin:20px 0'></div></section>
         <div id=b style='margin-top:5px'></div>",
        800.0,
    );
    let a = rect(&doc, &tree, "a");
    let b = rect(&doc, &tree, "b");
    assert_eq!(a.y, 30.0, "child margin collapses through the parent");
    assert_eq!(b.y, a.bottom() + 20.0, "larger of adjacent margins wins");
}

#[test]
fn auto_margins_center() {
    let (doc, tree) = layout(
        "<style>body{margin:0}</style><div id=a style='width:200px;margin:0 auto;height:1px'></div>",
        800.0,
    );
    assert_eq!(rect(&doc, &tree, "a").x, 300.0);
}

#[test]
fn center_and_align_attributes_center_block_children() {
    let (doc, tree) = layout(
        "<style>body{margin:0} i{display:block;width:100px;height:1px}</style>\
         <center><i id=a></i><div><i id=nested></i></div><i id=auto style='margin-left:auto'></i></center>\
         <div align=right><i id=r></i></div><div style='text-align:center'><i id=plain></i></div>",
        800.0,
    );
    assert_eq!(rect(&doc, &tree, "a").x, 350.0);
    assert_eq!(
        rect(&doc, &tree, "nested").x,
        350.0,
        "-webkit-center inherits"
    );
    assert_eq!(rect(&doc, &tree, "auto").x, 700.0, "auto margins still win");
    assert_eq!(rect(&doc, &tree, "r").x, 700.0);
    assert_eq!(
        rect(&doc, &tree, "plain").x,
        0.0,
        "plain `center` moves only inline content"
    );
}

#[test]
fn percentage_columns_widen_a_shrink_to_fit_table() {
    // The unpercented column holds its 200px maximum in the half left by two 25% columns.
    let (doc, tree) = layout(
        "<style>body{margin:0} td{padding:0}</style>\
         <table id=t cellspacing=0 cellpadding=0><tr><td id=a width=25%></td>\
         <td id=b><div style='width:200px'></div></td><td id=c width=25%></td></tr></table>",
        800.0,
    );
    assert_eq!(rect(&doc, &tree, "t").width, 400.0);
    assert_eq!(rect(&doc, &tree, "a").width, 100.0);
    assert_eq!(rect(&doc, &tree, "b").width, 200.0);
    assert_eq!(rect(&doc, &tree, "c").x, 300.0);
    let (doc, tree) = layout(
        "<style>body{margin:0} td{padding:0}</style>\
         <table id=t width=600 cellspacing=0><tr><td id=a width=50%>x</td><td id=b>y</td></tr></table>",
        800.0,
    );
    assert_eq!(rect(&doc, &tree, "a").width, 300.0);
    assert_eq!(rect(&doc, &tree, "b").width, 300.0);
}

#[test]
fn spaces_between_inline_elements_are_kept() {
    let (_, tree) = layout(
        "<p>Hello <b>bold</b> <i>world</i> and <a href=#>more</a>.</p>",
        800.0,
    );
    let t = all_texts(&tree);
    // Whitespace-only runs are not emitted, so rebuild spaces from horizontal gaps.
    let mut joined = String::new();
    let mut last_right = None;
    for (s, r) in &t {
        if last_right.is_some_and(|right: f32| r.x - right > 1.0) {
            joined.push(' ');
        }
        joined.push_str(s);
        last_right = Some(r.right());
    }
    assert_eq!(
        joined.split_whitespace().collect::<Vec<_>>(),
        ["Hello", "bold", "world", "and", "more."]
    );
    // Runs sit on one line, left to right, without overlapping.
    for w in t.windows(2) {
        assert!((w[0].1.y - w[1].1.y).abs() < 0.5, "{t:?}");
        assert!(w[1].1.x >= w[0].1.right() - 0.5, "{t:?}");
    }
}

#[test]
fn inline_code_does_not_become_block() {
    let (doc, tree) = layout("<p id=p>Use <code id=c>x()</code> here</p>", 800.0);
    let p = rect(&doc, &tree, "p");
    let c = rect(&doc, &tree, "c");
    assert!(c.width < 100.0, "code box is inline: {c:?}");
    assert!(c.x > p.x);
}

#[test]
fn text_wraps_within_width() {
    let words = "lorem ipsum dolor sit amet ".repeat(20);
    let (doc, tree) = layout(
        &format!("<div id=d style='width:200px'>{words}</div>"),
        800.0,
    );
    let d = rect(&doc, &tree, "d");
    let t = all_texts(&tree);
    let lines: std::collections::BTreeSet<i32> = t.iter().map(|(_, r)| r.y as i32).collect();
    assert!(lines.len() > 5, "wrapped into several lines");
    for (s, r) in &t {
        assert!(
            r.right() <= d.right() + 1.0 || s.trim().is_empty(),
            "{s:?} overflows"
        );
    }
}

#[test]
fn pieces_of_one_text_join_into_one_fragment() {
    let (_, tree) = layout("<p>fetched-by-script and more words</p>", 800.0);
    let t = all_texts(&tree);
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!(t[0].0, "fetched-by-script and more words");
}

#[test]
fn text_align_center() {
    let (doc, tree) = layout(
        "<div id=d style='text-align:center;width:400px'>hi</div>",
        800.0,
    );
    let d = rect(&doc, &tree, "d");
    let (_, r) = &all_texts(&tree)[0];
    let mid = r.x + r.width / 2.0;
    assert!((mid - (d.x + 200.0)).abs() < 2.0, "{r:?}");
}

#[test]
fn absolute_positioning_against_positioned_ancestor() {
    let (doc, tree) = layout(
        "<style>body{margin:0}</style>
         <div style='position:relative;margin-left:50px;margin-top:40px;width:300px;height:100px'>
           <span id=a style='position:absolute;right:10px;bottom:5px;width:20px;height:20px'></span>
         </div>",
        800.0,
    );
    let a = rect(&doc, &tree, "a");
    assert_eq!(
        (a.x, a.y),
        (50.0 + 300.0 - 10.0 - 20.0, 40.0 + 100.0 - 5.0 - 20.0)
    );
}

#[test]
fn inline_block_and_padding() {
    let (doc, tree) = layout(
        "<style>body{margin:0}</style><div><span id=a style='display:inline-block;width:50px;height:30px;padding:5px;border:2px solid'></span></div>",
        800.0,
    );
    let a = rect(&doc, &tree, "a");
    assert_eq!((a.width, a.height), (64.0, 44.0));
}

#[test]
fn list_items_get_markers() {
    let (_, tree) = layout(
        "<ul><li>one</li><li>two</li></ul><ol><li>x</li></ol>",
        800.0,
    );
    fn count_shapes(f: &Fragment) -> usize {
        usize::from(matches!(f.kind, FragmentKind::Shape(..)))
            + f.children.iter().map(count_shapes).sum::<usize>()
    }
    assert_eq!(count_shapes(&tree.root), 2);
    assert!(all_texts(&tree).iter().any(|(s, _)| s.starts_with('1')));
}

#[test]
fn hit_test_returns_element_for_text() {
    let (doc, tree) = layout(
        "<style>body{margin:0}</style><p style='margin:0'><a id=l href=#>link</a></p>",
        800.0,
    );
    let l = rect(&doc, &tree, "l");
    let hit = tree.hit_test(l.x + 2.0, l.y + l.height / 2.0);
    assert_eq!(hit, doc.get_element_by_id("l"));
}

#[test]
fn display_none_and_canvas_background() {
    let (doc, tree) = layout(
        "<body style='background:#123456'><div id=a style='display:none'>x</div><p>y</p></body>",
        800.0,
    );
    assert!(tree
        .node_rect(doc.get_element_by_id("a").unwrap())
        .is_none());
    assert_eq!(tree.canvas_background, Color::rgb(0x12, 0x34, 0x56));
}

#[test]
fn closed_details_render_only_their_summary() {
    let (_, tree) = layout(
        "<details><div style='display:contents'><summary>s</summary>hidden</div></details>
         <details><summary>first</summary>body</details>
         <details open><summary>open</summary>shown</details>",
        800.0,
    );
    let t: Vec<String> = all_texts(&tree).into_iter().map(|(s, _)| s).collect();
    assert_eq!(t, ["Details", "first", "open", "shown"], "{t:?}");
}

#[test]
fn floats_shorten_line_boxes_and_clear_moves_below() {
    let (doc, tree) = layout(
        "<style>body{margin:0;font-size:10px;line-height:10px}</style>
         <div style='width:300px'>
           <div id=f style='float:left;width:100px;height:50px'></div>
           <span id=t>text</span>
           <div id=c style='clear:left;height:5px'></div>
           <div id=r style='float:right;width:40px;height:10px'></div>
         </div>",
        800.0,
    );
    let f = rect(&doc, &tree, "f");
    assert_eq!((f.x, f.y), (0.0, 0.0));
    let t = rect(&doc, &tree, "t");
    assert!(
        t.x >= 100.0 && t.y < 10.0,
        "text flows beside the float: {t:?}"
    );
    assert_eq!(
        rect(&doc, &tree, "c").y,
        50.0,
        "clearance puts the box below the float"
    );
    let r = rect(&doc, &tree, "r");
    assert_eq!((r.x, r.y), (260.0, 55.0));
}

#[test]
fn block_formatting_context_contains_floats() {
    let (doc, tree) = layout(
        "<style>body{margin:0}</style>
         <div id=bfc style='overflow:hidden'><div style='float:left;width:10px;height:40px'></div></div>
         <div id=plain><div style='float:left;width:10px;height:40px'></div></div>",
        800.0,
    );
    assert_eq!(rect(&doc, &tree, "bfc").height, 40.0);
    assert_eq!(rect(&doc, &tree, "plain").height, 0.0);
}

#[test]
fn table_columns_rows_and_colspan() {
    let (doc, tree) = layout(
        "<style>body{margin:0} table{border-spacing:0} td{padding:0}</style>
         <table>
           <tr><td id=a style='width:50px;height:20px'></td><td id=b style='width:30px'></td></tr>
           <tr><td id=c colspan=2 style='height:10px'></td></tr>
         </table>",
        800.0,
    );
    let (a, b, c) = (
        rect(&doc, &tree, "a"),
        rect(&doc, &tree, "b"),
        rect(&doc, &tree, "c"),
    );
    assert_eq!((a.x, a.width, a.height), (0.0, 50.0, 20.0));
    assert_eq!(
        (b.x, b.width, b.height),
        (50.0, 30.0, 20.0),
        "cells stretch to the row"
    );
    assert_eq!(
        (c.x, c.y, c.width),
        (0.0, 20.0, 80.0),
        "colspan covers both columns"
    );
}

#[test]
fn content_height_covers_tall_content() {
    let (_, tree) = layout("<div style='height:2000px'></div>", 800.0);
    assert!(tree.content_height >= 2000.0);
}

fn xw(doc: &Document, tree: &LayoutTree, ids: &[&str]) -> Vec<(f32, f32)> {
    ids.iter()
        .map(|id| {
            let r = rect(doc, tree, id);
            (r.x, r.width)
        })
        .collect()
}

const FLEX_BASE: &str =
    "<style>body{margin:0} .f{display:flex;width:600px} .f>div{height:20px}</style>";

#[test]
fn flex_row_justify_content() {
    for (justify, want) in [
        ("flex-start", [0.0, 100.0, 200.0]),
        ("flex-end", [300.0, 400.0, 500.0]),
        ("center", [150.0, 250.0, 350.0]),
        ("space-between", [0.0, 250.0, 500.0]),
        ("space-around", [50.0, 250.0, 450.0]),
        ("space-evenly", [75.0, 250.0, 425.0]),
    ] {
        let html = format!(
            "{FLEX_BASE}<div class=f style='justify-content:{justify}'>
             <div id=a style='width:100px'></div><div id=b style='width:100px'></div>
             <div id=c style='width:100px'></div></div>"
        );
        let (doc, tree) = layout(&html, 800.0);
        let got: Vec<f32> = xw(&doc, &tree, &["a", "b", "c"])
            .iter()
            .map(|p| p.0)
            .collect();
        assert_eq!(got, want, "justify-content: {justify}");
    }
}

#[test]
fn flex_grow_shrink_and_basis() {
    let (doc, tree) = layout(
        &format!(
            "{FLEX_BASE}<div class=f><div id=a style='flex:1'></div><div id=b style='flex:2'></div></div>
             <div class=f><div id=c style='flex:0 0 100px'></div><div id=d style='flex:1 1 0'></div>
               <div id=e style='width:100px;flex-grow:1;max-width:150px'></div></div>
             <div class=f><div id=g style='flex:0 1 400px'></div><div id=h style='flex:0 3 400px'></div></div>"
        ),
        800.0,
    );
    assert_eq!(xw(&doc, &tree, &["a", "b"]), [(0.0, 200.0), (200.0, 400.0)]);
    // e hits its max-width and freezes; d takes the rest.
    assert_eq!(
        xw(&doc, &tree, &["c", "d", "e"]),
        [(0.0, 100.0), (100.0, 350.0), (450.0, 150.0)]
    );
    // 200px of overflow is shrunk in proportion to shrink factor × basis (1:3).
    assert_eq!(xw(&doc, &tree, &["g", "h"]), [(0.0, 350.0), (350.0, 250.0)]);
}

#[test]
fn flex_items_do_not_shrink_below_min_content() {
    let word = "W".repeat(40);
    let (doc, tree) = layout(
        &format!(
            "{FLEX_BASE}<div class=f style='width:100px'><div id=a>{word}</div><div id=b style='width:100px'></div></div>
             <div class=f style='width:100px'><div id=c style='min-width:0'>{word}</div><div id=d style='width:100px'></div></div>"
        ),
        800.0,
    );
    let (a, b) = (rect(&doc, &tree, "a"), rect(&doc, &tree, "b"));
    assert!(a.width > 100.0, "automatic minimum keeps the word: {a:?}");
    assert_eq!(b.x, a.right());
    let c = rect(&doc, &tree, "c");
    assert!(c.width < 100.0, "min-width: 0 lets it shrink: {c:?}");
}

#[test]
fn flex_cross_axis_alignment() {
    let (doc, tree) = layout(
        "<style>body{margin:0} .f{display:flex;width:600px}</style>
         <div class=f><div id=a style='width:50px;height:40px'></div><div id=b style='width:50px'></div></div>
         <div class=f style='align-items:center;height:100px'>
           <div id=c style='width:50px;height:40px'></div>
           <div id=d style='width:50px;height:20px;align-self:flex-end'></div>
           <div id=e style='width:50px;height:20px;margin:auto'></div></div>",
        800.0,
    );
    let b = rect(&doc, &tree, "b");
    assert_eq!(b.height, 40.0, "stretched to the line");
    let (c, d, e) = (
        rect(&doc, &tree, "c"),
        rect(&doc, &tree, "d"),
        rect(&doc, &tree, "e"),
    );
    assert_eq!(c.y, 40.0 + 30.0);
    assert_eq!(d.y, 40.0 + 80.0);
    assert_eq!(
        (e.x, e.y),
        (100.0 + 225.0, 40.0 + 40.0),
        "auto margins center"
    );
}

#[test]
fn flex_baseline_alignment() {
    let (doc, tree) = layout(
        "<style>body{margin:0}</style>
         <div style='display:flex;align-items:baseline'>
           <div id=a style='font-size:40px'>Big</div><div id=b style='font-size:10px'>small</div></div>",
        800.0,
    );
    let t = all_texts(&tree);
    let big = t.iter().find(|(s, _)| s.contains("Big")).unwrap().1;
    let small = t.iter().find(|(s, _)| s.contains("small")).unwrap().1;
    let a = rect(&doc, &tree, "a");
    assert!(rect(&doc, &tree, "b").y > a.y, "the small item moves down");
    // Glyph boxes differ in size, so compare baselines through the fonts' ascents.
    assert!(big.bottom() > small.bottom());
}

#[test]
fn flex_wrap_gap_and_align_content() {
    let (doc, tree) = layout(
        "<style>body{margin:0} .f{display:flex;flex-wrap:wrap;width:600px;gap:10px 20px}
         .f>div{width:250px;height:30px}</style>
         <div class=f id=f><div id=a></div><div id=b></div><div id=c></div></div>",
        800.0,
    );
    let (a, b, c) = (
        rect(&doc, &tree, "a"),
        rect(&doc, &tree, "b"),
        rect(&doc, &tree, "c"),
    );
    assert_eq!((a.x, a.y), (0.0, 0.0));
    assert_eq!((b.x, b.y), (270.0, 0.0));
    assert_eq!((c.x, c.y), (0.0, 40.0), "wraps below with the row gap");
    assert_eq!(rect(&doc, &tree, "f").height, 70.0);
}

#[test]
fn flex_direction_order_and_auto_margins() {
    let (doc, tree) = layout(
        &format!(
            "{FLEX_BASE}<div class=f style='flex-direction:row-reverse'>
               <div id=a style='width:100px'></div><div id=b style='width:100px'></div></div>
             <div class=f><div id=c style='width:100px;order:2'></div><div id=d style='width:100px'></div></div>
             <div class=f><div id=e style='width:100px'></div><div id=g style='width:100px;margin-left:auto'></div></div>"
        ),
        800.0,
    );
    assert_eq!(
        xw(&doc, &tree, &["a", "b"]),
        [(500.0, 100.0), (400.0, 100.0)]
    );
    assert_eq!(xw(&doc, &tree, &["c", "d"]), [(100.0, 100.0), (0.0, 100.0)]);
    assert_eq!(
        rect(&doc, &tree, "g").x,
        500.0,
        "margin-left: auto pushes right"
    );
}

#[test]
fn flex_column_fills_min_height() {
    let (doc, tree) = layout(
        "<style>body{margin:0}</style>
         <div id=page style='display:flex;flex-direction:column;min-height:300px'>
           <header id=h style='height:50px'></header>
           <main id=m style='flex:1'></main>
           <footer id=f style='height:30px'></footer></div>",
        800.0,
    );
    let (h, m, f) = (
        rect(&doc, &tree, "h"),
        rect(&doc, &tree, "m"),
        rect(&doc, &tree, "f"),
    );
    assert_eq!((h.y, h.width), (0.0, 800.0), "items stretch across");
    assert_eq!((m.y, m.height), (50.0, 220.0));
    assert_eq!(f.y, 270.0, "the footer sits at the bottom");
    assert_eq!(rect(&doc, &tree, "page").height, 300.0);
}

#[test]
fn flex_text_runs_become_anonymous_items_and_inline_flex_shrinks() {
    let (doc, tree) = layout(
        "<style>body{margin:0}</style>
         <div id=f style='display:flex'>Hello <span id=s>world</span></div>
         <span id=i style='display:inline-flex;border:1px solid'><span style='width:30px'></span><span style='width:20px'></span></span>",
        800.0,
    );
    let t = all_texts(&tree);
    let hello = t.iter().find(|(s, _)| s.contains("Hello")).unwrap().1;
    let s = rect(&doc, &tree, "s");
    // The fragment of "Hello " includes the trailing space, which hangs past the item.
    let space = axiom_text::measure(" ", &block::font_spec(&ComputedStyle::initial()));
    assert!(
        (s.x - (hello.right() - space)).abs() < 0.5 && s.y == hello.y,
        "the span is a separate item after the text: {hello:?} {s:?}"
    );
    assert_eq!(rect(&doc, &tree, "i").width, 52.0);
}

const GRID_BASE: &str =
    "<style>body{margin:0} .g{display:grid;width:700px} .g>div{height:20px}</style>";

#[test]
fn grid_fixed_and_fr_columns_with_gaps() {
    let (doc, tree) = layout(
        &format!(
            "{GRID_BASE}<div class=g style='grid-template-columns:100px 1fr 2fr;column-gap:30px;row-gap:5px'>
             <div id=a></div><div id=b></div><div id=c></div><div id=d></div></div>"
        ),
        800.0,
    );
    assert_eq!(
        xw(&doc, &tree, &["a", "b", "c"]),
        [(0.0, 100.0), (130.0, 180.0), (340.0, 360.0)]
    );
    let d = rect(&doc, &tree, "d");
    assert_eq!(
        (d.x, d.y),
        (0.0, 25.0),
        "auto-placement wraps to the next row"
    );
}

#[test]
fn grid_rows_fit_tallest_item_and_items_stretch() {
    let (doc, tree) = layout(
        "<style>body{margin:0}</style>
         <div id=g style='display:grid;grid-template-columns:1fr 1fr;width:400px'>
           <div id=a style='height:50px'></div><div id=b></div>
           <div id=c style='height:10px;align-self:end'></div></div>",
        800.0,
    );
    let b = rect(&doc, &tree, "b");
    assert_eq!((b.x, b.height), (200.0, 50.0));
    let c = rect(&doc, &tree, "c");
    assert_eq!((c.y, c.height), (50.0, 10.0));
    assert_eq!(rect(&doc, &tree, "g").height, 60.0);
}

#[test]
fn grid_template_areas_and_line_placement() {
    let (doc, tree) = layout(
        "<style>body{margin:0}
         .g{display:grid;width:600px;grid-template-columns:150px 1fr;grid-template-rows:40px 100px 30px;
            grid-template-areas:'head head' 'nav main' 'foot foot'}</style>
         <div class=g>
           <div id=main style='grid-area:main'></div><div id=head style='grid-area:head'></div>
           <div id=nav style='grid-area:nav'></div><div id=foot style='grid-area:foot'></div></div>
         <div style='display:grid;grid-template-columns:repeat(4,100px)'>
           <div id=s style='grid-column:2 / span 2'></div><div id=n style='grid-column:-2'></div>
           <div id=r style='grid-row:2;grid-column:1 / -1'></div></div>",
        800.0,
    );
    let r = |id| rect(&doc, &tree, id);
    assert_eq!(
        (r("head").x, r("head").y, r("head").width),
        (0.0, 0.0, 600.0)
    );
    assert_eq!(
        (r("nav").x, r("nav").y, r("nav").height),
        (0.0, 40.0, 100.0)
    );
    assert_eq!(
        (r("main").x, r("main").y, r("main").width),
        (150.0, 40.0, 450.0)
    );
    assert_eq!((r("foot").y, r("foot").width), (140.0, 600.0));
    assert_eq!((r("s").x, r("s").width), (100.0, 200.0));
    assert_eq!(r("n").x, 300.0, "negative lines count from the end");
    assert_eq!((r("r").x, r("r").width), (0.0, 400.0));
    assert_eq!(r("r").y, r("s").bottom());
}

#[test]
fn grid_auto_fill_and_auto_fit() {
    let (doc, tree) = layout(
        "<style>body{margin:0} .g>div{height:10px}</style>
         <div class=g style='display:grid;width:450px;grid-template-columns:repeat(auto-fill,minmax(100px,1fr))'>
           <div id=a></div><div id=b></div></div>
         <div class=g style='display:grid;width:450px;grid-template-columns:repeat(auto-fit,minmax(100px,1fr))'>
           <div id=c></div><div id=d></div></div>",
        800.0,
    );
    assert_eq!(
        xw(&doc, &tree, &["a", "b"]),
        [(0.0, 112.5), (112.5, 112.5)],
        "four columns, two left empty"
    );
    assert_eq!(
        xw(&doc, &tree, &["c", "d"]),
        [(0.0, 225.0), (225.0, 225.0)],
        "empty auto-fit tracks collapse"
    );
}

#[test]
fn grid_dense_packing_and_self_alignment() {
    let (doc, tree) = layout(
        "<style>body{margin:0} .g{display:grid;width:300px;grid-template-columns:repeat(3,100px);grid-auto-rows:40px}
         .g>div{min-height:0}</style>
         <div class=g style='grid-auto-flow:row dense'>
           <div id=a></div><div id=b style='grid-column:span 3'></div><div id=c></div></div>
         <div class=g style='justify-items:center'>
           <div id=d style='width:20px;height:10px;align-self:center'></div></div>",
        800.0,
    );
    let (a, b, c) = (
        rect(&doc, &tree, "a"),
        rect(&doc, &tree, "b"),
        rect(&doc, &tree, "c"),
    );
    assert_eq!((a.x, a.y), (0.0, 0.0));
    assert_eq!((b.y, b.width), (40.0, 300.0));
    assert_eq!((c.x, c.y), (100.0, 0.0), "dense fills the hole before b");
    let d = rect(&doc, &tree, "d");
    assert_eq!((d.x, d.y - 80.0), (40.0, 15.0));
}

#[test]
fn grid_intrinsic_columns_and_inline_grid() {
    let (doc, tree) = layout(
        "<style>body{margin:0}</style>
         <div style='display:grid;grid-template-columns:auto 1fr;width:500px'>
           <div id=a><span style='display:inline-block;width:80px'></span></div><div id=b></div></div>
         <span id=ig style='display:inline-grid;grid-template-columns:40px 60px'><span></span><span></span></span>",
        800.0,
    );
    assert_eq!(xw(&doc, &tree, &["a", "b"]), [(0.0, 80.0), (80.0, 420.0)]);
    assert_eq!(rect(&doc, &tree, "ig").width, 100.0);
}
