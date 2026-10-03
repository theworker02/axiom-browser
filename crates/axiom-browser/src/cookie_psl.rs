//! Public Suffix List abstraction (Wave D / D.1).
//!
//! Default production provider: [`PslPublicSuffixProvider`] backed by the
//! maintained [`psl`] crate (Mozilla Public Suffix List, ICANN + private
//! sections, auto-updated on crates.io).
//!
//! [`HeuristicPublicSuffixProvider`] remains available for deterministic unit
//! tests that must not depend on the full list.

/// Provider used when validating `Domain=` attributes and computing site keys.
pub trait PublicSuffixProvider: Send + Sync {
    /// True if `domain` is a public suffix (cookies must not be set for it).
    fn is_public_suffix(&self, domain: &str) -> bool;

    /// Effective TLD+1 (registrable domain) for SameSite / site grouping.
    fn registrable_domain(&self, host: &str) -> String;
}

/// Production provider: Mozilla PSL via the `psl` crate.
#[derive(Debug, Default, Clone)]
pub struct PslPublicSuffixProvider;

impl PublicSuffixProvider for PslPublicSuffixProvider {
    fn is_public_suffix(&self, domain: &str) -> bool {
        let d = normalize_host(domain);
        if d.is_empty() {
            return true;
        }
        // IP literals and localhost-like names are not valid cookie Domain targets.
        if is_ip_literal(&d) || is_localhost_like(&d) {
            return true;
        }
        // Mozilla PSL: no eTLD+1 means the name is itself a (public or private) suffix.
        psl::domain(d.as_bytes()).is_none()
    }

    fn registrable_domain(&self, host: &str) -> String {
        let h = normalize_host(host);
        if h.is_empty() {
            return h;
        }
        if is_ip_literal(&h) || is_localhost_like(&h) {
            return h;
        }
        match psl::domain(h.as_bytes()) {
            Some(d) => std::str::from_utf8(d.as_bytes())
                .unwrap_or(&h)
                .to_ascii_lowercase(),
            None => h,
        }
    }
}

/// Conservative heuristic — kept for injectable tests; not the production default.
#[derive(Debug, Default, Clone)]
pub struct HeuristicPublicSuffixProvider;

impl PublicSuffixProvider for HeuristicPublicSuffixProvider {
    fn is_public_suffix(&self, domain: &str) -> bool {
        let d = normalize_host(domain);
        if d.is_empty() || is_ip_literal(&d) || is_localhost_like(&d) {
            return true;
        }
        !d.contains('.')
    }

    fn registrable_domain(&self, host: &str) -> String {
        let h = normalize_host(host);
        if h.is_empty() || is_ip_literal(&h) || is_localhost_like(&h) {
            return h;
        }
        let parts: Vec<&str> = h.split('.').collect();
        if parts.len() <= 2 {
            return h;
        }
        format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
    }
}

fn normalize_host(host: &str) -> String {
    let h = host.trim().trim_matches('.').to_ascii_lowercase();
    // Strip surrounding brackets from IPv6 literals if present.
    if h.starts_with('[') && h.ends_with(']') && h.len() > 2 {
        h[1..h.len() - 1].to_string()
    } else {
        h
    }
}

fn is_localhost_like(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost")
}

fn is_ip_literal(host: &str) -> bool {
    // IPv4
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return true;
    }
    // IPv6 (with or without brackets already stripped)
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn psl() -> PslPublicSuffixProvider {
        PslPublicSuffixProvider
    }

    #[test]
    fn ordinary_tlds() {
        let p = psl();
        assert!(p.is_public_suffix("com"));
        assert!(p.is_public_suffix("net"));
        assert!(!p.is_public_suffix("example.com"));
        assert_eq!(p.registrable_domain("example.com"), "example.com");
        assert_eq!(p.registrable_domain("sub.example.com"), "example.com");
    }

    #[test]
    fn multi_label_suffixes() {
        let p = psl();
        assert!(p.is_public_suffix("co.uk"));
        assert!(p.is_public_suffix("com.au"));
        assert!(!p.is_public_suffix("example.co.uk"));
        assert!(!p.is_public_suffix("example.com.au"));
        assert_eq!(p.registrable_domain("example.co.uk"), "example.co.uk");
        assert_eq!(p.registrable_domain("sub.example.co.uk"), "example.co.uk");
        assert_eq!(p.registrable_domain("www.example.com.au"), "example.com.au");
    }

    #[test]
    fn private_suffix_github_io() {
        let p = psl();
        // github.io is on the PRIVATE section of the PSL.
        assert!(p.is_public_suffix("github.io"));
        assert!(!p.is_public_suffix("pages.github.io"));
        assert_eq!(p.registrable_domain("pages.github.io"), "pages.github.io");
        assert_eq!(
            p.registrable_domain("docs.pages.github.io"),
            "pages.github.io"
        );
    }

    #[test]
    fn idn_and_punycode() {
        let p = psl();
        // Punycode form of 中国 (China) — xn--fiqs8s is a public suffix label under .xn--fiqs8s
        // Use well-known: xn--55qx5d.cn style from psl docs.
        let puny = "xn--85x722f.xn--55qx5d.cn";
        assert!(!p.is_public_suffix(puny));
        assert_eq!(p.registrable_domain(&format!("www.{puny}")), puny);

        // If Unicode host is provided, psl accepts UTF-8 bytes for many IDNs.
        let uni = "食狮.中国";
        if psl::domain(uni.as_bytes()).is_some() {
            assert!(!p.is_public_suffix(uni));
            assert_eq!(p.registrable_domain(&format!("www.{uni}")), uni);
        }
    }

    #[test]
    fn localhost_and_ips() {
        let p = psl();
        assert!(p.is_public_suffix("localhost"));
        assert!(p.is_public_suffix("foo.localhost"));
        assert!(p.is_public_suffix("127.0.0.1"));
        assert!(p.is_public_suffix("::1"));
        assert!(p.is_public_suffix("[::1]"));
        assert_eq!(p.registrable_domain("127.0.0.1"), "127.0.0.1");
        assert_eq!(p.registrable_domain("localhost"), "localhost");
        assert_eq!(p.registrable_domain("::1"), "::1");
    }

    #[test]
    fn heuristic_still_available() {
        let p = HeuristicPublicSuffixProvider;
        assert!(p.is_public_suffix("com"));
        // Heuristic incorrectly treats co.uk as registrable — that's why PSL exists.
        assert!(!p.is_public_suffix("co.uk"));
        // Last-two-labels only → wrong eTLD+1 for multi-label suffixes.
        assert_eq!(p.registrable_domain("a.b.example.co.uk"), "co.uk");
    }
}
