use super::*;

const DOC: &str = "https://example.com/page";
// printf 'alert(1)' | openssl dgst -sha256 -binary | base64
const ALERT_SHA256: &str = "'sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI='";

fn enforced(policy: &str) -> CspList {
    let mut list = CspList::new(DOC);
    list.add_header(policy, Disposition::Enforce);
    list
}

fn url(s: &str) -> CspUrl {
    CspUrl::parse(s).unwrap()
}

fn script(list: &CspList, u: &str, info: RequestInfo<'_>) -> Verdict {
    list.check_request(FetchDirective::ScriptSrcElem, &url(u), info)
}

#[test]
fn parsing_lowercases_names_ignores_duplicates_and_splits_policies() {
    let mut list = CspList::new(DOC);
    list.add_header(
        " Script-Src 'self' ; script-src *; ;img-src data: , default-src 'none'",
        Disposition::Enforce,
    );
    assert_eq!(list.policies().len(), 2);
    let first = &list.policies()[0];
    assert_eq!(first.directives().len(), 2);
    assert_eq!(first.directive("script-src").unwrap().value, ["'self'"]);
    assert_eq!(first.directive("img-src").unwrap().text(), "img-src data:");
    assert_eq!(list.policies()[1].text(), "default-src 'none'");
}

#[test]
fn empty_or_invalid_policies_are_dropped() {
    let mut list = CspList::new(DOC);
    list.add_header("", Disposition::Enforce);
    list.add_header(" ; ; ", Disposition::Enforce);
    list.add_header("scr!pt-src 'none'", Disposition::Enforce);
    assert!(list.is_empty());
}

#[test]
fn meta_policies_drop_reporting_framing_and_sandbox_directives() {
    let mut list = CspList::new(DOC);
    list.add_meta("report-uri /r; frame-ancestors 'none'; sandbox");
    assert!(list.is_empty());
    list.add_meta("report-uri /r; img-src 'self'");
    let p = &list.policies()[0];
    assert_eq!(p.source, PolicySource::Meta);
    assert_eq!(p.disposition, Disposition::Enforce);
    assert!(p.directive("report-uri").is_none());
    assert!(p.directive("img-src").is_some());
}

#[test]
fn fetch_directives_fall_back_to_default_src() {
    let list = enforced("default-src 'self'; img-src *");
    let info = RequestInfo::default();
    assert!(list
        .check_request(FetchDirective::ImgSrc, &url("https://cdn.test/a.png"), info)
        .is_allowed());
    let v = list.check_request(
        FetchDirective::FontSrc,
        &url("https://cdn.test/a.woff"),
        info,
    );
    assert!(v.is_blocked());
    let violation = &v.violations[0];
    assert_eq!(violation.effective_directive, "font-src");
    assert_eq!(violation.violated_directive, "default-src");
    assert_eq!(violation.blocked_uri, "https://cdn.test/a.woff");
    assert!(violation
        .message()
        .contains("'font-src' was not explicitly set"));
    assert!(violation.message().contains("\"default-src 'self'\""));
}

#[test]
fn worker_and_frame_fallback_chains() {
    let list = enforced("script-src 'self'; child-src https://frames.test");
    let info = RequestInfo::default();
    // worker-src → child-src wins over script-src.
    assert!(list
        .check_request(
            FetchDirective::WorkerSrc,
            &url("https://frames.test/w.js"),
            info
        )
        .is_allowed());
    assert!(list
        .check_request(
            FetchDirective::FrameSrc,
            &url("https://example.com/f"),
            info
        )
        .is_blocked());
    // No directive in the chain: nothing to enforce.
    assert!(list
        .check_request(FetchDirective::ConnectSrc, &url("https://any.test/"), info)
        .is_allowed());
}

#[test]
fn a_request_must_satisfy_every_policy() {
    let mut list = enforced("script-src https://a.test https://b.test");
    list.add_header("script-src https://b.test", Disposition::Enforce);
    let info = RequestInfo::default();
    assert!(script(&list, "https://a.test/x.js", info).is_blocked());
    assert!(script(&list, "https://b.test/x.js", info).is_allowed());
}

#[test]
fn report_only_violations_do_not_block() {
    let mut list = CspList::new(DOC);
    list.add_header("script-src 'none'", Disposition::Report);
    let v = script(&list, "https://example.com/a.js", RequestInfo::default());
    assert!(v.is_allowed());
    assert_eq!(v.violations.len(), 1);
    assert_eq!(v.violations[0].disposition, Disposition::Report);
    assert!(v.violations[0]
        .message()
        .starts_with("[Report Only] Refused to load the script"));
}

#[test]
fn nonces_allow_scripts_and_styles() {
    let list = enforced("script-src 'nonce-r4nd0m'; style-src 'nonce-r4nd0m'");
    let with = RequestInfo {
        nonce: Some("r4nd0m"),
        ..Default::default()
    };
    let wrong = RequestInfo {
        nonce: Some("guess"),
        ..Default::default()
    };
    assert!(script(&list, "https://evil.test/a.js", with).is_allowed());
    assert!(script(&list, "https://evil.test/a.js", wrong).is_blocked());
    assert!(list
        .check_request(
            FetchDirective::StyleSrcElem,
            &url("https://x.test/a.css"),
            with
        )
        .is_allowed());
    // Nonces do not apply to images.
    let list = enforced("default-src 'nonce-r4nd0m'");
    assert!(list
        .check_request(FetchDirective::ImgSrc, &url("https://x.test/a.png"), with)
        .is_blocked());
}

#[test]
fn strict_dynamic_blocks_parser_inserted_scripts_and_trusts_the_rest() {
    let list = enforced("script-src 'nonce-n' 'strict-dynamic' https://allowed.test 'self'");
    let parser = RequestInfo {
        parser_inserted: true,
        ..Default::default()
    };
    let dynamic = RequestInfo::default();
    let nonced_parser = RequestInfo {
        nonce: Some("n"),
        parser_inserted: true,
        ..Default::default()
    };
    // Host sources and 'self' are ignored under 'strict-dynamic'.
    assert!(script(&list, "https://allowed.test/a.js", parser).is_blocked());
    assert!(script(&list, "https://example.com/a.js", parser).is_blocked());
    assert!(script(&list, "https://anything.test/a.js", dynamic).is_allowed());
    assert!(script(&list, "https://anything.test/a.js", nonced_parser).is_allowed());
    // 'strict-dynamic' only affects script-like requests.
    let list = enforced("default-src 'strict-dynamic'");
    assert!(list
        .check_request(
            FetchDirective::ImgSrc,
            &url("https://a.test/x.png"),
            dynamic
        )
        .is_blocked());
}

#[test]
fn redirected_requests_report_only_the_origin_and_skip_paths() {
    let list = enforced("script-src https://cdn.test/lib/a.js");
    let redirected = RequestInfo {
        redirected: true,
        ..Default::default()
    };
    assert!(script(&list, "https://cdn.test/other/b.js", redirected).is_allowed());
    let v = script(&list, "https://evil.test/secret/path?token=1", redirected);
    assert!(v.is_blocked());
    assert_eq!(v.violations[0].blocked_uri, "https://evil.test");
}

#[test]
fn unsafe_inline_is_ignored_when_a_nonce_or_hash_is_present() {
    let inline = enforced("script-src 'unsafe-inline'");
    assert!(inline
        .check_inline(InlineKind::Script, "x()", None)
        .is_allowed());
    assert!(inline
        .check_inline(InlineKind::ScriptAttribute, "x()", None)
        .is_allowed());

    let nonced = enforced("script-src 'unsafe-inline' 'nonce-n'");
    assert!(nonced
        .check_inline(InlineKind::Script, "x()", None)
        .is_blocked());
    assert!(nonced
        .check_inline(InlineKind::Script, "x()", Some("n"))
        .is_allowed());

    let dynamic = enforced("script-src 'unsafe-inline' 'strict-dynamic'");
    assert!(dynamic
        .check_inline(InlineKind::Script, "x()", None)
        .is_blocked());
    // 'strict-dynamic' does not neutralise 'unsafe-inline' for styles.
    let styles = enforced("default-src 'unsafe-inline' 'strict-dynamic'");
    assert!(styles
        .check_inline(InlineKind::Style, "a{}", None)
        .is_allowed());
}

#[test]
fn hashes_allow_matching_inline_elements_and_attributes_only_with_unsafe_hashes() {
    let list = enforced(&format!("script-src {ALERT_SHA256}"));
    assert!(list
        .check_inline(InlineKind::Script, "alert(1)", None)
        .is_allowed());
    assert!(list
        .check_inline(InlineKind::Script, "alert(2)", None)
        .is_blocked());
    assert!(list
        .check_inline(InlineKind::ScriptAttribute, "alert(1)", None)
        .is_blocked());
    let list = enforced(&format!("script-src 'unsafe-hashes' {ALERT_SHA256}"));
    assert!(list
        .check_inline(InlineKind::ScriptAttribute, "alert(1)", None)
        .is_allowed());
    // Nonces never apply to attributes.
    let list = enforced("script-src 'nonce-n'");
    assert!(list
        .check_inline(InlineKind::ScriptAttribute, "x()", Some("n"))
        .is_blocked());
}

#[test]
fn inline_attribute_directives_fall_back_through_script_src() {
    let list =
        enforced("script-src 'unsafe-inline'; script-src-attr 'none'; style-src-elem 'none'");
    assert!(list
        .check_inline(InlineKind::Script, "x()", None)
        .is_allowed());
    let v = list.check_inline(InlineKind::ScriptAttribute, "x()", None);
    assert!(v.is_blocked());
    assert_eq!(v.violations[0].effective_directive, "script-src-attr");
    assert!(v.violations[0]
        .message()
        .starts_with("Refused to execute inline event handler"));
    assert!(list
        .check_inline(InlineKind::Style, "a{}", None)
        .is_blocked());
    assert!(list
        .check_inline(InlineKind::StyleAttribute, "color:red", None)
        .is_allowed());
}

#[test]
fn samples_need_report_sample_and_are_truncated() {
    let list = enforced("script-src 'none' 'report-sample'");
    let long = "a".repeat(100);
    let v = list.check_inline(InlineKind::Script, &long, None);
    assert_eq!(v.violations[0].blocked_uri, "inline");
    assert_eq!(v.violations[0].sample.len(), 40);
    let list = enforced("script-src 'none'");
    assert_eq!(
        list.check_inline(InlineKind::Script, &long, None)
            .violations[0]
            .sample,
        ""
    );
}

#[test]
fn eval_needs_unsafe_eval_in_script_src_or_default_src() {
    assert!(CspList::new(DOC).check_eval("1").is_allowed());
    assert!(enforced("default-src 'self'").check_eval("1").is_blocked());
    assert!(enforced("default-src 'unsafe-eval'")
        .check_eval("1")
        .is_allowed());
    assert!(enforced("script-src 'unsafe-eval'; default-src 'none'")
        .check_eval("1")
        .is_allowed());
    // script-src-elem is not consulted for string compilation.
    assert!(enforced("script-src-elem 'none'")
        .check_eval("1")
        .is_allowed());
    let v = enforced("script-src 'self' 'report-sample'").check_eval("secret()");
    assert_eq!(v.violations[0].blocked_uri, "eval");
    assert_eq!(v.violations[0].sample, "secret()");
    assert!(v.violations[0]
        .message()
        .contains("evaluate a string as JavaScript"));
}

#[test]
fn form_action_and_base_uri_have_no_fallback() {
    let list = enforced("default-src 'none'");
    assert!(list
        .check_form_action(&url("https://evil.test/post"), false)
        .is_allowed());
    assert!(list.check_base_uri(&url("https://evil.test/")).is_allowed());

    let list = enforced("form-action 'self'; base-uri 'none'");
    assert!(list
        .check_form_action(&url("https://example.com/post"), false)
        .is_allowed());
    let v = list.check_form_action(&url("https://evil.test/post"), false);
    assert!(v.is_blocked());
    assert!(v.violations[0]
        .message()
        .contains("send form data to 'https://evil.test/post'"));
    assert!(list
        .check_base_uri(&url("https://example.com/"))
        .is_blocked());
}

#[test]
fn upgrade_insecure_requests_is_enforce_only() {
    assert!(enforced("upgrade-insecure-requests").upgrades_insecure_requests());
    let mut report = CspList::new(DOC);
    report.add_header("upgrade-insecure-requests", Disposition::Report);
    assert!(!report.upgrades_insecure_requests());
    let mut meta = CspList::new(DOC);
    meta.add_meta("upgrade-insecure-requests");
    assert!(meta.upgrades_insecure_requests());

    let up = upgrade_url(&Url::parse("http://a.test:80/x?q").unwrap()).unwrap();
    assert_eq!(up.as_str(), "https://a.test/x?q");
    let up = upgrade_url(&Url::parse("http://a.test:8080/x").unwrap()).unwrap();
    assert_eq!(up.as_str(), "https://a.test:8080/x");
    assert!(upgrade_url(&Url::parse("https://a.test/").unwrap()).is_none());
}

#[test]
fn self_is_the_document_origin() {
    let list = enforced("img-src 'self'");
    let info = RequestInfo::default();
    assert!(list
        .check_request(
            FetchDirective::ImgSrc,
            &url("https://example.com/a.png"),
            info
        )
        .is_allowed());
    assert!(list
        .check_request(
            FetchDirective::ImgSrc,
            &url("https://other.test/a.png"),
            info
        )
        .is_blocked());
    assert!(list
        .check_request(FetchDirective::ImgSrc, &url("data:image/png,x"), info)
        .is_blocked());
    // An opaque-origin document has no 'self'.
    let mut local = CspList::new("file:///C:/page.html");
    local.add_meta("img-src 'self'");
    assert!(local
        .check_request(FetchDirective::ImgSrc, &url("file:///C:/a.png"), info)
        .is_blocked());
}

#[test]
fn nonceable_elements() {
    assert!(is_nonceable([("src", "a.js"), ("nonce", "n")]));
    assert!(!is_nonceable([("src", "a.js"), ("x", "<SCRIPT")]));
    assert!(!is_nonceable([("<style", "")]));
}
