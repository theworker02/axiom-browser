//! Versioned typed settings store.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::search::{SearchProvider, SearchProviderService, CUSTOM, DEFAULT_PROVIDER_ID};

/// v2 (Wave F): `search_provider_id`.
pub const SETTINGS_SCHEMA_VERSION: i32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserSettings {
    pub schema_version: i32,
    pub homepage: String,
    pub new_tab_url: String,
    /// Selected [`SearchProvider`] id (`google`, `bing`, `duckduckgo`, `axiom`, `custom`).
    pub search_provider_id: String,
    /// Display name and template of the selected provider; the template is what a
    /// `custom` provider uses.
    pub search_provider_name: String,
    pub search_url_template: String,
    pub restore_previous_session: bool,
    pub theme: String,
    pub download_directory: String,
    pub performance_hud: bool,
    pub developer_features: bool,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        let provider = SearchProvider::duckduckgo();
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            homepage: "axiom://newtab".into(),
            new_tab_url: "axiom://newtab".into(),
            search_provider_id: provider.id,
            search_provider_name: provider.name,
            search_url_template: provider.search_url_template,
            restore_previous_session: true,
            theme: "system".into(),
            download_directory: String::new(),
            performance_hud: true,
            developer_features: false,
        }
    }
}

impl BrowserSettings {
    pub fn validate(&mut self) {
        if self.theme != "light" && self.theme != "dark" && self.theme != "system" {
            self.theme = "system".into();
        }
        if self.new_tab_url.is_empty() {
            self.new_tab_url = "axiom://newtab".into();
        }
        if self.schema_version > SETTINGS_SCHEMA_VERSION {
            // Newer unknown fields already ignored by serde; keep running.
            log::warn!(
                target: "axiom_persist",
                "settings schema {} newer than {}; using compatible subset",
                self.schema_version,
                SETTINGS_SCHEMA_VERSION
            );
        }
        if self.schema_version < 2 {
            self.search_provider_id = provider_id_for_template(&self.search_url_template);
        }
        let service = self.search_service();
        if service.default_provider().id != self.search_provider_id {
            log::warn!(
                target: "axiom_persist",
                "unknown search provider {:?}; using {DEFAULT_PROVIDER_ID}",
                self.search_provider_id
            );
        }
        self.set_search_provider(service.default_provider());
        self.schema_version = SETTINGS_SCHEMA_VERSION;
    }

    /// The providers this profile can choose from, with its selection applied.
    pub fn search_service(&self) -> SearchProviderService {
        let custom = SearchProvider::custom(&self.search_provider_name, &self.search_url_template)
            .filter(|_| self.search_provider_id == CUSTOM);
        SearchProviderService::with_selection(&self.search_provider_id, custom)
    }

    pub fn set_search_provider(&mut self, provider: &SearchProvider) {
        self.search_provider_id = provider.id.clone();
        self.search_provider_name = provider.name.clone();
        self.search_url_template = provider.search_url_template.clone();
    }

    /// Settings for a private window opened from this profile: preferences carry over,
    /// nothing that records activity or touches disk does.
    pub fn for_private(&self) -> Self {
        Self {
            restore_previous_session: false,
            download_directory: String::new(),
            ..self.clone()
        }
    }
}

/// Settings v1 stored only a name and template: map known templates back to their ids.
fn provider_id_for_template(template: &str) -> String {
    SearchProviderService::builtin()
        .providers()
        .iter()
        .find(|p| p.search_url_template == template)
        .map(|p| p.id.clone())
        .unwrap_or_else(|| {
            if SearchProvider::custom("", template).is_some() {
                CUSTOM.into()
            } else {
                DEFAULT_PROVIDER_ID.into()
            }
        })
}

pub struct SettingsRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SettingsRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn load(&self) -> rusqlite::Result<BrowserSettings> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = 'browser'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let mut settings = match json {
            Some(j) => serde_json::from_str(&j).unwrap_or_else(|e| {
                log::warn!(target: "axiom_persist", "unreadable settings ({e}); using defaults");
                BrowserSettings::default()
            }),
            None => BrowserSettings::default(),
        };
        settings.validate();
        Ok(settings)
    }

    pub fn save(&self, settings: &BrowserSettings) -> rusqlite::Result<()> {
        let mut s = settings.clone();
        s.validate();
        let json = serde_json::to_string(&s).unwrap_or_else(|_| "{}".into());
        self.conn.execute(
            "INSERT INTO settings (key, value_json) VALUES ('browser', ?1)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![json],
        )?;
        Ok(())
    }

    pub fn get_raw(&self, key: &str) -> rusqlite::Result<Option<Value>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_settings_migrate_to_provider_ids() {
        let v1 = r#"{"schema_version":1,"homepage":"axiom://newtab","new_tab_url":"axiom://newtab",
            "search_provider_name":"Google","search_url_template":"https://www.google.com/search?q={searchTerms}",
            "restore_previous_session":false,"theme":"dark","download_directory":"",
            "performance_hud":true,"developer_features":false}"#;
        let mut s: BrowserSettings = serde_json::from_str(v1).unwrap();
        s.validate();
        assert_eq!(s.search_provider_id, "google");
        assert_eq!(s.theme, "dark", "other v1 fields survive");
        assert!(!s.restore_previous_session);
        assert_eq!(s.schema_version, SETTINGS_SCHEMA_VERSION);

        let mut custom: BrowserSettings = serde_json::from_str(
            r#"{"schema_version":1,"search_provider_name":"Mine","search_url_template":"https://s.example/?q={searchTerms}"}"#,
        )
        .unwrap();
        custom.validate();
        assert_eq!(custom.search_provider_id, "custom");
        assert_eq!(
            custom.search_service().search_url("a b"),
            "https://s.example/?q=a+b"
        );
    }

    #[test]
    fn unknown_provider_falls_back_to_the_default() {
        let mut s = BrowserSettings {
            search_provider_id: "altavista".into(),
            ..BrowserSettings::default()
        };
        s.validate();
        assert_eq!(s.search_provider_id, DEFAULT_PROVIDER_ID);
    }

    #[test]
    fn private_settings_keep_preferences_only() {
        let mut s = BrowserSettings {
            download_directory: "D:/dl".into(),
            ..BrowserSettings::default()
        };
        s.set_search_provider(&SearchProvider::google());
        let p = s.for_private();
        assert_eq!(p.search_provider_id, "google");
        assert!(!p.restore_previous_session);
        assert!(p.download_directory.is_empty());
    }
}
