//! Forms as seen from script and input: control state versus default attributes,
//! activation behavior, FormData, constraint validation, submit events and the planned
//! submission (the browsing context navigates it; see `axiom-browser/tests/form_submission.rs`).

use axiom_engine::forms::{FormMethod, FormSubmission};
use axiom_engine::BrowsingContext;

fn page(html: &str) -> BrowsingContext {
    let mut ctx = BrowsingContext::new(400, 300);
    ctx.page
        .load_html("http://app.test/dir/form.html", html)
        .expect("load html");
    ctx
}

fn eval(ctx: &mut BrowsingContext, src: &str) -> String {
    let js = ctx.page.js.as_mut().expect("page has js");
    js.eval(src)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

fn node(ctx: &BrowsingContext, selector: &str) -> axiom_dom::NodeId {
    ctx.page
        .document
        .borrow()
        .query_selector(selector)
        .unwrap_or_else(|| panic!("{selector}"))
}

fn take_submission(ctx: &BrowsingContext) -> Option<FormSubmission> {
    ctx.page.shared.forms.borrow_mut().take_submission()
}

#[test]
fn typing_and_clicks_change_control_state_not_default_attributes() {
    let mut ctx = page(
        "<!doctype html><form id=f>\
         <input id=q name=q value=start maxlength=8>\
         <input type=checkbox id=c name=c><label id=l for=c>Check</label>\
         <input type=radio name=r id=r1 value=a checked><input type=radio name=r id=r2 value=b>\
         <textarea id=t name=t>hi</textarea></form>\
         <script>window.log = []; \
           document.getElementById('q').addEventListener('input', function (e) { \
             log.push('input:' + e.isTrusted); }); \
           document.getElementById('c').addEventListener('change', function () { \
             log.push('change:c'); });</script>",
    );
    let q = node(&ctx, "#q");
    ctx.click_node(q);
    for _ in 0..5 {
        ctx.handle_key("Backspace");
    }
    for ch in "hello world".chars() {
        ctx.handle_key(&ch.to_string());
    }
    ctx.click_node(node(&ctx, "#r2"));
    ctx.click_node(node(&ctx, "#l"));
    let t = node(&ctx, "#t");
    ctx.click_node(t);
    ctx.handle_key("Enter");
    ctx.handle_key("x");

    assert_eq!(
        eval(
            &mut ctx,
            "var q = document.getElementById('q'), c = document.getElementById('c'); \
             [q.value, q.getAttribute('value'), q.defaultValue, c.checked, \
              c.hasAttribute('checked'), document.getElementById('r1').checked, \
              document.getElementById('r2').checked].join()"
        ),
        "hello wo,start,start,true,false,false,true",
        "maxlength caps typing; checkedness follows clicks and labels; attributes stay defaults"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "JSON.stringify(document.getElementById('t').value)"
        ),
        r#""hi\nx""#
    );
    assert_eq!(
        eval(
            &mut ctx,
            "[log.filter(function (e) { return e === 'input:true'; }).length, \
              log.indexOf('change:c') >= 0].join()"
        ),
        "13,true",
        "5 backspaces + the 8 characters maxlength allows fire trusted input events; the \
         label click fires change"
    );
}

#[test]
fn canceled_clicks_restore_checkedness_and_radio_groups() {
    let mut ctx = page(
        "<!doctype html><input type=checkbox id=c>\
         <input type=radio name=g id=a checked><input type=radio name=g id=b>\
         <script>document.addEventListener('click', function (e) { \
           window.seen = e.target.checked; e.preventDefault(); });</script>",
    );
    ctx.click_node(node(&ctx, "#c"));
    ctx.click_node(node(&ctx, "#b"));
    assert_eq!(
        eval(
            &mut ctx,
            "[window.seen, document.getElementById('c').checked, \
              document.getElementById('a').checked, document.getElementById('b').checked].join()"
        ),
        "true,false,true,false",
        "listeners see the toggled state; cancellation restores the previous one"
    );
}

#[test]
fn select_option_and_form_collections() {
    let mut ctx = page(
        "<!doctype html><form id=f><select id=s name=s><option>one<option value=2>two</select>\
         <fieldset id=fs><input name=inner></fieldset><input type=image name=img></form>\
         <input id=outside name=outside form=f>",
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var s = document.getElementById('s'); var before = [s.selectedIndex, s.value]; \
             s.value = '2'; \
             [before, s.selectedIndex, s.value, s.options[1].selected, \
              s.options[0].hasAttribute('selected'), s.length].join()"
        ),
        "0,one,1,2,true,false,2"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var f = document.getElementById('f'); \
             [f.elements.length, f.length, document.forms.length, \
              document.getElementById('outside').form === f, \
              Array.prototype.map.call(f.elements, function (e) { return e.name || e.id; }).join('|')].join()"
        ),
        "4,4,1,true,s|fs|inner|outside",
        "form.elements lists owned controls (form attribute included) and skips image inputs"
    );
}

#[test]
fn form_data_reads_the_entry_list() {
    let mut ctx = page(
        "<!doctype html><form id=f><input name=q value='a b'><input type=checkbox name=c checked>\
         <input type=checkbox name=off><select name=s multiple><option selected>x<option selected>y</select>\
         <input name=dis value=1 disabled><button id=go name=go value=1>Go</button></form>",
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var f = document.getElementById('f'); var fd = new FormData(f); \
             var withSubmitter = new FormData(f, document.getElementById('go')); \
             [fd.get('q'), fd.get('c'), fd.has('off'), fd.getAll('s').join('+'), fd.has('dis'), \
              fd.has('go'), withSubmitter.get('go')].join()"
        ),
        "a b,on,false,x+y,false,false,1"
    );
}

#[test]
fn validation_and_submit_events_gate_the_planned_submission() {
    let mut ctx = page(
        "<!doctype html><form id=f method=post action=../post enctype=text/plain>\
         <input id=a name=a required><button id=b name=go value=1>Go</button></form>\
         <script>window.log = []; var f = document.getElementById('f'); \
           document.getElementById('a').addEventListener('invalid', function (e) { \
             log.push('invalid:' + e.target.id); }); \
           f.addEventListener('submit', function (e) { \
             log.push('submit:' + (e.submitter && e.submitter.id) + ':' + e.isTrusted); \
             if (window.cancel) e.preventDefault(); });</script>",
    );
    eval(&mut ctx, "f.requestSubmit(document.getElementById('b'))");
    assert!(
        take_submission(&ctx).is_none(),
        "an invalid form is not submitted"
    );
    assert_eq!(eval(&mut ctx, "log.join()"), "invalid:a");
    assert_eq!(
        eval(
            &mut ctx,
            "var a = document.getElementById('a'); \
             [a.validity.valueMissing, a.validity.valid, a.checkValidity()].join()"
        ),
        "true,false,false"
    );

    eval(
        &mut ctx,
        "document.getElementById('a').value = 'x y'; window.cancel = true; \
         f.requestSubmit(document.getElementById('b'))",
    );
    assert!(
        take_submission(&ctx).is_none(),
        "a canceled submit event stops it"
    );

    eval(
        &mut ctx,
        "window.cancel = false; f.requestSubmit(document.getElementById('b'))",
    );
    assert_eq!(
        take_submission(&ctx),
        Some(FormSubmission {
            url: "http://app.test/post".into(),
            method: FormMethod::Post,
            body: Some((b"a=x y\r\ngo=1\r\n".to_vec(), "text/plain".into())),
            document_url: "http://app.test/dir/form.html".into(),
        })
    );
    assert_eq!(
        eval(&mut ctx, "log.join()"),
        "invalid:a,invalid:a,submit:b:true,submit:b:true",
        "checkValidity() fires invalid too"
    );

    eval(
        &mut ctx,
        "document.getElementById('a').value = ''; f.submit()",
    );
    assert!(
        take_submission(&ctx).is_some(),
        "form.submit() skips validation and the submit event"
    );
    assert_eq!(eval(&mut ctx, "log.length"), "4");

    eval(
        &mut ctx,
        "f.setAttribute('action', 'javascript:alert(1)'); f.submit()",
    );
    assert!(
        take_submission(&ctx).is_none(),
        "javascript: actions never navigate"
    );
}

#[test]
fn get_submission_replaces_the_query_and_dialog_method_closes_the_dialog() {
    let mut ctx = page(
        "<!doctype html><form id=f action='/search?old=1#frag'><input name=q value='a&b c'>\
         <button id=b formmethod=get name=go value=yes>Go</button></form>\
         <dialog open id=d><form method=dialog id=df><button id=close>x</button></form></dialog>",
    );
    eval(
        &mut ctx,
        "document.getElementById('f').requestSubmit(document.getElementById('b'))",
    );
    let submission = take_submission(&ctx).expect("GET submission");
    assert_eq!(
        submission.url,
        "http://app.test/search?q=a%26b+c&go=yes#frag"
    );
    assert_eq!(submission.method, FormMethod::Get);
    assert_eq!(submission.body, None);

    assert_eq!(
        eval(
            &mut ctx,
            "document.getElementById('df').requestSubmit(); \
             document.getElementById('d').hasAttribute('open')"
        ),
        "false"
    );
    assert!(take_submission(&ctx).is_none());
}
