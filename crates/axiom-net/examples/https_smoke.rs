//! Manual real-world HTTPS smoke test. Needs internet access, so it is never run by CI.
//!
//! ```text
//! cargo run -p axiom-net --example https_smoke -- https://example.com https://www.wikipedia.org
//! ```
//!
//! For each URL: fetch through the normal NetworkService (bundled Mozilla roots from
//! `webpki-roots`, certificate verification on), then fetch again to exercise the cache. Prints facts only; exits
//! non-zero if any fetch fails.

use axiom_net::{NetworkRequest, NetworkService, NetworkServiceConfig, ResourceType};
use axiom_url::Url;

fn main() {
    let mut urls: Vec<String> = std::env::args().skip(1).collect();
    if urls.is_empty() {
        urls.push("https://example.com/".into());
    }
    let svc = NetworkService::new(NetworkServiceConfig {
        persistent_cache: false,
        label: "smoke".into(),
        ..NetworkServiceConfig::default()
    });
    let mut failures = 0;
    for u in &urls {
        let Ok(url) = Url::parse(u) else {
            eprintln!("{u}: not an http(s) URL");
            failures += 1;
            continue;
        };
        for pass in ["first", "second"] {
            let req = NetworkRequest::get(url.clone(), ResourceType::Document);
            let id = req.id;
            match svc.execute_blocking(req) {
                Ok(resp) => {
                    let body = resp.body.read_all(16 * 1024 * 1024);
                    let m = &resp.meta;
                    println!(
                        "{u} [{pass}] status={} protocol={} cache={} bytes={} final={}",
                        m.status,
                        m.protocol.as_str(),
                        m.cache_state.as_str(),
                        body.as_ref().map(Vec::len).unwrap_or(0),
                        m.final_url.as_str()
                    );
                    if let Some(tls) = &m.tls {
                        println!(
                            "  tls: verified={} alpn={} host={}",
                            tls.certificate_verified,
                            tls.alpn.as_str(),
                            tls.hostname.as_deref().unwrap_or("?")
                        );
                        if let Some(c) = &tls.certificate {
                            println!(
                                "  cert: subject={} issuer={} sha256={}",
                                c.subject, c.issuer, c.sha256_fingerprint
                            );
                        }
                    }
                    if let Some(e) = svc.log_entry(id) {
                        println!(
                            "  timing: ttfb_ms={:?} total_ms={:?} redirects={}",
                            e.timing.ttfb_ms,
                            e.timing.total_ms,
                            e.redirects.len()
                        );
                    }
                    if let Err(e) = body {
                        eprintln!("  body error: {e}");
                        failures += 1;
                    }
                }
                Err(e) => {
                    eprintln!("{u} [{pass}] failed ({}): {e}", e.kind_name());
                    failures += 1;
                    break;
                }
            }
        }
    }
    svc.shutdown();
    if failures > 0 {
        std::process::exit(1);
    }
}
