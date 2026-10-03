//! Selectors Level 4 engine for querySelector / matches / closest.
//!
//! Parsing follows CSS Syntax for identifiers, escapes, strings and comments, and
//! auto-closes blocks at the end of input. Matching follows the HTML case-sensitivity
//! rules for an HTML document (type and attribute names of HTML elements are ASCII
//! case-insensitive; classes and IDs are case-insensitive in quirks mode).
//!
//! Selectors that depend on state Axiom does not track (user action, focus, :target,
//! :visited, constraint validation, media playback) parse but never match. Namespace
//! prefixes other than `*|` and `|` are invalid because selector APIs have no namespace
//! map. A complex selector with a pseudo-element parses but matches no element.

use crate::{Document, Namespace, NodeId, NodeKind, QuirksMode, XML_NAMESPACE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorList(pub Vec<ComplexSelector>);

/// Compounds left to right; `combinators[i]` sits between `compounds[i]` and
/// `compounds[i + 1]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComplexSelector {
    pub compounds: Vec<Compound>,
    pub combinators: Vec<Combinator>,
    /// The pseudo-element the selector ends in; such selectors match no element.
    pub pseudo_element: Option<PseudoElement>,
}

/// Pseudo-elements with boxes of their own; the rest parse as `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PseudoElement {
    Before,
    After,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Compound {
    pub parts: Vec<Simple>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    Descendant,
    Child,
    AdjacentSibling,
    GeneralSibling,
}

/// The namespace component of a type or attribute selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsMatch {
    Any,
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrOp {
    Equals,
    Includes,
    DashMatch,
    Prefix,
    Suffix,
    Substring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrCase {
    Default,
    Insensitive,
    Sensitive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Simple {
    Universal(NsMatch),
    Type {
        ns: NsMatch,
        name: String,
        lower: String,
    },
    Id(String),
    Class(String),
    Attr {
        ns: NsMatch,
        name: String,
        op: Option<(AttrOp, String)>,
        case: AttrCase,
    },
    Pseudo(Pseudo),
    /// The subject of the enclosing `:has()`; the implicit start of a relative selector.
    HasAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pseudo {
    Root,
    Empty,
    Scope,
    Nth {
        a: i64,
        b: i64,
        from_end: bool,
        of_type: bool,
        of: Option<SelectorList>,
    },
    OnlyChild,
    OnlyOfType,
    Not(SelectorList),
    Is(SelectorList),
    /// `:where()`: matches like `:is()` but adds no specificity.
    Where(SelectorList),
    Has(SelectorList),
    AnyLink,
    Checked,
    Default,
    Disabled,
    Enabled,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    PlaceholderShown,
    Indeterminate,
    Defined,
    Lang(Vec<String>),
    Dir(Option<bool>),
    /// `:host` / `:host(<compound>)`: the shadow host, from its shadow tree's style rules.
    Host(Option<SelectorList>),
    /// `:host-context(<compound>)`: the shadow host when it or a shadow-including
    /// ancestor matches.
    HostContext(SelectorList),
    /// `::slotted(<compound>)`: an element assigned to a slot; the rest of the compound
    /// applies to the slot.
    Slotted(SelectorList),
    /// Valid, but depends on state Axiom does not track.
    Never,
}

/// Parses a selector list; `None` when it is invalid (a `SyntaxError` for the DOM APIs).
pub fn parse_selector_list(input: &str) -> Option<SelectorList> {
    let mut p = Parser::new(input);
    let list = p.list(Mode::TOP)?;
    (p.i == p.s.len() && !list.is_empty()).then_some(SelectorList(list))
}

/// Whether `node` matches any selector in `list`; `scope` is the `:scope` node (the
/// element for `matches()` / `closest()`, the root for `querySelector*`).
pub fn matches(doc: &Document, node: NodeId, list: &SelectorList, scope: Option<NodeId>) -> bool {
    let cx = Cx {
        doc,
        scope,
        anchor: None,
        host: None,
        quirks: doc.quirks_mode == QuirksMode::Quirks,
    };
    doc.is_element(node) && cx.list(list, node)
}

/// Whether `node` matches `list` as a style rule of `host`'s shadow tree: `:host`,
/// `:host-context()` and `::slotted()` refer to `host`, and combinators stop at it.
pub fn matches_in_shadow(doc: &Document, node: NodeId, list: &SelectorList, host: NodeId) -> bool {
    let cx = Cx {
        doc,
        scope: None,
        anchor: None,
        host: Some(host),
        quirks: doc.quirks_mode == QuirksMode::Quirks,
    };
    doc.is_element(node) && cx.list(list, node)
}

/// Whether `list` selects pseudo-element `pseudo` of element `node` (as a style rule of
/// `host`'s shadow tree when given).
pub fn matches_pseudo(
    doc: &Document,
    node: NodeId,
    list: &SelectorList,
    pseudo: PseudoElement,
    host: Option<NodeId>,
) -> bool {
    let cx = Cx {
        doc,
        scope: None,
        anchor: None,
        host,
        quirks: doc.quirks_mode == QuirksMode::Quirks,
    };
    doc.is_element(node)
        && list.0.iter().any(|s| {
            s.pseudo_element == Some(pseudo) && cx.match_from(s, s.compounds.len() - 1, node)
        })
}

/// The first descendant of `root` matching `selector`, in tree order.
pub fn query_selector(doc: &Document, root: NodeId, selector: &str) -> Option<NodeId> {
    let list = parse_selector_list(selector)?;
    let mut found = None;
    walk_descendants(doc, root, &mut |id| {
        if found.is_none() && matches(doc, id, &list, Some(root)) {
            found = Some(id);
        }
        found.is_none()
    });
    found
}

/// All descendants of `root` matching `selector`, in tree order; empty when invalid.
pub fn query_selector_all(doc: &Document, root: NodeId, selector: &str) -> Vec<NodeId> {
    let Some(list) = parse_selector_list(selector) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk_descendants(doc, root, &mut |id| {
        if matches(doc, id, &list, Some(root)) {
            out.push(id);
        }
        true
    });
    out
}

/// The nearest inclusive ancestor element of `node` matching `list`.
pub fn closest(doc: &Document, node: NodeId, list: &SelectorList) -> Option<NodeId> {
    let mut cur = Some(node);
    while let Some(id) = cur {
        if !doc.is_element(id) {
            return None;
        }
        if matches(doc, id, list, Some(node)) {
            return Some(id);
        }
        cur = doc.get(id).parent;
    }
    None
}

/// Pre-order walk of the descendants of `root`; stops when `f` returns false.
fn walk_descendants(doc: &Document, root: NodeId, f: &mut dyn FnMut(NodeId) -> bool) {
    let mut stack: Vec<NodeId> = doc.get(root).children.iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        if !f(id) {
            return;
        }
        stack.extend(doc.get(id).children.iter().rev().copied());
    }
}

// ---------------------------------------------------------------------------------
// Parsing

#[derive(Clone, Copy)]
struct Mode {
    /// Inside a functional pseudo-class: `)` ends the list.
    nested: bool,
    /// Invalid items are dropped (`:is()` / `:where()`).
    forgiving: bool,
    /// Items may start with a combinator (`:has()`).
    relative: bool,
}

impl Mode {
    const TOP: Mode = Mode {
        nested: false,
        forgiving: false,
        relative: false,
    };
    const NESTED: Mode = Mode {
        nested: true,
        forgiving: false,
        relative: false,
    };
}

struct Parser {
    s: Vec<char>,
    i: usize,
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n')
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c as u32 >= 0x80
}

fn is_name(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-'
}

impl Parser {
    fn new(input: &str) -> Self {
        let mut s = Vec::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    s.push('\n');
                }
                '\x0C' => s.push('\n'),
                '\0' => s.push('\u{FFFD}'),
                c => s.push(c),
            }
        }
        Parser { s, i: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn at(&self, k: usize) -> Option<char> {
        self.s.get(k).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn skip_comments(&mut self) {
        while self.peek() == Some('/') && self.at(self.i + 1) == Some('*') {
            self.i += 2;
            while self.i < self.s.len()
                && !(self.s[self.i] == '*' && self.at(self.i + 1) == Some('/'))
            {
                self.i += 1;
            }
            self.i = (self.i + 2).min(self.s.len());
        }
    }

    /// Skips whitespace and comments; true when any whitespace was skipped.
    fn skip_ws(&mut self) -> bool {
        let mut ws = false;
        loop {
            self.skip_comments();
            match self.peek() {
                Some(c) if is_ws(c) => {
                    ws = true;
                    self.i += 1;
                }
                _ => return ws,
            }
        }
    }

    fn valid_escape(&self, k: usize) -> bool {
        self.at(k) == Some('\\') && self.at(k + 1) != Some('\n')
    }

    fn starts_ident(&self, k: usize) -> bool {
        match self.at(k) {
            Some('-') => match self.at(k + 1) {
                Some(c) if is_name_start(c) || c == '-' => true,
                _ => self.valid_escape(k + 1),
            },
            Some(c) if is_name_start(c) => true,
            Some('\\') => self.valid_escape(k),
            _ => false,
        }
    }

    /// Consumes an escaped code point; the backslash is already consumed.
    fn escape(&mut self) -> char {
        let Some(c) = self.peek() else {
            return '\u{FFFD}';
        };
        if !c.is_ascii_hexdigit() {
            self.i += 1;
            return c;
        }
        let mut value: u32 = 0;
        let mut n = 0;
        while n < 6 {
            match self.peek() {
                Some(h) if h.is_ascii_hexdigit() => {
                    value = value * 16 + h.to_digit(16).unwrap_or(0);
                    self.i += 1;
                    n += 1;
                }
                _ => break,
            }
        }
        if matches!(self.peek(), Some(w) if is_ws(w)) {
            self.i += 1;
        }
        match char::from_u32(value) {
            Some(c) if value != 0 => c,
            _ => '\u{FFFD}',
        }
    }

    /// Consumes a name (identifier characters and escapes).
    fn name(&mut self) -> String {
        let mut out = String::new();
        loop {
            match self.peek() {
                Some(c) if is_name(c) => {
                    out.push(c);
                    self.i += 1;
                }
                Some('\\') if self.valid_escape(self.i) => {
                    self.i += 1;
                    out.push(self.escape());
                }
                _ => return out,
            }
        }
    }

    fn ident(&mut self) -> Option<String> {
        self.starts_ident(self.i).then(|| self.name())
    }

    fn string(&mut self) -> Option<String> {
        let quote = self.peek()?;
        self.i += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Some(out),
                Some(c) if c == quote => {
                    self.i += 1;
                    return Some(out);
                }
                Some('\n') => return None,
                Some('\\') => {
                    self.i += 1;
                    match self.peek() {
                        None => {}
                        Some('\n') => self.i += 1,
                        Some(_) => out.push(self.escape()),
                    }
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// Whether the input at the cursor ends the current list item.
    fn at_item_end(&self, mode: Mode) -> bool {
        match self.peek() {
            None | Some(',') => true,
            Some(')') => mode.nested,
            _ => false,
        }
    }

    /// Skips balanced input up to (not including) a top-level `,` or `)`, or the end.
    fn skip_item(&mut self, stop_at_comma: bool) {
        let mut depth = 0usize;
        while let Some(c) = self.peek() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => {
                    if depth == 0 {
                        if c == ')' {
                            return;
                        }
                    } else {
                        depth -= 1;
                    }
                }
                ',' if depth == 0 && stop_at_comma => return,
                '"' | '\'' => {
                    if self.string().is_none() {
                        self.i += 1;
                    }
                    continue;
                }
                '\\' => {
                    self.i += 1;
                    if self.peek().is_some() {
                        self.escape();
                    }
                    continue;
                }
                '/' if self.at(self.i + 1) == Some('*') => {
                    self.skip_comments();
                    continue;
                }
                _ => {}
            }
            self.i += 1;
        }
    }

    fn list(&mut self, mode: Mode) -> Option<Vec<ComplexSelector>> {
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            let start = self.i;
            let item = if mode.forgiving && self.at_item_end(mode) {
                None
            } else {
                self.complex(mode)
            };
            match item {
                Some(c) => out.push(c),
                None if mode.forgiving => {
                    self.i = start;
                    self.skip_item(true);
                }
                None => return None,
            }
            match self.peek() {
                Some(',') => self.i += 1,
                Some(')') if mode.nested => return Some(out),
                None => return Some(out),
                _ => return None,
            }
        }
    }

    fn combinator(&mut self) -> Option<Combinator> {
        let c = match self.peek()? {
            '>' => Combinator::Child,
            '+' => Combinator::AdjacentSibling,
            '~' => Combinator::GeneralSibling,
            _ => return None,
        };
        self.i += 1;
        Some(c)
    }

    fn complex(&mut self, mode: Mode) -> Option<ComplexSelector> {
        let mut sel = ComplexSelector {
            compounds: Vec::new(),
            combinators: Vec::new(),
            pseudo_element: None,
        };
        if mode.relative {
            let leading = self.combinator().unwrap_or(Combinator::Descendant);
            self.skip_ws();
            sel.compounds.push(Compound {
                parts: vec![Simple::HasAnchor],
            });
            sel.combinators.push(leading);
        }
        loop {
            let (compound, pseudo_element) = self.compound()?;
            if pseudo_element.is_some() && (mode.nested || sel.pseudo_element.is_some()) {
                return None;
            }
            sel.pseudo_element = sel.pseudo_element.or(pseudo_element);
            sel.compounds.push(compound);
            let ws = self.skip_ws();
            if self.at_item_end(mode) {
                return Some(sel);
            }
            let comb = match self.combinator() {
                Some(c) => {
                    self.skip_ws();
                    c
                }
                None if ws => Combinator::Descendant,
                None => return None,
            };
            if sel.pseudo_element.is_some() {
                return None;
            }
            sel.combinators.push(comb);
        }
    }

    /// `ns|` prefix handling shared by type and attribute selectors. `first` is the
    /// name before the cursor: `Some(None)` for `*`, `Some(Some(name))` for an ident.
    fn namespace_prefix(&mut self, first: &Option<Option<String>>) -> Option<Option<NsMatch>> {
        if self.peek() == Some('|') && self.at(self.i + 1) != Some('=') {
            self.i += 1;
            return match first {
                None => Some(Some(NsMatch::Null)),
                Some(None) => Some(Some(NsMatch::Any)),
                Some(Some(_)) => None,
            };
        }
        Some(None)
    }

    fn star_or_ident(&mut self) -> Option<Option<String>> {
        if self.eat('*') {
            Some(None)
        } else {
            self.ident().map(Some)
        }
    }

    fn type_selector(&mut self) -> Option<Option<Simple>> {
        let first = self.star_or_ident();
        let name = match self.namespace_prefix(&first)? {
            Some(ns) => (ns, self.star_or_ident()?),
            None => match first {
                None => return Some(None),
                Some(name) => (NsMatch::Any, name),
            },
        };
        Some(Some(match name {
            (ns, None) => Simple::Universal(ns),
            (ns, Some(name)) => Simple::Type {
                ns,
                lower: name.to_ascii_lowercase(),
                name,
            },
        }))
    }

    fn attribute(&mut self) -> Option<Simple> {
        self.skip_ws();
        let first = self.star_or_ident();
        let (ns, name) = match self.namespace_prefix(&first)? {
            Some(ns) => (ns, self.ident()?),
            None => (NsMatch::Null, first??),
        };
        self.skip_ws();
        if self.peek().is_none() || self.eat(']') {
            return Some(Simple::Attr {
                ns,
                name,
                op: None,
                case: AttrCase::Default,
            });
        }
        let op = match self.peek()? {
            '=' => AttrOp::Equals,
            c => {
                if self.at(self.i + 1) != Some('=') {
                    return None;
                }
                self.i += 1;
                match c {
                    '~' => AttrOp::Includes,
                    '|' => AttrOp::DashMatch,
                    '^' => AttrOp::Prefix,
                    '$' => AttrOp::Suffix,
                    '*' => AttrOp::Substring,
                    _ => return None,
                }
            }
        };
        self.i += 1;
        self.skip_ws();
        let value = match self.peek()? {
            '"' | '\'' => self.string()?,
            _ => self.ident()?,
        };
        self.skip_ws();
        let case = match self.ident() {
            None => AttrCase::Default,
            Some(f) if f.eq_ignore_ascii_case("i") => AttrCase::Insensitive,
            Some(f) if f.eq_ignore_ascii_case("s") => AttrCase::Sensitive,
            Some(_) => return None,
        };
        self.skip_ws();
        if !(self.peek().is_none() || self.eat(']')) {
            return None;
        }
        Some(Simple::Attr {
            ns,
            name,
            op: Some((op, value)),
            case,
        })
    }

    /// A compound selector and the pseudo-element it carries.
    fn compound(&mut self) -> Option<(Compound, Option<PseudoElement>)> {
        let mut parts = Vec::new();
        if let Some(t) = self.type_selector()? {
            parts.push(t);
        }
        let mut pseudo_element = None;
        let mut slotted = false;
        loop {
            self.skip_comments();
            match self.peek() {
                Some('#' | '.' | '[' | ':') if slotted => return None,
                Some('#' | '.' | '[') if pseudo_element.is_some() => return None,
                Some('#') => {
                    self.i += 1;
                    parts.push(Simple::Id(self.ident()?));
                }
                Some('.') => {
                    self.i += 1;
                    parts.push(Simple::Class(self.ident()?));
                }
                Some('[') => {
                    self.i += 1;
                    parts.push(self.attribute()?);
                }
                Some(':') => {
                    self.i += 1;
                    if self.eat(':') {
                        let save = self.i;
                        if pseudo_element.is_none()
                            && self
                                .ident()
                                .is_some_and(|n| n.eq_ignore_ascii_case("slotted"))
                            && self.eat('(')
                        {
                            let list = SelectorList(self.list(Mode::NESTED)?);
                            self.close_paren()?;
                            parts.push(Simple::Pseudo(Pseudo::Slotted(list)));
                            slotted = true;
                        } else {
                            self.i = save;
                            let e = self.pseudo_element()?;
                            // `::before::marker` and the like: a sub-pseudo-element.
                            pseudo_element = Some(match pseudo_element {
                                Some(_) => PseudoElement::Other,
                                None => e,
                            });
                        }
                    } else {
                        match self.pseudo_class()? {
                            PseudoParse::Class(p) => parts.push(Simple::Pseudo(p)),
                            PseudoParse::LegacyElement(e) => {
                                pseudo_element = Some(match pseudo_element {
                                    Some(_) => PseudoElement::Other,
                                    None => e,
                                });
                            }
                        }
                    }
                }
                _ => break,
            }
        }
        if parts.is_empty() && pseudo_element.is_none() {
            return None;
        }
        Some((Compound { parts }, pseudo_element))
    }

    fn close_paren(&mut self) -> Option<()> {
        self.skip_ws();
        (self.peek().is_none() || self.eat(')')).then_some(())
    }

    fn pseudo_element(&mut self) -> Option<PseudoElement> {
        let name = self.ident()?.to_ascii_lowercase();
        if self.eat('(') {
            if !matches!(
                name.as_str(),
                "part"
                    | "slotted"
                    | "highlight"
                    | "cue"
                    | "cue-region"
                    | "picker"
                    | "scroll-button"
                    | "view-transition-group"
                    | "view-transition-image-pair"
                    | "view-transition-old"
                    | "view-transition-new"
            ) {
                return None;
            }
            self.skip_item(false);
            return self.close_paren().map(|()| PseudoElement::Other);
        }
        match name.as_str() {
            "before" => return Some(PseudoElement::Before),
            "after" => return Some(PseudoElement::After),
            _ => {}
        }
        let known = matches!(
            name.as_str(),
            "before"
                | "after"
                | "first-line"
                | "first-letter"
                | "marker"
                | "placeholder"
                | "selection"
                | "backdrop"
                | "file-selector-button"
                | "grammar-error"
                | "spelling-error"
                | "target-text"
                | "cue"
                | "cue-region"
                | "details-content"
                | "checkmark"
                | "picker-icon"
                | "search-text"
                | "column"
                | "scroll-marker"
                | "scroll-marker-group"
                | "view-transition"
        ) || name.starts_with("-webkit-");
        known.then_some(PseudoElement::Other)
    }

    fn pseudo_class(&mut self) -> Option<PseudoParse> {
        let name = self.ident()?.to_ascii_lowercase();
        if self.eat('(') {
            return self.functional_pseudo(&name).map(PseudoParse::Class);
        }
        let p = match name.as_str() {
            "before" => return Some(PseudoParse::LegacyElement(PseudoElement::Before)),
            "after" => return Some(PseudoParse::LegacyElement(PseudoElement::After)),
            "first-line" | "first-letter" => {
                return Some(PseudoParse::LegacyElement(PseudoElement::Other))
            }
            "root" => Pseudo::Root,
            "empty" => Pseudo::Empty,
            "scope" => Pseudo::Scope,
            "first-child" => nth(0, 1, false, false),
            "last-child" => nth(0, 1, true, false),
            "first-of-type" => nth(0, 1, false, true),
            "last-of-type" => nth(0, 1, true, true),
            "only-child" => Pseudo::OnlyChild,
            "only-of-type" => Pseudo::OnlyOfType,
            "link" | "any-link" => Pseudo::AnyLink,
            "checked" => Pseudo::Checked,
            "default" => Pseudo::Default,
            "disabled" => Pseudo::Disabled,
            "enabled" => Pseudo::Enabled,
            "required" => Pseudo::Required,
            "optional" => Pseudo::Optional,
            "read-only" => Pseudo::ReadOnly,
            "read-write" => Pseudo::ReadWrite,
            "placeholder-shown" => Pseudo::PlaceholderShown,
            "indeterminate" => Pseudo::Indeterminate,
            "defined" => Pseudo::Defined,
            "hover" | "active" | "focus" | "focus-visible" | "focus-within" | "visited"
            | "target" | "target-within" | "current" | "past" | "future" | "playing" | "paused"
            | "seeking" | "buffering" | "stalled" | "muted" | "volume-locked" | "fullscreen"
            | "modal" | "picture-in-picture" | "autofill" | "-webkit-autofill" | "user-invalid"
            | "user-valid" | "valid" | "invalid" | "in-range" | "out-of-range" | "blank"
            | "popover-open" | "open" | "closed" | "local-link" => Pseudo::Never,
            "host" => Pseudo::Host(None),
            _ => return None,
        };
        Some(PseudoParse::Class(p))
    }

    fn functional_pseudo(&mut self, name: &str) -> Option<Pseudo> {
        let p = match name {
            "not" => Pseudo::Not(SelectorList(self.list(Mode::NESTED)?)),
            "is" => Pseudo::Is(SelectorList(self.list(Mode {
                forgiving: true,
                ..Mode::NESTED
            })?)),
            "where" => Pseudo::Where(SelectorList(self.list(Mode {
                forgiving: true,
                ..Mode::NESTED
            })?)),
            "has" => Pseudo::Has(SelectorList(self.list(Mode {
                relative: true,
                ..Mode::NESTED
            })?)),
            "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                let of_type = name.ends_with("of-type");
                let (a, b) = self.anb()?;
                let ws = self.skip_ws();
                let mut of = None;
                if !of_type && ws && self.ident_is("of") {
                    self.i += 2;
                    if !self.skip_ws() {
                        return None;
                    }
                    of = Some(SelectorList(self.list(Mode::NESTED)?));
                }
                Pseudo::Nth {
                    a,
                    b,
                    from_end: name.starts_with("nth-last"),
                    of_type,
                    of,
                }
            }
            "lang" => {
                let mut ranges = Vec::new();
                loop {
                    self.skip_ws();
                    let r = match self.peek()? {
                        '"' | '\'' => self.string()?,
                        _ => self.ident()?,
                    };
                    ranges.push(r);
                    self.skip_ws();
                    if !self.eat(',') {
                        break;
                    }
                }
                Pseudo::Lang(ranges)
            }
            "dir" => {
                self.skip_ws();
                let d = self.ident()?.to_ascii_lowercase();
                Pseudo::Dir(match d.as_str() {
                    "ltr" => Some(false),
                    "rtl" => Some(true),
                    _ => None,
                })
            }
            "host" => Pseudo::Host(Some(SelectorList(self.list(Mode::NESTED)?))),
            "host-context" => Pseudo::HostContext(SelectorList(self.list(Mode::NESTED)?)),
            "state" | "nth-col" | "nth-last-col" | "current" => {
                self.skip_item(false);
                Pseudo::Never
            }
            _ => return None,
        };
        self.close_paren()?;
        Some(p)
    }

    fn ident_is(&self, word: &str) -> bool {
        let w: Vec<char> = word.chars().collect();
        w.iter().enumerate().all(|(k, c)| {
            self.at(self.i + k)
                .is_some_and(|x| x.eq_ignore_ascii_case(c))
        }) && !self.at(self.i + w.len()).is_some_and(is_name)
    }

    fn digits(&mut self) -> Option<i64> {
        let start = self.i;
        let mut v: i64 = 0;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
            v = v.saturating_mul(10).saturating_add(d as i64);
            self.i += 1;
        }
        (self.i > start).then_some(v)
    }

    /// The `An+B` microsyntax.
    fn anb(&mut self) -> Option<(i64, i64)> {
        self.skip_ws();
        for (word, ab) in [("odd", (2, 1)), ("even", (2, 0))] {
            if self.ident_is(word) {
                self.i += word.len();
                return Some(ab);
            }
        }
        let sign = if self.eat('+') {
            1
        } else if self.eat('-') {
            -1
        } else {
            1
        };
        let digits = self.digits();
        if matches!(self.peek(), Some('n' | 'N')) {
            self.i += 1;
            let a = sign * digits.unwrap_or(1);
            if self.peek().is_some_and(|c| is_name(c) && c != '-') {
                return None;
            }
            let save = self.i;
            self.skip_ws();
            let b_sign = if self.eat('+') {
                1
            } else if self.eat('-') {
                -1
            } else {
                self.i = save;
                return Some((a, 0));
            };
            self.skip_ws();
            let b = self.digits()?;
            return Some((a, b_sign * b));
        }
        Some((0, sign * digits?))
    }
}

enum PseudoParse {
    Class(Pseudo),
    LegacyElement(PseudoElement),
}

fn nth(a: i64, b: i64, from_end: bool, of_type: bool) -> Pseudo {
    Pseudo::Nth {
        a,
        b,
        from_end,
        of_type,
        of: None,
    }
}

// ---------------------------------------------------------------------------------
// Matching

/// HTML attributes whose values selectors match ASCII case-insensitively on HTML
/// elements (HTML § "Case-sensitivity of selectors").
const CASE_INSENSITIVE_ATTRS: &[&str] = &[
    "accept",
    "accept-charset",
    "align",
    "alink",
    "axis",
    "bgcolor",
    "charset",
    "checked",
    "clear",
    "codetype",
    "color",
    "compact",
    "declare",
    "defer",
    "dir",
    "direction",
    "disabled",
    "enctype",
    "face",
    "frame",
    "hreflang",
    "http-equiv",
    "lang",
    "language",
    "link",
    "media",
    "method",
    "multiple",
    "nohref",
    "noresize",
    "noshade",
    "nowrap",
    "readonly",
    "rel",
    "rev",
    "rules",
    "scope",
    "scrolling",
    "selected",
    "shape",
    "target",
    "text",
    "type",
    "valign",
    "valuetype",
    "vlink",
];

#[derive(Clone, Copy)]
struct Cx<'a> {
    doc: &'a Document,
    scope: Option<NodeId>,
    anchor: Option<NodeId>,
    /// The shadow host whose shadow tree's rules are matching (`matches_in_shadow`).
    host: Option<NodeId>,
    quirks: bool,
}

fn slotted_argument(compound: &Compound) -> Option<&SelectorList> {
    compound.parts.iter().find_map(|p| match p {
        Simple::Pseudo(Pseudo::Slotted(list)) => Some(list),
        _ => None,
    })
}

impl<'a> Cx<'a> {
    fn list(&self, list: &SelectorList, node: NodeId) -> bool {
        list.0.iter().any(|s| self.complex(s, node))
    }

    fn complex(&self, sel: &ComplexSelector, node: NodeId) -> bool {
        sel.pseudo_element.is_none() && self.match_from(sel, sel.compounds.len() - 1, node)
    }

    fn match_from(&self, sel: &ComplexSelector, idx: usize, node: NodeId) -> bool {
        let compound = &sel.compounds[idx];
        let node = match slotted_argument(compound) {
            Some(argument) => match self.slotted(compound, argument, node) {
                Some(slot) => slot,
                None => return false,
            },
            None if self.compound(compound, node) => node,
            None => return false,
        };
        if idx == 0 {
            return true;
        }
        let doc = self.doc;
        match sel.combinators[idx - 1] {
            Combinator::Child => self
                .parent(node)
                .is_some_and(|p| self.match_from(sel, idx - 1, p)),
            Combinator::Descendant => {
                let mut cur = self.parent(node);
                while let Some(p) = cur {
                    if self.match_from(sel, idx - 1, p) {
                        return true;
                    }
                    cur = self.parent(p);
                }
                false
            }
            Combinator::AdjacentSibling => {
                prev_element(doc, node).is_some_and(|p| self.match_from(sel, idx - 1, p))
            }
            Combinator::GeneralSibling => {
                let mut cur = prev_element(doc, node);
                while let Some(p) = cur {
                    if self.match_from(sel, idx - 1, p) {
                        return true;
                    }
                    cur = prev_element(doc, p);
                }
                false
            }
        }
    }

    /// The parent for combinators. In a shadow tree's rules the shadow root steps to the
    /// (featureless) host, and nothing is above the host.
    fn parent(&self, node: NodeId) -> Option<NodeId> {
        let Some(host) = self.host else {
            return self.doc.get(node).parent;
        };
        if node == host {
            return None;
        }
        let parent = self.doc.get(node).parent?;
        if self.doc.shadow_host(parent) == Some(host) {
            return Some(host);
        }
        Some(parent)
    }

    /// `node` against a compound with `::slotted(argument)`: `node` must be assigned to a
    /// slot of this shadow tree and match `argument`; the other parts match that slot,
    /// which is returned for the combinators.
    fn slotted(
        &self,
        compound: &Compound,
        argument: &SelectorList,
        node: NodeId,
    ) -> Option<NodeId> {
        let host = self.host?;
        if self.doc.get(node).parent != Some(host) {
            return None;
        }
        let slot = self.doc.assigned_slot(node)?;
        let outside = Cx {
            host: None,
            ..*self
        };
        if !outside.list(argument, node) {
            return None;
        }
        compound
            .parts
            .iter()
            .all(|p| matches!(p, Simple::Pseudo(Pseudo::Slotted(_))) || self.simple(p, slot))
            .then_some(slot)
    }

    fn compound(&self, compound: &Compound, node: NodeId) -> bool {
        if self.host == Some(node) {
            // The host is featureless to its own shadow tree's rules: only compounds
            // with :host / :host-context() (plus other pseudo-classes) reach it.
            return compound
                .parts
                .iter()
                .any(|p| matches!(p, Simple::Pseudo(Pseudo::Host(_) | Pseudo::HostContext(_))))
                && compound
                    .parts
                    .iter()
                    .all(|p| matches!(p, Simple::Pseudo(_)) && self.simple(p, node));
        }
        if !self.doc.is_element(node) {
            // Only :scope (a document or fragment root) matches a non-element.
            return !compound.parts.is_empty()
                && compound.parts.iter().all(|p| {
                    matches!(p, Simple::Pseudo(Pseudo::Scope)) && self.scope == Some(node)
                });
        }
        compound.parts.iter().all(|p| self.simple(p, node))
    }

    fn is_html(&self, node: NodeId) -> bool {
        self.doc.namespace(node) == Some(Namespace::Html)
    }

    fn ns_ok(&self, ns: NsMatch, node: NodeId) -> bool {
        ns == NsMatch::Any || self.doc.namespace(node) == Some(Namespace::Null)
    }

    fn html_attr(&self, node: NodeId, name: &str) -> Option<&'a str> {
        self.doc.attr_ns(node, None, name)
    }

    fn simple(&self, part: &Simple, node: NodeId) -> bool {
        let doc = self.doc;
        match part {
            Simple::Universal(ns) => self.ns_ok(*ns, node),
            Simple::Type { ns, name, lower } => {
                let local = doc.tag_name(node).unwrap_or("");
                self.ns_ok(*ns, node)
                    && if self.is_html(node) {
                        local == lower
                    } else {
                        local == name
                    }
            }
            Simple::Id(id) => self.html_attr(node, "id").is_some_and(|v| {
                if self.quirks {
                    v.eq_ignore_ascii_case(id)
                } else {
                    v == id
                }
            }),
            Simple::Class(class) => self.html_attr(node, "class").is_some_and(|v| {
                v.split(is_ascii_ws).any(|c| {
                    if self.quirks {
                        c.eq_ignore_ascii_case(class)
                    } else {
                        c == class
                    }
                })
            }),
            Simple::Attr { ns, name, op, case } => self.attr(node, *ns, name, op, *case),
            Simple::Pseudo(p) => self.pseudo(p, node),
            Simple::HasAnchor => self.anchor == Some(node),
        }
    }

    fn attr(
        &self,
        node: NodeId,
        ns: NsMatch,
        name: &str,
        op: &Option<(AttrOp, String)>,
        case: AttrCase,
    ) -> bool {
        let html = self.is_html(node);
        self.doc.attributes(node).iter().any(|a| {
            if ns == NsMatch::Null && a.namespace.is_some() {
                return false;
            }
            let name_ok = if html {
                a.local_name.eq_ignore_ascii_case(name)
            } else {
                a.local_name == name
            };
            if !name_ok {
                return false;
            }
            let Some((op, want)) = op else {
                return true;
            };
            let insensitive = match case {
                AttrCase::Insensitive => true,
                AttrCase::Sensitive => false,
                AttrCase::Default => {
                    html && a.namespace.is_none()
                        && CASE_INSENSITIVE_ATTRS
                            .contains(&a.local_name.to_ascii_lowercase().as_str())
                }
            };
            let (v, w) = if insensitive {
                (a.value.to_ascii_lowercase(), want.to_ascii_lowercase())
            } else {
                (a.value.clone(), want.clone())
            };
            match op {
                AttrOp::Equals => v == w,
                AttrOp::Includes => {
                    !w.is_empty()
                        && !w.contains(is_ascii_ws)
                        && v.split(is_ascii_ws).any(|t| t == w)
                }
                AttrOp::DashMatch => v == w || (v.starts_with(&w) && v[w.len()..].starts_with('-')),
                AttrOp::Prefix => !w.is_empty() && v.starts_with(&w),
                AttrOp::Suffix => !w.is_empty() && v.ends_with(&w),
                AttrOp::Substring => !w.is_empty() && v.contains(&w),
            }
        })
    }

    fn pseudo(&self, p: &Pseudo, node: NodeId) -> bool {
        let doc = self.doc;
        match p {
            Pseudo::Root => doc
                .get(node)
                .parent
                .is_some_and(|p| matches!(doc.get(p).kind, NodeKind::Document)),
            Pseudo::Empty => doc
                .get(node)
                .children
                .iter()
                .all(|&c| match &doc.get(c).kind {
                    NodeKind::Element { .. } => false,
                    NodeKind::Text { data } | NodeKind::CData { data } => data.is_empty(),
                    _ => true,
                }),
            Pseudo::Scope => match self.scope {
                Some(s) => s == node,
                None => self.pseudo(&Pseudo::Root, node),
            },
            Pseudo::Nth {
                a,
                b,
                from_end,
                of_type,
                of,
            } => {
                let pos = self.nth_position(node, *from_end, *of_type, of.as_ref());
                match pos {
                    Some(pos) => nth_matches(*a, *b, pos),
                    None => false,
                }
            }
            Pseudo::OnlyChild => {
                self.nth_position(node, false, false, None) == Some(1)
                    && self.nth_position(node, true, false, None) == Some(1)
            }
            Pseudo::OnlyOfType => {
                self.nth_position(node, false, true, None) == Some(1)
                    && self.nth_position(node, true, true, None) == Some(1)
            }
            Pseudo::Not(list) => !self.list(list, node),
            Pseudo::Is(list) | Pseudo::Where(list) => self.list(list, node),
            Pseudo::Has(list) => self.has(list, node),
            Pseudo::AnyLink => {
                self.is_html(node)
                    && matches!(doc.tag_name(node), Some("a" | "area" | "link"))
                    && self.html_attr(node, "href").is_some()
            }
            Pseudo::Checked => self.checked(node),
            Pseudo::Default => self.checked(node) || self.is_default_button(node),
            Pseudo::Disabled => self.disabled(node),
            Pseudo::Enabled => {
                self.is_html(node)
                    && matches!(
                        doc.tag_name(node),
                        Some(
                            "button"
                                | "input"
                                | "select"
                                | "textarea"
                                | "optgroup"
                                | "option"
                                | "fieldset"
                        )
                    )
                    && !self.disabled(node)
            }
            Pseudo::Required | Pseudo::Optional => {
                self.is_html(node)
                    && matches!(doc.tag_name(node), Some("input" | "select" | "textarea"))
                    && (self.html_attr(node, "required").is_some() == matches!(p, Pseudo::Required))
            }
            Pseudo::ReadWrite => self.read_write(node),
            Pseudo::ReadOnly => !self.read_write(node),
            Pseudo::PlaceholderShown => {
                self.is_html(node)
                    && self.html_attr(node, "placeholder").is_some()
                    && match doc.tag_name(node) {
                        Some("input") => self.html_attr(node, "value").unwrap_or("").is_empty(),
                        Some("textarea") => doc.text_content(node).is_empty(),
                        _ => false,
                    }
            }
            Pseudo::Indeterminate => {
                self.is_html(node)
                    && doc.tag_name(node) == Some("progress")
                    && self.html_attr(node, "value").is_none()
            }
            Pseudo::Defined => {
                !(self.is_html(node) && doc.tag_name(node).is_some_and(|n| n.contains('-')))
                    || doc.is_custom_element_defined(node)
            }
            Pseudo::Host(argument) => {
                self.host == Some(node)
                    && argument.as_ref().is_none_or(|a| {
                        Cx {
                            host: None,
                            ..*self
                        }
                        .list(a, node)
                    })
            }
            Pseudo::HostContext(argument) => {
                if self.host != Some(node) {
                    return false;
                }
                let outside = Cx {
                    host: None,
                    ..*self
                };
                let mut cur = Some(node);
                while let Some(id) = cur {
                    if doc.is_element(id) && outside.list(argument, id) {
                        return true;
                    }
                    cur = doc.parent_or_host(id);
                }
                false
            }
            Pseudo::Slotted(_) => false,
            Pseudo::Lang(ranges) => match self.language(node) {
                Some(lang) if !lang.is_empty() => ranges.iter().any(|r| {
                    r == "*"
                        || lang.eq_ignore_ascii_case(r)
                        || (lang.len() > r.len()
                            && lang[..r.len()].eq_ignore_ascii_case(r)
                            && lang.as_bytes()[r.len()] == b'-')
                }),
                _ => false,
            },
            Pseudo::Dir(want) => want.is_some_and(|rtl| self.is_rtl(node) == rtl),
            Pseudo::Never => false,
        }
    }

    /// 1-based position of `node` among its element siblings (optionally only those
    /// of the same type, or matching `of`); `None` when `of` does not match `node`.
    fn nth_position(
        &self,
        node: NodeId,
        from_end: bool,
        of_type: bool,
        of: Option<&SelectorList>,
    ) -> Option<i64> {
        let doc = self.doc;
        if let Some(of) = of {
            if !self.list(of, node) {
                return None;
            }
        }
        let Some(parent) = doc.get(node).parent else {
            return Some(1);
        };
        let same = |c: NodeId| {
            doc.is_element(c)
                && (!of_type
                    || (doc.tag_name(c) == doc.tag_name(node)
                        && doc.namespace(c) == doc.namespace(node)))
                && of.is_none_or(|l| self.list(l, c))
        };
        let children = &doc.get(parent).children;
        let idx = children.iter().position(|&c| c == node)?;
        let count = if from_end {
            children[idx + 1..].iter().filter(|&&c| same(c)).count()
        } else {
            children[..idx].iter().filter(|&&c| same(c)).count()
        };
        Some(count as i64 + 1)
    }

    fn has(&self, list: &SelectorList, node: NodeId) -> bool {
        let doc = self.doc;
        let inner = Cx {
            anchor: Some(node),
            ..*self
        };
        let mut candidates = Vec::new();
        let sibling_start = list.0.iter().any(|s| {
            matches!(
                s.combinators.first(),
                Some(Combinator::AdjacentSibling | Combinator::GeneralSibling)
            )
        });
        let mut roots = vec![node];
        if sibling_start {
            if let Some(parent) = doc.get(node).parent {
                let children = &doc.get(parent).children;
                if let Some(idx) = children.iter().position(|&c| c == node) {
                    roots.extend(children[idx + 1..].iter().copied());
                }
            }
        }
        for (k, root) in roots.into_iter().enumerate() {
            if k > 0 {
                candidates.push(root);
            }
            walk_descendants(doc, root, &mut |id| {
                candidates.push(id);
                true
            });
        }
        candidates
            .into_iter()
            .filter(|&c| doc.is_element(c))
            .any(|c| inner.list(list, c))
    }

    fn input_type(&self, node: NodeId) -> String {
        self.html_attr(node, "type")
            .unwrap_or("")
            .to_ascii_lowercase()
    }

    fn checked(&self, node: NodeId) -> bool {
        if !self.is_html(node) {
            return false;
        }
        match self.doc.tag_name(node) {
            Some("input") => {
                matches!(self.input_type(node).as_str(), "checkbox" | "radio")
                    && self.html_attr(node, "checked").is_some()
            }
            Some("option") => self.html_attr(node, "selected").is_some(),
            _ => false,
        }
    }

    fn is_submit_button(&self, node: NodeId) -> bool {
        if !self.is_html(node) {
            return false;
        }
        match self.doc.tag_name(node) {
            Some("button") => !matches!(self.input_type(node).as_str(), "reset" | "button"),
            Some("input") => matches!(self.input_type(node).as_str(), "submit" | "image"),
            _ => false,
        }
    }

    /// The first submit button of the element's form.
    fn is_default_button(&self, node: NodeId) -> bool {
        if !self.is_submit_button(node) {
            return false;
        }
        let doc = self.doc;
        let mut form = doc.get(node).parent;
        while let Some(f) = form {
            if self.is_html(f) && doc.tag_name(f) == Some("form") {
                break;
            }
            form = doc.get(f).parent;
        }
        let Some(form) = form else {
            return false;
        };
        let mut first = None;
        walk_descendants(doc, form, &mut |id| {
            if self.is_submit_button(id) {
                first = Some(id);
                return false;
            }
            true
        });
        first == Some(node)
    }

    fn disabled(&self, node: NodeId) -> bool {
        if !self.is_html(node) {
            return false;
        }
        let doc = self.doc;
        let has_disabled = self.html_attr(node, "disabled").is_some();
        match doc.tag_name(node) {
            Some("optgroup") => has_disabled,
            Some("option") => {
                has_disabled
                    || doc.get(node).parent.is_some_and(|p| {
                        self.is_html(p)
                            && doc.tag_name(p) == Some("optgroup")
                            && self.html_attr(p, "disabled").is_some()
                    })
            }
            Some("button" | "input" | "select" | "textarea" | "fieldset") => {
                has_disabled || self.in_disabled_fieldset(node)
            }
            _ => false,
        }
    }

    /// Whether an ancestor fieldset is disabled, outside that fieldset's first legend.
    fn in_disabled_fieldset(&self, node: NodeId) -> bool {
        let doc = self.doc;
        let mut child = node;
        let mut cur = doc.get(node).parent;
        while let Some(p) = cur {
            if self.is_html(p)
                && doc.tag_name(p) == Some("fieldset")
                && self.html_attr(p, "disabled").is_some()
            {
                let first_legend = doc
                    .get(p)
                    .children
                    .iter()
                    .copied()
                    .find(|&c| self.is_html(c) && doc.tag_name(c) == Some("legend"));
                if first_legend != Some(child) {
                    return true;
                }
            }
            child = p;
            cur = doc.get(p).parent;
        }
        false
    }

    fn read_write(&self, node: NodeId) -> bool {
        let doc = self.doc;
        if self.is_html(node) {
            let mutable = self.html_attr(node, "readonly").is_none() && !self.disabled(node);
            match doc.tag_name(node) {
                Some("input") => {
                    let t = self.input_type(node);
                    let text_like = matches!(
                        t.as_str(),
                        "text"
                            | "search"
                            | "url"
                            | "tel"
                            | "email"
                            | "password"
                            | "date"
                            | "month"
                            | "week"
                            | "time"
                            | "datetime-local"
                            | "number"
                    ) || !is_known_input_type(&t);
                    return mutable && text_like;
                }
                Some("textarea") => return mutable,
                _ => {}
            }
        }
        // contenteditable elements and their descendants.
        let mut cur = Some(node);
        while let Some(id) = cur {
            if !doc.is_element(id) {
                break;
            }
            if self.is_html(id) {
                if let Some(v) = self.html_attr(id, "contenteditable") {
                    let v = v.to_ascii_lowercase();
                    match v.as_str() {
                        "" | "true" | "plaintext-only" => return true,
                        "false" => return false,
                        _ => {}
                    }
                }
            }
            cur = doc.get(id).parent;
        }
        false
    }

    fn language(&self, node: NodeId) -> Option<&'a str> {
        let doc = self.doc;
        let mut cur = Some(node);
        while let Some(id) = cur {
            if !doc.is_element(id) {
                return None;
            }
            if let Some(l) = doc.attr_ns(id, Some(XML_NAMESPACE), "lang") {
                return Some(l);
            }
            if let Some(l) = self.html_attr(id, "lang") {
                return Some(l);
            }
            cur = doc.get(id).parent;
        }
        None
    }

    fn is_rtl(&self, node: NodeId) -> bool {
        let doc = self.doc;
        let mut cur = Some(node);
        while let Some(id) = cur {
            if !doc.is_element(id) {
                break;
            }
            if self.is_html(id) {
                match self
                    .html_attr(id, "dir")
                    .map(|d| d.to_ascii_lowercase())
                    .as_deref()
                {
                    Some("rtl") => return true,
                    Some("ltr") | Some("auto") => return false,
                    _ => {}
                }
            }
            cur = doc.get(id).parent;
        }
        false
    }
}

fn is_known_input_type(t: &str) -> bool {
    matches!(
        t,
        "hidden"
            | "text"
            | "search"
            | "tel"
            | "url"
            | "email"
            | "password"
            | "date"
            | "month"
            | "week"
            | "time"
            | "datetime-local"
            | "number"
            | "range"
            | "color"
            | "checkbox"
            | "radio"
            | "file"
            | "submit"
            | "image"
            | "reset"
            | "button"
    )
}

fn is_ascii_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')
}

fn nth_matches(a: i64, b: i64, pos: i64) -> bool {
    if a == 0 {
        return pos == b;
    }
    let diff = pos - b;
    diff % a == 0 && diff / a >= 0
}

fn prev_element(doc: &Document, node: NodeId) -> Option<NodeId> {
    let parent = doc.get(node).parent?;
    let children = &doc.get(parent).children;
    let idx = children.iter().position(|&c| c == node)?;
    children[..idx]
        .iter()
        .rev()
        .copied()
        .find(|&c| doc.is_element(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(s: &str) -> bool {
        parse_selector_list(s).is_some()
    }

    #[test]
    fn parses_and_rejects() {
        for s in [
            "a",
            "*",
            "a b > c + d ~ e",
            "#id.cls[attr]",
            "[a=\"b\" i]",
            "[a|=b]",
            "*|a",
            "|a",
            ":not(a, b c)",
            ":is(a, :hover, ::before)",
            ":where()",
            ":nth-child(2n+1 of .x)",
            ":nth-child( -n + 3 )",
            ":nth-of-type(odd)",
            "a::before",
            "a:before",
            "#zero\\0",
            "#\\d83d x",
            "#eof\\",
            "[a=\"b",
            ":not(a",
            ":has(> a, + b)",
            "a/**/.b",
        ] {
            assert!(valid(s), "{s} should parse");
        }
        for s in [
            "",
            ",",
            "a,",
            "a b >",
            "#1",
            ".",
            "[]",
            "[a=1]",
            "ns|a",
            "a|",
            ":unknown",
            "::unknown",
            ":not(::before)",
            "a::before b",
            "a/**/b",
            ":nth-child(+ 2)",
            ":nth-child(2 n)",
            "a)",
            "a || b",
            ":nth-of-type(n of a)",
        ] {
            assert!(!valid(s), "{s} should not parse");
        }
    }

    #[test]
    fn escapes_decode() {
        let list = parse_selector_list("#\\31 23.a\\:b").unwrap();
        let parts = &list.0[0].compounds[0].parts;
        assert_eq!(parts[0], Simple::Id("123".into()));
        assert_eq!(parts[1], Simple::Class("a:b".into()));
        let list = parse_selector_list("#a\\110000").unwrap();
        assert_eq!(
            list.0[0].compounds[0].parts[0],
            Simple::Id("a\u{FFFD}".into())
        );
    }

    #[test]
    fn anb_values() {
        let nth_of = |s: &str| match &parse_selector_list(s).unwrap().0[0].compounds[0].parts[0] {
            Simple::Pseudo(Pseudo::Nth { a, b, .. }) => (*a, *b),
            other => panic!("{other:?}"),
        };
        assert_eq!(nth_of(":nth-child(even)"), (2, 0));
        assert_eq!(nth_of(":nth-child(-n+3)"), (-1, 3));
        assert_eq!(nth_of(":nth-child(2n - 1)"), (2, -1));
        assert_eq!(nth_of(":nth-child(n- 1)"), (1, -1));
        assert_eq!(nth_of(":nth-child(+5)"), (0, 5));
        assert!(nth_matches(2, 1, 3) && !nth_matches(2, 1, 4) && nth_matches(-1, 3, 2));
        assert!(!nth_matches(-1, 3, 4));
    }
}
