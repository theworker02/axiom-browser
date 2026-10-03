//! Namespaces and name validation (DOM §1.4, "validate and extract").

use std::fmt;

pub const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
pub const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
pub const MATHML_NAMESPACE: &str = "http://www.w3.org/1998/Math/MathML";
pub const XLINK_NAMESPACE: &str = "http://www.w3.org/1999/xlink";
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
pub const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// A DOM operation failure, named after the `DOMException` it becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomError {
    HierarchyRequest,
    NotFound,
    InvalidCharacter,
    Namespace,
    NotSupported,
}

impl DomError {
    /// The `DOMException` name.
    pub fn name(self) -> &'static str {
        match self {
            Self::HierarchyRequest => "HierarchyRequestError",
            Self::NotFound => "NotFoundError",
            Self::InvalidCharacter => "InvalidCharacterError",
            Self::Namespace => "NamespaceError",
            Self::NotSupported => "NotSupportedError",
        }
    }
}

impl fmt::Display for DomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for DomError {}

fn is_ascii_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

pub fn is_valid_namespace_prefix(s: &str) -> bool {
    !s.is_empty()
        && !s
            .chars()
            .any(|c| is_ascii_whitespace(c) || matches!(c, '\0' | '/' | '>'))
}

pub fn is_valid_attribute_local_name(s: &str) -> bool {
    !s.is_empty()
        && !s
            .chars()
            .any(|c| is_ascii_whitespace(c) || matches!(c, '\0' | '/' | '=' | '>'))
}

pub fn is_valid_element_local_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first.is_ascii_alphabetic() {
        return !s
            .chars()
            .any(|c| is_ascii_whitespace(c) || matches!(c, '\0' | '/' | '>'));
    }
    let wide = |c: char| c >= '\u{80}';
    (matches!(first, ':' | '_') || wide(first))
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '_') || wide(c))
}

pub fn is_valid_doctype_name(s: &str) -> bool {
    !s.chars()
        .any(|c| is_ascii_whitespace(c) || matches!(c, '\0' | '>'))
}

/// What a qualified name is validated for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameContext {
    Element,
    Attribute,
}

/// A validated namespace, prefix and local name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedName {
    pub namespace: Option<String>,
    pub prefix: Option<String>,
    pub local_name: String,
}

/// "Validate and extract a namespace and qualifiedName" (DOM §1.4). An empty namespace
/// means no namespace; the qualified name splits at its first colon.
pub fn validate_and_extract(
    namespace: Option<&str>,
    qualified_name: &str,
    context: NameContext,
) -> Result<QualifiedName, DomError> {
    let namespace = namespace.filter(|n| !n.is_empty());
    let (prefix, local_name) = match qualified_name.split_once(':') {
        Some((prefix, local)) => {
            if !is_valid_namespace_prefix(prefix) {
                return Err(DomError::InvalidCharacter);
            }
            (Some(prefix), local)
        }
        None => (None, qualified_name),
    };
    let valid_local = match context {
        NameContext::Attribute => is_valid_attribute_local_name(local_name),
        NameContext::Element => is_valid_element_local_name(local_name),
    };
    if !valid_local {
        return Err(DomError::InvalidCharacter);
    }
    if prefix.is_some() && namespace.is_none() {
        return Err(DomError::Namespace);
    }
    if prefix == Some("xml") && namespace != Some(XML_NAMESPACE) {
        return Err(DomError::Namespace);
    }
    let names_xmlns = qualified_name == "xmlns" || prefix == Some("xmlns");
    if names_xmlns != (namespace == Some(XMLNS_NAMESPACE)) {
        return Err(DomError::Namespace);
    }
    Ok(QualifiedName {
        namespace: namespace.map(str::to_string),
        prefix: prefix.map(str::to_string),
        local_name: local_name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(ns: Option<&str>, name: &str) -> Result<(Option<String>, String), DomError> {
        validate_and_extract(ns, name, NameContext::Element).map(|q| (q.prefix, q.local_name))
    }

    #[test]
    fn element_names_follow_the_relaxed_rules() {
        assert!(is_valid_element_local_name("f}oo"));
        assert!(is_valid_element_local_name("f<oo"));
        assert!(is_valid_element_local_name("\u{0BC6}foo"));
        assert!(!is_valid_element_local_name("}foo"));
        assert!(!is_valid_element_local_name("1foo"));
        assert!(!is_valid_element_local_name("foo>"));
        assert!(!is_valid_element_local_name("fo o"));
        assert!(!is_valid_element_local_name(""));
    }

    #[test]
    fn validate_and_extract_errors() {
        let ex = Some("http://example.com/");
        assert_eq!(check(None, "foo"), Ok((None, "foo".into())));
        assert_eq!(check(ex, "f:oo"), Ok((Some("f".into()), "oo".into())));
        assert_eq!(check(None, "f:oo"), Err(DomError::Namespace));
        assert_eq!(check(Some(""), "f:oo"), Err(DomError::Namespace));
        assert_eq!(check(None, ":foo"), Err(DomError::InvalidCharacter));
        assert_eq!(check(None, "foo:"), Err(DomError::InvalidCharacter));
        assert_eq!(check(None, "f::oo"), Err(DomError::Namespace));
        assert_eq!(check(ex, "xml:foo"), Err(DomError::Namespace));
        assert_eq!(
            check(Some(XML_NAMESPACE), "xml:foo"),
            Ok((Some("xml".into()), "foo".into()))
        );
        assert_eq!(check(None, "xmlns"), Err(DomError::Namespace));
        assert_eq!(
            check(Some(XMLNS_NAMESPACE), "foo"),
            Err(DomError::Namespace)
        );
        assert_eq!(
            check(Some(XMLNS_NAMESPACE), "xmlns:foo"),
            Ok((Some("xmlns".into()), "foo".into()))
        );
    }
}
