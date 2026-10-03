//! XML parser for Axiom: XML 1.0 (Fifth Edition) with Namespaces in XML 1.0, non-validating.
//!
//! Builds an [`axiom_dom::Document`] for script-created XML documents (`DOMParser` with an
//! XML type). Well-formedness errors are fatal, as in browsers; [`parse_for_dom_parser`]
//! turns them into the `parsererror` document the DOM Parsing spec describes.
//!
//! Not supported: DTD declarations beyond skipping the internal subset (so only the five
//! predefined entities and character references resolve), external entities (never
//! fetched, by design) and XML 1.1. CDATA sections become text.

use std::fmt;

use axiom_dom::{Document, NodeId, QualifiedName, XMLNS_NAMESPACE, XML_NAMESPACE};

/// Namespace of the `parsererror` root element of a document that failed to parse.
pub const PARSER_ERROR_NAMESPACE: &str = "http://www.mozilla.org/newlayout/xml/parsererror.xml";

/// A well-formedness or namespace error, with a 1-based position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (line {}, column {})",
            self.message, self.line, self.column
        )
    }
}

impl std::error::Error for XmlError {}

/// Parses a complete XML document.
pub fn parse(input: &str) -> Result<Document, XmlError> {
    Parser::new(input).parse()
}

/// `DOMParser` semantics: the parsed document, or on error a document whose root is a
/// `parsererror` element (in [`PARSER_ERROR_NAMESPACE`]) describing the error, with a
/// `sourcetext` child showing the offending line.
pub fn parse_for_dom_parser(input: &str) -> Document {
    match parse(input) {
        Ok(doc) => doc,
        Err(error) => error_document(&error, input),
    }
}

fn error_document(error: &XmlError, input: &str) -> Document {
    let mut doc = Document::new();
    let root = doc.document_id.expect("new documents have a root");
    let name = |local: &str| QualifiedName {
        namespace: Some(PARSER_ERROR_NAMESPACE.to_string()),
        prefix: None,
        local_name: local.to_string(),
    };
    let parsererror = doc.create_element_qualified(&name("parsererror"));
    doc.append_child(root, parsererror);
    let message = doc.create_text(&format!(
        "XML Parsing Error: {}\nLine Number {}, Column {}:",
        error.message, error.line, error.column
    ));
    doc.append_child(parsererror, message);
    let sourcetext = doc.create_element_qualified(&name("sourcetext"));
    doc.append_child(parsererror, sourcetext);
    let line = input.lines().nth(error.line - 1).unwrap_or("");
    let caret = format!("{line}\n{}^", "-".repeat(error.column.saturating_sub(1)));
    let caret = doc.create_text(&caret);
    doc.append_child(sourcetext, caret);
    doc
}

fn is_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn is_name_start(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}'
        | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}'
        | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// A start tag's name and attributes, before namespace processing.
struct StartTag {
    name: String,
    attrs: Vec<(String, String)>,
    empty: bool,
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    doc: Document,
    /// In-scope namespace declarations, innermost last: (prefix, URI), with an empty URI
    /// undeclaring the default namespace.
    declarations: Vec<(Option<String>, String)>,
    /// Per open element, the length of `declarations` before its own.
    scopes: Vec<usize>,
    text: String,
}

impl Parser {
    fn new(input: &str) -> Self {
        let input = input.strip_prefix('\u{feff}').unwrap_or(input);
        // End-of-line handling (XML §2.11).
        let chars = input
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .chars()
            .collect();
        Self {
            chars,
            pos: 0,
            doc: Document::new(),
            declarations: Vec::new(),
            scopes: Vec::new(),
            text: String::new(),
        }
    }

    fn error(&self, message: impl Into<String>) -> XmlError {
        let (mut line, mut column) = (1, 1);
        for &c in &self.chars[..self.pos.min(self.chars.len())] {
            if c == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        XmlError {
            message: message.into(),
            line,
            column,
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        (self.pos..)
            .zip(s.chars())
            .all(|(i, c)| self.chars.get(i) == Some(&c))
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.starts_with(s) {
            self.pos += s.chars().count();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, s: &str, what: &str) -> Result<(), XmlError> {
        if self.eat(s) {
            Ok(())
        } else {
            Err(self.error(format!("expected {what}")))
        }
    }

    fn skip_space(&mut self) -> bool {
        let start = self.pos;
        while self.peek().is_some_and(is_space) {
            self.pos += 1;
        }
        self.pos > start
    }

    fn name(&mut self) -> Result<String, XmlError> {
        match self.peek() {
            Some(c) if is_name_start(c) => {}
            _ => return Err(self.error("expected a name")),
        }
        let start = self.pos;
        while self.peek().is_some_and(is_name_char) {
            self.pos += 1;
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }

    /// Characters up to (not including) `end`, which is consumed.
    fn until(&mut self, end: &str, what: &str) -> Result<String, XmlError> {
        let start = self.pos;
        while !self.starts_with(end) {
            match self.peek() {
                None => return Err(self.error(format!("unterminated {what}"))),
                Some(c) if !is_char(c) => return Err(self.error("invalid character")),
                Some(_) => self.pos += 1,
            }
        }
        let s = self.chars[start..self.pos].iter().collect();
        self.pos += end.chars().count();
        Ok(s)
    }

    fn parse(mut self) -> Result<Document, XmlError> {
        let root = self.doc.document_id.expect("new documents have a root");
        if self.starts_with("<?xml")
            && self
                .chars
                .get(self.pos + 5)
                .is_some_and(|&c| is_space(c) || c == '?')
        {
            self.xml_declaration()?;
        }
        self.misc(root, true)?;
        if self.at_end() {
            return Err(self.error("no root element"));
        }
        if !self.starts_with("<") || self.starts_with("<!") || self.starts_with("<?") {
            return Err(self.error("content is not allowed in the prolog"));
        }
        self.elements(root)?;
        self.misc(root, false)?;
        if !self.at_end() {
            return Err(self.error("extra content at the end of the document"));
        }
        Ok(self.doc)
    }

    fn xml_declaration(&mut self) -> Result<(), XmlError> {
        self.pos += 5;
        let body = self.until("?>", "XML declaration")?;
        if !body.trim_start().starts_with("version") {
            return Err(self.error("the XML declaration must start with a version"));
        }
        Ok(())
    }

    /// Comments, processing instructions and white space around the root element, plus
    /// the doctype before it.
    fn misc(&mut self, parent: NodeId, before_root: bool) -> Result<(), XmlError> {
        let mut seen_doctype = false;
        loop {
            self.skip_space();
            if self.starts_with("<!--") {
                self.comment(parent)?;
            } else if self.starts_with("<?") {
                self.processing_instruction(parent)?;
            } else if before_root && !seen_doctype && self.starts_with("<!DOCTYPE") {
                self.doctype(parent)?;
                seen_doctype = true;
            } else {
                return Ok(());
            }
        }
    }

    fn comment(&mut self, parent: NodeId) -> Result<(), XmlError> {
        self.pos += 4;
        let data = self.until("--", "comment")?;
        if !self.eat(">") {
            return Err(self.error("'--' is not allowed in comments"));
        }
        self.flush_text(parent);
        let node = self.doc.create_comment(&data);
        self.doc.append_child(parent, node);
        Ok(())
    }

    fn processing_instruction(&mut self, parent: NodeId) -> Result<(), XmlError> {
        self.pos += 2;
        let target = self.name()?;
        if target.eq_ignore_ascii_case("xml") {
            return Err(
                self.error("the XML declaration is only allowed at the start of the document")
            );
        }
        if target.contains(':') {
            return Err(self.error("processing instruction targets cannot contain ':'"));
        }
        let data = if self.skip_space() {
            self.until("?>", "processing instruction")?
        } else {
            self.expect("?>", "'?>'")?;
            String::new()
        };
        self.flush_text(parent);
        let node = self.doc.create_processing_instruction(&target, &data);
        self.doc.append_child(parent, node);
        Ok(())
    }

    fn quoted(&mut self) -> Result<String, XmlError> {
        let quote = match self.peek() {
            Some(q @ ('"' | '\'')) => q,
            _ => return Err(self.error("expected a quoted string")),
        };
        self.pos += 1;
        let end = quote.to_string();
        self.until(&end, "quoted string")
    }

    fn doctype(&mut self, parent: NodeId) -> Result<(), XmlError> {
        self.pos += "<!DOCTYPE".len();
        if !self.skip_space() {
            return Err(self.error("expected white space after DOCTYPE"));
        }
        let name = self.name()?;
        self.skip_space();
        let (mut public_id, mut system_id) = (String::new(), String::new());
        if self.eat("PUBLIC") {
            self.skip_space();
            public_id = self.quoted()?;
            self.skip_space();
            system_id = self.quoted()?;
        } else if self.eat("SYSTEM") {
            self.skip_space();
            system_id = self.quoted()?;
        }
        self.skip_space();
        if self.eat("[") {
            self.skip_internal_subset()?;
            self.skip_space();
        }
        self.expect(">", "'>' to close the doctype")?;
        let node = self
            .doc
            .create_doctype_with_ids(&name, &public_id, &system_id);
        self.doc.append_child(parent, node);
        Ok(())
    }

    /// Skips declarations up to the `]` closing the internal subset.
    fn skip_internal_subset(&mut self) -> Result<(), XmlError> {
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated internal subset")),
                Some(']') => {
                    self.pos += 1;
                    return Ok(());
                }
                Some('"' | '\'') => {
                    self.quoted()?;
                }
                Some('<') if self.starts_with("<!--") => {
                    self.pos += 4;
                    self.until("-->", "comment")?;
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    fn flush_text(&mut self, parent: NodeId) {
        if self.text.is_empty() {
            return;
        }
        let data = std::mem::take(&mut self.text);
        let node = self.doc.create_text(&data);
        self.doc.append_child(parent, node);
    }

    /// The root element and its content, with an explicit stack so depth is unbounded.
    fn elements(&mut self, root: NodeId) -> Result<(), XmlError> {
        let mut open: Vec<(NodeId, String)> = Vec::new();
        loop {
            let parent = open.last().map_or(root, |(id, _)| *id);
            if self.at_end() {
                let name = open.last().map_or("", |(_, n)| n.as_str());
                return Err(self.error(format!("unclosed element '{name}'")));
            }
            if self.starts_with("</") {
                self.pos += 2;
                let name = self.name()?;
                self.skip_space();
                self.expect(">", "'>' to close the end tag")?;
                let Some((_, open_name)) = open.last() else {
                    return Err(self.error("unexpected end tag"));
                };
                if *open_name != name {
                    return Err(self.error(format!(
                        "end tag '{name}' does not match start tag '{open_name}'"
                    )));
                }
                self.flush_text(parent);
                open.pop();
                self.pop_scope();
                if open.is_empty() {
                    return Ok(());
                }
            } else if self.starts_with("<!--") {
                self.comment(parent)?;
            } else if self.starts_with("<![CDATA[") {
                self.flush_text(parent);
                self.pos += "<![CDATA[".len();
                let data = self.until("]]>", "CDATA section")?;
                let node = self.doc.create_cdata(&data);
                self.doc.append_child(parent, node);
            } else if self.starts_with("<?") {
                self.processing_instruction(parent)?;
            } else if self.starts_with("<!") {
                return Err(self.error("unexpected markup declaration"));
            } else if self.starts_with("<") {
                let tag = self.start_tag()?;
                self.flush_text(parent);
                let element = self.element(&tag)?;
                self.doc.append_child(parent, element);
                if tag.empty {
                    self.pop_scope();
                    if open.is_empty() {
                        return Ok(());
                    }
                } else {
                    open.push((element, tag.name));
                }
            } else {
                self.char_data()?;
            }
        }
    }

    fn char_data(&mut self) -> Result<(), XmlError> {
        while let Some(c) = self.peek() {
            match c {
                '<' => break,
                '&' => {
                    let r = self.reference()?;
                    self.text.push_str(&r);
                }
                ']' if self.starts_with("]]>") => {
                    return Err(self.error("']]>' is not allowed in content"));
                }
                c if !is_char(c) => return Err(self.error("invalid character")),
                c => {
                    self.text.push(c);
                    self.pos += 1;
                }
            }
        }
        Ok(())
    }

    /// An entity or character reference, starting at `&`.
    fn reference(&mut self) -> Result<String, XmlError> {
        self.pos += 1;
        if self.eat("#") {
            let hex = self.eat("x");
            let start = self.pos;
            while self.peek().is_some_and(|c| {
                if hex {
                    c.is_ascii_hexdigit()
                } else {
                    c.is_ascii_digit()
                }
            }) {
                self.pos += 1;
            }
            let digits: String = self.chars[start..self.pos].iter().collect();
            self.expect(";", "';' to end the character reference")?;
            let value = u32::from_str_radix(&digits, if hex { 16 } else { 10 }).ok();
            return match value.and_then(char::from_u32).filter(|&c| is_char(c)) {
                Some(c) => Ok(c.to_string()),
                None => Err(self.error("invalid character reference")),
            };
        }
        let name = self.name()?;
        self.expect(";", "';' to end the entity reference")?;
        Ok(match name.as_str() {
            "lt" => "<",
            "gt" => ">",
            "amp" => "&",
            "apos" => "'",
            "quot" => "\"",
            _ => return Err(self.error(format!("entity '{name}' is not defined"))),
        }
        .to_string())
    }

    fn start_tag(&mut self) -> Result<StartTag, XmlError> {
        self.pos += 1;
        let name = self.name()?;
        let mut attrs: Vec<(String, String)> = Vec::new();
        loop {
            let spaced = self.skip_space();
            if self.eat("/>") {
                return Ok(StartTag {
                    name,
                    attrs,
                    empty: true,
                });
            }
            if self.eat(">") {
                return Ok(StartTag {
                    name,
                    attrs,
                    empty: false,
                });
            }
            if self.at_end() {
                return Err(self.error(format!("unclosed start tag '{name}'")));
            }
            if !spaced {
                return Err(self.error("expected white space between attributes"));
            }
            let attr = self.name()?;
            self.skip_space();
            self.expect("=", "'=' after the attribute name")?;
            self.skip_space();
            let value = self.attribute_value()?;
            if attrs.iter().any(|(n, _)| *n == attr) {
                return Err(self.error(format!("attribute '{attr}' is repeated")));
            }
            attrs.push((attr, value));
        }
    }

    /// A quoted attribute value, with references expanded and white space normalized.
    fn attribute_value(&mut self) -> Result<String, XmlError> {
        let quote = match self.peek() {
            Some(q @ ('"' | '\'')) => q,
            _ => return Err(self.error("attribute values must be quoted")),
        };
        self.pos += 1;
        let mut value = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated attribute value")),
                Some(c) if c == quote => {
                    self.pos += 1;
                    return Ok(value);
                }
                Some('<') => return Err(self.error("'<' is not allowed in attribute values")),
                Some('&') => {
                    let r = self.reference()?;
                    value.push_str(&r);
                }
                Some(c) if !is_char(c) => return Err(self.error("invalid character")),
                Some(c) => {
                    value.push(if is_space(c) { ' ' } else { c });
                    self.pos += 1;
                }
            }
        }
    }

    fn split_qname<'n>(&self, name: &'n str) -> Result<(Option<&'n str>, &'n str), XmlError> {
        match name.split_once(':') {
            None => Ok((None, name)),
            Some((prefix, local))
                if !prefix.is_empty()
                    && !local.is_empty()
                    && !local.contains(':')
                    && local.starts_with(is_name_start) =>
            {
                Ok((Some(prefix), local))
            }
            Some(_) => Err(self.error(format!("'{name}' is not a valid qualified name"))),
        }
    }

    fn pop_scope(&mut self) {
        if let Some(len) = self.scopes.pop() {
            self.declarations.truncate(len);
        }
    }

    fn lookup(&self, prefix: Option<&str>) -> Option<&str> {
        self.declarations
            .iter()
            .rev()
            .find(|(p, _)| p.as_deref() == prefix)
            .map(|(_, uri)| uri.as_str())
            .filter(|uri| !uri.is_empty())
    }

    fn resolve(&self, prefix: &str) -> Result<String, XmlError> {
        if prefix == "xml" {
            return Ok(XML_NAMESPACE.to_string());
        }
        if prefix == "xmlns" {
            return Err(self.error("the 'xmlns' prefix cannot be used on elements"));
        }
        self.lookup(Some(prefix))
            .map(str::to_string)
            .ok_or_else(|| self.error(format!("namespace prefix '{prefix}' is not defined")))
    }

    /// Creates the element for `tag`, pushing its namespace declarations as a new scope.
    fn element(&mut self, tag: &StartTag) -> Result<NodeId, XmlError> {
        self.scopes.push(self.declarations.len());
        for (name, value) in &tag.attrs {
            let declared = if name == "xmlns" {
                None
            } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                if prefix == "xmlns" || (prefix == "xml") != (value == XML_NAMESPACE) {
                    return Err(self.error(format!("the '{prefix}' prefix cannot be declared")));
                }
                if value.is_empty() {
                    return Err(self.error(format!("prefix '{prefix}' cannot be undeclared")));
                }
                Some(prefix.to_string())
            } else {
                continue;
            };
            if value == XMLNS_NAMESPACE || (declared.is_none() && value == XML_NAMESPACE) {
                return Err(self.error(format!("'{value}' cannot be declared as a namespace")));
            }
            self.declarations.push((declared, value.clone()));
        }

        let (prefix, local) = self.split_qname(&tag.name)?;
        let namespace = match prefix {
            Some(p) => Some(self.resolve(p)?),
            None => self.lookup(None).map(str::to_string),
        };
        let element = self.doc.create_element_qualified(&QualifiedName {
            namespace,
            prefix: prefix.map(str::to_string),
            local_name: local.to_string(),
        });

        let mut seen: Vec<(Option<String>, String)> = Vec::new();
        for (name, value) in &tag.attrs {
            let (namespace, prefix, local) = if name == "xmlns" {
                (Some(XMLNS_NAMESPACE.to_string()), None, "xmlns")
            } else {
                match self.split_qname(name)? {
                    (Some("xmlns"), local) => {
                        (Some(XMLNS_NAMESPACE.to_string()), Some("xmlns"), local)
                    }
                    (Some(p), local) => (Some(self.resolve(p)?), Some(p), local),
                    (None, local) => (None, None, local),
                }
            };
            let key = (namespace.clone(), local.to_string());
            if seen.contains(&key) {
                return Err(self.error(format!("attribute '{name}' is repeated")));
            }
            seen.push(key);
            self.doc
                .set_attr_ns(element, namespace.as_deref(), prefix, local, value);
        }
        Ok(element)
    }
}
