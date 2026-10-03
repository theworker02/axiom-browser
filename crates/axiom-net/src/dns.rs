//! DNS resolution.
//!
//! The configured [`DnsResolver`] is injected into the HTTP transport, so every connection
//! the transport opens uses it (there is no separate "pre-check" lookup). Lookups run on the
//! runtime's blocking pool because the system resolver is synchronous.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use crate::error::NetworkError;

pub trait DnsResolver: Send + Sync {
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, NetworkError>;
}

#[derive(Debug, Default)]
pub struct SystemDnsResolver;

impl DnsResolver for SystemDnsResolver {
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, NetworkError> {
        if host.is_empty() {
            return Err(NetworkError::Dns("empty host".into()));
        }
        if let Ok(ip) = host
            .trim_matches(|c| c == '[' || c == ']')
            .parse::<IpAddr>()
        {
            return Ok(vec![ip]);
        }
        match (host, 0).to_socket_addrs() {
            Ok(iter) => {
                let addrs: Vec<IpAddr> = iter.map(|a| a.ip()).collect();
                if addrs.is_empty() {
                    Err(NetworkError::Dns(format!("no addresses for {host}")))
                } else {
                    Ok(addrs)
                }
            }
            Err(e) => Err(NetworkError::Dns(format!("{host}: {e}"))),
        }
    }
}

/// Deterministic resolver for tests: only hosts registered with [`FixedDnsResolver::with`]
/// resolve; everything else fails with [`NetworkError::Dns`].
#[derive(Debug, Default, Clone)]
pub struct FixedDnsResolver {
    entries: HashMap<String, Vec<IpAddr>>,
}

impl FixedDnsResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, host: &str, ip: IpAddr) -> Self {
        self.entries
            .entry(host.to_ascii_lowercase())
            .or_default()
            .push(ip);
        self
    }
}

impl DnsResolver for FixedDnsResolver {
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, NetworkError> {
        match self.entries.get(&host.to_ascii_lowercase()) {
            Some(ips) if !ips.is_empty() => Ok(ips.clone()),
            _ => Err(NetworkError::Dns(format!("no fixed entry for {host}"))),
        }
    }
}

/// Error type carried through reqwest's error chain so DNS failures stay typed.
#[derive(Debug)]
pub(crate) struct DnsFailure(pub String);

impl std::fmt::Display for DnsFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dns failure: {}", self.0)
    }
}

impl std::error::Error for DnsFailure {}

pub(crate) struct ReqwestDnsAdapter {
    pub inner: Arc<dyn DnsResolver>,
}

impl reqwest::dns::Resolve for ReqwestDnsAdapter {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let inner = Arc::clone(&self.inner);
        let host = name.as_str().to_string();
        Box::pin(async move {
            let result = tokio::task::spawn_blocking(move || inner.resolve(&host)).await;
            let failure = |msg: String| -> Box<dyn std::error::Error + Send + Sync> {
                Box::new(DnsFailure(msg))
            };
            match result {
                Ok(Ok(ips)) if !ips.is_empty() => {
                    let addrs: Vec<SocketAddr> =
                        ips.into_iter().map(|ip| SocketAddr::new(ip, 0)).collect();
                    Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
                }
                Ok(Ok(_)) => Err(failure("no addresses".into())),
                Ok(Err(NetworkError::Dns(msg))) => Err(failure(msg)),
                Ok(Err(other)) => Err(failure(other.to_string())),
                Err(join) => Err(failure(join.to_string())),
            }
        })
    }
}
