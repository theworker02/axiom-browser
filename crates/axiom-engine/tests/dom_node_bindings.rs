//! Node tree navigation and removal as seen from script (the surface testharness.js and
//! the dom/nodes WPT tests lean on).

use axiom_engine::BrowsingContext;

fn page(html: &str) -> BrowsingContext {
    let mut ctx = BrowsingContext::new(400, 300);
    ctx.page.load_html("about:nodes", html).expect("load html");
    ctx
}

fn eval(ctx: &mut BrowsingContext, src: &str) -> String {
    let js = ctx.page.js.as_mut().expect("page has js");
    js.eval(src)
        .map(|v| v.display)
        .unwrap_or_else(|e| format!("<{e}>"))
}

#[test]
fn navigation_reaches_parser_implied_elements() {
    let mut ctx = page("<!doctype html><p id=a>x</p><p id=b>y</p>");
    assert_eq!(eval(&mut ctx, "document.body.localName"), "body");
    assert_eq!(
        eval(
            &mut ctx,
            "document.body.parentNode === document.documentElement"
        ),
        "true"
    );
    assert_eq!(
        eval(&mut ctx, "document.documentElement.parentNode === document"),
        "true"
    );
    assert_eq!(eval(&mut ctx, "document.body.childNodes.length"), "2");
    assert_eq!(
        eval(
            &mut ctx,
            "document.getElementById('a').nextSibling === document.getElementById('b')"
        ),
        "true"
    );
}

#[test]
fn remove_child_detaches_and_rejects_non_children() {
    let mut ctx = page("<!doctype html><p id=a>x</p><p id=b>y</p>");
    assert_eq!(
        eval(
            &mut ctx,
            "var a = document.getElementById('a'); \
             document.body.removeChild(a) === a && a.parentNode === null"
        ),
        "true"
    );
    assert_eq!(eval(&mut ctx, "document.body.childNodes.length"), "1");
    assert_eq!(
        eval(
            &mut ctx,
            "try { document.body.removeChild(a); 'no throw' } catch (e) { e.name }"
        ),
        "NotFoundError"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "try { document.body.removeChild(null); 'no throw' } catch (e) { e.name }"
        ),
        "TypeError"
    );
}

#[test]
fn remove_detaches_and_is_a_no_op_without_a_parent() {
    let mut ctx = page("<!doctype html><p id=b>y</p>");
    assert_eq!(
        eval(
            &mut ctx,
            "var b = document.getElementById('b'); b.remove(); b.remove(); \
             b.parentNode === null && !document.body.hasChildNodes()"
        ),
        "true"
    );
}

#[test]
fn insert_before_enforces_hierarchy_rules() {
    let mut ctx = page("<!doctype html><div id=d><p id=p></p></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); \
             try { d.appendChild(document.documentElement); 'no throw' } catch (e) { e.name }"
        ),
        "HierarchyRequestError"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "try { d.insertBefore(document.createElement('i'), document.body); 'no throw' } \
             catch (e) { e.name }"
        ),
        "NotFoundError"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "try { document.createTextNode('t').appendChild(d); 'no throw' } catch (e) { e.name }"
        ),
        "HierarchyRequestError"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var f = document.createDocumentFragment(); \
             f.append('a', document.createElement('b')); \
             d.insertBefore(f, document.getElementById('p')); \
             [f.childNodes.length, d.firstChild.data, d.childNodes[1].localName].join()"
        ),
        "0,a,b"
    );
}

#[test]
fn element_names_class_list_and_live_collections() {
    let mut ctx = page("<!doctype html><div id=d class='a b'></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var svg = document.createElementNS('http://www.w3.org/2000/svg', 's:rect'); \
             [svg.prefix, svg.localName, svg.tagName, svg.namespaceURI].join()"
        ),
        "s,rect,s:rect,http://www.w3.org/2000/svg"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "try { document.createElement('1x'); 'no throw' } catch (e) { e.name }"
        ),
        "InvalidCharacterError"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); d.classList.add('c'); d.classList.remove('a'); \
             [d.classList.toggle('b'), d.className, d.classList.contains('c')].join()"
        ),
        "false,c,true"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var live = document.getElementsByTagName('span'), before = live.length; \
             d.appendChild(document.createElement('span')); \
             var fixed = document.querySelectorAll('span'); \
             d.appendChild(document.createElement('span')); \
             [before, live.length, fixed.length].join()"
        ),
        "0,2,1"
    );
}

#[test]
fn elements_with_ids_are_window_named_properties() {
    let mut ctx = page("<!doctype html><div id=target></div><p id=location></p>");
    assert_eq!(
        eval(
            &mut ctx,
            "[target === document.getElementById('target'), window.target.localName, \
              'target' in window, Object.prototype.hasOwnProperty.call(window, 'target'), \
              typeof missing, location === window.location].join()"
        ),
        "true,div,true,false,undefined,true"
    );
    assert_eq!(
        eval(&mut ctx, "var target = 5; target"),
        "5",
        "declarations shadow named properties"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var e = document.createElement('i'); e.id = 'later'; document.body.appendChild(e); \
             later === e"
        ),
        "true"
    );
}

#[test]
fn inner_and_outer_html_round_trip() {
    let mut ctx = page("<!doctype html><div id=d></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); \
             d.innerHTML = '<p class=x>a &amp; b<br></p><!--c--><script>1<2</script>'; \
             d.innerHTML"
        ),
        "<p class=\"x\">a &amp; b<br></p><!--c--><script>1<2</script>"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "d.querySelector('p').outerHTML = '<em>e</em>'; d.firstChild.outerHTML"
        ),
        "<em>e</em>"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "d.insertAdjacentHTML('afterbegin', '<b>1</b>'); \
             try { d.insertAdjacentHTML('nowhere', ''); } catch (e) { var err = e.name; } \
             [d.firstChild.localName, err].join()"
        ),
        "b,SyntaxError"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var t = document.createElement('template'); t.innerHTML = '<td>x</td>'; \
             [t.childNodes.length, t.content.firstChild.localName, t.innerHTML].join()"
        ),
        "0,td,<td>x</td>"
    );
}

#[test]
fn selector_engine_handles_scope_has_and_escapes() {
    let mut ctx =
        page("<!doctype html><div id=d><p class='a:b'><span></span></p><p id='1x'></p></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); \
             [d.querySelectorAll(':scope > p').length, \
              d.querySelectorAll('p:has(> span)').length, \
              d.querySelectorAll('.a\\\\:b').length, \
              d.querySelectorAll('#\\\\31 x').length, \
              d.querySelectorAll(':is(p, nope):not(:has(span))').length].join()"
        ),
        "2,1,1,1,1"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var s = d.querySelector('span'); \
             [s.closest('p') === d.firstChild, s.closest('section'), s.matches('div span')].join()"
        ),
        "true,,true"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "try { d.querySelector('p::'); 'no throw' } catch (e) { e.name }"
        ),
        "SyntaxError"
    );
}

#[test]
fn mutation_observer_records_tree_attribute_and_text_changes() {
    let mut ctx = page("<!doctype html><div id=d title=old><p>t</p></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); \
             var mo = new MutationObserver(function () {}); \
             mo.observe(d, { childList: true, attributes: true, attributeOldValue: true, \
                             characterData: true, characterDataOldValue: true, subtree: true }); \
             d.title = 'new'; \
             d.firstChild.firstChild.data = 'u'; \
             var q = document.createElement('q'); d.appendChild(q); \
             d.removeChild(d.firstChild); \
             mo.takeRecords().map(function (r) { \
               return [r.type, r.attributeName, r.oldValue, r.addedNodes.length, \
                       r.removedNodes.length].join(':'); \
             }).join(' ')"
        ),
        "attributes:title:old:0:0 characterData::t:0:0 childList:::1:0 childList:::0:1"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "mo.disconnect(); d.title = 'x'; mo.takeRecords().length"
        ),
        "0"
    );
}

#[test]
fn attr_nodes_are_live_and_keep_their_identity() {
    let mut ctx = page("<!doctype html><div id=d title=t data-x=1></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); var a = d.getAttributeNode('title'); \
             [a === d.attributes.title, a === d.attributes[1], a.ownerElement === d, \
              a.nodeType, a.name, d.attributes.length, \
              Object.getPrototypeOf(a) === Attr.prototype, a instanceof Node].join()"
        ),
        "true,true,true,2,title,3,true,true"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "a.value = 'u'; var before = d.getAttribute('title'); \
             d.setAttribute('title', 'v'); [before, a.value, a.textContent].join()"
        ),
        "u,v,v"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "d.removeAttribute('title'); \
             [a.ownerElement, a.value, d.hasAttribute('title'), d.attributes.length].join()"
        ),
        ",v,false,2"
    );
}

#[test]
fn set_and_remove_attribute_node_follow_the_spec() {
    let mut ctx = page("<!doctype html><div id=d title=old></div><p id=p></p>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); var old = d.getAttributeNode('title'); \
             var n = document.createAttribute('TITLE'); n.value = 'new'; \
             var r = d.setAttributeNode(n); \
             [r === old, old.ownerElement, n.ownerElement === d, n.name, \
              d.getAttribute('title')].join()"
        ),
        "true,,true,title,new"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var p = document.getElementById('p'); \
             try { p.setAttributeNode(n); 'no throw' } catch (e) { e.name }"
        ),
        "InUseAttributeError"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "try { p.removeAttributeNode(n); 'no throw' } catch (e) { e.name }"
        ),
        "NotFoundError"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "[d.removeAttributeNode(n) === n, n.ownerElement, d.hasAttribute('title'), \
              d.attributes.removeNamedItem === NamedNodeMap.prototype.removeNamedItem].join()"
        ),
        "true,,false,true"
    );
}

#[test]
fn create_attribute_ns_validates_and_orders_attrs() {
    let mut ctx = page("<!doctype html><div id=d></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var errs = []; \
             [['http://x', 'a b'], [null, 'p:a'], ['http://x', 'xmlns:a'], ['http://x', 'a:']] \
               .forEach(function (c) { \
                 try { document.createAttributeNS(c[0], c[1]); errs.push('ok') } \
                 catch (e) { errs.push(e.name) } }); errs.join()"
        ),
        "InvalidCharacterError,NamespaceError,NamespaceError,InvalidCharacterError"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var a = document.createAttributeNS('http://x', 'p:loc'); \
             [a.namespaceURI, a.prefix, a.localName, a.name, a.ownerElement].join()"
        ),
        "http://x,p,loc,p:loc,"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); d.setAttribute('a', '1'); d.setAttribute('b', '2'); \
             var x = d.attributes[0], y = d.attributes[1]; \
             [x.compareDocumentPosition(y), y.compareDocumentPosition(x), \
              d.compareDocumentPosition(x), x.compareDocumentPosition(d)].join()"
        ),
        "36,34,20,10"
    );
}

#[test]
fn ranges_track_tree_and_character_data_mutations() {
    let mut ctx = page("<!doctype html><div id=d>alpha<b>bravo</b>charlie</div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'); var r = document.createRange(); \
             r.setStart(d.firstChild, 2); r.setEnd(d.lastChild, 3); \
             [r.toString(), r.commonAncestorContainer === d, r.collapsed].join('|')"
        ),
        "phabravocha|true|false"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var before = document.createTextNode('x'); d.insertBefore(before, d.firstChild); \
             [r.startContainer.data, r.startOffset, r.endContainer.data, r.endOffset].join('|')"
        ),
        "alpha|2|charlie|3"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var q = document.createRange(); q.setStart(d, 1); q.setEnd(d, 3); \
             [r.compareBoundaryPoints(Range.START_TO_START, q), \
              r.compareBoundaryPoints(Range.START_TO_END, q), \
              r.compareBoundaryPoints(Range.END_TO_END, q), \
              r.compareBoundaryPoints(Range.END_TO_START, q)].join('|')"
        ),
        "1|1|1|-1"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "r.setStart(d.firstChild.nextSibling, 2); \
             d.firstChild.nextSibling.insertData(0, 'ZZ'); \
             [r.startContainer.data, r.startOffset].join('|')"
        ),
        "ZZalpha|4"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var gone = d.querySelector('b'); r.selectNode(gone); d.removeChild(gone); \
             [r.startContainer === d, r.startOffset, r.endOffset, r.collapsed].join('|')"
        ),
        "true|2|2|true"
    );
}

#[test]
fn ranges_clone_extract_and_selection_are_backed_by_the_dom() {
    let mut ctx = page("<!doctype html><div id=d><em>a</em><strong>b</strong></div>");
    assert_eq!(
        eval(
            &mut ctx,
            "var d = document.getElementById('d'), r = new Range(); \
             r.selectNodeContents(d); var f = r.cloneContents(); \
             [f.childNodes.length, f.firstChild.localName, f.lastChild.localName, r.toString()].join('|')"
        ),
        "2|em|strong|ab"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var b = d.lastChild; r.selectNode(b); var x = r.extractContents(); \
             [x.firstChild.localName, d.childNodes.length, r.collapsed, r.startContainer === d].join('|')"
        ),
        "strong|1|true|true"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "getSelection().selectAllChildren(d); \
             [document.getSelection().rangeCount, getSelection().toString(), getSelection().type].join('|')"
        ),
        "1|a|Range"
    );
}

#[test]
fn additional_documents_and_dom_parser_are_inert_and_namespace_aware() {
    let mut ctx = page("<!doctype html><p>live</p>");
    assert_eq!(
        eval(
            &mut ctx,
            "var extra = document.implementation.createHTMLDocument('inert'); \
             [extra.title, extra.URL, extra.body.localName, extra.defaultView, \
              extra !== document].join('|')"
        ),
        "inert|about:blank|body||true"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var xml = new DOMParser().parseFromString(\"<svg xmlns='http://www.w3.org/2000/svg'><g/></svg>\", 'image/svg+xml'); \
             [xml instanceof XMLDocument, xml.contentType, xml.documentElement.namespaceURI, \
              xml.documentElement.firstChild.localName].join('|')"
        ),
        "true|image/svg+xml|http://www.w3.org/2000/svg|g"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var invalid = new DOMParser().parseFromString('<a><b></a>', 'application/xml'); \
             [invalid.documentElement.localName, invalid.documentElement.namespaceURI].join('|')"
        ),
        "parsererror|http://www.mozilla.org/newlayout/xml/parsererror.xml"
    );
    assert_eq!(
        eval(
            &mut ctx,
            "var xmlDoc = document.implementation.createDocument(null, 'root', null); \
             var c = xmlDoc.createCDATASection('x<y'); xmlDoc.documentElement.appendChild(c); \
             [c.nodeType, c.data, xmlDoc.documentElement.textContent].join('|')"
        ),
        "4|x<y|x<y"
    );
}
