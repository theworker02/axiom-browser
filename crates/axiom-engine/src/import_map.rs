//! Import maps (`<script type="importmap">`): module specifiers remapped to URLs,
//! for the whole document and per scope (HTML "resolve a module specifier").

use axiom_js::{is_url_like_specifier, MODULE_RESOLVE_FAILURE};
use serde_json::{Map, Value};

use crate::fetch::resolve_url;

/// Normalized specifier key → URL (`None`: an invalid entry, which blocks the key).
/// Kept in descending code-point order so longer prefixes are tried first.
type SpecifierMap = Vec<(String, Option<String>)>;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ImportMap {
    imports: SpecifierMap,
    /// Scope prefix URL → its map, most specific prefix first.
    scopes: Vec<(String, SpecifierMap)>,
}

/// Why an import map was rejected: reported to script as this kind of error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportMapError {
    Syntax(String),
    Type(String),
}

fn parse_url_like(specifier: &str, base: &str) -> Option<String> {
    if is_url_like_specifier(specifier) {
        resolve_url(base, specifier)
    } else {
        None
    }
}

fn is_special(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, _)| {
        matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "ws" | "wss" | "ftp" | "file"
        )
    })
}

fn sort(map: &mut SpecifierMap) {
    map.sort_by(|a, b| b.0.cmp(&a.0));
}

fn normalize(entries: &Map<String, Value>, base: &str, warnings: &mut Vec<String>) -> SpecifierMap {
    let mut map = SpecifierMap::new();
    for (key, value) in entries {
        if key.is_empty() {
            warnings.push("empty specifier key ignored".into());
            continue;
        }
        let key = parse_url_like(key, base).unwrap_or_else(|| key.clone());
        let address = match value.as_str() {
            Some(v) => parse_url_like(v, base),
            None => None,
        };
        let address = match address {
            Some(a) if key.ends_with('/') && !a.ends_with('/') => {
                warnings.push(format!(
                    "\"{key}\" maps a prefix to \"{a}\", which lacks a trailing /"
                ));
                None
            }
            Some(a) => Some(a),
            None => {
                warnings.push(format!("\"{key}\" has an invalid address {value}"));
                None
            }
        };
        map.push((key, address));
    }
    sort(&mut map);
    map
}

fn merge_map(into: &mut SpecifierMap, new: SpecifierMap) {
    for (key, value) in new {
        if !into.iter().any(|(k, _)| *k == key) {
            into.push((key, value));
        }
    }
    sort(into);
}

fn resolve_match(
    normalized: &str,
    as_url: Option<&str>,
    map: &SpecifierMap,
) -> Result<Option<String>, String> {
    let blocked =
        || format!("{MODULE_RESOLVE_FAILURE} \"{normalized}\": blocked by the import map");
    for (key, address) in map {
        if key == normalized {
            return address.clone().map(Some).ok_or_else(blocked);
        }
        let prefix_match = key.ends_with('/')
            && normalized.starts_with(key.as_str())
            && as_url.is_none_or(is_special);
        if prefix_match {
            let address = address.as_deref().ok_or_else(blocked)?;
            let rest = &normalized[key.len()..];
            let url = resolve_url(address, rest)
                .filter(|u| u.starts_with(address))
                .ok_or_else(|| {
                    format!("{MODULE_RESOLVE_FAILURE} \"{normalized}\": it escapes \"{address}\"")
                })?;
            return Ok(Some(url));
        }
    }
    Ok(None)
}

impl ImportMap {
    /// Parse an import map's JSON text; relative addresses resolve against `base`.
    /// Invalid entries are dropped (and block their key) with a warning.
    pub fn parse(text: &str, base: &str) -> Result<(ImportMap, Vec<String>), ImportMapError> {
        let json: Value =
            serde_json::from_str(text).map_err(|e| ImportMapError::Syntax(e.to_string()))?;
        let Value::Object(top) = json else {
            return Err(ImportMapError::Type(
                "the import map must be a JSON object".into(),
            ));
        };
        let mut warnings = Vec::new();
        let mut map = ImportMap::default();
        match top.get("imports") {
            None => {}
            Some(Value::Object(imports)) => map.imports = normalize(imports, base, &mut warnings),
            Some(_) => {
                return Err(ImportMapError::Type(
                    "\"imports\" must be a JSON object".into(),
                ))
            }
        }
        match top.get("scopes") {
            None => {}
            Some(Value::Object(scopes)) => {
                for (prefix, entries) in scopes {
                    let Value::Object(entries) = entries else {
                        return Err(ImportMapError::Type(format!(
                            "scope \"{prefix}\" must be a JSON object"
                        )));
                    };
                    let Some(prefix_url) = resolve_url(base, prefix) else {
                        warnings.push(format!("scope prefix \"{prefix}\" is not a URL"));
                        continue;
                    };
                    let entries = normalize(entries, base, &mut warnings);
                    map.scopes.push((prefix_url, entries));
                }
                map.scopes.sort_by(|a, b| b.0.cmp(&a.0));
            }
            Some(_) => {
                return Err(ImportMapError::Type(
                    "\"scopes\" must be a JSON object".into(),
                ))
            }
        }
        for key in top.keys() {
            if !matches!(key.as_str(), "imports" | "scopes" | "integrity") {
                warnings.push(format!("unknown top-level key \"{key}\""));
            }
        }
        Ok((map, warnings))
    }

    /// Add a later import map: entries already defined keep their earlier mapping.
    pub fn merge(&mut self, new: ImportMap) {
        merge_map(&mut self.imports, new.imports);
        for (prefix, entries) in new.scopes {
            match self.scopes.iter_mut().find(|(p, _)| *p == prefix) {
                Some((_, existing)) => merge_map(existing, entries),
                None => self.scopes.push((prefix, entries)),
            }
        }
        self.scopes.sort_by(|a, b| b.0.cmp(&a.0));
    }

    /// Resolve `specifier` imported by the script or module at `base`.
    pub fn resolve(&self, specifier: &str, base: &str) -> Result<String, String> {
        let as_url = parse_url_like(specifier, base);
        let normalized = as_url.clone().unwrap_or_else(|| specifier.to_string());
        for (prefix, entries) in &self.scopes {
            let in_scope = prefix == base || (prefix.ends_with('/') && base.starts_with(prefix));
            if in_scope {
                if let Some(url) = resolve_match(&normalized, as_url.as_deref(), entries)? {
                    return Ok(url);
                }
            }
        }
        if let Some(url) = resolve_match(&normalized, as_url.as_deref(), &self.imports)? {
            return Ok(url);
        }
        as_url.ok_or_else(|| {
            format!(
                "{MODULE_RESOLVE_FAILURE} \"{specifier}\": bare specifiers must be mapped by an import map; relative references must start with \"/\", \"./\" or \"../\""
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://example.com/app/index.html";

    fn map(json: &str) -> ImportMap {
        ImportMap::parse(json, BASE).expect("valid import map").0
    }

    #[test]
    fn bare_prefix_and_url_specifiers_resolve() {
        let m = map(r#"{"imports": {
            "react": "/vendor/react.js",
            "lib/": "./lib/",
            "https://cdn.test/x.js": "/local/x.js"
        }}"#);
        assert_eq!(
            m.resolve("react", BASE).unwrap(),
            "https://example.com/vendor/react.js"
        );
        assert_eq!(
            m.resolve("lib/a/b.js", BASE).unwrap(),
            "https://example.com/app/lib/a/b.js"
        );
        assert_eq!(
            m.resolve("https://cdn.test/x.js", BASE).unwrap(),
            "https://example.com/local/x.js"
        );
        assert_eq!(
            m.resolve("./own.js", BASE).unwrap(),
            "https://example.com/app/own.js"
        );
        let err = m.resolve("vue", BASE).unwrap_err();
        assert!(err.starts_with(MODULE_RESOLVE_FAILURE), "{err}");
    }

    #[test]
    fn scopes_take_precedence_for_modules_inside_them() {
        let m = map(r#"{
            "imports": {"dep": "/dep-v2.js"},
            "scopes": {"/legacy/": {"dep": "/dep-v1.js"}}
        }"#);
        assert_eq!(
            m.resolve("dep", "https://example.com/legacy/main.js")
                .unwrap(),
            "https://example.com/dep-v1.js"
        );
        assert_eq!(
            m.resolve("dep", "https://example.com/app/main.js").unwrap(),
            "https://example.com/dep-v2.js"
        );
    }

    #[test]
    fn invalid_entries_block_and_bad_maps_are_rejected() {
        let (m, warnings) =
            ImportMap::parse(r#"{"imports": {"a": 1, "b/": "/no-slash"}}"#, BASE).unwrap();
        assert_eq!(warnings.len(), 2);
        assert!(m.resolve("a", BASE).unwrap_err().contains("blocked"));
        assert!(m.resolve("b/c.js", BASE).unwrap_err().contains("blocked"));
        assert!(matches!(
            ImportMap::parse("{", BASE),
            Err(ImportMapError::Syntax(_))
        ));
        assert!(matches!(
            ImportMap::parse("[]", BASE),
            Err(ImportMapError::Type(_))
        ));
        assert!(matches!(
            ImportMap::parse(r#"{"imports": []}"#, BASE),
            Err(ImportMapError::Type(_))
        ));
    }

    #[test]
    fn prefix_matches_cannot_escape_their_address() {
        let m = map(r#"{"imports": {"pkg/": "/pkg/"}}"#);
        assert!(m.resolve("pkg/../secret.js", BASE).is_err());
    }

    #[test]
    fn later_maps_do_not_override_earlier_entries() {
        let mut m = map(r#"{"imports": {"a": "/a1.js"}}"#);
        m.merge(map(r#"{"imports": {"a": "/a2.js", "b": "/b.js"}}"#));
        assert_eq!(m.resolve("a", BASE).unwrap(), "https://example.com/a1.js");
        assert_eq!(m.resolve("b", BASE).unwrap(), "https://example.com/b.js");
    }
}
