//! Wave B — chrome, omnibox, internal pages, tab synchronization.

use axiom_browser::{
    Browser, ClassifiedInput, FocusOwner, InternalPageRegistry, OmniboxInputClassifier,
    ReloadStopMode, SearchProvider, SecurityDisplay,
};
use std::path::PathBuf;

#[test]
fn classifier_urls_search_localhost_ip() {
    assert!(matches!(
        OmniboxInputClassifier::classify("https://example.com"),
        ClassifiedInput::Url(_)
    ));
    match OmniboxInputClassifier::classify("example.com") {
        ClassifiedInput::Url(u) => assert!(u.starts_with("https://")),
        _ => panic!(),
    }
    match OmniboxInputClassifier::classify("localhost:3000") {
        ClassifiedInput::Url(u) => assert!(u.contains("http://localhost:3000")),
        _ => panic!(),
    }
    match OmniboxInputClassifier::classify("192.168.1.1") {
        ClassifiedInput::Url(u) => assert!(u.starts_with("http://192.168.1.1")),
        _ => panic!(),
    }
    assert!(matches!(
        OmniboxInputClassifier::classify("hello world"),
        ClassifiedInput::Search(_)
    ));
}

#[test]
fn search_provider_encodes() {
    let p = SearchProvider::duckduckgo();
    assert_eq!(
        p.url_for_query("rust browser"),
        "https://duckduckgo.com/?q=rust+browser"
    );
}

#[test]
fn omnibox_editing_and_escape() {
    let mut browser = Browser::new_private(800, 600).expect("profile");
    assert_eq!(browser.window.tabs.active_tab().url(), "axiom://newtab");
    assert_eq!(browser.chrome.focus, FocusOwner::BrowserChrome);
    browser.handle_chrome_key("l", true, false, false);
    assert!(browser.chrome.omnibox.editing);
    assert!(browser.chrome.omnibox.has_selection());
    browser.handle_chrome_key("x", false, false, false);
    assert_eq!(browser.chrome.omnibox.text, "x");
    browser.handle_chrome_key("Escape", false, false, false);
    assert!(!browser.chrome.omnibox.editing);
    assert_eq!(browser.chrome.omnibox.text, "axiom://newtab");
}

#[test]
fn suggestion_navigation() {
    let mut browser = Browser::new_private(800, 600).expect("profile");
    browser.focus_omnibox();
    browser.chrome.omnibox.text = "example".into();
    browser.chrome.omnibox.caret = 7;
    browser.chrome.omnibox.clear_selection();
    browser.chrome.omnibox.editing = true;
    browser.refresh_suggestions();
    assert!(!browser.chrome.suggestions.is_empty());
    browser.handle_chrome_key("ArrowDown", false, false, false);
    assert_eq!(browser.chrome.suggestions.selected, Some(0));
}

#[test]
fn newtab_internal_routing() {
    let reg = InternalPageRegistry::new();
    assert!(reg.resolve("axiom://newtab").is_some());
    let browser = Browser::new_private(640, 480).expect("profile");
    assert!(browser
        .window
        .tabs
        .active_tab()
        .url()
        .starts_with("axiom://"));
    let state = browser.chrome_state();
    assert_eq!(state.security, SecurityDisplay::Internal);
    assert!(state.window_title.contains("Axiom"));
}

#[test]
fn tab_switch_syncs_omnibox() {
    let mut browser = Browser::new_private(800, 600).expect("profile");

    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .load_local_html(
            "axiom://version",
            "<html><head><title>Alpha</title><body>A</body></html>",
            true,
        )
        .unwrap();
    browser.window.tabs.active_tab_mut().refresh_origin_public();
    browser.select_tab(0);
    assert_eq!(browser.chrome_state().tab_url, "axiom://version");

    browser.new_tab();
    browser.navigate_resolved("axiom://settings");
    assert_eq!(browser.chrome_state().tab_url, "axiom://settings");
    assert!(!browser.chrome.omnibox.editing);

    browser.select_tab(0);
    assert_eq!(browser.chrome_state().tab_url, "axiom://version");
    assert_eq!(browser.chrome.omnibox.text, "axiom://version");
    assert!(
        browser.chrome_state().tab_title.contains("Alpha")
            || browser.chrome_state().tab_title.contains("Version")
    );

    browser.select_tab(1);
    assert!(browser.chrome_state().can_go_back); // newtab then settings
    assert_eq!(browser.chrome_state().reload_stop, ReloadStopMode::Reload);

    browser.back();
    let url = browser.chrome_state().tab_url;
    assert!(url.starts_with("axiom://"));

    browser.close_tab();
    assert_eq!(browser.window.tabs.len(), 1);
    assert_eq!(browser.chrome_state().tab_url, "axiom://version");

    browser.new_tab();
    browser.navigate_resolved("axiom://history");
    browser.close_tab();
    browser.restore_tab();
    assert_eq!(browser.window.tabs.len(), 2);
    assert!(browser
        .window
        .tabs
        .active_tab()
        .url()
        .starts_with("axiom://"));
}

#[test]
fn trust_boundary_independent_browsing_contexts() {
    let mut browser = Browser::new_private(800, 600).expect("profile");
    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .page
        .load_html(
            "about:a",
            "<html><body><div id='x'>secret-a</div></body></html>",
        )
        .unwrap();
    browser.window.tabs.active_tab_mut().context.page.url = "about:a".into();
    browser.new_tab();
    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .page
        .load_html(
            "about:b",
            "<html><body><div id='x'>secret-b</div></body></html>",
        )
        .unwrap();
    browser.window.tabs.active_tab_mut().context.page.url = "about:b".into();

    let b = {
        let tab = browser.window.tabs.active_tab();
        let id = tab
            .context
            .page
            .document
            .borrow()
            .query_selector("#x")
            .unwrap();
        tab.context.page.document.borrow().text_content(id)
    };
    assert!(b.contains("secret-b"));
    browser.select_tab(0);
    let a = {
        let tab = browser.window.tabs.active_tab();
        let id = tab
            .context
            .page
            .document
            .borrow()
            .query_selector("#x")
            .unwrap();
        tab.context.page.document.borrow().text_content(id)
    };
    assert!(a.contains("secret-a"));
    assert!(!a.contains("secret-b"));
}

#[test]
fn search_classification_builds_provider_url() {
    let browser = Browser::new_private(800, 600).expect("profile");
    match OmniboxInputClassifier::classify("rust browser engine") {
        ClassifiedInput::Search(q) => {
            let url = browser.search.search_url(&q);
            assert!(url.starts_with("https://duckduckgo.com/?q="));
            assert!(url.contains("rust"));
        }
        _ => panic!("expected search"),
    }
}

#[test]
fn window_title_format() {
    let mut browser = Browser::new_private(640, 480).expect("profile");
    browser.window.tabs.active_tab_mut().context.page.title = "Example".into();
    assert_eq!(browser.chrome_state().window_title, "Example — Axiom");
}

#[test]
fn navigate_resolved_is_public() {
    // ensure method used by tests is available
    let mut browser = Browser::new_private(640, 480).expect("profile");
    browser.navigate_resolved("axiom://version");
    assert_eq!(browser.chrome_state().tab_url, "axiom://version");
}

#[test]
fn profile_smoke() {
    let _ = Browser::new_normal(PathBuf::from("target/axiom-profile-waveb"), 800, 600)
        .expect("profile");
}
