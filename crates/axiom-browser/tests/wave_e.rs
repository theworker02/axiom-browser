//! Wave E — localStorage / sessionStorage integration tests.

use std::sync::Arc;

use axiom_browser::{
    make_storage_binder, Browser, BrowserDataStore, Profile, ProfileManager, ProfileStorage,
    StorageOrigin, StorageQuota, StorageService, SCHEMA_VERSION,
};
use tempfile::tempdir;

#[test]
fn schema_is_v3() {
    let store = BrowserDataStore::open_private().unwrap();
    assert_eq!(store.schema_version(), 3);
    assert_eq!(SCHEMA_VERSION, 3);
}

#[test]
fn local_storage_persists_across_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("prof");
    {
        let profile = Profile::open_persistent("Default", path.clone()).unwrap();
        let svc = StorageService::new();
        let origin = StorageOrigin::from_url_str("https://example.test/page").unwrap();
        svc.local_set(&profile.store, &origin, "theme", "dark")
            .unwrap();
    }
    let profile = Profile::open_persistent("Default", path).unwrap();
    let svc = StorageService::new();
    let origin = StorageOrigin::from_url_str("https://example.test/").unwrap();
    assert_eq!(
        svc.local_get(&profile.store, &origin, "theme")
            .unwrap()
            .as_deref(),
        Some("dark")
    );
}

#[test]
fn session_storage_does_not_persist() {
    let dir = tempdir().unwrap();
    let browser = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    let origin = StorageOrigin::from_url_str("https://example.test/").unwrap();
    let svc = StorageService::new();
    svc.local_set(&browser.profile.store, &origin, "l", "1")
        .unwrap();
    let session = browser.window.tabs.active_tab().ensure_session_storage();
    session
        .lock()
        .set(origin.as_str(), "s", "1", &StorageQuota::default())
        .unwrap();
    drop(browser);

    let browser2 = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    assert_eq!(
        svc.local_get(&browser2.profile.store, &origin, "l")
            .unwrap()
            .as_deref(),
        Some("1")
    );
    let session2 = browser2.window.tabs.active_tab().ensure_session_storage();
    assert!(session2.lock().get(origin.as_str(), "s").is_none());
}

#[test]
fn private_local_storage_memory_only() {
    let dir = tempdir().unwrap();
    let mgr = ProfileManager::new(dir.path());
    {
        let private = mgr.open_private("P").unwrap();
        let svc = StorageService::new();
        let origin = StorageOrigin::from_url_str("https://example.test/").unwrap();
        svc.local_set(&private.store, &origin, "secret", "x")
            .unwrap();
        assert_eq!(
            svc.local_get(&private.store, &origin, "secret")
                .unwrap()
                .as_deref(),
            Some("x")
        );
    }
    let persistent = mgr.open_or_create("A").unwrap();
    let svc = StorageService::new();
    let origin = StorageOrigin::from_url_str("https://example.test/").unwrap();
    assert!(svc
        .local_get(&persistent.store, &origin, "secret")
        .unwrap()
        .is_none());
}

#[test]
fn origin_isolation() {
    let store = BrowserDataStore::open_private().unwrap();
    let svc = StorageService::new();
    let a = StorageOrigin::from_url_str("https://a.example/").unwrap();
    let b = StorageOrigin::from_url_str("https://b.example/").unwrap();
    svc.local_set(&store, &a, "k", "A").unwrap();
    svc.local_set(&store, &b, "k", "B").unwrap();
    assert_eq!(
        svc.local_get(&store, &a, "k").unwrap().as_deref(),
        Some("A")
    );
    assert_eq!(
        svc.local_get(&store, &b, "k").unwrap().as_deref(),
        Some("B")
    );
}

#[test]
fn clear_site_data_storage() {
    let store = BrowserDataStore::open_private().unwrap();
    let svc = StorageService::new();
    let o = StorageOrigin::from_url_str("https://example.test/").unwrap();
    svc.local_set(&store, &o, "k", "v").unwrap();
    assert_eq!(store.clear_local_storage_for_origin(o.as_str()).unwrap(), 1);
    assert!(svc.local_get(&store, &o, "k").unwrap().is_none());
}

#[test]
fn js_localstorage_e2e() {
    let dir = tempdir().unwrap();
    let mut browser = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    let html = r#"<!DOCTYPE html><html><body>
<div id="out">?</div>
<script>
localStorage.setItem('theme','dark');
var el = document.getElementById('out');
el.textContent = localStorage.getItem('theme') || 'missing';
</script></body></html>"#;
    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .load_local_html("https://example.test/", html, true)
        .unwrap();
    let doc = browser
        .window
        .tabs
        .active_tab()
        .context
        .page
        .document
        .borrow();
    let id = doc.get_element_by_id("out").expect("out");
    let text = doc.text_content(id);
    drop(doc);
    assert_eq!(text, "dark");
    let origin = StorageOrigin::from_url_str("https://example.test/").unwrap();
    let v = StorageService::new()
        .local_get(&browser.profile.store, &origin, "theme")
        .unwrap();
    assert_eq!(v.as_deref(), Some("dark"));
}

#[test]
fn js_sessionstorage_tab_isolated() {
    let dir = tempdir().unwrap();
    let mut browser = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    let html = r#"<!DOCTYPE html><html><body>
<div id="out">?</div>
<script>
sessionStorage.setItem('tab','A');
document.getElementById('out').textContent = sessionStorage.getItem('tab') || '';
</script></body></html>"#;
    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .load_local_html("https://example.test/", html, true)
        .unwrap();
    {
        let doc = browser
            .window
            .tabs
            .active_tab()
            .context
            .page
            .document
            .borrow();
        let id = doc.get_element_by_id("out").unwrap();
        assert_eq!(doc.text_content(id), "A");
    }
    let origin = "https://example.test";
    assert_eq!(
        browser
            .window
            .tabs
            .active_tab()
            .ensure_session_storage()
            .lock()
            .get(origin, "tab")
            .as_deref(),
        Some("A")
    );

    browser.new_tab();
    let html_b = r#"<!DOCTYPE html><html><body>
<div id="out">?</div>
<script>
document.getElementById('out').textContent = sessionStorage.getItem('tab') || 'empty';
</script></body></html>"#;
    browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .load_local_html("https://example.test/", html_b, true)
        .unwrap();
    {
        let doc = browser
            .window
            .tabs
            .active_tab()
            .context
            .page
            .document
            .borrow();
        let id = doc.get_element_by_id("out").unwrap();
        assert_eq!(doc.text_content(id), "empty");
    }
}

#[test]
fn profile_storage_binder_builds() {
    let store = Arc::new(BrowserDataStore::open_private().unwrap());
    let profile = ProfileStorage::new(Arc::clone(&store));
    let session = profile.new_session_map();
    let binder = make_storage_binder(profile, session);
    let host = binder("https://example.test/").expect("host");
    host.local_set("k", "v").unwrap();
    assert_eq!(host.local_get("k").as_deref(), Some("v"));
}
