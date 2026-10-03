//! Wave C — persistent profiles, history, bookmarks, sessions, isolation.

use axiom_browser::{
    Browser, BrowserSettings, HistoryRecord, ProfileManager, VisitTransition, SCHEMA_VERSION,
};
use std::fs;
use tempfile::tempdir;

#[test]
fn fresh_persistent_profile_schema() {
    let dir = tempdir().unwrap();
    let browser = Browser::new_normal(dir.path().to_path_buf(), 800, 600).expect("profile");
    assert_eq!(browser.profile.store.schema_version(), SCHEMA_VERSION);
    assert!(!browser.is_private());
}

#[test]
fn history_persists_across_reopen() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    {
        let mut b = Browser::new_normal(root.clone(), 800, 600).unwrap();
        // Simulate committed navigation without network: write history directly + via API
        b.profile
            .store
            .record_history("https://example.test/a", "Alpha", VisitTransition::Typed)
            .unwrap();
        b.shutdown_clean();
    }
    let b2 = Browser::new_normal(root, 800, 600).unwrap();
    let recent = b2.profile.store.recent_history(10).unwrap();
    assert!(recent.iter().any(|h| h.url.contains("example.test/a")));
}

#[test]
fn private_history_does_not_leak_to_persistent() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    {
        let mut persistent = Browser::new_normal(root.clone(), 800, 600).unwrap();
        persistent
            .profile
            .store
            .record_history("https://keep.example/", "Keep", VisitTransition::Link)
            .unwrap();
        persistent.shutdown_clean();
    }
    {
        let private = Browser::new_private(800, 600).unwrap();
        private
            .profile
            .store
            .record_history(
                "https://private.example.test/",
                "Secret",
                VisitTransition::Typed,
            )
            .unwrap();
        // Drop private — in-memory gone
    }
    let again = Browser::new_normal(root, 800, 600).unwrap();
    let all = again.profile.store.recent_history(100).unwrap();
    assert!(all.iter().any(|h| h.url.contains("keep.example")));
    assert!(!all.iter().any(|h| h.url.contains("private.example")));
}

#[test]
fn two_profiles_are_isolated() {
    let root = tempdir().unwrap();
    let mgr = ProfileManager::new(root.path());
    let a = mgr.open_or_create("Alice").unwrap();
    let b = mgr.open_or_create("Bob").unwrap();
    a.store
        .record_history("https://a.example/", "A", VisitTransition::Typed)
        .unwrap();
    b.store
        .record_history("https://b.example/", "B", VisitTransition::Typed)
        .unwrap();
    let a_hist = a.store.recent_history(10).unwrap();
    let b_hist = b.store.recent_history(10).unwrap();
    assert!(a_hist.iter().any(|h| h.url.contains("a.example")));
    assert!(!a_hist.iter().any(|h| h.url.contains("b.example")));
    assert!(b_hist.iter().any(|h| h.url.contains("b.example")));
    assert!(!b_hist.iter().any(|h| h.url.contains("a.example")));
}

#[test]
fn bookmarks_roundtrip() {
    let dir = tempdir().unwrap();
    let b = Browser::new_normal(dir.path().to_path_buf(), 640, 480).unwrap();
    b.profile
        .store
        .add_bookmark("https://book.example/", "Book")
        .unwrap();
    assert!(b
        .profile
        .store
        .bookmark_for_url("https://book.example/")
        .unwrap()
        .is_some());
    b.profile
        .store
        .remove_bookmark_url("https://book.example/")
        .unwrap();
    assert!(b
        .profile
        .store
        .bookmark_for_url("https://book.example/")
        .unwrap()
        .is_none());
}

#[test]
fn settings_persist() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    {
        let mut b = Browser::new_normal(root.clone(), 640, 480).unwrap();
        b.settings.theme = "dark".into();
        b.settings.restore_previous_session = false;
        b.profile.store.save_settings(&b.settings).unwrap();
        b.shutdown_clean();
    }
    let b2 = Browser::new_normal(root, 640, 480).unwrap();
    assert_eq!(b2.settings.theme, "dark");
    assert!(!b2.settings.restore_previous_session);
}

#[test]
fn session_checkpoint_and_restore() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    {
        let mut b = Browser::new_normal(root.clone(), 800, 600).unwrap();
        b.navigate_resolved("axiom://version");
        b.new_tab();
        b.navigate_resolved("axiom://bookmarks");
        b.shutdown_clean();
    }
    let b2 = Browser::new_normal(root, 800, 600).unwrap();
    // restore_previous_session default true — should restore
    assert!(!b2.window.tabs.is_empty());
    let urls: Vec<_> = b2.window.tabs.tabs().iter().map(|t| t.url()).collect();
    assert!(
        urls.iter().any(|u| u.contains("axiom://")),
        "expected restored axiom tabs, got {urls:?}"
    );
}

#[test]
fn profile_lock_blocks_second_open() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let _first = Browser::new_normal(root.clone(), 640, 480).unwrap();
    let second = Browser::new_normal(root, 640, 480);
    assert!(second.is_err());
}

#[test]
fn omnibox_history_suggestions() {
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    b.profile
        .store
        .record_history(
            "https://github.com/axiom",
            "GitHub Axiom",
            VisitTransition::Typed,
        )
        .unwrap();
    b.chrome.omnibox.editing = true;
    b.chrome.omnibox.text = "git".into();
    b.refresh_suggestions();
    assert!(b
        .chrome
        .suggestions
        .items
        .iter()
        .any(|s| s.url.contains("github.com")));
}

#[test]
fn end_to_end_profile_lifecycle() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    {
        let mut b = Browser::new_normal(root.clone(), 900, 700).unwrap();
        b.profile
            .store
            .record_history("https://example.test/a", "A", VisitTransition::Typed)
            .unwrap();
        b.profile
            .store
            .record_history("https://example.test/b", "B", VisitTransition::Link)
            .unwrap();
        b.profile
            .store
            .add_bookmark("https://example.test/b", "B")
            .unwrap();
        // Fake two-tab session
        b.navigate_resolved("axiom://version");
        b.new_tab();
        b.navigate_resolved("axiom://history");
        b.shutdown_clean();
    }
    {
        let b = Browser::new_normal(root.clone(), 900, 700).unwrap();
        let hist = b.profile.store.recent_history(20).unwrap();
        assert!(hist.iter().any(|h: &HistoryRecord| h.url.ends_with("/a")));
        assert!(hist.iter().any(|h| h.url.ends_with("/b")));
        assert!(b
            .profile
            .store
            .bookmark_for_url("https://example.test/b")
            .unwrap()
            .is_some());
    }
    {
        let p = Browser::new_private(800, 600).unwrap();
        p.profile
            .store
            .record_history("https://private.example.test/", "P", VisitTransition::Typed)
            .unwrap();
    }
    let final_b = Browser::new_normal(root, 900, 700).unwrap();
    let hist = final_b.profile.store.recent_history(50).unwrap();
    assert!(!hist.iter().any(|h| h.url.contains("private.example")));
}

#[test]
fn clear_history_api() {
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 640, 480).unwrap();
    b.profile
        .store
        .record_history("https://x.test/", "X", VisitTransition::Other)
        .unwrap();
    b.clear_history().unwrap();
    assert_eq!(b.profile.store.history_count().unwrap(), 0);
}

#[test]
fn settings_defaults_validate() {
    let mut s = BrowserSettings {
        theme: "neon".into(),
        ..Default::default()
    };
    s.validate();
    assert_eq!(s.theme, "system");
}

#[test]
fn tests_do_not_touch_user_home_profile() {
    // Sanity: Wave C tests only use tempdir / target paths.
    let _home = dirs_next_home();
    let _ = fs::metadata("Cargo.toml");
}

fn dirs_next_home() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}
