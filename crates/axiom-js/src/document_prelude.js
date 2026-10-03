// Document lifecycle surface: document.readyState / currentScript / body / head /
// documentElement, readystatechange / DOMContentLoaded / load dispatch (driven by the
// engine), element load / error events and HTML attribute reflection. Listeners and
// dispatch are the events prelude's.
// document.write / writeln are not supported: they log and do nothing.
(function (global) {
  'use strict';
  var doc = global.document;
  var events = global.__axiom_events;
  global.__axiom_currentScriptId = -1;

  function nodeRef(n) { return n < 0 ? null : new ElementRef(n); }

  Object.defineProperty(doc, 'readyState', {
    get: function () { return String(__axiom_readyState()); },
    enumerable: true,
    configurable: true
  });
  Object.defineProperty(doc, 'currentScript', {
    get: function () { return nodeRef(global.__axiom_currentScriptId | 0); },
    enumerable: true,
    configurable: true
  });
  ['body', 'head', 'documentElement'].forEach(function (name) {
    Object.defineProperty(doc, name, {
      get: function () { return nodeRef(__axiom_documentNode(name)); },
      enumerable: true,
      configurable: true
    });
  });
  doc.write = doc.writeln = function () {
    __axiom_log('document.write is not supported; the call was ignored');
  };

  global.__axiom_fireDocumentEvent = function (type, bubbles) {
    events.fire(doc, String(type), { bubbles: !!bubbles });
  };
  // The window's load event targets the document (the legacy target override).
  global.__axiom_fireWindowEvent = function (type) {
    type = String(type);
    events.dispatchTrusted(global, new Event(type), type === 'load');
  };
  // Non-bubbling element event (resource load / error).
  global.__axiom_fireElementEvent = function (id, type) {
    events.fire(new ElementRef(id), String(type));
  };
  // CSP violation: at the element while it is connected, otherwise at the document.
  global.__axiom_fireCspViolation = function (id, init) {
    var el = id >= 0 ? new ElementRef(id) : null;
    var target = el && el.isConnected ? el : doc;
    init.bubbles = true;
    init.composed = true;
    events.dispatchTrusted(target, new SecurityPolicyViolationEvent('securitypolicyviolation', init));
  };

  var HP = global.HTMLElement.prototype;

  ['rel', 'type', 'name', 'content', 'title', 'lang', 'dir', 'charset', 'media'].forEach(function (name) {
    Object.defineProperty(HP, name, {
      get: function () { var v = __axiom_getAttr(this.__id, name); return v === null ? '' : String(v); },
      set: function (v) { __axiom_setAttr(this.__id, name, String(v)); },
      configurable: true
    });
  });
  // URL-valued reflection: the resolved URL (as in the DOM), the raw value if unresolvable.
  ['src', 'href'].forEach(function (name) {
    Object.defineProperty(HP, name, {
      get: function () {
        var v = __axiom_getAttr(this.__id, name);
        if (v === null) return '';
        var resolved = __axiom_resolveUrl(String(v));
        return resolved === null ? String(v) : String(resolved);
      },
      set: function (v) { __axiom_setAttr(this.__id, name, String(v)); }
    });
  });
  ['async', 'defer', 'hidden'].forEach(function (name) {
    Object.defineProperty(HP, name, {
      get: function () { return __axiom_getAttr(this.__id, name) !== null; },
      set: function (v) {
        if (v) __axiom_setAttr(this.__id, name, '');
        else __axiom_removeAttr(this.__id, name);
      },
      configurable: true
    });
  });

  // spellcheck and translate inherit the nearest ancestor's explicit state (HTML §6.8.4,
  // §3.2.6.9); both default to true, as in Chrome.
  function inheritedFlag(el, name, on, off) {
    for (var n = el; n && n.nodeType === 1; n = n.parentNode) {
      var v = n.getAttribute(name);
      if (v === null) continue;
      v = v.toLowerCase();
      if (on.indexOf(v) >= 0) return true;
      if (off.indexOf(v) >= 0) return false;
    }
    return true;
  }
  Object.defineProperty(HP, 'spellcheck', {
    get: function () { return inheritedFlag(this, 'spellcheck', ['', 'true'], ['false']); },
    set: function (v) { __axiom_setAttr(this.__id, 'spellcheck', v ? 'true' : 'false'); },
    configurable: true
  });
  Object.defineProperty(HP, 'translate', {
    get: function () { return inheritedFlag(this, 'translate', ['', 'yes'], ['no']); },
    set: function (v) { __axiom_setAttr(this.__id, 'translate', v ? 'yes' : 'no'); },
    configurable: true
  });
  Object.defineProperty(HP, 'draggable', {
    get: function () {
      var v = __axiom_getAttr(this.__id, 'draggable');
      if (v !== null && String(v).toLowerCase() === 'true') return true;
      if (v !== null && String(v).toLowerCase() === 'false') return false;
      var t = this.localName;
      return t === 'img' || (t === 'a' && this.hasAttribute('href'));
    },
    set: function (v) { __axiom_setAttr(this.__id, 'draggable', v ? 'true' : 'false'); },
    configurable: true
  });
  Object.defineProperty(HP, 'autocapitalize', {
    get: function () {
      var v = __axiom_getAttr(this.__id, 'autocapitalize');
      if (v === null) return '';
      v = String(v).toLowerCase();
      if (v === 'off' || v === 'none') return 'none';
      if (v === 'words' || v === 'characters') return v;
      return 'sentences';
    },
    set: function (v) { __axiom_setAttr(this.__id, 'autocapitalize', String(v)); },
    configurable: true
  });
  var ENTER_KEY_HINTS = ['enter', 'done', 'go', 'next', 'previous', 'search', 'send'];
  Object.defineProperty(HP, 'enterKeyHint', {
    get: function () {
      var v = __axiom_getAttr(this.__id, 'enterkeyhint');
      v = v === null ? '' : String(v).toLowerCase();
      return ENTER_KEY_HINTS.indexOf(v) >= 0 ? v : '';
    },
    set: function (v) { __axiom_setAttr(this.__id, 'enterkeyhint', String(v)); },
    configurable: true
  });

  // innerText without consulting layout: collapsed text with <br> and block boundaries as
  // line breaks, <pre> kept verbatim, and script / style / template / hidden content
  // skipped.
  var BLOCKS = ['address', 'article', 'aside', 'blockquote', 'dd', 'details', 'dialog', 'div',
    'dl', 'dt', 'fieldset', 'figcaption', 'figure', 'footer', 'form', 'h1', 'h2', 'h3', 'h4',
    'h5', 'h6', 'header', 'hr', 'li', 'main', 'nav', 'ol', 'p', 'pre', 'section', 'summary',
    'table', 'tr', 'ul'];
  var SKIPPED = ['script', 'style', 'template', 'noscript', 'head', 'title'];
  function innerText(el) {
    var out = [];
    (function walk(node) {
      for (var c = node.firstChild; c; c = c.nextSibling) {
        if (c.nodeType === 3) {
          out.push(c.data.replace(/[\t\n\f\r ]+/g, ' '));
        } else if (c.nodeType === 1) {
          var t = c.localName;
          if (t === 'br') { out.push('\n'); continue; }
          if (SKIPPED.indexOf(t) >= 0 || c.hasAttribute('hidden')) continue;
          var block = BLOCKS.indexOf(t) >= 0;
          if (block) out.push('\n');
          if (t === 'pre') out.push(c.textContent);
          else walk(c);
          if (block) out.push('\n');
        }
      }
    })(el);
    return out.join('').replace(/ *\n */g, '\n').replace(/\n{2,}/g, '\n')
      .replace(/^[\n ]+|[\n ]+$/g, '');
  }
  Object.defineProperty(HP, 'innerText', {
    get: function () { return innerText(this); },
    set: function (v) {
      var self = this;
      while (self.firstChild) self.removeChild(self.firstChild);
      String(v).split(/\r\n|\r|\n/).forEach(function (line, i) {
        if (i) self.appendChild(doc.createElement('br'));
        if (line) self.appendChild(doc.createTextNode(line));
      });
    },
    configurable: true
  });

  // ---------------------------------------------------------------------------
  // Nested browsing contexts. Child node IDs are intentionally never converted
  // into ElementRef: they are scoped to a different DOM arena. These small proxy
  // objects carry both the embedding iframe ID and child-local node ID on every
  // host call, which makes an accidental cross-realm ID collision impossible.
  // ---------------------------------------------------------------------------
  function frameInfo(el) { return __axiom_frameInfo(el.__id | 0); }
  function sameOriginFrame(el) {
    var info = frameInfo(el);
    return info && info[2] ? info : null;
  }
  function foreignNode(frame, id) {
    if (id < 0) return null;
    var node = Object.create(ForeignNode.prototype);
    Object.defineProperty(node, '__axiom_frame', { value: frame, configurable: false });
    Object.defineProperty(node, '__axiom_foreign_id', { value: id, configurable: false });
    return node;
  }
  function ForeignNode() { throw new TypeError('Illegal constructor'); }
  Object.defineProperty(ForeignNode.prototype, 'textContent', {
    get: function () { return String(__axiom_frameNodeText(this.__axiom_frame, this.__axiom_foreign_id)); },
    enumerable: true
  });
  Object.defineProperty(ForeignNode.prototype, 'tagName', {
    get: function () { return String(__axiom_frameNodeTagName(this.__axiom_frame, this.__axiom_foreign_id)); },
    enumerable: true
  });
  Object.defineProperty(ForeignNode.prototype, 'namespaceURI', {
    get: function () { return __axiom_frameNodeNamespaceUri(this.__axiom_frame, this.__axiom_foreign_id); },
    enumerable: true
  });
  Object.defineProperty(ForeignNode.prototype, 'nodeType', { value: 1, enumerable: true });

  function ForeignDocument(frame, info) {
    Object.defineProperty(this, '__axiom_frame', { value: frame, configurable: false });
    Object.defineProperty(this, '__axiom_document_id', { value: info[1], configurable: false });
    Object.defineProperty(this, '__axiom_url', { value: String(info[3]), configurable: false });
    Object.defineProperty(this, '__axiom_content_type', { value: String(info[4] || 'text/html'), configurable: false });
  }
  ForeignDocument.prototype.getElementById = function (id) {
    return foreignNode(this.__axiom_frame, __axiom_frameGetElementById(this.__axiom_frame, String(id)) | 0);
  };
  ForeignDocument.prototype.querySelector = function (selector) {
    return foreignNode(this.__axiom_frame, __axiom_frameQuerySelector(this.__axiom_frame, String(selector)) | 0);
  };
  ForeignDocument.prototype.createElement = function (localName) {
    var name = String(localName);
    // This realm proxy intentionally carries its own small XML-name check rather than
    // reaching into dom_prelude's closure. Full qualified-name validation remains on
    // the child DOM boundary once cross-realm mutation is broadened.
    if (!/^[A-Za-z_][A-Za-z0-9._:-]*$/.test(name)) throw new DOMException("'" + name + "' is not a valid element name.", 'InvalidCharacterError');
    return foreignNode(this.__axiom_frame, __axiom_frameCreateElement(this.__axiom_frame, name) | 0);
  };
  ['body', 'head', 'documentElement'].forEach(function (name) {
    Object.defineProperty(ForeignDocument.prototype, name, {
      get: function () { return foreignNode(this.__axiom_frame, __axiom_frameDocumentNode(this.__axiom_frame, name) | 0); },
      enumerable: true
    });
  });
  Object.defineProperty(ForeignDocument.prototype, 'URL', {
    get: function () { return this.__axiom_url; }, enumerable: true
  });
  Object.defineProperty(ForeignDocument.prototype, 'contentType', {
    get: function () { return this.__axiom_content_type; }, enumerable: true
  });

  function FrameWindowProxy(frame) {
    Object.defineProperty(this, '__axiom_frame', { value: frame, configurable: false });
  }
  Object.defineProperty(FrameWindowProxy.prototype, 'document', {
    get: function () {
      var info = __axiom_frameInfo(this.__axiom_frame);
      if (!info || !info[2]) throw new DOMException('Blocked a cross-origin frame access.', 'SecurityError');
      return new ForeignDocument(this.__axiom_frame, info);
    }, enumerable: true
  });
  Object.defineProperty(FrameWindowProxy.prototype, 'location', {
    get: function () {
      var info = __axiom_frameInfo(this.__axiom_frame);
      if (!info || !info[2]) throw new DOMException('Blocked a cross-origin frame access.', 'SecurityError');
      return { href: String(info[3]) };
    }, enumerable: true
  });
  FrameWindowProxy.prototype.postMessage = function () {
    if (arguments.length < 1) throw new TypeError("Failed to execute 'postMessage': 1 argument required.");
    var target = arguments.length > 1 ? arguments[1] : '/';
    if (target !== null && typeof target === 'object') target = target.targetOrigin === undefined ? '/' : target.targetOrigin;
    target = String(target);
    if (target !== '*' && target !== '/') {
      try { target = new global.URL(target).origin; }
      catch (_) { throw new DOMException("Invalid target origin '" + target + "'.", 'SyntaxError'); }
    }
    var payload;
    try { payload = JSON.stringify(arguments[0]); }
    catch (_) { throw new DOMException('The message could not be cloned.', 'DataCloneError'); }
    if (payload === undefined) throw new DOMException('The message could not be cloned.', 'DataCloneError');
    __axiom_framePostMessage(this.__axiom_frame, payload, target);
  };

  var IFP = global.HTMLIFrameElement && global.HTMLIFrameElement.prototype;
  if (IFP) {
    Object.defineProperty(IFP, 'contentDocument', {
      get: function () {
        var info = sameOriginFrame(this);
        return info ? new ForeignDocument(this.__id | 0, info) : null;
      }, enumerable: true, configurable: true
    });
    Object.defineProperty(IFP, 'contentWindow', {
      get: function () { return frameInfo(this) ? new FrameWindowProxy(this.__id | 0) : null; },
      enumerable: true, configurable: true
    });
  }
})(this);
