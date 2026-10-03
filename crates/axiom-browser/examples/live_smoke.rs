//! Manual live-web smoke probe (Wave F). Needs internet access; never part of CI.
//!
//! ```text
//! cargo run --release -p axiom-browser --example live_smoke -- [--png DIR] [--provider ID] [--eval EXPR]... [PROBE|URL|QUERY]...
//! ```
//!
//! Each target is typed into the omnibox of a private window, exactly as a user would, so
//! URL-vs-search classification and the selected search provider are part of the run.
//! Without targets a fixed set of probes is used (static page, search engine home and
//! query, docs, Wikipedia, GitHub, CSS-heavy, JS-heavy, forms).
//!
//! Stages are reported separately. The ratings are coarse heuristics that detect
//! breakage (a failed request, an empty DOM, a blank frame, script errors), not visual
//! correctness: a PASS in RENDER means pixels were painted, not that the page matches
//! other browsers. `--png DIR` writes each frame so the rendering can be judged by eye.
//! `--eval EXPR` (repeatable) evaluates a diagnostic expression in each settled page and
//! prints the result. The `forms` probe then types a query into the page's search field
//! and presses Enter (SUBMIT).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axiom_browser::Browser;
use axiom_engine::{forms, SecurityState};

const PROBES: &[(&str, &str)] = &[
    ("static", "https://example.com/"),
    ("search-home", "https://www.google.com/"),
    ("search-query", "rust programming language"),
    ("docs", "https://doc.rust-lang.org/book/"),
    (
        "wikipedia",
        "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    ),
    ("github", "https://github.com/rust-lang/rust"),
    (
        "css-heavy",
        "https://developer.mozilla.org/en-US/docs/Web/CSS",
    ),
    ("js-heavy", "https://react.dev/"),
    ("forms", "https://html.duckduckgo.com/html/"),
];

const SETTLE: Duration = Duration::from_secs(20);

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let mut png_dir: Option<PathBuf> = None;
    let mut provider = "google".to_string();
    let mut targets: Vec<(String, String)> = Vec::new();
    let mut evals: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--png" => png_dir = args.next().map(PathBuf::from),
            "--provider" => provider = args.next().unwrap_or(provider),
            "--eval" => evals.extend(args.next()),
            _ => match PROBES.iter().find(|(name, _)| *name == a) {
                Some((name, input)) => targets.push((name.to_string(), input.to_string())),
                None => targets.push((a.clone(), a)),
            },
        }
    }
    if targets.is_empty() {
        targets = PROBES
            .iter()
            .map(|(n, i)| (n.to_string(), i.to_string()))
            .collect();
    }
    if let Some(dir) = &png_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("cannot create {}: {e}", dir.display());
            std::process::exit(2);
        }
    }

    let mut browser = match Browser::new_private(1280, 900) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot open a private profile: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = browser.set_search_provider(&provider) {
        eprintln!("{e}");
        std::process::exit(2);
    }
    println!(
        "Axiom live smoke — search provider: {} — {} targets\n",
        browser.search.default_provider().name,
        targets.len()
    );
    for (name, input) in &targets {
        probe(&mut browser, name, input, png_dir.as_ref(), &evals);
    }
}

fn probe(b: &mut Browser, name: &str, input: &str, png_dir: Option<&PathBuf>, evals: &[String]) {
    println!("== {name}: {input}");
    let started = Instant::now();
    b.navigate(input);
    let settled = b
        .window
        .tabs
        .active_tab_mut()
        .context
        .run_until_idle(SETTLE);
    b.tick();
    let elapsed = started.elapsed();
    let tab = b.window.tabs.active_tab();
    let ctx = &tab.context;

    // NETWORK
    let final_url = tab.url();
    let entry = b
        .network()
        .recent_log(500)
        .into_iter()
        .rev()
        .find(|e| e.resource_type == "document");
    let status = entry.as_ref().and_then(|e| e.status);
    let tls = match &ctx.security {
        SecurityState::Secure(t) => format!("TLS verified ({:?})", t.alpn),
        SecurityState::Insecure => "plain HTTP".into(),
        SecurityState::CertificateError(k) => format!("certificate error {k:?}"),
        _ => "not network".into(),
    };
    let net_ok = ctx.last_error.is_none() && status.is_some_and(|s| s < 400);
    line(
        "NETWORK",
        rate(net_ok, false),
        &format!(
            "status {} → {} · {tls} · {} ms{}",
            status.map_or("none".into(), |s| s.to_string()),
            strip_query(&final_url),
            elapsed.as_millis(),
            if settled { "" } else { " (did not settle)" }
        ),
    );
    if let Some(e) = &ctx.last_error {
        line("", "", &format!("error: {e}"));
    }

    // DOCUMENT PARSE
    let doc = ctx.page.document.borrow();
    let nodes = doc.len();
    let title = ctx.page.title.trim().to_string();
    let has_body = doc.body().is_some();
    line(
        "DOCUMENT",
        rate(has_body && nodes > 20, has_body),
        &format!("{nodes} nodes · title {title:?}"),
    );

    // Resources by kind.
    let resources = ctx.resource_diagnostics();
    let count = |kind: &str| {
        let all: Vec<_> = resources.iter().filter(|r| r.kind == kind).collect();
        let failed = all.iter().filter(|r| r.failure.is_some()).count();
        (all.len(), failed)
    };
    let (css, css_failed) = count("stylesheet");
    let inline_styles = doc.get_elements_by_tag_name("style").len();
    line(
        "CSS",
        rate(css_failed == 0, css_failed < css),
        &format!("{css} external ({css_failed} failed) · {inline_styles} inline <style>"),
    );
    let (images, images_failed) = count("image");
    line(
        "IMAGES",
        rate(images_failed == 0, images_failed < images),
        &format!("{images} requested ({images_failed} failed)"),
    );

    // RENDER
    let (painted, total) = match tab.framebuffer() {
        Some(fb) => (
            fb.pixels
                .iter()
                .filter(|p| (**p & 0x00FF_FFFF) != 0x00FF_FFFF)
                .count(),
            fb.pixels.len().max(1),
        ),
        None => (0, 1),
    };
    let share = painted as f64 * 100.0 / total as f64;
    line(
        "RENDER",
        rate(share > 2.0, share > 0.0),
        &format!("{share:.1}% of the viewport painted non-white"),
    );

    // SCRIPT
    let (scripts, scripts_failed) = count("script");
    let inline_scripts = doc
        .get_elements_by_tag_name("script")
        .into_iter()
        .filter(|s| doc.attr(*s, "src").is_none())
        .count();
    let errors = ctx
        .document_diagnostics()
        .map(|d| d.script_errors)
        .unwrap_or(0);
    let total_scripts = scripts + inline_scripts;
    line(
        "SCRIPT",
        rate(
            errors == 0 && scripts_failed == 0,
            errors < total_scripts,
        ),
        &format!(
            "{scripts} external ({scripts_failed} failed) · {inline_scripts} inline · {errors} script errors"
        ),
    );

    // INTERACTION (inventory; the forms probe also submits its search form)
    let links = doc
        .get_elements_by_tag_name("a")
        .into_iter()
        .filter(|a| doc.attr(*a, "href").is_some())
        .count();
    let forms = doc.get_elements_by_tag_name("form").len();
    let inputs = doc.get_elements_by_tag_name("input").len();
    line(
        "INTERACT",
        "MANUAL",
        &format!("{links} links · {forms} forms · {inputs} inputs"),
    );
    drop(doc);
    save_frame(b, name, png_dir);

    if let Some(js) = b.window.tabs.active_tab_mut().context.page.js.as_mut() {
        for expr in evals {
            let result = js
                .eval(expr)
                .map(|v| v.display)
                .unwrap_or_else(|e| format!("<{e}>"));
            line("EVAL", "", &format!("{expr}\n{result}"));
        }
    }

    if name == "forms" {
        submit_form(b, png_dir);
    }
    println!();
}

/// Type a query into the page's first form text field and press Enter, as a user would,
/// then report the submission's document request.
fn submit_form(b: &mut Browser, png_dir: Option<&PathBuf>) {
    let field = {
        let doc = b.window.tabs.active_tab().context.page.document.borrow();
        doc.get_elements_by_tag_name("input")
            .into_iter()
            .find(|&n| forms::is_text_control(&doc, n) && forms::form_owner(&doc, n).is_some())
    };
    let Some(field) = field else {
        line("SUBMIT", "FAIL", "no text field inside a form");
        return;
    };
    b.click_node(field);
    let ctx = &mut b.window.tabs.active_tab_mut().context;
    for ch in "axiom browser engine".chars() {
        ctx.handle_key(&ch.to_string());
    }
    let started = Instant::now();
    ctx.handle_key("Enter");
    let settled = ctx.run_until_idle(SETTLE);
    b.tick();
    let entry = b
        .network()
        .recent_log(500)
        .into_iter()
        .rev()
        .find(|e| e.resource_type == "document");
    let tab = b.window.tabs.active_tab();
    let (method, status) = entry
        .as_ref()
        .map_or(("none".to_string(), None), |e| (e.method.clone(), e.status));
    let results = {
        let doc = tab.context.page.document.borrow();
        doc.get_elements_by_tag_name("a").len()
    };
    line(
        "SUBMIT",
        rate(
            status.is_some_and(|s| s < 400) && tab.context.last_error.is_none(),
            status.is_some(),
        ),
        &format!(
            "{method} {} → {} · {results} links · title {:?} · {} ms{}",
            status.map_or("none".into(), |s| s.to_string()),
            strip_query(&tab.url()),
            tab.context.page.title.trim(),
            started.elapsed().as_millis(),
            if settled { "" } else { " (did not settle)" }
        ),
    );
    save_frame(b, "forms-submitted", png_dir);
}

fn save_frame(b: &Browser, name: &str, png_dir: Option<&PathBuf>) {
    let tab = b.window.tabs.active_tab();
    if let (Some(dir), Some(fb)) = (png_dir, tab.framebuffer()) {
        let path = dir.join(format!("{name}.png"));
        let mut img = image::RgbImage::new(fb.width, fb.height);
        // Framebuffer pixels are 0xAABBGGRR.
        for (i, px) in fb.pixels.iter().enumerate() {
            let (x, y) = (i as u32 % fb.width, i as u32 / fb.width);
            img.put_pixel(
                x,
                y,
                image::Rgb([*px as u8, (px >> 8) as u8, (px >> 16) as u8]),
            );
        }
        match img.save(&path) {
            Ok(()) => line("", "", &format!("frame: {}", path.display())),
            Err(e) => line("", "", &format!("frame not saved: {e}")),
        }
    }
}

fn rate(pass: bool, partial: bool) -> &'static str {
    if pass {
        "PASS"
    } else if partial {
        "PARTIAL"
    } else {
        "FAIL"
    }
}

fn line(stage: &str, rating: &str, detail: &str) {
    println!("  {stage:<9} {rating:<8} {detail}");
}

fn strip_query(url: &str) -> String {
    match url.find(['?', '#']) {
        Some(i) => format!("{}?…", &url[..i]),
        None => url.to_string(),
    }
}
