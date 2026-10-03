// Custom elements (HTML §4.13): window.customElements (define / get / getName /
// whenDefined / upgrade), constructible HTMLElement for registered classes, upgrades of
// existing and parser-created elements, and the connected / disconnected /
// attributeChanged reactions for script-driven mutations. Reactions run synchronously
// after the mutation that caused them.
// Not implemented: customized built-in elements (`extends` / `is`: defined but never
// upgraded), form-associated custom elements, ElementInternals, adoptedCallback and
// scoped registries.
(function (global) {
  'use strict';
  var doc = global.document;
  var HTML_NS = 'http://www.w3.org/1999/xhtml';
  var OldHTMLElement = global.HTMLElement;
  var ALREADY_CONSTRUCTED = {};

  var definitions = Object.create(null); // local name -> definition
  var byConstructor = new Map();
  var pendingWhenDefined = Object.create(null); // name -> { promise, resolve }
  var state = new WeakMap(); // element wrapper -> 'custom' | 'failed'
  var definedCount = 0;
  var defining = false;

  function domError(name, message) { return new DOMException(message, name); }
  function wrap(id) { return id < 0 ? null : new ElementRef(id); }
  function isElement(id) { return (__axiom_nodeType(id) | 0) === 1; }
  function localName(id) { return String(__axiom_tagName(id)); }
  function isHtml(id) { return __axiom_namespaceURI(id) === HTML_NS; }
  function isConnected(id) { var w = wrap(id); return w !== null && w.isConnected === true; }
  var RESERVED = ['annotation-xml', 'color-profile', 'font-face', 'font-face-src',
    'font-face-uri', 'font-face-format', 'font-face-name', 'missing-glyph'];
  function validName(n) {
    return /^[a-z][\-.0-9_a-z\u00B7\u00C0-\uFFFF]*$/.test(n) && n.indexOf('-') > 0 && RESERVED.indexOf(n) < 0;
  }
  function report(e) {
    if (typeof __axiom_reportError === 'function') __axiom_reportError(e);
  }
  function definitionFor(id) {
    if (!definedCount || !isElement(id) || !isHtml(id)) return null;
    var def = definitions[localName(id)];
    return def && !def.extends ? def : null;
  }
  // Elements of the inclusive subtree of `id`, in shadow-including tree order.
  function subtreeElements(id, out) {
    if (isElement(id)) {
      out.push(id);
      var shadow = __axiom_shadowRootInfo(id);
      if (shadow) subtreeElements(shadow[0], out);
    }
    var kids = __axiom_childNodes(id);
    for (var i = 0; i < kids.length; i++) subtreeElements(kids[i], out);
    return out;
  }
  // Connected elements named `name`, in the document and every connected shadow tree.
  function connectedNamed(name) {
    var roots = [__axiom_documentRoot() | 0].concat(Array.prototype.slice.call(__axiom_shadowRoots()));
    var out = [];
    roots.forEach(function (root) {
      Array.prototype.push.apply(out, Array.prototype.slice.call(__axiom_elementsByTagName(root, name)));
    });
    return out;
  }
  function markCustom(el) {
    state.set(el, 'custom');
    __axiom_setCustomElementDefined(el.__id);
  }
  function callback(el, name, args) {
    var def = byConstructor.get(el.constructor) || definitions[localName(el.__id)];
    var fn = def && def.callbacks[name];
    if (typeof fn !== 'function') return;
    try { fn.apply(el, args); } catch (e) { report(e); }
  }

  // HTML element constructors (HTML §3.2.3): `super()` from a registered class creates
  // the element, or returns the element being upgraded.
  function HTMLElement() {
    var newTarget = new.target;
    if (newTarget === undefined) {
      throw new TypeError("Failed to construct 'HTMLElement': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
    }
    var def = byConstructor.get(newTarget);
    if (!def) throw new TypeError('Illegal constructor');
    var proto = newTarget.prototype;
    if (def.construction.length === 0) {
      var el = wrap(nativeCreateElement(def.name));
      Object.setPrototypeOf(el, proto);
      markCustom(el);
      return el;
    }
    var candidate = def.construction[def.construction.length - 1];
    if (candidate === ALREADY_CONSTRUCTED) {
      throw domError('InvalidStateError', "Failed to construct 'HTMLElement': This instance is already constructed.");
    }
    Object.setPrototypeOf(candidate, proto);
    def.construction[def.construction.length - 1] = ALREADY_CONSTRUCTED;
    return candidate;
  }
  HTMLElement.prototype = OldHTMLElement.prototype;
  Object.defineProperty(HTMLElement.prototype, 'constructor', { value: HTMLElement, writable: true, configurable: true });
  Object.setPrototypeOf(HTMLElement, global.Element);
  Object.getOwnPropertyNames(global).forEach(function (name) {
    if (!/^HTML\w*Element$/.test(name)) return;
    var F = global[name];
    if (typeof F === 'function' && Object.getPrototypeOf(F) === OldHTMLElement) Object.setPrototypeOf(F, HTMLElement);
  });
  Object.defineProperty(global, 'HTMLElement', { value: HTMLElement, writable: true, configurable: true });

  // Upgrade (HTML §4.13.5): construct the definition over the existing element, then
  // replay its observed attributes and, when connected, connectedCallback.
  function upgrade(id, def) {
    var el = wrap(id);
    var s = state.get(el);
    if (s === 'custom' || s === 'failed') return;
    def.construction.push(el);
    try {
      var result = Reflect.construct(def.ctor, []);
      if (result !== el) {
        throw domError('InvalidStateError', 'Custom element constructor did not produce the upgraded element.');
      }
    } catch (e) {
      state.set(el, 'failed');
      report(e);
      return;
    } finally {
      def.construction.pop();
    }
    markCustom(el);
    if (def.callbacks.attributeChangedCallback) {
      __axiom_attributes(id).forEach(function (a) {
        if (a[0] === null && def.observed.indexOf(a[2]) >= 0) {
          callback(el, 'attributeChangedCallback', [a[2], null, a[3], null]);
        }
      });
    }
    if (isConnected(id)) callback(el, 'connectedCallback', []);
  }
  function tryUpgrade(id) {
    var def = definitionFor(id);
    if (def) upgrade(id, def);
  }

  // Tree-mutation reactions.
  function connected(ids) {
    ids.forEach(function (root) {
      if (!isConnected(root)) return;
      subtreeElements(root, []).forEach(function (id) {
        var el = wrap(id);
        var s = state.get(el);
        if (s === 'custom') callback(el, 'connectedCallback', []);
        else if (s === undefined) tryUpgrade(id);
      });
    });
  }
  function disconnected(elements) {
    elements.forEach(function (id) {
      var el = wrap(id);
      if (state.get(el) === 'custom') callback(el, 'disconnectedCallback', []);
    });
  }
  // Connected custom elements in the subtrees of `ids` (captured before a removal).
  function connectedCustom(ids) {
    var out = [];
    if (!definedCount) return out;
    ids.forEach(function (root) {
      if (root < 0 || !isConnected(root)) return;
      subtreeElements(root, []).forEach(function (id) {
        if (state.get(wrap(id)) === 'custom') out.push(id);
      });
    });
    return out;
  }
  function inserted(node) {
    return (__axiom_nodeType(node) | 0) === 11 ? Array.prototype.slice.call(__axiom_childNodes(node)) : [node];
  }

  var nativeCreateElement = __axiom_createElement;
  global.__axiom_createElement = function (name) {
    var def = definedCount ? definitions[String(name)] : null;
    if (!def || def.extends) return nativeCreateElement(name);
    try {
      var el = new def.ctor();
      return el.__id;
    } catch (e) {
      report(e);
      var id = nativeCreateElement(name);
      state.set(wrap(id), 'failed');
      return id;
    }
  };

  var insertBefore = __axiom_insertBefore;
  global.__axiom_insertBefore = function (parent, node, child) {
    if (!definedCount) return insertBefore(parent, node, child);
    var moved = connectedCustom([node]);
    var added = inserted(node);
    var err = insertBefore(parent, node, child);
    if (err !== null) return err;
    disconnected(moved);
    connected(added);
    return err;
  };
  var replaceChild = __axiom_replaceChild;
  global.__axiom_replaceChild = function (parent, node, child) {
    if (!definedCount) return replaceChild(parent, node, child);
    var gone = connectedCustom(node === child ? [node] : [child, node]);
    var added = inserted(node);
    var err = replaceChild(parent, node, child);
    if (err !== null) return err;
    disconnected(gone);
    connected(added);
    return err;
  };
  var removeChild = __axiom_removeChild;
  global.__axiom_removeChild = function (parent, child) {
    if (!definedCount) return removeChild(parent, child);
    var gone = connectedCustom([child]);
    var ok = removeChild(parent, child);
    if (ok) disconnected(gone);
    return ok;
  };
  // Replacing children wholesale (textContent, innerHTML, outerHTML, insertAdjacentHTML).
  function replacing(native, targetOf) {
    return function () {
      if (!definedCount) return native.apply(null, arguments);
      var target = targetOf.apply(null, arguments);
      var before = target >= 0 ? Array.prototype.slice.call(__axiom_childNodes(target)) : [];
      var gone = connectedCustom(before);
      var r = native.apply(null, arguments);
      var after = target >= 0 ? Array.prototype.slice.call(__axiom_childNodes(target)) : [];
      disconnected(gone.filter(function (id) { return !isConnected(id); }));
      connected(after.filter(function (id) { return before.indexOf(id) < 0; }));
      return r;
    };
  }
  function self(id) { return id; }
  function parent(id) { return __axiom_parentNode(id) | 0; }
  global.__axiom_setText = replacing(__axiom_setText, self);
  global.__axiom_setInnerHTML = replacing(__axiom_setInnerHTML, self);
  global.__axiom_setOuterHTML = replacing(__axiom_setOuterHTML, parent);
  global.__axiom_insertAdjacentHTML = replacing(__axiom_insertAdjacentHTML, function (id, where) {
    return /^(beforebegin|afterend)$/i.test(where) ? parent(id) : id;
  });

  // attributeChangedCallback for observed, non-namespaced attributes.
  function observing(id, local) {
    if (!definedCount) return null;
    var el = wrap(id);
    if (state.get(el) !== 'custom') return null;
    var def = byConstructor.get(el.constructor);
    return def && def.observed.indexOf(local) >= 0 ? el : null;
  }
  function attrValue(id, local) {
    var rows = __axiom_attributes(id);
    for (var i = 0; i < rows.length; i++) if (rows[i][0] === null && rows[i][2] === local) return rows[i][3];
    return null;
  }
  function attributeHook(native, localOf) {
    return function () {
      var id = arguments[0];
      var local = localOf.apply(null, arguments);
      var el = local === null ? null : observing(id, local);
      if (!el) return native.apply(null, arguments);
      var old = attrValue(id, local);
      var r = native.apply(null, arguments);
      var now = attrValue(id, local);
      if (old !== null || now !== null) callback(el, 'attributeChangedCallback', [local, old, now, null]);
      return r;
    };
  }
  function plainLocal(id, name) { return String(name); }
  function nsLocal(id, ns) {
    if (ns !== null && ns !== '' && ns !== undefined) return null;
    var q = String(arguments[2]);
    return q.substring(q.indexOf(':') + 1);
  }
  global.__axiom_setAttr = attributeHook(__axiom_setAttr, plainLocal);
  global.__axiom_removeAttr = attributeHook(__axiom_removeAttr, plainLocal);
  global.__axiom_setAttrNS = attributeHook(__axiom_setAttrNS, nsLocal);
  global.__axiom_removeAttrNS = attributeHook(__axiom_removeAttrNS, nsLocal);

  // Parser-created elements: the engine calls this after parser steps once a definition
  // exists.
  global.__axiom_upgradeCustomElements = function () {
    Object.keys(definitions).forEach(function (name) {
      var def = definitions[name];
      if (def.extends) return;
      connectedNamed(name).forEach(function (id) {
        if (isHtml(id) && state.get(wrap(id)) === undefined) upgrade(id, def);
      });
    });
  };

  // CustomElementRegistry.
  function CustomElementRegistry() { throw new TypeError('Illegal constructor'); }
  var CRP = CustomElementRegistry.prototype;
  function method(name, fn) {
    Object.defineProperty(CRP, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  method('define', function define(name, ctor, options) {
    if (arguments.length < 2) throw new TypeError("Failed to execute 'define' on 'CustomElementRegistry': 2 arguments required.");
    name = String(name);
    if (typeof ctor !== 'function' || !ctor.prototype) {
      throw new TypeError("Failed to execute 'define' on 'CustomElementRegistry': The provided value cannot be converted to a constructor.");
    }
    if (!validName(name)) {
      throw domError('SyntaxError', "Failed to execute 'define' on 'CustomElementRegistry': \"" + name + '" is not a valid custom element name');
    }
    if (definitions[name]) {
      throw domError('NotSupportedError', "Failed to execute 'define' on 'CustomElementRegistry': the name \"" + name + '" has already been used with this registry');
    }
    if (byConstructor.has(ctor)) {
      throw domError('NotSupportedError', "Failed to execute 'define' on 'CustomElementRegistry': this constructor has already been used with this registry");
    }
    if (defining) throw domError('NotSupportedError', 'A custom element is already being defined.');
    defining = true;
    var def;
    try {
      var proto = ctor.prototype;
      if (proto === null || (typeof proto !== 'object' && typeof proto !== 'function')) {
        throw new TypeError("Failed to execute 'define': The prototype is not an object.");
      }
      var callbacks = {};
      ['connectedCallback', 'disconnectedCallback', 'adoptedCallback', 'attributeChangedCallback'].forEach(function (n) {
        var v = proto[n];
        if (v !== undefined && typeof v !== 'function') {
          throw new TypeError("Failed to execute 'define': The '" + n + "' property is not a function.");
        }
        callbacks[n] = v;
      });
      var observed = [];
      if (callbacks.attributeChangedCallback) {
        var list = ctor.observedAttributes;
        if (list !== undefined && list !== null) observed = Array.from(list).map(String);
      }
      var ext = options && options.extends !== undefined ? String(options.extends) : null;
      def = { name: name, ctor: ctor, callbacks: callbacks, observed: observed, construction: [], extends: ext };
    } finally {
      defining = false;
    }
    definitions[name] = def;
    byConstructor.set(ctor, def);
    definedCount++;
    __axiom_customElementsDefined();
    if (!def.extends) {
      connectedNamed(name).forEach(function (id) {
        if (isHtml(id)) upgrade(id, def);
      });
    }
    var pending = pendingWhenDefined[name];
    if (pending) {
      delete pendingWhenDefined[name];
      pending.resolve(ctor);
    }
  });
  method('get', function get(name) {
    var def = definitions[String(name)];
    return def ? def.ctor : undefined;
  });
  method('getName', function getName(ctor) {
    var def = byConstructor.get(ctor);
    return def ? def.name : null;
  });
  method('whenDefined', function whenDefined(name) {
    name = String(name);
    if (!validName(name)) {
      return Promise.reject(domError('SyntaxError', '"' + name + '" is not a valid custom element name'));
    }
    if (definitions[name]) return Promise.resolve(definitions[name].ctor);
    var pending = pendingWhenDefined[name];
    if (!pending) {
      pending = pendingWhenDefined[name] = {};
      pending.promise = new Promise(function (resolve) { pending.resolve = resolve; });
    }
    return pending.promise;
  });
  method('upgrade', function upgrade_(root) {
    subtreeElements(root === doc ? __axiom_documentRoot() | 0 : root.__id, []).forEach(tryUpgrade);
  });
  Object.defineProperty(CRP, Symbol.toStringTag, { value: 'CustomElementRegistry', configurable: true });
  Object.defineProperty(global, 'CustomElementRegistry', { value: CustomElementRegistry, writable: true, configurable: true });
  var registry = Object.create(CRP);
  Object.defineProperty(global, 'customElements', {
    get: function () { return registry; },
    enumerable: true,
    configurable: true
  });
})(this);
