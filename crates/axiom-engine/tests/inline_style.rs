//! element.style (CSSStyleDeclaration over the style attribute) and its effect on paint.

use axiom_engine::BrowsingContext;

fn page(html: &str) -> BrowsingContext {
    let mut ctx = BrowsingContext::new(200, 100);
    ctx.set_chrome_height(0);
    ctx.load_local_html("about:style", html, false)
        .expect("load html");
    ctx
}

fn eval(ctx: &mut BrowsingContext, src: &str) -> String {
    let js = ctx.page.js.as_mut().expect("page has js");
    js.eval(src)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

fn pixel(ctx: &BrowsingContext, x: u32, y: u32) -> u32 {
    let fb = ctx.framebuffer().expect("framebuffer");
    fb.pixels[(y * fb.width + x) as usize]
}

#[test]
fn style_reads_and_writes_the_style_attribute() {
    let mut ctx = page("<div id=d style='color: red; margin-top: 4px !important'></div>");
    assert_eq!(eval(&mut ctx, "var s = d.style; s.color"), "red");
    assert_eq!(eval(&mut ctx, "s.length + ' ' + s.item(1)"), "2 margin-top");
    assert_eq!(
        eval(&mut ctx, "s.getPropertyPriority('margin-top')"),
        "important"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "s === d.style && s instanceof CSSStyleDeclaration"
        ),
        "true"
    );

    eval(
        &mut ctx,
        "s.backgroundColor = 'blue'; s['font-size'] = '12px'; s.color = ''",
    );
    assert_eq!(
        eval(&mut ctx, "d.getAttribute('style')"),
        "margin-top: 4px !important; background-color: blue; font-size: 12px;"
    );
    assert_eq!(eval(&mut ctx, "s.removeProperty('font-size')"), "12px");
    assert_eq!(eval(&mut ctx, "s.cssFloat = 'left'; s.float"), "left");
    assert_eq!(
        eval(
            &mut ctx,
            "s.setProperty('--gap', ' 3px '); s.getPropertyValue('--gap')"
        ),
        "3px"
    );
    // Unbalanced values and unknown priorities are ignored.
    assert_eq!(eval(&mut ctx, "s.width = 'calc(1px'; s.width"), "");
    assert_eq!(
        eval(&mut ctx, "s.setProperty('width', '1px', 'loud'); s.width"),
        ""
    );

    eval(&mut ctx, "d.style = 'width: 5px'");
    assert_eq!(eval(&mut ctx, "d.getAttribute('style')"), "width: 5px;");
    assert_eq!(
        eval(&mut ctx, "d.setAttribute('style', 'height:2px'); s.cssText"),
        "height: 2px;"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "s.webkitTransform = 'none'; s.getPropertyValue('-webkit-transform')"
        ),
        "none"
    );
}

#[test]
fn style_without_attribute_does_not_create_one() {
    let mut ctx = page("<p id=p>");
    assert_eq!(
        eval(
            &mut ctx,
            "p.style.removeProperty('color'); p.hasAttribute('style')"
        ),
        "false"
    );
    assert_eq!(eval(&mut ctx, "p.style.cssText"), "");
}

#[test]
fn script_style_changes_repaint() {
    let mut ctx = page(
        "<style>body { margin: 0 } #box { width: 50px; height: 50px; background: red }</style>\
         <div id=box></div>",
    );
    ctx.tick();
    assert_eq!(pixel(&ctx, 25, 25), 0xff_00_00_ff);
    eval(
        &mut ctx,
        "box.style.backgroundColor = 'rgb(0, 128, 0)'; box.style.width = '100px'",
    );
    ctx.tick();
    assert_eq!(pixel(&ctx, 25, 25), 0xff_00_80_00);
    assert_eq!(pixel(&ctx, 75, 25), 0xff_00_80_00);
    assert_eq!(pixel(&ctx, 150, 25), 0xff_ff_ff_ff);
}
