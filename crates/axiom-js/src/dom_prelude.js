// DOM core (DOM Standard §4): the interface hierarchy (EventTarget, Node, Document,
// DocumentType, DocumentFragment, Element with HTML / SVG / MathML interfaces,
// CharacterData, Text, Comment, ProcessingInstruction), node wrappers that get the
// prototype of their node type, node factories and constructors, checked tree mutation,
// ParentNode / ChildNode, character data, namespaced attributes, DOMTokenList (classList),
// live HTMLCollection / NodeList, Attr / NamedNodeMap, MutationObserver,
// document.implementation, and shadow trees (attachShadow, ShadowRoot, slots; no
// slotchange event yet).
// The realm's document is the only live one. new Document(), createHTMLDocument(),
// createDocument() and DOMParser make inert documents: Document nodes in the same arena
// that are never connected, so nothing in them is styled, loaded or run.
(function (global) {
  'use strict';
  var doc = global.document;
  // Inert documents: document id -> { kind: 'html' | 'xml' | 'document', contentType,
  // url, quirks }. A detached tree belongs to the document that created (or last held)
  // its root: ownerOf maps such roots to their document id.
  var docInfo = Object.create(null);
  var otherDocCount = 0;
  var ownerOf = Object.create(null);
  var HTML_NS = 'http://www.w3.org/1999/xhtml';
  var SVG_NS = 'http://www.w3.org/2000/svg';
  var MATHML_NS = 'http://www.w3.org/1998/Math/MathML';
  var XML_NS = 'http://www.w3.org/XML/1998/namespace';
  var XMLNS_NS = 'http://www.w3.org/2000/xmlns/';

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------
  function docId() { return __axiom_documentRoot() | 0; }
  function idOf(node) { return node === doc ? docId() : node.__id | 0; }
  function wrap(id) {
    id = id | 0;
    return id < 0 ? null : new ElementRef(id);
  }
  function typeOf(node) { return node === doc ? 9 : __axiom_nodeType(node.__id) | 0; }
  function typeOfId(id) { return __axiom_nodeType(id) | 0; }
  function parentOf(id) { return __axiom_parentNode(id) | 0; }
  function childIds(id) { return __axiom_childNodes(id); }
  // A shadow root's host id; -1 for any other node.
  function hostOf(id) { return __axiom_shadowHost(id) | 0; }
  function textOf(id) { return String(__axiom_getText(id)); }
  function asciiLower(s) { return String(s).replace(/[A-Z]+/g, function (m) { return m.toLowerCase(); }); }
  function asciiUpper(s) { return String(s).replace(/[a-z]+/g, function (m) { return m.toUpperCase(); }); }
  function domError(name, message) { return new DOMException(message || name, name); }
  function check(result) { if (result !== null && result !== undefined) throw domError(String(result)); }
  function isNode(v) { return v instanceof Node; }
  function requireNode(v, where) {
    if (!isNode(v)) throw new TypeError("Failed to execute '" + where + "': parameter is not of type 'Node'.");
    return v;
  }
  function requireArgs(args, n, where) {
    if (args.length < n) {
      throw new TypeError("Failed to execute '" + where + "': " + n + ' argument(s) required, but only ' +
        args.length + ' present.');
    }
  }
  function optNamespace(ns) { return ns === null || ns === undefined ? null : String(ns); }
  // A `DOMString?` attribute value set as a string: null and undefined are empty.
  function nullableString(v) { return v === null || v === undefined ? '' : String(v); }
  // The live realm document is not stored in `docInfo` (that table only owns inert
  // documents), but it can itself be XML/XHTML. Ask the host rather than assuming
  // every browsing context is an HTML document.
  function liveDocumentIsHtml() {
    return String(__axiom_documentInfo('contentType')).toLowerCase() === 'text/html';
  }
  // Whether node `id` belongs to an HTML document, where HTML element and attribute
  // names are ASCII case-insensitive.
  function inHtmlDocument(id) {
    if (otherDocCount === 0) return liveDocumentIsHtml();
    var info = docInfo[documentOf(id)];
    return info ? info.kind === 'html' : liveDocumentIsHtml();
  }
  function isHtmlDocument(d) {
    if (d === doc) return liveDocumentIsHtml();
    var info = docInfo[d.__id | 0];
    return !info || info.kind === 'html';
  }
  function getter(obj, name, get, set) {
    Object.defineProperty(obj, name, { get: get, set: set, enumerable: true, configurable: true });
  }
  function method(obj, name, fn) {
    Object.defineProperty(obj, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  function onEach(protos, name, fn) { protos.forEach(function (p) { method(p, name, fn); }); }
  function getterOnEach(protos, name, get, set) { protos.forEach(function (p) { getter(p, name, get, set); }); }

  var VALID_ELEMENT_NAME = /^(?:[A-Za-z][^\0\t\n\f\r \/>]*|[:_\u0080-\uFFFF][A-Za-z0-9\-.:_\u0080-\uFFFF]*)$/;
  var INVALID_ATTRIBUTE_CHAR = /[\0\t\n\f\r \/=>]/;
  var INVALID_DOCTYPE_CHAR = /[\0\t\n\f\r >]/;
  var XML_NAME_START = ':A-Z_a-z\u00C0-\u00D6\u00D8-\u00F6\u00F8-\u02FF\u0370-\u037D\u037F-\u1FFF' +
    '\u200C-\u200D\u2070-\u218F\u2C00-\u2FEF\u3001-\uD7FF\uF900-\uFDCF\uFDF0-\uFFFD';
  var XML_NAME = new RegExp('^[' + XML_NAME_START + '][' + XML_NAME_START +
    '\\-.0-9\u00B7\u0300-\u036F\u203F-\u2040]*$');

  // ---------------------------------------------------------------------------
  // Interfaces
  // ---------------------------------------------------------------------------
  function illegal() { throw new TypeError('Illegal constructor'); }
  function expose(name, F) {
    Object.defineProperty(global, name, { value: F, writable: true, configurable: true });
  }
  function iface(name, parent, ctor, proto) {
    var F = ctor || function () { illegal(); };
    Object.defineProperty(F, 'name', { value: name, configurable: true });
    F.prototype = proto || Object.create(parent ? parent.prototype : Object.prototype);
    Object.defineProperty(F.prototype, 'constructor', { value: F, writable: true, configurable: true });
    if (typeof Symbol === 'function' && Symbol.toStringTag) {
      Object.defineProperty(F.prototype, Symbol.toStringTag, { value: name, configurable: true });
    }
    if (parent) Object.setPrototypeOf(F, parent);
    expose(name, F);
    return F;
  }

  var EventTarget = global.EventTarget;
  // Node wrappers are ElementRef objects, so Node.prototype is ElementRef.prototype.
  Object.setPrototypeOf(ElementRef.prototype, EventTarget.prototype);
  var Node = iface('Node', EventTarget, null, ElementRef.prototype);
  var NP = Node.prototype;

  var NODE_CONSTANTS = {
    ELEMENT_NODE: 1, ATTRIBUTE_NODE: 2, TEXT_NODE: 3, CDATA_SECTION_NODE: 4,
    ENTITY_REFERENCE_NODE: 5, ENTITY_NODE: 6, PROCESSING_INSTRUCTION_NODE: 7, COMMENT_NODE: 8,
    DOCUMENT_NODE: 9, DOCUMENT_TYPE_NODE: 10, DOCUMENT_FRAGMENT_NODE: 11, NOTATION_NODE: 12,
    DOCUMENT_POSITION_DISCONNECTED: 1, DOCUMENT_POSITION_PRECEDING: 2,
    DOCUMENT_POSITION_FOLLOWING: 4, DOCUMENT_POSITION_CONTAINS: 8,
    DOCUMENT_POSITION_CONTAINED_BY: 16, DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC: 32
  };
  Object.keys(NODE_CONSTANTS).forEach(function (k) {
    Object.defineProperty(Node, k, { value: NODE_CONSTANTS[k], enumerable: true });
    Object.defineProperty(NP, k, { value: NODE_CONSTANTS[k], enumerable: true });
  });

  var Document = iface('Document', Node, function Document() {
    if (!(this instanceof Document)) throw new TypeError("Constructor Document requires 'new'");
    return newDocument('document', 'application/xml', 'about:blank');
  });
  var HTMLDocument = iface('HTMLDocument', Document);
  var XMLDocument = iface('XMLDocument', Document);
  var DocumentType = iface('DocumentType', Node);
  var DocumentFragment = iface('DocumentFragment', Node, function DocumentFragment() {
    if (!(this instanceof DocumentFragment)) throw new TypeError("Constructor DocumentFragment requires 'new'");
    return wrap(__axiom_createDocumentFragment());
  });
  var ShadowRoot = iface('ShadowRoot', DocumentFragment);
  var CharacterData = iface('CharacterData', Node);
  var Text = iface('Text', CharacterData, function Text(data) {
    if (!(this instanceof Text)) throw new TypeError("Constructor Text requires 'new'");
    return wrap(__axiom_createTextNode(data === undefined ? '' : String(data)));
  });
  iface('CDATASection', Text);
  var Comment = iface('Comment', CharacterData, function Comment(data) {
    if (!(this instanceof Comment)) throw new TypeError("Constructor Comment requires 'new'");
    return wrap(__axiom_createComment(data === undefined ? '' : String(data)));
  });
  var ProcessingInstruction = iface('ProcessingInstruction', CharacterData);
  var Attr = iface('Attr', Node);
  var Element = iface('Element', Node);
  var HTMLElement = iface('HTMLElement', Element);
  var HTMLUnknownElement = iface('HTMLUnknownElement', HTMLElement);
  var HTMLMediaElement = iface('HTMLMediaElement', HTMLElement);
  var SVGElement = iface('SVGElement', Element);
  var MathMLElement = iface('MathMLElement', Element);
  var HTMLCollection = iface('HTMLCollection', null);
  var NodeList = iface('NodeList', null);
  var NamedNodeMap = iface('NamedNodeMap', null);
  var DOMTokenList = iface('DOMTokenList', null);
  var DOMImplementation = iface('DOMImplementation', null);

  // ---------------------------------------------------------------------------
  // Range (DOM §5).  A Range is deliberately implemented over the public DOM
  // primitives rather than as a second tree model: boundaries therefore remain
  // meaningful for inert documents as well as the page document.  The mutation
  // shims below keep its boundary points live.
  // ---------------------------------------------------------------------------
  var RANGE = typeof Symbol === 'function' ? Symbol('range') : '__axiom_range';
  var liveRanges = [];
  function rangeState(range) {
    if (!(range instanceof Range)) throw new TypeError('Illegal invocation');
    return range[RANGE];
  }
  function boundaryLength(id) {
    var t = typeOfId(id);
    return isCharacterData(t) ? textOf(id).length : childIds(id).length;
  }
  function boundaryRoot(id) { return rootOf(id); }
  function checkBoundary(node, offset, where) {
    requireNode(node, where);
    if (isAttr(node) || typeOf(node) === 10) throw domError('InvalidNodeTypeError', 'The node is not a valid Range boundary container.');
    offset = Number(offset);
    if (!isFinite(offset) || offset < 0 || Math.floor(offset) !== offset || offset > boundaryLength(idOf(node))) {
      throw domError('IndexSizeError', 'The offset is outside the boundary container.');
    }
    return { id: idOf(node), offset: offset };
  }
  function childIndex(parent, child) { return childIds(parent).indexOf(child); }
  // -1, 0, 1 for before, equal and after.  Both points must be in one tree.
  function comparePoints(aid, ao, bid, bo) {
    if (aid === bid) return ao < bo ? -1 : ao > bo ? 1 : 0;
    var a = ancestry(aid), b = ancestry(bid);
    if (a[0] !== b[0]) throw domError('WrongDocumentError', 'The boundary points are in different documents.');
    var i = 0;
    while (i < a.length && i < b.length && a[i] === b[i]) i++;
    if (i === a.length) {
      var childA = b[i];
      return ao <= childIndex(aid, childA) ? -1 : 1;
    }
    if (i === b.length) {
      var childB = a[i];
      return bo <= childIndex(bid, childB) ? 1 : -1;
    }
    var parent = a[i - 1];
    return childIndex(parent, a[i]) < childIndex(parent, b[i]) ? -1 : 1;
  }
  function rangeOrder(st) { return comparePoints(st.sc, st.so, st.ec, st.eo); }
  function setStartState(st, point) {
    if (boundaryRoot(point.id) !== boundaryRoot(st.ec) || comparePoints(point.id, point.offset, st.ec, st.eo) > 0) {
      st.ec = point.id; st.eo = point.offset;
    }
    st.sc = point.id; st.so = point.offset;
  }
  function setEndState(st, point) {
    if (boundaryRoot(point.id) !== boundaryRoot(st.sc) || comparePoints(st.sc, st.so, point.id, point.offset) > 0) {
      st.sc = point.id; st.so = point.offset;
    }
    st.ec = point.id; st.eo = point.offset;
  }
  function isDescendantOrSelf(id, ancestor) {
    for (; id >= 0; id = parentOf(id)) if (id === ancestor) return true;
    return false;
  }
  function rangeInserted(parent, index, count) {
    liveRanges.forEach(function (r) {
      var s = r[RANGE]; if (s.detached) return;
      if (s.sc === parent && s.so > index) s.so += count;
      if (s.ec === parent && s.eo > index) s.eo += count;
    });
  }
  function rangeRemoved(parent, child, index) {
    liveRanges.forEach(function (r) {
      var s = r[RANGE]; if (s.detached) return;
      if (isDescendantOrSelf(s.sc, child)) { s.sc = parent; s.so = index; }
      else if (s.sc === parent && s.so > index) s.so--;
      if (isDescendantOrSelf(s.ec, child)) { s.ec = parent; s.eo = index; }
      else if (s.ec === parent && s.eo > index) s.eo--;
    });
  }
  function rangeTextReplaced(id, oldLength, newLength) {
    liveRanges.forEach(function (r) {
      var s = r[RANGE]; if (s.detached) return;
      if (s.sc === id && s.so > newLength) s.so = newLength;
      if (s.ec === id && s.eo > newLength) s.eo = newLength;
    });
  }
  function rangeDataReplaced(id, offset, count, dataLength) {
    var end = offset + count;
    function adjust(s, container, offsetName) {
      if (s[container] !== id) return;
      if (s[offsetName] > end) s[offsetName] += dataLength - count;
      else if (s[offsetName] > offset) s[offsetName] = offset + dataLength;
    }
    liveRanges.forEach(function (r) {
      var s = r[RANGE]; if (s.detached) return;
      adjust(s, 'sc', 'so'); adjust(s, 'ec', 'eo');
    });
  }
  function rangeSplitText(oldId, newId, offset) {
    liveRanges.forEach(function (r) {
      var s = r[RANGE]; if (s.detached) return;
      if (s.sc === oldId && s.so > offset) { s.sc = newId; s.so -= offset; }
      if (s.ec === oldId && s.eo > offset) { s.ec = newId; s.eo -= offset; }
    });
  }
  // Node.normalize() merges adjacent Text nodes.  Boundary points in a node being
  // merged move into the surviving node before the normal child-removal adjustment.
  function rangeMergeText(parent, survivor, removed, removedIndex, survivorLength, removedLength) {
    liveRanges.forEach(function (r) {
      var s = r[RANGE]; if (s.detached) return;
      if (s.sc === removed) { s.sc = survivor; s.so += survivorLength; }
      if (s.ec === removed) { s.ec = survivor; s.eo += survivorLength; }
      if (s.sc === parent && s.so === removedIndex) { s.sc = survivor; s.so = survivorLength; }
      if (s.ec === parent && s.eo === removedIndex) { s.ec = survivor; s.eo = survivorLength; }
      if (s.sc === parent && s.so === removedIndex + 1) { s.sc = survivor; s.so = survivorLength + removedLength; }
      if (s.ec === parent && s.eo === removedIndex + 1) { s.ec = survivor; s.eo = survivorLength + removedLength; }
    });
  }
  var Range = iface('Range', null, function Range() {
    if (!(this instanceof Range)) throw new TypeError("Constructor Range requires 'new'");
    var id = docId();
    Object.defineProperty(this, RANGE, { value: { sc: id, so: 0, ec: id, eo: 0, detached: false } });
    liveRanges.push(this);
  });
  ['START_TO_START', 'START_TO_END', 'END_TO_END', 'END_TO_START'].forEach(function (name, i) {
    Object.defineProperty(Range, name, { value: i, enumerable: true });
    Object.defineProperty(Range.prototype, name, { value: i, enumerable: true });
  });
  function newRangeFor(document) {
    var r = Object.create(Range.prototype), id = idOf(document);
    Object.defineProperty(r, RANGE, { value: { sc: id, so: 0, ec: id, eo: 0, detached: false } });
    liveRanges.push(r); return r;
  }
  function rangeText(st) {
    if (rangeOrder(st) === 0) return '';
    var out = [];
    function visit(id) {
      var t = typeOfId(id);
      if (isCharacterData(t)) {
        var len = textOf(id).length;
        if (comparePoints(id, len, st.sc, st.so) <= 0 || comparePoints(id, 0, st.ec, st.eo) >= 0) return;
        var from = id === st.sc ? st.so : 0, to = id === st.ec ? st.eo : len;
        out.push(textOf(id).substring(from, to)); return;
      }
      childIds(id).forEach(visit);
    }
    visit(boundaryRoot(st.sc)); return out.join('');
  }
  function commonAncestor(a, b) {
    var aa = ancestry(a), bb = ancestry(b), i = 0;
    while (i < aa.length && i < bb.length && aa[i] === bb[i]) i++;
    return aa[i - 1];
  }
  function fullyContained(st, id) {
    var p = parentOf(id); if (p < 0) return false;
    var i = childIndex(p, id);
    return comparePoints(st.sc, st.so, p, i) <= 0 && comparePoints(p, i + 1, st.ec, st.eo) <= 0;
  }
  function intersects(st, id) {
    var t = typeOfId(id);
    if (isCharacterData(t)) return comparePoints(id, textOf(id).length, st.sc, st.so) > 0 && comparePoints(id, 0, st.ec, st.eo) < 0;
    var p = parentOf(id); if (p < 0) return false;
    var i = childIndex(p, id);
    return comparePoints(p, i + 1, st.sc, st.so) > 0 && comparePoints(p, i, st.ec, st.eo) < 0;
  }
  function clonePartial(st, id) {
    if (!intersects(st, id)) return null;
    var t = typeOfId(id);
    if (isCharacterData(t)) {
      var len = textOf(id).length, from = id === st.sc ? st.so : 0, to = id === st.ec ? st.eo : len;
      var c = wrap(__axiom_cloneNode(id, false)); __axiom_setText(c.__id, textOf(id).substring(from, to)); return c;
    }
    if (fullyContained(st, id)) return wrap(__axiom_cloneNode(id, true));
    var copy = wrap(__axiom_cloneNode(id, false));
    childIds(id).forEach(function (c) { var part = clonePartial(st, c); if (part) check(__axiom_insertBefore(copy.__id, part.__id, -1)); });
    return copy;
  }
  function cloneContents(st) {
    var fragment = wrap(__axiom_createDocumentFragment());
    if (rangeOrder(st) === 0) return fragment;
    if (isCharacterData(typeOfId(st.sc)) && st.sc === st.ec) {
      check(__axiom_insertBefore(fragment.__id, __axiom_createTextNode(textOf(st.sc).substring(st.so, st.eo)), -1)); return fragment;
    }
    var ancestor = commonAncestor(st.sc, st.ec);
    if (isCharacterData(typeOfId(ancestor))) ancestor = parentOf(ancestor);
    childIds(ancestor).forEach(function (c) { var part = clonePartial(st, c); if (part) check(__axiom_insertBefore(fragment.__id, part.__id, -1)); });
    return fragment;
  }
  function deleteContents(st) {
    if (rangeOrder(st) === 0) return;
    if (isCharacterData(typeOfId(st.sc)) && st.sc === st.ec) {
      var d = textOf(st.sc); __axiom_setText(st.sc, d.substring(0, st.so) + d.substring(st.eo)); st.eo = st.so; return;
    }
    var collapseNode = st.sc, collapseOffset = st.so;
    function erase(id) {
      if (!intersects(st, id)) return;
      if (fullyContained(st, id)) { var p = parentOf(id); if (p >= 0) __axiom_removeChild(p, id); return; }
      if (isCharacterData(typeOfId(id))) {
        var d = textOf(id), from = id === st.sc ? st.so : 0, to = id === st.ec ? st.eo : d.length;
        __axiom_setText(id, d.substring(0, from) + d.substring(to)); return;
      }
      childIds(id).slice().forEach(erase);
    }
    erase(commonAncestor(st.sc, st.ec));
    st.sc = collapseNode; st.so = collapseOffset; st.ec = collapseNode; st.eo = collapseOffset;
  }
  function pointBefore(id) { var p = parentOf(id); return { id: p, offset: childIndex(p, id) }; }
  function pointAfter(id) { var p = parentOf(id); return { id: p, offset: childIndex(p, id) + 1 }; }
  var RP = Range.prototype;
  getter(RP, 'startContainer', function () { return wrap(rangeState(this).sc); });
  getter(RP, 'startOffset', function () { return rangeState(this).so; });
  getter(RP, 'endContainer', function () { return wrap(rangeState(this).ec); });
  getter(RP, 'endOffset', function () { return rangeState(this).eo; });
  getter(RP, 'collapsed', function () { var s = rangeState(this); return s.sc === s.ec && s.so === s.eo; });
  getter(RP, 'commonAncestorContainer', function () { var s = rangeState(this); return wrap(commonAncestor(s.sc, s.ec)); });
  method(RP, 'setStart', function (node, offset) { requireArgs(arguments, 2, 'Range.setStart'); setStartState(rangeState(this), checkBoundary(node, offset, 'Range.setStart')); });
  method(RP, 'setEnd', function (node, offset) { requireArgs(arguments, 2, 'Range.setEnd'); setEndState(rangeState(this), checkBoundary(node, offset, 'Range.setEnd')); });
  method(RP, 'setStartBefore', function (node) { requireArgs(arguments, 1, 'Range.setStartBefore'); requireNode(node, 'Range.setStartBefore'); setStartState(rangeState(this), pointBefore(idOf(node))); });
  method(RP, 'setStartAfter', function (node) { requireArgs(arguments, 1, 'Range.setStartAfter'); requireNode(node, 'Range.setStartAfter'); setStartState(rangeState(this), pointAfter(idOf(node))); });
  method(RP, 'setEndBefore', function (node) { requireArgs(arguments, 1, 'Range.setEndBefore'); requireNode(node, 'Range.setEndBefore'); setEndState(rangeState(this), pointBefore(idOf(node))); });
  method(RP, 'setEndAfter', function (node) { requireArgs(arguments, 1, 'Range.setEndAfter'); requireNode(node, 'Range.setEndAfter'); setEndState(rangeState(this), pointAfter(idOf(node))); });
  method(RP, 'collapse', function (toStart) { var s = rangeState(this); if (toStart) { s.ec = s.sc; s.eo = s.so; } else { s.sc = s.ec; s.so = s.eo; } });
  method(RP, 'selectNode', function (node) { requireArgs(arguments, 1, 'Range.selectNode'); requireNode(node, 'Range.selectNode'); var a = pointBefore(idOf(node)), b = pointAfter(idOf(node)); var s = rangeState(this); s.sc = a.id; s.so = a.offset; s.ec = b.id; s.eo = b.offset; });
  method(RP, 'selectNodeContents', function (node) { requireArgs(arguments, 1, 'Range.selectNodeContents'); requireNode(node, 'Range.selectNodeContents'); var s = rangeState(this), id = idOf(node); s.sc = id; s.so = 0; s.ec = id; s.eo = boundaryLength(id); });
  method(RP, 'compareBoundaryPoints', function (how, sourceRange) { requireArgs(arguments, 2, 'Range.compareBoundaryPoints'); var a = rangeState(this), b = rangeState(sourceRange); how = Number(how); var x, y; if (how === 0) { x = [a.sc, a.so]; y = [b.sc, b.so]; } else if (how === 1) { x = [a.ec, a.eo]; y = [b.sc, b.so]; } else if (how === 2) { x = [a.ec, a.eo]; y = [b.ec, b.eo]; } else if (how === 3) { x = [a.sc, a.so]; y = [b.ec, b.eo]; } else throw domError('NotSupportedError', 'Invalid boundary point comparison type.'); return comparePoints(x[0], x[1], y[0], y[1]); });
  method(RP, 'cloneRange', function () { var s = rangeState(this), r = newRangeFor(wrap(boundaryRoot(s.sc))), d = rangeState(r); d.sc = s.sc; d.so = s.so; d.ec = s.ec; d.eo = s.eo; return r; });
  method(RP, 'toString', function () { return rangeText(rangeState(this)); });
  method(RP, 'cloneContents', function () { return cloneContents(rangeState(this)); });
  method(RP, 'deleteContents', function () { deleteContents(rangeState(this)); });
  method(RP, 'extractContents', function () { var s = rangeState(this), f = cloneContents(s); deleteContents(s); return f; });
  method(RP, 'insertNode', function (node) { requireArgs(arguments, 1, 'Range.insertNode'); requireNode(node, 'Range.insertNode'); var s = rangeState(this), parent = s.sc, at = s.so; if (isCharacterData(typeOfId(parent))) { var text = textOf(parent), suffix = wrap(__axiom_createTextNode(text.substring(at))), old = parent; var p = parentOf(old); var next = sibling(wrap(old), 1); __axiom_setText(old, text.substring(0, at)); check(__axiom_insertBefore(p, suffix.__id, next === null ? -1 : idOf(next))); parent = p; at = childIndex(p, old) + 1; } var child = childIds(parent)[at]; check(__axiom_insertBefore(parent, idOf(node), child === undefined ? -1 : child)); });
  method(RP, 'surroundContents', function (node) { requireArgs(arguments, 1, 'Range.surroundContents'); requireNode(node, 'Range.surroundContents'); var s = rangeState(this), fragment = this.extractContents(); this.insertNode(node); while (fragment.firstChild) node.appendChild(fragment.firstChild); this.selectNode(node); });
  method(RP, 'createContextualFragment', function (markup) { var d = this.startContainer.ownerDocument || doc, holder = d.createElement('template'); holder.innerHTML = String(markup); var f = d.createDocumentFragment(); while (holder.content.firstChild) f.appendChild(holder.content.firstChild); return f; });
  method(RP, 'detach', function () { var s = rangeState(this); s.detached = true; var i = liveRanges.indexOf(this); if (i >= 0) liveRanges.splice(i, 1); });

  // HTML element interfaces by local name (HTML §3.2.2 and §16).
  var HTML_INTERFACES = Object.create(null);
  function htmlInterfaces(suffix, names, parent) {
    var F = global['HTML' + suffix + 'Element'] || iface('HTML' + suffix + 'Element', parent || HTMLElement);
    names.split(' ').forEach(function (n) { HTML_INTERFACES[n] = F; });
  }
  [
    ['Anchor', 'a'], ['Area', 'area'], ['Base', 'base'], ['Quote', 'blockquote q'], ['Body', 'body'],
    ['BR', 'br'], ['Button', 'button'], ['Canvas', 'canvas'], ['TableCaption', 'caption'],
    ['TableCol', 'col colgroup'], ['Data', 'data'], ['DataList', 'datalist'], ['Mod', 'del ins'],
    ['Details', 'details'], ['Dialog', 'dialog'], ['Directory', 'dir'], ['Div', 'div'], ['DList', 'dl'],
    ['Embed', 'embed'], ['FieldSet', 'fieldset'], ['Font', 'font'], ['Form', 'form'], ['Frame', 'frame'],
    ['FrameSet', 'frameset'], ['Heading', 'h1 h2 h3 h4 h5 h6'], ['Head', 'head'], ['HR', 'hr'],
    ['Html', 'html'], ['IFrame', 'iframe'], ['Image', 'img'], ['Input', 'input'], ['Label', 'label'],
    ['Legend', 'legend'], ['LI', 'li'], ['Link', 'link'], ['Map', 'map'], ['Marquee', 'marquee'],
    ['Menu', 'menu'], ['Meta', 'meta'], ['Meter', 'meter'], ['Object', 'object'], ['OList', 'ol'],
    ['OptGroup', 'optgroup'], ['Option', 'option'], ['Output', 'output'], ['Paragraph', 'p'],
    ['Param', 'param'], ['Picture', 'picture'], ['Pre', 'pre listing xmp'], ['Progress', 'progress'],
    ['Script', 'script'], ['Select', 'select'], ['SelectedContent', 'selectedcontent'], ['Slot', 'slot'],
    ['Source', 'source'], ['Span', 'span'], ['Style', 'style'], ['Table', 'table'],
    ['TableSection', 'tbody thead tfoot'], ['TableCell', 'td th'], ['Template', 'template'],
    ['TextArea', 'textarea'], ['Time', 'time'], ['Title', 'title'], ['TableRow', 'tr'],
    ['Track', 'track'], ['UList', 'ul']
  ].forEach(function (e) { htmlInterfaces(e[0], e[1]); });
  htmlInterfaces('Audio', 'audio', HTMLMediaElement);
  htmlInterfaces('Video', 'video', HTMLMediaElement);
  ('abbr address article aside b bdi bdo cite code dd dfn dt em figcaption figure footer header ' +
   'hgroup i kbd main mark nav noscript rp rt ruby s samp search section small strong sub summary ' +
   'sup u var wbr acronym basefont big center nobr noembed noframes plaintext rb rtc strike tt')
    .split(' ').forEach(function (n) { HTML_INTERFACES[n] = HTMLElement; });
  var RESERVED_CUSTOM = ['annotation-xml', 'color-profile', 'font-face', 'font-face-src',
    'font-face-uri', 'font-face-format', 'font-face-name', 'missing-glyph'];
  function isCustomElementName(n) {
    return /^[a-z][\-.0-9_a-z\u00B7\u00C0-\uFFFF]*$/.test(n) && n.indexOf('-') > 0 &&
      RESERVED_CUSTOM.indexOf(n) < 0;
  }
  function htmlInterface(local) {
    return HTML_INTERFACES[local] || (isCustomElementName(local) ? HTMLElement : HTMLUnknownElement);
  }

  // The prototype a new wrapper for node `id` gets.
  global.__axiom_protoFor = function (id) {
    switch (typeOfId(id)) {
      case 1: {
        var ns = __axiom_namespaceURI(id);
        if (ns === HTML_NS) return htmlInterface(String(__axiom_tagName(id))).prototype;
        if (ns === SVG_NS) return SVGElement.prototype;
        if (ns === MATHML_NS) return MathMLElement.prototype;
        return Element.prototype;
      }
      case 3: return Text.prototype;
      case 4: return CDATASection.prototype;
      case 7: return ProcessingInstruction.prototype;
      case 8: return Comment.prototype;
      case 9: {
        var info = docInfo[id];
        if (!info) return HTMLDocument.prototype;
        return info.kind === 'html' ? HTMLDocument.prototype :
          info.kind === 'xml' ? XMLDocument.prototype : Document.prototype;
      }
      case 10: return DocumentType.prototype;
      case 11: return hostOf(id) >= 0 ? ShadowRoot.prototype : DocumentFragment.prototype;
      default: return NP;
    }
  };

  // The document object: its node id comes from the host, its prototype is HTMLDocument.
  Object.setPrototypeOf(doc, HTMLDocument.prototype);
  Object.defineProperty(doc, '__id', { get: docId, configurable: true });
  ['getElementById', 'querySelector', 'createElement'].forEach(function (n) { delete doc[n]; });

  // ---------------------------------------------------------------------------
  // Live collections
  // ---------------------------------------------------------------------------
  var STATE = typeof Symbol === 'function' ? Symbol('collection') : '__axiom_collection';
  // Attr objects: element id -> attribute key -> Attr, so an attribute keeps one object.
  var ATTR = typeof Symbol === 'function' ? Symbol('attr') : '__axiom_attr';
  var attrCache = Object.create(null);
  var attrSerial = 0;
  function isIndex(p) {
    return typeof p === 'string' && /^(?:0|[1-9][0-9]*)$/.test(p) && Number(p) < 4294967295;
  }
  // The node ids of a collection, recomputed when the host tree version changes.
  function idsOf(target) {
    var st = target[STATE];
    if (st.fixed) return st.ids;
    var v = __axiom_domVersion();
    if (v < 0 || st.version !== v) {
      st.ids = st.compute();
      st.version = v;
    }
    return st.ids;
  }
  function namedMatch(id, name) {
    if (__axiom_getAttr(id, 'id') === name) return true;
    return __axiom_namespaceURI(id) === HTML_NS && __axiom_getAttr(id, 'name') === name;
  }
  function namedItem(target, name) {
    if (name === '') return null;
    var ids = idsOf(target);
    for (var i = 0; i < ids.length; i++) if (namedMatch(ids[i], name)) return wrap(ids[i]);
    return null;
  }
  function supportedNames(target) {
    var out = [];
    idsOf(target).forEach(function (id) {
      var a = __axiom_getAttr(id, 'id');
      if (a !== null && a !== '' && out.indexOf(a) < 0) out.push(a);
      if (__axiom_namespaceURI(id) === HTML_NS) {
        var n = __axiom_getAttr(id, 'name');
        if (n !== null && n !== '' && out.indexOf(n) < 0) out.push(n);
      }
    });
    return out;
  }
  // Indexed properties over `items(target)`, plus legacy unenumerable named properties
  // when `named` gives `{ names(t), get(t, name) }`.
  function indexedHandler(items, item, named) {
    function named_(t, p) { return named && typeof p === 'string' && !isIndex(p) && !(p in t) ? named.get(t, p) : null; }
    return {
      get: function (t, p, r) {
        if (isIndex(p)) { var list = items(t); var i = Number(p); return i < list.length ? item(list[i]) : undefined; }
        var n = named_(t, p);
        return n !== null ? n : Reflect.get(t, p, r);
      },
      has: function (t, p) {
        if (isIndex(p)) return Number(p) < items(t).length;
        return named_(t, p) !== null || Reflect.has(t, p);
      },
      ownKeys: function (t) {
        var keys = [];
        var n = items(t).length;
        for (var i = 0; i < n; i++) keys.push(String(i));
        if (named) named.names(t).forEach(function (k) { if (!isIndex(k) && !(k in t)) keys.push(k); });
        return keys.concat(Reflect.ownKeys(t));
      },
      getOwnPropertyDescriptor: function (t, p) {
        if (isIndex(p)) {
          var list = items(t);
          var i = Number(p);
          return i < list.length ? { value: item(list[i]), writable: false, enumerable: true, configurable: true } : undefined;
        }
        var n = named_(t, p);
        if (n !== null) return { value: n, writable: false, enumerable: false, configurable: true };
        return Reflect.getOwnPropertyDescriptor(t, p);
      },
      set: function (t, p, v, r) {
        if (isIndex(p) || named_(t, p) !== null) return false;
        return Reflect.set(t, p, v, r);
      },
      defineProperty: function (t, p, d) {
        if (isIndex(p) || named_(t, p) !== null) return false;
        return Reflect.defineProperty(t, p, d);
      },
      deleteProperty: function (t, p) {
        if (isIndex(p)) return Number(p) >= items(t).length;
        if (named_(t, p) !== null) return false;
        return Reflect.deleteProperty(t, p);
      }
    };
  }
  var collectionHandler = indexedHandler(idsOf, wrap, { names: supportedNames, get: namedItem });
  var nodeListHandler = indexedHandler(idsOf, wrap, false);
  function makeCollection(Iface, state) {
    var target = Object.create(Iface.prototype);
    Object.defineProperty(target, STATE, { value: state });
    return new Proxy(target, Iface === NodeList ? nodeListHandler : collectionHandler);
  }
  var liveCache = Object.create(null);
  function live(Iface, key, compute) {
    var c = liveCache[key];
    if (!c) {
      c = makeCollection(Iface, { compute: compute, version: -2, ids: [] });
      liveCache[key] = c;
    }
    return c;
  }
  // A static list never changes, so its indexed properties are plain own properties: no
  // proxy trap on every read.
  function staticNodeList(ids) {
    var list = Object.create(NodeList.prototype);
    Object.defineProperty(list, STATE, { value: { fixed: true, ids: ids } });
    for (var i = 0; i < ids.length; i++) {
      Object.defineProperty(list, i, { value: wrap(ids[i]), enumerable: true });
    }
    return list;
  }

  var HCP = HTMLCollection.prototype;
  getter(HCP, 'length', function () { return idsOf(this).length; });
  method(HCP, 'item', function (i) { var ids = idsOf(this); i = Number(i) >>> 0; return i < ids.length ? wrap(ids[i]) : null; });
  method(HCP, 'namedItem', function (name) { requireArgs(arguments, 1, 'HTMLCollection.namedItem'); return namedItem(this, String(name)); });
  var NLP = NodeList.prototype;
  getter(NLP, 'length', function () { return idsOf(this).length; });
  method(NLP, 'item', function (i) { var ids = idsOf(this); i = Number(i) >>> 0; return i < ids.length ? wrap(ids[i]) : null; });
  method(NLP, 'forEach', Array.prototype.forEach);
  method(NLP, 'entries', Array.prototype.entries);
  method(NLP, 'keys', Array.prototype.keys);
  method(NLP, 'values', Array.prototype.values);
  if (typeof Symbol === 'function' && Symbol.iterator) {
    method(NLP, Symbol.iterator, Array.prototype[Symbol.iterator]);
    method(HCP, Symbol.iterator, Array.prototype[Symbol.iterator]);
  }

  function elementIds(rootId) { return __axiom_elementsByTagName(rootId, '*'); }
  function byTagName(root, name) {
    var id = idOf(root);
    name = String(name);
    return live(HTMLCollection, 'tag|' + id + '|' + name, function () { return __axiom_elementsByTagName(id, name); });
  }
  function byTagNameNS(root, ns, local) {
    var id = idOf(root);
    ns = optNamespace(ns);
    if (ns === '') ns = null;
    local = String(local);
    return live(HTMLCollection, 'tagns|' + id + '|' + ns + '|' + local, function () {
      return elementIds(id).filter(function (e) {
        return (ns === '*' || __axiom_namespaceURI(e) === ns) &&
          (local === '*' || String(__axiom_tagName(e)) === local);
      });
    });
  }
  function classTokens(s) {
    var out = [];
    String(s).split(/[\t\n\f\r ]+/).forEach(function (t) { if (t && out.indexOf(t) < 0) out.push(t); });
    return out;
  }
  function byClassName(root, names) {
    var id = idOf(root);
    names = String(names);
    var wanted = classTokens(names);
    return live(HTMLCollection, 'class|' + id + '|' + names, function () {
      if (!wanted.length) return [];
      return elementIds(id).filter(function (e) {
        var c = __axiom_getAttr(e, 'class');
        if (c === null) return false;
        var have = classTokens(c);
        return wanted.every(function (w) { return have.indexOf(w) >= 0; });
      });
    });
  }

  // ---------------------------------------------------------------------------
  // Node
  // ---------------------------------------------------------------------------
  function qualifiedName(id) {
    var p = __axiom_prefix(id);
    var l = String(__axiom_tagName(id));
    return p === null ? l : p + ':' + l;
  }
  function isHtml(id) { return __axiom_namespaceURI(id) === HTML_NS; }
  function tagNameOf(id) { var q = qualifiedName(id); return isHtml(id) && inHtmlDocument(id) ? asciiUpper(q) : q; }
  function isCharacterData(t) { return t === 3 || t === 4 || t === 7 || t === 8; }
  function rootOf(id) { var p; while ((p = parentOf(id)) >= 0) id = p; return id; }
  // The shadow-including root: the tree root, continuing through shadow hosts.
  function shadowIncludingRootOf(id) {
    var r = rootOf(id);
    var h;
    while ((h = hostOf(r)) >= 0) r = rootOf(h);
    return r;
  }
  function ancestry(id) { var out = [id]; var p; while ((p = parentOf(id)) >= 0) { out.unshift(p); id = p; } return out; }
  function sibling(node, step) {
    if (node === doc) return null;
    var p = parentOf(node.__id);
    if (p < 0) return null;
    var ids = childIds(p);
    var j = ids.indexOf(node.__id) + step;
    return j >= 0 && j < ids.length ? wrap(ids[j]) : null;
  }

  getter(NP, 'nodeType', function () { return typeOf(this); });
  getter(NP, 'nodeName', function () {
    var id = idOf(this);
    switch (typeOfId(id)) {
      case 1: return tagNameOf(id);
      case 3: return '#text';
      case 4: return '#cdata-section';
      case 7: case 10: return String(__axiom_tagName(id));
      case 8: return '#comment';
      case 9: return '#document';
      case 11: return '#document-fragment';
      default: return '';
    }
  });
  getter(NP, 'baseURI', function () { var u = __axiom_resolveUrl(''); return u === null ? 'about:blank' : String(u); });
  getter(NP, 'isConnected', function () { return shadowIncludingRootOf(idOf(this)) === docId(); });
  getter(NP, 'ownerDocument', function () {
    if (this === doc) return null;
    if (otherDocCount === 0) return doc;
    var id = this.__id | 0;
    return docInfo[id] ? null : wrap(documentOf(id));
  });
  method(NP, 'getRootNode', function (options) {
    var composed = options !== undefined && options !== null && !!options.composed;
    var id = idOf(this);
    return wrap(composed ? shadowIncludingRootOf(id) : rootOf(id));
  });
  getter(NP, 'parentNode', function () { return this === doc ? null : wrap(parentOf(this.__id)); });
  getter(NP, 'parentElement', function () {
    if (this === doc) return null;
    var p = parentOf(this.__id);
    return p >= 0 && typeOfId(p) === 1 ? wrap(p) : null;
  });
  getter(NP, 'childNodes', function () {
    var id = idOf(this);
    return live(NodeList, 'childNodes|' + id, function () { return childIds(id); });
  });
  getter(NP, 'firstChild', function () { var ids = childIds(idOf(this)); return ids.length ? wrap(ids[0]) : null; });
  getter(NP, 'lastChild', function () { var ids = childIds(idOf(this)); return ids.length ? wrap(ids[ids.length - 1]) : null; });
  getter(NP, 'previousSibling', function () { return sibling(this, -1); });
  getter(NP, 'nextSibling', function () { return sibling(this, 1); });
  method(NP, 'hasChildNodes', function () { return childIds(idOf(this)).length > 0; });
  getter(NP, 'nodeValue', function () {
    return isCharacterData(typeOf(this)) ? textOf(this.__id) : null;
  }, function (v) {
    if (isCharacterData(typeOf(this))) __axiom_setText(this.__id, nullableString(v));
  });
  getter(NP, 'textContent', function () {
    var t = typeOf(this);
    if (t === 9 || t === 10) return null;
    return textOf(idOf(this));
  }, function (v) {
    var t = typeOf(this);
    if (t === 9 || t === 10) return;
    __axiom_setText(idOf(this), nullableString(v));
  });

  function normalizeChildren(pid) {
    var kids = childIds(pid);
    for (var i = 0; i < kids.length; i++) {
      var k = kids[i];
      if (typeOfId(k) !== 3) { normalizeChildren(k); continue; }
      var data = textOf(k);
      var j = i + 1;
      for (; j < kids.length && typeOfId(kids[j]) === 3; j++) {
        rangeMergeText(pid, k, kids[j], childIndex(pid, kids[j]), data.length, textOf(kids[j]).length);
        data += textOf(kids[j]);
        __axiom_removeChild(pid, kids[j]);
      }
      if (data.length === 0) __axiom_removeChild(pid, k);
      else if (j > i + 1) __axiom_setText(k, data);
      i = j - 1;
    }
  }
  method(NP, 'normalize', function () { normalizeChildren(idOf(this)); });

  method(NP, 'cloneNode', function (deep) {
    var src = idOf(this);
    if (typeOf(this) !== 9) {
      var clone = __axiom_cloneNode(src, !!deep) | 0;
      if (otherDocCount > 0 && clone >= 0) {
        var owner = documentOf(src);
        if (owner !== docId()) ownerOf[clone] = owner;
      }
      return wrap(clone);
    }
    // A document clone is an inert document of the same type.
    var info = docInfo[src];
    var id = __axiom_createDocument() | 0;
    var copy = info ? registerDocument(id, info.kind, info.contentType, info.url)
      : registerDocument(id, 'html', String(doc.contentType), String(doc.URL));
    copy.quirks = info ? info.quirks : doc.compatMode === 'BackCompat';
    if (deep) {
      childIds(src).forEach(function (c) { check(__axiom_insertBefore(id, __axiom_cloneNode(c, true), -1)); });
    }
    return wrap(id);
  });

  function sameAttributes(a, b) {
    var x = __axiom_attributes(a), y = __axiom_attributes(b);
    if (x.length !== y.length) return false;
    return x.every(function (p) {
      return y.some(function (q) { return p[0] === q[0] && p[2] === q[2] && p[3] === q[3]; });
    });
  }
  function equalNodes(a, b) {
    var t = typeOfId(a);
    if (t !== typeOfId(b)) return false;
    switch (t) {
      case 10: {
        var ia = __axiom_doctypeIds(a), ib = __axiom_doctypeIds(b);
        if (String(__axiom_tagName(a)) !== String(__axiom_tagName(b)) || ia[0] !== ib[0] || ia[1] !== ib[1]) return false;
        break;
      }
      case 1:
        if (__axiom_namespaceURI(a) !== __axiom_namespaceURI(b) || __axiom_prefix(a) !== __axiom_prefix(b) ||
            String(__axiom_tagName(a)) !== String(__axiom_tagName(b)) || !sameAttributes(a, b)) return false;
        break;
      case 7:
        if (String(__axiom_tagName(a)) !== String(__axiom_tagName(b)) || textOf(a) !== textOf(b)) return false;
        break;
      case 3: case 4: case 8:
        if (textOf(a) !== textOf(b)) return false;
        break;
    }
    var ka = childIds(a), kb = childIds(b);
    if (ka.length !== kb.length) return false;
    for (var i = 0; i < ka.length; i++) if (!equalNodes(ka[i], kb[i])) return false;
    return true;
  }
  method(NP, 'isEqualNode', function (other) {
    if (other === null || other === undefined) return false;
    if (isAttr(requireNode(other, 'Node.isEqualNode'))) return false;
    return equalNodes(idOf(this), idOf(other));
  });
  method(NP, 'isSameNode', function (other) { return this === other; });

  // A stable order for nodes in different trees (DOM: "implementation-specific").
  function disconnectedKey(n) {
    if (!isAttr(n)) return idOf(n);
    var st = n[ATTR];
    return st.owner >= 0 ? st.owner + 0.5 : 1e9 + st.serial;
  }
  method(NP, 'compareDocumentPosition', function (other) {
    requireArgs(arguments, 1, 'Node.compareDocumentPosition');
    requireNode(other, 'Node.compareDocumentPosition');
    if (this === other) return 0;
    var node1 = other, node2 = this, attr1 = null, attr2 = null;
    if (isAttr(node1)) { attr1 = node1; node1 = attrOwnerNode(attr1); }
    if (isAttr(node2)) {
      attr2 = node2;
      node2 = attrOwnerNode(attr2);
      if (attr1 !== null && node1 !== null && node2 === node1) {
        var list = attrObjects(idOf(node2));
        for (var k = 0; k < list.length; k++) {
          if (list[k] === attr1) return 32 | 2;
          if (list[k] === attr2) return 32 | 4;
        }
      }
    }
    if (node1 === null || node2 === null) {
      return 1 | 32 | (disconnectedKey(other) < disconnectedKey(this) ? 2 : 4);
    }
    var id1 = idOf(node1), id2 = idOf(node2);
    var a = ancestry(id1), b = ancestry(id2);
    if (a[0] !== b[0]) return 1 | 32 | (id1 < id2 ? 2 : 4);
    if ((attr1 === null && a.length < b.length && b[a.length - 1] === id1) || (id1 === id2 && attr2 !== null)) return 8 | 2;
    if ((attr2 === null && b.length < a.length && a[b.length - 1] === id2) || (id1 === id2 && attr1 !== null)) return 16 | 4;
    var i = 0;
    while (i < a.length && i < b.length && a[i] === b[i]) i++;
    if (i === a.length) return 2;
    if (i === b.length) return 4;
    var siblings = childIds(a[i - 1]);
    return siblings.indexOf(a[i]) < siblings.indexOf(b[i]) ? 2 : 4;
  });
  method(NP, 'contains', function (other) {
    if (other === null || other === undefined) return false;
    if (isAttr(requireNode(other, 'Node.contains'))) return false;
    var target = idOf(this);
    for (var n = idOf(requireNode(other, 'Node.contains')); n >= 0; n = parentOf(n)) if (n === target) return true;
    return false;
  });

  // Namespace lookup (DOM §4.4 "locate a namespace" / "locate a namespace prefix").
  function parentElementId(id) { var p = parentOf(id); return p >= 0 && typeOfId(p) === 1 ? p : -1; }
  // The document element of document node `d`.
  function rootElementOf(d) { var ids = childIds(d).filter(function (c) { return typeOfId(c) === 1; }); return ids.length ? ids[0] : -1; }
  function locateNamespace(id, prefix) {
    switch (typeOfId(id)) {
      case 1: {
        if (prefix === 'xml') return XML_NS;
        if (prefix === 'xmlns') return XMLNS_NS;
        var ns = __axiom_namespaceURI(id);
        if (ns !== null && __axiom_prefix(id) === prefix) return ns;
        var attrs = __axiom_attributes(id);
        for (var i = 0; i < attrs.length; i++) {
          var a = attrs[i];
          if (a[0] !== XMLNS_NS) continue;
          if ((a[1] === 'xmlns' && a[2] === prefix) || (prefix === null && a[1] === null && a[2] === 'xmlns')) {
            return a[3] === '' ? null : a[3];
          }
        }
        var pe = parentElementId(id);
        return pe < 0 ? null : locateNamespace(pe, prefix);
      }
      case 9: { var de = rootElementOf(id); return de < 0 ? null : locateNamespace(de, prefix); }
      case 10: case 11: return null;
      default: { var p = parentElementId(id); return p < 0 ? null : locateNamespace(p, prefix); }
    }
  }
  function locatePrefix(id, ns) {
    var p = __axiom_prefix(id);
    if (__axiom_namespaceURI(id) === ns && p !== null) return p;
    var attrs = __axiom_attributes(id);
    for (var i = 0; i < attrs.length; i++) {
      if (attrs[i][1] === 'xmlns' && attrs[i][3] === ns) return attrs[i][2];
    }
    var pe = parentElementId(id);
    return pe < 0 ? null : locatePrefix(pe, ns);
  }
  method(NP, 'lookupNamespaceURI', function (prefix) {
    prefix = optNamespace(prefix);
    return locateNamespace(idOf(this), prefix === '' ? null : prefix);
  });
  method(NP, 'isDefaultNamespace', function (ns) {
    ns = optNamespace(ns);
    return locateNamespace(idOf(this), null) === (ns === '' ? null : ns);
  });
  method(NP, 'lookupPrefix', function (ns) {
    ns = optNamespace(ns);
    if (ns === null || ns === '') return null;
    var id = idOf(this);
    switch (typeOfId(id)) {
      case 1: return locatePrefix(id, ns);
      case 9: { var de = rootElementOf(id); return de < 0 ? null : locatePrefix(de, ns); }
      case 10: case 11: return null;
      default: { var p = parentElementId(id); return p < 0 ? null : locatePrefix(p, ns); }
    }
  });

  // Attr nodes are never in a tree: the pre-insert / replace validity steps that
  // concern them, in spec order.
  function checkNotAttrs(parent, node, child) {
    if (isAttr(parent)) throw domError('HierarchyRequestError', 'An Attr node cannot have children.');
    if (child && isAttr(child)) throw domError('NotFoundError', 'The reference node is not a child of this node.');
    if (isAttr(node)) throw domError('HierarchyRequestError', 'An Attr node cannot be inserted.');
  }
  function preInsert(parent, node, childId) {
    check(__axiom_insertBefore(idOf(parent), idOf(node), childId));
    return node;
  }
  method(NP, 'insertBefore', function (node, child) {
    requireArgs(arguments, 2, 'Node.insertBefore');
    requireNode(node, 'Node.insertBefore');
    if (child !== null && child !== undefined) requireNode(child, 'Node.insertBefore');
    checkNotAttrs(this, node, child);
    var childId = child === null || child === undefined ? -1 : idOf(child);
    return preInsert(this, node, childId);
  });
  method(NP, 'appendChild', function (node) {
    requireArgs(arguments, 1, 'Node.appendChild');
    checkNotAttrs(this, requireNode(node, 'Node.appendChild'), null);
    return preInsert(this, node, -1);
  });
  method(NP, 'replaceChild', function (node, child) {
    requireArgs(arguments, 2, 'Node.replaceChild');
    requireNode(node, 'Node.replaceChild');
    requireNode(child, 'Node.replaceChild');
    checkNotAttrs(this, node, child);
    check(__axiom_replaceChild(idOf(this), idOf(node), idOf(child)));
    return child;
  });
  method(NP, 'removeChild', function (child) {
    requireArgs(arguments, 1, 'Node.removeChild');
    requireNode(child, 'Node.removeChild');
    if (!__axiom_removeChild(idOf(this), idOf(child))) {
      throw domError('NotFoundError', 'The node to be removed is not a child of this node.');
    }
    return child;
  });

  // ---------------------------------------------------------------------------
  // MutationObserver (DOM § 4.3). Script-driven mutations reach the tree through the
  // natives wrapped below, which queue the mutation records. Parser and engine mutations
  // are not observed.
  // ---------------------------------------------------------------------------
  var registry = Object.create(null);
  var pendingObservers = [];
  var notifyQueued = false;
  var observing = false;
  var suppressRecords = 0;
  var observerCount = 0;

  var MutationRecord = iface('MutationRecord', null);
  ['type', 'target', 'addedNodes', 'removedNodes', 'previousSibling', 'nextSibling',
    'attributeName', 'attributeNamespace', 'oldValue'].forEach(function (name) {
    getter(MutationRecord.prototype, name, function () { return this[STATE][name]; });
  });
  function makeRecord(type, target, name, ns, oldValue, added, removed, prev, next) {
    var r = Object.create(MutationRecord.prototype);
    Object.defineProperty(r, STATE, {
      value: {
        type: type, target: wrap(target), addedNodes: staticNodeList(added),
        removedNodes: staticNodeList(removed), previousSibling: wrap(prev), nextSibling: wrap(next),
        attributeName: name, attributeNamespace: ns, oldValue: oldValue
      }
    });
    return r;
  }

  var MutationObserver = iface('MutationObserver', null, function (callback) {
    if (!(this instanceof MutationObserver)) {
      throw new TypeError("Failed to construct 'MutationObserver': Please use the 'new' operator.");
    }
    if (typeof callback !== 'function') {
      throw new TypeError("Failed to construct 'MutationObserver': parameter 1 is not of type 'MutationCallback'.");
    }
    Object.defineProperty(this, STATE, {
      value: { callback: callback, queue: [], nodes: [], transient: [], index: observerCount++ }
    });
  });
  var MOP = MutationObserver.prototype;
  function optionalBool(v) { return v === undefined ? undefined : !!v; }
  method(MOP, 'observe', function (target, options) {
    requireArgs(arguments, 1, 'MutationObserver.observe');
    requireNode(target, 'MutationObserver.observe');
    if (options !== undefined && options !== null && typeof options !== 'object' && typeof options !== 'function') {
      throw new TypeError("Failed to execute 'observe': parameter 2 is not an object.");
    }
    options = options || {};
    var filter = options.attributeFilter;
    var o = {
      attributeFilter: filter === undefined ? undefined : Array.prototype.map.call(Array.from(filter), String),
      attributeOldValue: optionalBool(options.attributeOldValue),
      attributes: optionalBool(options.attributes),
      characterData: optionalBool(options.characterData),
      characterDataOldValue: optionalBool(options.characterDataOldValue),
      childList: !!options.childList,
      subtree: !!options.subtree
    };
    if ((o.attributeOldValue !== undefined || o.attributeFilter !== undefined) && o.attributes === undefined) o.attributes = true;
    if (o.characterDataOldValue !== undefined && o.characterData === undefined) o.characterData = true;
    function fail(msg) { throw new TypeError("Failed to execute 'observe' on 'MutationObserver': " + msg); }
    if (!o.childList && !o.attributes && !o.characterData) {
      fail("The options object must set at least one of 'attributes', 'characterData', or 'childList' to true.");
    }
    if (o.attributeOldValue && !o.attributes) fail("The options object may only set 'attributeOldValue' to true when 'attributes' is true or not present.");
    if (o.attributeFilter !== undefined && !o.attributes) fail("The options object may only set 'attributeFilter' when 'attributes' is true or not present.");
    if (o.characterDataOldValue && !o.characterData) fail("The options object may only set 'characterDataOldValue' to true when 'characterData' is true or not present.");
    var st = this[STATE];
    var id = idOf(target);
    var list = registry[id] || (registry[id] = []);
    var existing = null;
    for (var i = 0; i < list.length; i++) if (list[i].observer === this && !list[i].source) existing = list[i];
    if (existing) {
      st.transient.forEach(function (n) {
        if (registry[n]) registry[n] = registry[n].filter(function (r) { return r.source !== existing; });
      });
      existing.options = o;
    } else {
      list.push({ observer: this, options: o, source: null });
      st.nodes.push(id);
    }
    observing = true;
  });
  method(MOP, 'disconnect', function () {
    var mo = this, st = this[STATE];
    st.nodes.concat(st.transient).forEach(function (n) {
      if (registry[n]) registry[n] = registry[n].filter(function (r) { return r.observer !== mo; });
    });
    st.nodes = [];
    st.transient = [];
    st.queue = [];
  });
  method(MOP, 'takeRecords', function () {
    var st = this[STATE];
    var records = st.queue;
    st.queue = [];
    return records;
  });

  function notifyObservers() {
    notifyQueued = false;
    var set = pendingObservers.sort(function (a, b) { return a[STATE].index - b[STATE].index; });
    pendingObservers = [];
    set.forEach(function (mo) {
      var st = mo[STATE];
      var records = st.queue;
      st.queue = [];
      st.transient.forEach(function (n) {
        if (registry[n]) registry[n] = registry[n].filter(function (r) { return !(r.source && r.observer === mo); });
      });
      st.transient = [];
      if (records.length) {
        try { st.callback.call(mo, records, mo); } catch (e) { __axiom_reportError(e); }
      }
    });
  }
  function queueRecord(type, target, name, ns, oldValue, added, removed, prev, next) {
    var interested = [];
    for (var n = target, self = true; n >= 0; n = parentOf(n), self = false) {
      var list = registry[n];
      if (!list) continue;
      for (var i = 0; i < list.length; i++) {
        var o = list[i].options;
        if (!self && !o.subtree) continue;
        if (type === 'attributes' && (!o.attributes || (o.attributeFilter && (ns !== null || o.attributeFilter.indexOf(name) < 0)))) continue;
        if (type === 'characterData' && !o.characterData) continue;
        if (type === 'childList' && !o.childList) continue;
        var entry = null;
        for (var k = 0; k < interested.length; k++) if (interested[k].observer === list[i].observer) entry = interested[k];
        if (!entry) interested.push(entry = { observer: list[i].observer, old: null });
        if ((type === 'attributes' && o.attributeOldValue) || (type === 'characterData' && o.characterDataOldValue)) {
          entry.old = oldValue;
        }
      }
    }
    interested.forEach(function (e) {
      e.observer[STATE].queue.push(makeRecord(type, target, name, ns, e.old, added, removed, prev, next));
      if (pendingObservers.indexOf(e.observer) < 0) pendingObservers.push(e.observer);
    });
    if (interested.length && !notifyQueued) {
      notifyQueued = true;
      if (typeof global.queueMicrotask === 'function') global.queueMicrotask(notifyObservers);
      else Promise.resolve().then(notifyObservers);
    }
  }
  // Transient registrations keep a removed subtree observed until the next delivery.
  function addTransient(removed, parent) {
    for (var n = parent; n >= 0; n = parentOf(n)) {
      (registry[n] || []).slice().forEach(function (reg) {
        if (!reg.options.subtree) return;
        (registry[removed] || (registry[removed] = [])).push({ observer: reg.observer, options: reg.options, source: reg });
        reg.observer[STATE].transient.push(removed);
      });
    }
  }
  function childListRecord(parent, added, removed, prev, next) {
    removed.forEach(function (r) { addTransient(r, parent); });
    queueRecord('childList', parent, null, null, null, added, removed, prev, next);
  }
  function recording() { return observing && suppressRecords === 0; }
  function withoutRecords(fn) {
    suppressRecords++;
    try { return fn(); } finally { suppressRecords--; }
  }
  function siblingsIn(list, node) {
    var i = list.indexOf(node);
    return [i > 0 ? list[i - 1] : -1, i >= 0 && i + 1 < list.length ? list[i + 1] : -1];
  }
  function nextSiblingId(id) { var p = parentOf(id); return p < 0 ? -1 : siblingsIn(childIds(p), id)[1]; }
  function previousSiblingId(id) { var p = parentOf(id); return p < 0 ? -1 : siblingsIn(childIds(p), id)[0]; }
  function lastChildId(id) { var c = childIds(id); return c.length ? c[c.length - 1] : -1; }
  // Records for moving `node` into a new place, captured before the move: the fragment
  // record, or the removal from its old parent.
  function detachRecords(node) {
    if (typeOfId(node) === 11) {
      var kids = childIds(node);
      return function () { if (kids.length) childListRecord(node, [], kids, -1, -1); };
    }
    var oldParent = parentOf(node);
    if (oldParent < 0) return function () {};
    var around = siblingsIn(childIds(oldParent), node);
    return function () { childListRecord(oldParent, [], [node], around[0], around[1]); };
  }
  function insertedIds(node) { return typeOfId(node) === 11 ? childIds(node) : [node]; }

  var nativeInsertBefore = __axiom_insertBefore;
  global.__axiom_insertBefore = function (parent, node, child) {
    var addedForRange = insertedIds(node).slice();
    var oldForRange = addedForRange.map(function (id) {
      return { id: id, parent: parentOf(id), index: parentOf(id) < 0 ? -1 : childIndex(parentOf(id), id) };
    });
    if (!recording()) {
      var plain = nativeInsertBefore(parent, node, child);
      if (plain === null) {
        oldForRange.forEach(function (old) { if (old.parent >= 0) rangeRemoved(old.parent, old.id, old.index); });
        if (addedForRange.length) rangeInserted(parent, childIndex(parent, addedForRange[0]), addedForRange.length);
      }
      return plain;
    }
    var ref = child === node ? nextSiblingId(node) : child;
    var prev = ref >= 0 ? previousSiblingId(ref) : lastChildId(parent);
    var added = insertedIds(node);
    var detached = detachRecords(node);
    var err = withoutRecords(function () { return nativeInsertBefore(parent, node, child); });
    if (err !== null) return err;
    oldForRange.forEach(function (old) { if (old.parent >= 0) rangeRemoved(old.parent, old.id, old.index); });
    if (addedForRange.length) rangeInserted(parent, childIndex(parent, addedForRange[0]), addedForRange.length);
    detached();
    if (added.length) childListRecord(parent, added, [], prev, ref);
    return err;
  };
  var nativeReplaceChild = __axiom_replaceChild;
  global.__axiom_replaceChild = function (parent, node, child) {
    var addedForRange = insertedIds(node).slice();
    var oldForRange = addedForRange.map(function (id) {
      return { id: id, parent: parentOf(id), index: parentOf(id) < 0 ? -1 : childIndex(parentOf(id), id) };
    });
    var removedIndex = childIndex(parent, child);
    if (!recording()) {
      var plain = nativeReplaceChild(parent, node, child);
      if (plain === null) {
        oldForRange.forEach(function (old) { if (old.parent >= 0) rangeRemoved(old.parent, old.id, old.index); });
        if (node !== child) rangeRemoved(parent, child, removedIndex);
        if (addedForRange.length) rangeInserted(parent, childIndex(parent, addedForRange[0]), addedForRange.length);
      }
      return plain;
    }
    var ref = nextSiblingId(child);
    if (ref === node) ref = nextSiblingId(node);
    var prev = previousSiblingId(child);
    if (prev === node) prev = previousSiblingId(node);
    var added = insertedIds(node);
    // Adopting `node` removes it first; a self-replacement leaves nothing to remove.
    var detached = detachRecords(node);
    var err = withoutRecords(function () { return nativeReplaceChild(parent, node, child); });
    if (err !== null) return err;
    oldForRange.forEach(function (old) { if (old.parent >= 0) rangeRemoved(old.parent, old.id, old.index); });
    if (node !== child) rangeRemoved(parent, child, removedIndex);
    if (addedForRange.length) rangeInserted(parent, childIndex(parent, addedForRange[0]), addedForRange.length);
    detached();
    childListRecord(parent, added, node === child ? [] : [child], prev, ref);
    return err;
  };
  var nativeRemoveChild = __axiom_removeChild;
  global.__axiom_removeChild = function (parent, child) {
    var rangeIndex = childIndex(parent, child);
    if (!recording()) {
      var plain = nativeRemoveChild(parent, child);
      if (plain) rangeRemoved(parent, child, rangeIndex);
      return plain;
    }
    var around = siblingsIn(childIds(parent), child);
    var ok = nativeRemoveChild(parent, child);
    if (ok) { rangeRemoved(parent, child, rangeIndex); childListRecord(parent, [], [child], around[0], around[1]); }
    return ok;
  };
  var nativeSetText = __axiom_setText;
  global.__axiom_setText = function (id, text) {
    var t = typeOfId(id);
    if (t === 3 || t === 4 || t === 7 || t === 8) {
      var old = textOf(id);
      if (!recording()) { var plain = nativeSetText(id, text); rangeTextReplaced(id, old.length, String(text).length); return plain; }
      var r = nativeSetText(id, text);
      rangeTextReplaced(id, old.length, String(text).length);
      queueRecord('characterData', id, null, null, old, [], [], -1, -1);
      return r;
    }
    var removed = childIds(id);
    if (!recording()) {
      var plainResult = nativeSetText(id, text), plainAdded = childIds(id);
      removed.forEach(function (child, index) { rangeRemoved(id, child, index); });
      if (plainAdded.length) rangeInserted(id, 0, plainAdded.length);
      return plainResult;
    }
    var result = nativeSetText(id, text);
    var added = childIds(id);
    removed.forEach(function (child, index) { rangeRemoved(id, child, index); });
    if (added.length) rangeInserted(id, 0, added.length);
    if (removed.length || added.length) childListRecord(id, added, removed, -1, -1);
    return result;
  };
  function findAttr(id, pred) {
    var list = __axiom_attributes(id);
    for (var i = 0; i < list.length; i++) if (pred(list[i])) return list[i];
    return null;
  }
  function byQualifiedName(name) {
    return function (a) { return (a[1] ? a[1] + ':' + a[2] : a[2]) === name; };
  }
  function byNamespace(ns, local) {
    return function (a) { return a[0] === ns && a[2] === local; };
  }
  function attributeRecord(id, a, name, ns) {
    queueRecord('attributes', id, a ? a[2] : name, a ? a[0] : ns, a ? a[3] : null, [], [], -1, -1);
  }
  var nativeSetAttr = __axiom_setAttr;
  global.__axiom_setAttr = function (id, name, value) {
    if (!recording()) return nativeSetAttr(id, name, value);
    var a = findAttr(id, byQualifiedName(name));
    var r = nativeSetAttr(id, name, value);
    attributeRecord(id, a, name, null);
    return r;
  };
  var nativeRemoveAttr = __axiom_removeAttr;
  global.__axiom_removeAttr = function (id, name) {
    if (!recording()) return nativeRemoveAttr(id, name);
    var a = findAttr(id, byQualifiedName(name));
    var r = nativeRemoveAttr(id, name);
    if (a) attributeRecord(id, a, name, null);
    return r;
  };
  var nativeSetAttrNS = __axiom_setAttrNS;
  global.__axiom_setAttrNS = function (id, ns, qname, value) {
    if (!recording()) return nativeSetAttrNS(id, ns, qname, value);
    ns = ns === '' ? null : ns;
    var local = String(qname).substring(String(qname).indexOf(':') + 1);
    var a = findAttr(id, byNamespace(ns, local));
    var err = nativeSetAttrNS(id, ns, qname, value);
    if (err === null) attributeRecord(id, a, local, ns);
    return err;
  };
  var nativeRemoveAttrNS = __axiom_removeAttrNS;
  global.__axiom_removeAttrNS = function (id, ns, local) {
    if (!recording()) return nativeRemoveAttrNS(id, ns, local);
    var a = findAttr(id, byNamespace(ns === '' ? null : ns, local));
    var r = nativeRemoveAttrNS(id, ns, local);
    if (a) attributeRecord(id, a, local, a[0]);
    return r;
  };
  var nativeReplaceAttrNS = __axiom_replaceAttrNS;
  global.__axiom_replaceAttrNS = function (id, ns, prefix, local, value) {
    if (!recording()) return nativeReplaceAttrNS(id, ns, prefix, local, value);
    var a = findAttr(id, byNamespace(ns, local));
    nativeReplaceAttrNS(id, ns, prefix, local, value);
    attributeRecord(id, a, local, ns);
  };
  // Removing an attribute detaches its Attr object, which keeps the last value.
  var recordedRemoveAttr = __axiom_removeAttr;
  global.__axiom_removeAttr = function (id, name) {
    var row = attrCache[id] ? findAttr(id, byQualifiedName(name)) : null;
    var r = recordedRemoveAttr(id, name);
    if (row) detachAttr(id, row);
    return r;
  };
  var recordedRemoveAttrNS = __axiom_removeAttrNS;
  global.__axiom_removeAttrNS = function (id, ns, local) {
    var row = attrCache[id] ? findAttr(id, byNamespace(ns === '' ? null : ns, local)) : null;
    var r = recordedRemoveAttrNS(id, ns, local);
    if (row) detachAttr(id, row);
    return r;
  };
  // Markup insertion: the added nodes are the new ids among the target's children.
  function markupRecord(target, before, removed, prev, next) {
    var added = childIds(target).filter(function (c) { return before.indexOf(c) < 0; });
    if (!added.length && !removed.length) return;
    if (prev === undefined) {
      prev = added.length ? previousSiblingId(added[0]) : -1;
      next = added.length ? nextSiblingId(added[added.length - 1]) : -1;
    }
    childListRecord(target, added, removed, prev, next);
  }
  var nativeSetInnerHTML = __axiom_setInnerHTML;
  global.__axiom_setInnerHTML = function (id, markup) {
    if (!recording()) return nativeSetInnerHTML(id, markup);
    var removed = childIds(id);
    var err = nativeSetInnerHTML(id, markup);
    if (err === null) markupRecord(id, [], removed, -1, -1);
    return err;
  };
  var nativeSetOuterHTML = __axiom_setOuterHTML;
  global.__axiom_setOuterHTML = function (id, markup) {
    var parent = parentOf(id);
    if (!recording() || parent < 0) return nativeSetOuterHTML(id, markup);
    var before = childIds(parent);
    var around = siblingsIn(before, id);
    var err = nativeSetOuterHTML(id, markup);
    if (err === null) markupRecord(parent, before, [id], around[0], around[1]);
    return err;
  };
  var nativeInsertAdjacentHTML = __axiom_insertAdjacentHTML;
  global.__axiom_insertAdjacentHTML = function (id, where, markup) {
    if (!recording()) return nativeInsertAdjacentHTML(id, where, markup);
    var outside = /^(beforebegin|afterend)$/i.test(where);
    var target = outside ? parentOf(id) : id;
    var before = target >= 0 ? childIds(target) : [];
    var err = nativeInsertAdjacentHTML(id, where, markup);
    if (err === null && target >= 0) markupRecord(target, before, []);
    return err;
  };

  // ---------------------------------------------------------------------------
  // ParentNode, NonElementParentNode, ChildNode, NonDocumentTypeChildNode
  // ---------------------------------------------------------------------------
  var DP = Document.prototype, DFP = DocumentFragment.prototype, EP = Element.prototype;
  var CDP = CharacterData.prototype, DTP = DocumentType.prototype;
  var PARENTS = [DP, DFP, EP];

  function elementChildren(id) { return childIds(id).filter(function (c) { return typeOfId(c) === 1; }); }
  getterOnEach(PARENTS, 'children', function () {
    var id = idOf(this);
    return live(HTMLCollection, 'children|' + id, function () { return elementChildren(id); });
  });
  getterOnEach(PARENTS, 'firstElementChild', function () { var e = elementChildren(idOf(this)); return e.length ? wrap(e[0]) : null; });
  getterOnEach(PARENTS, 'lastElementChild', function () { var e = elementChildren(idOf(this)); return e.length ? wrap(e[e.length - 1]) : null; });
  getterOnEach(PARENTS, 'childElementCount', function () { return elementChildren(idOf(this)).length; });

  // "Convert nodes into a node": strings become Text nodes; several nodes go in a fragment.
  function nodesToNode(args) {
    var nodes = [];
    for (var i = 0; i < args.length; i++) {
      nodes.push(isNode(args[i]) ? args[i] : wrap(__axiom_createTextNode(String(args[i]))));
    }
    if (nodes.length === 1) return nodes[0];
    var fragment = wrap(__axiom_createDocumentFragment());
    nodes.forEach(function (n) { preInsert(fragment, n, -1); });
    return fragment;
  }
  onEach(PARENTS, 'prepend', function () {
    var node = nodesToNode(arguments);
    var first = childIds(idOf(this));
    preInsert(this, node, first.length ? first[0] : -1);
  });
  onEach(PARENTS, 'append', function () { preInsert(this, nodesToNode(arguments), -1); });
  onEach(PARENTS, 'replaceChildren', function () {
    var node = nodesToNode(arguments);
    var id = idOf(this), nid = idOf(node);
    var old = childIds(id);
    var added = insertedIds(nid);
    var detached = recording() ? detachRecords(nid) : null;
    // One "replace all" record; a failed insertion restores the old children unobserved.
    var err = withoutRecords(function () {
      old.forEach(function (c) { __axiom_removeChild(id, c); });
      var e = __axiom_insertBefore(id, nid, -1);
      if (e !== null) old.forEach(function (c) { __axiom_insertBefore(id, c, -1); });
      return e;
    });
    if (err !== null) throw domError(String(err));
    if (detached && old.indexOf(nid) < 0) detached();
    if (detached && (old.length || added.length)) childListRecord(id, added, old, -1, -1);
  });
  function checkSelectors(id, selectors) {
    if (__axiom_matches(id, selectors) === null) {
      throw domError('SyntaxError', "'" + selectors + "' is not a valid selector.");
    }
  }
  onEach(PARENTS, 'querySelector', function (selectors) {
    requireArgs(arguments, 1, 'querySelector');
    var id = idOf(this);
    selectors = String(selectors);
    checkSelectors(id, selectors);
    var ids = __axiom_querySelectorAll(id, selectors);
    return ids.length ? wrap(ids[0]) : null;
  });
  onEach(PARENTS, 'querySelectorAll', function (selectors) {
    requireArgs(arguments, 1, 'querySelectorAll');
    var id = idOf(this);
    selectors = String(selectors);
    checkSelectors(id, selectors);
    return staticNodeList(__axiom_querySelectorAll(id, selectors));
  });
  method(DP, 'getElementById', function (elementId) {
    requireArgs(arguments, 1, 'Document.getElementById');
    return wrap(__axiom_getElementById(String(elementId)));
  });
  // Legacy live collections of the document.
  [['images', function (e) { return tagOf(e) === 'img'; }],
   ['embeds', function (e) { return tagOf(e) === 'embed'; }],
   ['plugins', function (e) { return tagOf(e) === 'embed'; }],
   ['scripts', function (e) { return tagOf(e) === 'script'; }],
   ['links', function (e) {
     var t = tagOf(e);
     return (t === 'a' || t === 'area') && __axiom_getAttr(e, 'href') !== null;
   }],
   ['anchors', function (e) { return tagOf(e) === 'a' && __axiom_getAttr(e, 'name') !== null; }]
  ].forEach(function (entry) {
    getter(DP, entry[0], function () {
      var id = idOf(this);
      return live(HTMLCollection, 'doc-' + entry[0] + '|' + id, function () {
        return elementIds(id).filter(entry[1]);
      });
    });
  });
  function tagOf(e) {
    return __axiom_namespaceURI(e) === 'http://www.w3.org/1999/xhtml' ? String(__axiom_tagName(e)) : '';
  }
  // A live NodeList of the HTML elements whose name attribute is `elementName`.
  method(DP, 'getElementsByName', function (elementName) {
    requireArgs(arguments, 1, 'Document.getElementsByName');
    var id = idOf(this);
    elementName = String(elementName);
    return live(NodeList, 'name|' + id + '|' + elementName, function () {
      return elementIds(id).filter(function (e) {
        return __axiom_namespaceURI(e) === 'http://www.w3.org/1999/xhtml' &&
          __axiom_getAttr(e, 'name') === elementName;
      });
    });
  });
  method(DFP, 'getElementById', function (elementId) {
    requireArgs(arguments, 1, 'DocumentFragment.getElementById');
    elementId = String(elementId);
    var ids = elementIds(this.__id);
    for (var i = 0; i < ids.length; i++) if (__axiom_getAttr(ids[i], 'id') === elementId) return wrap(ids[i]);
    return null;
  });
  [DP, EP].forEach(function (p) {
    method(p, 'getElementsByTagName', function (name) {
      requireArgs(arguments, 1, 'getElementsByTagName');
      return byTagName(this, name);
    });
    method(p, 'getElementsByTagNameNS', function (ns, local) {
      requireArgs(arguments, 2, 'getElementsByTagNameNS');
      return byTagNameNS(this, ns, local);
    });
    method(p, 'getElementsByClassName', function (names) {
      requireArgs(arguments, 1, 'getElementsByClassName');
      return byClassName(this, names);
    });
  });

  var CHILDREN = [DTP, EP, CDP];
  function inArgs(args, id) {
    for (var i = 0; i < args.length; i++) if (isNode(args[i]) && idOf(args[i]) === id) return true;
    return false;
  }
  function viableSibling(node, args, step) {
    var s = sibling(node, step);
    while (s !== null && inArgs(args, idOf(s))) s = sibling(s, step);
    return s;
  }
  onEach(CHILDREN, 'before', function () {
    var p = parentOf(this.__id);
    if (p < 0) return;
    var prev = viableSibling(this, arguments, -1);
    var node = nodesToNode(arguments);
    var ref;
    if (prev === null) { var kids = childIds(p); ref = kids.length ? kids[0] : -1; } else {
      var next = sibling(prev, 1);
      ref = next === null ? -1 : idOf(next);
    }
    preInsert(wrap(p), node, ref);
  });
  onEach(CHILDREN, 'after', function () {
    var p = parentOf(this.__id);
    if (p < 0) return;
    var next = viableSibling(this, arguments, 1);
    preInsert(wrap(p), nodesToNode(arguments), next === null ? -1 : idOf(next));
  });
  onEach(CHILDREN, 'replaceWith', function () {
    var p = parentOf(this.__id);
    if (p < 0) return;
    var next = viableSibling(this, arguments, 1);
    var node = nodesToNode(arguments);
    if (parentOf(this.__id) === p) check(__axiom_replaceChild(p, idOf(node), this.__id));
    else preInsert(wrap(p), node, next === null ? -1 : idOf(next));
  });
  onEach(CHILDREN, 'remove', function () {
    var p = parentOf(this.__id);
    if (p >= 0) __axiom_removeChild(p, this.__id);
  });
  function elementSibling(node, step) {
    var s = sibling(node, step);
    while (s !== null && typeOf(s) !== 1) s = sibling(s, step);
    return s;
  }
  getterOnEach([EP, CDP], 'previousElementSibling', function () { return elementSibling(this, -1); });
  getterOnEach([EP, CDP], 'nextElementSibling', function () { return elementSibling(this, 1); });

  // ---------------------------------------------------------------------------
  // CharacterData, Text, ProcessingInstruction, DocumentType
  // ---------------------------------------------------------------------------
  getter(CDP, 'data', function () { return textOf(this.__id); }, function (v) {
    __axiom_setText(this.__id, v === null ? '' : String(v));
  });
  getter(CDP, 'length', function () { return textOf(this.__id).length; });
  function replaceData(node, offset, count, data) {
    var d = textOf(node.__id);
    offset = Number(offset) >>> 0;
    count = Number(count) >>> 0;
    if (offset > d.length) throw domError('IndexSizeError', 'The offset is greater than the length.');
    if (offset + count > d.length) count = d.length - offset;
    rangeDataReplaced(node.__id, offset, count, data.length);
    __axiom_setText(node.__id, d.substring(0, offset) + data + d.substring(offset + count));
  }
  method(CDP, 'substringData', function (offset, count) {
    requireArgs(arguments, 2, 'CharacterData.substringData');
    var d = textOf(this.__id);
    offset = Number(offset) >>> 0;
    count = Number(count) >>> 0;
    if (offset > d.length) throw domError('IndexSizeError', 'The offset is greater than the length.');
    return d.substring(offset, offset + count);
  });
  method(CDP, 'appendData', function (data) {
    requireArgs(arguments, 1, 'CharacterData.appendData');
    replaceData(this, textOf(this.__id).length, 0, String(data));
  });
  method(CDP, 'insertData', function (offset, data) {
    requireArgs(arguments, 2, 'CharacterData.insertData');
    replaceData(this, offset, 0, String(data));
  });
  method(CDP, 'deleteData', function (offset, count) {
    requireArgs(arguments, 2, 'CharacterData.deleteData');
    replaceData(this, offset, count, '');
  });
  method(CDP, 'replaceData', function (offset, count, data) {
    requireArgs(arguments, 3, 'CharacterData.replaceData');
    replaceData(this, offset, count, String(data));
  });
  var TP = Text.prototype;
  method(TP, 'splitText', function (offset) {
    requireArgs(arguments, 1, 'Text.splitText');
    var d = textOf(this.__id);
    offset = Number(offset) >>> 0;
    if (offset > d.length) throw domError('IndexSizeError', 'The offset is greater than the length.');
    var created = wrap(__axiom_createTextNode(d.substring(offset)));
    var p = parentOf(this.__id);
    if (p >= 0) {
      var next = sibling(this, 1);
      preInsert(wrap(p), created, next === null ? -1 : idOf(next));
    }
    rangeSplitText(this.__id, created.__id, offset);
    __axiom_setText(this.__id, d.substring(0, offset));
    return created;
  });
  getter(TP, 'wholeText', function () {
    var start = this;
    var s;
    while ((s = sibling(start, -1)) !== null && typeOf(s) === 3) start = s;
    var out = '';
    for (var n = start; n !== null && typeOf(n) === 3; n = sibling(n, 1)) out += textOf(n.__id);
    return out;
  });
  getter(ProcessingInstruction.prototype, 'target', function () { return String(__axiom_tagName(this.__id)); });
  getter(DTP, 'name', function () { return String(__axiom_tagName(this.__id)); });
  getter(DTP, 'publicId', function () { return String(__axiom_doctypeIds(this.__id)[0]); });
  getter(DTP, 'systemId', function () { return String(__axiom_doctypeIds(this.__id)[1]); });

  // ---------------------------------------------------------------------------
  // Element
  // ---------------------------------------------------------------------------
  getter(EP, 'namespaceURI', function () { return __axiom_namespaceURI(this.__id); });
  getter(EP, 'prefix', function () { return __axiom_prefix(this.__id); });
  getter(EP, 'localName', function () { return String(__axiom_tagName(this.__id)); });
  getter(EP, 'tagName', function () { return tagNameOf(this.__id); });
  function attrName(el, name) {
    name = String(name);
    return isHtml(el.__id) && inHtmlDocument(el.__id) ? asciiLower(name) : name;
  }
  function reflect(proto, prop, attr) {
    getter(proto, prop, function () { var v = __axiom_getAttr(this.__id, attr); return v === null ? '' : String(v); },
      function (v) { __axiom_setAttr(this.__id, attr, String(v)); });
  }
  function reflectBoolean(proto, prop, attr) {
    getter(proto, prop, function () { return __axiom_getAttr(this.__id, attr) !== null; },
      function (v) {
        if (v) __axiom_setAttr(this.__id, attr, '');
        else __axiom_removeAttr(this.__id, attr);
      });
  }
  reflect(EP, 'id', 'id');
  reflect(EP, 'className', 'class');
  ['Button', 'FieldSet', 'Input', 'Link', 'OptGroup', 'Option', 'Select', 'TextArea'].forEach(function (n) {
    reflectBoolean(global['HTML' + n + 'Element'].prototype, 'disabled', 'disabled');
  });
  method(EP, 'getAttribute', function (name) {
    requireArgs(arguments, 1, 'Element.getAttribute');
    return __axiom_getAttr(this.__id, attrName(this, name));
  });
  method(EP, 'getAttributeNS', function (ns, local) {
    requireArgs(arguments, 2, 'Element.getAttributeNS');
    return __axiom_getAttrNS(this.__id, optNamespace(ns), String(local));
  });
  method(EP, 'hasAttribute', function (name) {
    requireArgs(arguments, 1, 'Element.hasAttribute');
    return __axiom_getAttr(this.__id, attrName(this, name)) !== null;
  });
  method(EP, 'hasAttributeNS', function (ns, local) {
    requireArgs(arguments, 2, 'Element.hasAttributeNS');
    return __axiom_getAttrNS(this.__id, optNamespace(ns), String(local)) !== null;
  });
  function validAttributeName(name) {
    if (name === '' || INVALID_ATTRIBUTE_CHAR.test(name)) {
      throw domError('InvalidCharacterError', "'" + name + "' is not a valid attribute name.");
    }
  }
  method(EP, 'setAttribute', function (name, value) {
    requireArgs(arguments, 2, 'Element.setAttribute');
    validAttributeName(String(name));
    __axiom_setAttr(this.__id, attrName(this, name), String(value));
  });
  method(EP, 'setAttributeNS', function (ns, name, value) {
    requireArgs(arguments, 3, 'Element.setAttributeNS');
    check(__axiom_setAttrNS(this.__id, optNamespace(ns), String(name), String(value)));
  });
  method(EP, 'removeAttribute', function (name) {
    requireArgs(arguments, 1, 'Element.removeAttribute');
    __axiom_removeAttr(this.__id, attrName(this, name));
  });
  method(EP, 'removeAttributeNS', function (ns, local) {
    requireArgs(arguments, 2, 'Element.removeAttributeNS');
    __axiom_removeAttrNS(this.__id, optNamespace(ns), String(local));
  });
  method(EP, 'toggleAttribute', function (name, force) {
    requireArgs(arguments, 1, 'Element.toggleAttribute');
    validAttributeName(String(name));
    name = attrName(this, name);
    var has = __axiom_getAttr(this.__id, name) !== null;
    var given = arguments.length > 1 && force !== undefined;
    if (!has) {
      if (!given || force) { __axiom_setAttr(this.__id, name, ''); return true; }
      return false;
    }
    if (!given || !force) { __axiom_removeAttr(this.__id, name); return false; }
    return true;
  });
  method(EP, 'hasAttributes', function () { return __axiom_attributes(this.__id).length > 0; });
  method(EP, 'getAttributeNames', function () {
    return __axiom_attributes(this.__id).map(function (a) { return a[1] === null ? a[2] : a[1] + ':' + a[2]; });
  });

  // ---------------------------------------------------------------------------
  // Attr and NamedNodeMap (DOM §4.9). The element's attribute list in the host is the
  // source of truth; an Attr object is a handle on one entry of it (owner >= 0) or,
  // once removed or before it is set, carries its own value (owner -1).
  // ---------------------------------------------------------------------------
  function isAttr(v) { return v instanceof Attr; }
  function attrKey(ns, local) { return (ns === null ? '\u0001' : ns) + '\u0000' + local; }
  function newAttr(owner, ns, prefix, local, value) {
    var a = Object.create(Attr.prototype);
    Object.defineProperty(a, ATTR, {
      value: { owner: owner, ns: ns, prefix: prefix, local: local, value: value, serial: ++attrSerial, kids: null }
    });
    return a;
  }
  // The Attr object for attribute row `row` ([ns, prefix, local, value]) of element `el`.
  function attrFor(el, row) {
    var map = attrCache[el] || (attrCache[el] = Object.create(null));
    var key = attrKey(row[0], row[2]);
    return map[key] || (map[key] = newAttr(el, row[0], row[1], row[2], row[3]));
  }
  function attrObjects(el) { return __axiom_attributes(el).map(function (r) { return attrFor(el, r); }); }
  function detachAttr(el, row) {
    var map = attrCache[el];
    var key = attrKey(row[0], row[2]);
    var a = map && map[key];
    if (!a) return;
    var st = a[ATTR];
    st.owner = -1;
    st.prefix = row[1];
    st.value = row[3];
    delete map[key];
  }
  // The Attr's current state, refreshed from its element.
  function attrState(a) {
    var st = a[ATTR];
    if (!st) throw new TypeError('Illegal invocation');
    if (st.owner >= 0) {
      var row = findAttr(st.owner, byNamespace(st.ns, st.local));
      if (row) {
        st.prefix = row[1];
        st.value = row[3];
      } else {
        var map = attrCache[st.owner], key = attrKey(st.ns, st.local);
        if (map && map[key] === a) delete map[key];
        st.owner = -1;
      }
    }
    return st;
  }
  function attrOwnerNode(a) { var st = attrState(a); return st.owner >= 0 ? wrap(st.owner) : null; }
  function attrQualifiedName(st) { return st.prefix === null ? st.local : st.prefix + ':' + st.local; }
  function setAttrValue(a, v) {
    var st = attrState(a);
    if (st.owner < 0) { st.value = v; return; }
    __axiom_replaceAttrNS(st.owner, st.ns, st.prefix, st.local, v);
    st.value = v;
  }
  // "Get an attribute by name": HTML elements of HTML documents match the lowercased name.
  function attrByName(el, name) {
    var row = findAttr(el, byQualifiedName(isHtml(el) && inHtmlDocument(el) ? asciiLower(name) : name));
    return row ? attrFor(el, row) : null;
  }
  function attrByNamespace(el, ns, local) {
    ns = optNamespace(ns);
    var row = findAttr(el, byNamespace(ns === '' ? null : ns, String(local)));
    return row ? attrFor(el, row) : null;
  }
  function requireAttr(v, where) {
    if (!isAttr(v)) throw new TypeError("Failed to execute '" + where + "': parameter is not of type 'Attr'.");
    return v;
  }
  // "Set an attribute" (DOM §4.9): returns the Attr it replaced, or null.
  function setAttrNode(el, attr) {
    var st = attrState(attr);
    if (st.owner >= 0 && st.owner !== el) throw domError('InUseAttributeError', 'The attribute is in use by another element.');
    var row = findAttr(el, byNamespace(st.ns, st.local));
    var old = row ? attrFor(el, row) : null;
    if (old === attr) return attr;
    __axiom_replaceAttrNS(el, st.ns, st.prefix, st.local, st.value);
    if (old) {
      var os = old[ATTR];
      os.owner = -1;
      os.prefix = row[1];
      os.value = row[3];
    }
    (attrCache[el] || (attrCache[el] = Object.create(null)))[attrKey(st.ns, st.local)] = attr;
    st.owner = el;
    return old;
  }
  function removeAttrNode(el, attr) {
    var st = attrState(attr);
    if (st.owner !== el) throw domError('NotFoundError', 'The attribute is not owned by this element.');
    __axiom_removeAttrNS(el, st.ns, st.local);
    return attr;
  }

  var AP = Attr.prototype;
  getter(AP, 'namespaceURI', function () { return attrState(this).ns; });
  getter(AP, 'prefix', function () { return attrState(this).prefix; });
  getter(AP, 'localName', function () { return attrState(this).local; });
  getter(AP, 'name', function () { return attrQualifiedName(attrState(this)); });
  getter(AP, 'value', function () { return attrState(this).value; }, function (v) { setAttrValue(this, String(v)); });
  getter(AP, 'ownerElement', function () { return attrOwnerNode(this); });
  getter(AP, 'specified', function () { return true; });
  // Node members, for a node that is never in a tree.
  getter(AP, 'nodeType', function () { attrState(this); return 2; });
  getter(AP, 'nodeName', function () { return attrQualifiedName(attrState(this)); });
  ['nodeValue', 'textContent'].forEach(function (name) {
    getter(AP, name, function () { return attrState(this).value; },
      function (v) { setAttrValue(this, nullableString(v)); });
  });
  getter(AP, 'ownerDocument', function () {
    var owner = attrState(this).owner;
    return owner >= 0 ? wrap(documentOf(owner)) : doc;
  });
  getter(AP, 'isConnected', function () { return false; });
  ['parentNode', 'parentElement', 'firstChild', 'lastChild', 'previousSibling', 'nextSibling'].forEach(function (name) {
    getter(AP, name, function () { return null; });
  });
  getter(AP, 'childNodes', function () {
    var st = attrState(this);
    return st.kids || (st.kids = staticNodeList([]));
  });
  method(AP, 'hasChildNodes', function () { return false; });
  method(AP, 'getRootNode', function () { return this; });
  method(AP, 'normalize', function () {});
  method(AP, 'cloneNode', function () {
    var st = attrState(this);
    return newAttr(-1, st.ns, st.prefix, st.local, st.value);
  });
  method(AP, 'isEqualNode', function (other) {
    if (other === null || other === undefined) return false;
    if (!isAttr(requireNode(other, 'Node.isEqualNode'))) return false;
    var a = attrState(this), b = attrState(other);
    return a.ns === b.ns && a.local === b.local && a.value === b.value;
  });
  method(AP, 'contains', function (other) {
    if (other === null || other === undefined) return false;
    return requireNode(other, 'Node.contains') === this;
  });
  method(AP, 'lookupNamespaceURI', function (prefix) {
    var el = attrOwnerNode(this);
    return el === null ? null : NP.lookupNamespaceURI.call(el, prefix);
  });
  method(AP, 'lookupPrefix', function (ns) {
    var el = attrOwnerNode(this);
    return el === null ? null : NP.lookupPrefix.call(el, ns);
  });
  method(AP, 'isDefaultNamespace', function (ns) {
    var el = attrOwnerNode(this);
    if (el !== null) return NP.isDefaultNamespace.call(el, ns);
    ns = optNamespace(ns);
    return ns === null || ns === '';
  });

  // NamedNodeMap: a live view of one element's attribute list.
  function mapElement(t) { return t[STATE].el; }
  function mapNames(t) {
    var el = mapElement(t), html = isHtml(el), out = [];
    __axiom_attributes(el).forEach(function (r) {
      var q = r[1] === null ? r[2] : r[1] + ':' + r[2];
      if (html && /[A-Z]/.test(q)) return;
      if (out.indexOf(q) < 0) out.push(q);
    });
    return out;
  }
  var namedNodeMapHandler = indexedHandler(
    function (t) { return attrObjects(mapElement(t)); },
    function (a) { return a; },
    {
      names: mapNames,
      get: function (t, name) { return mapNames(t).indexOf(name) >= 0 ? attrByName(mapElement(t), name) : null; }
    });
  var mapCache = Object.create(null);
  getter(EP, 'attributes', function () {
    var id = this.__id;
    var m = mapCache[id];
    if (!m) {
      var target = Object.create(NamedNodeMap.prototype);
      Object.defineProperty(target, STATE, { value: { el: id } });
      m = mapCache[id] = new Proxy(target, namedNodeMapHandler);
    }
    return m;
  });
  var NNP = NamedNodeMap.prototype;
  getter(NNP, 'length', function () { return __axiom_attributes(mapElement(this)).length; });
  method(NNP, 'item', function (i) {
    requireArgs(arguments, 1, 'NamedNodeMap.item');
    var rows = __axiom_attributes(mapElement(this));
    i = Number(i) >>> 0;
    return i < rows.length ? attrFor(mapElement(this), rows[i]) : null;
  });
  method(NNP, 'getNamedItem', function (name) {
    requireArgs(arguments, 1, 'NamedNodeMap.getNamedItem');
    return attrByName(mapElement(this), String(name));
  });
  method(NNP, 'getNamedItemNS', function (ns, local) {
    requireArgs(arguments, 2, 'NamedNodeMap.getNamedItemNS');
    return attrByNamespace(mapElement(this), ns, local);
  });
  ['setNamedItem', 'setNamedItemNS'].forEach(function (name) {
    method(NNP, name, function (attr) {
      requireArgs(arguments, 1, 'NamedNodeMap.' + name);
      return setAttrNode(mapElement(this), requireAttr(attr, 'NamedNodeMap.' + name));
    });
  });
  method(NNP, 'removeNamedItem', function (name) {
    requireArgs(arguments, 1, 'NamedNodeMap.removeNamedItem');
    var a = attrByName(mapElement(this), String(name));
    if (a === null) throw domError('NotFoundError', "No attribute named '" + name + "'.");
    return removeAttrNode(mapElement(this), a);
  });
  method(NNP, 'removeNamedItemNS', function (ns, local) {
    requireArgs(arguments, 2, 'NamedNodeMap.removeNamedItemNS');
    var a = attrByNamespace(mapElement(this), ns, local);
    if (a === null) throw domError('NotFoundError', "No attribute named '" + local + "'.");
    return removeAttrNode(mapElement(this), a);
  });
  if (typeof Symbol === 'function' && Symbol.iterator) method(NNP, Symbol.iterator, Array.prototype[Symbol.iterator]);

  method(EP, 'getAttributeNode', function (name) {
    requireArgs(arguments, 1, 'Element.getAttributeNode');
    return attrByName(this.__id, String(name));
  });
  method(EP, 'getAttributeNodeNS', function (ns, local) {
    requireArgs(arguments, 2, 'Element.getAttributeNodeNS');
    return attrByNamespace(this.__id, ns, local);
  });
  ['setAttributeNode', 'setAttributeNodeNS'].forEach(function (name) {
    method(EP, name, function (attr) {
      requireArgs(arguments, 1, 'Element.' + name);
      return setAttrNode(this.__id, requireAttr(attr, 'Element.' + name));
    });
  });
  method(EP, 'removeAttributeNode', function (attr) {
    requireArgs(arguments, 1, 'Element.removeAttributeNode');
    return removeAttrNode(this.__id, requireAttr(attr, 'Element.removeAttributeNode'));
  });
  method(EP, 'matches', function (selectors) {
    requireArgs(arguments, 1, 'Element.matches');
    var m = __axiom_matches(this.__id, String(selectors));
    if (m === null) throw domError('SyntaxError', "'" + selectors + "' is not a valid selector.");
    return m;
  });
  method(EP, 'webkitMatchesSelector', EP.matches);
  method(EP, 'closest', function (selectors) {
    requireArgs(arguments, 1, 'Element.closest');
    selectors = String(selectors);
    var n = __axiom_closest(this.__id, selectors);
    if (n === null) throw domError('SyntaxError', "'" + selectors + "' is not a valid selector.");
    return n < 0 ? null : wrap(n);
  });
  function insertAdjacent(el, where, node) {
    var p = parentOf(el.__id);
    switch (asciiLower(String(where))) {
      case 'beforebegin':
        if (p < 0) return null;
        return preInsert(wrap(p), node, el.__id);
      case 'afterbegin': {
        var kids = childIds(el.__id);
        return preInsert(el, node, kids.length ? kids[0] : -1);
      }
      case 'beforeend':
        return preInsert(el, node, -1);
      case 'afterend': {
        if (p < 0) return null;
        var next = sibling(el, 1);
        return preInsert(wrap(p), node, next === null ? -1 : idOf(next));
      }
      default:
        throw domError('SyntaxError', "'" + where + "' is not a valid position.");
    }
  }
  method(EP, 'insertAdjacentElement', function (where, element) {
    requireArgs(arguments, 2, 'Element.insertAdjacentElement');
    if (!(element instanceof Element)) throw new TypeError("parameter 2 is not of type 'Element'.");
    return insertAdjacent(this, where, element);
  });
  method(EP, 'insertAdjacentText', function (where, data) {
    requireArgs(arguments, 2, 'Element.insertAdjacentText');
    insertAdjacent(this, where, wrap(__axiom_createTextNode(String(data))));
  });

  // HTML serialization and fragment parsing. Scripts parsed here never run.
  function markupResult(err, what) {
    if (err !== null) throw domError(err, what + ' failed: ' + err + '.');
  }
  function markupString(v) { return v === null ? '' : String(v); }
  getter(EP, 'innerHTML', function () { return __axiom_innerHTML(this.__id); }, function (v) {
    markupResult(__axiom_setInnerHTML(this.__id, markupString(v)), 'Setting innerHTML');
  });
  getter(EP, 'outerHTML', function () { return __axiom_outerHTML(this.__id); }, function (v) {
    markupResult(__axiom_setOuterHTML(this.__id, markupString(v)), 'Setting outerHTML');
  });
  getter(global.HTMLTemplateElement.prototype, 'content', function () {
    var id = __axiom_templateContent(idOf(this));
    return id === null ? null : wrap(id);
  });

  // ---------------------------------------------------------------------------
  // Shadow DOM (DOM §4.2.2, §4.8; HTML §4.12.4 slots)
  // ---------------------------------------------------------------------------
  var SHADOW_OPEN = 1, SHADOW_DELEGATES_FOCUS = 2, SHADOW_MANUAL = 4, SHADOW_CLONABLE = 8,
    SHADOW_SERIALIZABLE = 16;
  // The flags of shadow root `root`; 0 when it is not one.
  function shadowFlags(root) {
    var host = hostOf(root);
    var info = host >= 0 ? __axiom_shadowRootInfo(host) : null;
    return info ? info[1] | 0 : 0;
  }
  function isOpenShadowRoot(id) { return hostOf(id) >= 0 && (shadowFlags(id) & SHADOW_OPEN) !== 0; }
  function shadowRootId(node) {
    var id = idOf(node);
    if (hostOf(id) < 0) throw new TypeError('Illegal invocation');
    return id;
  }
  // "Retarget" `id` against node `against`: climb out of shadow trees that are not
  // shadow-including ancestors of `against`.
  function retargetId(id, against) {
    for (;;) {
      var root = rootOf(id);
      var host = hostOf(root);
      if (host < 0 || shadowIncludingContains(root, against)) return id;
      id = host;
    }
  }
  function shadowIncludingContains(ancestor, id) {
    while (id >= 0) {
      if (id === ancestor) return true;
      var p = parentOf(id);
      id = p >= 0 ? p : hostOf(id);
    }
    return false;
  }

  method(EP, 'attachShadow', function attachShadow(init) {
    requireArgs(arguments, 1, 'Element.attachShadow');
    if (init === null || (typeof init !== 'object' && typeof init !== 'function')) {
      throw new TypeError("Failed to execute 'attachShadow' on 'Element': The provided value is not of type 'ShadowRootInit'.");
    }
    var clonable = !!init.clonable;
    var delegatesFocus = !!init.delegatesFocus;
    var mode = init.mode;
    if (mode === undefined) {
      throw new TypeError("Failed to execute 'attachShadow' on 'Element': Failed to read the 'mode' property from 'ShadowRootInit': Required member is undefined.");
    }
    mode = String(mode);
    if (mode !== 'open' && mode !== 'closed') {
      throw new TypeError("Failed to execute 'attachShadow' on 'Element': The provided value '" + mode + "' is not a valid enum value of type ShadowRootMode.");
    }
    var serializable = !!init.serializable;
    var slotAssignment = init.slotAssignment === undefined ? 'named' : String(init.slotAssignment);
    if (slotAssignment !== 'named' && slotAssignment !== 'manual') {
      throw new TypeError("Failed to execute 'attachShadow' on 'Element': The provided value '" + slotAssignment + "' is not a valid enum value of type SlotAssignmentMode.");
    }
    var flags = (mode === 'open' ? SHADOW_OPEN : 0) | (delegatesFocus ? SHADOW_DELEGATES_FOCUS : 0) |
      (slotAssignment === 'manual' ? SHADOW_MANUAL : 0) | (clonable ? SHADOW_CLONABLE : 0) |
      (serializable ? SHADOW_SERIALIZABLE : 0);
    var root = __axiom_attachShadow(this.__id, flags) | 0;
    if (root < 0) {
      throw domError('NotSupportedError', "Failed to execute 'attachShadow' on 'Element': This element does not support attachShadow");
    }
    return wrap(root);
  });
  getter(EP, 'shadowRoot', function () {
    var info = __axiom_shadowRootInfo(this.__id);
    return info && (info[1] & SHADOW_OPEN) ? wrap(info[0]) : null;
  });
  reflect(EP, 'slot', 'slot');
  // Slottable.assignedSlot: only slots in open shadow trees are exposed.
  function assignedSlotOf() {
    var slot = __axiom_assignedSlot(this.__id) | 0;
    return slot >= 0 && isOpenShadowRoot(rootOf(slot)) ? wrap(slot) : null;
  }
  getter(EP, 'assignedSlot', assignedSlotOf);
  getter(Text.prototype, 'assignedSlot', assignedSlotOf);

  var SRP = ShadowRoot.prototype;
  getter(SRP, 'host', function () { return wrap(hostOf(shadowRootId(this))); });
  getter(SRP, 'mode', function () { return shadowFlags(shadowRootId(this)) & SHADOW_OPEN ? 'open' : 'closed'; });
  getter(SRP, 'delegatesFocus', function () { return (shadowFlags(shadowRootId(this)) & SHADOW_DELEGATES_FOCUS) !== 0; });
  getter(SRP, 'slotAssignment', function () { return shadowFlags(shadowRootId(this)) & SHADOW_MANUAL ? 'manual' : 'named'; });
  getter(SRP, 'clonable', function () { return (shadowFlags(shadowRootId(this)) & SHADOW_CLONABLE) !== 0; });
  getter(SRP, 'serializable', function () { return (shadowFlags(shadowRootId(this)) & SHADOW_SERIALIZABLE) !== 0; });
  getter(SRP, 'innerHTML', function () { return __axiom_innerHTML(shadowRootId(this)); }, function (v) {
    markupResult(__axiom_setInnerHTML(shadowRootId(this), markupString(v)), 'Setting innerHTML');
  });
  method(SRP, 'setHTMLUnsafe', function setHTMLUnsafe(html) {
    requireArgs(arguments, 1, 'ShadowRoot.setHTMLUnsafe');
    markupResult(__axiom_setInnerHTML(shadowRootId(this), markupString(html)), 'setHTMLUnsafe');
  });
  method(SRP, 'getHTML', function getHTML() { return __axiom_innerHTML(shadowRootId(this)); });
  // DocumentOrShadowRoot.activeElement: the focused element retargeted against this root.
  getter(SRP, 'activeElement', function () {
    var root = shadowRootId(this);
    var focused = __axiom_focusedElement() | 0;
    if (focused < 0) return null;
    var id = retargetId(focused, root);
    return rootOf(id) === root ? wrap(id) : null;
  });
  global.__axiom_retargetId = retargetId;

  var SLP = global.HTMLSlotElement.prototype;
  reflect(SLP, 'name', 'name');
  function slotAssigned(slot, options, elementsOnly) {
    var flatten = options !== undefined && options !== null && !!options.flatten;
    var ids = Array.prototype.slice.call(__axiom_assignedNodes(slot.__id, flatten));
    if (elementsOnly) ids = ids.filter(function (id) { return typeOfId(id) === 1; });
    return ids.map(wrap);
  }
  method(SLP, 'assignedNodes', function assignedNodes(options) { return slotAssigned(this, options, false); });
  method(SLP, 'assignedElements', function assignedElements(options) { return slotAssigned(this, options, true); });
  method(SLP, 'assign', function assign() {
    var ids = [];
    for (var i = 0; i < arguments.length; i++) {
      var n = arguments[i];
      if (!isNode(n) || (typeOf(n) !== 1 && typeOf(n) !== 3)) {
        throw new TypeError("Failed to execute 'assign' on 'HTMLSlotElement': parameter " + (i + 1) + " is not of type 'Element' or 'Text'.");
      }
      ids.push(n.__id | 0);
    }
    __axiom_assignSlot(this.__id, ids);
  });
  method(EP, 'insertAdjacentHTML', function (where, markup) {
    requireArgs(arguments, 2, 'Element.insertAdjacentHTML');
    markupResult(__axiom_insertAdjacentHTML(this.__id, String(where), String(markup)), 'insertAdjacentHTML');
  });

  // DOMTokenList over an attribute (classList).
  var TOKENS = typeof Symbol === 'function' ? Symbol('tokens') : '__axiom_tokens';
  function tokensOf(list) {
    var st = list[TOKENS];
    var v = __axiom_getAttr(st.id, st.attr);
    return v === null ? [] : classTokens(v);
  }
  function validToken(token) {
    token = String(token);
    if (token === '') throw domError('SyntaxError', 'The token must not be empty.');
    if (/[\t\n\f\r ]/.test(token)) throw domError('InvalidCharacterError', 'The token must not contain whitespace.');
    return token;
  }
  function updateTokens(list, tokens) {
    var st = list[TOKENS];
    if (__axiom_getAttr(st.id, st.attr) === null && tokens.length === 0) return;
    __axiom_setAttr(st.id, st.attr, tokens.join(' '));
  }
  var tokenHandler = indexedHandler(function (t) { return tokensOf(t); }, function (s) { return s; }, false);
  var tokenLists = Object.create(null);
  function tokenList(el, attr) {
    var key = el.__id + '|' + attr;
    var list = tokenLists[key];
    if (!list) {
      var target = Object.create(DOMTokenList.prototype);
      Object.defineProperty(target, TOKENS, { value: { id: el.__id, attr: attr } });
      list = tokenLists[key] = new Proxy(target, tokenHandler);
    }
    return list;
  }
  var TLP = DOMTokenList.prototype;
  getter(TLP, 'length', function () { return tokensOf(this).length; });
  method(TLP, 'item', function (i) { var t = tokensOf(this); i = Number(i) >>> 0; return i < t.length ? t[i] : null; });
  method(TLP, 'contains', function (token) { return tokensOf(this).indexOf(String(token)) >= 0; });
  method(TLP, 'add', function () {
    var add = Array.prototype.map.call(arguments, validToken);
    var tokens = tokensOf(this);
    add.forEach(function (t) { if (tokens.indexOf(t) < 0) tokens.push(t); });
    updateTokens(this, tokens);
  });
  method(TLP, 'remove', function () {
    var remove = Array.prototype.map.call(arguments, validToken);
    updateTokens(this, tokensOf(this).filter(function (t) { return remove.indexOf(t) < 0; }));
  });
  method(TLP, 'toggle', function (token, force) {
    requireArgs(arguments, 1, 'DOMTokenList.toggle');
    token = validToken(token);
    var given = arguments.length > 1 && force !== undefined;
    var tokens = tokensOf(this);
    var i = tokens.indexOf(token);
    if (i >= 0) {
      if (!given || !force) { tokens.splice(i, 1); updateTokens(this, tokens); return false; }
      return true;
    }
    if (!given || force) { tokens.push(token); updateTokens(this, tokens); return true; }
    return false;
  });
  method(TLP, 'replace', function (token, replacement) {
    requireArgs(arguments, 2, 'DOMTokenList.replace');
    token = String(token);
    replacement = String(replacement);
    if (token === '' || replacement === '') throw domError('SyntaxError', 'The token must not be empty.');
    validToken(token);
    validToken(replacement);
    var tokens = tokensOf(this);
    if (tokens.indexOf(token) < 0) return false;
    var out = [];
    tokens.forEach(function (t) {
      var v = t === token || t === replacement ? replacement : t;
      if (out.indexOf(v) < 0) out.push(v);
    });
    updateTokens(this, out);
    return true;
  });
  method(TLP, 'supports', function () { throw new TypeError('This DOMTokenList has no supported tokens.'); });
  getter(TLP, 'value', function () {
    var st = this[TOKENS];
    var v = __axiom_getAttr(st.id, st.attr);
    return v === null ? '' : String(v);
  }, function (v) { var st = this[TOKENS]; __axiom_setAttr(st.id, st.attr, String(v)); });
  method(TLP, 'toString', function () { return this.value; });
  method(TLP, 'forEach', Array.prototype.forEach);
  method(TLP, 'entries', Array.prototype.entries);
  method(TLP, 'keys', Array.prototype.keys);
  method(TLP, 'values', Array.prototype.values);
  if (typeof Symbol === 'function' && Symbol.iterator) method(TLP, Symbol.iterator, Array.prototype[Symbol.iterator]);
  getter(EP, 'classList', function () { return tokenList(this, 'class'); }, function (v) { this.classList.value = v; });

  // ---------------------------------------------------------------------------
  // Document
  // ---------------------------------------------------------------------------
  method(DP, 'createElement', function (localName) {
    requireArgs(arguments, 1, 'Document.createElement');
    var name = String(localName);
    if (!VALID_ELEMENT_NAME.test(name)) {
      throw domError('InvalidCharacterError', "'" + name + "' is not a valid element name.");
    }
    return wrap(__axiom_createElement(asciiLower(name)));
  });
  method(DP, 'createElementNS', function (ns, qualified) {
    requireArgs(arguments, 2, 'Document.createElementNS');
    var r = __axiom_createElementNS(optNamespace(ns), String(qualified));
    if (typeof r === 'string') throw domError(r);
    return wrap(r);
  });
  method(DP, 'createTextNode', function (data) {
    requireArgs(arguments, 1, 'Document.createTextNode');
    return wrap(__axiom_createTextNode(String(data)));
  });
  method(DP, 'createComment', function (data) {
    requireArgs(arguments, 1, 'Document.createComment');
    return wrap(__axiom_createComment(String(data)));
  });
  method(DP, 'createCDATASection', function (data) {
    requireArgs(arguments, 1, 'Document.createCDATASection');
    if (isHtmlDocument(this)) {
      throw domError('NotSupportedError', 'CDATA sections cannot be created in HTML documents.');
    }
    data = String(data);
    if (data.indexOf(']]>') >= 0) throw domError('InvalidCharacterError', 'CDATA data cannot contain ]]>.');
    return wrap(__axiom_createCDATASection(data));
  });
  method(DP, 'createProcessingInstruction', function (target, data) {
    requireArgs(arguments, 2, 'Document.createProcessingInstruction');
    target = String(target);
    data = String(data);
    if (!XML_NAME.test(target) || data.indexOf('?>') >= 0) {
      throw domError('InvalidCharacterError', 'Invalid processing instruction.');
    }
    return wrap(__axiom_createProcessingInstruction(target, data));
  });
  method(DP, 'createDocumentFragment', function () { return wrap(__axiom_createDocumentFragment()); });
  method(DP, 'createAttribute', function (localName) {
    requireArgs(arguments, 1, 'Document.createAttribute');
    var name = String(localName);
    validAttributeName(name);
    return newAttr(-1, null, null, isHtmlDocument(this) ? asciiLower(name) : name, '');
  });
  method(DP, 'createAttributeNS', function (ns, qualified) {
    requireArgs(arguments, 2, 'Document.createAttributeNS');
    var r = __axiom_validateAttrName(optNamespace(ns), String(qualified));
    if (typeof r === 'string') throw domError(r);
    return newAttr(-1, r[0], r[1], r[2], '');
  });
  method(DP, 'importNode', function (node, deep) {
    requireArgs(arguments, 1, 'Document.importNode');
    requireNode(node, 'Document.importNode');
    if (typeOf(node) === 9) throw domError('NotSupportedError', 'Documents cannot be imported.');
    return wrap(__axiom_cloneNode(node.__id, !!deep));
  });
  method(DP, 'adoptNode', function (node) {
    requireArgs(arguments, 1, 'Document.adoptNode');
    requireNode(node, 'Document.adoptNode');
    if (typeOf(node) === 9) throw domError('NotSupportedError', 'Documents cannot be adopted.');
    var p = parentOf(node.__id);
    if (p >= 0) __axiom_removeChild(p, node.__id);
    return node;
  });
  getter(DP, 'doctype', function () {
    var ids = childIds(idOf(this)).filter(function (c) { return typeOfId(c) === 10; });
    return ids.length ? wrap(ids[0]) : null;
  });

  var implementation = Object.create(DOMImplementation.prototype);
  method(DOMImplementation.prototype, 'hasFeature', function () { return true; });
  method(DOMImplementation.prototype, 'createDocumentType', function (name, publicId, systemId) {
    requireArgs(arguments, 3, 'DOMImplementation.createDocumentType');
    name = String(name);
    if (INVALID_DOCTYPE_CHAR.test(name)) {
      throw domError('InvalidCharacterError', "'" + name + "' is not a valid doctype name.");
    }
    var id = __axiom_createDocumentType(name, String(publicId), String(systemId)) | 0;
    var owner = this[IMPL_DOCUMENT];
    if (owner !== undefined && owner !== docId()) ownerOf[id] = owner;
    return wrap(id);
  });
  // Each document has its own DOMImplementation; doctypes it creates belong to it.
  var IMPL_DOCUMENT = typeof Symbol === 'function' ? Symbol('document') : '__axiom_implDocument';
  var implementations = Object.create(null);
  getter(DP, 'implementation', function () {
    if (this === doc || !isOtherDocument(this)) return implementation;
    var id = this.__id | 0;
    var impl = implementations[id];
    if (!impl) {
      impl = implementations[id] = Object.create(DOMImplementation.prototype);
      Object.defineProperty(impl, IMPL_DOCUMENT, { value: id });
    }
    return impl;
  });
  // [LegacyUnforgeable] location is the realm document's own; other documents have none.
  getter(DP, 'location', function () { return this === doc ? global.location : null; });

  // ---------------------------------------------------------------------------
  // Inert documents
  // ---------------------------------------------------------------------------
  function registerDocument(id, kind, contentType, url) {
    if (id < 0) throw domError('NotSupportedError', 'Creating additional documents is not supported.');
    docInfo[id] = { kind: kind, contentType: contentType, url: url, quirks: false };
    otherDocCount++;
    return docInfo[id];
  }
  function newDocument(kind, contentType, url) {
    var id = __axiom_createDocument() | 0;
    registerDocument(id, kind, contentType, url);
    return wrap(id);
  }
  function isOtherDocument(v) { return v !== doc && v instanceof Node && docInfo[v.__id | 0] !== undefined; }
  // The id of the document node `id` belongs to.
  function documentOf(id) {
    if (otherDocCount === 0) return docId();
    var root = rootOf(id);
    var host = hostOf(root);
    if (host >= 0) return documentOf(host);
    if (typeOfId(root) === 9) return root;
    var owner = ownerOf[root];
    return owner === undefined ? docId() : owner;
  }
  function claim(document, id) {
    if (document === doc || !isOtherDocument(document)) delete ownerOf[id];
    else ownerOf[id] = document.__id | 0;
  }
  // A node removed from an inert document's tree stays that document's.
  var removeChildBeforeDocuments = __axiom_removeChild;
  global.__axiom_removeChild = function (parent, child) {
    if (otherDocCount === 0) return removeChildBeforeDocuments(parent, child);
    var owner = documentOf(parent);
    var ok = removeChildBeforeDocuments(parent, child);
    if (ok) {
      if (owner === docId()) delete ownerOf[child];
      else ownerOf[child] = owner;
    }
    return ok;
  };
  // XML documents keep element names as given; application/xhtml+xml ones put them in
  // the HTML namespace.
  var createElementInHtml = DP.createElement;
  method(DP, 'createElement', function (localName) {
    var info = this === doc
      ? { kind: liveDocumentIsHtml() ? 'html' : 'xml', contentType: String(doc.contentType) }
      : (isOtherDocument(this) ? docInfo[this.__id | 0] : null);
    if (!info || info.kind === 'html') return createElementInHtml.apply(this, arguments);
    requireArgs(arguments, 1, 'Document.createElement');
    var name = String(localName);
    if (!VALID_ELEMENT_NAME.test(name)) {
      throw domError('InvalidCharacterError', "'" + name + "' is not a valid element name.");
    }
    return this.createElementNS(info.contentType === 'application/xhtml+xml' ? HTML_NS : null, name);
  });
  ['createElement', 'createElementNS', 'createTextNode', 'createComment', 'createProcessingInstruction',
   'createDocumentFragment', 'importNode'].forEach(function (name) {
    var create = DP[name];
    method(DP, name, function () {
      var node = create.apply(this, arguments);
      if (this !== doc && node && !isAttr(node)) claim(this, node.__id | 0);
      return node;
    });
  });
  var adoptIntoMain = DP.adoptNode;
  method(DP, 'adoptNode', function (node) {
    var adopted = adoptIntoMain.apply(this, arguments);
    if (!isAttr(adopted)) claim(this, adopted.__id | 0);
    return adopted;
  });
  var mainGetElementById = DP.getElementById;
  method(DP, 'getElementById', function (elementId) {
    if (this === doc || !isOtherDocument(this)) return mainGetElementById.apply(this, arguments);
    requireArgs(arguments, 1, 'Document.getElementById');
    elementId = String(elementId);
    if (elementId === '') return null;
    var ids = elementIds(this.__id);
    for (var i = 0; i < ids.length; i++) if (__axiom_getAttr(ids[i], 'id') === elementId) return wrap(ids[i]);
    return null;
  });

  method(DOMImplementation.prototype, 'createHTMLDocument', function (title) {
    var id = __axiom_createDocument() | 0;
    registerDocument(id, 'html', 'text/html', 'about:blank');
    function append(parent, child) { check(__axiom_insertBefore(parent, child, -1)); return child; }
    append(id, __axiom_createDocumentType('html', '', ''));
    var html = append(id, __axiom_createElement('html'));
    var head = append(html, __axiom_createElement('head'));
    if (title !== undefined) {
      append(append(head, __axiom_createElement('title')), __axiom_createTextNode(String(title)));
    }
    append(html, __axiom_createElement('body'));
    return wrap(id);
  });
  method(DOMImplementation.prototype, 'createDocument', function (namespace, qualifiedName, doctype) {
    requireArgs(arguments, 2, 'DOMImplementation.createDocument');
    var ns = optNamespace(namespace);
    var qualified = qualifiedName === null ? '' : String(qualifiedName);
    if (doctype !== undefined && doctype !== null && !(doctype instanceof DocumentType)) {
      throw new TypeError("Failed to execute 'createDocument' on 'DOMImplementation': parameter 3 is not of type 'DocumentType'.");
    }
    var element = -1;
    if (qualified !== '') {
      var r = __axiom_createElementNS(ns, qualified);
      if (typeof r === 'string') throw domError(r);
      element = r;
    }
    var id = __axiom_createDocument() | 0;
    registerDocument(id, 'xml', ns === HTML_NS ? 'application/xhtml+xml' :
      ns === SVG_NS ? 'image/svg+xml' : 'application/xml', 'about:blank');
    if (doctype) check(__axiom_insertBefore(id, doctype.__id, -1));
    if (element >= 0) check(__axiom_insertBefore(id, element, -1));
    return wrap(id);
  });

  // Document attributes of inert documents; interfaces_prelude.js routes the realm
  // document's accessors here when they are called on another document.
  function htmlChildOf(id, names) {
    if (id < 0) return -1;
    var kids = childIds(id);
    for (var i = 0; i < kids.length; i++) if (isHtmlNamed(kids[i], names)) return kids[i];
    return -1;
  }
  function htmlRootOf(d) { var r = rootElementOf(d); return r >= 0 && isHtmlNamed(r, ['html']) ? r : -1; }
  function titleElementOf(d) {
    var root = rootElementOf(d);
    if (root >= 0 && __axiom_namespaceURI(root) === SVG_NS && String(__axiom_tagName(root)) === 'svg') {
      var kids = childIds(root);
      for (var i = 0; i < kids.length; i++) {
        if (typeOfId(kids[i]) === 1 && __axiom_namespaceURI(kids[i]) === SVG_NS &&
            String(__axiom_tagName(kids[i])) === 'title') return kids[i];
      }
      return -1;
    }
    var titles = elementIds(d).filter(function (e) { return isHtmlNamed(e, ['title']); });
    return titles.length ? titles[0] : -1;
  }
  function infoOf(d) { return docInfo[d.__id | 0]; }
  function ignore() {}
  var DOCUMENT_FALLBACKS = {
    documentElement: { get: function () { return wrap(rootElementOf(this.__id)); } },
    head: { get: function () { return wrap(htmlChildOf(htmlRootOf(this.__id), ['head'])); } },
    body: { get: function () { return wrap(htmlChildOf(htmlRootOf(this.__id), ['body', 'frameset'])); } },
    title: {
      get: function () {
        var t = titleElementOf(this.__id);
        return t < 0 ? '' : textOf(t).replace(/[\t\n\f\r ]+/g, ' ').replace(/^ | $/g, '');
      },
      set: function (v) {
        var t = titleElementOf(this.__id);
        if (t < 0) {
          var head = htmlChildOf(htmlRootOf(this.__id), ['head']);
          if (head < 0) return;
          t = __axiom_createElement('title');
          check(__axiom_insertBefore(head, t, -1));
        }
        __axiom_setText(t, String(v));
      }
    },
    readyState: { get: function () { return 'complete'; } },
    currentScript: { get: function () { return null; } },
    defaultView: { get: function () { return null; } },
    activeElement: {
      get: function () {
        var body = htmlChildOf(htmlRootOf(this.__id), ['body', 'frameset']);
        return wrap(body >= 0 ? body : rootElementOf(this.__id));
      }
    },
    scrollingElement: {
      get: function () {
        if (infoOf(this).quirks) return wrap(htmlChildOf(htmlRootOf(this.__id), ['body']));
        return wrap(rootElementOf(this.__id));
      }
    },
    URL: { get: function () { return infoOf(this).url; } },
    documentURI: { get: function () { return infoOf(this).url; } },
    referrer: { get: function () { return ''; } },
    contentType: { get: function () { return infoOf(this).contentType; } },
    compatMode: { get: function () { return infoOf(this).quirks ? 'BackCompat' : 'CSS1Compat'; } },
    characterSet: { get: function () { return 'UTF-8'; } },
    charset: { get: function () { return 'UTF-8'; } },
    inputEncoding: { get: function () { return 'UTF-8'; } },
    lastModified: {
      get: function () {
        var d = new Date();
        function two(n) { return (n < 10 ? '0' : '') + n; }
        return two(d.getMonth() + 1) + '/' + two(d.getDate()) + '/' + d.getFullYear() + ' ' +
          two(d.getHours()) + ':' + two(d.getMinutes()) + ':' + two(d.getSeconds());
      }
    },
    // Documents without a browsing context are cookie-averse and have no domain.
    cookie: { get: function () { return ''; }, set: ignore },
    domain: {
      get: function () { return ''; },
      set: function () { throw domError('SecurityError', "Failed to set the 'domain' property on 'Document': Assignment is forbidden for sandboxed iframes."); }
    },
    designMode: { get: function () { return 'off'; }, set: ignore },
    dir: {
      get: function () {
        var root = htmlRootOf(this.__id);
        var v = root < 0 ? null : __axiom_getAttr(root, 'dir');
        v = v === null ? '' : asciiLower(v);
        return v === 'ltr' || v === 'rtl' || v === 'auto' ? v : '';
      },
      set: function (v) {
        var root = htmlRootOf(this.__id);
        if (root >= 0) __axiom_setAttr(root, 'dir', String(v));
      }
    },
    forms: {
      get: function () {
        var id = this.__id | 0;
        return live(HTMLCollection, 'doc-forms|' + id, function () {
          return elementIds(id).filter(function (e) { return isHtmlNamed(e, ['form']); });
        });
      }
    }
  };
  Object.defineProperty(global, '__axiom_otherDocuments', {
    value: { is: isOtherDocument, fallbacks: DOCUMENT_FALLBACKS },
    configurable: true
  });

  // ---------------------------------------------------------------------------
  // DOMParser and XMLSerializer (DOM Parsing)
  // ---------------------------------------------------------------------------
  var XML_TYPES = ['text/xml', 'application/xml', 'application/xhtml+xml', 'image/svg+xml'];
  var DOMParser = iface('DOMParser', null, function DOMParser() {
    if (!(this instanceof DOMParser)) throw new TypeError("Constructor DOMParser requires 'new'");
  });
  method(DOMParser.prototype, 'parseFromString', function (string, type) {
    requireArgs(arguments, 2, 'DOMParser.parseFromString');
    string = String(string);
    type = String(type);
    var id, info;
    if (type === 'text/html') {
      id = __axiom_parseHtmlDocument(string) | 0;
      info = registerDocument(id, 'html', type, String(doc.URL));
      var doctypes = childIds(id).filter(function (c) { return typeOfId(c) === 10; });
      info.quirks = !doctypes.length || asciiLower(__axiom_tagName(doctypes[0])) !== 'html';
      return wrap(id);
    }
    if (XML_TYPES.indexOf(type) >= 0) {
      id = __axiom_parseXmlDocument(string) | 0;
      registerDocument(id, 'xml', type, String(doc.URL));
      return wrap(id);
    }
    throw new TypeError("Failed to execute 'parseFromString' on 'DOMParser': The provided value '" + type +
      "' is not a valid enum value of type DOMParserSupportedType.");
  });

  var VOID_ELEMENTS = ['area', 'base', 'basefont', 'bgsound', 'br', 'col', 'embed', 'frame', 'hr', 'img',
    'input', 'keygen', 'link', 'meta', 'param', 'source', 'track', 'wbr'];
  function escapeXml(s, attribute) {
    s = String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    return attribute ? s.replace(/"/g, '&quot;') : s;
  }
  // `prefixes` maps in-scope prefixes ('' for the default namespace) to namespaces.
  function serializeXml(id, prefixes, out) {
    switch (typeOfId(id)) {
      case 1: {
        var ns = __axiom_namespaceURI(id), prefix = __axiom_prefix(id);
        var local = String(__axiom_tagName(id));
        var qname = prefix === null ? local : prefix + ':' + local;
        var scope = Object.create(prefixes);
        var attrs = __axiom_attributes(id), declared = [], rest = [];
        attrs.forEach(function (a) {
          if (a[0] === XMLNS_NS) {
            scope[a[1] === null ? '' : a[2]] = a[3];
            declared.push(a);
          } else {
            rest.push(a);
          }
        });
        var key = prefix === null ? '' : prefix;
        var decl = scope[key] !== (ns === null ? '' : ns);
        if (decl) scope[key] = ns === null ? '' : ns;
        out.push('<' + qname);
        if (decl) out.push(' xmlns' + (prefix === null ? '' : ':' + prefix) + '="' + escapeXml(ns === null ? '' : ns, true) + '"');
        declared.forEach(function (a) {
          out.push(' ' + (a[1] === null ? a[2] : a[1] + ':' + a[2]) + '="' + escapeXml(a[3], true) + '"');
        });
        rest.forEach(function (a) {
          var p = a[1];
          if (a[0] === XML_NS) p = 'xml';
          else if (a[0] !== null && (p === null || scope[p] !== a[0])) {
            if (p === null) {
              var n = 1;
              while (scope['ns' + n] !== undefined) n++;
              p = 'ns' + n;
            }
            scope[p] = a[0];
            out.push(' xmlns:' + p + '="' + escapeXml(a[0], true) + '"');
          }
          out.push(' ' + (p === null || a[0] === null ? a[2] : p + ':' + a[2]) + '="' + escapeXml(a[3], true) + '"');
        });
        var kids = isHtmlNamed(id, ['template']) ? childIds(wrap(id).content.__id) : childIds(id);
        if (!kids.length) {
          if (ns !== HTML_NS) out.push('/>');
          else if (VOID_ELEMENTS.indexOf(local) >= 0) out.push(' />');
          else out.push('></' + qname + '>');
          return;
        }
        out.push('>');
        kids.forEach(function (k) { serializeXml(k, scope, out); });
        out.push('</' + qname + '>');
        return;
      }
      case 3: out.push(escapeXml(textOf(id), false)); return;
      case 4: out.push('<![CDATA[' + textOf(id) + ']]>'); return;
      case 7: out.push('<?' + String(__axiom_tagName(id)) + ' ' + textOf(id) + '?>'); return;
      case 8: out.push('<!--' + textOf(id) + '-->'); return;
      case 10: {
        var dt = wrap(id);
        out.push('<!DOCTYPE ' + dt.name);
        if (dt.publicId) out.push(' PUBLIC "' + dt.publicId + '"');
        else if (dt.systemId) out.push(' SYSTEM');
        if (dt.systemId) out.push(' "' + dt.systemId + '"');
        out.push('>');
        return;
      }
      case 9: case 11:
        childIds(id).forEach(function (k) { serializeXml(k, prefixes, out); });
        return;
      default:
    }
  }
  var XMLSerializer = iface('XMLSerializer', null, function XMLSerializer() {
    if (!(this instanceof XMLSerializer)) throw new TypeError("Constructor XMLSerializer requires 'new'");
  });
  method(XMLSerializer.prototype, 'serializeToString', function (root) {
    requireArgs(arguments, 1, 'XMLSerializer.serializeToString');
    requireNode(root, 'XMLSerializer.serializeToString');
    if (isAttr(root)) return '';
    var out = [], prefixes = Object.create(null);
    prefixes[''] = '';
    prefixes.xml = XML_NS;
    serializeXml(idOf(root), prefixes, out);
    return out.join('');
  });

  // ---------------------------------------------------------------------------
  // Events: "get the parent", passive defaults, event handler content attributes,
  // document.createEvent and click()
  // ---------------------------------------------------------------------------
  var events = global.__axiom_events;
  // "Get the parent": a slotted node's slot, a shadow root's host (unless the event is not
  // composed and started in that shadow tree), otherwise the parent node.
  events.getParent = function (t, type, event) {
    if (t === doc) return type === 'load' ? null : global;
    if (!(t instanceof Node) || isAttr(t)) return null;
    var id = t.__id | 0;
    if (!__axiom_hasShadowTrees()) return wrap(parentOf(id));
    var slot = __axiom_assignedSlot(id) | 0;
    if (slot >= 0) return wrap(slot);
    var p = parentOf(id);
    if (p >= 0) return wrap(p);
    var host = hostOf(id);
    if (host < 0) return null;
    var first = event && event.path.length ? event.path[0].item : null;
    if (event && !event.composed && first instanceof Node && rootOf(idOf(first)) === id) return null;
    return wrap(host);
  };
  function eventNodeId(t) { return t === doc ? docId() : t instanceof Node && !isAttr(t) ? t.__id | 0 : -1; }
  events.shadow = {
    active: function () { return !!__axiom_hasShadowTrees(); },
    isNode: function (t) { return t instanceof Node; },
    root: function (t) { var id = eventNodeId(t); return id < 0 ? null : wrap(rootOf(id)); },
    isShadowRoot: function (t) { var id = eventNodeId(t); return id >= 0 && hostOf(id) >= 0; },
    isClosedShadowRoot: function (t) {
      var id = eventNodeId(t);
      return id >= 0 && hostOf(id) >= 0 && (shadowFlags(id) & SHADOW_OPEN) === 0;
    },
    isAssigned: function (t) { var id = eventNodeId(t); return id >= 0 && (__axiom_assignedSlot(id) | 0) >= 0; },
    contains: function (a, b) {
      var ia = eventNodeId(a), ib = eventNodeId(b);
      return ia >= 0 && ib >= 0 && shadowIncludingContains(ia, ib);
    },
    retarget: function (a, b) {
      var ia = eventNodeId(a);
      return ia < 0 ? a : wrap(retargetId(ia, eventNodeId(b)));
    }
  };
  function documentElementId() { return __axiom_documentNode('documentElement') | 0; }
  function isHtmlNamed(id, names) {
    return typeOfId(id) === 1 && isHtml(id) && names.indexOf(String(__axiom_tagName(id))) >= 0;
  }
  // The body element: the first body or frameset child of the html element.
  function bodyElementId() {
    var root = documentElementId();
    if (root < 0 || !isHtmlNamed(root, ['html'])) return -1;
    var kids = childIds(root);
    for (var i = 0; i < kids.length; i++) if (isHtmlNamed(kids[i], ['body', 'frameset'])) return kids[i];
    return -1;
  }
  events.passiveByDefault = function (t) {
    if (t === doc) return true;
    if (!(t instanceof Node) || isAttr(t)) return false;
    return t.__id === documentElementId() || t.__id === bodyElementId();
  };
  events.added = function (t, type) {
    if (t instanceof Node && !isAttr(t) && t !== doc) __axiom_registerListener(t.__id, type);
  };

  var GLOBAL_HANDLER = Object.create(null);
  events.GLOBAL_HANDLERS.forEach(function (n) { GLOBAL_HANDLER[n] = true; });
  var WINDOW_REFLECTING = Object.create(null);
  events.WINDOW_REFLECTING.forEach(function (n) { WINDOW_REFLECTING[n] = true; });
  function contentAttribute(id, name, value) {
    var ns = __axiom_namespaceURI(id);
    if (ns !== HTML_NS && ns !== SVG_NS && ns !== MATHML_NS) return;
    if (WINDOW_REFLECTING[name] && isHtmlNamed(id, ['body', 'frameset'])) {
      events.contentAttributeChanged(global, name, value, null);
    } else if (GLOBAL_HANDLER[name]) {
      events.contentAttributeChanged(wrap(id), name, value, wrap(id));
    }
  }
  // Handlers from attributes the element already had (parser, cloning, markup) are
  // activated the first time the element's listeners are touched.
  var scannedForHandlers = Object.create(null);
  function scanElement(id) {
    if (scannedForHandlers[id]) return;
    scannedForHandlers[id] = true;
    __axiom_attributes(id).forEach(function (a) {
      if (a[0] === null && String(a[2]).substring(0, 2) === 'on') contentAttribute(id, String(a[2]), String(a[3]));
    });
  }
  events.scan = function (t) {
    if (t === global) {
      var body = bodyElementId();
      if (body >= 0) scanElement(body);
    } else if (t instanceof Node && !isAttr(t) && t !== doc && typeOf(t) === 1) {
      scanElement(t.__id);
    }
  };
  function syncHandler(id, local) {
    local = String(local);
    if (local.substring(0, 2) !== 'on' || typeOfId(id) !== 1) return;
    if (!scannedForHandlers[id]) { scanElement(id); return; }
    var row = findAttr(id, byNamespace(null, local));
    contentAttribute(id, local, row ? String(row[3]) : null);
  }
  var handlerSetAttr = __axiom_setAttr;
  global.__axiom_setAttr = function (id, name, value) {
    var r = handlerSetAttr(id, name, value);
    syncHandler(id, name);
    return r;
  };
  var handlerRemoveAttr = __axiom_removeAttr;
  global.__axiom_removeAttr = function (id, name) {
    var r = handlerRemoveAttr(id, name);
    syncHandler(id, name);
    return r;
  };
  var handlerSetAttrNS = __axiom_setAttrNS;
  global.__axiom_setAttrNS = function (id, ns, qname, value) {
    var r = handlerSetAttrNS(id, ns, qname, value);
    if (ns === null || ns === '') syncHandler(id, qname);
    return r;
  };
  var handlerRemoveAttrNS = __axiom_removeAttrNS;
  global.__axiom_removeAttrNS = function (id, ns, local) {
    var r = handlerRemoveAttrNS(id, ns, local);
    if (ns === null || ns === '') syncHandler(id, local);
    return r;
  };
  var handlerReplaceAttrNS = __axiom_replaceAttrNS;
  global.__axiom_replaceAttrNS = function (id, ns, prefix, local, value) {
    var r = handlerReplaceAttrNS(id, ns, prefix, local, value);
    if (ns === null) syncHandler(id, local);
    return r;
  };

  [HTMLElement, SVGElement, MathMLElement, Document].forEach(function (I) {
    events.defineHandlers(I.prototype, events.GLOBAL_HANDLERS);
  });
  events.defineHandlers(Document.prototype, ['onreadystatechange', 'onvisibilitychange']);
  [global.HTMLBodyElement, global.HTMLFrameSetElement].forEach(function (I) {
    events.defineHandlers(I.prototype, events.WINDOW_REFLECTING, function () { return global; });
  });

  // Range / Selection.  Selection owns at most one live Range in this browsing context;
  // inert documents intentionally expose no selection because they have no window.
  method(DP, 'createRange', function () { return newRangeFor(this); });
  var Selection = iface('Selection', null);
  var selection = Object.create(Selection.prototype);
  Object.defineProperty(selection, STATE, { value: { range: null, direction: 'none' } });
  function selectionState(value) {
    if (!(value instanceof Selection)) throw new TypeError('Illegal invocation');
    return value[STATE];
  }
  function selectedRange(value) { return selectionState(value).range; }
  getter(Selection.prototype, 'rangeCount', function () { return selectedRange(this) ? 1 : 0; });
  getter(Selection.prototype, 'anchorNode', function () { var r = selectedRange(this); return r ? r.startContainer : null; });
  getter(Selection.prototype, 'anchorOffset', function () { var r = selectedRange(this); return r ? r.startOffset : 0; });
  getter(Selection.prototype, 'focusNode', function () { var r = selectedRange(this); return r ? r.endContainer : null; });
  getter(Selection.prototype, 'focusOffset', function () { var r = selectedRange(this); return r ? r.endOffset : 0; });
  getter(Selection.prototype, 'isCollapsed', function () { var r = selectedRange(this); return !r || r.collapsed; });
  getter(Selection.prototype, 'type', function () { var r = selectedRange(this); return !r ? 'None' : r.collapsed ? 'Caret' : 'Range'; });
  method(Selection.prototype, 'getRangeAt', function (index) { requireArgs(arguments, 1, 'Selection.getRangeAt'); var r = selectedRange(this); if (!r || Number(index) !== 0) throw domError('IndexSizeError', 'There is no range at this index.'); return r; });
  method(Selection.prototype, 'removeAllRanges', function () { selectionState(this).range = null; selectionState(this).direction = 'none'; });
  method(Selection.prototype, 'empty', function () { this.removeAllRanges(); });
  method(Selection.prototype, 'addRange', function (range) { requireArgs(arguments, 1, 'Selection.addRange'); rangeState(range); var st = selectionState(this); st.range = range; st.direction = 'forward'; });
  method(Selection.prototype, 'collapse', function (node, offset) { requireArgs(arguments, 1, 'Selection.collapse'); if (node === null) return this.removeAllRanges(); var r = newRangeFor(node.ownerDocument || doc); r.setStart(node, offset === undefined ? 0 : offset); r.collapse(true); this.addRange(r); });
  method(Selection.prototype, 'collapseToStart', function () { var r = selectedRange(this); if (!r) throw domError('InvalidStateError', 'There is no range to collapse.'); r.collapse(true); });
  method(Selection.prototype, 'collapseToEnd', function () { var r = selectedRange(this); if (!r) throw domError('InvalidStateError', 'There is no range to collapse.'); r.collapse(false); });
  method(Selection.prototype, 'selectAllChildren', function (node) { requireArgs(arguments, 1, 'Selection.selectAllChildren'); var r = newRangeFor(node.ownerDocument || doc); r.selectNodeContents(node); this.addRange(r); });
  method(Selection.prototype, 'deleteFromDocument', function () { var r = selectedRange(this); if (r) r.deleteContents(); });
  method(Selection.prototype, 'toString', function () { var r = selectedRange(this); return r ? r.toString() : ''; });
  method(Selection.prototype, 'containsNode', function (node, allowPartial) { var r = selectedRange(this); if (!r) return false; requireNode(node, 'Selection.containsNode'); var s = rangeState(r), id = idOf(node), p = parentOf(id); if (p < 0) return false; var i = childIndex(p, id), starts = comparePoints(s.sc, s.so, p, i), ends = comparePoints(p, i + 1, s.ec, s.eo); return allowPartial ? starts < 0 && ends < 0 : starts <= 0 && ends <= 0; });
  method(DP, 'getSelection', function () { return this === doc ? selection : null; });
  expose('getSelection', function () { return selection; });

  // document.createEvent's interface table (DOM §4.5), matched ASCII case-insensitively.
  var CREATE_EVENT = Object.create(null);
  [['beforeunloadevent', 'BeforeUnloadEvent'], ['compositionevent', 'CompositionEvent'],
    ['customevent', 'CustomEvent'], ['devicemotionevent', 'DeviceMotionEvent'],
    ['deviceorientationevent', 'DeviceOrientationEvent'], ['dragevent', 'DragEvent'], ['event', 'Event'],
    ['events', 'Event'], ['focusevent', 'FocusEvent'], ['hashchangeevent', 'HashChangeEvent'],
    ['htmlevents', 'Event'], ['keyboardevent', 'KeyboardEvent'], ['messageevent', 'MessageEvent'],
    ['mouseevent', 'MouseEvent'], ['mouseevents', 'MouseEvent'], ['storageevent', 'StorageEvent'],
    ['svgevents', 'Event'], ['textevent', 'TextEvent'], ['uievent', 'UIEvent'], ['uievents', 'UIEvent']
  ].forEach(function (e) { CREATE_EVENT[e[0]] = e[1]; });
  method(DP, 'createEvent', function (interfaceName) {
    requireArgs(arguments, 1, 'Document.createEvent');
    var name = CREATE_EVENT[asciiLower(String(interfaceName))];
    var F = name ? global[name] : undefined;
    if (typeof F !== 'function') {
      throw domError('NotSupportedError', "The provided event type ('" + interfaceName + "') is invalid.");
    }
    return events.createEvent(F);
  });

  // click(): a synthetic, untrusted click (HTML §6.5.1). Disabled form controls ignore it.
  // Activation behavior (form controls, labels) comes from the forms prelude.
  var clickInProgress = Object.create(null);
  method(HTMLElement.prototype, 'click', function click() {
    var id = idOf(this);
    if (isHtmlNamed(id, ['button', 'input', 'select', 'textarea']) && __axiom_getAttr(id, 'disabled') !== null) return;
    if (clickInProgress[id]) return;
    clickInProgress[id] = true;
    try {
      var activation = global.__axiom_activation;
      var state = activation ? activation.pre(id) : null;
      var ev = new global.PointerEvent('click', { bubbles: true, cancelable: true, composed: true, view: global });
      var notCanceled = EventTarget.prototype.dispatchEvent.call(this, ev);
      if (activation) activation.post(state, notCanceled);
    } finally {
      delete clickInProgress[id];
    }
  });

  // Window named properties (HTML 7.2.2.3): elements with an id are reachable as globals
  // through a named properties object between Window.prototype and EventTarget.prototype.
  // Own properties of the global and of Window.prototype take precedence.
  (function () {
    var WP = Object.getPrototypeOf(global);
    var named = function (key) {
      if (typeof key !== 'string' || key === '') return null;
      var id = __axiom_getElementById(key) | 0;
      return id < 0 ? null : id;
    };
    var props = new Proxy(Object.create(Object.getPrototypeOf(WP)), {
      has: function (t, key) { return named(key) !== null || Reflect.has(t, key); },
      get: function (t, key, receiver) {
        var n = named(key);
        return n !== null ? wrap(n) : Reflect.get(t, key, receiver);
      },
      getOwnPropertyDescriptor: function (t, key) {
        var n = named(key);
        if (n === null) return Reflect.getOwnPropertyDescriptor(t, key);
        return { value: wrap(n), writable: true, enumerable: false, configurable: true };
      },
      defineProperty: function () { return false; },
      deleteProperty: function (t, key) { return named(key) === null && Reflect.deleteProperty(t, key); }
    });
    Object.setPrototypeOf(WP, props);
  })();
})(this);
