//! Subresource Integrity (W3C SRI): parse `integrity` metadata and check a body against it.
//!
//! Supported hash algorithms are SHA-256, SHA-384 and SHA-512. Only the strongest
//! algorithm present in the metadata is compared, and any one matching digest of that
//! algorithm passes. Metadata without a single valid token imposes no check.

use axiom_csp::hash::{is_base64_value, normalize_base64, HashAlgorithm};

/// Parsed metadata reduced to the digests of its strongest algorithm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Integrity {
    algorithm: HashAlgorithm,
    /// Base64 digests normalized to the standard alphabet without padding.
    digests: Vec<String>,
}

/// Parse `integrity` metadata. `None` means there is nothing to check (empty metadata or
/// no token with a supported algorithm and a base64 value).
pub fn parse_integrity(metadata: &str) -> Option<Integrity> {
    let mut tokens: Vec<(HashAlgorithm, String)> = Vec::new();
    for token in metadata.split_ascii_whitespace() {
        // `alg-value?options`: options are reserved and ignored.
        let expression = token.split('?').next().unwrap_or("");
        let Some((alg, value)) = expression.split_once('-') else {
            continue;
        };
        let Some(alg) = HashAlgorithm::parse(alg) else {
            continue;
        };
        if is_base64_value(value) {
            tokens.push((alg, normalize_base64(value)));
        }
    }
    let strongest = tokens.iter().map(|(a, _)| *a).max()?;
    Some(Integrity {
        algorithm: strongest,
        digests: tokens
            .into_iter()
            .filter(|(a, _)| *a == strongest)
            .map(|(_, d)| d)
            .collect(),
    })
}

impl Integrity {
    pub fn matches(&self, body: &[u8]) -> bool {
        let actual = self.algorithm.digest_base64(body);
        self.digests.contains(&actual)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // printf 'alert(1)' | openssl dgst -sha256 -binary | base64
    const ALERT_SHA256: &str = "sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI=";

    #[test]
    fn matching_and_mismatching_digests() {
        let i = parse_integrity(ALERT_SHA256).unwrap();
        assert!(i.matches(b"alert(1)"));
        assert!(!i.matches(b"alert(2)"));
    }

    #[test]
    fn base64url_and_unpadded_values_are_accepted() {
        let url_safe = ALERT_SHA256
            .replace('+', "-")
            .trim_end_matches('=')
            .to_string();
        assert!(parse_integrity(&url_safe).unwrap().matches(b"alert(1)"));
    }

    #[test]
    fn only_the_strongest_algorithm_is_compared() {
        // A wrong SHA-512 digest outranks the correct SHA-256 one.
        let meta = format!("{ALERT_SHA256} sha512-AAAA");
        assert!(!parse_integrity(&meta).unwrap().matches(b"alert(1)"));
        // Several digests of the strongest algorithm: any match passes.
        let meta = format!("sha256-AAAA {ALERT_SHA256}?ignored-option");
        assert!(parse_integrity(&meta).unwrap().matches(b"alert(1)"));
    }

    #[test]
    fn unknown_or_malformed_tokens_impose_no_check() {
        assert!(parse_integrity("").is_none());
        assert!(parse_integrity("md5-abc sha1-abc").is_none());
        assert!(parse_integrity("sha256-").is_none());
        assert!(parse_integrity("sha256-not*base64").is_none());
    }
}
