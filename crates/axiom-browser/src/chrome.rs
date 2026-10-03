//! Browser chrome state — derived from the active tab; no duplicate URL authority.

use axiom_engine::{DocumentReadyState, SecurityState};

use crate::omnibox::Omnibox;
use crate::origin::Origin;
use crate::suggestions::SuggestionModel;
use crate::tab::TabId;

/// Where keyboard events are routed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusOwner {
    BrowserChrome,
    WebContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeControl {
    None,
    Omnibox,
    Back,
    Forward,
    ReloadStop,
    SecurityIndicator,
    TabStrip,
    NewTab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityDisplay {
    /// Committed over TLS with a verified certificate.
    Https,
    /// Secure document that loaded plain-HTTP subresources.
    Mixed,
    Http,
    Local,
    Internal,
    /// The navigation failed certificate validation.
    CertificateError,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadStopMode {
    Reload,
    Stop,
}

/// Snapshot of chrome for painting / tests — derived, not authoritative for URL.
#[derive(Debug, Clone)]
pub struct ChromeState {
    pub active_tab: TabId,
    pub tab_title: String,
    pub tab_url: String,
    pub loading: bool,
    pub ready: DocumentReadyState,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub reload_stop: ReloadStopMode,
    pub security: SecurityDisplay,
    /// Factual connection line for the site-info panel.
    pub connection: String,
    pub origin: Option<Origin>,
    pub focus: FocusOwner,
    pub focused_control: ChromeControl,
    pub status_text: String,
    pub window_title: String,
    pub site_info_open: bool,
    pub private: bool,
    pub bookmarked: bool,
}

pub struct BrowserChrome {
    pub focus: FocusOwner,
    pub focused_control: ChromeControl,
    pub omnibox: Omnibox,
    pub suggestions: SuggestionModel,
    pub status_text: String,
    pub site_info_open: bool,
    /// Tab strip scroll offset (overflow architecture).
    pub tab_strip_offset: usize,
}

impl Default for BrowserChrome {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserChrome {
    pub fn new() -> Self {
        Self {
            focus: FocusOwner::WebContent,
            focused_control: ChromeControl::None,
            omnibox: Omnibox::new(),
            suggestions: SuggestionModel::default(),
            status_text: String::new(),
            site_info_open: false,
            tab_strip_offset: 0,
        }
    }

    pub fn focus_omnibox(&mut self, url: &str) {
        self.focus = FocusOwner::BrowserChrome;
        self.focused_control = ChromeControl::Omnibox;
        self.omnibox.focus_select_all(url);
    }

    pub fn focus_content(&mut self) {
        self.focus = FocusOwner::WebContent;
        self.focused_control = ChromeControl::None;
        self.omnibox.blur_commit();
        self.suggestions.clear();
        self.site_info_open = false;
    }

    pub fn sync_omnibox_from_tab(&mut self, url: &str) {
        self.omnibox.sync_from_url(url);
    }
}

/// One factual line for the site-info panel, from the committed connection.
pub fn connection_detail(security: &SecurityState, mixed_content: bool) -> String {
    match security {
        SecurityState::Secure(tls) => {
            let mut s = format!("Verified TLS, {}", tls.alpn.as_str());
            if let Some(cert) = &tls.certificate {
                s.push_str(&format!(", issuer {}", cert.issuer));
            }
            if mixed_content {
                s.push_str(" (insecure subresources)");
            }
            s
        }
        SecurityState::Insecure => "Not encrypted (HTTP)".into(),
        SecurityState::CertificateError(kind) => kind.describe().into(),
        SecurityState::NotNetwork => "Not loaded from the network".into(),
    }
}

/// Security indicator from the committed document's connection facts. An `https://` URL
/// alone never produces [`SecurityDisplay::Https`]; only a verified TLS response does.
pub fn security_display(
    url: &str,
    security: &SecurityState,
    mixed_content: bool,
    nav_failed: bool,
) -> SecurityDisplay {
    if let SecurityState::CertificateError(_) = security {
        return SecurityDisplay::CertificateError;
    }
    if nav_failed {
        return SecurityDisplay::Failed;
    }
    match security {
        SecurityState::Secure(tls) if tls.certificate_verified => {
            if mixed_content {
                SecurityDisplay::Mixed
            } else {
                SecurityDisplay::Https
            }
        }
        SecurityState::Insecure => SecurityDisplay::Http,
        _ => {
            let lower = url.to_ascii_lowercase();
            if lower.starts_with("axiom://") || lower.starts_with("about:") {
                SecurityDisplay::Internal
            } else if lower.starts_with("file:") {
                SecurityDisplay::Local
            } else {
                SecurityDisplay::Unknown
            }
        }
    }
}

pub fn window_title_for(page_title: &str) -> String {
    let t = page_title.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("new tab") {
        "Axiom".into()
    } else {
        format!("{t} — Axiom")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_net::{CertificateErrorKind, HttpProtocol, TlsInfo};

    fn verified_tls() -> SecurityState {
        SecurityState::Secure(Box::new(TlsInfo {
            alpn: HttpProtocol::Http2,
            certificate_verified: true,
            version: None,
            cipher_suite: None,
            hostname: Some("example.com".into()),
            certificate: None,
        }))
    }

    #[test]
    fn https_indicator_requires_a_verified_tls_response() {
        let url = "https://example.com/";
        assert_eq!(
            security_display(url, &verified_tls(), false, false),
            SecurityDisplay::Https
        );
        // The scheme alone never earns the secure indicator.
        assert_eq!(
            security_display(url, &SecurityState::NotNetwork, false, false),
            SecurityDisplay::Unknown
        );
        assert_eq!(
            security_display(url, &verified_tls(), true, false),
            SecurityDisplay::Mixed
        );
        assert_eq!(
            security_display(
                "http://example.com/",
                &SecurityState::Insecure,
                false,
                false
            ),
            SecurityDisplay::Http
        );
    }

    #[test]
    fn certificate_errors_and_failures_are_distinct() {
        let bad = SecurityState::CertificateError(CertificateErrorKind::Expired);
        assert_eq!(
            security_display("https://expired.test/", &bad, false, true),
            SecurityDisplay::CertificateError
        );
        assert_eq!(
            security_display(
                "https://down.test/",
                &SecurityState::NotNetwork,
                false,
                true
            ),
            SecurityDisplay::Failed
        );
        assert!(connection_detail(&bad, false)
            .to_ascii_lowercase()
            .contains("expired"));
        assert_eq!(
            security_display("axiom://network", &SecurityState::NotNetwork, false, false),
            SecurityDisplay::Internal
        );
    }

    #[test]
    fn connection_detail_states_only_measured_facts() {
        let s = connection_detail(&verified_tls(), false);
        assert!(s.starts_with("Verified TLS"), "{s}");
        assert!(
            !s.contains("TLS 1."),
            "version is not exposed by the transport"
        );
        assert!(connection_detail(&verified_tls(), true).contains("insecure subresources"));
        assert_eq!(
            connection_detail(&SecurityState::Insecure, false),
            "Not encrypted (HTTP)"
        );
    }
}
