// Fetch bindings. Normalizes arguments the way WebIDL would and keeps per-object state;
// every security decision (URL schemes, origins, forbidden headers, modes) is made again
// by the host in Rust, which is the boundary.
(function (global) {
  'use strict';

  var INTERNAL = {};

  // ---------------------------------------------------------------------------
  // DOMException
  // ---------------------------------------------------------------------------
  // Legacy names with codes (WebIDL §2.8.1), and their constant names.
  var DOM_CODES = [
    ['IndexSizeError', 1, 'INDEX_SIZE_ERR'], ['HierarchyRequestError', 3, 'HIERARCHY_REQUEST_ERR'],
    ['WrongDocumentError', 4, 'WRONG_DOCUMENT_ERR'], ['InvalidCharacterError', 5, 'INVALID_CHARACTER_ERR'],
    ['NoModificationAllowedError', 7, 'NO_MODIFICATION_ALLOWED_ERR'], ['NotFoundError', 8, 'NOT_FOUND_ERR'],
    ['NotSupportedError', 9, 'NOT_SUPPORTED_ERR'], ['InUseAttributeError', 10, 'INUSE_ATTRIBUTE_ERR'],
    ['InvalidStateError', 11, 'INVALID_STATE_ERR'], ['SyntaxError', 12, 'SYNTAX_ERR'],
    ['InvalidModificationError', 13, 'INVALID_MODIFICATION_ERR'], ['NamespaceError', 14, 'NAMESPACE_ERR'],
    ['InvalidAccessError', 15, 'INVALID_ACCESS_ERR'], ['TypeMismatchError', 17, 'TYPE_MISMATCH_ERR'],
    ['SecurityError', 18, 'SECURITY_ERR'], ['NetworkError', 19, 'NETWORK_ERR'],
    ['AbortError', 20, 'ABORT_ERR'], ['URLMismatchError', 21, 'URL_MISMATCH_ERR'],
    ['QuotaExceededError', 22, 'QUOTA_EXCEEDED_ERR'], ['TimeoutError', 23, 'TIMEOUT_ERR'],
    ['InvalidNodeTypeError', 24, 'INVALID_NODE_TYPE_ERR'], ['DataCloneError', 25, 'DATA_CLONE_ERR']
  ];
  var CODE_BY_NAME = Object.create(null);
  DOM_CODES.forEach(function (c) { CODE_BY_NAME[c[0]] = c[1]; });
  class DOMException extends Error {
    constructor(message, name) {
      super(message === undefined ? '' : String(message));
      Object.defineProperty(this, 'name', {
        value: name === undefined ? 'Error' : String(name),
        writable: true, configurable: true
      });
    }
    get code() { return CODE_BY_NAME[this.name] || 0; }
  }
  DOM_CODES.forEach(function (c) {
    Object.defineProperty(DOMException, c[2], { value: c[1], enumerable: true });
    Object.defineProperty(DOMException.prototype, c[2], { value: c[1], enumerable: true });
  });

  function abortError() {
    return new DOMException('signal is aborted without reason', 'AbortError');
  }

  // ---------------------------------------------------------------------------
  // AbortController / AbortSignal
  // ---------------------------------------------------------------------------
  var events = global.__axiom_events;
  class AbortSignal extends global.EventTarget {
    constructor(key) {
      if (key !== INTERNAL) throw new TypeError('Illegal constructor');
      super();
      this._aborted = false;
      this._reason = undefined;
      this._algorithms = [];
      this._followers = [];
    }
    get aborted() { return this._aborted; }
    get reason() { return this._reason; }
    throwIfAborted() { if (this._aborted) throw this._reason; }
    static abort(reason) {
      var s = new AbortSignal(INTERNAL);
      signalAbort(s, reason);
      return s;
    }
    static timeout(ms) {
      var s = new AbortSignal(INTERNAL);
      setTimeout(function () {
        signalAbort(s, new DOMException('signal timed out', 'TimeoutError'));
      }, Number(ms) || 0);
      return s;
    }
    static any(signals) {
      var s = new AbortSignal(INTERNAL);
      var list = Array.from(signals);
      for (var i = 0; i < list.length; i++) {
        if (list[i]._aborted) { signalAbort(s, list[i]._reason); return s; }
      }
      for (var j = 0; j < list.length; j++) list[j]._followers.push(s);
      return s;
    }
  }

  function signalAbort(signal, reason) {
    if (signal._aborted) return;
    signal._aborted = true;
    signal._reason = reason === undefined ? abortError() : reason;
    var algorithms = signal._algorithms.splice(0);
    for (var i = 0; i < algorithms.length; i++) {
      try { algorithms[i](); } catch (e) { __axiom_log(String(e)); }
    }
    events.fire(signal, 'abort');
    var followers = signal._followers.splice(0);
    for (var k = 0; k < followers.length; k++) signalAbort(followers[k], signal._reason);
  }

  events.defineHandlers(AbortSignal.prototype, ['onabort']);
  events.addAbortAlgorithm = function (signal, fn) { signal._algorithms.push(fn); };

  function followSignal(follower, parent) {
    if (!parent) return;
    if (parent._aborted) signalAbort(follower, parent._reason);
    else parent._followers.push(follower);
  }

  class AbortController {
    constructor() { this._signal = new AbortSignal(INTERNAL); }
    get signal() { return this._signal; }
    abort(reason) { signalAbort(this._signal, reason); }
  }

  // ---------------------------------------------------------------------------
  // ReadableStream (default readers only)
  // ---------------------------------------------------------------------------
  class ReadableStreamDefaultController {
    constructor(key, stream) {
      if (key !== INTERNAL) throw new TypeError('Illegal constructor');
      this._stream = stream;
    }
    get desiredSize() {
      var s = this._stream;
      if (s._state === 'errored') return null;
      if (s._state === 'closed') return 0;
      return s._highWaterMark - s._queue.length;
    }
    enqueue(chunk) {
      var s = this._stream;
      if (s._closeRequested || s._state !== 'readable') {
        throw new TypeError('Cannot enqueue into a closed stream');
      }
      var r = s._reader;
      if (r && r._readRequests.length) {
        if (s._onDequeue) s._onDequeue(chunk);
        r._readRequests.shift().resolve({ value: chunk, done: false });
      } else {
        s._queue.push(chunk);
      }
      pullIfNeeded(s);
    }
    close() {
      var s = this._stream;
      if (s._closeRequested || s._state !== 'readable') {
        throw new TypeError('Stream is already closed');
      }
      s._closeRequested = true;
      if (!s._queue.length) finishClose(s);
    }
    error(e) { errorStream(this._stream, e); }
  }

  class ReadableStream {
    constructor(source, strategy) {
      if (source === undefined || source === null) source = {};
      if (source.type === 'bytes') throw new TypeError('Byte streams are not supported');
      this._source = source;
      this._queue = [];
      this._state = 'readable';
      this._error = undefined;
      this._reader = null;
      this._disturbed = false;
      this._closeRequested = false;
      this._started = false;
      this._pulling = false;
      this._pullAgain = false;
      var hwm = strategy && strategy.highWaterMark !== undefined ? Number(strategy.highWaterMark) : 1;
      this._highWaterMark = hwm >= 0 ? hwm : 1;
      this._controller = new ReadableStreamDefaultController(INTERNAL, this);
      var self = this;
      var started;
      try {
        started = typeof source.start === 'function' ? source.start(this._controller) : undefined;
      } catch (e) {
        errorStream(this, e);
        return;
      }
      Promise.resolve(started).then(function () {
        self._started = true;
        pullIfNeeded(self);
      }, function (e) { errorStream(self, e); });
    }
    get locked() { return this._reader !== null; }
    getReader(options) {
      if (options && options.mode !== undefined) {
        throw new TypeError('Only default readers are supported');
      }
      return new ReadableStreamDefaultReader(this);
    }
    cancel(reason) {
      if (this._reader) return Promise.reject(new TypeError('Cannot cancel a locked stream'));
      return cancelStream(this, reason);
    }
    tee() {
      if (this._reader) throw new TypeError('Cannot tee a locked stream');
      var reader = this.getReader();
      var c1, c2, canceled1 = false, canceled2 = false, reading = false;
      function safe(fn) { try { fn(); } catch (_) { /* branch already closed */ } }
      function pull() {
        if (reading) return Promise.resolve();
        reading = true;
        return reader.read().then(function (r) {
          reading = false;
          if (r.done) {
            if (!canceled1) safe(function () { c1.close(); });
            if (!canceled2) safe(function () { c2.close(); });
            return;
          }
          if (!canceled1) safe(function () { c1.enqueue(r.value); });
          if (!canceled2) safe(function () { c2.enqueue(r.value); });
        }, function (e) {
          safe(function () { c1.error(e); });
          safe(function () { c2.error(e); });
        });
      }
      var b1 = new ReadableStream({
        start: function (c) { c1 = c; },
        pull: pull,
        cancel: function (reason) { canceled1 = true; if (canceled2) return reader.cancel(reason); }
      });
      var b2 = new ReadableStream({
        start: function (c) { c2 = c; },
        pull: pull,
        cancel: function (reason) { canceled2 = true; if (canceled1) return reader.cancel(reason); }
      });
      return [b1, b2];
    }
    values() {
      var reader = this.getReader();
      return {
        next: function () { return reader.read(); },
        return: function (v) {
          return reader.cancel().then(function () { return { value: v, done: true }; });
        },
        [Symbol.asyncIterator]: function () { return this; }
      };
    }
    [Symbol.asyncIterator]() { return this.values(); }
  }

  class ReadableStreamDefaultReader {
    constructor(stream) {
      if (!(stream instanceof ReadableStream)) throw new TypeError('Not a ReadableStream');
      if (stream._reader) throw new TypeError('ReadableStream is locked');
      this._stream = stream;
      this._readRequests = [];
      stream._reader = this;
      var self = this;
      this._closed = new Promise(function (resolve, reject) {
        self._closedResolve = resolve;
        self._closedReject = reject;
      });
      this._closed.catch(function () {});
      if (stream._state === 'closed') this._closedResolve();
      else if (stream._state === 'errored') this._closedReject(stream._error);
    }
    get closed() { return this._closed; }
    read() {
      var s = this._stream;
      if (!s) return Promise.reject(new TypeError('Reader has been released'));
      s._disturbed = true;
      if (s._queue.length) {
        var chunk = s._queue.shift();
        if (s._onDequeue) s._onDequeue(chunk);
        if (s._closeRequested && !s._queue.length) finishClose(s);
        else pullIfNeeded(s);
        return Promise.resolve({ value: chunk, done: false });
      }
      if (s._state === 'closed') return Promise.resolve({ value: undefined, done: true });
      if (s._state === 'errored') return Promise.reject(s._error);
      var self = this;
      return new Promise(function (resolve, reject) {
        self._readRequests.push({ resolve: resolve, reject: reject });
        pullIfNeeded(s);
      });
    }
    releaseLock() {
      var s = this._stream;
      if (!s) return;
      var err = new TypeError('Reader was released');
      this._readRequests.splice(0).forEach(function (q) { q.reject(err); });
      if (s._state === 'readable') this._closedReject(err);
      s._reader = null;
      this._stream = null;
    }
    cancel(reason) {
      if (!this._stream) return Promise.reject(new TypeError('Reader has been released'));
      return cancelStream(this._stream, reason);
    }
  }

  function finishClose(s) {
    if (s._state !== 'readable') return;
    s._state = 'closed';
    var r = s._reader;
    if (r) {
      r._readRequests.splice(0).forEach(function (q) { q.resolve({ value: undefined, done: true }); });
      r._closedResolve();
    }
  }

  function errorStream(s, e) {
    if (s._state !== 'readable') return;
    s._state = 'errored';
    s._error = e;
    s._queue = [];
    var r = s._reader;
    if (r) {
      r._readRequests.splice(0).forEach(function (q) { q.reject(e); });
      r._closedReject(e);
    }
  }

  function cancelStream(s, reason) {
    s._disturbed = true;
    if (s._state === 'closed') return Promise.resolve();
    if (s._state === 'errored') return Promise.reject(s._error);
    s._queue = [];
    finishClose(s);
    try {
      var fn = s._source.cancel;
      return Promise.resolve(typeof fn === 'function' ? fn.call(s._source, reason) : undefined)
        .then(function () {});
    } catch (e) {
      return Promise.reject(e);
    }
  }

  function pullIfNeeded(s) {
    if (!s._started || s._state !== 'readable' || s._closeRequested) return;
    if (typeof s._source.pull !== 'function') return;
    var waiting = s._reader && s._reader._readRequests.length > 0;
    if (!waiting && s._queue.length >= s._highWaterMark) return;
    if (s._pulling) { s._pullAgain = true; return; }
    s._pulling = true;
    Promise.resolve().then(function () {
      return s._source.pull.call(s._source, s._controller);
    }).then(function () {
      s._pulling = false;
      if (s._pullAgain) { s._pullAgain = false; pullIfNeeded(s); }
    }, function (e) { errorStream(s, e); });
  }

  function streamFromBytes(bytes) {
    return new ReadableStream({
      start: function (c) {
        if (bytes.byteLength) c.enqueue(bytes);
        c.close();
      }
    });
  }

  // ---------------------------------------------------------------------------
  // TextEncoder / TextDecoder (UTF-8)
  // ---------------------------------------------------------------------------
  class TextEncoder {
    get encoding() { return 'utf-8'; }
    encode(input) {
      return new Uint8Array(__axiom_utf8Encode(input === undefined ? '' : String(input)));
    }
  }

  class TextDecoder {
    constructor(label) {
      var l = label === undefined ? 'utf-8' : String(label).trim().toLowerCase();
      if (l !== 'utf-8' && l !== 'utf8' && l !== 'unicode-1-1-utf-8') {
        throw new RangeError('TextDecoder only supports UTF-8');
      }
    }
    get encoding() { return 'utf-8'; }
    decode(input) {
      if (input === undefined) return '';
      return __axiom_utf8Decode(toArrayBuffer(input));
    }
  }

  function toArrayBuffer(data) {
    if (data instanceof ArrayBuffer) return data.slice(0);
    if (ArrayBuffer.isView(data)) {
      return data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength);
    }
    throw new TypeError('Expected an ArrayBuffer or ArrayBufferView');
  }

  // ---------------------------------------------------------------------------
  // Headers
  // ---------------------------------------------------------------------------
  var TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  var FORBIDDEN_REQUEST = [
    'accept-charset', 'accept-encoding', 'access-control-request-headers',
    'access-control-request-method', 'connection', 'content-length', 'cookie', 'cookie2',
    'date', 'dnt', 'expect', 'host', 'keep-alive', 'origin', 'referer', 'set-cookie', 'te',
    'trailer', 'transfer-encoding', 'upgrade', 'via'
  ];
  var FORBIDDEN_RESPONSE = ['set-cookie', 'set-cookie2'];
  var SAFE_CONTENT_TYPES = ['application/x-www-form-urlencoded', 'multipart/form-data', 'text/plain'];

  function normalizeValue(v) { return String(v).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, ''); }

  function validate(name, value) {
    if (!TOKEN.test(name)) throw new TypeError('Invalid header name: ' + name);
    if (/[\0\r\n]/.test(value)) throw new TypeError('Invalid header value for ' + name);
  }

  function forbiddenRequestHeader(name, value) {
    if (FORBIDDEN_REQUEST.indexOf(name) >= 0) return true;
    if (name.indexOf('proxy-') === 0 || name.indexOf('sec-') === 0) return true;
    if (name === 'x-http-method' || name === 'x-http-method-override' || name === 'x-method-override') {
      return value.split(',').some(function (m) {
        var t = m.trim().toUpperCase();
        return t === 'CONNECT' || t === 'TRACE' || t === 'TRACK';
      });
    }
    return false;
  }

  function noCorsSafelisted(name, value) {
    if (value.length > 128) return false;
    if (name === 'accept' || name === 'accept-language' || name === 'content-language') return true;
    if (name === 'content-type') {
      return SAFE_CONTENT_TYPES.indexOf(value.split(';')[0].trim().toLowerCase()) >= 0;
    }
    return false;
  }

  // Whether the guard lets (name, value) in; throws for an immutable guard.
  function guardAllows(h, name, value) {
    switch (h._guard) {
      case 'immutable': throw new TypeError('Headers are immutable');
      case 'request': return !forbiddenRequestHeader(name, value);
      case 'request-no-cors':
        return !forbiddenRequestHeader(name, value) && noCorsSafelisted(name, value);
      case 'response': return FORBIDDEN_RESPONSE.indexOf(name) < 0;
      default: return true;
    }
  }

  class Headers {
    constructor(init) {
      this._list = [];
      this._guard = 'none';
      if (init !== undefined && init !== null) fillHeaders(this, init);
    }
    append(name, value) {
      var n = String(name).toLowerCase();
      var v = normalizeValue(value);
      validate(n, v);
      if (!guardAllows(this, n, v)) return;
      this._list.push([n, v]);
    }
    delete(name) {
      var n = String(name).toLowerCase();
      if (!TOKEN.test(n)) throw new TypeError('Invalid header name: ' + name);
      if (!guardAllows(this, n, '')) return;
      this._list = this._list.filter(function (p) { return p[0] !== n; });
    }
    get(name) {
      var n = String(name).toLowerCase();
      if (!TOKEN.test(n)) throw new TypeError('Invalid header name: ' + name);
      var values = this._list.filter(function (p) { return p[0] === n; })
        .map(function (p) { return p[1]; });
      return values.length ? values.join(', ') : null;
    }
    getSetCookie() {
      return this._list.filter(function (p) { return p[0] === 'set-cookie'; })
        .map(function (p) { return p[1]; });
    }
    has(name) {
      var n = String(name).toLowerCase();
      if (!TOKEN.test(n)) throw new TypeError('Invalid header name: ' + name);
      return this._list.some(function (p) { return p[0] === n; });
    }
    set(name, value) {
      var n = String(name).toLowerCase();
      var v = normalizeValue(value);
      validate(n, v);
      if (!guardAllows(this, n, v)) return;
      var i = this._list.findIndex(function (p) { return p[0] === n; });
      if (i < 0) { this._list.push([n, v]); return; }
      this._list[i] = [n, v];
      this._list = this._list.filter(function (p, j) { return j <= i || p[0] !== n; });
    }
    forEach(callback, thisArg) {
      var entries = sortedEntries(this);
      for (var i = 0; i < entries.length; i++) {
        callback.call(thisArg, entries[i][1], entries[i][0], this);
      }
    }
    entries() { return sortedEntries(this)[Symbol.iterator](); }
    keys() { return sortedEntries(this).map(function (e) { return e[0]; })[Symbol.iterator](); }
    values() { return sortedEntries(this).map(function (e) { return e[1]; })[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
  }

  function sortedEntries(h) {
    var names = [];
    h._list.forEach(function (p) { if (names.indexOf(p[0]) < 0) names.push(p[0]); });
    names.sort();
    var out = [];
    names.forEach(function (n) {
      if (n === 'set-cookie') {
        h.getSetCookie().forEach(function (v) { out.push([n, v]); });
      } else {
        out.push([n, h.get(n)]);
      }
    });
    return out;
  }

  function fillHeaders(h, init) {
    if (init instanceof Headers) {
      init._list.forEach(function (p) { h.append(p[0], p[1]); });
    } else if (typeof init === 'object' && typeof init[Symbol.iterator] === 'function') {
      for (var pair of init) {
        var p = Array.from(pair);
        if (p.length !== 2) throw new TypeError('Header pairs must have exactly two items');
        h.append(p[0], p[1]);
      }
    } else if (typeof init === 'object') {
      Object.keys(init).forEach(function (k) { h.append(k, init[k]); });
    } else {
      throw new TypeError('Invalid HeadersInit');
    }
  }

  function copyHeaders(src, guard) {
    var h = new Headers();
    h._list = src._list.map(function (p) { return [p[0], p[1]]; });
    h._guard = guard;
    return h;
  }

  // ---------------------------------------------------------------------------
  // Body mixin
  // ---------------------------------------------------------------------------
  // Returns { bytes: Uint8Array|null, stream: ReadableStream|null, type: string|null }.
  function extractBody(body) {
    if (body instanceof ReadableStream) {
      if (body.locked || body._disturbed) throw new TypeError('ReadableStream is locked or disturbed');
      return { bytes: null, stream: body, type: null };
    }
    if (body instanceof ArrayBuffer || ArrayBuffer.isView(body)) {
      return { bytes: new Uint8Array(toArrayBuffer(body)), stream: null, type: null };
    }
    if (body instanceof Blob) {
      return { bytes: body._bytes.slice(), stream: null, type: body._type || null };
    }
    if (body instanceof FormData) {
      var form = __axiom_platform.serializeFormData(body);
      return { bytes: form.bytes, stream: null, type: form.type };
    }
    if (body instanceof URLSearchParams) {
      return {
        bytes: new Uint8Array(__axiom_utf8Encode(body.toString())),
        stream: null,
        type: 'application/x-www-form-urlencoded;charset=UTF-8'
      };
    }
    return {
      bytes: new Uint8Array(__axiom_utf8Encode(String(body))),
      stream: null,
      type: 'text/plain;charset=UTF-8'
    };
  }

  // Lazily materialized body stream (null for a null body). `_bodyBytes` stays as the
  // body's source so a byte body is never mistaken for a streaming upload.
  function bodyStream(obj) {
    if (!obj._body && obj._bodyBytes) obj._body = streamFromBytes(obj._bodyBytes);
    return obj._body;
  }

  function bodyUsed(obj) {
    var s = obj._body;
    return !!(s && s._disturbed);
  }

  function consumeBody(obj) {
    var s = bodyStream(obj);
    if (!s) return Promise.resolve(new Uint8Array(0));
    if (s._disturbed || s.locked) return Promise.reject(new TypeError('Body has already been consumed'));
    var reader = s.getReader();
    var chunks = [];
    var total = 0;
    function next() {
      return reader.read().then(function (r) {
        if (r.done) {
          var out = new Uint8Array(total);
          var offset = 0;
          chunks.forEach(function (c) { out.set(c, offset); offset += c.byteLength; });
          return out;
        }
        if (!(r.value instanceof Uint8Array)) throw new TypeError('Body chunks must be Uint8Array');
        chunks.push(r.value);
        total += r.value.byteLength;
        return next();
      });
    }
    return next();
  }

  var bodyMethods = {
    arrayBuffer: function () {
      return consumeBody(this).then(function (b) { return b.buffer; });
    },
    bytes: function () { return consumeBody(this); },
    text: function () {
      return consumeBody(this).then(function (b) { return __axiom_utf8Decode(b.buffer); });
    },
    json: function () {
      return this.text().then(function (t) { return JSON.parse(t); });
    },
    blob: function () {
      var self = this;
      return consumeBody(this).then(function (b) {
        return __axiom_platform.makeBlob(b, self._headers.get('content-type') || '');
      });
    },
    formData: function () {
      var self = this;
      return consumeBody(this).then(function (b) {
        var type = self._headers.get('content-type') || '';
        var essence = type.split(';')[0].trim().toLowerCase();
        if (essence === 'multipart/form-data') return __axiom_platform.parseMultipart(b, type);
        if (essence === 'application/x-www-form-urlencoded') {
          return __axiom_platform.formDataFromList(
            __axiom_platform.parseUrlencoded(__axiom_utf8Decode(b.buffer)));
        }
        throw new TypeError('Could not parse content as FormData');
      });
    }
  };

  function installBody(proto) {
    Object.keys(bodyMethods).forEach(function (k) {
      Object.defineProperty(proto, k, { value: bodyMethods[k], writable: true, configurable: true });
    });
    Object.defineProperty(proto, 'body', {
      get: function () { return bodyStream(this); }, configurable: true
    });
    Object.defineProperty(proto, 'bodyUsed', {
      get: function () { return bodyUsed(this); }, configurable: true
    });
  }

  // ---------------------------------------------------------------------------
  // Request
  // ---------------------------------------------------------------------------
  var MODES = ['cors', 'no-cors', 'same-origin'];
  var CREDENTIALS = ['omit', 'same-origin', 'include'];
  var CACHES = ['default', 'no-store', 'reload', 'no-cache', 'force-cache', 'only-if-cached'];
  var REDIRECTS = ['follow', 'error', 'manual'];
  var PRIORITIES = ['high', 'low', 'auto'];
  var REFERRER_POLICIES = [
    '', 'no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin',
    'strict-origin', 'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url'
  ];
  var STANDARD_METHODS = ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT', 'PATCH'];

  function enumValue(v, allowed, what) {
    var s = String(v);
    if (allowed.indexOf(s) < 0) throw new TypeError("'" + s + "' is not a valid " + what);
    return s;
  }

  function normalizeMethod(m) {
    var s = String(m);
    if (!TOKEN.test(s)) throw new TypeError("'" + s + "' is not a valid HTTP method");
    var upper = s.toUpperCase();
    if (upper === 'CONNECT' || upper === 'TRACE' || upper === 'TRACK') {
      throw new TypeError("'" + s + "' HTTP method is unsupported");
    }
    return STANDARD_METHODS.indexOf(upper) >= 0 ? upper : s;
  }

  function parseUrl(input) {
    var url = __axiom_resolveUrl(String(input));
    if (url === null) throw new TypeError('Failed to parse URL from ' + input);
    if (/^[a-z][a-z0-9+.\-]*:\/\/[^\/?#]*@/i.test(url)) {
      throw new TypeError('Request cannot be constructed from a URL that includes credentials');
    }
    return url;
  }

  class Request {
    constructor(input, init) {
      if (init === undefined || init === null) init = {};
      var base = input instanceof Request ? input : null;
      this._url = base ? base._url : parseUrl(input);
      this._method = base ? base._method : 'GET';
      this._mode = base ? base._mode : 'cors';
      this._credentials = base ? base._credentials : 'same-origin';
      this._cache = base ? base._cache : 'default';
      this._redirect = base ? base._redirect : 'follow';
      this._referrer = base ? base._referrer : 'about:client';
      this._referrerPolicy = base ? base._referrerPolicy : '';
      this._integrity = base ? base._integrity : '';
      this._keepalive = base ? base._keepalive : false;
      this._priority = base ? base._priority : 'auto';

      if (init.mode !== undefined) {
        if (String(init.mode) === 'navigate') throw new TypeError("Cannot construct a Request with mode 'navigate'");
        this._mode = enumValue(init.mode, MODES, 'RequestMode');
      }
      if (init.credentials !== undefined) this._credentials = enumValue(init.credentials, CREDENTIALS, 'RequestCredentials');
      if (init.cache !== undefined) this._cache = enumValue(init.cache, CACHES, 'RequestCache');
      if (init.redirect !== undefined) this._redirect = enumValue(init.redirect, REDIRECTS, 'RequestRedirect');
      if (init.referrerPolicy !== undefined) this._referrerPolicy = enumValue(init.referrerPolicy, REFERRER_POLICIES, 'ReferrerPolicy');
      if (init.priority !== undefined) this._priority = enumValue(init.priority, PRIORITIES, 'RequestPriority');
      if (init.integrity !== undefined) this._integrity = String(init.integrity);
      if (init.keepalive !== undefined) this._keepalive = !!init.keepalive;
      if (init.referrer !== undefined) {
        var r = String(init.referrer);
        if (r === '') {
          this._referrer = '';
        } else {
          var parsedReferrer = __axiom_resolveUrl(r);
          if (parsedReferrer === null) throw new TypeError('Failed to parse referrer URL from ' + r);
          // Cross-origin referrer URLs (and about:client) fall back to the client.
          this._referrer = __axiom_fetchReferrer(parsedReferrer);
        }
      }
      if (init.window !== undefined && init.window !== null) {
        throw new TypeError("'window' must be null");
      }
      if (this._cache === 'only-if-cached' && this._mode !== 'same-origin') {
        throw new TypeError("'only-if-cached' can be set only with 'same-origin' mode");
      }
      if (init.method !== undefined) this._method = normalizeMethod(init.method);

      var guard = this._mode === 'no-cors' ? 'request-no-cors' : 'request';
      if (this._mode === 'no-cors' && ['GET', 'HEAD', 'POST'].indexOf(this._method) < 0) {
        throw new TypeError("'" + this._method + "' is unsupported in no-cors mode");
      }
      this._headers = new Headers();
      this._headers._guard = guard;
      fillHeaders(this._headers, init.headers !== undefined ? init.headers : (base ? base._headers : []));

      this._signal = new AbortSignal(INTERNAL);
      followSignal(this._signal, init.signal !== undefined ? init.signal : (base ? base._signal : null));

      this._body = null;
      this._bodyBytes = null;
      var hasInitBody = init.body !== undefined && init.body !== null;
      if ((hasInitBody || (base && (base._body || base._bodyBytes))) &&
          (this._method === 'GET' || this._method === 'HEAD')) {
        throw new TypeError('Request with GET/HEAD method cannot have body');
      }
      if (hasInitBody) {
        var ex = extractBody(init.body);
        if (ex.stream) {
          if (init.duplex === undefined) {
            throw new TypeError("RequestInit's duplex member must be 'half' for a ReadableStream body");
          }
          if (String(init.duplex) !== 'half') throw new TypeError("'" + init.duplex + "' is not a valid RequestDuplex");
          this._body = ex.stream;
        } else {
          this._bodyBytes = ex.bytes;
        }
        if (ex.type && !this._headers.has('content-type')) this._headers.append('content-type', ex.type);
      } else if (base) {
        if (bodyUsed(base)) throw new TypeError('Cannot construct a Request with a Request whose body is already used');
        // The input request's body is transferred to the new request.
        if (base._bodyBytes) {
          this._bodyBytes = base._bodyBytes;
          base._bodyBytes = null;
          base._body = streamFromBytes(new Uint8Array(0));
          base._body._disturbed = true;
        } else if (base._body) {
          this._body = base._body;
          base._body = streamFromBytes(new Uint8Array(0));
          base._body._disturbed = true;
        }
      }
      if (this._body) {
        // A body with no byte source is streamed to the network.
        if (this._keepalive) throw new TypeError('keepalive requests cannot have a ReadableStream body');
        if (this._mode !== 'cors' && this._mode !== 'same-origin') {
          throw new TypeError("ReadableStream request bodies require 'cors' or 'same-origin' mode");
        }
      }
    }
    get method() { return this._method; }
    get url() { return this._url; }
    get headers() { return this._headers; }
    get destination() { return ''; }
    get referrer() {
      if (this._referrer === '') return '';
      return this._referrer === 'about:client' ? 'about:client' : this._referrer;
    }
    get referrerPolicy() { return this._referrerPolicy; }
    get mode() { return this._mode; }
    get credentials() { return this._credentials; }
    get cache() { return this._cache; }
    get redirect() { return this._redirect; }
    get integrity() { return this._integrity; }
    get keepalive() { return this._keepalive; }
    get signal() { return this._signal; }
    get duplex() { return 'half'; }
    clone() {
      if (bodyUsed(this)) throw new TypeError('Cannot clone a Request whose body is already used');
      var c = Object.create(Request.prototype);
      Object.keys(this).forEach(function (k) { c[k] = this[k]; }, this);
      c._headers = copyHeaders(this._headers, this._headers._guard);
      c._signal = new AbortSignal(INTERNAL);
      followSignal(c._signal, this._signal);
      c._body = null;
      c._bodyBytes = null;
      if (this._bodyBytes) {
        c._bodyBytes = this._bodyBytes.slice();
      } else if (this._body) {
        var branches = this._body.tee();
        this._body = branches[0];
        c._body = branches[1];
      }
      return c;
    }
  }
  installBody(Request.prototype);

  // ---------------------------------------------------------------------------
  // Response
  // ---------------------------------------------------------------------------
  var NULL_BODY_STATUS = [101, 103, 204, 205, 304];
  var REDIRECT_STATUS = [301, 302, 303, 307, 308];

  function makeResponse(type, status, statusText, headers, url, redirected) {
    var r = Object.create(Response.prototype);
    r._type = type;
    r._status = status;
    r._statusText = statusText;
    r._headers = headers;
    r._url = url;
    r._redirected = redirected;
    r._body = null;
    r._bodyBytes = null;
    return r;
  }

  class Response {
    constructor(body, init) {
      if (init === undefined || init === null) init = {};
      var status = init.status === undefined ? 200 : Number(init.status);
      if (!(status >= 200 && status <= 599) || Math.floor(status) !== status) {
        throw new RangeError('Response status must be in the range 200 to 599');
      }
      var statusText = init.statusText === undefined ? '' : String(init.statusText);
      if (/[\r\n]/.test(statusText)) throw new TypeError('Invalid statusText');
      this._type = 'default';
      this._status = status;
      this._statusText = statusText;
      this._headers = new Headers();
      this._headers._guard = 'response';
      if (init.headers !== undefined) fillHeaders(this._headers, init.headers);
      this._url = '';
      this._redirected = false;
      this._body = null;
      this._bodyBytes = null;
      if (body !== undefined && body !== null) {
        if (NULL_BODY_STATUS.indexOf(status) >= 0) {
          throw new TypeError('Response with null body status cannot have body');
        }
        var ex = extractBody(body);
        this._body = ex.stream;
        this._bodyBytes = ex.bytes;
        if (ex.type && !this._headers.has('content-type')) this._headers.append('content-type', ex.type);
      }
    }
    static error() {
      var h = new Headers();
      h._guard = 'immutable';
      return makeResponse('error', 0, '', h, '', false);
    }
    static redirect(url, status) {
      var s = status === undefined ? 302 : Number(status);
      if (REDIRECT_STATUS.indexOf(s) < 0) throw new RangeError('Invalid redirect status');
      var h = new Headers();
      h._list.push(['location', parseUrl(url)]);
      h._guard = 'immutable';
      return makeResponse('default', s, '', h, '', false);
    }
    static json(data, init) {
      var text = JSON.stringify(data);
      if (text === undefined) throw new TypeError('Value is not JSON serializable');
      var r = new Response(text, init);
      r._headers._guard = 'none';
      r._headers.set('content-type', 'application/json');
      r._headers._guard = 'response';
      return r;
    }
    get type() { return this._type; }
    get url() { return this._url; }
    get redirected() { return this._redirected; }
    get status() { return this._status; }
    get ok() { return this._status >= 200 && this._status <= 299; }
    get statusText() { return this._statusText; }
    get headers() { return this._headers; }
    clone() {
      if (bodyUsed(this) || (this._body && this._body.locked)) {
        throw new TypeError('Cannot clone a Response whose body is already used');
      }
      var c = makeResponse(this._type, this._status, this._statusText,
        copyHeaders(this._headers, this._headers._guard), this._url, this._redirected);
      var s = bodyStream(this);
      if (s) {
        var branches = s.tee();
        this._body = branches[0];
        c._body = branches[1];
      }
      return c;
    }
  }
  installBody(Response.prototype);

  // ---------------------------------------------------------------------------
  // fetch()
  // ---------------------------------------------------------------------------
  var FETCHES = Object.create(null);

  function fetch(input, init) {
    return new Promise(function (resolve, reject) {
      var request;
      try {
        request = new Request(input, init);
      } catch (e) {
        reject(e);
        return;
      }
      var signal = request._signal;
      if (signal._aborted) { reject(signal._reason); return; }
      if (bodyUsed(request) || (request._body && request._body.locked)) {
        reject(new TypeError('Request body is already used'));
        return;
      }
      var body = null;
      var upload = null;
      if (request._bodyBytes) {
        body = toArrayBuffer(request._bodyBytes);
        request._body = streamFromBytes(new Uint8Array(0));
        request._body._disturbed = true;
        request._bodyBytes = null;
      } else if (request._body) {
        upload = request._body;
      }
      var id;
      try {
        id = __axiom_fetchStart({
          method: request._method,
          url: request._url,
          headers: request._headers._list,
          body: body,
          bodyStream: upload !== null,
          mode: request._mode,
          credentials: request._credentials,
          cache: request._cache,
          redirect: request._redirect,
          referrer: request._referrer,
          referrerPolicy: request._referrerPolicy,
          integrity: request._integrity,
          keepalive: request._keepalive,
          priority: request._priority
        });
      } catch (e) {
        reject(e instanceof TypeError ? e : new TypeError(String(e)));
        return;
      }
      FETCHES[id] = {
        resolve: resolve, reject: reject, request: request, signal: signal,
        response: null, controller: null
      };
      signal._algorithms.push(function () { abortFetch(id, signal._reason); });
      if (upload) pumpUpload(id, upload);
    });
  }

  // Streams a request body to the host. The host reports whether its buffer is below
  // the high-water mark; when it is not, pumping resumes on __axiom_fetchOnUploadDrain.
  var UPLOADS = Object.create(null);

  function pumpUpload(id, stream) {
    var reader = stream.getReader();
    var state = { reader: reader, waiting: null };
    UPLOADS[id] = state;
    function fail(message, reason) {
      if (UPLOADS[id] !== state) return;
      delete UPLOADS[id];
      __axiom_fetchUploadError(id, message);
      reader.cancel(reason).catch(function () {});
    }
    function step() {
      if (UPLOADS[id] !== state) return;
      reader.read().then(function (r) {
        if (UPLOADS[id] !== state) return;
        if (r.done) {
          delete UPLOADS[id];
          __axiom_fetchUploadClose(id);
          return;
        }
        if (!(r.value instanceof Uint8Array)) {
          var err = new TypeError('Request body stream chunks must be Uint8Array');
          fail(err.message, err);
          return;
        }
        var more;
        try {
          more = __axiom_fetchUploadWrite(id, toArrayBuffer(r.value));
        } catch (e) {
          // The request is gone (failed, completed or aborted): stop reading.
          delete UPLOADS[id];
          reader.cancel(e).catch(function () {});
          return;
        }
        if (more) step();
        else state.waiting = step;
      }, function (e) {
        fail('request body stream errored: ' + String(e), e);
      });
    }
    step();
  }

  function cancelUpload(id, reason) {
    var u = UPLOADS[id];
    if (!u) return;
    delete UPLOADS[id];
    u.reader.cancel(reason).catch(function () {});
  }

  global.__axiom_fetchOnUploadDrain = function (id) {
    var u = UPLOADS[id];
    if (!u || !u.waiting) return;
    var resume = u.waiting;
    u.waiting = null;
    resume();
  };

  function abortFetch(id, reason) {
    var e = FETCHES[id];
    if (!e) return;
    delete FETCHES[id];
    __axiom_fetchAbort(id);
    cancelUpload(id, reason);
    if (!e.response) e.reject(reason);
    else if (e.controller) { try { e.controller.error(reason); } catch (_) { /* closed */ } }
  }

  global.__axiom_fetchOnResponse = function (id, type, status, statusText, url, headers, redirected) {
    var e = FETCHES[id];
    if (!e) return;
    var h = new Headers();
    for (var i = 0; i < headers.length; i++) {
      h._list.push([String(headers[i][0]).toLowerCase(), String(headers[i][1])]);
    }
    h._guard = 'immutable';
    var r = makeResponse(type, status, statusText, h, url, redirected);
    var nullBody = (type !== 'basic' && type !== 'cors') || e.request._method === 'HEAD' ||
      NULL_BODY_STATUS.indexOf(status) >= 0;
    if (!nullBody) {
      r._body = new ReadableStream({
        start: function (c) { e.controller = c; },
        cancel: function () {
          if (FETCHES[id]) { delete FETCHES[id]; __axiom_fetchAbort(id); }
        }
      });
      // Bytes leave the network's flow-control window only when script reads them.
      r._body._onDequeue = function (chunk) { __axiom_fetchConsumed(id, chunk.byteLength); };
    }
    e.response = r;
    e.resolve(r);
  };

  global.__axiom_fetchOnChunk = function (id, buffer) {
    var e = FETCHES[id];
    if (!e || !e.controller) {
      __axiom_fetchConsumed(id, buffer.byteLength);
      return;
    }
    try { e.controller.enqueue(new Uint8Array(buffer)); } catch (_) { /* cancelled */ }
  };

  global.__axiom_fetchOnComplete = function (id) {
    var e = FETCHES[id];
    cancelUpload(id, undefined);
    if (!e) return;
    delete FETCHES[id];
    if (!e.response) { e.reject(new TypeError('Failed to fetch')); return; }
    if (e.controller) { try { e.controller.close(); } catch (_) { /* cancelled */ } }
  };

  global.__axiom_fetchOnError = function (id) {
    var e = FETCHES[id];
    cancelUpload(id, new TypeError('Failed to fetch'));
    if (!e) return;
    delete FETCHES[id];
    var err = new TypeError('Failed to fetch');
    if (!e.response) e.reject(err);
    else if (e.controller) { try { e.controller.error(err); } catch (_) { /* cancelled */ } }
  };

  global.DOMException = DOMException;
  global.AbortController = AbortController;
  global.AbortSignal = AbortSignal;
  global.ReadableStream = ReadableStream;
  global.ReadableStreamDefaultReader = ReadableStreamDefaultReader;
  global.ReadableStreamDefaultController = ReadableStreamDefaultController;
  global.TextEncoder = TextEncoder;
  global.TextDecoder = TextDecoder;
  global.Headers = Headers;
  global.Request = Request;
  global.Response = Response;
  global.fetch = fetch;
})(this);
