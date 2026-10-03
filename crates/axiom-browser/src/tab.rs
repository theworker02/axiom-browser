//! A single browser tab — owns an independent `BrowsingContext`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axiom_engine::{BrowsingContext, HudStats, NavigationCause, NavigationId};
use axiom_paint::Framebuffer;
use axiom_url::Url;
use parking_lot::Mutex;

use crate::history_repo::VisitTransition;
use crate::origin::{Origin, SecurityOrigin};
use crate::profile::ProfileId;
use crate::storage_service::SessionStorageMap;

static NEXT_TAB: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabLifecycle {
    Active,
    Background,
    Throttled,
    /// Reserved for Wave AC.
    Frozen,
    Suspended,
    Discarded,
}

pub struct Tab {
    pub id: TabId,
    pub profile_id: ProfileId,
    pub context: BrowsingContext,
    pub pinned: bool,
    pub muted: bool,
    pub lifecycle: TabLifecycle,
    pub created_at_ms: u64,
    /// Closed tabs stash URL for Ctrl+Shift+T.
    pub last_url: String,
    pub security_origin: SecurityOrigin,
    /// Per-tab sessionStorage (never durable).
    session_storage: Arc<Mutex<SessionStorageMap>>,
    /// Latest browser-initiated navigation and the history transition it records.
    pending_visit: Option<(NavigationId, VisitTransition)>,
}

impl Tab {
    pub fn new(profile_id: ProfileId, viewport_w: u32, viewport_h: u32) -> Self {
        let id = TabId(NEXT_TAB.fetch_add(1, Ordering::Relaxed));
        let mut context = BrowsingContext::new(viewport_w, viewport_h);
        context.set_owner(Some(id.0), Some(profile_id.to_string()));
        // Host draws tab strip (28) + toolbar (48).
        context.set_chrome_height(crate::CHROME_H);
        context.set_viewport(viewport_w, viewport_h);
        Self {
            id,
            profile_id,
            context,
            pinned: false,
            muted: false,
            lifecycle: TabLifecycle::Active,
            created_at_ms: now_ms(),
            last_url: "about:blank".into(),
            security_origin: SecurityOrigin::opaque(),
            session_storage: Arc::new(Mutex::new(SessionStorageMap::new())),
            pending_visit: None,
        }
    }

    pub fn ensure_session_storage(&self) -> Arc<Mutex<SessionStorageMap>> {
        Arc::clone(&self.session_storage)
    }

    pub fn title(&self) -> String {
        let t = self.context.page.title.trim();
        if t.is_empty() {
            "New Tab".into()
        } else {
            t.to_string()
        }
    }

    /// Address of the committed document; on an error page, the URL that failed.
    pub fn url(&self) -> String {
        self.context.display_url()
    }

    /// What the omnibox shows: the pending URL while a browser-initiated (typed,
    /// bookmark, restore) navigation loads, otherwise [`url`](Self::url). Page-initiated
    /// navigations keep showing the committed URL until they commit, so a page cannot
    /// make the address bar claim a destination it has not reached.
    pub fn omnibox_url(&self) -> String {
        match (self.context.pending_navigation_id(), self.pending_visit) {
            (Some(pending), Some((id, _))) if pending == id => self
                .context
                .pending_url()
                .map(str::to_string)
                .unwrap_or_else(|| self.url()),
            _ => self.url(),
        }
    }

    pub fn is_loading(&self) -> bool {
        self.context.loading
    }

    /// Navigate on behalf of the browser UI; `transition` is recorded in history if the
    /// navigation reaches a document.
    pub fn navigate_with_transition(&mut self, url: &str, transition: VisitTransition) {
        let id = self.context.navigate_to(url);
        self.pending_visit = Some((id, transition));
        self.sync_after_navigation();
    }

    pub fn navigate(&mut self, url: &str) {
        self.navigate_with_transition(url, VisitTransition::Other);
    }

    pub fn reload(&mut self) {
        self.context.reload();
        self.sync_after_navigation();
    }

    pub fn back(&mut self) {
        self.context.back();
        self.sync_after_navigation();
    }

    pub fn forward(&mut self) {
        self.context.forward();
        self.sync_after_navigation();
    }

    /// History transition for navigation `id`: the one the browser UI started it with,
    /// else derived from its cause. `None` for back/forward (not a new visit).
    pub fn visit_transition(
        &self,
        id: NavigationId,
        cause: NavigationCause,
    ) -> Option<VisitTransition> {
        match cause {
            NavigationCause::History => None,
            NavigationCause::Reload => Some(VisitTransition::Reload),
            NavigationCause::Link | NavigationCause::Script => Some(VisitTransition::Link),
            NavigationCause::FormSubmission => Some(VisitTransition::FormSubmit),
            NavigationCause::Embedder => Some(match self.pending_visit {
                Some((pid, t)) if pid == id => t,
                _ => VisitTransition::Other,
            }),
        }
    }

    pub fn sync_after_navigation(&mut self) {
        self.last_url = self.url();
        self.refresh_origin();
    }

    pub fn tick(&mut self) -> HudStats {
        self.context.tick()
    }

    pub fn framebuffer(&self) -> Option<&Framebuffer> {
        self.context.framebuffer()
    }

    fn refresh_origin(&mut self) {
        if let Ok(url) = Url::parse(&self.context.page.url) {
            self.security_origin = SecurityOrigin::from_url(&url);
        } else if self.context.page.url.starts_with("file:") {
            self.security_origin = SecurityOrigin {
                origin: Origin {
                    scheme: "file".into(),
                    host: String::new(),
                    port: 0,
                },
                is_opaque: false,
            };
        } else if self.context.page.url.starts_with("axiom://") {
            if let Ok(url) = Url::parse(&self.context.page.url) {
                self.security_origin = SecurityOrigin::from_url(&url);
            } else {
                self.security_origin = SecurityOrigin::opaque();
            }
        } else {
            self.security_origin = SecurityOrigin::opaque();
        }
    }

    /// Public for internal navigation path.
    pub fn refresh_origin_public(&mut self) {
        self.refresh_origin();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
