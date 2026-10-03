//! Trusted internal pages for search: `axiom://settings`, `axiom://settings/search` and
//! the `axiom://search` placeholder of the future Axiom Search provider.

use crate::browser::html_escape;
use crate::search::{SearchProviderService, AXIOM_SEARCH};
use crate::settings_repo::BrowserSettings;

const STYLE: &str =
    "body{font-family:system-ui,sans-serif;margin:40px;background:#f6f7f9;color:#1a1a1a;max-width:980px}\
h1{font-weight:700} h2{margin:30px 0 10px} a{color:#1a5fb4} .meta{color:#667085;font-size:12px}\
code{background:#e8ebf0;padding:2px 6px;border-radius:4px;word-break:break-all}\
li{margin:8px 0}.current{font-weight:600}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:12px}\
.card{background:#fff;border:1px solid #d8dee9;border-radius:12px;padding:16px}.state{color:#0b7a4b;font-size:12px;font-weight:700}";

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
<p class="meta">Axiom mirrors Chrome's broad settings categories without copying its UI. Green items are profile-persisted; controls labelled planned are not exposed as fake toggles.</p>
<div class="grid">
<section class="card"><h2>Appearance</h2><p>Theme: <code>{theme}</code><br>Default zoom: <code>{zoom}%</code><br>Reduced motion: <code>{motion}</code><br>High contrast: <code>{contrast}</code></p><span class="state">PERSISTED</span></section>
<section class="card"><h2>On startup</h2><p>Homepage: <code>{homepage}</code><br>New tab: <code>{new_tab}</code><br>Restore session: <code>{restore}</code></p><span class="state">PERSISTED</span></section>
<section class="card"><h2>Search</h2><p>Default engine: <span class="current">{engine}</span><br><a href="axiom://settings/search">Manage search engines</a></p><span class="state">FUNCTIONAL</span></section>
<section class="card"><h2>Privacy & security</h2><p>{privacy}<br>Referrer: <code>{referrer}</code><br>Mixed content: <code>{mixed}</code><br>Safe browsing: <code>{safe}</code></p><span class="state">LOCAL-FIRST</span></section>
<section class="card"><h2>Cookies & site data</h2><p>Block all cookies: <code>{cookies}</code><br>Clear cookies on exit: <code>{clear_cookies}</code><br>Clear history on exit: <code>{clear_history}</code><br><a href="axiom://cookies">Inspect cookies</a></p><span class="state">PROFILE-OWNED</span></section>
<section class="card"><h2>Downloads</h2><p>Folder: <code>{downloads}</code><br>Ask where to save: <code>{ask_download}</code><br>Automatic downloads: <code>{automatic_downloads}</code><br>Open PDFs externally: <code>{pdf}</code></p><span class="state">PERSISTED</span></section>
<section class="card"><h2>Languages & accessibility</h2><p>Languages: <code>{languages}</code><br>Spell check: <code>{spell}</code><br>Translate pages: <code>{translate}</code></p><span class="state">PREFERENCES</span></section>
<section class="card"><h2>Permissions</h2><p>Notifications: <code>{notifications}</code><br>Camera: <code>{camera}</code><br>Microphone: <code>{microphone}</code><br>Location: <code>{location}</code></p><span class="state">DEFAULT POLICY</span></section>
<section class="card"><h2>System & developer</h2><p>Background networking: <code>{background}</code><br>Hardware acceleration: <code>{hardware}</code><br>Performance HUD: <code>{hud}</code><br>Developer features: <code>{developer}</code></p><span class="state">PREFERENCES</span></section>
</div>
<p class="meta">Trusted internal page — axiom://settings</p></body></html>"#,
        engine = html_escape(&search.default_provider().name),
        homepage = html_escape(&settings.homepage),
        restore = settings.restore_previous_session,
        theme = html_escape(&settings.theme),
        zoom = settings.default_zoom_percent,
        motion = settings.reduced_motion,
        contrast = settings.high_contrast,
        new_tab = html_escape(&settings.new_tab_url),
        privacy = settings.privacy_summary(),
        referrer = settings.send_referrer,
        mixed = settings.allow_insecure_content,
        safe = settings.safe_browsing,
        cookies = settings.block_all_cookies,
        clear_cookies = settings.clear_cookies_on_exit,
        clear_history = settings.clear_history_on_exit,
        downloads = html_escape(if settings.download_directory.is_empty() {
            "Profile downloads"
        } else {
            &settings.download_directory
        }),
        ask_download = settings.ask_download_location,
        automatic_downloads = settings.automatic_downloads,
        pdf = settings.pdf_open_externally,
        languages = html_escape(&settings.preferred_languages.join(", ")),
        spell = settings.spell_check,
        translate = settings.page_translation,
        notifications = html_escape(&settings.site_notifications),
        camera = html_escape(&settings.camera_permission),
        microphone = html_escape(&settings.microphone_permission),
        location = html_escape(&settings.location_permission),
        background = settings.background_networking,
        hardware = settings.hardware_acceleration,
        hud = settings.performance_hud,
        developer = settings.developer_features,
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
