//! Render one vendored WPT page through the harness server and write a PPM:
//!
//! `cargo run -p axiom-compat --example wpt_render -- css/CSS2/colors/color-000.xht out.ppm`

use std::time::Duration;

use axiom_compat::wpt::{self, Harness, VIEWPORT};
use axiom_engine::BrowsingContext;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(rel), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("usage: wpt_render <wpt-relative-path> <out.ppm>");
        std::process::exit(2);
    };
    let root = axiom_compat::workspace_root();
    let harness = Harness::new(&root);
    let url = harness.server.url(rel);
    let mut ctx = BrowsingContext::headless(VIEWPORT.0, VIEWPORT.1, harness.scheduler.clone());
    let loaded = wpt::load(&mut ctx, &url, Duration::from_secs(10));
    println!(
        "{url}\n{loaded:#?}\nrender_blocked: {}",
        ctx.page.render_blocked
    );
    if let Some(d) = ctx.document_diagnostics() {
        println!(
            "blocking stylesheets: {:?}, parsing: {}",
            d.blocking_stylesheets, d.parsing_state
        );
    }
    for r in ctx.resource_diagnostics() {
        println!("  {} {} {}", r.state, r.kind, r.url);
    }
    let Some(fb) = ctx.framebuffer() else {
        eprintln!("no frame");
        std::process::exit(1);
    };
    let distinct: std::collections::HashSet<u32> = fb.pixels.iter().copied().collect();
    println!(
        "frame {}x{}, {} distinct colors",
        fb.width,
        fb.height,
        distinct.len()
    );
    std::fs::write(out, fb.to_ppm()).expect("write ppm");
    harness.report_not_found();
}
