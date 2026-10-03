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
            ("axiom://version", "Version", "Axiom 1.2.8"),
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
    background: linear-gradient(160deg, #eef1f6 0%, #f8f9fb 45%, #e8edf5 100%);
    color: #1c1f26;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .wrap { text-align: center; max-width: 520px; padding: 24px; }
  h1 {
    font-size: 42px;
    letter-spacing: 0.18em;
    font-weight: 700;
    margin: 0 0 12px;
  }
  .hint {
    font-size: 15px;
    color: #5a6270;
    margin-bottom: 28px;
  }
  .box {
    border: 1px solid #c5ccd8;
    background: #ffffffcc;
    border-radius: 10px;
    padding: 14px 18px;
    color: #7a8494;
    font-size: 14px;
  }
  .recent {
    margin-top: 36px;
    font-size: 13px;
    color: #8a93a3;
  }
</style>
</head>
<body>
  <div class="wrap">
    <h1>AXIOM</h1>
    <p class="hint">Search or enter address</p>
    <div class="box">Use the omnibox above — Ctrl+L to focus</div>
    <p class="recent">Recently visited — architecture placeholder</p>
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
