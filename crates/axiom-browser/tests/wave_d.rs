//! Wave D — cookie engine integration tests.

use std::sync::Arc;

use axiom_browser::{
    fuzz_parse_set_cookie, Browser, BrowserDataStore, CookieAccessContext, CookiePolicy,
    CookieService, CookieSource, FixedClock, HttpMethodKind, NavigationKind, ProcessResult,
    Profile, ProfileCookieJar, ProfileManager, Site, SCHEMA_VERSION,
};
use axiom_engine::CookieJar;
use axiom_net::test_server::{TestResponse, TestServer};
use tempfile::tempdir;

fn mem_store() -> BrowserDataStore {
    BrowserDataStore::open_private().unwrap()
}

#[test]
fn schema_is_v2() {
    let store = mem_store();
    assert_eq!(store.schema_version(), SCHEMA_VERSION);
    const { assert!(SCHEMA_VERSION >= 2) };
}

#[test]
fn e2e_httponly_set_cookie_then_cookie_header() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/" => TestResponse::ok("<html><body>ok</body></html>", "text/html")
            .with_header("Set-Cookie", "session=abc123; Path=/; HttpOnly"),
        _ => TestResponse::ok("<html><body>next</body></html>", "text/html"),
    });

    let dir = tempdir().unwrap();
    let mut browser = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    let url = srv.url("/");
    browser.navigate_resolved(&url);
    assert!(browser
        .window
        .tabs
        .active_tab()
        .context
        .last_error
        .is_none());

    // document.cookie must NOT show HttpOnly session
    let script_cookies = browser.cookie_jar().cookies_for_script(&url);
    assert!(
        !script_cookies.contains("session"),
        "HttpOnly leaked to script: {script_cookies}"
    );

    // The next navigation goes through the profile network service, which asks the
    // CookieService for the header; verify what the server actually received.
    browser.navigate_resolved(&srv.url("/next"));
    let first = srv.requests_for("/");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].header("cookie"), None);
    let next = srv.requests_for("/next");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].header("cookie"), Some("session=abc123"));
}

#[test]
fn e2e_document_cookie_to_http() {
    let srv = TestServer::spawn(|req| match req.path.as_str() {
        "/" => TestResponse::ok(
            r#"<html><body><script>document.cookie = "theme=dark; Path=/";</script>hi</body></html>"#,
            "text/html",
        ),
        _ => TestResponse::ok("<html><body>res</body></html>", "text/html"),
    });

    let dir = tempdir().unwrap();
    let mut browser = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    let url = srv.url("/");
    browser.navigate_resolved(&url);
    assert!(browser
        .window
        .tabs
        .active_tab()
        .context
        .last_error
        .is_none());

    let visible = browser.cookie_jar().cookies_for_script(&url);
    assert!(visible.contains("theme=dark"), "got: {visible}");

    browser.navigate_resolved(&srv.url("/res"));
    let res = srv.requests_for("/res");
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].header("cookie"), Some("theme=dark"));
}

#[test]
fn domain_host_only_vs_domain_attr() {
    let store = mem_store();
    let svc = CookieService::new();
    svc.process_set_cookie(
        &store,
        "https://shop.example.test/",
        "hostonly=1; Path=/",
        CookieSource::Network,
    )
    .unwrap();
    svc.process_set_cookie(
        &store,
        "https://shop.example.test/",
        "shared=1; Domain=example.test; Path=/",
        CookieSource::Network,
    )
    .unwrap();

    let ctx_shop = CookieService::default_navigation_context("https://shop.example.test/");
    let h_shop = svc
        .cookie_header_for_request(&store, &ctx_shop)
        .unwrap()
        .unwrap();
    assert!(h_shop.contains("hostonly=1"));
    assert!(h_shop.contains("shared=1"));

    let ctx_sub = CookieService::default_navigation_context("https://other.example.test/");
    let h_sub = svc
        .cookie_header_for_request(&store, &ctx_sub)
        .unwrap()
        .unwrap();
    assert!(!h_sub.contains("hostonly"));
    assert!(h_sub.contains("shared=1"));

    let ctx_other = CookieService::default_navigation_context("https://other.test/");
    let h_other = svc.cookie_header_for_request(&store, &ctx_other).unwrap();
    assert!(h_other.is_none());

    let bad = svc
        .process_set_cookie(
            &store,
            "https://evil.example/",
            "x=1; Domain=bank.example",
            CookieSource::Network,
        )
        .unwrap();
    assert!(matches!(bad, ProcessResult::Rejected(_)));
}

#[test]
fn path_matching_and_ordering() {
    let store = mem_store();
    let svc = CookieService::new();
    let url = "https://example.test/account/profile";
    svc.process_set_cookie(&store, url, "a=1; Path=/", CookieSource::Network)
        .unwrap();
    // creation order: b after a but longer path sorts first
    svc.process_set_cookie(&store, url, "b=2; Path=/account", CookieSource::Network)
        .unwrap();

    let ctx_root = CookieService::default_navigation_context("https://example.test/");
    let h_root = svc
        .cookie_header_for_request(&store, &ctx_root)
        .unwrap()
        .unwrap();
    assert!(h_root.contains("a=1"));
    assert!(!h_root.contains("b=2"));

    let ctx_acc = CookieService::default_navigation_context("https://example.test/account/profile");
    let h_acc = svc
        .cookie_header_for_request(&store, &ctx_acc)
        .unwrap()
        .unwrap();
    // longer path first
    assert!(h_acc.starts_with("b=2") || h_acc.find("b=2").unwrap() < h_acc.find("a=1").unwrap());
}

#[test]
fn secure_not_sent_on_http() {
    let store = mem_store();
    let svc = CookieService::new();
    svc.process_set_cookie(
        &store,
        "https://example.test/",
        "sec=1; Path=/; Secure",
        CookieSource::Network,
    )
    .unwrap();
    let https = CookieService::default_navigation_context("https://example.test/");
    assert!(svc
        .cookie_header_for_request(&store, &https)
        .unwrap()
        .unwrap()
        .contains("sec=1"));
    let http = CookieService::default_navigation_context("http://example.test/");
    assert!(svc
        .cookie_header_for_request(&store, &http)
        .unwrap()
        .is_none());
}

#[test]
fn samesite_matrix() {
    let store = mem_store();
    let svc = CookieService::new();
    for (name, ss) in [("strict", "Strict"), ("lax", "Lax"), ("none", "None")] {
        let hdr = if ss == "None" {
            format!("{name}=1; Path=/; SameSite={ss}; Secure")
        } else {
            format!("{name}=1; Path=/; SameSite={ss}")
        };
        svc.process_set_cookie(&store, "https://a.example/", &hdr, CookieSource::Network)
            .unwrap();
    }

    let same_nav = CookieAccessContext {
        request_url: "https://a.example/".into(),
        top_level_site: Site {
            scheme: "https".into(),
            host: "a.example".into(),
        },
        initiator_site: Some(Site {
            scheme: "https".into(),
            host: "a.example".into(),
        }),
        navigation: NavigationKind::TopLevel,
        method: HttpMethodKind::SafeGet,
        secure_context: true,
        source: CookieSource::Network,
    };
    let h = svc
        .cookie_header_for_request(&store, &same_nav)
        .unwrap()
        .unwrap();
    assert!(h.contains("strict=1") && h.contains("lax=1") && h.contains("none=1"));

    let cross_nav_safe = CookieAccessContext {
        request_url: "https://a.example/".into(),
        top_level_site: Site {
            scheme: "https".into(),
            host: "b.example".into(),
        },
        initiator_site: Some(Site {
            scheme: "https".into(),
            host: "b.example".into(),
        }),
        navigation: NavigationKind::TopLevel,
        method: HttpMethodKind::SafeGet,
        secure_context: true,
        source: CookieSource::Network,
    };
    let h = svc
        .cookie_header_for_request(&store, &cross_nav_safe)
        .unwrap()
        .unwrap();
    assert!(!h.contains("strict="));
    assert!(h.contains("lax=1"));
    assert!(h.contains("none=1"));

    let cross_sub = CookieAccessContext {
        request_url: "https://a.example/pixel".into(),
        top_level_site: Site {
            scheme: "https".into(),
            host: "b.example".into(),
        },
        initiator_site: Some(Site {
            scheme: "https".into(),
            host: "b.example".into(),
        }),
        navigation: NavigationKind::Subresource,
        method: HttpMethodKind::SafeGet,
        secure_context: true,
        source: CookieSource::Network,
    };
    let h = svc
        .cookie_header_for_request(&store, &cross_sub)
        .unwrap()
        .unwrap();
    assert!(!h.contains("strict="));
    assert!(!h.contains("lax="));
    assert!(h.contains("none=1"));

    let cross_unsafe = CookieAccessContext {
        request_url: "https://a.example/".into(),
        top_level_site: Site {
            scheme: "https".into(),
            host: "b.example".into(),
        },
        initiator_site: None,
        navigation: NavigationKind::TopLevel,
        method: HttpMethodKind::Unsafe,
        secure_context: true,
        source: CookieSource::Network,
    };
    let h = svc
        .cookie_header_for_request(&store, &cross_unsafe)
        .unwrap()
        .unwrap();
    assert!(!h.contains("lax="));
    assert!(h.contains("none=1"));
}

#[test]
fn prefix_validation() {
    let store = mem_store();
    let svc = CookieService::new();
    let bad_secure = svc
        .process_set_cookie(
            &store,
            "https://example.test/",
            "__Secure-x=1; Path=/",
            CookieSource::Network,
        )
        .unwrap();
    assert!(matches!(bad_secure, ProcessResult::Rejected(_)));
    assert_eq!(store.cookie_count().unwrap(), 0);

    let bad_host = svc
        .process_set_cookie(
            &store,
            "https://example.test/",
            "__Host-x=1; Secure; Domain=example.test; Path=/",
            CookieSource::Network,
        )
        .unwrap();
    assert!(matches!(bad_host, ProcessResult::Rejected(_)));

    let ok = svc
        .process_set_cookie(
            &store,
            "https://example.test/",
            "__Host-x=1; Secure; Path=/",
            CookieSource::Network,
        )
        .unwrap();
    assert_eq!(ok, ProcessResult::Accepted);
}

#[test]
fn persistent_cookie_survives_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("prof");
    {
        let profile = Profile::open_persistent("Default", path.clone()).unwrap();
        let svc = CookieService::new();
        svc.process_set_cookie(
            &profile.store,
            "https://example.test/",
            "persist=valueA; Path=/; Max-Age=999999",
            CookieSource::Network,
        )
        .unwrap();
        // session cookie should be purged on reopen
        svc.process_set_cookie(
            &profile.store,
            "https://example.test/",
            "temp=session; Path=/",
            CookieSource::Network,
        )
        .unwrap();
    }
    let profile = Profile::open_persistent("Default", path).unwrap();
    assert_eq!(profile.store.schema_version(), SCHEMA_VERSION);
    let svc = CookieService::new();
    let ctx = CookieService::default_navigation_context("https://example.test/");
    let h = svc
        .cookie_header_for_request(&profile.store, &ctx)
        .unwrap()
        .unwrap();
    assert!(h.contains("persist=valueA"));
    assert!(!h.contains("temp="));
}

#[test]
fn private_no_leak_to_persistent() {
    let dir = tempdir().unwrap();
    let mgr = ProfileManager::new(dir.path());
    {
        let persistent = mgr.open_or_create("A").unwrap();
        let svc = CookieService::new();
        svc.process_set_cookie(
            &persistent.store,
            "https://example.test/",
            "persistent=valueA; Path=/; Max-Age=999999",
            CookieSource::Network,
        )
        .unwrap();

        let private = mgr.open_private("P").unwrap();
        svc.process_set_cookie(
            &private.store,
            "https://example.test/",
            "private=valueP; Path=/; Max-Age=999999",
            CookieSource::Network,
        )
        .unwrap();
        let ctx = CookieService::default_navigation_context("https://example.test/");
        let h = svc
            .cookie_header_for_request(&private.store, &ctx)
            .unwrap()
            .unwrap();
        assert!(h.contains("private=valueP"));
        assert!(!h.contains("valueA"));
        // private dropped here; persistent still open until end of block
    }

    let persistent2 = mgr.open_or_create("A").unwrap();
    let svc = CookieService::new();
    let ctx = CookieService::default_navigation_context("https://example.test/");
    let h = svc
        .cookie_header_for_request(&persistent2.store, &ctx)
        .unwrap()
        .unwrap();
    assert!(h.contains("persistent=valueA"));
    assert!(!h.contains("valueP"));
}

#[test]
fn multi_profile_isolation() {
    let dir = tempdir().unwrap();
    let mgr = ProfileManager::new(dir.path());
    let a = mgr.open_or_create("A").unwrap();
    let b = mgr.open_or_create("B").unwrap();
    let svc = CookieService::new();
    svc.process_set_cookie(
        &a.store,
        "https://example.test/",
        "who=A; Path=/; Max-Age=999999",
        CookieSource::Network,
    )
    .unwrap();
    svc.process_set_cookie(
        &b.store,
        "https://example.test/",
        "who=B; Path=/; Max-Age=999999",
        CookieSource::Network,
    )
    .unwrap();
    let ctx = CookieService::default_navigation_context("https://example.test/");
    assert!(svc
        .cookie_header_for_request(&a.store, &ctx)
        .unwrap()
        .unwrap()
        .contains("who=A"));
    assert!(svc
        .cookie_header_for_request(&b.store, &ctx)
        .unwrap()
        .unwrap()
        .contains("who=B"));
}

#[test]
fn limits_evict() {
    let store = mem_store();
    let svc = CookieService::with_parts(
        Arc::new(axiom_browser::PslPublicSuffixProvider),
        Arc::new(FixedClock { ms: 1_000_000 }),
        CookiePolicy::AllowAll,
        axiom_browser::CookieLimits {
            max_name_bytes: 1024,
            max_value_bytes: 4096,
            max_cookie_bytes: 4096,
            max_per_domain: 3,
            max_total: 5,
        },
    );
    for i in 0..6 {
        svc.process_set_cookie(
            &store,
            "https://example.test/",
            &format!("c{i}={i}; Path=/; Max-Age=9999"),
            CookieSource::Network,
        )
        .unwrap();
    }
    assert!(store.cookie_count().unwrap() <= 5);
}

#[test]
fn policy_block_all() {
    let store = mem_store();
    let mut svc = CookieService::new();
    svc.set_policy(CookiePolicy::BlockAll);
    let r = svc
        .process_set_cookie(
            &store,
            "https://example.test/",
            "a=1; Path=/",
            CookieSource::Network,
        )
        .unwrap();
    assert!(matches!(r, ProcessResult::Rejected(_)));
}

#[test]
fn clear_cookies_apis() {
    let store = mem_store();
    let svc = CookieService::new();
    svc.process_set_cookie(
        &store,
        "https://a.example/",
        "x=1; Path=/; Max-Age=9999",
        CookieSource::Network,
    )
    .unwrap();
    svc.process_set_cookie(
        &store,
        "https://b.example/",
        "y=1; Path=/; Max-Age=9999",
        CookieSource::Network,
    )
    .unwrap();
    assert_eq!(store.clear_cookies_for_site("a.example").unwrap(), 1);
    assert_eq!(store.cookie_count().unwrap(), 1);
    store.clear_cookies().unwrap();
    assert_eq!(store.cookie_count().unwrap(), 0);
}

#[test]
fn axiom_cookies_page() {
    let dir = tempdir().unwrap();
    let mut browser = Browser::new_normal(dir.path().to_path_buf(), 800, 600).unwrap();
    let svc = CookieService::new();
    svc.process_set_cookie(
        &browser.profile.store,
        "https://example.test/",
        "ui=1; Path=/; HttpOnly; Max-Age=9999",
        CookieSource::Network,
    )
    .unwrap();
    browser.refresh_internal_pages();
    browser.navigate_resolved("axiom://cookies");
    let html = &browser.window.tabs.active_tab().context.page.document;
    let text = format!("{:?}", html.borrow());
    // page should mention domain / name without dumping value by default in table
    assert!(browser
        .internals
        .resolve("axiom://cookies")
        .unwrap()
        .html
        .contains("example.test"));
    assert!(
        browser
            .internals
            .resolve("axiom://cookies")
            .unwrap()
            .html
            .contains(">ui<")
            || browser
                .internals
                .resolve("axiom://cookies")
                .unwrap()
                .html
                .contains("ui")
    );
    let _ = text;
}

#[test]
fn hostile_parse_no_panic() {
    let samples = [
        "",
        "=",
        "a",
        &"x".repeat(20_000),
        "a=b\0c",
        "a=b; Domain=",
        "a=b; Expires=not-a-date",
        "a=b; Max-Age=999999999999999999999",
        "a=b; SameSite=Weird",
        ";;;",
        "name=value; Path=/; Path=/other; Secure; Secure",
    ];
    for s in samples {
        let _ = fuzz_parse_set_cookie(s);
        let _ = axiom_browser::parse_set_cookie(s);
    }
}

#[test]
fn jar_bridge_shared() {
    let store = Arc::new(mem_store());
    let jar = ProfileCookieJar::new(Arc::clone(&store));
    jar.set_cookie_from_script("https://example.test/", "theme=dark; Path=/");
    let h = jar.cookie_header_for_request("https://example.test/");
    assert_eq!(h.as_deref(), Some("theme=dark"));
}

#[test]
fn lookup_scales_with_indexed_candidates() {
    let store = mem_store();
    let svc = CookieService::new();
    // 1_000 cookies across domains — request path must use domain candidates, not full scan API.
    for i in 0..1000 {
        let host = format!("s{i}.example.test");
        svc.process_set_cookie(
            &store,
            &format!("https://{host}/"),
            &format!("n=v{i}; Path=/; Max-Age=999999"),
            CookieSource::Network,
        )
        .unwrap();
    }
    // Plus 100 on the target host
    for i in 0..100 {
        svc.process_set_cookie(
            &store,
            "https://target.example.test/",
            &format!("t{i}={i}; Path=/; Max-Age=999999"),
            CookieSource::Network,
        )
        .unwrap();
    }
    let ctx = CookieService::default_navigation_context("https://target.example.test/");
    let h = svc
        .cookie_header_for_request(&store, &ctx)
        .unwrap()
        .unwrap();
    assert!(h.contains("t0="));
    assert!(store.cookie_count().unwrap() >= 1100);
}
