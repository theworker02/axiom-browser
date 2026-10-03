use super::*;

fn styled(html: &str) -> (Document, StyleMap) {
    let doc = axiom_html::parse_html(html).expect("parse");
    let mut engine = StyleEngine::with_viewport(1000.0, 800.0);
    engine.add_author_css(&doc.collect_style_text());
    let styles = engine.compute_document(&doc);
    (doc, styles)
}

fn by_id<'a>(doc: &Document, styles: &'a StyleMap, id: &str) -> &'a ComputedStyle {
    let node = doc.get_element_by_id(id).expect("element");
    styles.get(node).expect("style")
}

#[test]
fn specificity_order_and_importance() {
    let (doc, s) = styled(
        r#"<style>
        p { color: red }
        .a { color: green }
        #x { color: blue }
        p.a { color: black !important }
        #y { color: blue }
        .b { color: green !important }
        .c { color: green }
        :where(#z) { color: red }
        p { background-color: #eee }
        </style>
        <p id=x class=a>1</p><p id=y class=b style="color: red">2</p>
        <p id=z class=c>3</p><p id=w style="color: red !important" class=b>4</p>"#,
    );
    assert_eq!(by_id(&doc, &s, "x").color, Color::BLACK);
    assert_eq!(by_id(&doc, &s, "y").color, Color::rgb(0, 128, 0));
    assert_eq!(by_id(&doc, &s, "z").color, Color::rgb(0, 128, 0));
    assert_eq!(by_id(&doc, &s, "w").color, Color::rgb(255, 0, 0));
    assert_eq!(
        by_id(&doc, &s, "w").background_color,
        Color::rgb(0xee, 0xee, 0xee)
    );
}

#[test]
fn inheritance_and_relative_units() {
    let (doc, s) = styled(
        r#"<style>
        html { font-size: 20px }
        div { font-size: 1.5em; line-height: 1.5; padding: 1em 2rem; width: calc(100% - 1em) }
        span { font-size: 50% }
        </style><div id=d><span id=s>x</span></div>"#,
    );
    let d = by_id(&doc, &s, "d");
    assert_eq!(d.font_size, 30.0);
    assert_eq!(d.padding.top, Length::Px(30.0));
    assert_eq!(d.padding.left, Length::Px(40.0));
    assert_eq!(d.width.resolve(300.0), Some(270.0));
    let sp = by_id(&doc, &s, "s");
    assert_eq!(sp.font_size, 15.0);
    assert_eq!(sp.line_height, LineHeight::Number(1.5));
}

#[test]
fn custom_properties_and_var_fallbacks() {
    let (doc, s) = styled(
        r#"<style>
        :root { --main: #ff0000; --pad: 4px; --ref: var(--main) }
        .x { color: var(--ref); padding: var(--pad) var(--missing, 8px); margin: var(--nope) }
        .y { --main: #00ff00; color: var(--main) }
        </style><p id=a class=x>a</p><div class=y><p id=b class=x>b</p></div>"#,
    );
    let a = by_id(&doc, &s, "a");
    assert_eq!(a.color, Color::rgb(255, 0, 0));
    assert_eq!(a.padding.top, Length::Px(4.0));
    assert_eq!(a.padding.right, Length::Px(8.0));
    // Invalid at computed-value time: margin falls back to unset (the UA's margin is gone).
    assert_eq!(a.margin.top, Length::ZERO);
    let b = by_id(&doc, &s, "b");
    // `--ref` was substituted where it was declared (:root), so it stays red.
    assert_eq!(b.color, Color::rgb(255, 0, 0));
}

#[test]
fn media_and_supports_rules() {
    let (doc, s) = styled(
        r#"<style>
        @media (max-width: 600px) { #a { color: red } }
        @media (min-width: 600px) { #a { color: blue } }
        @supports (display: grid) { #b { display: grid } }
        @supports (-axiom-nope: 1) { #b { display: flex } }
        @layer base { #c { color: green } }
        </style><p id=a>a</p><p id=b>b</p><p id=c>c</p>"#,
    );
    assert_eq!(by_id(&doc, &s, "a").color, Color::rgb(0, 0, 255));
    assert_eq!(by_id(&doc, &s, "b").display, Display::Grid);
    assert_eq!(by_id(&doc, &s, "c").color, Color::rgb(0, 128, 0));
}

#[test]
fn shorthands_expand() {
    let (doc, s) = styled(
        r#"<style>
        #a { border: 2px dashed #00f; border-left: none; margin: 1px 2px 3px }
        #b { font: italic bold 12px/30px Georgia, "Times New Roman", serif }
        #c { flex: 1; display: flex; flex-flow: column wrap; gap: 4px 8px }
        #d { background: url(x.png) no-repeat #123456 }
        #e { list-style: square inside }
        #f { white-space: pre; text-decoration: underline red }
        </style>
        <div id=a></div><p id=b>b</p><div id=c></div><div id=d></div><ul id=e></ul><p id=f>f</p>"#,
    );
    let a = by_id(&doc, &s, "a");
    assert_eq!(a.border[0].used_width(), 2.0);
    assert_eq!(a.border[0].style, BorderStyle::Dashed);
    assert_eq!(a.border[0].color, Color::rgb(0, 0, 255));
    assert_eq!(a.border[3].used_width(), 0.0);
    assert_eq!(a.margin.left, Length::Px(2.0));
    assert_eq!(a.margin.bottom, Length::Px(3.0));
    let b = by_id(&doc, &s, "b");
    assert!(b.italic);
    assert_eq!(b.font_weight, 700);
    assert_eq!(b.font_size, 12.0);
    assert_eq!(b.line_height, LineHeight::Px(30.0));
    assert_eq!(&*b.font_family, r#"Georgia, "Times New Roman", serif"#);
    let c = by_id(&doc, &s, "c");
    assert_eq!(c.flex_grow, 1.0);
    assert_eq!(c.flex_basis, Length::Percent(0.0));
    assert_eq!(c.flex_direction, FlexDirection::Column);
    assert_eq!(c.flex_wrap, FlexWrap::Wrap);
    assert_eq!(c.row_gap, Length::Px(4.0));
    assert_eq!(c.column_gap, Length::Px(8.0));
    assert_eq!(
        by_id(&doc, &s, "d").background_color,
        Color::rgb(0x12, 0x34, 0x56)
    );
    let e = by_id(&doc, &s, "e");
    assert_eq!(e.list_style_type, ListStyleType::Square);
    assert!(e.list_style_inside);
    let f = by_id(&doc, &s, "f");
    assert!(f.preserves_spaces() && f.nowrap);
    assert_eq!(f.decorations_in_effect, UNDERLINE);
    assert_eq!(f.decoration_color, Color::rgb(255, 0, 0));
}

#[test]
fn user_agent_defaults_and_hints() {
    let (doc, s) = styled(
        r#"<body><h1 id=h>t</h1><ul><li id=li>x</li></ul><a id=a href=/>l</a>
        <table id=t width=300 align=center bgcolor=ccc><tr><td id=td>c</td></tr></table>
        <div hidden id=hid>h</div><img id=img width=10 height="20"><code id=code>c</code>
        <b id=b><b id=bb>x</b></b><font id=font color=red size=5>f</font></body>"#,
    );
    let h = by_id(&doc, &s, "h");
    assert_eq!(h.font_size, 32.0);
    assert_eq!(h.font_weight, 700);
    assert_eq!(by_id(&doc, &s, "li").display, Display::ListItem);
    let a = by_id(&doc, &s, "a");
    assert_eq!(a.color, Color::rgb(0, 0, 0xee));
    assert_eq!(a.decorations_in_effect, UNDERLINE);
    let t = by_id(&doc, &s, "t");
    assert_eq!(t.display, Display::Table);
    assert_eq!(t.width, Length::Px(300.0));
    assert_eq!(t.margin.left, Length::Auto);
    assert_eq!(t.background_color, Color::rgb(0xcc, 0xcc, 0xcc));
    assert_eq!(by_id(&doc, &s, "td").display, Display::TableCell);
    assert_eq!(by_id(&doc, &s, "hid").display, Display::None);
    let img = by_id(&doc, &s, "img");
    assert_eq!(
        (img.width.clone(), img.height.clone()),
        (Length::Px(10.0), Length::Px(20.0))
    );
    assert_eq!(&*by_id(&doc, &s, "code").font_family, "monospace");
    assert_eq!(by_id(&doc, &s, "bb").font_weight, 900);
    let font = by_id(&doc, &s, "font");
    assert_eq!(font.color, Color::rgb(255, 0, 0));
    assert_eq!(font.font_size, 24.0);
    // `display: none` subtrees get no styles; head is display: none.
    let title_parent = doc.head().unwrap();
    assert_eq!(s.get(title_parent).unwrap().display, Display::None);
}

#[test]
fn blockification_and_global_keywords() {
    let (doc, s) = styled(
        r#"<style>
        #f { display: flex } #f span { color: inherit }
        #abs { position: absolute; display: inline }
        #fl { float: left }
        #u { color: red } #u i { color: initial; font-weight: unset }
        </style>
        <div id=f><span id=item>x</span></div><span id=abs>a</span><span id=fl>b</span>
        <p id=u><b><i id=ui>c</i></b></p>"#,
    );
    assert_eq!(by_id(&doc, &s, "item").display, Display::Block);
    assert_eq!(by_id(&doc, &s, "abs").display, Display::Block);
    assert_eq!(by_id(&doc, &s, "fl").display, Display::Block);
    let ui = by_id(&doc, &s, "ui");
    assert_eq!(ui.color, Color::BLACK);
    assert_eq!(ui.font_weight, 700);
}

#[test]
fn text_nodes_share_parent_style() {
    let (doc, s) = styled("<p id=p style='color: #123'>hello</p>");
    let p = doc.get_element_by_id("p").unwrap();
    let text = doc.get(p).children[0];
    assert!(Arc::ptr_eq(s.get(p).unwrap(), s.get(text).unwrap()));
}
