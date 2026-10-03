//! Local WPT-style HTTP server over the vendored `tests/wpt` tree (offline, loopback).
//!
//! Implements the parts of wptserve the vendored tests rely on most:
//!
//! * static files with WPT's MIME mapping, plus `<file>.headers` and `__dir__.headers`;
//! * `.sub.` files: `{{host}}`, `{{domains[…]}}`, `{{hosts[…][…]}}`, `{{ports[…][…]}}`,
//!   `{{location[…]}}` and `{{GET[…]}}` substitutions;
//! * generated wrappers for `*.window.js` / `*.any.js` tests (`.window.html`,
//!   `.any.html`), honoring `// META: script=` and `// META: title=`;
//! * `/resources/testharnessreport.js` is replaced by Axiom's reporter, which stores the
//!   harness results in `window.__axiom_wpt_result` (the vendored file is untouched).
//!
//! Python handlers and multiple origins are not provided; tests needing them fail.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axiom_net::test_server::{TestRequest, TestResponse, TestServer};

/// Replacement for `/resources/testharnessreport.js`.
pub const REPORT_JS: &str = r#"(function () {
  setup({ output: false });
  add_completion_callback(function (tests, status) {
    var out = {
      status: status.status,
      message: status.message == null ? null : String(status.message),
      tests: []
    };
    for (var i = 0; i < tests.length; i++) {
      var t = tests[i];
      out.tests.push({
        name: String(t.name),
        status: t.status,
        message: t.message == null ? null : String(t.message)
      });
    }
    window.__axiom_wpt_result = JSON.stringify(out);
  });
})();
"#;

pub struct WptServer {
    server: TestServer,
    not_found: Arc<Mutex<HashSet<String>>>,
}

impl WptServer {
    pub fn spawn(wpt_root: impl Into<PathBuf>) -> Self {
        let root = wpt_root.into();
        let not_found = Arc::new(Mutex::new(HashSet::new()));
        let nf = Arc::clone(&not_found);
        Self {
            server: TestServer::spawn(move |req| {
                let resp = routes(&root, req);
                if matches!(resp, TestResponse::Fixed { status: 404, .. }) {
                    let path = req.path.split('?').next().unwrap_or("");
                    let rel = percent_decode(path).trim_start_matches('/').to_string();
                    nf.lock().unwrap().insert(rel);
                }
                resp
            }),
            not_found,
        }
    }

    /// Every path answered 404 so far, sorted.
    pub fn not_found_paths(&self) -> Vec<String> {
        let mut v: Vec<String> = self.not_found.lock().unwrap().iter().cloned().collect();
        v.sort();
        v
    }

    /// Whether `rel` (a WPT-root-relative path) was requested and answered 404.
    pub fn was_not_found(&self, rel: &str) -> bool {
        self.not_found.lock().unwrap().contains(rel)
    }

    /// URL of `path` (a path under the WPT root, with or without a leading `/`).
    pub fn url(&self, path: &str) -> String {
        let path = path.replace('\\', "/");
        let path = path.trim_start_matches('/');
        self.server.url(&format!("/{}", encode_path(path)))
    }

    pub fn port(&self) -> u16 {
        self.server.port()
    }
}

fn encode_path(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'/'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'!'
            | b'('
            | b')'
            | b','
            | b'='
            | b'+'
            | b'@'
            | b'$'
            | b'\''
            | b'*'
            | b';'
            | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn mime_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "xhtml" | "xht" => "application/xhtml+xml",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "txt" => "text/plain",
        "png" => "image/png",
        "gif" => "image/gif",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

fn not_found() -> TestResponse {
    TestResponse::status(404, "not found").with_header("Content-Type", "text/plain")
}

pub fn routes(root: &Path, req: &TestRequest) -> TestResponse {
    let (raw_path, query) = req.path.split_once('?').unwrap_or((&req.path, ""));
    let path = percent_decode(raw_path);
    let rel = path.trim_start_matches('/');
    if rel.split('/').any(|s| s == "..") || rel.contains('\\') {
        return not_found();
    }
    if rel == "resources/testharnessreport.js" {
        return TestResponse::ok(REPORT_JS, "text/javascript")
            .with_header("Cache-Control", "no-store");
    }
    // Minimal deterministic replacement for the WPT handler used by the
    // Document.contentType tests. It only reflects the requested media type;
    // executing arbitrary Python handlers remains deliberately out of scope.
    if rel.ends_with("/support/contenttype_setter.py") {
        let params: std::collections::HashMap<_, _> = query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(key, value)| (percent_decode(key), percent_decode(value)))
            .collect();
        let type_ = params.get("type").map(String::as_str).unwrap_or("text");
        let subtype = params.get("subtype").map(String::as_str).unwrap_or("plain");
        return TestResponse::Fixed {
            status: 200,
            headers: vec![
                ("Content-Type".into(), format!("{type_}/{subtype}")),
                ("Cache-Control".into(), "no-store".into()),
            ],
            body: Vec::new(),
        };
    }
    let file = root.join(rel);
    let body = if file.is_file() {
        match std::fs::read(&file) {
            Ok(b) => b,
            Err(_) => return not_found(),
        }
    } else if let Some(wrapper) = generated_wrapper(root, rel) {
        wrapper.into_bytes()
    } else {
        return not_found();
    };
    let body = if rel.contains(".sub.") {
        let host = req.header("host").unwrap_or("127.0.0.1");
        substitute(&String::from_utf8_lossy(&body), host, query).into_bytes()
    } else {
        body
    };
    let mut resp = TestResponse::ok(body, mime_for(rel)).with_header("Cache-Control", "no-store");
    for headers_file in [
        file.parent().map(|d| d.join("__dir__.headers")),
        Some(PathBuf::from(format!("{}.headers", file.display()))),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(text) = std::fs::read_to_string(&headers_file) {
            for line in text.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    resp = resp.with_header(k.trim(), v.trim());
                }
            }
        }
    }
    resp
}

/// `foo.window.html` / `foo.any.html` for `foo.window.js` / `foo.any.js`.
fn generated_wrapper(root: &Path, rel: &str) -> Option<String> {
    let js_rel = if let Some(base) = rel.strip_suffix(".window.html") {
        format!("{base}.window.js")
    } else if let Some(base) = rel.strip_suffix(".any.html") {
        format!("{base}.any.js")
    } else {
        return None;
    };
    let source = std::fs::read_to_string(root.join(&js_rel)).ok()?;
    let mut head = String::new();
    for line in source.lines() {
        let Some(meta) = line.strip_prefix("// META:") else {
            if line.trim().is_empty() || line.starts_with("//") {
                continue;
            }
            break;
        };
        let meta = meta.trim();
        if let Some(src) = meta.strip_prefix("script=") {
            head.push_str(&format!("<script src=\"{}\"></script>\n", src.trim()));
        } else if let Some(title) = meta.strip_prefix("title=") {
            head.push_str(&format!("<title>{}</title>\n", title.trim()));
        }
    }
    let name = js_rel.rsplit('/').next().unwrap_or(&js_rel);
    Some(format!(
        "<!doctype html>\n<meta charset=utf-8>\n{head}<script src=\"/resources/testharness.js\"></script>\n<script src=\"/resources/testharnessreport.js\"></script>\n<div id=log></div>\n<script src=\"{name}\"></script>\n"
    ))
}

/// wptserve `.sub.` template substitution for a single-origin loopback server.
pub fn substitute(text: &str, host_header: &str, query: &str) -> String {
    let (host, port) = host_header
        .rsplit_once(':')
        .map_or((host_header, "80"), |(h, p)| (h, p));
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let expr = after[..end].trim();
        let value = if expr == "host" || expr.starts_with("domains[") || expr.starts_with("hosts[")
        {
            Some(host.to_string())
        } else if expr.starts_with("ports[") {
            Some(port.to_string())
        } else if let Some(key) = expr
            .strip_prefix("location[")
            .and_then(|k| k.strip_suffix(']'))
        {
            match key {
                "host" => Some(host_header.to_string()),
                "hostname" => Some(host.to_string()),
                "port" => Some(port.to_string()),
                "scheme" => Some("http".to_string()),
                "server" => Some(format!("http://{host_header}")),
                _ => None,
            }
        } else {
            expr.strip_prefix("GET[")
                .and_then(|k| k.strip_suffix(']'))
                .map(|key| {
                    query
                        .split('&')
                        .filter_map(|kv| kv.split_once('='))
                        .find(|(k, _)| *k == key)
                        .map(|(_, v)| percent_decode(v))
                        .unwrap_or_default()
                })
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(path: &str) -> TestRequest {
        TestRequest {
            method: "GET".into(),
            path: path.into(),
            headers: vec![("Host".into(), "127.0.0.1:8123".into())],
            body: Vec::new(),
            body_chunks: 0,
            connection_id: 1,
        }
    }

    fn body(r: &TestResponse) -> String {
        match r {
            TestResponse::Fixed { body, .. } => String::from_utf8_lossy(body).into_owned(),
            _ => String::new(),
        }
    }

    fn status(r: &TestResponse) -> u16 {
        match r {
            TestResponse::Fixed { status, .. } => *status,
            _ => 0,
        }
    }

    #[test]
    fn substitutions_use_the_loopback_origin() {
        let s = substitute(
            "{{host}}:{{ports[http][0]}} {{domains[www]}} {{location[server]}} {{GET[a]}} {{unknown}}",
            "127.0.0.1:8123",
            "a=x%20y",
        );
        assert_eq!(
            s,
            "127.0.0.1:8123 127.0.0.1 http://127.0.0.1:8123 x y {{unknown}}"
        );
    }

    #[test]
    fn serves_vendored_files_wrappers_and_the_reporter() {
        let root = crate::workspace_root().join("tests/wpt");
        let r = routes(&root, &req("/resources/testharnessreport.js"));
        assert!(body(&r).contains("__axiom_wpt_result"));
        let r = routes(&root, &req("/resources/testharness.js"));
        assert_eq!(status(&r), 200);
        assert!(body(&r).contains("add_completion_callback"));
        assert_eq!(status(&routes(&root, &req("/../Cargo.toml"))), 404);
        assert_eq!(status(&routes(&root, &req("/nope.html"))), 404);
        let any_js = std::fs::read_dir(root.join("dom/nodes"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .find(|n| n.ends_with(".window.js"))
            .expect("a .window.js test is vendored");
        let html = any_js.replace(".window.js", ".window.html");
        let r = routes(&root, &req(&format!("/dom/nodes/{html}")));
        assert_eq!(status(&r), 200);
        let b = body(&r);
        assert!(
            b.contains("/resources/testharness.js") && b.contains(&any_js),
            "{b}"
        );
    }
}
