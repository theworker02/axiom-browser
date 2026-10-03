//! SVG documents as `<img>` sources, rendered end to end from the fixture in
//! `tests/rendering/svg-image.html` (served by `TestServer::spawn_site`).

use std::time::Duration;

use axiom_browser::Browser;
use axiom_engine::BrowsingContext;
use axiom_net::test_server::TestServer;
use tempfile::tempdir;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/rendering");

fn ctx(b: &mut Browser) -> &mut BrowsingContext {
    &mut b.window.tabs.active_tab_mut().context
}

#[test]
fn svg_images_decode_size_and_paint_without_fetching_external_references() {
    let srv = TestServer::spawn_site(FIXTURES);
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 200, 200).unwrap();
    ctx(&mut b).set_chrome_height(0);
    b.navigate_resolved(&srv.url("/svg-image.html"));
    assert!(
        ctx(&mut b).run_until_idle(Duration::from_secs(15)),
        "fixture never went idle"
    );
    ctx(&mut b).tick();

    let fb = ctx(&mut b).framebuffer().expect("framebuffer").clone();
    // Pixels are 0xAABBGGRR.
    let at = |x: u32, y: u32| fb.pixels[(y * fb.width + x) as usize];
    assert_eq!(at(25, 20), 0xff_00_00_ff, "declared.svg, left half");
    assert_eq!(at(75, 20), 0xff_ff_00_00, "declared.svg, right half");
    assert_eq!(at(105, 20), 0xff_ff_ff_ff, "declared.svg is 100px wide");
    assert_eq!(at(25, 65), 0xff_00_80_00, "viewbox.svg scaled to 50px");
    assert_eq!(at(55, 65), 0xff_ff_ff_ff, "viewbox.svg keeps its 1:1 ratio");
    assert_eq!(at(20, 100), 0xff_00_ff_ff, "data: SVG at 20px tall");
    assert_eq!(at(45, 100), 0xff_ff_ff_ff, "data: SVG keeps its 2:1 ratio");
    assert_eq!(at(15, 115), 0xff_ff_00_ff, "external-ref.svg still renders");
    assert!(
        srv.requests_for("/leak.png").is_empty()
            && srv.requests_for("/svg-image/leak.png").is_empty(),
        "an SVG image must not fetch the resources it references"
    );
}
