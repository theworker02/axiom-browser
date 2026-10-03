//! CSS parser: style sheets into rules, following the CSS Syntax error-recovery model.
//!
//! Comments, strings, escapes and nested `()` / `[]` / `{}` blocks are respected, so a
//! `;` or `}` inside a string or function never ends a declaration. Conditional group
//! rules (`@media`, `@supports`, `@layer`, `@container`, `@document`) keep their
//! conditions and child rules; the style engine evaluates them. Other at-rules are kept
//! by name with their raw prelude so callers can ignore them explicitly. Selector text
//! is kept raw: the style engine compiles it with the Selectors Level 4 engine in
//! `axiom-dom`. Nested style rules (CSS Nesting) are skipped.

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stylesheet {
    pub rules: Vec<CssRule>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CssRule {
    Style(StyleRule),
    Media {
        query: String,
        rules: Vec<CssRule>,
    },
    Supports {
        condition: String,
        rules: Vec<CssRule>,
    },
    /// `@layer name { … }`, `@container … { … }`, `@document … { … }`: children apply
    /// unconditionally.
    Group {
        name: String,
        rules: Vec<CssRule>,
    },
    Import {
        prelude: String,
    },
    /// Any other at-rule (`@font-face`, `@keyframes`, `@page`, …), kept by name.
    Other {
        name: String,
        prelude: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyleRule {
    pub selectors: String,
    pub declarations: Vec<Declaration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    /// Lowercase, except custom properties (`--name`), which are case-sensitive.
    pub property: String,
    pub value: String,
    pub important: bool,
}

pub fn parse_stylesheet(input: &str) -> Stylesheet {
    let mut p = Parser {
        s: input.as_bytes(),
        src: input,
        i: 0,
    };
    Stylesheet {
        rules: p.rule_list(true),
    }
}

/// Declarations of a `style` attribute or a declaration block body.
pub fn parse_declarations(input: &str) -> Vec<Declaration> {
    let mut p = Parser {
        s: input.as_bytes(),
        src: input,
        i: 0,
    };
    p.declaration_list()
}

struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn starts_with(&self, t: &str) -> bool {
        self.s[self.i..].starts_with(t.as_bytes())
    }

    fn skip_comment(&mut self) -> bool {
        if self.starts_with("/*") {
            match self.src[self.i + 2..].find("*/") {
                Some(end) => self.i += end + 4,
                None => self.i = self.s.len(),
            }
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.i += 1;
            }
            if self.skip_comment() {
                continue;
            }
            if self.starts_with("<!--") {
                self.i += 4;
                continue;
            }
            if self.starts_with("-->") {
                self.i += 3;
                continue;
            }
            // XHTML style sheets wrap rules in CDATA sections, which an XML parser would
            // strip before CSS sees them; the HTML parser keeps the markers as text.
            if self.starts_with("<![CDATA[") {
                self.i += 9;
                continue;
            }
            if self.starts_with("]]>") {
                self.i += 3;
                continue;
            }
            break;
        }
    }

    fn skip_string(&mut self, quote: u8) {
        self.i += 1;
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'\\' => self.i += 1,
                b'\n' => return,
                c if c == quote => return,
                _ => {}
            }
        }
        self.i = self.i.min(self.s.len());
    }

    /// Advance past a balanced block whose opener is at `self.i`.
    fn skip_block(&mut self) {
        let mut stack = Vec::new();
        while let Some(c) = self.peek() {
            match c {
                b'"' | b'\'' => {
                    self.skip_string(c);
                    continue;
                }
                b'/' if self.skip_comment() => continue,
                b'\\' => self.i += 1,
                b'{' => stack.push(b'}'),
                b'(' => stack.push(b')'),
                b'[' => stack.push(b']'),
                b'}' | b')' | b']' => {
                    if stack.last() == Some(&c) {
                        stack.pop();
                    }
                    if stack.is_empty() {
                        self.i += 1;
                        return;
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
    }

    /// Raw text up to (not including) the first top-level byte in `stops`.
    fn consume_until(&mut self, stops: &[u8]) -> String {
        let start = self.i;
        let mut out = String::new();
        let mut last = start;
        while let Some(c) = self.peek() {
            if stops.contains(&c) {
                break;
            }
            match c {
                b'"' | b'\'' => self.skip_string(c),
                b'(' | b'[' => self.skip_block(),
                b'{' if !stops.contains(&b'{') => self.skip_block(),
                b'\\' => self.i = (self.i + 2).min(self.s.len()),
                b'/' if self.starts_with("/*") => {
                    out.push_str(&self.src[last..self.i]);
                    self.skip_comment();
                    out.push(' ');
                    last = self.i;
                }
                _ => self.i += 1,
            }
        }
        let end = self.i.min(self.s.len());
        out.push_str(&self.src[last..end]);
        out
    }

    fn ident(&mut self) -> String {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c >= 0x80)
        {
            self.i += 1;
        }
        self.src[start..self.i].to_string()
    }

    fn rule_list(&mut self, top: bool) -> Vec<CssRule> {
        let mut rules = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(b'}') => {
                    if top {
                        self.i += 1;
                        continue;
                    }
                    self.i += 1;
                    break;
                }
                Some(b'@') => {
                    self.i += 1;
                    if let Some(rule) = self.at_rule() {
                        rules.push(rule);
                    }
                }
                Some(_) => {
                    let prelude = self.consume_until(b"{}");
                    // A stray `}` is handled by the loop; EOF ends the list.
                    if self.peek() == Some(b'{') {
                        self.i += 1;
                        let declarations = self.declaration_list();
                        let selectors = prelude.trim().to_string();
                        if !selectors.is_empty() {
                            rules.push(CssRule::Style(StyleRule {
                                selectors,
                                declarations,
                            }));
                        }
                    }
                }
            }
        }
        rules
    }

    fn at_rule(&mut self) -> Option<CssRule> {
        let name = self.ident().to_ascii_lowercase();
        let prelude = self.consume_until(b";{}").trim().to_string();
        match self.peek() {
            Some(b';') => {
                self.i += 1;
                return Some(if name == "import" {
                    CssRule::Import { prelude }
                } else {
                    CssRule::Other { name, prelude }
                });
            }
            Some(b'{') => {}
            _ => {
                return (name == "import").then_some(CssRule::Import { prelude });
            }
        }
        self.i += 1;
        let rule = match name.as_str() {
            "media" => CssRule::Media {
                query: prelude,
                rules: self.rule_list(false),
            },
            "supports" => CssRule::Supports {
                condition: prelude,
                rules: self.rule_list(false),
            },
            "layer" | "container" | "document" | "-moz-document" | "scope" | "starting-style" => {
                CssRule::Group {
                    name,
                    rules: self.rule_list(false),
                }
            }
            _ => {
                self.i -= 1;
                self.skip_block();
                CssRule::Other { name, prelude }
            }
        };
        Some(rule)
    }

    /// Declarations until the matching `}` (consumed) or EOF. Nested rules are skipped.
    fn declaration_list(&mut self) -> Vec<Declaration> {
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(b'}') => {
                    self.i += 1;
                    break;
                }
                Some(b';') => {
                    self.i += 1;
                    continue;
                }
                _ => {}
            }
            let start = self.i;
            let text = self.consume_until(b";{}");
            if self.peek() == Some(b'{') {
                // A nested rule (`&:hover { … }`, `@media … { … }`): skip it.
                self.skip_block();
                continue;
            }
            if self.peek() == Some(b';') {
                self.i += 1;
            }
            if self.i == start {
                self.i += 1;
                continue;
            }
            if let Some(decl) = parse_one_declaration(&text) {
                out.push(decl);
            }
        }
        out
    }
}

fn parse_one_declaration(text: &str) -> Option<Declaration> {
    let (name, value) = text.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    let property = if name.starts_with("--") {
        name.to_string()
    } else {
        name.to_ascii_lowercase()
    };
    let mut value = value.trim();
    let mut important = false;
    if let Some(bang) = value.rfind('!') {
        if value[bang + 1..].trim().eq_ignore_ascii_case("important") {
            important = true;
            value = value[..bang].trim_end();
        }
    }
    if value.is_empty() && !property.starts_with("--") {
        return None;
    }
    Some(Declaration {
        property,
        value: value.to_string(),
        important,
    })
}

/// The HTML user-agent style sheet (a subset of the HTML Standard's rendering section).
pub fn user_agent_stylesheet() -> Stylesheet {
    parse_stylesheet(UA_CSS)
}

pub const UA_CSS: &str = r#"
html, address, blockquote, body, center, dialog, div, figure, figcaption, footer, form,
header, hr, legend, listing, main, p, plaintext, pre, search, xmp, article, aside, h1, h2,
h3, h4, h5, h6, hgroup, nav, section, dl, dd, dt, ol, ul, menu, details, summary,
fieldset, optgroup, address, frameset, frame { display: block; }
[hidden]:not([hidden=until-found i]), area, base, basefont, datalist, head, link, meta,
noembed, noframes, param, rp, script, style, template, title, noscript, dialog:not([open]),
input[type=hidden i] { display: none; }
slot { display: contents; }
li { display: list-item; }
table { display: table; }
caption { display: table-caption; }
colgroup, colgroup[hidden] { display: table-column-group; }
col { display: table-column; }
thead { display: table-header-group; }
tbody { display: table-row-group; }
tfoot { display: table-footer-group; }
tr { display: table-row; }
td, th { display: table-cell; }
ruby { display: ruby; }
rt { display: ruby-text; }
summary { display: list-item; list-style-type: disclosure-closed; }
details[open] > summary:first-of-type { list-style-type: disclosure-open; }
html { color: #000; }
body { margin: 8px; }
p, blockquote, figure, listing, plaintext, pre, xmp, dl { margin-top: 1em; margin-bottom: 1em; }
blockquote, figure { margin-left: 40px; margin-right: 40px; }
dd { margin-left: 40px; }
address, i, cite, dfn, em, var { font-style: italic; }
b, strong, th { font-weight: bolder; }
h1 { font-size: 2em; margin-top: 0.67em; margin-bottom: 0.67em; font-weight: bold; }
h2 { font-size: 1.5em; margin-top: 0.83em; margin-bottom: 0.83em; font-weight: bold; }
h3 { font-size: 1.17em; margin-top: 1em; margin-bottom: 1em; font-weight: bold; }
h4 { margin-top: 1.33em; margin-bottom: 1.33em; font-weight: bold; }
h5 { font-size: 0.83em; margin-top: 1.67em; margin-bottom: 1.67em; font-weight: bold; }
h6 { font-size: 0.67em; margin-top: 2.33em; margin-bottom: 2.33em; font-weight: bold; }
article h1, aside h1, nav h1, section h1 { font-size: 1.5em; margin-top: 0.83em; margin-bottom: 0.83em; }
ol, ul, menu { margin-top: 1em; margin-bottom: 1em; padding-left: 40px; }
ul, menu { list-style-type: disc; }
ol { list-style-type: decimal; }
ul ul, ol ul, menu ul, ul menu { list-style-type: circle; }
ul ul, ul ol, ol ul, ol ol, ul menu, menu ul { margin-top: 0; margin-bottom: 0; }
pre, listing, plaintext, xmp, code, kbd, samp, tt { font-family: monospace; }
pre, listing, plaintext, xmp { white-space: pre; }
pre, code, kbd, samp, tt { font-size: 0.8125em; }
pre code, pre kbd, pre samp, pre tt { font-size: 1em; }
textarea { white-space: pre-wrap; }
small { font-size: smaller; }
big { font-size: larger; }
sub { vertical-align: sub; font-size: smaller; }
sup { vertical-align: super; font-size: smaller; }
u, ins { text-decoration: underline; }
s, strike, del { text-decoration: line-through; }
mark { background-color: #ffff00; color: #000; }
nobr { white-space: nowrap; }
q::before { content: open-quote; }
q::after { content: close-quote; }
center { text-align: -webkit-center; }
a:any-link { color: #0000ee; text-decoration: underline; cursor: pointer; }
hr { color: gray; border-style: inset; border-width: 1px; margin: 0.5em auto; overflow: hidden; }
fieldset { margin-left: 2px; margin-right: 2px; border: 2px groove #c0c0c0; padding: 0.35em 0.75em 0.625em; }
legend { padding-left: 2px; padding-right: 2px; }
table { border-spacing: 2px; border-collapse: separate; text-indent: 0; }
td, th { padding: 1px; vertical-align: inherit; }
thead, tbody, tfoot, tr { vertical-align: middle; }
th { text-align: center; }
caption { text-align: center; }
img, svg, video, canvas, iframe, embed, object { display: inline-block; }
iframe { border: 2px inset #767676; }
input, select, button, textarea { display: inline-block; font-family: sans-serif; font-size: 13.333px; color: #000; letter-spacing: normal; text-align: start; }
input, textarea { background-color: #fff; border: 2px inset #767676; padding: 1px 2px; }
select { background-color: #fff; border: 1px solid #767676; padding: 1px 2px; border-radius: 3px; }
button, input[type=submit i], input[type=button i], input[type=reset i] { background-color: #efefef; border: 2px outset #767676; padding: 1px 6px; text-align: center; }
input[type=checkbox i], input[type=radio i] { margin: 3px 3px 3px 4px; padding: 0; border: 1px solid #767676; }
input[type=image i] { border: 0; padding: 0; }
input[type=range i], input[type=color i], input[type=file i] { border: 0; background: transparent; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn style_rules(sheet: &Stylesheet) -> Vec<&StyleRule> {
        sheet
            .rules
            .iter()
            .filter_map(|r| match r {
                CssRule::Style(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parses_rules_declarations_and_important() {
        let sheet = parse_stylesheet(
            "body { background-color: #eee; width: 600px !important; margin: 0 auto }\nh1, h2 > a { color: #444; }",
        );
        let rules = style_rules(&sheet);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].selectors, "body");
        assert_eq!(rules[0].declarations.len(), 3);
        assert!(rules[0].declarations[1].important);
        assert_eq!(rules[0].declarations[1].value, "600px");
        assert_eq!(rules[1].selectors, "h1, h2 > a");
    }

    #[test]
    fn strings_and_functions_do_not_end_declarations() {
        let sheet = parse_stylesheet(
            r#"a::after { content: "}; x"; background: url(data:image/png;base64,AA==) } b { color: red }"#,
        );
        let rules = style_rules(&sheet);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].declarations[0].value, r#""}; x""#);
        assert_eq!(
            rules[0].declarations[1].value,
            "url(data:image/png;base64,AA==)"
        );
        assert_eq!(rules[1].declarations[0].value, "red");
    }

    #[test]
    fn media_and_supports_keep_children_and_other_at_rules_are_skipped() {
        let sheet = parse_stylesheet(
            "@charset 'utf-8'; @import url(a.css) screen; @media (min-width: 600px) { p { margin: 0 } } \
             @font-face { font-family: X; src: url(x.woff) } @keyframes k { from { opacity: 0 } } \
             @supports (display: grid) { div { display: grid } } @layer base { i { color: red } } q { color: blue }",
        );
        let kinds: Vec<&str> = sheet
            .rules
            .iter()
            .map(|r| match r {
                CssRule::Style(_) => "style",
                CssRule::Media { .. } => "media",
                CssRule::Supports { .. } => "supports",
                CssRule::Group { .. } => "group",
                CssRule::Import { .. } => "import",
                CssRule::Other { .. } => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            ["other", "import", "media", "other", "other", "supports", "group", "style"]
        );
        match &sheet.rules[2] {
            CssRule::Media { query, rules } => {
                assert_eq!(query, "(min-width: 600px)");
                assert_eq!(rules.len(), 1);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn nested_rules_and_bad_declarations_are_skipped() {
        let sheet = parse_stylesheet(
            ".a { color: red; &:hover { color: blue } ; bogus; margin: 1px } .b { --Custom: { x }; padding: 2px }",
        );
        let rules = style_rules(&sheet);
        assert_eq!(rules.len(), 2);
        let props: Vec<&str> = rules[0]
            .declarations
            .iter()
            .map(|d| d.property.as_str())
            .collect();
        assert_eq!(props, ["color", "margin"]);
        assert_eq!(rules[1].declarations.last().unwrap().property, "padding");
    }

    #[test]
    fn comments_and_custom_property_case() {
        let decls =
            parse_declarations("/* c */ --Main-Color : #fff ; COLOR: var(--Main-Color) /* x */");
        assert_eq!(decls[0].property, "--Main-Color");
        assert_eq!(decls[0].value, "#fff");
        assert_eq!(decls[1].property, "color");
        assert_eq!(decls[1].value.trim(), "var(--Main-Color)");
    }

    #[test]
    fn cdo_cdc_and_cdata_markers_are_ignored_between_rules() {
        let sheet =
            parse_stylesheet("<!-- <![CDATA[\n div { height: 5px }\n ]]> --> p { margin: 0 }");
        let rules = style_rules(&sheet);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].declarations[0].property, "height");
    }

    #[test]
    fn user_agent_sheet_parses() {
        let ua = user_agent_stylesheet();
        assert!(style_rules(&ua).len() > 40);
    }
}
