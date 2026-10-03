//! The `URL` interface (WHATWG URL Standard): parsing against a base, the getters, the
//! setters' failure rules, `searchParams` kept in sync with the query, and the statics.

use axiom_js::{JsContext, JsRuntime};

fn eval(ctx: &mut JsContext, src: &str) -> String {
    ctx.eval(src)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

fn context() -> JsContext {
    JsRuntime::new().create_context().expect("realm")
}

#[test]
fn parses_absolute_and_relative_urls_into_components() {
    let mut c = context();
    let parts = eval(
        &mut c,
        "var u = new URL('../b/c?x=1#frag', 'https://user:pw@EXAMPLE.com:8080/a/d/e');\n\
         [u.href, u.origin, u.protocol, u.username, u.password, u.host, u.hostname, u.port,\n\
          u.pathname, u.search, u.hash].join('|')",
    );
    assert_eq!(
        parts,
        "https://user:pw@example.com:8080/a/b/c?x=1#frag|https://example.com:8080|https:|user|pw|\
         example.com:8080|example.com|8080|/a/b/c|?x=1|#frag"
    );
    assert_eq!(
        eval(&mut c, "new URL('https://bücher.example/').hostname"),
        "xn--bcher-kva.example"
    );
    assert_eq!(eval(&mut c, "new URL('HTTP://h:80/').href"), "http://h/");
    assert_eq!(eval(&mut c, "new URL('data:text/plain,hi').origin"), "null");
    assert_eq!(
        eval(&mut c, "String(new URL('https://a.test/p'))"),
        "https://a.test/p"
    );
    assert_eq!(
        eval(&mut c, "JSON.stringify({ u: new URL('https://a.test') })"),
        r#"{"u":"https://a.test/"}"#
    );
}

#[test]
fn invalid_input_throws_and_the_statics_do_not() {
    let mut c = context();
    assert_eq!(
        eval(
            &mut c,
            "try { new URL('nope'); 'parsed' } catch (e) { e.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        eval(
            &mut c,
            "try { new URL('/x', 'not a base'); 'parsed' } catch (e) { e.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        eval(
            &mut c,
            "[URL.canParse('nope'), URL.canParse('/x', 'https://a.test'), URL.parse('nope')].join()"
        ),
        "false,true,"
    );
    assert_eq!(eval(&mut c, "typeof webkitURL"), "function");
}

#[test]
fn setters_follow_the_standard() {
    let mut c = context();
    let result = eval(
        &mut c,
        "var u = new URL('https://a.test/p?q#h');\n\
         u.pathname = 'x y'; u.search = 'k=v'; u.hash = 'top'; u.port = '8443';\n\
         u.hostname = 'b.test'; u.username = 'me';\n\
         var r = [u.href];\n\
         u.port = 'bad'; u.protocol = 'foo:';\n\
         r.push(u.port, u.protocol);\n\
         u.protocol = 'http:'; u.port = '80';\n\
         r.push(u.href);\n\
         r.join('|')",
    );
    assert_eq!(
        result,
        "https://me@b.test:8443/x%20y?k=v#top|8443|https:|http://me@b.test/x%20y?k=v#top"
    );
    assert_eq!(
        eval(
            &mut c,
            "try { new URL('https://a.test').href = 'nope'; 'set' } catch (e) { e.name }"
        ),
        "TypeError"
    );
}

#[test]
fn search_params_stay_in_sync_with_the_query() {
    let mut c = context();
    let result = eval(
        &mut c,
        "var u = new URL('https://a.test/?a=1&b=2');\n\
         var p = u.searchParams;\n\
         var r = [p.get('b')];\n\
         p.append('c', 'x y'); r.push(u.search);\n\
         p.delete('a'); r.push(u.href);\n\
         u.search = '?z=9'; r.push(p.get('z'), p.has('b'));\n\
         p.delete('z'); r.push(u.href);\n\
         r.push(u.searchParams === p);\n\
         r.join('|')",
    );
    assert_eq!(
        result,
        "2|?a=1&b=2&c=x+y|https://a.test/?b=2&c=x+y|9|false|https://a.test/|true"
    );
}
