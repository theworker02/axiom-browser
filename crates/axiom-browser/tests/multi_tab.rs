//! Independent tabs must not share document/JS state.

use axiom_browser::Browser;
use std::path::PathBuf;

#[test]
fn tabs_are_independent_browsing_contexts() {
    let mut browser =
        Browser::new_normal(PathBuf::from("target/axiom-profile-test"), 800, 600).expect("profile");

    browser.navigate("about:blank");
    // Load distinct HTML into tab 0 via browsing context helper
    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .page
        .load_html(
            "about:tab-a",
            r#"<html><body><div id="mark">AAA</div></body></html>"#,
        )
        .unwrap();
    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .page
        .update_rendering_if_needed();

    let tab_b = browser.new_tab();
    assert_ne!(browser.window.tabs.tabs()[0].id, tab_b);

    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .page
        .load_html(
            "about:tab-b",
            r#"<html><body><div id="mark">BBB</div></body></html>"#,
        )
        .unwrap();
    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .page
        .update_rendering_if_needed();

    // Active is tab B
    let b_text = {
        let tab = browser.window.tabs.active_tab();
        let id = tab
            .context
            .page
            .document
            .borrow()
            .query_selector("#mark")
            .unwrap();
        tab.context.page.document.borrow().text_content(id)
    };
    assert!(b_text.contains("BBB"));

    // Switch to tab A — still AAA
    browser.window.tabs.select(0);
    let a_text = {
        let tab = browser.window.tabs.active_tab();
        let id = tab
            .context
            .page
            .document
            .borrow()
            .query_selector("#mark")
            .unwrap();
        tab.context.page.document.borrow().text_content(id)
    };
    assert!(a_text.contains("AAA"));
    assert!(!a_text.contains("BBB"));
}

#[test]
fn close_and_restore_tab() {
    let mut browser = Browser::new_private(640, 480).expect("profile");
    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .page
        .load_html("about:one", "<html><body>one</body></html>")
        .unwrap();
    browser.new_tab();
    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .page
        .load_html("about:two", "<html><body>two</body></html>")
        .unwrap();
    browser.window.tabs.active_tab_mut().last_url = "about:two".into();

    assert_eq!(browser.window.tabs.len(), 2);
    browser.close_tab();
    assert_eq!(browser.window.tabs.len(), 1);

    browser.restore_tab();
    assert_eq!(browser.window.tabs.len(), 2);
}

#[test]
fn session_snapshot_roundtrip_json() {
    let mut browser = Browser::new_private(640, 480).expect("profile");
    browser.new_tab();
    let snap = browser.snapshot_session();
    let json = snap.to_json();
    let parsed = axiom_browser::SessionSnapshot::from_json(&json).expect("json");
    assert_eq!(parsed.tabs.len(), 2);
}
