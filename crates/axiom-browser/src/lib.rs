//! Axiom browser application layer (Phase 3).
//!
//! Tabs are **independent browsing contexts**, not cosmetic labels.
//! Browser chrome is trusted UI; web content cannot draw into it.
//! Profile-owned repositories own durable browser data (Wave C).
//! CookieService is the single cookie model for HTTP + JS (Wave D).

mod bookmarks_repo;
mod browser;
mod chrome;
mod classifier;
mod clipboard;
mod cookie_bridge;
mod cookie_parse;
mod cookie_psl;
mod cookie_repo;
mod cookie_service;
mod cookie_types;
mod history_repo;
mod internal;
mod lock;
mod omnibox;
mod origin;
mod paths;
mod profile;
mod schema;
mod search;
mod search_pages;
mod session;
mod session_repo;
mod settings_repo;
mod storage_bridge;
mod storage_repo;
mod storage_service;
mod storage_types;
mod store;
mod suggestions;
mod tab;
mod tab_manager;
mod time_util;

pub use bookmarks_repo::{Bookmark, BookmarkFolder, BookmarkRepository};
pub use browser::{Browser, BrowserWindow};
pub use chrome::{ChromeControl, ChromeState, FocusOwner, ReloadStopMode, SecurityDisplay};
pub use classifier::{ClassifiedInput, OmniboxInput, OmniboxInputClassifier};
pub use cookie_bridge::ProfileCookieJar;
pub use cookie_parse::{parse_cookie_date, parse_set_cookie};
pub use cookie_psl::{
    HeuristicPublicSuffixProvider, PslPublicSuffixProvider, PublicSuffixProvider,
};
pub use cookie_repo::CookieRepository;
pub use cookie_service::{
    fuzz_parse_set_cookie, Clock, CookieLimits, CookiePolicy, CookieService, FixedClock,
    ProcessResult, SystemClock,
};
pub use cookie_types::{
    Cookie, CookieAccessContext, CookieExpiration, CookieId, CookieInspectRecord, CookieSameSite,
    CookieSource, HttpMethodKind, NavigationKind, Site,
};
pub use history_repo::{HistoryRecord, HistoryRepository, VisitTransition};
pub use internal::{InternalPage, InternalPageRegistry};
pub use lock::{LockError, ProfileLock};
pub use omnibox::Omnibox;
pub use origin::{Origin, SecurityOrigin};
pub use paths::ProfilePaths;
pub use profile::{Profile, ProfileId, ProfileKind, ProfileManager, ProfileMetadata};
pub use schema::SCHEMA_VERSION;
pub use search::{SearchProvider, SearchProviderService, UnknownSearchProvider};
pub use session::{SessionSnapshot, SessionTab};
pub use settings_repo::{BrowserSettings, SettingsRepository};
pub use storage_bridge::make_storage_binder;
pub use storage_service::{OriginStorageAccess, ProfileStorage, SessionStorageMap, StorageService};
pub use storage_types::{StorageEntry, StorageError, StorageKind, StorageOrigin, StorageQuota};
pub use store::{BrowserDataStore, StoreError};
pub use suggestions::{Suggestion, SuggestionKind, SuggestionModel, SuggestionSource};
pub use tab::{Tab, TabId, TabLifecycle};
pub use tab_manager::{ClosedTab, TabManager};

/// Layout constants for trusted chrome (must match gfx).
pub const TAB_STRIP_H: u32 = 32;
pub const TOOLBAR_H: u32 = 44;
pub const STATUS_H: u32 = 22;
pub const TOP_CHROME_H: u32 = TAB_STRIP_H + TOOLBAR_H;
pub const CHROME_H: u32 = TOP_CHROME_H + STATUS_H;

/// Preferred visible tab width; overflow scrolls rather than shrinking forever.
pub const TAB_MIN_W: u32 = 120;
pub const TAB_MAX_W: u32 = 180;
