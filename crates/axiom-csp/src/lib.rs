//! Content Security Policy (CSP Level 3): parsing, source matching and the checks a
//! document runs before fetching, executing inline code, compiling strings, submitting
//! forms and setting its base URL.
//!
//! A [`CspList`] belongs to one document. Every check returns a [`Verdict`] listing the
//! violated policies; the request or code is blocked when any *enforced* policy is
//! violated, while report-only violations are only reported.

pub mod hash;
mod source;
mod url;

use axiom_url::{Origin, Url};
use source::{Keyword, SelfContext};

pub use source::SourceList;
pub use url::CspUrl;

/// How a policy was delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySource {
    Header,
    Meta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Enforce,
    Report,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    /// Lowercase directive name.
    pub name: String,
    /// The value tokens as written.
    pub value: Vec<String>,
    sources: SourceList,
}

impl Directive {
    /// `name value…`, as quoted in console messages.
    pub fn text(&self) -> String {
        if self.value.is_empty() {
            self.name.clone()
        } else {
            format!("{} {}", self.name, self.value.join(" "))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub disposition: Disposition,
    pub source: PolicySource,
    directives: Vec<Directive>,
    text: String,
}

impl Policy {
    /// CSP3 §2.2.1 "parse a serialized CSP". Returns `None` when no directive survives.
    pub fn parse(serialized: &str, disposition: Disposition, source: PolicySource) -> Option<Self> {
        let mut directives: Vec<Directive> = Vec::new();
        for token in serialized.split(';') {
            let token = token.trim_matches(|c: char| c.is_ascii_whitespace());
            if token.is_empty() {
                continue;
            }
            let mut parts = token.split_ascii_whitespace();
            let Some(name) = parts.next() else {
                continue;
            };
            if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                continue;
            }
            let name = name.to_ascii_lowercase();
            // A repeated directive is ignored; the first one wins.
            if directives.iter().any(|d| d.name == name) {
                continue;
            }
            if source == PolicySource::Meta
                && matches!(name.as_str(), "report-uri" | "frame-ancestors" | "sandbox")
            {
                continue;
            }
            let value: Vec<String> = parts.map(str::to_string).collect();
            let sources = SourceList::parse(value.iter().map(String::as_str));
            directives.push(Directive {
                name,
                value,
                sources,
            });
        }
        if directives.is_empty() {
            return None;
        }
        Some(Self {
            disposition,
            source,
            directives,
            text: serialized.trim().to_string(),
        })
    }

    pub fn directives(&self) -> &[Directive] {
        &self.directives
    }

    pub fn directive(&self, name: &str) -> Option<&Directive> {
        self.directives.iter().find(|d| d.name == name)
    }

    /// The serialized policy, as reported in `originalPolicy`.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The first directive of `fallbacks` present in this policy.
    fn effective(&self, fallbacks: &[&str]) -> Option<&Directive> {
        fallbacks.iter().find_map(|name| self.directive(name))
    }
}

/// Fetch directives a request is checked against (CSP3 §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchDirective {
    ScriptSrcElem,
    StyleSrcElem,
    ImgSrc,
    FontSrc,
    ConnectSrc,
    MediaSrc,
    ObjectSrc,
    FrameSrc,
    WorkerSrc,
    ManifestSrc,
}

impl FetchDirective {
    pub fn name(self) -> &'static str {
        self.fallbacks()[0]
    }

    /// CSP3 §6.8.3 "get the effective directive fallback list".
    fn fallbacks(self) -> &'static [&'static str] {
        match self {
            Self::ScriptSrcElem => &["script-src-elem", "script-src", "default-src"],
            Self::StyleSrcElem => &["style-src-elem", "style-src", "default-src"],
            Self::ImgSrc => &["img-src", "default-src"],
            Self::FontSrc => &["font-src", "default-src"],
            Self::ConnectSrc => &["connect-src", "default-src"],
            Self::MediaSrc => &["media-src", "default-src"],
            Self::ObjectSrc => &["object-src", "default-src"],
            Self::FrameSrc => &["frame-src", "child-src", "default-src"],
            Self::WorkerSrc => &["worker-src", "child-src", "script-src", "default-src"],
            Self::ManifestSrc => &["manifest-src", "default-src"],
        }
    }

    fn is_script_like(self) -> bool {
        matches!(self, Self::ScriptSrcElem | Self::WorkerSrc)
    }

    fn noun(self) -> &'static str {
        match self {
            Self::ScriptSrcElem => "the script",
            Self::StyleSrcElem => "the stylesheet",
            Self::ImgSrc => "the image",
            Self::FontSrc => "the font",
            Self::ConnectSrc => "connect to",
            Self::MediaSrc => "the media",
            Self::ObjectSrc => "the plugin",
            Self::FrameSrc => "frame",
            Self::WorkerSrc => "the worker",
            Self::ManifestSrc => "the manifest",
        }
    }
}

/// Kinds of inline code (CSP3 §6.7.3.3 "type").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineKind {
    /// An inline `<script>` element.
    Script,
    /// An event handler content attribute (`onclick="…"`).
    ScriptAttribute,
    /// An inline `<style>` element.
    Style,
    /// A `style="…"` attribute.
    StyleAttribute,
    /// A `javascript:` URL navigation.
    Navigation,
}

impl InlineKind {
    fn fallbacks(self) -> &'static [&'static str] {
        match self {
            Self::Script | Self::Navigation => &["script-src-elem", "script-src", "default-src"],
            Self::ScriptAttribute => &["script-src-attr", "script-src", "default-src"],
            Self::Style => &["style-src-elem", "style-src", "default-src"],
            Self::StyleAttribute => &["style-src-attr", "style-src", "default-src"],
        }
    }

    fn is_script(self) -> bool {
        matches!(
            self,
            Self::Script | Self::ScriptAttribute | Self::Navigation
        )
    }

    fn is_element(self) -> bool {
        matches!(self, Self::Script | Self::Style)
    }

    fn action(self) -> &'static str {
        match self {
            Self::Script => "execute inline script",
            Self::ScriptAttribute => "execute inline event handler",
            Self::Style => "apply inline style",
            Self::StyleAttribute => "apply inline style attribute",
            Self::Navigation => "run the javascript: URL",
        }
    }
}

/// Request metadata the script and style directives look at.
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestInfo<'a> {
    /// The initiating element's `nonce` attribute (when the element is nonceable).
    pub nonce: Option<&'a str>,
    /// True for elements created by the HTML parser (`'strict-dynamic'` blocks these).
    pub parser_inserted: bool,
    /// True when the URL is a redirect target; paths are then not compared and reports
    /// only name the origin.
    pub redirected: bool,
}

/// One policy's objection to a request or to inline code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The directive the check was for, e.g. `script-src-elem`.
    pub effective_directive: String,
    /// The directive in the policy that failed, e.g. `default-src` when it is the fallback.
    pub violated_directive: String,
    /// `name value…` of the failed directive.
    pub directive_text: String,
    /// `inline`, `eval`, or the blocked URL (only its origin after a redirect).
    pub blocked_uri: String,
    /// The first 40 characters of inline code or eval input when the directive has
    /// `'report-sample'`; empty otherwise.
    pub sample: String,
    pub disposition: Disposition,
    pub source: PolicySource,
    pub original_policy: String,
    action: String,
}

impl Violation {
    /// A console message in the familiar browser wording.
    pub fn message(&self) -> String {
        let prefix = match self.disposition {
            Disposition::Enforce => "",
            Disposition::Report => "[Report Only] ",
        };
        let mut msg = format!(
            "{prefix}Refused to {} because it violates the following Content Security Policy directive: \"{}\".",
            self.action, self.directive_text
        );
        if self.violated_directive != self.effective_directive {
            msg.push_str(&format!(
                " Note that '{}' was not explicitly set, so '{}' is used as a fallback.",
                self.effective_directive, self.violated_directive
            ));
        }
        msg
    }
}

/// The result of a CSP check.
#[must_use]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Verdict {
    pub violations: Vec<Violation>,
}

impl Verdict {
    /// True when an enforced policy was violated.
    pub fn is_blocked(&self) -> bool {
        self.violations
            .iter()
            .any(|v| v.disposition == Disposition::Enforce)
    }

    pub fn is_allowed(&self) -> bool {
        !self.is_blocked()
    }
}

/// A document's policies.
#[derive(Debug, Clone)]
pub struct CspList {
    policies: Vec<Policy>,
    ctx: SelfContext,
}

impl CspList {
    /// An empty list for the document at `document_url`, which `'self'` resolves against.
    pub fn new(document_url: &str) -> Self {
        let scheme = CspUrl::parse(document_url)
            .map(|u| u.scheme)
            .unwrap_or_default();
        Self {
            policies: Vec::new(),
            ctx: SelfContext {
                origin: Origin::of_document(document_url),
                scheme,
            },
        }
    }

    /// Add the policies of one `Content-Security-Policy` (`Enforce`) or
    /// `Content-Security-Policy-Report-Only` (`Report`) header value, which may hold
    /// several comma-separated policies.
    pub fn add_header(&mut self, value: &str, disposition: Disposition) {
        for serialized in value.split(',') {
            if let Some(p) = Policy::parse(serialized, disposition, PolicySource::Header) {
                self.policies.push(p);
            }
        }
    }

    /// Add a `<meta http-equiv="Content-Security-Policy">` policy (always enforced).
    pub fn add_meta(&mut self, content: &str) {
        if let Some(p) = Policy::parse(content, Disposition::Enforce, PolicySource::Meta) {
            self.policies.push(p);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }

    pub fn policies(&self) -> &[Policy] {
        &self.policies
    }

    /// CSP3 §4.1.2 "should request be blocked": the pre-request check for `url`.
    pub fn check_request(
        &self,
        directive: FetchDirective,
        url: &CspUrl,
        request: RequestInfo<'_>,
    ) -> Verdict {
        let blocked_uri = if request.redirected {
            url.origin_string()
        } else {
            url.report_string()
        };
        let action = format!("load {} '{blocked_uri}'", directive.noun());
        let action = if directive == FetchDirective::ConnectSrc {
            format!("connect to '{blocked_uri}'")
        } else {
            action
        };
        self.check(directive.fallbacks(), &blocked_uri, "", &action, |list| {
            if (directive.is_script_like() || directive == FetchDirective::StyleSrcElem)
                && list.matches_nonce(request.nonce)
            {
                return true;
            }
            if directive.is_script_like() && list.has_keyword(Keyword::StrictDynamic) {
                return !request.parser_inserted;
            }
            list.matches_url(url, &self.ctx, request.redirected)
        })
    }

    /// CSP3 §4.2.3 "should element's inline type behavior be blocked": `source` is the
    /// script or style text, or the attribute value.
    pub fn check_inline(&self, kind: InlineKind, source: &str, nonce: Option<&str>) -> Verdict {
        self.check(kind.fallbacks(), "inline", source, kind.action(), |list| {
            let overridden = list.has_nonce_or_hash()
                || (kind.is_script() && list.has_keyword(Keyword::StrictDynamic));
            let allow_all_inline = !overridden && list.has_keyword(Keyword::UnsafeInline);
            if allow_all_inline {
                return true;
            }
            if kind.is_element() && list.matches_nonce(nonce) {
                return true;
            }
            (kind.is_element() || list.has_keyword(Keyword::UnsafeHashes))
                && list.matches_hash(source)
        })
    }

    /// CSP3 §4.4 "EnsureCSPDoesNotBlockStringCompilation" (`eval`, `new Function`,
    /// string timers).
    pub fn check_eval(&self, source: &str) -> Verdict {
        self.check(
            &["script-src", "default-src"],
            "eval",
            source,
            "evaluate a string as JavaScript",
            |list| list.has_keyword(Keyword::UnsafeEval),
        )
    }

    /// `form-action` for a form submission to `url` (no fallback).
    pub fn check_form_action(&self, url: &CspUrl, redirected: bool) -> Verdict {
        let blocked = if redirected {
            url.origin_string()
        } else {
            url.report_string()
        };
        let action = format!("send form data to '{blocked}'");
        self.check(&["form-action"], &blocked, "", &action, |list| {
            list.matches_url(url, &self.ctx, redirected)
        })
    }

    /// `base-uri` for a `<base href>` resolving to `url` (no fallback).
    pub fn check_base_uri(&self, url: &CspUrl) -> Verdict {
        let blocked = url.report_string();
        let action = format!("set the document's base URI to '{blocked}'");
        self.check(&["base-uri"], &blocked, "", &action, |list| {
            list.matches_url(url, &self.ctx, false)
        })
    }

    /// True when an enforced policy has `upgrade-insecure-requests`.
    pub fn upgrades_insecure_requests(&self) -> bool {
        self.policies.iter().any(|p| {
            p.disposition == Disposition::Enforce
                && p.directive("upgrade-insecure-requests").is_some()
        })
    }

    fn check(
        &self,
        fallbacks: &[&str],
        blocked_uri: &str,
        sample_source: &str,
        action: &str,
        allows: impl Fn(&SourceList) -> bool,
    ) -> Verdict {
        let mut verdict = Verdict::default();
        for policy in &self.policies {
            let Some(directive) = policy.effective(fallbacks) else {
                continue;
            };
            if allows(&directive.sources) {
                continue;
            }
            let sample = if !sample_source.is_empty()
                && directive.sources.has_keyword(Keyword::ReportSample)
            {
                sample_source.chars().take(40).collect()
            } else {
                String::new()
            };
            verdict.violations.push(Violation {
                effective_directive: fallbacks[0].to_string(),
                violated_directive: directive.name.clone(),
                directive_text: directive.text(),
                blocked_uri: blocked_uri.to_string(),
                sample,
                disposition: policy.disposition,
                source: policy.source,
                original_policy: policy.text.clone(),
                action: action.to_string(),
            });
        }
        verdict
    }
}

/// `upgrade-insecure-requests`: the `https` form of an `http` URL (port 80 becomes the
/// default port), or `None` when `url` is not `http`.
pub fn upgrade_url(url: &Url) -> Option<Url> {
    if url.scheme != "http" {
        return None;
    }
    let mut upgraded = url.clone();
    upgraded.scheme = "https".into();
    if upgraded.port == Some(80) {
        upgraded.port = None;
    }
    Some(upgraded)
}

/// HTML "is element nonceable": an element whose attribute names or values contain
/// `<script` or `<style` may be the result of markup injection into a nonced element, so
/// its nonce is disregarded.
pub fn is_nonceable<'a>(attributes: impl IntoIterator<Item = (&'a str, &'a str)>) -> bool {
    !attributes.into_iter().any(|(name, value)| {
        [name, value].iter().any(|s| {
            let lower = s.to_ascii_lowercase();
            lower.contains("<script") || lower.contains("<style")
        })
    })
}

#[cfg(test)]
mod tests;
