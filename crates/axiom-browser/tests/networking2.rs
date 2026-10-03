//! Phase 3 Wave E — Networking 2.0 through the browser: navigation races, download
//! handoff, per-profile cookie and cache isolation, private cleanup and the request
//! detail page.

use std::time::{Duration, Instant};

use axiom_browser::{Browser, SecurityDisplay};
use axiom_download::DownloadState;
use axiom_net::test_server::{download_body, TestServer, DOWNLOAD_LEN};
use axiom_net::{CacheState, LogState};
use tempfile::tempdir;

fn wait(b: &mut Browser, what: &str, mut done: impl FnMut(&mut Browser) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(b) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        b.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn log_state(b: &Browser, path: &str) -> Option<LogState> {
    b.network()
        .recent_log(100)
        .into_iter()
        .rev()
        .find(|e| e.url.ends_with(path))
        .map(|e| e.state)
}

#[test]
fn newer_navigation_supersedes_a_slow_one() {
    let srv = TestServer::spawn_standard();
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    let slow = srv.url("/slow");
    let fast = srv.url("/ok");
    let a = b
        .window
        .tabs
        .active_tab_mut()
        .context
        .start_navigation(&slow);
    let deadline = Instant::now() + Duration::from_secs(5);
    while srv.requests_for("/slow").is_empty() {
        assert!(
            Instant::now() < deadline,
            "slow navigation never reached the server"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let second = b
        .window
        .tabs
        .active_tab_mut()
        .context
        .start_navigation(&fast);
    assert_ne!(a, second);
    assert_eq!(
        b.window.tabs.active_tab().context.pending_navigation(),
        Some(second)
    );
    wait(&mut b, "the second navigation to commit", |b| {
        b.window.tabs.active_tab().context.committed_navigation() == Some(second)
    });
    assert_eq!(b.window.tabs.active_tab().url(), fast);
    // Keep the loop running past the slow response: it must never commit.
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        b.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    let tab = b.window.tabs.active_tab();
    assert_eq!(tab.url(), fast, "stale navigation committed");
    assert_eq!(tab.context.committed_navigation(), Some(second));
    assert_eq!(tab.context.pending_navigation(), None);
    assert_eq!(
        log_state(&b, "/slow"),
        Some(LogState::Cancelled),
        "superseded in-flight request was not cancelled"
    );
}

#[test]
fn navigating_to_an_attachment_hands_off_to_the_download_manager() {
    let srv = TestServer::spawn_standard();
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    b.navigate_resolved(&srv.url("/download"));
    let record = b.downloads().list().pop().expect("download started");
    let done = b
        .downloads()
        .wait_until_settled(record.id, Duration::from_secs(10))
        .unwrap();
    assert_eq!(done.state, DownloadState::Completed, "{:?}", done.error);
    assert_eq!(done.filename.as_deref(), Some("evil name.bin"));
    let dest = done.destination.clone().unwrap();
    assert_eq!(dest.parent(), Some(dir.path().join("downloads").as_path()));
    assert_eq!(std::fs::read(&dest).unwrap(), download_body());
    assert_eq!(done.downloaded, DOWNLOAD_LEN as u64);
    // The document request stopped at headers; the body was fetched once, by the download.
    assert_eq!(srv.requests_for("/download").len(), 2);
    b.refresh_internal_pages();
    let page = b
        .internals
        .resolve("axiom://downloads")
        .unwrap()
        .html
        .clone();
    assert!(page.contains("evil name.bin"));
    assert!(page.contains("completed"));
}

#[test]
fn private_window_forgets_downloads_and_deletes_partials() {
    let srv = TestServer::spawn_standard();
    let mut b = Browser::new_private(800, 600).unwrap();
    b.navigate_resolved(&srv.url("/download/slow"));
    let record = b.downloads().list().pop().expect("download started");
    let deadline = Instant::now() + Duration::from_secs(5);
    let dest = loop {
        let r = b.downloads().get(record.id).unwrap();
        if let (Some(d), true) = (r.destination, r.downloaded > 0) {
            assert_eq!(r.state, DownloadState::Downloading);
            break d;
        }
        assert!(Instant::now() < deadline, "download never started writing");
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut partial = dest.as_os_str().to_owned();
    partial.push(".axiomdownload");
    let partial = std::path::PathBuf::from(partial);
    assert!(partial.exists(), "download streams to a partial file");
    drop(b);
    assert!(!partial.exists(), "partial file survived a private window");
    assert!(!dest.exists(), "unfinished private download was kept");
}

#[test]
fn cookies_are_isolated_between_profiles() {
    let srv = TestServer::spawn_standard();
    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    let mut a = Browser::new_normal(dir_a.path().to_path_buf(), 800, 600).unwrap();
    a.navigate_resolved(&srv.url("/set-cookie"));
    a.navigate_resolved(&srv.url("/echo-cookie"));
    let mut b = Browser::new_normal(dir_b.path().to_path_buf(), 800, 600).unwrap();
    b.navigate_resolved(&srv.url("/echo-cookie"));
    let mut p = Browser::new_private(800, 600).unwrap();
    p.navigate_resolved(&srv.url("/echo-cookie"));
    let echoes = srv.requests_for("/echo-cookie");
    assert_eq!(echoes.len(), 3);
    assert_eq!(echoes[0].header("cookie"), Some("session=abc"));
    assert_eq!(echoes[1].header("cookie"), None, "profile B saw A's cookie");
    assert_eq!(
        echoes[2].header("cookie"),
        None,
        "private window saw A's cookie"
    );
}

#[test]
fn http_cache_is_per_profile_and_persists_only_for_normal_profiles() {
    let srv = TestServer::spawn_standard();
    let dir = tempdir().unwrap();
    let url = srv.url("/cache/max-age");
    {
        let mut a = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
        assert!(a.network().cache().is_disk_backed());
        a.navigate_resolved(&url);
        let mut p = Browser::new_private(800, 600).unwrap();
        assert!(!p.network().cache().is_disk_backed());
        p.navigate_resolved(&url);
        assert_eq!(
            srv.requests_for("/cache/max-age").len(),
            2,
            "private window reused the normal profile's cache"
        );
    }
    assert!(dir.path().join("network-cache").join("objects").is_dir());
    // Reopening the normal profile serves the page from its disk cache.
    let mut a = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    a.navigate_resolved(&url);
    assert_eq!(srv.requests_for("/cache/max-age").len(), 2);
    let entry = a
        .network()
        .recent_log(10)
        .into_iter()
        .rev()
        .find(|e| e.url.ends_with("/cache/max-age"))
        .unwrap();
    assert_eq!(entry.cache_state, Some(CacheState::Hit));
}

#[test]
fn request_detail_page_shows_redirects_and_hides_secrets() {
    let srv = TestServer::spawn_standard();
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    b.navigate_resolved(&srv.url("/set-cookie"));
    b.navigate_resolved(&srv.url("/redirect?token=hunter2"));
    let entry = b
        .network()
        .recent_log(10)
        .into_iter()
        .rev()
        .find(|e| e.url.contains("/redirect"))
        .unwrap();
    let html = b.network_request_page(entry.id);
    assert!(html.contains("Redirects"));
    assert!(html.contains("/ok"), "redirect target missing");
    assert!(html.contains("document"));
    assert!(
        html.contains("not measured"),
        "unmeasured phases must say so"
    );
    assert!(!html.contains("hunter2"), "query string leaked");
    assert!(!html.contains("session=abc"), "cookie value leaked");

    // Navigating to the detail URL renders the same page as a trusted internal page.
    b.navigate_resolved(&format!("axiom://network/{}", entry.id.0));
    assert_eq!(
        b.window.tabs.active_tab().url(),
        format!("axiom://network/{}", entry.id.0)
    );
    assert_eq!(b.chrome_state().security, SecurityDisplay::Internal);
    // Unknown ids are handled, not a panic or a blank page.
    let missing = b.network_request_page(axiom_net::NetworkRequestId(u64::MAX));
    assert!(missing.contains("no longer in the profile's network log"));
}

#[test]
fn plain_http_pages_are_marked_insecure() {
    let srv = TestServer::spawn_standard();
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    b.navigate_resolved(&srv.url("/ok"));
    let state = b.chrome_state();
    assert_eq!(state.security, SecurityDisplay::Http);
    assert_eq!(state.connection, "Not encrypted (HTTP)");
    b.navigate_resolved(&srv.url("/status/500"));
    assert_eq!(b.chrome_state().security, SecurityDisplay::Http);
}
