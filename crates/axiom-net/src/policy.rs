//! Network policy hooks (content blocking, mixed-content, enterprise rules, …).
//!
//! Policies run inside the service for every hop — including each redirect target — and
//! again on the final response headers. A rejection surfaces as [`NetworkError::Blocked`].
//!
//! [`NetworkError::Blocked`]: crate::NetworkError::Blocked

use crate::redirect::RedirectRecord;
use crate::request::NetworkRequest;
use crate::response::ResponseMeta;

pub trait NetworkPolicy: Send + Sync {
    fn name(&self) -> &str;

    /// Called before each hop is sent. `redirect` is the record that led to this hop.
    fn check_request(
        &self,
        _req: &NetworkRequest,
        _redirect: Option<&RedirectRecord>,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Called once response headers for the final hop are known.
    fn check_response(&self, _req: &NetworkRequest, _meta: &ResponseMeta) -> Result<(), String> {
        Ok(())
    }
}

/// Blocks requests whose host matches (or is a subdomain of) any listed host.
pub struct HostBlocklist {
    hosts: Vec<String>,
}

impl HostBlocklist {
    pub fn new<I, S>(hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            hosts: hosts
                .into_iter()
                .map(|h| h.into().to_ascii_lowercase())
                .collect(),
        }
    }
}

impl NetworkPolicy for HostBlocklist {
    fn name(&self) -> &str {
        "host-blocklist"
    }

    fn check_request(
        &self,
        req: &NetworkRequest,
        _redirect: Option<&RedirectRecord>,
    ) -> Result<(), String> {
        let host = req.url.host.to_ascii_lowercase();
        for blocked in &self.hosts {
            if host == *blocked || host.ends_with(&format!(".{blocked}")) {
                return Err(format!("host {host} is blocked"));
            }
        }
        Ok(())
    }
}
