//! The active document's Content Security Policy: its policy list, the checks the engine
//! runs against it, and the violations waiting to be reported to script and the console.

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use axiom_csp::{
    is_nonceable, upgrade_url, CspList, CspUrl, Disposition, FetchDirective, InlineKind,
    RequestInfo, Verdict, Violation,
};
use axiom_dom::{Document, NodeId};
use axiom_loader::RedirectCheck;
use axiom_url::Url;
use parking_lot::Mutex;

/// Violations kept for diagnostics.
const MAX_RECORDED: usize = 256;
/// Violations waiting to be reported; more are counted but dropped (a script can produce
/// them in a loop).
const MAX_PENDING: usize = 1024;

/// A violation and the element it is reported at (`None`: the document).
#[derive(Debug, Clone)]
pub struct CspViolationRecord {
    pub target: Option<NodeId>,
    pub violation: Violation,
}

#[derive(Default)]
struct Queue {
    pending: Vec<CspViolationRecord>,
    dropped: u64,
}

pub struct DocumentCsp {
    list: RefCell<Arc<CspList>>,
    /// Shared with redirect checks, which run on the network thread.
    queue: Arc<Mutex<Queue>>,
    reported: RefCell<Vec<Violation>>,
    reported_total: Cell<u64>,
    /// Decisions per element and source text, so re-cascading never re-reports.
    inline_styles: RefCell<HashMap<NodeId, (u64, bool)>>,
    style_attributes: RefCell<HashMap<NodeId, (u64, bool)>>,
}

impl std::fmt::Debug for DocumentCsp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentCsp")
            .field("policies", &self.list.borrow().policies().len())
            .field("reported", &self.reported_total.get())
            .finish_non_exhaustive()
    }
}

impl DocumentCsp {
    pub fn new(document_url: &str) -> Self {
        Self {
            list: RefCell::new(Arc::new(CspList::new(document_url))),
            queue: Arc::new(Mutex::new(Queue::default())),
            reported: RefCell::new(Vec::new()),
            reported_total: Cell::new(0),
            inline_styles: RefCell::new(HashMap::new()),
            style_attributes: RefCell::new(HashMap::new()),
        }
    }

    pub fn list(&self) -> Arc<CspList> {
        Arc::clone(&self.list.borrow())
    }

    /// True when the document has at least one policy.
    pub fn is_active(&self) -> bool {
        !self.list.borrow().is_empty()
    }

    /// Add a `Content-Security-Policy[-Report-Only]` header value.
    pub fn add_header(&self, value: &str, disposition: Disposition) {
        let mut list = CspList::clone(&self.list.borrow());
        list.add_header(value, disposition);
        *self.list.borrow_mut() = Arc::new(list);
    }

    /// Add a `<meta http-equiv="Content-Security-Policy">` policy; it applies to what
    /// happens after it was parsed.
    pub fn add_meta(&self, content: &str) {
        let mut list = CspList::clone(&self.list.borrow());
        list.add_meta(content);
        *self.list.borrow_mut() = Arc::new(list);
    }

    /// Queue `verdict`'s violations; `true` when nothing enforced was violated.
    pub fn record(&self, target: Option<NodeId>, verdict: &Verdict) -> bool {
        push_violations(&self.queue, target, verdict);
        verdict.is_allowed()
    }

    pub fn has_pending(&self) -> bool {
        !self.queue.lock().pending.is_empty()
    }

    /// Violations to report now (oldest first). They move to the diagnostics history.
    pub fn take_pending(&self) -> Vec<CspViolationRecord> {
        let (pending, dropped) = {
            let mut q = self.queue.lock();
            (
                std::mem::take(&mut q.pending),
                std::mem::take(&mut q.dropped),
            )
        };
        if dropped > 0 {
            log::warn!("csp: {dropped} violation reports dropped (queue full)");
        }
        let mut reported = self.reported.borrow_mut();
        for r in &pending {
            if reported.len() == MAX_RECORDED {
                reported.remove(0);
            }
            reported.push(r.violation.clone());
        }
        self.reported_total
            .set(self.reported_total.get() + pending.len() as u64 + dropped);
        pending
    }

    /// Reported violations, oldest first (the most recent [`MAX_RECORDED`]).
    pub fn violations(&self) -> Vec<Violation> {
        self.reported.borrow().clone()
    }

    pub fn violation_count(&self) -> u64 {
        self.reported_total.get()
    }

    /// `upgrade-insecure-requests`: the `https` URL to fetch instead of `url`, if any.
    pub fn upgrade(&self, url: &Url) -> Option<Url> {
        if self.list.borrow().upgrades_insecure_requests() {
            upgrade_url(url)
        } else {
            None
        }
    }

    /// Pre-request check of a subresource or `fetch()`; `true` when it may be fetched.
    pub fn check_request(
        &self,
        directive: FetchDirective,
        url: &CspUrl,
        nonce: Option<&str>,
        parser_inserted: bool,
        target: Option<NodeId>,
    ) -> bool {
        let list = self.list();
        if list.is_empty() {
            return true;
        }
        let info = RequestInfo {
            nonce,
            parser_inserted,
            redirected: false,
        };
        self.record(target, &list.check_request(directive, url, info))
    }

    /// The same check for every redirect target of the request, against the policies in
    /// force now. `None` when the document has no policy.
    pub fn redirect_check(
        &self,
        directive: FetchDirective,
        nonce: Option<String>,
        parser_inserted: bool,
        target: Option<NodeId>,
    ) -> Option<RedirectCheck> {
        let list = self.list();
        if list.is_empty() {
            return None;
        }
        let queue = Arc::clone(&self.queue);
        Some(RedirectCheck::new(move |url: &Url| {
            let info = RequestInfo {
                nonce: nonce.as_deref(),
                parser_inserted,
                redirected: true,
            };
            let verdict = list.check_request(directive, &CspUrl::from_url(url), info);
            blocked_error(&queue, target, verdict)
        }))
    }

    /// `form-action` for a submission to `url`; `true` when it may navigate.
    pub fn check_form_action(&self, url: &CspUrl) -> bool {
        let list = self.list();
        list.is_empty() || self.record(None, &list.check_form_action(url, false))
    }

    /// `form-action` for the redirects of a form submission.
    pub fn form_action_redirect_check(&self) -> Option<RedirectCheck> {
        let list = self.list();
        if list
            .policies()
            .iter()
            .all(|p| p.directive("form-action").is_none())
        {
            return None;
        }
        let queue = Arc::clone(&self.queue);
        Some(RedirectCheck::new(move |url: &Url| {
            let verdict = list.check_form_action(&CspUrl::from_url(url), true);
            blocked_error(&queue, None, verdict)
        }))
    }

    /// `base-uri` for a `<base href>` resolving to `url`.
    pub fn check_base_uri(&self, url: &str, target: NodeId) -> bool {
        let list = self.list();
        if list.is_empty() {
            return true;
        }
        match CspUrl::parse(url) {
            Some(u) => self.record(Some(target), &list.check_base_uri(&u)),
            None => true,
        }
    }

    /// Inline script, event handler or style check.
    pub fn check_inline(
        &self,
        kind: InlineKind,
        target: Option<NodeId>,
        source: &str,
        nonce: Option<&str>,
    ) -> bool {
        let list = self.list();
        list.is_empty() || self.record(target, &list.check_inline(kind, source, nonce))
    }

    /// `eval` and friends.
    pub fn check_eval(&self, source: &str) -> bool {
        let list = self.list();
        list.is_empty() || self.record(None, &list.check_eval(source))
    }

    /// An inline `<style>` element's text may apply. Decided once per text.
    pub fn inline_style_allowed(&self, node: NodeId, text: &str, nonce: Option<&str>) -> bool {
        self.cached(&self.inline_styles, node, text, || {
            self.check_inline(InlineKind::Style, Some(node), text, nonce)
        })
    }

    /// A `style` attribute value may apply. Decided once per value.
    pub fn style_attribute_allowed(&self, node: NodeId, value: &str) -> bool {
        self.cached(&self.style_attributes, node, value, || {
            self.check_inline(InlineKind::StyleAttribute, Some(node), value, None)
        })
    }

    fn cached(
        &self,
        cache: &RefCell<HashMap<NodeId, (u64, bool)>>,
        node: NodeId,
        text: &str,
        decide: impl FnOnce() -> bool,
    ) -> bool {
        if !self.is_active() {
            return true;
        }
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let key = hasher.finish();
        if let Some(&(k, allowed)) = cache.borrow().get(&node) {
            if k == key {
                return allowed;
            }
        }
        let allowed = decide();
        cache.borrow_mut().insert(node, (key, allowed));
        allowed
    }
}

fn push_violations(queue: &Mutex<Queue>, target: Option<NodeId>, verdict: &Verdict) {
    if verdict.violations.is_empty() {
        return;
    }
    let mut q = queue.lock();
    for v in &verdict.violations {
        if q.pending.len() >= MAX_PENDING {
            q.dropped += 1;
            continue;
        }
        q.pending.push(CspViolationRecord {
            target,
            violation: v.clone(),
        });
    }
}

fn blocked_error(
    queue: &Mutex<Queue>,
    target: Option<NodeId>,
    verdict: Verdict,
) -> Result<(), String> {
    push_violations(queue, target, &verdict);
    match verdict
        .violations
        .iter()
        .find(|v| v.disposition == Disposition::Enforce)
    {
        Some(v) => Err(format!("csp: {}", v.message())),
        None => Ok(()),
    }
}

/// The element's `nonce` attribute, unless the element is not nonceable.
pub fn element_nonce(doc: &Document, node: NodeId) -> Option<String> {
    let nonce = doc.attr(node, "nonce")?;
    let attrs = doc.attributes(node);
    is_nonceable(
        attrs
            .iter()
            .map(|a| (a.local_name.as_str(), a.value.as_str())),
    )
    .then(|| nonce.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csp(policy: &str) -> DocumentCsp {
        let c = DocumentCsp::new("https://site.test/");
        c.add_header(policy, Disposition::Enforce);
        c
    }

    #[test]
    fn violations_are_queued_then_recorded() {
        let c = csp("img-src 'self'");
        let url = CspUrl::parse("https://evil.test/a.png").unwrap();
        assert!(!c.check_request(FetchDirective::ImgSrc, &url, None, true, Some(NodeId(3))));
        let pending = c.take_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].target, Some(NodeId(3)));
        assert!(c.take_pending().is_empty());
        assert_eq!(c.violations().len(), 1);
        assert_eq!(c.violation_count(), 1);
    }

    #[test]
    fn no_policy_allows_everything_without_reports() {
        let c = DocumentCsp::new("https://site.test/");
        assert!(!c.is_active());
        assert!(c.check_eval("1"));
        assert!(c
            .redirect_check(FetchDirective::ScriptSrcElem, None, true, None)
            .is_none());
        assert!(c.form_action_redirect_check().is_none());
        assert!(c.take_pending().is_empty());
    }

    #[test]
    fn inline_style_decisions_are_cached_per_text() {
        let c = csp("style-src 'self'");
        assert!(!c.inline_style_allowed(NodeId(1), "a{}", None));
        assert!(!c.inline_style_allowed(NodeId(1), "a{}", None));
        assert_eq!(c.take_pending().len(), 1);
        assert!(!c.inline_style_allowed(NodeId(1), "b{}", None));
        assert_eq!(c.take_pending().len(), 1);
        assert!(!c.style_attribute_allowed(NodeId(2), "color:red"));
        assert!(!c.style_attribute_allowed(NodeId(2), "color:red"));
        assert_eq!(c.take_pending().len(), 1);
    }

    #[test]
    fn redirect_checks_block_with_a_message_and_queue_the_violation() {
        let c = csp("script-src https://cdn.test");
        let check = c
            .redirect_check(FetchDirective::ScriptSrcElem, None, false, Some(NodeId(7)))
            .unwrap();
        assert!(check
            .check(&Url::parse("https://cdn.test/b.js").unwrap())
            .is_ok());
        let err = check
            .check(&Url::parse("https://evil.test/secret?x").unwrap())
            .unwrap_err();
        assert!(
            err.starts_with("csp: Refused to load the script 'https://evil.test'"),
            "{err}"
        );
        let pending = c.take_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].violation.blocked_uri, "https://evil.test");
    }

    #[test]
    fn meta_policies_copy_on_write() {
        let c = DocumentCsp::new("https://site.test/");
        let before = c.list();
        c.add_meta("img-src 'none'");
        assert!(before.is_empty());
        assert!(c.is_active());
    }

    #[test]
    fn the_pending_queue_is_bounded() {
        let c = csp("script-src 'self'");
        for _ in 0..(MAX_PENDING + 10) {
            assert!(!c.check_eval("x"));
        }
        assert_eq!(c.take_pending().len(), MAX_PENDING);
        assert_eq!(c.violation_count(), (MAX_PENDING + 10) as u64);
        assert!(c.violations().len() <= MAX_RECORDED);
    }
}
