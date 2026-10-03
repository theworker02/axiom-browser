// Platform objects that are not DOM nodes: promise rejection tracking
// (unhandledrejection / rejectionhandled), Blob, File, URLSearchParams and FormData.
// Evaluated after the events prelude and before the fetch bindings.
(function (global) {
  'use strict';
  var events = global.__axiom_events;
  var PromiseRejectionEvent = global.PromiseRejectionEvent;

  function describe(v) {
    if (v instanceof Error) return v.name + ': ' + v.message;
    try { return typeof v === 'string' ? v : String(v); } catch (_) { return '<value>'; }
  }

  // ---------------------------------------------------------------------------
  // Promise rejection tracking (HTML "notify about rejected promises")
  // ---------------------------------------------------------------------------
  var aboutToBeNotified = [];
  var outstanding = new WeakSet();
  var handledLater = [];

  // Called by the host's HostPromiseRejectionTracker.
  global.__axiom_trackRejection = function (promise, reason, rejected) {
    if (rejected) {
      aboutToBeNotified.push({ promise: promise, reason: reason });
      return;
    }
    for (var i = 0; i < aboutToBeNotified.length; i++) {
      if (aboutToBeNotified[i].promise === promise) {
        aboutToBeNotified.splice(i, 1);
        return;
      }
    }
    if (outstanding.has(promise)) {
      outstanding.delete(promise);
      handledLater.push({ promise: promise, reason: reason });
    }
  };

  // Called by the host at the end of every microtask checkpoint.
  global.__axiom_notifyRejections = function () {
    var handled = handledLater.splice(0);
    for (var h = 0; h < handled.length; h++) {
      events.dispatchTrusted(global, new PromiseRejectionEvent('rejectionhandled', {
        promise: handled[h].promise, reason: handled[h].reason
      }));
    }
    var list = aboutToBeNotified.splice(0);
    for (var i = 0; i < list.length; i++) {
      var ev = new PromiseRejectionEvent('unhandledrejection', {
        cancelable: true, promise: list[i].promise, reason: list[i].reason
      });
      if (events.dispatchTrusted(global, ev)) {
        var reason = list[i].reason;
        if (reason instanceof Error && typeof reason.stack === 'string') __axiom_debugStack(reason.stack);
        __axiom_log('Uncaught (in promise)', describe(reason));
      }
      outstanding.add(list[i].promise);
    }
  };

  // ---------------------------------------------------------------------------
  // Byte helpers
  // ---------------------------------------------------------------------------
  function utf8(s) { return new Uint8Array(__axiom_utf8Encode(String(s))); }

  function copyBytes(data) {
    if (data instanceof ArrayBuffer) return new Uint8Array(data.slice(0));
    if (ArrayBuffer.isView(data)) {
      return new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
    }
    throw new TypeError('Expected an ArrayBuffer or ArrayBufferView');
  }

  function concatBytes(parts) {
    var total = 0;
    parts.forEach(function (p) { total += p.byteLength; });
    var out = new Uint8Array(total);
    var off = 0;
    parts.forEach(function (p) { out.set(p, off); off += p.byteLength; });
    return out;
  }

  function decodeUtf8(bytes) {
    return __axiom_utf8Decode(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
  }

  // ---------------------------------------------------------------------------
  // Blob / File
  // ---------------------------------------------------------------------------
  function normalizeType(t) {
    var s = t === undefined ? '' : String(t);
    return /^[\x20-\x7E]*$/.test(s) ? s.toLowerCase() : '';
  }

  class Blob {
    constructor(parts, options) {
      options = options || {};
      var endings = options.endings === undefined ? 'transparent' : String(options.endings);
      if (endings !== 'transparent' && endings !== 'native') {
        throw new TypeError("'" + endings + "' is not a valid EndingType");
      }
      var chunks = [];
      if (parts !== undefined && parts !== null) {
        if (typeof parts !== 'object' || typeof parts[Symbol.iterator] !== 'function') {
          throw new TypeError('Blob parts must be a sequence');
        }
        for (var part of parts) {
          if (part instanceof Blob) chunks.push(part._bytes);
          else if (part instanceof ArrayBuffer || ArrayBuffer.isView(part)) chunks.push(copyBytes(part));
          else {
            var s = String(part);
            if (endings === 'native') s = s.replace(/\r\n|\r|\n/g, '\n');
            chunks.push(utf8(s));
          }
        }
      }
      this._bytes = concatBytes(chunks);
      this._type = normalizeType(options.type);
    }
    get size() { return this._bytes.byteLength; }
    get type() { return this._type; }
    slice(start, end, contentType) {
      var size = this._bytes.byteLength;
      function rel(v, dflt) {
        if (v === undefined) return dflt;
        v = Math.trunc(Number(v)) || 0;
        return v < 0 ? Math.max(size + v, 0) : Math.min(v, size);
      }
      var s = rel(start, 0), e = rel(end, size);
      var b = Object.create(Blob.prototype);
      b._bytes = this._bytes.slice(s, Math.max(s, e));
      b._type = normalizeType(contentType);
      return b;
    }
    arrayBuffer() { return Promise.resolve(this._bytes.slice().buffer); }
    bytes() { return Promise.resolve(this._bytes.slice()); }
    text() { return Promise.resolve(decodeUtf8(this._bytes)); }
    stream() {
      var bytes = this._bytes.slice();
      return new ReadableStream({
        start: function (c) {
          if (bytes.byteLength) c.enqueue(bytes);
          c.close();
        }
      });
    }
    get [Symbol.toStringTag]() { return 'Blob'; }
  }

  class File extends Blob {
    constructor(bits, name, options) {
      if (arguments.length < 2) throw new TypeError('File constructor requires bits and name');
      super(bits, options);
      options = options || {};
      this._name = String(name);
      this._lastModified = options.lastModified === undefined ? Date.now() : Number(options.lastModified);
    }
    get name() { return this._name; }
    get lastModified() { return this._lastModified; }
    get webkitRelativePath() { return ''; }
    get [Symbol.toStringTag]() { return 'File'; }
  }

  function makeFile(bytes, name, type, lastModified) {
    var f = Object.create(File.prototype);
    f._bytes = bytes;
    f._type = normalizeType(type);
    f._name = String(name);
    f._lastModified = lastModified === undefined ? Date.now() : lastModified;
    return f;
  }

  // ---------------------------------------------------------------------------
  // URLSearchParams (application/x-www-form-urlencoded)
  // ---------------------------------------------------------------------------
  var HEX = '0123456789ABCDEF';

  function formEncode(s) {
    var bytes = utf8(s);
    var out = '';
    for (var i = 0; i < bytes.length; i++) {
      var b = bytes[i];
      if ((b >= 0x30 && b <= 0x39) || (b >= 0x41 && b <= 0x5A) || (b >= 0x61 && b <= 0x7A) ||
          b === 0x2A || b === 0x2D || b === 0x2E || b === 0x5F) {
        out += String.fromCharCode(b);
      } else if (b === 0x20) {
        out += '+';
      } else {
        out += '%' + HEX[b >> 4] + HEX[b & 15];
      }
    }
    return out;
  }

  function percentDecode(bytes) {
    var out = [];
    for (var i = 0; i < bytes.length; i++) {
      var b = bytes[i];
      if (b === 0x25 && i + 2 < bytes.length) {
        var h = parseInt(String.fromCharCode(bytes[i + 1], bytes[i + 2]), 16);
        if (!isNaN(h) && /^[0-9a-fA-F]{2}$/.test(String.fromCharCode(bytes[i + 1], bytes[i + 2]))) {
          out.push(h);
          i += 2;
          continue;
        }
      }
      out.push(b);
    }
    return new Uint8Array(out);
  }

  function parseUrlencoded(input) {
    var bytes = utf8(input);
    var list = [];
    var start = 0;
    for (var i = 0; i <= bytes.length; i++) {
      if (i < bytes.length && bytes[i] !== 0x26) continue;
      var seq = bytes.slice(start, i);
      start = i + 1;
      if (!seq.length) continue;
      var eq = seq.indexOf(0x3D);
      var name = eq < 0 ? seq : seq.slice(0, eq);
      var value = eq < 0 ? new Uint8Array(0) : seq.slice(eq + 1);
      name = name.map(function (b) { return b === 0x2B ? 0x20 : b; });
      value = value.map(function (b) { return b === 0x2B ? 0x20 : b; });
      list.push([decodeUtf8(percentDecode(name)), decodeUtf8(percentDecode(value))]);
    }
    return list;
  }

  function toUSV(s) { return String(s); }

  class URLSearchParams {
    constructor(init) {
      this._list = [];
      if (init === undefined || init === null) return;
      if (init instanceof URLSearchParams) {
        this._list = init._list.map(function (p) { return [p[0], p[1]]; });
      } else if (typeof init === 'object' && typeof init[Symbol.iterator] === 'function') {
        for (var pair of init) {
          var p = Array.from(pair);
          if (p.length !== 2) throw new TypeError('URLSearchParams pairs must have exactly two items');
          this._list.push([toUSV(p[0]), toUSV(p[1])]);
        }
      } else if (typeof init === 'object') {
        var self = this;
        Object.keys(init).forEach(function (k) { self._list.push([k, toUSV(init[k])]); });
      } else {
        var s = String(init);
        this._list = parseUrlencoded(s.charAt(0) === '?' ? s.slice(1) : s);
      }
    }
    get size() { return this._list.length; }
    // A URL's searchParams writes every change back to the URL's query.
    _update() {
      if (!this._url) return;
      var query = this.toString();
      this._url._setQuery(query === '' ? null : query);
    }
    append(name, value) { this._list.push([toUSV(name), toUSV(value)]); this._update(); }
    delete(name, value) {
      var n = toUSV(name), v = value === undefined ? undefined : toUSV(value);
      this._list = this._list.filter(function (p) { return !(p[0] === n && (v === undefined || p[1] === v)); });
      this._update();
    }
    get(name) {
      var n = toUSV(name);
      for (var i = 0; i < this._list.length; i++) if (this._list[i][0] === n) return this._list[i][1];
      return null;
    }
    getAll(name) {
      var n = toUSV(name);
      return this._list.filter(function (p) { return p[0] === n; }).map(function (p) { return p[1]; });
    }
    has(name, value) {
      var n = toUSV(name), v = value === undefined ? undefined : toUSV(value);
      return this._list.some(function (p) { return p[0] === n && (v === undefined || p[1] === v); });
    }
    set(name, value) {
      var n = toUSV(name), v = toUSV(value);
      var i = this._list.findIndex(function (p) { return p[0] === n; });
      if (i < 0) {
        this._list.push([n, v]);
      } else {
        this._list[i] = [n, v];
        this._list = this._list.filter(function (p, j) { return j <= i || p[0] !== n; });
      }
      this._update();
    }
    sort() {
      // Stable sort by UTF-16 code units of the name.
      var indexed = this._list.map(function (p, i) { return [p, i]; });
      indexed.sort(function (a, b) {
        if (a[0][0] < b[0][0]) return -1;
        if (a[0][0] > b[0][0]) return 1;
        return a[1] - b[1];
      });
      this._list = indexed.map(function (x) { return x[0]; });
      this._update();
    }
    forEach(callback, thisArg) {
      var list = this._list.slice();
      for (var i = 0; i < list.length; i++) callback.call(thisArg, list[i][1], list[i][0], this);
    }
    entries() { return this._list.map(function (p) { return [p[0], p[1]]; })[Symbol.iterator](); }
    keys() { return this._list.map(function (p) { return p[0]; })[Symbol.iterator](); }
    values() { return this._list.map(function (p) { return p[1]; })[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    toString() {
      return this._list.map(function (p) { return formEncode(p[0]) + '=' + formEncode(p[1]); }).join('&');
    }
    get [Symbol.toStringTag]() { return 'URLSearchParams'; }
  }

  // ---------------------------------------------------------------------------
  // URL (WHATWG URL Standard), parsed and mutated by the `url` crate in Rust
  // ---------------------------------------------------------------------------
  var urlParse = global.__axiom_urlParse;
  var urlSet = global.__axiom_urlSet;
  var URL_PARTS = ['href', 'origin', 'protocol', 'username', 'password', 'host', 'hostname',
                   'port', 'pathname', 'search', 'hash'];

  function parseOrNull(url, base) {
    var parts = base === undefined ? urlParse(String(url)) : urlParse(String(url), String(base));
    return parts === null ? null : parts;
  }

  class URL {
    constructor(url, base) {
      if (arguments.length === 0) throw new TypeError("Failed to construct 'URL': 1 argument required");
      var parts = parseOrNull(url, base);
      if (parts === null) {
        throw new TypeError("Failed to construct 'URL': Invalid URL");
      }
      this._parts = parts;
      this._params = new URLSearchParams(parts[9]);
      this._params._url = this;
    }
    static canParse(url, base) { return parseOrNull(url, base) !== null; }
    static parse(url, base) {
      try { return new URL(url, base); } catch (e) { return null; }
    }
    _set(name, value) {
      this._parts = urlSet(this._parts[0], name, String(value));
      if (name === 'href' || name === 'search') this._params._list = parseUrlencoded(this._parts[9].replace(/^\?/, ''));
    }
    _setQuery(query) { this._parts = urlSet(this._parts[0], 'search', query === null ? '' : query); }
    get searchParams() { return this._params; }
    toString() { return this._parts[0]; }
    toJSON() { return this._parts[0]; }
    get [Symbol.toStringTag]() { return 'URL'; }
  }
  URL_PARTS.forEach(function (name, i) {
    var desc = { get: function () { return this._parts[i]; }, enumerable: true, configurable: true };
    if (name !== 'origin') desc.set = function (v) { this._set(name, v); };
    Object.defineProperty(URL.prototype, name, desc);
  });
  URL.createObjectURL = function () {
    throw new DOMException('Blob URLs are not supported', 'NotSupportedError');
  };
  URL.revokeObjectURL = function () {};

  // ---------------------------------------------------------------------------
  // FormData (multipart/form-data)
  // ---------------------------------------------------------------------------
  function formEntryValue(value, filename) {
    if (value instanceof Blob) {
      var name = filename !== undefined ? String(filename)
        : (value instanceof File ? value._name : 'blob');
      if (value instanceof File && filename === undefined) return value;
      return makeFile(value._bytes, name, value._type,
        value instanceof File ? value._lastModified : undefined);
    }
    return String(value);
  }

  class FormData {
    constructor(form, submitter) {
      this._list = [];
      if (form === undefined) return;
      var entries = global.__axiom_formEntryList ? global.__axiom_formEntryList(form, submitter) : null;
      if (entries === null) {
        throw new TypeError("Failed to construct 'FormData': parameter 1 is not of type 'HTMLFormElement'.");
      }
      var list = this._list;
      entries.forEach(function (e) {
        list.push([e[0], e[2] ? makeFile(new Uint8Array(0), '', 'application/octet-stream') : e[1]]);
      });
    }
    append(name, value, filename) {
      this._list.push([String(name), formEntryValue(value, filename)]);
    }
    delete(name) {
      var n = String(name);
      this._list = this._list.filter(function (p) { return p[0] !== n; });
    }
    get(name) {
      var n = String(name);
      for (var i = 0; i < this._list.length; i++) if (this._list[i][0] === n) return this._list[i][1];
      return null;
    }
    getAll(name) {
      var n = String(name);
      return this._list.filter(function (p) { return p[0] === n; }).map(function (p) { return p[1]; });
    }
    has(name) {
      var n = String(name);
      return this._list.some(function (p) { return p[0] === n; });
    }
    set(name, value, filename) {
      var n = String(name), v = formEntryValue(value, filename);
      var i = this._list.findIndex(function (p) { return p[0] === n; });
      if (i < 0) { this._list.push([n, v]); return; }
      this._list[i] = [n, v];
      this._list = this._list.filter(function (p, j) { return j <= i || p[0] !== n; });
    }
    forEach(callback, thisArg) {
      var list = this._list.slice();
      for (var i = 0; i < list.length; i++) callback.call(thisArg, list[i][1], list[i][0], this);
    }
    entries() { return this._list.map(function (p) { return [p[0], p[1]]; })[Symbol.iterator](); }
    keys() { return this._list.map(function (p) { return p[0]; })[Symbol.iterator](); }
    values() { return this._list.map(function (p) { return p[1]; })[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    get [Symbol.toStringTag]() { return 'FormData'; }
  }

  function escapeMultipartName(s) {
    return s.replace(/\n/g, '%0A').replace(/\r/g, '%0D').replace(/"/g, '%22');
  }

  function randomBoundary() {
    var chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
    var s = '----AxiomFormBoundary';
    for (var i = 0; i < 16; i++) s += chars[Math.floor(Math.random() * chars.length)];
    return s;
  }

  // HTML "multipart/form-data encoding algorithm" (UTF-8).
  function serializeFormData(fd) {
    var boundary = randomBoundary();
    var parts = [];
    fd._list.forEach(function (entry) {
      var name = escapeMultipartName(entry[0].replace(/\r\n|\r|\n/g, '\r\n'));
      var head = '--' + boundary + '\r\nContent-Disposition: form-data; name="' + name + '"';
      if (entry[1] instanceof File) {
        var f = entry[1];
        head += '; filename="' + escapeMultipartName(f._name) + '"\r\nContent-Type: ' +
          (f._type || 'application/octet-stream') + '\r\n\r\n';
        parts.push(utf8(head), f._bytes, utf8('\r\n'));
      } else {
        head += '\r\n\r\n';
        parts.push(utf8(head + entry[1].replace(/\r\n|\r|\n/g, '\r\n') + '\r\n'));
      }
    });
    parts.push(utf8('--' + boundary + '--\r\n'));
    return { bytes: concatBytes(parts), type: 'multipart/form-data; boundary=' + boundary };
  }

  function indexOfBytes(hay, needle, from) {
    outer: for (var i = from; i <= hay.length - needle.length; i++) {
      for (var j = 0; j < needle.length; j++) if (hay[i + j] !== needle[j]) continue outer;
      return i;
    }
    return -1;
  }

  function headerParam(value, name) {
    var re = new RegExp('(?:^|;)\\s*' + name + '\\s*=\\s*(?:"((?:[^"\\\\]|\\\\.)*)"|([^;\\s]*))', 'i');
    var m = re.exec(value);
    if (!m) return null;
    return m[1] !== undefined ? m[1].replace(/\\(.)/g, '$1') : m[2];
  }

  function parseMultipart(bytes, contentType) {
    var boundary = headerParam(contentType, 'boundary');
    if (!boundary) throw new TypeError('multipart/form-data body has no boundary');
    var delim = utf8('--' + boundary);
    var fd = new FormData();
    var pos = indexOfBytes(bytes, delim, 0);
    if (pos < 0) throw new TypeError('Malformed multipart/form-data body');
    while (true) {
      pos += delim.length;
      if (bytes[pos] === 0x2D && bytes[pos + 1] === 0x2D) break; // closing delimiter
      if (bytes[pos] === 0x0D && bytes[pos + 1] === 0x0A) pos += 2;
      var headerEnd = indexOfBytes(bytes, utf8('\r\n\r\n'), pos);
      if (headerEnd < 0) throw new TypeError('Malformed multipart/form-data body');
      var headers = decodeUtf8(bytes.slice(pos, headerEnd)).split('\r\n');
      var disposition = null, type = '';
      headers.forEach(function (line) {
        var c = line.indexOf(':');
        if (c < 0) return;
        var k = line.slice(0, c).trim().toLowerCase(), v = line.slice(c + 1).trim();
        if (k === 'content-disposition') disposition = v;
        else if (k === 'content-type') type = v;
      });
      var next = indexOfBytes(bytes, utf8('\r\n--' + boundary), headerEnd + 4);
      if (next < 0 || disposition === null) throw new TypeError('Malformed multipart/form-data body');
      var body = bytes.slice(headerEnd + 4, next);
      var name = headerParam(disposition, 'name');
      if (name === null) throw new TypeError('multipart/form-data part has no name');
      var filename = headerParam(disposition, 'filename');
      if (filename !== null) {
        fd._list.push([name, makeFile(body, filename, type || 'text/plain')]);
      } else {
        fd._list.push([name, decodeUtf8(body)]);
      }
      pos = next + 2;
    }
    return fd;
  }

  // Shared with the fetch bindings (body extraction and formData()).
  global.__axiom_platform = {
    serializeFormData: serializeFormData,
    parseMultipart: parseMultipart,
    parseUrlencoded: parseUrlencoded,
    makeBlob: function (bytes, type) {
      var b = Object.create(Blob.prototype);
      b._bytes = bytes;
      b._type = normalizeType(type);
      return b;
    },
    formDataFromList: function (list) {
      var fd = new FormData();
      list.forEach(function (p) { fd._list.push([p[0], p[1]]); });
      return fd;
    }
  };

  global.Blob = Blob;
  global.File = File;
  global.URLSearchParams = URLSearchParams;
  global.URL = URL;
  global.webkitURL = URL;
  global.FormData = FormData;
  // console: every method exists (pages call them unconditionally); output goes to the
  // host log. Grouping indents, timers and counters follow the Console Standard.
  var con = global.console;
  var log = con.log;
  var indent = '';
  function emit(args) {
    var parts = Array.prototype.slice.call(args);
    if (indent) parts.unshift(indent.substring(1));
    log.apply(null, parts);
  }
  ['log', 'error', 'warn', 'info', 'debug', 'trace', 'dir', 'dirxml', 'table'].forEach(function (name) {
    con[name] = function () { emit(arguments); };
  });
  con.assert = function (condition) {
    if (condition) return;
    var rest = Array.prototype.slice.call(arguments, 1);
    rest.unshift('Assertion failed' + (rest.length ? ':' : ''));
    emit(rest);
  };
  con.group = con.groupCollapsed = function () {
    if (arguments.length) emit(arguments);
    indent += '  ';
  };
  con.groupEnd = function () { indent = indent.substring(2); };
  var counts = Object.create(null);
  con.count = function (label) {
    label = label === undefined ? 'default' : String(label);
    counts[label] = (counts[label] || 0) + 1;
    emit([label + ': ' + counts[label]]);
  };
  con.countReset = function (label) { counts[label === undefined ? 'default' : String(label)] = 0; };
  var timers = Object.create(null);
  function now() { return typeof __axiom_now === 'function' ? __axiom_now() : Date.now(); }
  con.time = function (label) { timers[label === undefined ? 'default' : String(label)] = now(); };
  function elapsed(label, end) {
    label = label === undefined ? 'default' : String(label);
    if (!(label in timers)) return;
    emit([label + ': ' + (now() - timers[label]) + ' ms']);
    if (end) delete timers[label];
  }
  con.timeLog = function (label) { elapsed(label, false); };
  con.timeEnd = function (label) { elapsed(label, true); };
  con.timeStamp = con.profile = con.profileEnd = con.clear = function () {};
})(this);
