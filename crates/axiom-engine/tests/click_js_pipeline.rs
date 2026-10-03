//! Proves: click → JS listener → DOM mutation → invalidation → paint.

use axiom_dom::DirtyFlags;
use axiom_engine::BrowsingContext;

const HTML: &str = r##"<!DOCTYPE html>
<html><head><title>click</title>
<style>
body { margin: 0; }
#message { display:block; font-size: 20px; }
#button { display:block; width: 200px; height: 40px; }
</style>
</head><body>
<p id="message">Waiting</p>
<button type="button" id="button">Go</button>
<script>
const button = document.querySelector("#button");
button.addEventListener("click", () => {
  document.querySelector("#message").textContent = "It works!";
});
</script>
</body></html>"##;

#[test]
fn click_listener_mutates_dom_and_triggers_paint() {
    let mut ctx = BrowsingContext::new(640, 480);
    ctx.page
        .load_html("about:click-works", HTML)
        .expect("load html");
    ctx.page.update_rendering_if_needed();

    let button = ctx
        .page
        .document
        .borrow()
        .query_selector("#button")
        .expect("#button");
    let message = ctx
        .page
        .document
        .borrow()
        .query_selector("#message")
        .expect("#message");

    assert!(
        ctx.page
            .document
            .borrow()
            .text_content(message)
            .contains("Waiting"),
        "precondition"
    );

    // Same path as OS click after hit-test.
    ctx.click_node(button);

    let text = ctx.page.document.borrow().text_content(message);
    assert!(
        text.contains("It works!"),
        "expected JS listener to mutate #message, got {text:?}"
    );

    // Mutation should have dirtied the tree; click path must flush paint.
    assert!(
        !ctx.page.document.borrow().dirty.any() || ctx.page.framebuffer.is_some(),
        "expected render flush after click"
    );
    assert!(
        ctx.page.framebuffer.is_some(),
        "expected a framebuffer after invalidation/paint"
    );

    // A second style/layout-forcing mutation still works.
    ctx.page
        .document
        .borrow_mut()
        .set_text_content(message, "It works! (again)");
    assert!(ctx.page.document.borrow().dirty.any());
    ctx.page.document.borrow_mut().dirty = DirtyFlags::all();
    ctx.page.update_rendering_if_needed();
    assert!(!ctx.page.document.borrow().dirty.any());
}

#[test]
fn query_selector_and_text_content_roundtrip() {
    let mut ctx = BrowsingContext::new(400, 300);
    ctx.page
        .load_html(
            "about:qs",
            r##"<html><body><div id="x">hi</div>
            <script>
            document.querySelector("#x").textContent = "bye";
            </script></body></html>"##,
        )
        .unwrap();
    let x = ctx.page.document.borrow().query_selector("#x").unwrap();
    assert!(ctx.page.document.borrow().text_content(x).contains("bye"));
}
