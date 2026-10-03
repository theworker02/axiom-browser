//! Multi-value HTTP headers (preserves Set-Cookie multiplicity).

use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct HeaderMap {
    map: BTreeMap<String, Vec<String>>,
}

impl HeaderMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        let key = name.to_ascii_lowercase();
        self.map.insert(key, vec![value.into()]);
    }

    pub fn append(&mut self, name: &str, value: impl Into<String>) {
        let key = name.to_ascii_lowercase();
        self.map.entry(key).or_default().push(value.into());
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.map
            .get(&name.to_ascii_lowercase())
            .and_then(|v| v.first())
            .map(|s| s.as_str())
    }

    pub fn get_all(&self, name: &str) -> &[String] {
        self.map
            .get(&name.to_ascii_lowercase())
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.map.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.map.contains_key(&name.to_ascii_lowercase())
    }

    pub fn remove(&mut self, name: &str) {
        self.map.remove(&name.to_ascii_lowercase());
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Copy with credential-bearing values replaced, for logs and `axiom://network`.
    pub fn redacted(&self) -> HeaderMap {
        let mut out = self.clone();
        for (name, values) in out.map.iter_mut() {
            if SENSITIVE_HEADERS.contains(&name.as_str()) {
                for v in values.iter_mut() {
                    *v = "<redacted>".to_string();
                }
            }
        }
        out
    }
}

const SENSITIVE_HEADERS: &[&str] = &[
    "cookie",
    "set-cookie",
    "authorization",
    "proxy-authorization",
];
