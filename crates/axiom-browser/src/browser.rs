//! Top-level Browser object owning a profile, chrome, and tab manager.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::chrome::{
    connection_detail, security_display, window_title_for, BrowserChrome, ChromeControl,
    ChromeState, FocusOwner, ReloadStopMode,
};
use crate::classifier::{ClassifiedInput, OmniboxInputClassifier};
use crate::cookie_bridge::ProfileCookieJar;
use crate::history_repo::VisitTransition;
use crate::internal::InternalPageRegistry;
use crate::origin::Origin;
use crate::profile::{Profile, ProfileId};
use crate::search::{query_param, SearchProviderService, UnknownSearchProvider};
use crate::search_pages::{
    render_axiom_search_page, render_search_settings_page, render_settings_page,
};
use crate::session::{SessionSnapshot, SessionTab};
use crate::settings_repo::BrowserSettings;
use crate::storage_bridge::make_storage_binder;
use crate::storage_service::ProfileStorage;
use crate::store::StoreError;
use crate::tab::TabId;
use crate::tab_manager::TabManager;

use std::sync::Arc;

use axiom_download::{DownloadConfig, DownloadManager, DownloadRecord};
use axiom_engine::NavigationEventKind;
use axiom_net::{
    CacheLimits, NetworkRequestId, NetworkService, NetworkServiceConfig, RequestScheduler,
    SchedulerConfig,
};

pub struct BrowserWindow {
    pub width: u32,
    pub height: u32,
    pub tabs: TabManager,
}

pub struct Browser {
    pub profile: Profile,
    pub window: BrowserWindow,
    pub chrome: BrowserChrome,
    /// Omnibox searches go through the profile's selected provider.
    pub search: SearchProviderService,
    pub internals: InternalPageRegistry,
    pub show_hud: bool,
    pub settings: BrowserSettings,
    /// Tabs navigate without blocking the caller (the desktop UI thread).
    background_navigation: bool,
    last_session_write: Instant,
    session_dirty: bool,
    cookie_jar: Arc<ProfileCookieJar>,
    storage_profile: Arc<ProfileStorage>,
    network: Arc<NetworkService>,
    scheduler: Arc<RequestScheduler>,
    downloads: DownloadManager,
}

impl Drop for Browser {
    fn drop(&mut self) {
        // Exit-cleanup is a profile-owned policy. Private profiles are already in-memory,
        // so only durable profiles need repository mutations here.
        if !self.is_private() {
            if self.settings.clear_history_on_exit {
                let _ = self.profile.store.clear_history();
            }
            if self.settings.clear_cookies_on_exit {
                let _ = self.profile.store.clear_cookies();
            }
        }
        // Downloads first (they run on the scheduler; a private profile forgets them and
        // deletes partial files), then workers before the service so no request outlives
        // the profile; a private profile's cache is cleared by the service shutdown.
        self.downloads.shutdown();
        self.scheduler.shutdown();
        self.network.shutdown();
    }
}

/// One network service, HTTP cache and scheduler per profile. Normal profiles get a
/// disk-backed cache under the profile directory; private profiles get a memory cache
/// that is cleared on shutdown and never shared with normal ones.
fn profile_network(
    profile: &Profile,
    cookies: Arc<ProfileCookieJar>,
) -> (Arc<NetworkService>, Arc<RequestScheduler>) {
    let private = profile.is_private();
    let cache_dir = profile.paths.as_ref().map(|p| p.network_cache_dir());
    let config = NetworkServiceConfig {
        persistent_cache: !private,
        cache_limits: if cache_dir.is_some() && !private {
            CacheLimits {
                max_total_bytes: 256 * 1024 * 1024,
                max_entry_bytes: 16 * 1024 * 1024,
            }
        } else {
            CacheLimits::default()
        },
        cache_dir: if private { None } else { cache_dir },
        label: if private { "private" } else { "persistent" }.into(),
        ..NetworkServiceConfig::default()
    };
    let network = Arc::new(NetworkService::new(config).with_cookies(cookies));
    let scheduler = Arc::new(RequestScheduler::new(
        Arc::clone(&network),
        SchedulerConfig::default(),
    ));
    (network, scheduler)
}

/// Downloads go to the configured directory, else the profile's `downloads/`, else (a
/// private profile) a per-process temporary directory.
fn download_directory(profile: &Profile, settings: &BrowserSettings) -> PathBuf {
    if !settings.download_directory.trim().is_empty() {
        return PathBuf::from(settings.download_directory.trim());
    }
    match &profile.paths {
        Some(p) => p.downloads_dir(),
        None => std::env::temp_dir().join(format!("axiom-downloads-{}", profile.id())),
    }
}

impl Browser {
    pub fn new_normal(data_dir: PathBuf, width: u32, height: u32) -> Result<Self, StoreError> {
        let profile = Profile::open_persistent("Default", data_dir)?;
        Ok(Self::with_profile(profile, width, height))
    }

    pub fn new_private(width: u32, height: u32) -> Result<Self, StoreError> {
        let profile = Profile::private("Private")?;
        Ok(Self::with_profile(profile, width, height))
    }

    /// A private browser that starts from a normal profile's preferences (search
    /// provider, homepage, theme). Changes made in it stay in its memory-only store.
    pub fn new_private_inheriting(
        parent: &BrowserSettings,
        width: u32,
        height: u32,
    ) -> Result<Self, StoreError> {
        let profile = Profile::private("Private")?;
        profile.store.save_settings(&parent.for_private())?;
        Ok(Self::with_profile(profile, width, height))
    }

    pub fn with_profile(profile: Profile, width: u32, height: u32) -> Self {
        let settings = match profile.store.load_settings() {
            Ok(s) => s,
            Err(e) => {
                log::warn!(target: "axiom_persist", "settings unreadable, using defaults: {e}");
                BrowserSettings::default()
            }
        };
        let search = settings.search_service();
        let show_hud = settings.performance_hud;
        let background_navigation = settings.background_networking;
        let cookie_jar = ProfileCookieJar::new(Arc::clone(&profile.store));
        let storage_profile = ProfileStorage::new(Arc::clone(&profile.store));
        let (network, scheduler) = profile_network(&profile, Arc::clone(&cookie_jar));
        let downloads = DownloadManager::new(
            Arc::clone(&scheduler),
            DownloadConfig {
                private: profile.is_private(),
                ..DownloadConfig::new(download_directory(&profile, &settings))
            },
        );
        let tabs = TabManager::new(profile.id(), width, height);
        let mut browser = Self {
            profile,
            window: BrowserWindow {
                width,
                height,
                tabs,
            },
            chrome: BrowserChrome::new(),
            search,
            internals: InternalPageRegistry::new(),
            show_hud,
            settings,
            // Apply the persisted preference before tabs are wired to the scheduler.
            // Desktop mode defaults to asynchronous navigation; deterministic tools can
            // deliberately opt out.
            background_navigation,
            last_session_write: Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
            session_dirty: false,
            cookie_jar,
            storage_profile,
            network,
            scheduler,
            downloads,
        };
        browser.attach_cookies_to_all_tabs();
        browser.attach_storage_to_all_tabs();
        browser.refresh_internal_pages();
        if browser.settings.restore_previous_session && !browser.is_private() {
            if let Ok(Some((snap, clean))) = browser.profile.store.load_session() {
                if clean || browser.profile.store.appears_crashed() {
                    log::info!(target: "axiom_persist", "restoring previous session clean={clean}");
                    browser.restore_session(&snap);
                    browser.chrome.focus_content();
                    return browser;
                }
            }
        }
        browser.open_new_tab_page();
        browser
    }

    /// Attach the profile network (scheduler, cache, cookie provider) and the
    /// script-facing cookie jar to every tab.
    fn attach_cookies_to_all_tabs(&mut self) {
        let jar: Arc<dyn axiom_engine::CookieJar> = self.cookie_jar.clone();
        let blocking = !self.background_navigation;
        for tab in self.window.tabs.tabs_mut() {
            tab.context.set_network(Arc::clone(&self.scheduler));
            tab.context.set_cookie_jar(Some(Arc::clone(&jar)));
            tab.context.set_blocking_navigation(blocking);
        }
    }

    fn attach_cookies_to_active(&mut self) {
        let jar: Arc<dyn axiom_engine::CookieJar> = self.cookie_jar.clone();
        let blocking = !self.background_navigation;
        let context = &mut self.window.tabs.active_tab_mut().context;
        context.set_network(Arc::clone(&self.scheduler));
        context.set_cookie_jar(Some(jar));
        context.set_blocking_navigation(blocking);
    }

    /// Background mode: navigation calls return immediately and documents commit from
    /// [`tick`](Self::tick), which the UI calls every frame. Blocking mode (the default,
    /// used by tests and tools) returns once the document is interactive.
    pub fn set_background_navigation(&mut self, background: bool) {
        self.background_navigation = background;
        for tab in self.window.tabs.tabs_mut() {
            tab.context.set_blocking_navigation(!background);
        }
    }

    pub fn background_navigation(&self) -> bool {
        self.background_navigation
    }

    /// Apply one trusted settings action. Web pages cannot navigate to `axiom://settings`,
    /// so this is deliberately the only mutation surface for the internal dashboard.
    pub fn set_boolean_setting(&mut self, key: &str, value: bool) -> Result<(), &'static str> {
        match key {
            "performance_hud" => {
                self.settings.performance_hud = value;
                self.show_hud = value;
            }
            "background_networking" => {
                self.settings.background_networking = value;
                self.set_background_navigation(value);
            }
            "restore_previous_session" => self.settings.restore_previous_session = value,
            "clear_history_on_exit" => self.settings.clear_history_on_exit = value,
            "clear_cookies_on_exit" => self.settings.clear_cookies_on_exit = value,
            _ => return Err("unknown boolean setting"),
        }
        self.profile
            .store
            .save_settings(&self.settings)
            .map_err(|_| "could not persist setting")?;
        self.refresh_internal_pages();
        Ok(())
    }

    /// Select the default search provider by id and persist it in this profile.
    pub fn set_search_provider(&mut self, id: &str) -> Result<(), UnknownSearchProvider> {
        let provider = self.search.select(id)?.clone();
        self.settings.set_search_provider(&provider);
        if let Err(e) = self.profile.store.save_settings(&self.settings) {
            log::warn!(target: "axiom_persist", "could not save search provider: {e}");
        }
        self.refresh_internal_pages();
        Ok(())
    }

    /// Profile-owned network service (shared by all tabs of this profile).
    pub fn downloads(&self) -> &DownloadManager {
        &self.downloads
    }

    /// HTML of `axiom://network/<id>` (headers redacted, query strings hidden).
    pub fn network_request_page(&self, id: NetworkRequestId) -> String {
        render_network_detail(self, id)
    }

    /// One turn of the browser loop: drive every tab's event loop and hand navigations
    /// that turned out to be downloads to the download manager.
    pub fn tick(&mut self) -> axiom_engine::HudStats {
        let hud = self.window.tabs.tick_active();
        self.process_navigation_events();
        self.collect_downloads();
        hud
    }

    /// Apply what the tabs' navigations reported: the omnibox follows commits (including
    /// redirects), failures and cancellations of the active tab; a visit is recorded
    /// once a document becomes interactive, under its final URL; internal-page requests
    /// from trusted internal pages are served, those from web content are refused.
    fn process_navigation_events(&mut self) {
        let active = self.window.tabs.active_tab().id;
        let mut sync_active = false;
        let mut visits = Vec::new();
        let mut internal_requests = Vec::new();
        let mut any = false;
        for tab in self.window.tabs.tabs_mut() {
            let events = tab.context.take_navigation_events();
            if events.is_empty() {
                continue;
            }
            any = true;
            for ev in events {
                log::debug!(
                    target: "axiom_nav",
                    "tab {} navigation {} {:?} {:?} {}",
                    tab.id.0,
                    ev.id.0,
                    ev.cause,
                    ev.kind,
                    strip_query(&ev.url)
                );
                match ev.kind {
                    NavigationEventKind::Committed { .. }
                    | NavigationEventKind::Failed { .. }
                    | NavigationEventKind::Cancelled => sync_active |= tab.id == active,
                    NavigationEventKind::Interactive => {
                        if let Some(transition) = tab.visit_transition(ev.id, ev.cause) {
                            visits.push((ev.url, tab.title(), transition));
                        }
                    }
                    NavigationEventKind::InternalRequested { initiator } => {
                        internal_requests.push((tab.id, ev.url, initiator));
                    }
                    NavigationEventKind::Started
                    | NavigationEventKind::Completed
                    | NavigationEventKind::Download => {}
                }
            }
            tab.sync_after_navigation();
        }
        if !any {
            return;
        }
        for (url, title, transition) in &visits {
            if let Err(e) = self.profile.store.record_history(url, title, *transition) {
                log::warn!(target: "axiom_persist", "history write failed: {e}");
            }
        }
        for (tab, url, initiator) in internal_requests {
            if tab == active && InternalPageRegistry::is_internal(&initiator) {
                self.navigate_internal(&url);
                sync_active = true;
            } else {
                log::warn!(
                    target: "axiom_nav",
                    "refused navigation to {} initiated by web content",
                    strip_query(&url)
                );
            }
        }
        if !visits.is_empty() {
            self.refresh_internal_pages();
        }
        if sync_active {
            let url = self.window.tabs.active_tab().omnibox_url();
            self.chrome.sync_omnibox_from_tab(&url);
        }
        self.mark_session_dirty();
    }

    fn collect_downloads(&mut self) {
        for tab in self.window.tabs.tabs_mut() {
            for candidate in tab.context.take_download_requests() {
                if self.downloads.start_candidate(&candidate).is_none() {
                    log::warn!(target: "axiom_download", "download handoff rejected");
                }
            }
        }
    }

    pub fn network(&self) -> &Arc<NetworkService> {
        &self.network
    }

    pub fn scheduler(&self) -> &Arc<RequestScheduler> {
        &self.scheduler
    }

    fn attach_storage_to_all_tabs(&mut self) {
        let profile = Arc::clone(&self.storage_profile);
        for tab in self.window.tabs.tabs_mut() {
            let session = tab.ensure_session_storage();
            let binder = make_storage_binder(Arc::clone(&profile), session);
            tab.context.set_storage_binder(Some(binder));
        }
    }

    fn attach_storage_to_active(&mut self) {
        let profile = Arc::clone(&self.storage_profile);
        let tab = self.window.tabs.active_tab_mut();
        let session = tab.ensure_session_storage();
        let binder = make_storage_binder(profile, session);
        tab.context.set_storage_binder(Some(binder));
    }

    pub fn profile_id(&self) -> ProfileId {
        self.profile.id()
    }

    pub fn is_private(&self) -> bool {
        self.profile.is_private()
    }

    pub fn active_tab_id(&self) -> TabId {
        self.window.tabs.active_tab().id
    }

    pub fn chrome_state(&self) -> ChromeState {
        let tab = self.window.tabs.active_tab();
        let url = tab.url();
        let loading = tab.is_loading();
        let failed = tab.context.last_error.is_some();
        let bookmarked = self
            .profile
            .store
            .bookmark_for_url(&url)
            .ok()
            .flatten()
            .is_some();
        ChromeState {
            active_tab: tab.id,
            tab_title: tab.title(),
            tab_url: url.clone(),
            loading,
            ready: tab.context.page.ready,
            can_go_back: tab.context.can_go_back(),
            can_go_forward: tab.context.can_go_forward(),
            reload_stop: if loading {
                ReloadStopMode::Stop
            } else {
                ReloadStopMode::Reload
            },
            security: security_display(
                &url,
                &tab.context.security,
                tab.context.mixed_content,
                failed,
            ),
            connection: connection_detail(&tab.context.security, tab.context.mixed_content),
            origin: Origin::try_from_str(&url),
            focus: self.chrome.focus,
            focused_control: self.chrome.focused_control,
            status_text: self.chrome.status_text.clone(),
            window_title: window_title_for(&tab.title()),
            site_info_open: self.chrome.site_info_open,
            private: self.is_private(),
            bookmarked,
        }
    }

    pub fn open_new_tab_page(&mut self) {
        let url = self.settings.new_tab_url.clone();
        self.navigate_internal(&url);
        let url = self.window.tabs.active_tab().url();
        self.chrome.focus_omnibox(&url);
        self.mark_session_dirty();
    }

    pub fn new_tab(&mut self) -> TabId {
        let id = self.window.tabs.new_tab();
        self.attach_cookies_to_active();
        self.attach_storage_to_active();
        self.open_new_tab_page();
        self.refresh_suggestions();
        id
    }

    pub fn close_tab(&mut self) {
        self.window.tabs.close_active();
        self.after_tab_switch();
        self.mark_session_dirty();
    }

    pub fn restore_tab(&mut self) -> Option<TabId> {
        let closed = self.window.tabs.restore_closed()?;
        let id = self.window.tabs.reopen_shell();
        self.attach_cookies_to_active();
        self.attach_storage_to_active();
        self.navigate_resolved(&closed.url);
        self.after_tab_switch();
        Some(id)
    }

    pub fn duplicate_tab(&mut self) -> TabId {
        let url = self.window.tabs.active_tab().url();
        let id = self.window.tabs.new_tab();
        self.attach_cookies_to_active();
        self.attach_storage_to_active();
        self.navigate_resolved(&url);
        self.after_tab_switch();
        id
    }

    pub fn select_tab(&mut self, index: usize) {
        self.window.tabs.select(index);
        self.after_tab_switch();
        self.mark_session_dirty();
    }

    pub fn select_next_tab(&mut self) {
        self.window.tabs.select_next();
        self.after_tab_switch();
        self.mark_session_dirty();
    }

    pub fn select_prev_tab(&mut self) {
        self.window.tabs.select_prev();
        self.after_tab_switch();
        self.mark_session_dirty();
    }

    fn after_tab_switch(&mut self) {
        self.attach_cookies_to_active();
        self.attach_storage_to_active();
        let url = self.window.tabs.active_tab().url();
        self.chrome.focus = FocusOwner::WebContent;
        self.chrome.focused_control = ChromeControl::None;
        self.chrome.omnibox.editing = false;
        self.chrome.sync_omnibox_from_tab(&url);
        self.chrome.suggestions.clear();
        self.chrome.site_info_open = false;
        self.chrome.status_text.clear();
    }

    pub fn navigate(&mut self, input: &str) {
        let classified = OmniboxInputClassifier::classify(input);
        let (url, transition) = match classified {
            ClassifiedInput::Url(u) => (u, VisitTransition::Typed),
            ClassifiedInput::Search(q) => {
                if q.is_empty() {
                    return;
                }
                (self.search.search_url(&q), VisitTransition::Typed)
            }
        };
        self.navigate_resolved_with_transition(&url, transition);
    }

    pub fn navigate_resolved(&mut self, url: &str) {
        self.navigate_resolved_with_transition(url, VisitTransition::Other);
    }

    pub fn navigate_resolved_with_transition(&mut self, url: &str, transition: VisitTransition) {
        if InternalPageRegistry::is_internal(url) {
            self.navigate_internal(url);
        } else {
            self.window
                .tabs
                .active_tab_mut()
                .navigate_with_transition(url, transition);
            self.process_navigation_events();
            self.collect_downloads();
        }
        let url = self.window.tabs.active_tab().omnibox_url();
        self.chrome.sync_omnibox_from_tab(&url);
        self.chrome.focus_content();
        self.mark_session_dirty();
        self.maybe_checkpoint_session(false);
    }

    fn navigate_internal(&mut self, url: &str) {
        self.navigate_internal_hist(url, true);
    }

    /// Internal pages whose content depends on the path or query.
    fn routed_internal_page(&mut self, url: &str) -> Option<crate::internal::InternalPage> {
        if let Some(id) = network_detail_id(url) {
            return Some(crate::internal::InternalPage {
                url: format!("axiom://network/{}", id.0),
                title: format!("Request {}", id.0),
                html: render_network_detail(self, id),
            });
        }
        match internal_route(url).as_str() {
            "axiom://search" => {
                let query = query_param(url, "q").unwrap_or_default();
                Some(crate::internal::InternalPage {
                    url: url.trim().to_string(),
                    title: "Axiom Search".into(),
                    html: render_axiom_search_page(&query, &self.search),
                })
            }
            "axiom://settings/search" => {
                // Only reachable from the omnibox or a trusted internal page link: web
                // content cannot navigate to axiom:// URLs.
                if let Some(id) = query_param(url, "provider") {
                    if let Err(e) = self.set_search_provider(&id) {
                        log::warn!(target: "axiom_nav", "{e}");
                    }
                }
                Some(crate::internal::InternalPage {
                    url: "axiom://settings/search".into(),
                    title: "Search engine".into(),
                    html: render_search_settings_page(&self.search, self.is_private()),
                })
            }
            "axiom://settings" => {
                if let (Some(key), Some(value)) =
                    (query_param(url, "setting"), query_param(url, "value"))
                {
                    let value = match value.as_str() {
                        "true" => true,
                        "false" => false,
                        _ => return None,
                    };
                    if let Err(error) = self.set_boolean_setting(&key, value) {
                        log::warn!(target: "axiom_settings", "settings action rejected: {error}");
                    }
                }
                Some(crate::internal::InternalPage {
                    url: "axiom://settings".into(),
                    title: "Settings".into(),
                    html: render_settings_page(&self.settings, &self.search, self.is_private()),
                })
            }
            _ => None,
        }
    }

    fn navigate_internal_hist(&mut self, url: &str, push_history: bool) {
        self.refresh_internal_pages();
        // Choosing an option on the search settings page updates it in place.
        let push_history = push_history
            && !(internal_route(url) == "axiom://settings/search"
                && self.window.tabs.active_tab().url() == "axiom://settings/search");
        let page = match self.routed_internal_page(url) {
            Some(page) => Some(page),
            None => self.internals.resolve(url).cloned(),
        };
        let Some(page) = page else {
            self.window.tabs.active_tab_mut().context.last_error =
                Some(format!("unknown internal page: {url}"));
            return;
        };
        let tab = self.window.tabs.active_tab_mut();
        tab.context.loading = true;
        tab.context.last_error = None;
        if let Err(e) = tab
            .context
            .load_local_html(&page.url, &page.html, push_history)
        {
            tab.context.last_error = Some(e);
            tab.context.loading = false;
            return;
        }
        tab.last_url = page.url.clone();
        tab.context.loading = false;
        tab.refresh_origin_public();
    }

    pub fn back(&mut self) {
        if !self.window.tabs.active_tab().context.can_go_back() {
            return;
        }
        self.window.tabs.active_tab_mut().back();
        self.after_history_navigation();
    }

    fn after_history_navigation(&mut self) {
        let url = self.window.tabs.active_tab().url();
        if InternalPageRegistry::is_internal(&url) {
            self.navigate_internal_hist(&url, false);
        }
        self.process_navigation_events();
        let url = self.window.tabs.active_tab().omnibox_url();
        self.chrome.sync_omnibox_from_tab(&url);
        self.mark_session_dirty();
    }

    pub fn forward(&mut self) {
        if !self.window.tabs.active_tab().context.can_go_forward() {
            return;
        }
        self.window.tabs.active_tab_mut().forward();
        self.after_history_navigation();
    }

    pub fn reload_or_stop(&mut self) {
        if self.window.tabs.active_tab().is_loading() {
            self.window.tabs.active_tab_mut().context.stop_loading();
        } else {
            let url = self.window.tabs.active_tab().url();
            if InternalPageRegistry::is_internal(&url) {
                self.navigate_internal_hist(&url, false);
            } else {
                self.window.tabs.active_tab_mut().reload();
                self.collect_downloads();
            }
        }
        self.process_navigation_events();
        let url = self.window.tabs.active_tab().omnibox_url();
        self.chrome.sync_omnibox_from_tab(&url);
    }

    pub fn toggle_bookmark(&mut self) {
        let url = self.window.tabs.active_tab().url();
        let title = self.window.tabs.active_tab().title();
        if self
            .profile
            .store
            .bookmark_for_url(&url)
            .ok()
            .flatten()
            .is_some()
        {
            let _ = self.profile.store.remove_bookmark_url(&url);
        } else {
            let _ = self.profile.store.add_bookmark(&url, &title);
        }
        self.refresh_internal_pages();
    }

    pub fn submit_omnibox(&mut self) {
        if let Some(sel) = self.chrome.suggestions.selected_suggestion().cloned() {
            if let Some(idx) = sel.open_tab_index {
                self.select_tab(idx);
                self.chrome.focus_content();
                return;
            }
            self.navigate_resolved_with_transition(&sel.url, VisitTransition::Typed);
            return;
        }
        let text = self.chrome.omnibox.text.clone();
        self.navigate(&text);
    }

    pub fn focus_omnibox(&mut self) {
        let url = self.window.tabs.active_tab().url();
        self.chrome.focus_omnibox(&url);
        self.refresh_suggestions();
    }

    /// Open the profile-local workspace from trusted chrome or an internal page.
    /// Ordinary websites cannot invoke this privileged navigation path.
    pub fn open_focus_space(&mut self) {
        self.navigate_internal("axiom://focus");
    }

    pub fn refresh_suggestions(&mut self) {
        if !self.chrome.omnibox.editing {
            self.chrome.suggestions.clear();
            return;
        }
        let open: Vec<(String, String, usize)> = self
            .window
            .tabs
            .tabs()
            .iter()
            .enumerate()
            .map(|(i, t)| (t.title(), t.url(), i))
            .collect();
        let q = self.chrome.omnibox.text.clone();
        let history = self.profile.store.search_history(&q, 8).unwrap_or_default();
        let bookmarks = self
            .profile
            .store
            .search_bookmarks(&q, 8)
            .unwrap_or_default();
        self.chrome.suggestions.rebuild_from_sources(
            &q,
            &open,
            &history,
            &bookmarks,
            self.search.default_provider(),
        );
    }

    /// A click in the content area (window coordinates). Link navigations it starts are
    /// recorded like any other (history, omnibox, downloads).
    pub fn handle_content_click(&mut self, x: f32, y: f32) {
        self.window.tabs.active_tab_mut().context.handle_click(x, y);
        self.process_navigation_events();
        self.collect_downloads();
    }

    /// Programmatic click on a node of the active tab (tests, accessibility).
    pub fn click_node(&mut self, node: axiom_dom::NodeId) {
        self.window.tabs.active_tab_mut().context.click_node(node);
        self.process_navigation_events();
        self.collect_downloads();
    }

    pub fn set_hover_status(&mut self, content_x: f32, content_y: f32) {
        if self.chrome.focus == FocusOwner::BrowserChrome && self.chrome.omnibox.editing {
            return;
        }
        let href = self
            .window
            .tabs
            .active_tab()
            .context
            .link_href_at(content_x, content_y);
        self.chrome.status_text = href.unwrap_or_default();
    }

    pub fn toggle_site_info(&mut self) {
        self.chrome.site_info_open = !self.chrome.site_info_open;
        if self.chrome.site_info_open {
            self.chrome.focused_control = ChromeControl::SecurityIndicator;
        }
    }

    pub fn set_viewport(&mut self, w: u32, h: u32) {
        self.window.width = w;
        self.window.height = h;
        self.window.tabs.set_viewport(w, h);
    }

    pub fn snapshot_session(&self) -> SessionSnapshot {
        let active = self.window.tabs.active_index();
        SessionSnapshot {
            profile_id: self.profile.id(),
            tabs: self
                .window
                .tabs
                .tabs()
                .iter()
                .enumerate()
                .map(|(i, t)| SessionTab {
                    url: t.url(),
                    title: t.title(),
                    pinned: t.pinned,
                    active: i == active,
                })
                .collect(),
        }
    }

    pub fn restore_session(&mut self, snap: &SessionSnapshot) {
        if snap.tabs.is_empty() {
            return;
        }
        while self.window.tabs.len() > 1 {
            self.window.tabs.close_at(self.window.tabs.len() - 1);
        }
        self.navigate_resolved(&snap.tabs[0].url);
        self.window.tabs.active_tab_mut().pinned = snap.tabs[0].pinned;
        let mut active_idx = 0;
        for (i, st) in snap.tabs.iter().enumerate().skip(1) {
            self.window.tabs.new_tab();
            self.attach_cookies_to_active();
            self.attach_storage_to_active();
            self.navigate_resolved(&st.url);
            self.window.tabs.active_tab_mut().pinned = st.pinned;
            if st.active {
                active_idx = i;
            }
        }
        if snap.tabs[0].active {
            active_idx = 0;
        }
        self.select_tab(active_idx);
    }

    fn mark_session_dirty(&mut self) {
        self.session_dirty = true;
    }

    pub fn maybe_checkpoint_session(&mut self, force: bool) {
        if self.is_private() {
            return;
        }
        if !force && !self.session_dirty {
            return;
        }
        if !force && self.last_session_write.elapsed() < Duration::from_millis(750) {
            return;
        }
        let snap = self.snapshot_session();
        let _ = self.profile.store.checkpoint_session(&snap, false);
        self.last_session_write = Instant::now();
        self.session_dirty = false;
    }

    pub fn shutdown_clean(&mut self) {
        if self.is_private() {
            return;
        }
        let snap = self.snapshot_session();
        let _ = self.profile.store.checkpoint_session(&snap, true);
        let _ = self.profile.store.mark_clean_shutdown();
        let _ = self.profile.store.save_settings(&self.settings);
    }

    pub fn clear_history(&mut self) -> Result<(), StoreError> {
        self.profile.store.clear_history()?;
        self.refresh_internal_pages();
        Ok(())
    }

    pub fn clear_bookmarks(&mut self) -> Result<(), StoreError> {
        self.profile.store.clear_bookmarks()?;
        self.refresh_internal_pages();
        Ok(())
    }

    pub fn clear_session_state(&mut self) -> Result<(), StoreError> {
        self.profile.store.clear_session_state()
    }

    pub fn clear_cookies(&mut self) -> Result<(), StoreError> {
        self.profile.store.clear_cookies()?;
        self.refresh_internal_pages();
        Ok(())
    }

    pub fn clear_cookies_for_site(&mut self, host: &str) -> Result<u64, StoreError> {
        let n = self.profile.store.clear_cookies_for_site(host)?;
        self.refresh_internal_pages();
        Ok(n)
    }

    /// Clear the profile's HTTP cache and its CORS-preflight cache.
    pub fn clear_cache(&mut self) {
        self.network.clear_cache();
    }

    pub fn clear_local_storage(&mut self) -> Result<(), StoreError> {
        self.profile.store.clear_local_storage()
    }

    pub fn clear_local_storage_for_origin(&mut self, origin: &str) -> Result<u64, StoreError> {
        self.profile.store.clear_local_storage_for_origin(origin)
    }

    pub fn cookie_jar(&self) -> Arc<ProfileCookieJar> {
        Arc::clone(&self.cookie_jar)
    }

    pub fn storage_profile(&self) -> Arc<ProfileStorage> {
        Arc::clone(&self.storage_profile)
    }

    pub fn refresh_internal_pages(&mut self) {
        let hist = self.profile.store.recent_history(50).unwrap_or_default();
        let bookmarks = self.profile.store.list_bookmarks(50).unwrap_or_default();
        let cookies = self
            .profile
            .store
            .list_cookies_inspect()
            .unwrap_or_default();
        let hist_html = render_history_page(&hist, self.is_private());
        let bm_html = render_bookmarks_page(&bookmarks);
        let ver_html = render_version_page(&self.profile, &self.settings);
        let cookie_html = render_cookies_page(&cookies, self.is_private());
        let network_html = render_network_page(self);
        let downloads_html = render_downloads_page(&self.downloads.list(), self.is_private());
        let document_html = render_document_page(&self.window.tabs.active_tab().context);
        let focus_html = render_focus_page(self);
        self.internals
            .insert_dynamic("axiom://document", "Document", document_html);
        self.internals
            .insert_dynamic("axiom://history", "History", hist_html);
        self.internals
            .insert_dynamic("axiom://bookmarks", "Bookmarks", bm_html);
        self.internals
            .insert_dynamic("axiom://version", "Version", ver_html);
        self.internals
            .insert_dynamic("axiom://cookies", "Cookies", cookie_html);
        self.internals
            .insert_dynamic("axiom://network", "Network", network_html);
        self.internals
            .insert_dynamic("axiom://downloads", "Downloads", downloads_html);
        let settings_html = render_settings_page(&self.settings, &self.search, self.is_private());
        self.internals
            .insert_dynamic("axiom://settings", "Settings", settings_html);
        self.internals
            .insert_dynamic("axiom://focus", "Focus Space", focus_html);
    }

    pub fn debug_profile_info(&self) -> String {
        format!(
            "profile_id={} kind={:?} schema={} history={} bookmarks={} private={} crashed={}",
            self.profile.id(),
            self.profile.kind(),
            self.profile.store.schema_version(),
            self.profile.store.history_count().unwrap_or(0),
            self.profile.store.bookmark_count().unwrap_or(0),
            self.is_private(),
            self.profile.store.appears_crashed()
        )
    }

    pub fn handle_chrome_key(&mut self, key: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        if ctrl && !alt {
            match key {
                "l" | "L" => {
                    self.focus_omnibox();
                    return true;
                }
                "t" | "T" if !shift => {
                    self.new_tab();
                    return true;
                }
                "w" | "W" => {
                    self.close_tab();
                    return true;
                }
                "t" | "T" if shift => {
                    self.restore_tab();
                    return true;
                }
                "r" | "R" => {
                    self.reload_or_stop();
                    return true;
                }
                "d" | "D" => {
                    self.toggle_bookmark();
                    return true;
                }
                " " if shift => {
                    self.open_focus_space();
                    return true;
                }
                "a" | "A" if self.chrome.omnibox.editing => {
                    self.chrome.omnibox.select_all();
                    return true;
                }
                "c" | "C" if self.chrome.omnibox.editing => {
                    self.chrome.omnibox.copy();
                    return true;
                }
                "x" | "X" if self.chrome.omnibox.editing => {
                    self.chrome.omnibox.cut();
                    self.refresh_suggestions();
                    return true;
                }
                "v" | "V" if self.chrome.omnibox.editing => {
                    self.chrome.omnibox.paste();
                    self.refresh_suggestions();
                    return true;
                }
                "Tab" if shift => {
                    self.select_prev_tab();
                    return true;
                }
                "Tab" => {
                    self.select_next_tab();
                    return true;
                }
                _ => {}
            }
        }
        if alt {
            match key {
                "ArrowLeft" => {
                    self.back();
                    return true;
                }
                "ArrowRight" => {
                    self.forward();
                    return true;
                }
                _ => {}
            }
        }

        if self.chrome.focus != FocusOwner::BrowserChrome || !self.chrome.omnibox.editing {
            return false;
        }

        match key {
            "Escape" => {
                self.chrome.omnibox.escape();
                self.chrome.focus = FocusOwner::WebContent;
                self.chrome.focused_control = ChromeControl::None;
                self.chrome.suggestions.clear();
                true
            }
            "Enter" => {
                self.submit_omnibox();
                true
            }
            "ArrowDown" => {
                self.chrome.suggestions.select_next();
                true
            }
            "ArrowUp" => {
                self.chrome.suggestions.select_prev();
                true
            }
            "Home" => {
                self.chrome.omnibox.move_home(shift);
                true
            }
            "End" => {
                self.chrome.omnibox.move_end(shift);
                true
            }
            "ArrowLeft" => {
                self.chrome.omnibox.move_left(shift);
                true
            }
            "ArrowRight" => {
                self.chrome.omnibox.move_right(shift);
                true
            }
            "Backspace" => {
                self.chrome.omnibox.delete_backward();
                self.refresh_suggestions();
                true
            }
            "Delete" => {
                self.chrome.omnibox.delete_forward();
                self.refresh_suggestions();
                true
            }
            s if s.chars().count() == 1 && !ctrl && !alt => {
                self.chrome.omnibox.insert_text(s);
                self.refresh_suggestions();
                true
            }
            _ => false,
        }
    }
}

/// A profile-local workspace view. It deliberately derives its data from the active
/// browser model and never uploads tab, history, cookie, or activity information.
fn render_focus_page(browser: &Browser) -> String {
    let tabs = browser.window.tabs.tabs();
    let active = browser.window.tabs.active_index();
    let loading = tabs.iter().filter(|tab| tab.is_loading()).count();
    let bookmarks = browser.profile.store.bookmark_count().unwrap_or(0);
    let history = browser.profile.store.history_count().unwrap_or(0);
    let downloading = browser
        .downloads
        .list()
        .iter()
        .filter(|download| matches!(download.state, axiom_download::DownloadState::Downloading))
        .count();
    let mut cards = String::new();
    for (index, tab) in tabs.iter().enumerate() {
        let marker = if index == active { "ACTIVE" } else { "OPEN" };
        let active_class = if index == active { " active" } else { "" };
        cards.push_str(&format!(
            "<article class=\"tab{active_class}\"><span>{marker}</span><h2>{}</h2><p>{}</p></article>",
            html_escape(&tab.title()),
            html_escape(&tab.url()),
        ));
    }
    let mode = if browser.is_private() {
        "Private / memory-only"
    } else {
        "Persistent / local-only"
    };
    format!(
        r#"<!doctype html><html><head><title>Focus Space</title><style>
body{{font-family:system-ui,sans-serif;margin:0;padding:44px;color:#eef3ff;background:#0b1220;max-width:1160px}}
h1{{font-size:40px;line-height:1.05;margin:5px 0 12px;max-width:760px}} h2{{font-size:17px;margin:10px 0}} p{{line-height:1.55}} a{{color:#dbeafe}} .meta{{color:#c9d7ff}} .eyebrow{{letter-spacing:2px;font-size:12px;font-weight:800;color:#9fd7ff}}
.actions{{margin-top:24px}} .action{{margin-right:14px;color:#9fd7ff;font-weight:700}}
.stats{{margin-top:22px}} .grid{{margin-top:22px}}
.stat{{margin-bottom:10px;border:1px solid #334b70;border-radius:14px;padding:13px;background-color:#121d31}} .tab{{margin-bottom:14px;border:1px solid #334b70;border-radius:18px;padding:16px;background-color:#121d31}}
.stat strong{{display:block;font-size:27px;color:#b8e7ff}} .stat span{{font-size:11px;color:#8ab4ff;font-weight:800;letter-spacing:1px}} .tab span{{font-size:11px;color:#8ab4ff;font-weight:800;letter-spacing:1px}} .section-title{{margin:32px 0 2px;font-size:22px}}
.tab{{min-height:120px}} .tab.active{{background-color:#2a406d;border-color:#8ab4ff}} .tab h2{{font-size:18px;margin:10px 0}} .tab p{{color:#b8c2d9;word-break:break-all;font-size:13px}}
</style></head><body><section class=\"hero\"><p class=\"eyebrow\">AXIOM FOCUS SPACE</p><h1>Your browsing, arranged as a calm local workspace.</h1><p>{mode} · {count} open tabs · no telemetry, no AI, no cloud sync.</p><p class=\"actions\"><a class=\"action\" href=\"axiom://newtab\">New tab</a> · <a class=\"action\" href=\"axiom://settings\">Settings</a> · <a class=\"action\" href=\"axiom://network\">Network</a> · <a class=\"action\" href=\"axiom://downloads\">Downloads</a></p></section><main class=\"stats\"><p class=\"stat\"><strong>{count}</strong><br>OPEN TABS</p><p class=\"stat\"><strong>{loading}</strong><br>LOADING NOW</p><p class=\"stat\"><strong>{bookmarks}</strong><br>SAVED BOOKMARKS</p><p class=\"stat\"><strong>{history}</strong><br>LOCAL VISITS</p><p class=\"stat\"><strong>{downloading}</strong><br>ACTIVE DOWNLOADS</p></main><h2 class=\"section-title\">Open workspace</h2><main class=\"grid\">{cards}</main><p class=\"meta\">Trusted internal page — axiom://focus. This view is calculated locally from the current profile and never uploads activity.</p></body></html>"#,
        count = tabs.len(),
        loading = loading,
        bookmarks = bookmarks,
        history = history,
        downloading = downloading,
    )
}

fn render_history_page(entries: &[crate::history_repo::HistoryRecord], private: bool) -> String {
    let mut rows = String::new();
    if private {
        rows.push_str("<p>Private browsing — history is session-local and not restored.</p>");
    }
    for e in entries {
        rows.push_str(&format!(
            "<li><a href=\"{}\">{}</a> <span class=\"meta\">{}</span></li>",
            html_escape(&e.url),
            html_escape(&e.title),
            e.visit_count
        ));
    }
    if entries.is_empty() {
        rows.push_str("<li>No history yet.</li>");
    }
    format!(
        r#"<!DOCTYPE html><html><head><title>History</title>
<style>
body{{font-family:system-ui,sans-serif;margin:40px;background:#f6f7f9;color:#1a1a1a}}
h1{{font-weight:600}} a{{color:#1a5fb4}} .meta{{color:#888;font-size:12px}}
</style></head><body><h1>History</h1><ul>{rows}</ul>
<p class="meta">Trusted internal page — axiom://history</p></body></html>"#
    )
}

fn render_bookmarks_page(entries: &[crate::bookmarks_repo::Bookmark]) -> String {
    let mut rows = String::new();
    for e in entries {
        rows.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>",
            html_escape(&e.url),
            html_escape(&e.title)
        ));
    }
    if entries.is_empty() {
        rows.push_str("<li>No bookmarks yet. Press Ctrl+D to bookmark the current page.</li>");
    }
    format!(
        r#"<!DOCTYPE html><html><head><title>Bookmarks</title>
<style>
body{{font-family:system-ui,sans-serif;margin:40px;background:#f6f7f9;color:#1a1a1a}}
h1{{font-weight:600}} a{{color:#1a5fb4}}
</style></head><body><h1>Bookmarks</h1><ul>{rows}</ul>
<p>Trusted internal page — axiom://bookmarks</p></body></html>"#
    )
}

fn render_version_page(profile: &Profile, settings: &BrowserSettings) -> String {
    format!(
        r#"<!DOCTYPE html><html><head><title>Version</title>
<style>
body{{font-family:system-ui,sans-serif;margin:40px;background:#f6f7f9;color:#1a1a1a}}
h1{{font-weight:600}} code{{background:#e8ebf0;padding:2px 6px;border-radius:4px}}
</style></head><body>
<h1>Axiom</h1>
<ul>
<li>Version: <code>{}</code></li>
<li>Build profile: <code>{}</code></li>
<li>Target: <code>{}</code></li>
<li>Profile kind: <code>{:?}</code></li>
<li>Profile id: <code>{}</code></li>
<li>Schema: <code>v{}</code></li>
<li>Theme setting: <code>{}</code></li>
<li>Restore session: <code>{}</code></li>
</ul>
<p>Independent engine — no Chromium/WebKit/Gecko embedding.</p>
</body></html>"#,
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        std::env::consts::ARCH,
        profile.kind(),
        profile.id(),
        profile.store.schema_version(),
        settings.theme,
        settings.restore_previous_session,
    )
}

fn render_cookies_page(
    cookies: &[crate::cookie_types::CookieInspectRecord],
    private: bool,
) -> String {
    let mut rows = String::new();
    if private {
        rows.push_str(
            "<p>Private profile — cookies are memory-only and never written to disk.</p>",
        );
    }
    rows.push_str(
        "<p class=\"meta\">Values hidden by default (security). Trusted browser UI only.</p>",
    );
    rows.push_str("<table><thead><tr>");
    for h in [
        "Site/Domain",
        "Name",
        "Path",
        "Expires",
        "Secure",
        "HttpOnly",
        "SameSite",
    ] {
        rows.push_str(&format!("<th>{h}</th>"));
    }
    rows.push_str("</tr></thead><tbody>");
    if cookies.is_empty() {
        rows.push_str("<tr><td colspan=\"7\">No cookies stored.</td></tr>");
    }
    for c in cookies {
        let exp = if c.session {
            "session".to_string()
        } else {
            c.expires_at_ms
                .map(|t| t.to_string())
                .unwrap_or_else(|| "—".into())
        };
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            html_escape(&c.domain),
            html_escape(&c.name),
            html_escape(&c.path),
            html_escape(&exp),
            c.secure,
            c.http_only,
            html_escape(&c.same_site),
        ));
    }
    rows.push_str("</tbody></table>");
    format!(
        r#"<!DOCTYPE html><html><head><title>Cookies</title>
<style>
body{{font-family:system-ui,sans-serif;margin:40px;background:#f6f7f9;color:#1a1a1a}}
h1{{font-weight:600}} table{{border-collapse:collapse;width:100%;font-size:13px}}
th,td{{border:1px solid #ddd;padding:6px 8px;text-align:left}} th{{background:#e8ebf0}}
.meta{{color:#666;font-size:12px}}
</style></head><body><h1>Cookies</h1>{rows}
<p class="meta">Trusted internal page — axiom://cookies — Clear via browser data APIs.</p>
</body></html>"#
    )
}

/// `axiom://document`: the active tab's document loader state, its resources, lifecycle
/// timeline and recent structured events (URLs without query strings).
fn render_document_page(ctx: &axiom_engine::BrowsingContext) -> String {
    let mut body = String::new();
    let mut sections: Vec<(
        axiom_engine::DocumentDiagnostics,
        Vec<axiom_engine::ResourceDiagnostics>,
        Vec<axiom_engine::DocumentEvent>,
    )> = Vec::new();
    if let Some(d) = ctx.document_diagnostics() {
        sections.push((d, ctx.resource_diagnostics(), ctx.document_events()));
    }
    for r in ctx.retired_documents().iter().rev() {
        sections.push((r.diagnostics.clone(), r.resources.clone(), r.events.clone()));
    }
    if sections.is_empty() {
        body.push_str("<p>No document has been loaded in this tab.</p>");
    }
    for (i, (d, resources, events)) in sections.iter().enumerate() {
        let heading = if i == 0 && !d.canceled {
            "Current document"
        } else {
            "Previous document"
        };
        let timeline: Vec<String> = d
            .timeline
            .iter()
            .map(|(name, ms)| match ms {
                Some(ms) => format!("{name}={ms:.1}ms"),
                None => format!("{name}=not reached"),
            })
            .collect();
        body.push_str(&format!(
            "<h2>{heading}: {}</h2>\
             <p class=\"stats\">{} · kind={} · origin={} · encoding={} · readyState={} · parser={} ({} bytes)</p>\
             <p class=\"stats\">DOMContentLoaded={} · load={} · canceled={} · resources={} pending={} failed={} rejected={} · script errors={} · stale events dropped={}</p>\
             <p class=\"stats\">blocking stylesheets={} · parser-blocking script={} · defer queue={} · load-blocking={}</p>\
             <p class=\"stats\">{}</p>",
            d.document,
            html_escape(&truncate_url(&strip_query(&d.url), 100)),
            d.kind,
            html_escape(&d.origin),
            html_escape(d.encoding.as_deref().unwrap_or("n/a")),
            d.ready_state,
            d.parsing_state,
            d.bytes_parsed,
            d.dom_content_loaded_fired,
            d.load_fired,
            d.canceled,
            d.total_resources,
            d.pending_resources,
            d.failed_resources,
            d.rejected_resources,
            d.script_errors,
            d.stale_events_dropped,
            d.blocking_stylesheets.len(),
            html_escape(
                &d.parser_blocking_script
                    .as_deref()
                    .map(|s| truncate_url(&strip_query(s), 80))
                    .unwrap_or_else(|| "none".into())
            ),
            d.defer_queue.len(),
            d.load_blocking,
            html_escape(&timeline.join(" · ")),
        ));
        body.push_str(
            "<table><thead><tr><th>Id</th><th>Type</th><th>URL</th><th>Initiator</th><th>State</th>\
             <th>Priority</th><th>Blocking</th><th>Status</th><th>Cache</th><th>Bytes</th><th>ms</th>\
             <th>Error</th></tr></thead><tbody>",
        );
        for r in resources {
            let blocking = match (r.render_blocking, r.load_blocking) {
                (true, true) => "render+load",
                (true, false) => "render",
                (false, true) => "load",
                (false, false) => "—",
            };
            body.push_str(&format!(
                "<tr><td>{}</td><td>{}{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                r.id,
                r.kind,
                r.script_kind.map(|k| format!(" ({k})")).unwrap_or_default(),
                html_escape(&truncate_url(&strip_query(&r.url), 80)),
                r.initiator,
                r.state,
                r.priority,
                blocking,
                r.status.map(|s| s.to_string()).unwrap_or_else(|| "—".into()),
                r.cache.unwrap_or("n/a"),
                r.transferred_bytes,
                r.duration_ms
                    .map(|ms| format!("{ms:.1}"))
                    .unwrap_or_else(|| "pending".into()),
                html_escape(r.failure.as_deref().unwrap_or("")),
            ));
        }
        body.push_str("</tbody></table>");
        let tail = events.len().saturating_sub(40);
        body.push_str("<details><summary>Recent events</summary><ol class=\"events\">");
        for e in &events[tail..] {
            body.push_str(&format!(
                "<li>#{} {} {}{}</li>",
                e.seq,
                e.kind.name(),
                e.resource.map(|r| format!("{r} ")).unwrap_or_default(),
                html_escape(&truncate_url(&strip_query(&e.detail), 100)),
            ));
        }
        body.push_str("</ol></details>");
    }
    format!(
        r#"<!DOCTYPE html><html><head><title>Document</title>
<style>
body{{font-family:system-ui,sans-serif;margin:40px;background:#f6f7f9;color:#1a1a1a}}
h1,h2{{font-weight:600}} table{{border-collapse:collapse;width:100%;font-size:12px}}
th,td{{border:1px solid #ddd;padding:5px 7px;text-align:left;word-break:break-all}} th{{background:#e8ebf0}}
.meta{{color:#666;font-size:12px}} .stats{{margin:6px 0;color:#333}} .events{{font-size:12px;color:#333}}
</style></head><body><h1>Document</h1>
<p class="stats">browsing context {}</p>
{body}
<p class="meta">Trusted internal page — axiom://document — query strings are not shown; response bodies are never displayed.</p>
</body></html>"#,
        ctx.browsing_context_id(),
    )
}

fn render_network_page(browser: &Browser) -> String {
    let mut rows = String::new();
    // Newest first. The profile service log covers every tab of this profile.
    let mut entries = browser.network.recent_log(50);
    entries.reverse();
    if entries.is_empty() {
        rows.push_str(
            "<tr><td colspan=\"10\">No recent network activity in this profile.</td></tr>",
        );
    }
    for e in &entries {
        let status = e
            .status
            .map(|s| s.to_string())
            .or_else(|| e.error.clone())
            .unwrap_or_else(|| "—".into());
        // Query strings can carry tokens; show path-level URLs only.
        let url_show = truncate_url(&strip_query(&e.url), 80);
        let redirects = if e.redirects.is_empty() {
            String::new()
        } else {
            format!(" ({} redirects)", e.redirects.len())
        };
        rows.push_str(&format!(
            "<tr><td>{}</td><td><a href=\"axiom://network/{}\">{}</a>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            html_escape(&e.method),
            e.id.0,
            html_escape(&url_show),
            html_escape(&redirects),
            html_escape(&status),
            e.resource_type,
            e.protocol.map(|p| p.as_str()).unwrap_or("n/a"),
            e.transferred_bytes,
            e.decoded_bytes,
            e.cache_state.map(|c| c.as_str()).unwrap_or("n/a"),
            phase(e.timing.ttfb_ms),
            phase(e.timing.total_ms),
        ));
    }
    rows.push_str("</tbody></table>");
    let m = browser.network.metrics().snapshot();
    let cache = browser.network.cache().stats();
    let pool = browser.network.pool_stats();
    let sched = browser.scheduler.stats();
    let opt = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_else(|| "n/a".into());
    format!(
        r#"<!DOCTYPE html><html><head><title>Network</title>
<style>
body{{font-family:system-ui,sans-serif;margin:40px;background:#f6f7f9;color:#1a1a1a}}
h1{{font-weight:600}} table{{border-collapse:collapse;width:100%;font-size:12px}}
th,td{{border:1px solid #ddd;padding:5px 7px;text-align:left;word-break:break-all}} th{{background:#e8ebf0}}
.meta{{color:#666;font-size:12px}} .stats{{margin:6px 0;color:#333}}
</style></head><body><h1>Network</h1>
<p class="stats">requests={} network={} transferred={} decoded={} cancelled={} failed={} blocked={}</p>
<p class="stats">cache: entries={} bytes={} hits={} misses={} revalidations={} evictions={} persistent={}</p>
<p class="stats">connections: opened={} reused={} active={} · scheduler: queued={} in_flight={} peak={} limit={}</p>
<table><thead><tr><th>Method</th><th>URL</th><th>Status</th><th>Type</th><th>Protocol</th><th>Transferred</th><th>Decoded</th><th>Cache</th><th>TTFB ms</th><th>Total ms</th></tr></thead><tbody>
{rows}
<p class="meta">Trusted internal page — axiom://network — cookie, authorization and query values are not shown. DNS/connect/TLS phase timings are not measured by this transport and are omitted.</p>
</body></html>"#,
        m.requests,
        m.network_requests,
        m.bytes_transferred,
        m.bytes_decoded,
        m.cancelled,
        m.failed,
        m.blocked,
        cache.entries,
        cache.bytes,
        cache.hits,
        cache.misses,
        cache.revalidations,
        cache.evictions,
        browser.network.cache().is_persistent(),
        opt(pool.connections_opened),
        opt(pool.connections_reused),
        opt(pool.active_connections),
        sched.queued,
        sched.in_flight,
        sched.peak_in_flight,
        browser.scheduler.config().max_concurrent,
    )
}

/// `axiom://network/<request id>` → the id. The internal registry keys pages by host, so
/// the detail view is resolved before the registry.
fn network_detail_id(url: &str) -> Option<NetworkRequestId> {
    let t = url.trim();
    let prefix = "axiom://network/";
    if t.len() <= prefix.len() || !t[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return None;
    }
    let rest = t[prefix.len()..].split(['/', '?', '#']).next()?;
    rest.parse().ok().map(NetworkRequestId)
}

fn render_network_detail(browser: &Browser, id: NetworkRequestId) -> String {
    let style = r#"<style>
body{font-family:system-ui,sans-serif;margin:40px;background:#f6f7f9;color:#1a1a1a}
h1{font-weight:600} h2{font-size:16px;margin-top:24px} table{border-collapse:collapse;font-size:12px}
th,td{border:1px solid #ddd;padding:4px 7px;text-align:left;word-break:break-all} th{background:#e8ebf0}
.meta{color:#666;font-size:12px}
</style>"#;
    let Some(e) = browser.network.log_entry(id) else {
        return format!(
            "<!DOCTYPE html><html><head><title>Request {0}</title>{style}</head><body>\
             <h1>Request {0}</h1><p>This request is no longer in the profile's network log.</p>\
             <p><a href=\"axiom://network\">Back to Network</a></p></body></html>",
            id.0
        );
    };
    let row = |k: &str, v: &str| {
        format!(
            "<tr><th>{}</th><td>{}</td></tr>",
            html_escape(k),
            html_escape(v)
        )
    };
    let na = |v: Option<String>| v.unwrap_or_else(|| "n/a".into());
    let mut general = String::new();
    general.push_str(&row("URL", &strip_query(&e.url)));
    if let Some(f) = &e.final_url {
        general.push_str(&row("Final URL", &strip_query(f)));
    }
    general.push_str(&row("Method", &e.method));
    general.push_str(&row("Status", &na(e.status.map(|s| s.to_string()))));
    general.push_str(&row("State", &format!("{:?}", e.state)));
    general.push_str(&row("Resource type", e.resource_type));
    general.push_str(&row("Priority", e.priority));
    general.push_str(&row("Initiator", &e.initiator));
    general.push_str(&row("Origin", &na(e.origin.clone())));
    general.push_str(&row(
        "Referrer",
        &na(e.referrer.as_deref().map(strip_query)),
    ));
    general.push_str(&row(
        "Protocol",
        e.protocol.map(|p| p.as_str()).unwrap_or("n/a"),
    ));
    general.push_str(&row(
        "Cache",
        e.cache_state.map(|c| c.as_str()).unwrap_or("n/a"),
    ));
    general.push_str(&row("Transferred bytes", &e.transferred_bytes.to_string()));
    general.push_str(&row("Decoded bytes", &e.decoded_bytes.to_string()));
    if let Some(err) = &e.error {
        general.push_str(&row("Error", err));
        general.push_str(&row("Error kind", e.error_kind.unwrap_or("n/a")));
    }

    let t = &e.timing;
    let mut timing = String::new();
    for (k, v) in [
        ("Queued", t.queued_ms),
        ("DNS", t.dns_ms),
        ("Connect", t.connect_ms),
        ("TLS", t.tls_ms),
        ("Request sent", t.request_sent_ms),
        ("Waiting (TTFB)", t.ttfb_ms),
        ("Download", t.download_ms),
        ("Total", t.total_ms),
    ] {
        let shown = v
            .map(|d| format!("{d:.1} ms"))
            .unwrap_or_else(|| "not measured".into());
        timing.push_str(&row(k, &shown));
    }

    let mut redirects = String::new();
    if e.redirects.is_empty() {
        redirects.push_str("<p>No redirects.</p>");
    } else {
        redirects.push_str("<table><tr><th>Status</th><th>From</th><th>To</th><th>Method</th><th>Cross-origin</th></tr>");
        for r in &e.redirects {
            redirects.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                r.status,
                html_escape(&strip_query(&r.from)),
                html_escape(&strip_query(&r.to)),
                html_escape(&r.method),
                r.cross_origin
            ));
        }
        redirects.push_str("</table>");
    }

    let tls = match &e.tls {
        None => {
            "<p>Not a TLS connection (or the request failed before a TLS response).</p>".to_string()
        }
        Some(tls) => {
            let mut s = String::from("<table>");
            s.push_str(&row(
                "Certificate verified",
                if tls.certificate_verified {
                    "yes"
                } else {
                    "no"
                },
            ));
            s.push_str(&row("ALPN", tls.alpn.as_str()));
            s.push_str(&row(
                "TLS version",
                tls.version.as_deref().unwrap_or("not exposed by transport"),
            ));
            s.push_str(&row(
                "Cipher suite",
                tls.cipher_suite
                    .as_deref()
                    .unwrap_or("not exposed by transport"),
            ));
            s.push_str(&row("Hostname", &na(tls.hostname.clone())));
            if let Some(c) = &tls.certificate {
                s.push_str(&row("Subject", &c.subject));
                s.push_str(&row("Issuer", &c.issuer));
                s.push_str(&row("Subject alt names", &c.subject_alt_names.join(", ")));
                s.push_str(&row("Serial", &c.serial_hex));
                s.push_str(&row("Valid from (unix)", &c.not_before_unix.to_string()));
                s.push_str(&row("Valid until (unix)", &c.not_after_unix.to_string()));
                s.push_str(&row("SHA-256", &c.sha256_fingerprint));
            }
            s.push_str("</table>");
            s
        }
    };

    let headers = |h: &axiom_net::HeaderMap| {
        let h = h.redacted();
        let mut s = String::from("<table>");
        let mut any = false;
        for (name, values) in h.iter() {
            for v in values {
                any = true;
                s.push_str(&row(name, v));
            }
        }
        if !any {
            s.push_str("<tr><td>none recorded</td></tr>");
        }
        s.push_str("</table>");
        s
    };

    format!(
        r#"<!DOCTYPE html><html><head><title>Request {id}</title>{style}</head><body>
<h1>Request {id}</h1><p><a href="axiom://network">Back to Network</a></p>
<h2>General</h2><table>{general}</table>
<h2>Timing</h2><table>{timing}</table>
<h2>Redirects</h2>{redirects}
<h2>Security</h2>{tls}
<h2>Request headers</h2>{req}
<h2>Response headers</h2>{resp}
<p class="meta">Trusted internal page. Authorization, Proxy-Authorization, Cookie and Set-Cookie values are redacted; query strings are hidden; bodies are never recorded.</p>
</body></html>"#,
        id = id.0,
        req = headers(&e.request_headers),
        resp = headers(&e.response_headers),
    )
}

fn render_downloads_page(records: &[DownloadRecord], private: bool) -> String {
    let mut rows = String::new();
    if records.is_empty() {
        rows.push_str("<tr><td colspan=\"5\">No downloads in this session.</td></tr>");
    }
    for r in records.iter().rev() {
        let size = match r.expected_size {
            Some(total) => format!("{} / {}", r.downloaded, total),
            None => r.downloaded.to_string(),
        };
        let state = match &r.error {
            Some(err) => format!("{} ({})", r.state.as_str(), err),
            None => r.state.as_str().to_string(),
        };
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            html_escape(r.filename.as_deref().unwrap_or("(pending)")),
            html_escape(&state),
            html_escape(&size),
            html_escape(&truncate_url(&strip_query(&r.url), 80)),
            html_escape(
                &r.destination
                    .as_ref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default()
            ),
        ));
    }
    let note = if private {
        "Private window: this list is forgotten and unfinished files are deleted when the window closes."
    } else {
        "Downloads stream to a partial file and are renamed when complete."
    };
    format!(
        r#"<!DOCTYPE html><html><head><title>Downloads</title>
<style>
body{{font-family:system-ui,sans-serif;margin:40px;background:#f6f7f9;color:#1a1a1a}}
h1{{font-weight:600}} table{{border-collapse:collapse;width:100%;font-size:12px}}
th,td{{border:1px solid #ddd;padding:5px 7px;text-align:left;word-break:break-all}} th{{background:#e8ebf0}}
.meta{{color:#666;font-size:12px}}
</style></head><body><h1>Downloads</h1>
<table><thead><tr><th>File</th><th>State</th><th>Bytes</th><th>URL</th><th>Saved to</th></tr></thead><tbody>
{rows}</tbody></table>
<p class="meta">Trusted internal page — axiom://downloads — {note}</p>
</body></html>"#
    )
}

fn phase(ms: Option<f64>) -> String {
    ms.map(|d| format!("{d:.0}"))
        .unwrap_or_else(|| "n/a".into())
}

fn strip_query(url: &str) -> String {
    match url.find(['?', '#']) {
        Some(i) => format!("{}…", &url[..i]),
        None => url.to_string(),
    }
}

fn truncate_url(url: &str, max: usize) -> String {
    match url.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &url[..i]),
        None => url.to_string(),
    }
}

/// `axiom://host/path` without query or fragment, host lowercased; the trailing slash of
/// a bare host is dropped.
fn internal_route(url: &str) -> String {
    let t = url.trim();
    let end = t.find(['?', '#']).unwrap_or(t.len());
    let base = t[..end].trim_end_matches('/');
    match base.split_once("://") {
        Some((scheme, rest)) => {
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            let mut route = format!(
                "{}://{}",
                scheme.to_ascii_lowercase(),
                host.to_ascii_lowercase()
            );
            if !path.is_empty() {
                route.push('/');
                route.push_str(path);
            }
            route
        }
        None => base.to_string(),
    }
}

pub(crate) fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
