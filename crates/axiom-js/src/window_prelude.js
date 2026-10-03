// Window environment: self / parent / top / frames / opener, location (navigable),
// document.URL / documentURI / title / activeElement, performance and viewport metrics.
// The node tree surface lives in the DOM prelude.
// Axiom has no frames or popups: parent and top are the window itself, opener is null.
(function (global) {
  'use strict';
  var doc = global.document;

  global.self = global;
  global.parent = global;
  global.top = global;
  global.frames = global;
  global.opener = null;

  function documentId() { return __axiom_documentRoot() | 0; }

  // location: the document URL split into its components.
  function documentUrl() {
    var u = __axiom_resolveUrl('');
    return u === null ? 'about:blank' : String(u);
  }
  function splitUrl(href) {
    var m = /^([a-zA-Z][a-zA-Z0-9+.\-]*:)(?:\/\/([^\/?#]*))?([^?#]*)(\?[^#]*)?(#.*)?$/.exec(href);
    if (!m) return { protocol: '', host: '', pathname: href, search: '', hash: '' };
    var host = m[2] || '';
    var at = host.lastIndexOf('@');
    if (at >= 0) host = host.substring(at + 1);
    return {
      protocol: m[1].toLowerCase(),
      host: host,
      pathname: m[3],
      search: m[4] && m[4].length > 1 ? m[4] : '',
      hash: m[5] && m[5].length > 1 ? m[5] : ''
    };
  }
  // Navigations are queued for the browsing context, which applies them after the
  // current task (a fragment-only change stays in this document and fires hashchange).
  function Location() { throw new TypeError('Illegal constructor'); }
  Object.defineProperty(Location.prototype, Symbol.toStringTag, { value: 'Location', configurable: true });
  Object.defineProperty(global, 'Location', { value: Location, writable: true, configurable: true });
  var location = Object.create(Location.prototype);
  function locationNavigate(url, replace, error) {
    var resolved = __axiom_resolveUrl(String(url));
    if (resolved === null || !__axiom_navigate(String(resolved), !!replace)) {
      var message = "Failed to navigate: '" + url + "' is not a valid URL.";
      throw error === 'TypeError' ? new TypeError(message) : new DOMException(message, 'SyntaxError');
    }
  }
  function defineLocation(name, get, set) {
    Object.defineProperty(location, name, { get: get, set: set, enumerable: true, configurable: false });
  }
  function setComponent(name) {
    return function (value) {
      var current = documentUrl();
      var url = new global.URL(current);
      value = String(value);
      if (name === 'hash') {
        var fragment = value.charAt(0) === '#' ? value.substring(1) : value;
        url.hash = fragment;
        var target = fragment === '' ? url.href.replace(/#.*$/, '') + '#' : url.href;
        var fragmentOf = function (u) { var i = u.indexOf('#'); return i < 0 ? null : u.substring(i + 1); };
        if (fragmentOf(target) === fragmentOf(current)) return;
        locationNavigate(target, false);
        return;
      }
      url[name] = value;
      if (name === 'protocol' && url.protocol !== 'http:' && url.protocol !== 'https:') return;
      locationNavigate(url.href, false);
    };
  }
  defineLocation('href', documentUrl, function (v) { locationNavigate(v, false, 'TypeError'); });
  defineLocation('protocol', function () { return splitUrl(documentUrl()).protocol; }, setComponent('protocol'));
  defineLocation('host', function () { return splitUrl(documentUrl()).host; }, setComponent('host'));
  defineLocation('hostname', function () {
    var host = splitUrl(documentUrl()).host;
    if (host.charAt(0) === '[') return host.substring(0, host.indexOf(']') + 1);
    var colon = host.lastIndexOf(':');
    return colon >= 0 ? host.substring(0, colon) : host;
  }, setComponent('hostname'));
  defineLocation('port', function () {
    var host = splitUrl(documentUrl()).host;
    var close = host.lastIndexOf(']');
    var colon = host.lastIndexOf(':');
    return colon > close ? host.substring(colon + 1) : '';
  }, setComponent('port'));
  defineLocation('pathname', function () { return splitUrl(documentUrl()).pathname; }, setComponent('pathname'));
  defineLocation('search', function () { return splitUrl(documentUrl()).search; }, setComponent('search'));
  defineLocation('hash', function () { return splitUrl(documentUrl()).hash; }, setComponent('hash'));
  defineLocation('origin', function () {
    var parts = splitUrl(documentUrl());
    if (parts.protocol === 'http:' || parts.protocol === 'https:') {
      return parts.protocol + '//' + parts.host;
    }
    return 'null';
  });
  function locationMethod(name, fn) {
    Object.defineProperty(location, name, { value: fn, writable: false, enumerable: true, configurable: false });
  }
  locationMethod('assign', function assign(url) {
    if (arguments.length < 1) throw new TypeError("Failed to execute 'assign' on 'Location': 1 argument required");
    locationNavigate(url, false);
  });
  locationMethod('replace', function replace(url) {
    if (arguments.length < 1) throw new TypeError("Failed to execute 'replace' on 'Location': 1 argument required");
    locationNavigate(url, true);
  });
  locationMethod('reload', function reload() { __axiom_reload(); });
  locationMethod('toString', function toString() { return documentUrl(); });
  locationMethod('valueOf', function valueOf() { return this; });
  // `window.location = url` / `document.location = url` forward to href.
  function locationAccessor(target) {
    Object.defineProperty(target, 'location', {
      get: function () { return location; },
      set: function (v) { location.href = v; },
      enumerable: true,
      configurable: false
    });
  }
  locationAccessor(global);
  locationAccessor(doc);
  Object.defineProperty(doc, 'URL', { get: documentUrl, enumerable: true, configurable: true });
  Object.defineProperty(doc, 'documentURI', { get: documentUrl, enumerable: true, configurable: true });

  // Response metadata and document modes.
  function docGetter(name, get, set) {
    Object.defineProperty(doc, name, { get: get, set: set, enumerable: true, configurable: true });
  }
  function info(key) { return String(__axiom_documentInfo(key)); }
  docGetter('defaultView', function () { return global; });
  docGetter('referrer', function () { return info('referrer'); });
  docGetter('contentType', function () { return info('contentType'); });
  docGetter('compatMode', function () { return info('compatMode'); });
  ['characterSet', 'charset', 'inputEncoding'].forEach(function (name) {
    docGetter(name, function () { return info('characterSet'); });
  });
  // "MM/DD/YYYY hh:mm:ss" in local time; the current time when the server gave none.
  docGetter('lastModified', function () {
    var seconds = info('lastModified');
    var d = seconds === '' ? new Date() : new Date(Number(seconds) * 1000);
    function two(n) { return (n < 10 ? '0' : '') + n; }
    return two(d.getMonth() + 1) + '/' + two(d.getDate()) + '/' + d.getFullYear() + ' ' +
      two(d.getHours()) + ':' + two(d.getMinutes()) + ':' + two(d.getSeconds());
  });
  // Editing is not implemented; the mode is remembered so feature checks see their value.
  var designMode = 'off';
  docGetter('designMode', function () { return designMode; }, function (v) {
    v = String(v).toLowerCase();
    if (v === 'on' || v === 'off') designMode = v;
  });
  docGetter('dir', function () {
    var root = doc.documentElement;
    var v = root ? String(root.getAttribute('dir') || '').toLowerCase() : '';
    return v === 'ltr' || v === 'rtl' || v === 'auto' ? v : '';
  }, function (v) {
    if (doc.documentElement) doc.documentElement.setAttribute('dir', String(v));
  });
  // document.domain: the host of a tuple origin, '' for an opaque one. Documents are
  // origin-keyed (no frames share an agent cluster), so a valid assignment changes
  // nothing, as in Chrome.
  function documentHost() {
    var parts = splitUrl(documentUrl());
    if (parts.protocol !== 'http:' && parts.protocol !== 'https:') return '';
    return location.hostname.toLowerCase();
  }
  function domainError(message) {
    return new DOMException("Failed to set the 'domain' property on 'Document': " + message, 'SecurityError');
  }
  docGetter('domain', documentHost, function (v) {
    var host = documentHost();
    if (host === '') throw domainError('Assignment is forbidden for sandboxed iframes.');
    var value = String(v).toLowerCase();
    var ip = /^\[|^\d+\.\d+\.\d+\.\d+$/.test(host);
    if (value === host) return;
    if (ip || value === '' || host.slice(-(value.length + 1)) !== '.' + value) {
      throw domainError("'" + v + "' is not a suffix of '" + host + "'.");
    }
    if (value.indexOf('.') < 0) throw domainError("'" + v + "' is a top-level domain.");
  });

  // document.title: the first <title> element's text with ASCII whitespace collapsed.
  Object.defineProperty(doc, 'title', {
    get: function () {
      var ids = __axiom_elementsByTagName(documentId(), 'title');
      if (!ids.length) return '';
      return String(__axiom_getText(ids[0])).replace(/[\t\n\f\r ]+/g, ' ').replace(/^ | $/g, '');
    },
    set: function (v) {
      var ids = __axiom_elementsByTagName(documentId(), 'title');
      if (ids.length) __axiom_setText(ids[0], String(v));
    },
    enumerable: true,
    configurable: true
  });

  // performance: the monotonic clock behind event.timeStamp (0.1 ms resolution), with
  // the realm's creation as its time origin.
  var timeOrigin = Date.now() - __axiom_now();
  function Performance() { throw new TypeError('Illegal constructor'); }
  Performance.prototype = Object.create(global.EventTarget.prototype);
  Object.setPrototypeOf(Performance, global.EventTarget);
  Object.defineProperty(Performance.prototype, 'constructor', { value: Performance, writable: true, configurable: true });
  Object.defineProperty(Performance.prototype, Symbol.toStringTag, { value: 'Performance', configurable: true });
  Object.defineProperty(Performance.prototype, 'now', {
    value: function now() { return __axiom_now(); }, writable: true, enumerable: true, configurable: true
  });
  Object.defineProperty(Performance.prototype, 'timeOrigin', {
    get: function () { return timeOrigin; }, enumerable: true, configurable: true
  });
  Object.defineProperty(Performance.prototype, 'toJSON', {
    value: function toJSON() { return { timeOrigin: timeOrigin }; }, writable: true, enumerable: true, configurable: true
  });
  Object.defineProperty(global, 'Performance', { value: Performance, writable: true, configurable: true });
  global.performance = Object.create(Performance.prototype);

  // Viewport metrics ([Replaceable]: assigning shadows them with a plain value). Axiom
  // renders at 1 device pixel per CSS px and has no window decorations.
  function replaceable(name, get) {
    Object.defineProperty(global, name, {
      get: get,
      set: function (v) {
        Object.defineProperty(global, name, { value: v, writable: true, enumerable: true, configurable: true });
      },
      enumerable: true,
      configurable: true
    });
  }
  function viewportWidth() { return __axiom_viewportSize()[0] | 0; }
  function viewportHeight() { return __axiom_viewportSize()[1] | 0; }
  replaceable('innerWidth', viewportWidth);
  replaceable('innerHeight', viewportHeight);
  replaceable('outerWidth', viewportWidth);
  replaceable('outerHeight', viewportHeight);
  replaceable('devicePixelRatio', function () { return 1; });

  // document.activeElement: the focused element (retargeted out of shadow trees), else the
  // body, else the root element.
  Object.defineProperty(doc, 'activeElement', {
    get: function () {
      var focused = __axiom_focusedElement() | 0;
      if (focused >= 0) return new ElementRef(__axiom_retargetId(focused, __axiom_documentRoot() | 0));
      return doc.body || doc.documentElement || null;
    },
    enumerable: true,
    configurable: true
  });
})(this);
