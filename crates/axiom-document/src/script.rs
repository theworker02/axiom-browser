//! `<script>` classification.

use axiom_net::MimeType;

/// How a script element executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScriptKind {
    /// Inline classic script: runs at its parser position (after script-blocking
    /// stylesheets), or synchronously on insertion when added by script.
    ClassicInline,
    /// External parser-inserted classic script without `async`/`defer`: pauses the parser
    /// until fetched and executed.
    ClassicBlocking,
    /// `async`: runs as soon as it is available, in no particular order.
    ClassicAsync,
    /// `defer`: runs after parsing, in document order, before `DOMContentLoaded`.
    ClassicDefer,
    /// External script inserted by script: never parser-blocking; runs when available.
    ClassicDynamic,
    /// Parser-inserted `type="module"` without `async`: its graph loads during parsing
    /// and it runs after parsing, in document order with `defer` scripts.
    Module,
    /// `type="module"` with `async`, or inserted by script: runs once its graph loaded.
    ModuleAsync,
    /// `type="importmap"`: inline JSON registered with the document's module resolution.
    ImportMap,
    /// Any other `type` (data block): never executed.
    DataBlock,
}

impl ScriptKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClassicInline => "inline",
            Self::ClassicBlocking => "parser-blocking",
            Self::ClassicAsync => "async",
            Self::ClassicDefer => "defer",
            Self::ClassicDynamic => "dynamic",
            Self::Module => "module",
            Self::ModuleAsync => "async module",
            Self::ImportMap => "import map",
            Self::DataBlock => "data block",
        }
    }

    pub fn is_executable(self) -> bool {
        self != Self::DataBlock
    }

    pub fn is_module(self) -> bool {
        matches!(self, Self::Module | Self::ModuleAsync)
    }

    pub fn is_external(self) -> bool {
        matches!(
            self,
            Self::ClassicBlocking | Self::ClassicAsync | Self::ClassicDefer | Self::ClassicDynamic
        )
    }
}

/// The attributes that decide a script's kind.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScriptElement<'a> {
    pub src: Option<&'a str>,
    pub is_async: bool,
    pub defer: bool,
    pub type_attr: Option<&'a str>,
    /// Inserted by the HTML parser (as opposed to by script).
    pub parser_inserted: bool,
}

/// Whether a `<script type>` denotes a classic script.
pub fn is_classic_script_type(type_attr: Option<&str>) -> bool {
    match type_attr.map(str::trim) {
        None | Some("") => true,
        Some(t) => MimeType::parse(t).is_some_and(|m| m.is_javascript()),
    }
}

/// Script text of an XHTML document parsed as HTML: an XML parser would have replaced
/// each `<![CDATA[ … ]]>` section with its contents.
pub fn unwrap_cdata(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<![CDATA[") {
        let inner = &rest[start + 9..];
        let Some(end) = inner.find("]]>") else { break };
        out.push_str(&rest[..start]);
        out.push_str(&inner[..end]);
        rest = &inner[end + 3..];
    }
    out.push_str(rest);
    out
}

pub fn classify_script(el: ScriptElement<'_>) -> ScriptKind {
    if el
        .type_attr
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("module"))
    {
        return if el.is_async || !el.parser_inserted {
            ScriptKind::ModuleAsync
        } else {
            ScriptKind::Module
        };
    }
    if el
        .type_attr
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("importmap"))
    {
        return ScriptKind::ImportMap;
    }
    if !is_classic_script_type(el.type_attr) {
        return ScriptKind::DataBlock;
    }
    match el.src.map(str::trim).filter(|s| !s.is_empty()) {
        None => ScriptKind::ClassicInline,
        Some(_) if !el.parser_inserted => ScriptKind::ClassicDynamic,
        Some(_) if el.is_async => ScriptKind::ClassicAsync,
        Some(_) if el.defer => ScriptKind::ClassicDefer,
        Some(_) => ScriptKind::ClassicBlocking,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser(src: Option<&str>, is_async: bool, defer: bool, ty: Option<&str>) -> ScriptKind {
        classify_script(ScriptElement {
            src,
            is_async,
            defer,
            type_attr: ty,
            parser_inserted: true,
        })
    }

    #[test]
    fn classifies_every_script_kind() {
        assert_eq!(parser(None, false, false, None), ScriptKind::ClassicInline);
        // async/defer are ignored on inline scripts.
        assert_eq!(parser(None, true, true, None), ScriptKind::ClassicInline);
        assert_eq!(
            parser(Some("a.js"), false, false, None),
            ScriptKind::ClassicBlocking
        );
        assert_eq!(
            parser(Some("a.js"), true, true, None),
            ScriptKind::ClassicAsync
        );
        assert_eq!(
            parser(Some("a.js"), false, true, None),
            ScriptKind::ClassicDefer
        );
        assert_eq!(
            parser(Some("a.js"), false, false, Some("text/javascript")),
            ScriptKind::ClassicBlocking
        );
        assert_eq!(
            parser(Some("m.js"), false, false, Some(" Module ")),
            ScriptKind::Module
        );
        assert_eq!(
            parser(None, false, false, Some("application/json")),
            ScriptKind::DataBlock
        );
        assert_eq!(
            parser(Some(" "), false, false, None),
            ScriptKind::ClassicInline
        );
        let dynamic = classify_script(ScriptElement {
            src: Some("d.js"),
            parser_inserted: false,
            ..ScriptElement::default()
        });
        assert_eq!(dynamic, ScriptKind::ClassicDynamic);
        assert_eq!(
            parser(Some("m.js"), true, false, Some("module")),
            ScriptKind::ModuleAsync
        );
        assert_eq!(
            parser(None, false, true, Some("module")),
            ScriptKind::Module
        );
        let dynamic_module = classify_script(ScriptElement {
            type_attr: Some("module"),
            ..ScriptElement::default()
        });
        assert_eq!(dynamic_module, ScriptKind::ModuleAsync);
        assert_eq!(
            parser(None, false, false, Some("importmap")),
            ScriptKind::ImportMap
        );
        assert!(ScriptKind::Module.is_executable());
        assert!(!ScriptKind::DataBlock.is_executable());
    }

    #[test]
    fn cdata_sections_are_unwrapped() {
        assert_eq!(unwrap_cdata("<![CDATA[ a < b ]]>"), " a < b ");
        assert_eq!(unwrap_cdata("x<![CDATA[1]]>y<![CDATA[2]]>z"), "x1y2z");
        assert_eq!(unwrap_cdata("//<![CDATA[\nf();\n//]]>"), "//\nf();\n//");
        assert_eq!(
            unwrap_cdata("<![CDATA[ unterminated"),
            "<![CDATA[ unterminated"
        );
    }
}
