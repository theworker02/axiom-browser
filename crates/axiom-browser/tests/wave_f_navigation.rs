//! Phase 3 Wave F — real navigation through the browser: omnibox classification and the
//! search provider service, redirects and the address bar, session and visit history
//! across failures, non-blocking navigation, link clicks, internal-page policy and the
//! persisted search engine preference. Everything runs against local test servers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axiom_browser::{
    Browser, BrowserDataStore, SearchProvider, SearchProviderService, VisitTransition,
};
use axiom_engine::{BrowsingContext, NavigationEventKind, NavigationState};
use axiom_net::test_server::{TestResponse, TestServer};
use tempfile::tempdir;

fn wait(b: &mut Browser, what: &str, mut done: impl FnMut(&mut Browser) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(b) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        b.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn visited(b: &Browser) -> Vec<(String, VisitTransition)> {
    b.profile
        .store
        .recent_history(50)
        .unwrap()
        .into_iter()
        .map(|r| (r.url, r.transition))
        .collect()
}

fn html(body: &str) -> TestResponse {
    TestResponse::ok(
        format!("<html><head><title>t</title></head><body>{body}</body></html>"),
        "text/html",
    )
}

fn node(b: &Browser, id: &str) -> axiom_dom::NodeId {
    b.window
        .tabs
        .active_tab()
        .context
        .page
        .document
        .borrow()
        .get_element_by_id(id)
        .unwrap_or_else(|| panic!("no element #{id}"))
}

#[test]
fn typed_search_goes_to_the_selected_provider_with_an_encoded_query() {
    let srv = TestServer::spawn(|req| match req.path.split('?').next().unwrap_or("") {
        "/search" => html("<p>results</p>"),
        _ => TestResponse::status(404, "no"),
    });
    let template = format!("{}?q={{searchTerms}}", srv.url("/search"));
    let local = SearchProvider::custom("Local", &template).expect("valid template");
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    b.search = SearchProviderService::with_selection("custom", Some(local));

    b.navigate("rust programming language");
    let expected = srv.url("/search?q=rust+programming+language");
    assert_eq!(
        srv.requests_for("/search?q=rust+programming+language")
            .len(),
        1
    );
    assert_eq!(b.window.tabs.active_tab().url(), expected);
    assert_eq!(b.chrome.omnibox.text, expected);
    assert!(visited(&b).contains(&(expected, VisitTransition::Typed)));

    b.navigate("c++ \"templates\" & more?");
    assert_eq!(
        srv.requests_for("/search?q=c%2B%2B+%22templates%22+%26+more%3F")
            .len(),
        1
    );
}

#[test]
fn typed_ip_and_port_navigates_directly_instead_of_searching() {
    let srv = TestServer::spawn_standard();
    let mut b = Browser::new_private(800, 600).unwrap();
    let typed = format!("127.0.0.1:{}/ok", srv.port());
    b.navigate(&typed);
    assert_eq!(srv.requests_for("/ok").len(), 1);
    assert_eq!(b.window.tabs.active_tab().url(), srv.url("/ok"));
}

#[test]
fn redirects_update_the_address_bar_and_record_one_visit_for_the_final_url() {
    let srv = TestServer::spawn_standard();
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    b.navigate(&srv.url("/redirect-chain"));
    let final_url = srv.url("/ok");
    assert_eq!(b.window.tabs.active_tab().url(), final_url);
    assert_eq!(b.chrome.omnibox.text, final_url);
    let visits = visited(&b);
    assert_eq!(visits, vec![(final_url, VisitTransition::Typed)]);
    // One session history entry: back returns to the new tab page.
    assert!(b.window.tabs.active_tab().context.can_go_back());
    b.back();
    assert_eq!(b.window.tabs.active_tab().url(), "axiom://newtab");
    assert!(!b.window.tabs.active_tab().context.can_go_back());
}

#[test]
fn failed_navigation_keeps_history_and_reload_retries_the_failed_url() {
    let up = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&up);
    let srv = TestServer::spawn(move |req| match req.path.as_str() {
        "/flaky" if !flag.load(Ordering::SeqCst) => TestResponse::Close,
        "/flaky" => html("<p id=back>back online</p>"),
        _ => html("<p>first</p>"),
    });
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    let first = srv.url("/first");
    let flaky = srv.url("/flaky");
    b.navigate_resolved(&first);

    b.navigate(&flaky);
    let tab = b.window.tabs.active_tab();
    assert_eq!(tab.context.page.url, "axiom://network-error");
    assert_eq!(tab.url(), flaky, "address bar lost the URL that failed");
    assert_eq!(b.chrome.omnibox.text, flaky);
    assert_eq!(tab.context.navigation_state(), NavigationState::Failed);
    assert_eq!(tab.context.failed_url(), Some(flaky.as_str()));
    assert!(
        !visited(&b).iter().any(|(u, _)| *u == flaky),
        "a failed navigation was recorded as a visit"
    );

    // Back returns to the page before the failure.
    b.back();
    assert_eq!(b.window.tabs.active_tab().url(), first);
    // Forward retries the failed URL (still down).
    b.forward();
    assert_eq!(b.window.tabs.active_tab().url(), flaky);
    assert_eq!(
        b.window.tabs.active_tab().context.page.url,
        "axiom://network-error"
    );

    // Reload retries it once the server is back, without adding a history entry.
    up.store(true, Ordering::SeqCst);
    b.reload_or_stop();
    let tab = b.window.tabs.active_tab();
    assert_eq!(tab.context.page.url, flaky);
    assert_eq!(tab.url(), flaky);
    assert!(tab.context.failed_url().is_none());
    assert!(tab.context.last_error.is_none());
    let _ = node(&b, "back");
    b.back();
    assert_eq!(b.window.tabs.active_tab().url(), first);
}

#[test]
fn background_navigation_returns_immediately_and_commits_from_tick() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/slow-page" => html("<p>slow</p>").delayed(400),
        _ => html("<p>fast</p>"),
    });
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    b.set_background_navigation(true);
    let url = srv.url("/slow-page");
    let started = Instant::now();
    b.navigate(&url);
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "navigate blocked for {:?}",
        started.elapsed()
    );
    let tab = b.window.tabs.active_tab();
    assert!(tab.is_loading());
    assert_eq!(tab.context.navigation_state(), NavigationState::Connecting);
    // A typed navigation shows its destination while it loads; the document is unchanged.
    assert_eq!(b.chrome.omnibox.text, url);
    assert_eq!(tab.context.page.url, "axiom://newtab");

    wait(&mut b, "the slow page to commit", |b| {
        !b.window.tabs.active_tab().is_loading()
    });
    assert_eq!(b.window.tabs.active_tab().url(), url);
    assert_eq!(b.chrome.omnibox.text, url);
    assert!(visited(&b).contains(&(url, VisitTransition::Typed)));
}

#[test]
fn a_newer_background_navigation_supersedes_the_pending_one() {
    let srv = TestServer::spawn_standard();
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    b.set_background_navigation(true);
    let slow = srv.url("/slow");
    let fast = srv.url("/ok");
    b.navigate(&slow);
    b.navigate(&fast);
    wait(&mut b, "the fast page", |b| {
        b.window.tabs.active_tab().url() == fast && !b.window.tabs.active_tab().is_loading()
    });
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        b.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(b.window.tabs.active_tab().url(), fast);
    assert_eq!(b.chrome.omnibox.text, fast);
    let visits = visited(&b);
    assert!(visits.iter().any(|(u, _)| *u == fast));
    assert!(
        !visits.iter().any(|(u, _)| *u == slow),
        "stale navigation recorded"
    );
}

#[test]
fn link_clicks_are_recorded_and_update_the_address_bar() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/next" => html("<p>next</p>"),
        _ => html(r#"<a id="next" href="/next">next</a>"#),
    });
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    b.navigate_resolved(&srv.url("/"));
    let link = node(&b, "next");
    b.click_node(link);
    let next = srv.url("/next");
    assert_eq!(b.window.tabs.active_tab().url(), next);
    assert_eq!(b.chrome.omnibox.text, next);
    assert!(visited(&b).contains(&(next, VisitTransition::Link)));
    b.back();
    assert_eq!(b.window.tabs.active_tab().url(), srv.url("/"));
}

#[test]
fn web_content_cannot_open_internal_pages() {
    let srv = TestServer::spawn(|_| {
        html(r#"<a id="evil" href="axiom://settings/search?provider=google">settings</a>"#)
    });
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    let page = srv.url("/");
    b.navigate_resolved(&page);
    let before = b.search.default_provider().id.clone();
    let link = node(&b, "evil");
    b.click_node(link);
    assert_eq!(b.window.tabs.active_tab().url(), page);
    assert_eq!(b.search.default_provider().id, before);
}

#[test]
fn search_engine_choice_persists_per_profile_and_private_windows_inherit_it() {
    let dir = tempdir().unwrap();
    {
        let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
        assert_eq!(b.search.default_provider().id, "duckduckgo");
        // Typed settings URL applies the choice.
        b.navigate("axiom://settings/search?provider=google");
        assert_eq!(b.search.default_provider().id, "google");
        assert_eq!(b.window.tabs.active_tab().url(), "axiom://settings/search");
        // A link on the trusted settings page applies another one, in place.
        let link = node(&b, "provider-bing");
        b.click_node(link);
        assert_eq!(b.search.default_provider().id, "bing");
        assert_eq!(b.window.tabs.active_tab().url(), "axiom://settings/search");
        b.set_search_provider("google").unwrap();
        assert!(b.set_search_provider("nope").is_err());
        // Readable without the profile lock while the profile is open.
        let read = BrowserDataStore::read_settings(dir.path().to_path_buf())
            .unwrap()
            .unwrap();
        assert_eq!(read.search_provider_id, "google");
    }
    let b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    assert_eq!(b.search.default_provider().id, "google");
    assert!(b
        .search
        .search_url("hello world")
        .starts_with("https://www.google.com/search?q=hello+world"));

    let mut private = Browser::new_private_inheriting(&b.settings, 800, 600).unwrap();
    assert_eq!(private.search.default_provider().id, "google");
    private.set_search_provider("duckduckgo").unwrap();
    drop(private);
    let reread = BrowserDataStore::read_settings(dir.path().to_path_buf())
        .unwrap()
        .unwrap();
    assert_eq!(
        reread.search_provider_id, "google",
        "a private window changed the normal profile"
    );
}

#[test]
fn axiom_search_is_an_honest_placeholder() {
    let mut b = Browser::new_private(800, 600).unwrap();
    b.set_search_provider("axiom").unwrap();
    b.navigate("rust <b>browser</b>");
    let tab = b.window.tabs.active_tab();
    assert!(tab.url().starts_with("axiom://search?q="));
    let text = {
        let doc = tab.context.page.document.borrow();
        doc.text_content(doc.body().expect("body"))
    };
    assert!(
        text.contains("rust <b>browser</b>"),
        "query not shown as text"
    );
    assert!(text.contains("not available yet"));
}

#[test]
fn downloads_leave_the_current_page_and_history_alone() {
    let srv = TestServer::spawn_standard();
    let dir = tempdir().unwrap();
    let mut b = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    let ok = srv.url("/ok");
    b.navigate_resolved(&ok);
    b.navigate_resolved(&srv.url("/download"));
    assert_eq!(b.downloads().list().len(), 1);
    let tab = b.window.tabs.active_tab();
    assert_eq!(tab.url(), ok);
    assert!(!tab.is_loading());
    assert!(!tab.context.can_go_forward());
    assert!(!visited(&b).iter().any(|(u, _)| u.ends_with("/download")));
}

#[test]
fn navigation_events_report_the_lifecycle_in_order() {
    let srv = TestServer::spawn_standard();
    let mut ctx = BrowsingContext::new(800, 600);
    let id = ctx.navigate_to(&srv.url("/redirect"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while ctx.navigation_state() != NavigationState::Completed {
        assert!(Instant::now() < deadline, "load never completed");
        ctx.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
    let events = ctx.take_navigation_events();
    let kinds: Vec<_> = events.iter().map(|e| e.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            NavigationEventKind::Started,
            NavigationEventKind::Committed { redirected: true },
            NavigationEventKind::Interactive,
            NavigationEventKind::Completed,
        ]
    );
    assert!(events.iter().all(|e| e.id == id));
    assert_eq!(events[0].url, srv.url("/redirect"));
    assert_eq!(events[1].url, srv.url("/ok"));

    // A failure is terminal and names a stable error kind, never library text.
    let refused = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        format!("http://127.0.0.1:{port}/")
    };
    ctx.navigate_to(&refused);
    let events = ctx.take_navigation_events();
    let last = events.last().unwrap();
    assert!(last.is_terminal());
    assert!(
        matches!(last.kind, NavigationEventKind::Failed { .. }),
        "{last:?}"
    );
    assert_eq!(ctx.display_url(), refused);
}

#[test]
fn requests_identify_as_axiom_and_send_accept_language() {
    let srv = TestServer::spawn_standard();
    let mut b = Browser::new_private(800, 600).unwrap();
    b.navigate_resolved(&srv.url("/ok"));
    let req = &srv.requests_for("/ok")[0];
    let ua = req.header("user-agent").unwrap();
    assert!(ua.starts_with("Axiom/"), "{ua}");
    for token in ["Mozilla", "Chrome", "Safari", "AppleWebKit"] {
        assert!(!ua.contains(token), "user agent impersonates {token}: {ua}");
    }
    assert_eq!(req.header("accept-language"), Some("en-US,en;q=0.9"));
    assert_eq!(req.header("accept-encoding"), Some("gzip, deflate, br"));
}
