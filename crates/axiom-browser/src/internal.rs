//! Trusted internal `axiom://` pages — never fetched over HTTP.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct InternalPage {
    pub url: String,
    pub title: String,
    pub html: String,
}

pub struct InternalPageRegistry {
    pages: HashMap<String, InternalPage>,
}

impl Default for InternalPageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl InternalPageRegistry {
    pub fn new() -> Self {
        let mut pages = HashMap::new();
        pages.insert(
            "axiom://newtab".into(),
            InternalPage {
                url: "axiom://newtab".into(),
                title: "New Tab".into(),
                html: NEWTAB_HTML.into(),
            },
        );
        // Extensible placeholders for later waves
        for (url, title, body) in [
            ("axiom://settings", "Settings", "Settings (coming soon)"),
            ("axiom://history", "History", "History (coming soon)"),
            ("axiom://cookies", "Cookies", "Cookies (loading…)"),
            ("axiom://network", "Network", "Network (loading…)"),
            ("axiom://document", "Document", "Document (loading…)"),
            ("axiom://downloads", "Downloads", "Downloads (coming soon)"),
            ("axiom://version", "Version", "Axiom 1.3.1"),
            (
                "axiom://performance",
                "Performance",
                "Performance (coming soon)",
            ),
        ] {
            pages.insert(
                url.into(),
                InternalPage {
                    url: url.into(),
                    title: title.into(),
                    html: format!(
                        r#"<!DOCTYPE html><html><head><title>{title}</title>
<style>
body {{ font-family: system-ui, sans-serif; margin: 48px; color: #1a1a1a; background: #f6f7f9; }}
h1 {{ font-weight: 600; }}
p {{ color: #555; }}
</style></head><body><h1>{title}</h1><p>{body}</p></body></html>"#
                    ),
                },
            );
        }
        Self { pages }
    }

    pub fn resolve(&self, url: &str) -> Option<&InternalPage> {
        let key = normalize_internal(url)?;
        self.pages.get(&key)
    }

    pub fn is_internal(url: &str) -> bool {
        normalize_internal(url).is_some()
    }

    pub fn insert_dynamic(&mut self, url: &str, title: impl Into<String>, html: String) {
        let title = title.into();
        let key = normalize_internal(url).unwrap_or_else(|| url.to_string());
        self.pages.insert(
            key.clone(),
            InternalPage {
                url: key,
                title,
                html,
            },
        );
    }
}

fn normalize_internal(url: &str) -> Option<String> {
    let t = url.trim();
    if !t.to_ascii_lowercase().starts_with("axiom://") {
        return None;
    }
    let rest = &t["axiom://".len()..];
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    Some(format!("axiom://{}", host.to_ascii_lowercase()))
}

const NEWTAB_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
<title>New Tab</title>
<style>
  html, body { height: 100%; margin: 0; }
  body {
    font-family: "Segoe UI", system-ui, sans-serif;
    background: radial-gradient(circle at 50% 28%, #243b70 0%, #111827 42%, #080d19 100%);
    color: #f3f7ff;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .wrap { text-align: center; max-width: 520px; padding: 24px; }
  .mark {
    width: 86px; height: 86px; margin: 0 auto 20px; border: 8px solid #447aff;
    border-radius: 50%; position: relative; box-sizing: border-box;
    box-shadow: 0 0 0 10px #447aff22, 0 18px 56px #0008;
  }
  .mark::before { content: "A"; color: #f3f7ff; font-size: 50px; font-weight: 800;
    line-height: 70px; position: absolute; inset: 0; }
  .mark::after { content: ""; position: absolute; left: 12px; right: 12px; top: 37px;
    border-top: 6px solid #f3f7ff; transform: skewX(-16deg); }
  h1 {
    font-size: 42px;
    letter-spacing: 0.18em;
    font-weight: 700;
    margin: 0 0 12px;
  }
  .hint {
    font-size: 15px;
    color: #b8c7ea;
    margin-bottom: 28px;
  }
  .box {
    border: 1px solid #5174ba;
    background: #17243dcc;
    border-radius: 10px;
    padding: 14px 18px;
    color: #cbd8f5;
    font-size: 14px;
  }
  .recent {
    margin-top: 36px;
    font-size: 13px;
    color: #8ea4d4;
  }
</style>
</head>
<body>
  <div class="wrap">
    <div class="mark" aria-label="Axiom"></div>
    <h1>AXIOM</h1>
    <p class="hint">Independent browser engine</p>
    <div class="box">Search or enter an address above — Ctrl+L to focus</div>
    <p class="recent">No AI. No telemetry. Your profile stays local.</p>
  </div>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_newtab() {
        let reg = InternalPageRegistry::new();
        let p = reg.resolve("axiom://newtab").expect("newtab");
        assert_eq!(p.title, "New Tab");
        assert!(p.html.contains("AXIOM"));
        assert!(InternalPageRegistry::is_internal("axiom://newtab"));
        assert!(!InternalPageRegistry::is_internal("https://example.com"));
    }
}
