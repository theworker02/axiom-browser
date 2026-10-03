//! Document-loading benchmarks: navigation → parse → DOMContentLoaded → load, cold and
//! warm, against the in-process test server.
//!
//! ```text
//! cargo run -p axiom-browser --example docbench --release
//! ```
//!
//! Prints one JSON object on stdout. Every number comes from the document's measured
//! timeline (or a wall clock around the navigation); loopback numbers measure Axiom's own
//! pipeline overhead, not real-world network performance.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axiom_browser::Browser;
use axiom_net::test_server::{png_1x1, TestResponse, TestServer};

const RESOURCES: usize = 12;
const COLD_RUNS: usize = 5;
const WARM_RUNS: usize = 10;

/// Half of each resource type is fresh for an hour, half revalidates with an ETag.
fn cache_headers(i: usize, resp: TestResponse) -> TestResponse {
    if i.is_multiple_of(2) {
        resp.with_header("Cache-Control", "max-age=3600")
    } else {
        resp.with_header("Cache-Control", "no-cache")
            .with_header("ETag", "\"b1\"")
    }
}

fn page() -> String {
    let mut html = String::from("<!DOCTYPE html><html><head><title>bench</title>");
    for i in 0..RESOURCES {
        html.push_str(&format!("<link rel=\"stylesheet\" href=\"/s{i}.css\">"));
    }
    for i in 0..RESOURCES {
        html.push_str(&format!("<script defer src=\"/j{i}.js\"></script>"));
    }
    html.push_str("</head><body>");
    for i in 0..200 {
        html.push_str(&format!(
            "<p class=\"c{}\">paragraph {i}</p>",
            i % RESOURCES
        ));
    }
    for i in 0..RESOURCES {
        html.push_str(&format!("<img src=\"/i{i}.png\">"));
    }
    html.push_str("</body></html>");
    html
}

fn large_page() -> String {
    let mut html = String::from("<!DOCTYPE html><html><head><title>large</title></head><body>");
    let mut i = 0;
    while html.len() < 2 * 1024 * 1024 {
        html.push_str(&format!(
            "<div class=\"row\"><span id=\"s{i}\">item {i}</span> <a href=\"/x{i}\">link</a></div>\n"
        ));
        i += 1;
    }
    html.push_str("</body></html>");
    html
}

fn server() -> TestServer {
    let page = page();
    let large = large_page();
    TestServer::spawn(move |req| {
        let path = req.path.as_str();
        let index = |p: &str| {
            p[2..p.find('.').unwrap_or(p.len())]
                .parse::<usize>()
                .unwrap_or(0)
        };
        if path == "/" || path == "/large" {
            let body = if path == "/" {
                page.clone()
            } else {
                large.clone()
            };
            return TestResponse::ok(body, "text/html; charset=utf-8")
                .with_header("Cache-Control", "no-cache")
                .with_header("ETag", "\"doc\"");
        }
        let i = index(path);
        let etag = i % 2 == 1 || path == "/" || path == "/large";
        if etag && req.header("if-none-match").is_some() {
            return TestResponse::status(304, Vec::new()).with_header("ETag", "\"b1\"");
        }
        let resp = if path.ends_with(".css") {
            TestResponse::ok(format!(".c{i} {{ color: rgb({i}, 0, 0); }}"), "text/css")
        } else if path.ends_with(".js") {
            TestResponse::ok(format!("var j{i} = {i};"), "text/javascript")
        } else if path.ends_with(".png") {
            TestResponse::ok(png_1x1(), "image/png")
        } else {
            return TestResponse::status(404, "not found");
        };
        cache_headers(i, resp)
    })
}

#[derive(Default)]
struct Sample {
    wall_ms: f64,
    milestones: HashMap<&'static str, f64>,
    cache: HashMap<&'static str, usize>,
    resources: usize,
}

fn measure(b: &mut Browser, url: &str, reload: bool) -> Sample {
    let t = Instant::now();
    if reload {
        b.window.tabs.active_tab_mut().reload();
    } else {
        b.navigate_resolved(url);
    }
    let ctx = &mut b.window.tabs.active_tab_mut().context;
    assert!(
        ctx.run_until_idle(Duration::from_secs(60)),
        "{url} never finished loading"
    );
    let wall_ms = t.elapsed().as_secs_f64() * 1000.0;
    let d = ctx.document_diagnostics().expect("document");
    assert!(d.load_fired);
    let mut cache = HashMap::new();
    let resources = ctx.resource_diagnostics();
    for r in &resources {
        *cache.entry(r.cache.unwrap_or("none")).or_insert(0) += 1;
    }
    Sample {
        wall_ms,
        milestones: d
            .timeline
            .iter()
            .filter_map(|(k, v)| v.map(|v| (*k, v)))
            .collect(),
        cache,
        resources: resources.len(),
    }
}

fn p50(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn summary(samples: &[Sample]) -> String {
    let metric = |name: &str| {
        let v: Vec<f64> = samples
            .iter()
            .filter_map(|s| s.milestones.get(name).copied())
            .collect();
        if v.len() == samples.len() {
            format!("{:.2}", p50(v))
        } else {
            "null".into()
        }
    };
    let last = samples.last().expect("samples");
    let mut cache: Vec<_> = last.cache.iter().collect();
    cache.sort();
    let cache = cache
        .iter()
        .map(|(k, v)| format!(r#""{k}":{v}"#))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"samples":{},"resources":{},"p50_wall_ms":{:.2},"p50_first_bytes_ms":{},"p50_parse_end_ms":{},"p50_dom_content_loaded_ms":{},"p50_load_ms":{},"cache_states":{{{cache}}}}}"#,
        samples.len(),
        last.resources,
        p50(samples.iter().map(|s| s.wall_ms).collect()),
        metric("first_bytes"),
        metric("parse_end"),
        metric("dom_content_loaded"),
        metric("load"),
    )
}

fn main() {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let srv = server();
    let url = srv.url("/");

    let cold: Vec<Sample> = (0..COLD_RUNS)
        .map(|_| {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut b = Browser::new_normal(dir.path().to_path_buf(), 1024, 768).expect("browser");
            measure(&mut b, &url, false)
        })
        .collect();

    let dir = tempfile::tempdir().expect("tempdir");
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 1024, 768).expect("browser");
    measure(&mut b, &url, false);
    let warm: Vec<Sample> = (0..WARM_RUNS)
        .map(|_| measure(&mut b, &url, false))
        .collect();
    let reload: Vec<Sample> = (0..WARM_RUNS)
        .map(|_| measure(&mut b, &url, true))
        .collect();

    let large_url = srv.url("/large");
    let large: Vec<Sample> = (0..3).map(|_| measure(&mut b, &large_url, false)).collect();
    let large_parse: Vec<f64> = large
        .iter()
        .filter_map(|s| Some(s.milestones.get("parse_end")? - s.milestones.get("parse_start")?))
        .collect();
    let bytes = large_page().len();
    let parse_ms = p50(large_parse);

    println!(
        "{{\n  \"profile\": \"{profile}\",\n  \"transport\": \"loopback http/1.1\",\n  \
         \"page\": {{\"stylesheets\":{RESOURCES},\"defer_scripts\":{RESOURCES},\"images\":{RESOURCES}}},\n  \
         \"cold\": {},\n  \"warm\": {},\n  \"reload\": {},\n  \
         \"large_document\": {{\"bytes\":{bytes},\"p50_parse_ms\":{parse_ms:.2},\"mib_per_s\":{:.1},\"summary\":{}}}\n}}",
        summary(&cold),
        summary(&warm),
        summary(&reload),
        bytes as f64 / (1024.0 * 1024.0) / (parse_ms / 1000.0),
        summary(&large),
    );
}
