// Runs last. The other preludes define document attributes on the realm's one document;
// browsers expose them as accessors on Document.prototype, and pages rely on that
// (`Object.getOwnPropertyDescriptor(Document.prototype, 'cookie').get.call(document)`).
// Move them there. `location` stays own ([LegacyUnforgeable]), as does the node id.
// Called on an inert document (createHTMLDocument, DOMParser, …) they use that
// document's fallbacks from dom_prelude.js; anything else is an illegal invocation.
(function (global) {
  'use strict';
  var doc = global.document;
  var DP = global.Document.prototype;
  var others = global.__axiom_otherDocuments;
  var KEEP = ['location', '__id'];
  function guard(name, kind, fn) {
    return function () {
      if (this === doc) return fn.apply(doc, arguments);
      var fallback = others.is(this) ? others.fallbacks[name] : null;
      if (fallback && fallback[kind]) return fallback[kind].apply(this, arguments);
      throw new TypeError('Illegal invocation');
    };
  }
  Object.getOwnPropertyNames(doc).forEach(function (name) {
    var d = Object.getOwnPropertyDescriptor(doc, name);
    if (KEEP.indexOf(name) >= 0 || !d.configurable || Object.getOwnPropertyDescriptor(DP, name)) return;
    var moved = { enumerable: d.enumerable, configurable: true };
    if (d.get || d.set) {
      if (d.get) moved.get = guard(name, 'get', d.get);
      if (d.set) moved.set = guard(name, 'set', d.set);
    } else {
      moved.value = d.value;
      moved.writable = d.writable;
    }
    Object.defineProperty(DP, name, moved);
    delete doc[name];
  });
})(this);
