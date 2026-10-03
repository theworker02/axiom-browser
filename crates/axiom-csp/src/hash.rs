//! SHA-2 digests and base64 as used by CSP hash sources and Subresource Integrity.

use ring::digest;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HashAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlgorithm {
    /// `sha256` / `sha384` / `sha512`, ASCII case-insensitive.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "sha256" => Some(Self::Sha256),
            "sha384" => Some(Self::Sha384),
            "sha512" => Some(Self::Sha512),
            _ => None,
        }
    }

    pub fn digest(self, data: &[u8]) -> Vec<u8> {
        let alg = match self {
            Self::Sha256 => &digest::SHA256,
            Self::Sha384 => &digest::SHA384,
            Self::Sha512 => &digest::SHA512,
        };
        digest::digest(alg, data).as_ref().to_vec()
    }

    /// The digest of `data` as unpadded standard base64 (comparable with
    /// [`normalize_base64`] output).
    pub fn digest_base64(self, data: &[u8]) -> String {
        base64_encode(&self.digest(data))
    }
}

/// True when `s` is a non-empty base64 or base64url value (padding allowed).
pub fn is_base64_value(s: &str) -> bool {
    let body = s.trim_end_matches('=');
    !body.is_empty()
        && s.len() - body.len() <= 2
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/-_".contains(&b))
}

/// Map base64url to base64 and drop padding so both spellings compare equal.
pub fn normalize_base64(s: &str) -> String {
    s.trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            c => c,
        })
        .collect()
}

/// Standard base64 without padding.
pub fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = match chunk.len() {
            3 => (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]),
            2 => (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8),
            _ => u32::from(chunk[0]) << 16,
        };
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg");
        assert_eq!(base64_encode(b"fo"), "Zm8");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_values() {
        assert!(is_base64_value("abc+/="));
        assert!(is_base64_value("abc-_"));
        assert!(!is_base64_value(""));
        assert!(!is_base64_value("==="));
        assert!(!is_base64_value("a*b"));
        assert_eq!(normalize_base64("a-b_=="), "a+b/");
    }
}
