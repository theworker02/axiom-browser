//! Trusted internal pages for search: `axiom://settings`, `axiom://settings/search` and
//! the `axiom://search` placeholder of the future Axiom Search provider.

use crate::browser::html_escape;
use crate::search::{SearchProviderService, AXIOM_SEARCH};
use crate::settings_repo::BrowserSettings;

const STYLE: &str =
    "body{font-family:system-ui,sans-serif;margin:40px;background:#f6f7f9;color:#1a1a1a}\
h1{font-weight:600} a{color:#1a5fb4} .meta{color:#777;font-size:12px}\
code{background:#e8ebf0;padding:2px 6px;border-radius:4px;word-break:break-all}\
li{margin:8px 0} .current{font-weight:600}";

pub(crate) fn render_settings_page(
    settings: &BrowserSettings,
    search: &SearchProviderService,
    private: bool,
) -> String {
    let note = if private {
        "<p class=\"meta\">Private window — settings changed here last until the window closes.</p>"
    } else {
        ""
    };
    format!(
        r#"<!DOCTYPE html><html><head><title>Settings</title><style>{STYLE}</style></head><body>
<h1>Settings</h1>{note}
<ul>
<li>Search engine: <span class="current">{engine}</span> — <a href="axiom://settings/search">Change</a></li>
<li>Homepage: <code>{homepage}</code></li>
<li>Restore previous session: <code>{restore}</code></li>
<li>Theme: <code>{theme}</code></li>
</ul>
<p class="meta">Trusted internal page — axiom://settings</p></body></html>"#,
        engine = html_escape(&search.default_provider().name),
        homepage = html_escape(&settings.homepage),
        restore = settings.restore_previous_session,
        theme = html_escape(&settings.theme),
    )
}

pub(crate) fn render_search_settings_page(search: &SearchProviderService, private: bool) -> String {
    let current = search.default_provider().id.clone();
    let mut rows = String::new();
    for p in search.providers() {
        let name = html_escape(&p.name);
        let template = html_escape(&p.search_url_template);
        if p.id == current {
            rows.push_str(&format!(
                "<li class=\"current\">{name} (default) <code>{template}</code></li>"
            ));
        } else {
            rows.push_str(&format!(
                "<li><a id=\"provider-{id}\" href=\"axiom://settings/search?provider={id}\">Use {name}</a> <code>{template}</code></li>",
                id = html_escape(&p.id),
            ));
        }
    }
    let note = if private {
        "Private window — the choice lasts until the window closes and is not saved."
    } else {
        "The choice is saved in this profile."
    };
    format!(
        r#"<!DOCTYPE html><html><head><title>Search engine</title><style>{STYLE}</style></head><body>
<h1>Search engine</h1>
<p>Text typed in the address bar that is not an address is sent to the default search engine.</p>
<ul>{rows}</ul>
<p class="meta">{note}</p>
<p class="meta">Trusted internal page — axiom://settings/search</p></body></html>"#
    )
}

/// Axiom Search has no index yet: the page says so and offers the query to the
/// web providers instead of pretending to return results.
pub(crate) fn render_axiom_search_page(query: &str, search: &SearchProviderService) -> String {
    let shown = html_escape(query);
    let mut alternatives = String::new();
    if !query.trim().is_empty() {
        for p in search.providers().iter().filter(|p| p.id != AXIOM_SEARCH) {
            alternatives.push_str(&format!(
                "<li><a href=\"{url}\">Search {name}</a></li>",
                url = html_escape(&p.url_for_query(query)),
                name = html_escape(&p.name),
            ));
        }
    }
    format!(
        r#"<!DOCTYPE html><html><head><title>Axiom Search</title><style>{STYLE}</style></head><body>
<h1>Axiom Search</h1>
<p>Query: <code>{shown}</code></p>
<p>Axiom Search is not available yet: there is no crawler or index behind it. The planned design is described in <code>docs/AXIOM_SEARCH_ARCHITECTURE.md</code>.</p>
<ul>{alternatives}</ul>
<p><a href="axiom://settings/search">Choose a default search engine</a></p>
<p class="meta">Trusted internal page — axiom://search</p></body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axiom_search_page_escapes_query_and_offers_web_providers() {
        let service = SearchProviderService::builtin();
        let html = render_axiom_search_page("<script>alert(1)</script> & co", &service);
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("https://www.google.com/search?q="));
        assert!(html.contains("not available yet"));
    }

    #[test]
    fn search_settings_page_marks_default_and_links_others() {
        let mut service = SearchProviderService::builtin();
        service.select("google").expect("google");
        let html = render_search_settings_page(&service, false);
        assert!(html.contains("Google (default)"));
        assert!(html.contains("axiom://settings/search?provider=duckduckgo"));
        assert!(!html.contains("provider=google"));
    }
}
