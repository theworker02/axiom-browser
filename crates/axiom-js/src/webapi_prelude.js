// Web platform APIs without a prelude of their own: the performance timeline (User
// Timing marks and measures, PerformanceObserver, legacy timing / navigation), atob /
// btoa, crypto.getRandomValues / randomUUID, structuredClone, element dataset, the CSS
// namespace, document visibility, focus() / blur(), reportError, MessageChannel and
// window.postMessage, TreeWalker / NodeIterator, XMLHttpRequest on top of fetch(), and
// the History interface.
// Not implemented: synchronous XMLHttpRequest (send() throws NetworkError), responseXML
// and responseType "document" (null), NodeIterator pre-removing steps, crypto.subtle,
// resource / navigation / paint timing entries.
(function (global) {
  'use strict';
  var doc = global.document;
  var events = global.__axiom_events;
  var EventTarget = global.EventTarget;
  var DOMException = global.DOMException;
  var setTimeout_ = global.setTimeout;
  var clearTimeout_ = global.clearTimeout;
  var fetch_ = global.fetch;
  var reportError_ = global.__axiom_reportError;

  function getter(obj, name, get, set) {
    Object.defineProperty(obj, name, { get: get, set: set, enumerable: true, configurable: true });
  }
  function method(obj, name, fn) {
    Object.defineProperty(obj, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  function constant(obj, name, value) {
    Object.defineProperty(obj, name, { value: value, enumerable: true });
  }
  function expose(name, value) {
    Object.defineProperty(global, name, { value: value, writable: true, configurable: true });
  }
  function domError(name, message) { return new DOMException(message, name); }
  function illegal() { throw new TypeError('Illegal constructor'); }
  function requireNew(target, name) {
    if (target === undefined) {
      throw new TypeError("Failed to construct '" + name + "': Please use the 'new' operator.");
    }
  }
  function requireArgs(args, n, what) {
    if (args.length < n) {
      throw new TypeError('Failed to execute ' + what + ': ' + n + ' argument' + (n > 1 ? 's' : '') +
        ' required, but only ' + args.length + ' present.');
    }
  }
  function iface(name, F, parent) {
    if (parent) {
      F.prototype = Object.create(parent.prototype);
      Object.setPrototypeOf(F, parent);
    }
    Object.defineProperty(F.prototype, 'constructor', { value: F, writable: true, configurable: true });
    Object.defineProperty(F.prototype, Symbol.toStringTag, { value: name, configurable: true });
    expose(name, F);
    return F;
  }
  function stateOf(map, obj) {
    var s = map.get(obj);
    if (!s) throw new TypeError('Illegal invocation');
    return s;
  }
  function fire(target, event) { events.dispatchTrusted(target, event); }

  // ---------------------------------------------------------------------------
  // structuredClone (HTML §2.7.3 StructuredSerialize / Deserialize, in one pass)
  // ---------------------------------------------------------------------------
  var ERROR_NAMES = ['Error', 'EvalError', 'RangeError', 'ReferenceError', 'SyntaxError', 'TypeError', 'URIError'];
  var TYPED_ARRAYS = ['Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array',
    'Int32Array', 'Uint32Array', 'Float32Array', 'Float64Array', 'BigInt64Array', 'BigUint64Array'];
  function dataCloneError(what) {
    return domError('DataCloneError', what + ' could not be cloned.');
  }
  function cloneValue(v, memory) {
    var t = typeof v;
    if (v === null || t === 'undefined' || t === 'boolean' || t === 'number' || t === 'string' || t === 'bigint') {
      return v;
    }
    if (t === 'symbol') throw dataCloneError('Symbol()');
    if (t === 'function') throw dataCloneError('function ' + (v.name || '') + '()');
    if (memory.has(v)) return memory.get(v);
    var tag = Object.prototype.toString.call(v).slice(8, -1);
    var out;
    if (global.File && v instanceof global.File) {
      out = new global.File([v], v.name, { type: v.type, lastModified: v.lastModified });
    } else if (global.Blob && v instanceof global.Blob) {
      out = v.slice(0, v.size, v.type);
    } else if (tag === 'Boolean') {
      out = Object(Boolean.prototype.valueOf.call(v));
    } else if (tag === 'Number') {
      out = Object(Number.prototype.valueOf.call(v));
    } else if (tag === 'String') {
      out = Object(String.prototype.valueOf.call(v));
    } else if (tag === 'BigInt') {
      out = Object(BigInt.prototype.valueOf.call(v));
    } else if (tag === 'Date') {
      out = new Date(Date.prototype.getTime.call(v));
    } else if (tag === 'RegExp') {
      out = new RegExp(v.source, v.flags);
    } else if (tag === 'ArrayBuffer') {
      out = ArrayBuffer.prototype.slice.call(v, 0);
    } else if (tag === 'DataView') {
      out = new DataView(cloneValue(v.buffer, memory), v.byteOffset, v.byteLength);
    } else if (TYPED_ARRAYS.indexOf(tag) >= 0) {
      out = new global[tag](cloneValue(v.buffer, memory), v.byteOffset, v.length);
    } else if (tag === 'Map') {
      out = new Map();
      memory.set(v, out);
      var entries = [];
      Map.prototype.forEach.call(v, function (value, key) { entries.push([key, value]); });
      entries.forEach(function (e) { out.set(cloneValue(e[0], memory), cloneValue(e[1], memory)); });
      return out;
    } else if (tag === 'Set') {
      out = new Set();
      memory.set(v, out);
      var values = [];
      Set.prototype.forEach.call(v, function (value) { values.push(value); });
      values.forEach(function (value) { out.add(cloneValue(value, memory)); });
      return out;
    } else if (tag === 'Error') {
      var name = ERROR_NAMES.indexOf(v.name) >= 0 ? v.name : 'Error';
      out = new global[name]();
      var message = Object.getOwnPropertyDescriptor(v, 'message');
      if (message && 'value' in message) out.message = String(message.value);
      else delete out.message;
      if (typeof v.stack === 'string') {
        Object.defineProperty(out, 'stack', { value: v.stack, writable: true, configurable: true });
      }
    } else if (tag === 'Array') {
      out = new Array(v.length);
      memory.set(v, out);
      Object.keys(v).forEach(function (k) { out[k] = cloneValue(v[k], memory); });
      return out;
    } else if (tag === 'Object' && !isPlatformObject(v)) {
      out = {};
      memory.set(v, out);
      Object.keys(v).forEach(function (k) { out[k] = cloneValue(v[k], memory); });
      return out;
    } else {
      throw dataCloneError(Object.prototype.toString.call(v));
    }
    memory.set(v, out);
    return out;
  }
  function isPlatformObject(v) {
    return typeof v.__id === 'number' || v instanceof EventTarget ||
      (global.Event && v instanceof global.Event);
  }
  function structuredClone(value) {
    requireArgs(arguments, 1, "'structuredClone' on 'Window'");
    return cloneValue(value, new Map());
  }
  method(global, 'structuredClone', structuredClone);

  // ---------------------------------------------------------------------------
  // Performance timeline: User Timing marks / measures and PerformanceObserver
  // ---------------------------------------------------------------------------
  var perf = global.performance;
  var PP = global.Performance.prototype;
  var ENTRY = new WeakMap();
  var BUFFER = [];
  var TIMING_NAMES = ['navigationStart', 'unloadEventStart', 'unloadEventEnd', 'redirectStart',
    'redirectEnd', 'fetchStart', 'domainLookupStart', 'domainLookupEnd', 'connectStart', 'connectEnd',
    'secureConnectionStart', 'requestStart', 'responseStart', 'responseEnd', 'domLoading',
    'domInteractive', 'domContentLoadedEventStart', 'domContentLoadedEventEnd', 'domComplete',
    'loadEventStart', 'loadEventEnd'];

  function PerformanceEntry() { illegal(); }
  iface('PerformanceEntry', PerformanceEntry);
  ['name', 'entryType', 'startTime', 'duration'].forEach(function (k) {
    getter(PerformanceEntry.prototype, k, function () { return stateOf(ENTRY, this)[k]; });
  });
  method(PerformanceEntry.prototype, 'toJSON', function toJSON() {
    var s = stateOf(ENTRY, this);
    var out = { name: s.name, entryType: s.entryType, startTime: s.startTime, duration: s.duration };
    if (s.entryType === 'mark' || s.entryType === 'measure') out.detail = s.detail;
    return out;
  });

  function PerformanceMark(name, options) {
    requireNew(new.target, 'PerformanceMark');
    requireArgs(arguments, 1, "'PerformanceMark'");
    name = String(name);
    options = options === undefined || options === null ? {} : options;
    if (TIMING_NAMES.indexOf(name) >= 0) {
      throw domError('SyntaxError', "Failed to construct 'PerformanceMark': '" + name +
        "' is part of the PerformanceTiming interface, and cannot be used as a mark name.");
    }
    var start = options.startTime === undefined ? perf.now() : Number(options.startTime);
    if (!(start >= 0)) {
      throw new TypeError("Failed to construct 'PerformanceMark': '" + options.startTime +
        "' cannot have a negative start time.");
    }
    var detail = options.detail === undefined ? null : cloneValue(options.detail, new Map());
    ENTRY.set(this, { name: name, entryType: 'mark', startTime: start, duration: 0, detail: detail });
  }
  iface('PerformanceMark', PerformanceMark, PerformanceEntry);
  getter(PerformanceMark.prototype, 'detail', function () { return stateOf(ENTRY, this).detail; });

  function PerformanceMeasure() { illegal(); }
  iface('PerformanceMeasure', PerformanceMeasure, PerformanceEntry);
  getter(PerformanceMeasure.prototype, 'detail', function () { return stateOf(ENTRY, this).detail; });

  function entries(list, name, type) {
    return list.filter(function (e) {
      var s = ENTRY.get(e);
      return (name === undefined || s.name === name) && (type === undefined || s.entryType === type);
    }).sort(function (a, b) { return ENTRY.get(a).startTime - ENTRY.get(b).startTime; });
  }

  // Legacy PerformanceTiming: fetch and response times collapse to navigationStart; DOM
  // milestones are recorded when they happen.
  var navigationStart = Math.round(perf.timeOrigin);
  var timingValues = {};
  TIMING_NAMES.forEach(function (n) { timingValues[n] = 0; });
  ['navigationStart', 'fetchStart', 'domainLookupStart', 'domainLookupEnd', 'connectStart',
    'connectEnd', 'requestStart', 'responseStart', 'responseEnd', 'domLoading'].forEach(function (n) {
    timingValues[n] = navigationStart;
  });
  function stamp() { return Math.round(perf.timeOrigin + perf.now()); }
  doc.addEventListener('readystatechange', function () {
    if (doc.readyState === 'interactive' && !timingValues.domInteractive) timingValues.domInteractive = stamp();
    if (doc.readyState === 'complete' && !timingValues.domComplete) timingValues.domComplete = stamp();
  });
  doc.addEventListener('DOMContentLoaded', function () {
    timingValues.domContentLoadedEventStart = timingValues.domContentLoadedEventEnd = stamp();
  });
  global.addEventListener('load', function () {
    timingValues.loadEventStart = timingValues.loadEventEnd = stamp();
  });
  function PerformanceTiming() { illegal(); }
  iface('PerformanceTiming', PerformanceTiming);
  TIMING_NAMES.forEach(function (n) {
    getter(PerformanceTiming.prototype, n, function () { return timingValues[n]; });
  });
  method(PerformanceTiming.prototype, 'toJSON', function toJSON() {
    var out = {};
    TIMING_NAMES.forEach(function (n) { out[n] = timingValues[n]; });
    return out;
  });
  var timing = Object.create(PerformanceTiming.prototype);
  getter(PP, 'timing', function () { return timing; });

  function PerformanceNavigation() { illegal(); }
  iface('PerformanceNavigation', PerformanceNavigation);
  [['TYPE_NAVIGATE', 0], ['TYPE_RELOAD', 1], ['TYPE_BACK_FORWARD', 2], ['TYPE_RESERVED', 255]].forEach(function (c) {
    constant(PerformanceNavigation, c[0], c[1]);
    constant(PerformanceNavigation.prototype, c[0], c[1]);
  });
  getter(PerformanceNavigation.prototype, 'type', function () { return 0; });
  getter(PerformanceNavigation.prototype, 'redirectCount', function () { return 0; });
  method(PerformanceNavigation.prototype, 'toJSON', function toJSON() { return { type: 0, redirectCount: 0 }; });
  var navigation = Object.create(PerformanceNavigation.prototype);
  getter(PP, 'navigation', function () { return navigation; });

  function markTime(value) {
    if (typeof value === 'string') {
      if (TIMING_NAMES.indexOf(value) >= 0) {
        var t = timingValues[value];
        if (t === 0) {
          throw domError('InvalidAccessError', "Failed to execute 'measure' on 'Performance': '" + value +
            "' is empty: either the event hasn't happened yet, or it would provide cross-origin timing information.");
        }
        return t - navigationStart;
      }
      var marks = entries(BUFFER, value, 'mark');
      if (!marks.length) {
        throw domError('SyntaxError', "Failed to execute 'measure' on 'Performance': The mark '" + value +
          "' does not exist.");
      }
      return ENTRY.get(marks[marks.length - 1]).startTime;
    }
    var n = Number(value);
    if (n < 0) throw new TypeError("Failed to execute 'measure' on 'Performance': '" + value + "' cannot be negative.");
    return n;
  }

  method(PP, 'mark', function mark(name, options) {
    requireArgs(arguments, 1, "'mark' on 'Performance'");
    var m = new PerformanceMark(name, options);
    queueEntry(m);
    return m;
  });
  method(PP, 'measure', function measure(name, startOrOptions, endMark) {
    requireArgs(arguments, 1, "'measure' on 'Performance'");
    name = String(name);
    var isDict = startOrOptions === undefined || startOrOptions === null ||
      typeof startOrOptions === 'object' || typeof startOrOptions === 'function';
    var o = isDict && startOrOptions ? startOrOptions : {};
    var nonEmpty = isDict && (o.start !== undefined || o.end !== undefined || o.duration !== undefined ||
      o.detail !== undefined);
    if (nonEmpty) {
      if (endMark !== undefined) throw new TypeError("Failed to execute 'measure' on 'Performance': If a non-empty PerformanceMeasureOptions object was passed, |end_mark| must not be passed.");
      if (o.start === undefined && o.end === undefined) throw new TypeError("Failed to execute 'measure' on 'Performance': If a non-empty PerformanceMeasureOptions object was passed, at least one of its 'start' or 'end' properties must be present.");
      if (o.start !== undefined && o.end !== undefined && o.duration !== undefined) throw new TypeError("Failed to execute 'measure' on 'Performance': If a non-empty PerformanceMeasureOptions object was passed, it must not have all of its 'start', 'duration', and 'end' properties defined");
    }
    var end, start;
    if (endMark !== undefined) end = markTime(String(endMark));
    else if (nonEmpty && o.end !== undefined) end = markTime(o.end);
    else if (nonEmpty && o.start !== undefined && o.duration !== undefined) end = markTime(o.start) + Number(o.duration);
    else end = perf.now();
    if (nonEmpty && o.start !== undefined) start = markTime(o.start);
    else if (nonEmpty && o.duration !== undefined && o.end !== undefined) start = end - Number(o.duration);
    else if (!isDict) start = markTime(String(startOrOptions));
    else start = 0;
    var detail = nonEmpty && o.detail !== undefined ? cloneValue(o.detail, new Map()) : null;
    var m = Object.create(PerformanceMeasure.prototype);
    ENTRY.set(m, { name: name, entryType: 'measure', startTime: start, duration: end - start, detail: detail });
    queueEntry(m);
    return m;
  });
  function clearer(type) {
    return function (name) {
      BUFFER = BUFFER.filter(function (e) {
        var s = ENTRY.get(e);
        return s.entryType !== type || (name !== undefined && s.name !== String(name));
      });
    };
  }
  method(PP, 'clearMarks', clearer('mark'));
  method(PP, 'clearMeasures', clearer('measure'));
  method(PP, 'clearResourceTimings', function clearResourceTimings() {});
  method(PP, 'setResourceTimingBufferSize', function setResourceTimingBufferSize() {});
  method(PP, 'getEntries', function getEntries() { return entries(BUFFER); });
  method(PP, 'getEntriesByType', function getEntriesByType(type) {
    requireArgs(arguments, 1, "'getEntriesByType' on 'Performance'");
    return entries(BUFFER, undefined, String(type));
  });
  method(PP, 'getEntriesByName', function getEntriesByName(name, type) {
    requireArgs(arguments, 1, "'getEntriesByName' on 'Performance'");
    return entries(BUFFER, String(name), type === undefined ? undefined : String(type));
  });
  var baseToJSON = PP.toJSON;
  method(PP, 'toJSON', function toJSON() {
    var out = baseToJSON.call(this);
    out.timing = timing.toJSON();
    out.navigation = navigation.toJSON();
    return out;
  });

  var OBSERVER = new WeakMap();
  var OBSERVERS = [];
  var SUPPORTED_TYPES = Object.freeze(['mark', 'measure']);
  function PerformanceObserverEntryList() { illegal(); }
  iface('PerformanceObserverEntryList', PerformanceObserverEntryList);
  var LISTS = new WeakMap();
  method(PerformanceObserverEntryList.prototype, 'getEntries', function getEntries() {
    return entries(stateOf(LISTS, this));
  });
  method(PerformanceObserverEntryList.prototype, 'getEntriesByType', function getEntriesByType(type) {
    return entries(stateOf(LISTS, this), undefined, String(type));
  });
  method(PerformanceObserverEntryList.prototype, 'getEntriesByName', function getEntriesByName(name, type) {
    return entries(stateOf(LISTS, this), String(name), type === undefined ? undefined : String(type));
  });

  function PerformanceObserver(callback) {
    requireNew(new.target, 'PerformanceObserver');
    if (typeof callback !== 'function') {
      throw new TypeError("Failed to construct 'PerformanceObserver': parameter 1 is not of type 'Function'.");
    }
    OBSERVER.set(this, { callback: callback, types: [], buffer: [], mode: null, pending: false });
  }
  iface('PerformanceObserver', PerformanceObserver);
  getter(PerformanceObserver, 'supportedEntryTypes', function () { return SUPPORTED_TYPES; });
  method(PerformanceObserver.prototype, 'observe', function observe(options) {
    var s = stateOf(OBSERVER, this);
    options = options || {};
    var hasList = options.entryTypes !== undefined;
    var hasType = options.type !== undefined;
    if (!hasList && !hasType) {
      throw new TypeError("Failed to execute 'observe' on 'PerformanceObserver': An observe() call must include either entryTypes or type arguments.");
    }
    if (hasList && (hasType || options.buffered !== undefined)) {
      throw new TypeError("Failed to execute 'observe' on 'PerformanceObserver': An observe() call must not include both entryTypes and type arguments.");
    }
    if ((s.mode === 'multiple' && hasType) || (s.mode === 'single' && hasList)) {
      throw domError('InvalidModificationError', "Failed to execute 'observe' on 'PerformanceObserver': This observer has performed observe({" +
        (s.mode === 'multiple' ? 'entryTypes' : 'type') + '}) and therefore can no longer perform observe({' +
        (hasType ? 'type' : 'entryTypes') + '}).');
    }
    if (hasList) {
      var types = Array.prototype.slice.call(options.entryTypes).map(String).filter(function (t) {
        return SUPPORTED_TYPES.indexOf(t) >= 0;
      });
      if (!types.length) return;
      s.mode = 'multiple';
      s.types = types;
    } else {
      var type = String(options.type);
      if (SUPPORTED_TYPES.indexOf(type) < 0) return;
      s.mode = 'single';
      if (s.types.indexOf(type) < 0) s.types.push(type);
      if (options.buffered) {
        entries(BUFFER, undefined, type).forEach(function (e) { s.buffer.push(e); });
        schedule(this, s);
      }
    }
    if (OBSERVERS.indexOf(this) < 0) OBSERVERS.push(this);
  });
  method(PerformanceObserver.prototype, 'disconnect', function disconnect() {
    var s = stateOf(OBSERVER, this);
    var i = OBSERVERS.indexOf(this);
    if (i >= 0) OBSERVERS.splice(i, 1);
    s.buffer = [];
    s.types = [];
    s.mode = null;
  });
  method(PerformanceObserver.prototype, 'takeRecords', function takeRecords() {
    var s = stateOf(OBSERVER, this);
    return s.buffer.splice(0);
  });
  function schedule(observer, s) {
    if (s.pending) return;
    s.pending = true;
    setTimeout_(function () {
      s.pending = false;
      if (!s.buffer.length) return;
      var list = Object.create(PerformanceObserverEntryList.prototype);
      LISTS.set(list, s.buffer.splice(0));
      try {
        s.callback.call(observer, list, observer, { droppedEntriesCount: 0 });
      } catch (e) {
        reportError_(e);
      }
    }, 0);
  }
  function queueEntry(entry) {
    BUFFER.push(entry);
    var type = ENTRY.get(entry).entryType;
    OBSERVERS.forEach(function (o) {
      var s = OBSERVER.get(o);
      if (s.types.indexOf(type) < 0) return;
      s.buffer.push(entry);
      schedule(o, s);
    });
  }

  // ---------------------------------------------------------------------------
  // atob / btoa (forgiving-base64, HTML §8.3)
  // ---------------------------------------------------------------------------
  var B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  method(global, 'btoa', function btoa(data) {
    requireArgs(arguments, 1, "'btoa' on 'Window'");
    var s = String(data);
    var out = '';
    for (var i = 0; i < s.length; i += 3) {
      var a = s.charCodeAt(i), b = s.charCodeAt(i + 1), c = s.charCodeAt(i + 2);
      if (a > 255 || b > 255 || c > 255) {
        throw domError('InvalidCharacterError', "Failed to execute 'btoa' on 'Window': The string to be encoded contains characters outside of the Latin1 range.");
      }
      var n = (a << 16) | ((b || 0) << 8) | (c || 0);
      out += B64.charAt(n >> 18) + B64.charAt((n >> 12) & 63) +
        (i + 1 < s.length ? B64.charAt((n >> 6) & 63) : '=') +
        (i + 2 < s.length ? B64.charAt(n & 63) : '=');
    }
    return out;
  });
  method(global, 'atob', function atob(data) {
    requireArgs(arguments, 1, "'atob' on 'Window'");
    var s = String(data).replace(/[\t\n\f\r ]/g, '');
    if (s.length % 4 === 0) s = s.replace(/==?$/, '');
    if (s.length % 4 === 1 || /[^A-Za-z0-9+\/]/.test(s)) {
      throw domError('InvalidCharacterError', "Failed to execute 'atob' on 'Window': The string to be decoded is not correctly encoded.");
    }
    var out = '';
    var bits = 0, acc = 0;
    for (var i = 0; i < s.length; i++) {
      acc = (acc << 6) | B64.indexOf(s.charAt(i));
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out += String.fromCharCode((acc >> bits) & 255);
      }
    }
    return out;
  });

  // ---------------------------------------------------------------------------
  // crypto.getRandomValues / randomUUID (OS randomness)
  // ---------------------------------------------------------------------------
  var INTEGER_ARRAYS = ['Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array',
    'Int32Array', 'Uint32Array', 'BigInt64Array', 'BigUint64Array'];
  function Crypto() { illegal(); }
  iface('Crypto', Crypto);
  var crypto = Object.create(Crypto.prototype);
  method(Crypto.prototype, 'getRandomValues', function getRandomValues(array) {
    if (this !== crypto) throw new TypeError('Illegal invocation');
    requireArgs(arguments, 1, "'getRandomValues' on 'Crypto'");
    var tag = ArrayBuffer.isView(array) ? Object.prototype.toString.call(array).slice(8, -1) : '';
    if (INTEGER_ARRAYS.indexOf(tag) < 0) {
      throw domError('TypeMismatchError', "Failed to execute 'getRandomValues' on 'Crypto': The provided ArrayBufferView is of type '" +
        (tag || typeof array) + "', which is not an integer array type.");
    }
    if (array.byteLength > 65536) {
      throw domError('QuotaExceededError', "Failed to execute 'getRandomValues' on 'Crypto': The ArrayBufferView's byte length (" +
        array.byteLength + ') exceeds the number of bytes of entropy available via this API (65536).');
    }
    new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(__axiom_randomBytes(array.byteLength));
    return array;
  });
  method(Crypto.prototype, 'randomUUID', function randomUUID() {
    if (this !== crypto) throw new TypeError('Illegal invocation');
    var b = __axiom_randomBytes(16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    var hex = b.map(function (x) { return (x + 0x100).toString(16).slice(1); }).join('');
    return hex.slice(0, 8) + '-' + hex.slice(8, 12) + '-' + hex.slice(12, 16) + '-' + hex.slice(16, 20) + '-' + hex.slice(20);
  });
  getter(global, 'crypto', function () { return crypto; });

  // ---------------------------------------------------------------------------
  // HTMLElement / SVGElement / MathMLElement dataset (HTML §3.2.6.6)
  // ---------------------------------------------------------------------------
  function DOMStringMap() { illegal(); }
  iface('DOMStringMap', DOMStringMap);
  var DATASETS = new WeakMap();
  function hasDashLower(p) { return /-[a-z]/.test(p); }
  function attrName(p) { return 'data-' + p.replace(/[A-Z]/g, function (c) { return '-' + c.toLowerCase(); }); }
  function propName(a) { return a.slice(5).replace(/-([a-z])/g, function (m, c) { return c.toUpperCase(); }); }
  function dataValue(el, p) {
    return typeof p === 'string' && !hasDashLower(p) ? el.getAttribute(attrName(p)) : null;
  }
  function datasetFor(el) {
    var ds = DATASETS.get(el);
    if (ds) return ds;
    ds = new Proxy(Object.create(DOMStringMap.prototype), {
      get: function (t, p, r) {
        var v = dataValue(el, p);
        return v !== null ? v : Reflect.get(t, p, r);
      },
      set: function (t, p, v, r) {
        if (typeof p === 'symbol') return Reflect.set(t, p, v, r);
        if (hasDashLower(p)) {
          throw domError('SyntaxError', "Failed to set a named property '" + p + "' on 'DOMStringMap': '" + p +
            "' is not a valid property name.");
        }
        el.setAttribute(attrName(p), String(v));
        return true;
      },
      deleteProperty: function (t, p) {
        if (dataValue(el, p) !== null) { el.removeAttribute(attrName(p)); return true; }
        return Reflect.deleteProperty(t, p);
      },
      has: function (t, p) { return dataValue(el, p) !== null || Reflect.has(t, p); },
      ownKeys: function (t) {
        var names = [];
        el.getAttributeNames().forEach(function (a) {
          if (a.slice(0, 5) === 'data-' && !/[A-Z]/.test(a)) {
            var p = propName(a);
            if (names.indexOf(p) < 0) names.push(p);
          }
        });
        return names.concat(Reflect.ownKeys(t));
      },
      getOwnPropertyDescriptor: function (t, p) {
        var v = dataValue(el, p);
        if (v !== null) return { value: v, writable: true, enumerable: true, configurable: true };
        return Reflect.getOwnPropertyDescriptor(t, p);
      },
      defineProperty: function (t, p, desc) {
        if (typeof p === 'symbol' || !('value' in desc)) return Reflect.defineProperty(t, p, desc);
        if (hasDashLower(p)) throw domError('SyntaxError', "'" + p + "' is not a valid property name.");
        el.setAttribute(attrName(p), String(desc.value));
        return true;
      }
    });
    DATASETS.set(el, ds);
    return ds;
  }
  ['HTMLElement', 'SVGElement', 'MathMLElement'].forEach(function (name) {
    if (global[name]) getter(global[name].prototype, 'dataset', function () { return datasetFor(this); });
  });

  // ---------------------------------------------------------------------------
  // CSS namespace: escape() and supports() (the @supports evaluator of the style engine)
  // ---------------------------------------------------------------------------
  var CSS = {};
  method(CSS, 'escape', function escape(ident) {
    requireArgs(arguments, 1, "'escape' on 'CSS'");
    var s = String(ident);
    var out = '';
    var first = s.charCodeAt(0);
    for (var i = 0; i < s.length; i++) {
      var c = s.charCodeAt(i);
      if (c === 0) out += '\uFFFD';
      else if ((c >= 1 && c <= 0x1f) || c === 0x7f || (i === 0 && c >= 0x30 && c <= 0x39) ||
        (i === 1 && c >= 0x30 && c <= 0x39 && first === 0x2d)) out += '\\' + c.toString(16) + ' ';
      else if (i === 0 && s.length === 1 && c === 0x2d) out += '\\-';
      else if (c >= 0x80 || c === 0x2d || c === 0x5f || (c >= 0x30 && c <= 0x39) ||
        (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a)) out += s.charAt(i);
      else out += '\\' + s.charAt(i);
    }
    return out;
  });
  method(CSS, 'supports', function supports(conditionOrProperty, value) {
    requireArgs(arguments, 1, "'supports' on 'CSS'");
    if (arguments.length >= 2) {
      return __axiom_cssSupports('(' + String(conditionOrProperty) + ': ' + String(value) + ')');
    }
    var c = String(conditionOrProperty);
    return __axiom_cssSupports(c) || __axiom_cssSupports('(' + c + ')');
  });
  expose('CSS', CSS);

  // ---------------------------------------------------------------------------
  // Document visibility, window origin / isSecureContext, reportError
  // ---------------------------------------------------------------------------
  var DP = global.Document.prototype;
  getter(DP, 'visibilityState', function () { return 'visible'; });
  getter(DP, 'hidden', function () { return false; });
  method(DP, 'hasFocus', function hasFocus() { return true; });
  function secureContext() {
    var l = global.location;
    if (l.protocol === 'https:' || l.protocol === 'wss:' || l.protocol === 'file:') return true;
    var host = l.hostname;
    return host === 'localhost' || /\.localhost$/.test(host) || host === '[::1]' || /^127\.\d+\.\d+\.\d+$/.test(host);
  }
  getter(global, 'origin', function () { return global.location.origin; }, function (v) {
    Object.defineProperty(global, 'origin', { value: v, writable: true, enumerable: true, configurable: true });
  });
  getter(global, 'isSecureContext', function () { return secureContext(); });
  getter(global, 'crossOriginIsolated', function () { return false; });
  method(global, 'reportError', function reportError(e) {
    requireArgs(arguments, 1, "'reportError' on 'Window'");
    reportError_(e);
  });

  // ---------------------------------------------------------------------------
  // focus() / blur() (HTML §6.6.4, focus update steps) and tabIndex
  // ---------------------------------------------------------------------------
  var HTML_NS = 'http://www.w3.org/1999/xhtml';
  function defaultFocusable(el) {
    if (el.namespaceURI !== HTML_NS) return false;
    var tag = el.localName;
    if (tag === 'input') return (el.getAttribute('type') || '').toLowerCase() !== 'hidden' && !el.hasAttribute('disabled');
    if (tag === 'select' || tag === 'textarea' || tag === 'button') return !el.hasAttribute('disabled');
    if (tag === 'a' || tag === 'area') return el.hasAttribute('href');
    if (tag === 'iframe' || tag === 'summary') return true;
    var editable = el.getAttribute('contenteditable');
    return editable !== null && editable.toLowerCase() !== 'false';
  }
  function tabIndexAttr(el) {
    var v = el.getAttribute('tabindex');
    if (v === null || !/^\s*[+-]?\d+/.test(v)) return null;
    return parseInt(v, 10);
  }
  function focusable(el) {
    return el.isConnected && (tabIndexAttr(el) !== null || defaultFocusable(el));
  }
  function focusEvent(target, type, related, bubbles) {
    fire(target, new global.FocusEvent(type, { bubbles: bubbles, composed: true, relatedTarget: related, view: global }));
  }
  function focusedNode() {
    var id = __axiom_focusedElement() | 0;
    return id >= 0 ? new ElementRef(id) : null;
  }
  function focus() {
    if (!focusable(this)) return;
    var previous = focusedNode();
    if (previous === this) return;
    if (previous) {
      __axiom_setFocus(-1);
      focusEvent(previous, 'blur', this, false);
      focusEvent(previous, 'focusout', this, true);
      if (focusedNode() !== null) return;
    }
    __axiom_setFocus(this.__id);
    focusEvent(this, 'focus', previous, false);
    focusEvent(this, 'focusin', previous, true);
  }
  function blur() {
    if (focusedNode() !== this) return;
    __axiom_setFocus(-1);
    focusEvent(this, 'blur', null, false);
    focusEvent(this, 'focusout', null, true);
  }
  ['HTMLElement', 'SVGElement', 'MathMLElement'].forEach(function (name) {
    var I = global[name];
    if (!I) return;
    method(I.prototype, 'focus', focus);
    method(I.prototype, 'blur', blur);
    if (!Object.getOwnPropertyDescriptor(I.prototype, 'tabIndex')) {
      getter(I.prototype, 'tabIndex', function () {
        var v = tabIndexAttr(this);
        return v !== null ? v : (defaultFocusable(this) ? 0 : -1);
      }, function (v) { this.setAttribute('tabindex', String(v | 0)); });
    }
  });

  // ---------------------------------------------------------------------------
  // MessageChannel / MessagePort and window.postMessage (HTML §9.4–9.5)
  // ---------------------------------------------------------------------------
  var PORTS = new WeakMap();
  function MessagePort() { illegal(); }
  iface('MessagePort', MessagePort, EventTarget);
  events.defineHandlers(MessagePort.prototype, ['onmessage', 'onmessageerror']);
  var onmessage = Object.getOwnPropertyDescriptor(MessagePort.prototype, 'onmessage');
  getter(MessagePort.prototype, 'onmessage', onmessage.get, function (v) {
    onmessage.set.call(this, v);
    this.start();
  });
  function newPort() {
    var p = Object.create(MessagePort.prototype);
    PORTS.set(p, { other: null, queue: [], started: false, closed: false, scheduled: false });
    return p;
  }
  function pumpPort(port) {
    var s = PORTS.get(port);
    if (!s.started || s.closed || s.scheduled || !s.queue.length) return;
    s.scheduled = true;
    setTimeout_(function () {
      s.scheduled = false;
      if (!s.started || s.closed || !s.queue.length) return;
      fire(port, new global.MessageEvent('message', { data: s.queue.shift() }));
      pumpPort(port);
    }, 0);
  }
  method(MessagePort.prototype, 'postMessage', function postMessage(message) {
    var s = stateOf(PORTS, this);
    requireArgs(arguments, 1, "'postMessage' on 'MessagePort'");
    var data = cloneValue(message, new Map());
    if (s.closed || !s.other) return;
    var target = PORTS.get(s.other);
    if (target.closed) return;
    target.queue.push(data);
    pumpPort(s.other);
  });
  method(MessagePort.prototype, 'start', function start() {
    var s = stateOf(PORTS, this);
    s.started = true;
    pumpPort(this);
  });
  method(MessagePort.prototype, 'close', function close() {
    var s = stateOf(PORTS, this);
    s.closed = true;
    if (s.other) PORTS.get(s.other).other = null;
    s.other = null;
  });
  var CHANNELS = new WeakMap();
  function MessageChannel() {
    requireNew(new.target, 'MessageChannel');
    var a = newPort(), b = newPort();
    PORTS.get(a).other = b;
    PORTS.get(b).other = a;
    CHANNELS.set(this, { port1: a, port2: b });
  }
  iface('MessageChannel', MessageChannel);
  getter(MessageChannel.prototype, 'port1', function () { return stateOf(CHANNELS, this).port1; });
  getter(MessageChannel.prototype, 'port2', function () { return stateOf(CHANNELS, this).port2; });

  method(global, 'postMessage', function postMessage(message, options) {
    requireArgs(arguments, 1, "'postMessage' on 'Window'");
    var target = '/';
    if (options !== null && typeof options === 'object') {
      if (options.targetOrigin !== undefined) target = String(options.targetOrigin);
    } else if (options !== undefined) {
      target = String(options);
    }
    var origin = global.location.origin;
    if (target === '/') {
      target = origin;
    } else if (target !== '*') {
      try {
        target = new global.URL(target).origin;
      } catch (e) {
        throw domError('SyntaxError', "Failed to execute 'postMessage' on 'Window': Invalid target origin '" +
          target + "' in a call to 'postMessage'.");
      }
    }
    var data = cloneValue(message, new Map());
    setTimeout_(function () {
      if (target !== '*' && target !== global.location.origin) return;
      fire(global, new global.MessageEvent('message', { data: data, origin: origin, source: global }));
    }, 0);
  });
  // Cross-realm iframe delivery arrives here from the engine after its target-origin
  // check. JSON is parsed in the receiving realm, which gives it a fresh value rather
  // than sharing a Boa heap object with the sender.
  global.__axiom_receiveFrameMessage = function (dataJSON, origin) {
    var data;
    try { data = JSON.parse(String(dataJSON)); }
    catch (_) { return; }
    fire(global, new global.MessageEvent('message', {
      data: data, origin: String(origin), source: null
    }));
  };

  // ---------------------------------------------------------------------------
  // NodeFilter, TreeWalker and NodeIterator (DOM §6)
  // ---------------------------------------------------------------------------
  function NodeFilter() { illegal(); }
  var FILTER_ACCEPT = 1, FILTER_REJECT = 2, FILTER_SKIP = 3;
  [['FILTER_ACCEPT', 1], ['FILTER_REJECT', 2], ['FILTER_SKIP', 3], ['SHOW_ALL', 0xFFFFFFFF],
    ['SHOW_ELEMENT', 0x1], ['SHOW_ATTRIBUTE', 0x2], ['SHOW_TEXT', 0x4], ['SHOW_CDATA_SECTION', 0x8],
    ['SHOW_ENTITY_REFERENCE', 0x10], ['SHOW_ENTITY', 0x20], ['SHOW_PROCESSING_INSTRUCTION', 0x40],
    ['SHOW_COMMENT', 0x80], ['SHOW_DOCUMENT', 0x100], ['SHOW_DOCUMENT_TYPE', 0x200],
    ['SHOW_DOCUMENT_FRAGMENT', 0x400], ['SHOW_NOTATION', 0x800]].forEach(function (c) {
    constant(NodeFilter, c[0], c[1]);
  });
  expose('NodeFilter', NodeFilter);

  var TRAVERSAL = new WeakMap();
  function traversalInit(obj, root, whatToShow, filter) {
    TRAVERSAL.set(obj, {
      root: root, whatToShow: whatToShow === undefined ? 0xFFFFFFFF : whatToShow >>> 0,
      filter: filter === undefined ? null : filter, active: false,
      current: root, reference: root, before: true
    });
  }
  function filterNode(s, node) {
    if (s.active) throw domError('InvalidStateError', 'The filter is already running.');
    if (!((1 << (node.nodeType - 1)) & s.whatToShow)) return FILTER_SKIP;
    if (s.filter === null) return FILTER_ACCEPT;
    s.active = true;
    try {
      var result = typeof s.filter === 'function'
        ? s.filter.call(undefined, node)
        : s.filter.acceptNode(node);
      return (result >>> 0) & 0xFFFF;
    } finally {
      s.active = false;
    }
  }
  function traversalGetters(proto) {
    getter(proto, 'root', function () { return stateOf(TRAVERSAL, this).root; });
    getter(proto, 'whatToShow', function () { return stateOf(TRAVERSAL, this).whatToShow; });
    getter(proto, 'filter', function () { return stateOf(TRAVERSAL, this).filter; });
  }
  function checkNode(n, what) {
    if (!(n instanceof global.Node)) {
      throw new TypeError('Failed to execute ' + what + ": parameter 1 is not of type 'Node'.");
    }
  }

  function TreeWalker() { illegal(); }
  iface('TreeWalker', TreeWalker);
  traversalGetters(TreeWalker.prototype);
  getter(TreeWalker.prototype, 'currentNode', function () {
    return stateOf(TRAVERSAL, this).current;
  }, function (n) {
    checkNode(n, "'currentNode' on 'TreeWalker'");
    stateOf(TRAVERSAL, this).current = n;
  });
  method(TreeWalker.prototype, 'parentNode', function parentNode() {
    var s = stateOf(TRAVERSAL, this);
    var node = s.current;
    while (node !== null && node !== s.root) {
      node = node.parentNode;
      if (node !== null && filterNode(s, node) === FILTER_ACCEPT) { s.current = node; return node; }
    }
    return null;
  });
  function traverseChildren(s, first) {
    var node = first ? s.current.firstChild : s.current.lastChild;
    while (node !== null) {
      var result = filterNode(s, node);
      if (result === FILTER_ACCEPT) { s.current = node; return node; }
      if (result === FILTER_SKIP) {
        var child = first ? node.firstChild : node.lastChild;
        if (child !== null) { node = child; continue; }
      }
      while (node !== null) {
        var sibling = first ? node.nextSibling : node.previousSibling;
        if (sibling !== null) { node = sibling; break; }
        var parent = node.parentNode;
        if (parent === null || parent === s.root || parent === s.current) return null;
        node = parent;
      }
    }
    return null;
  }
  function traverseSiblings(s, next) {
    var node = s.current;
    if (node === s.root) return null;
    for (;;) {
      var sibling = next ? node.nextSibling : node.previousSibling;
      while (sibling !== null) {
        node = sibling;
        var result = filterNode(s, node);
        if (result === FILTER_ACCEPT) { s.current = node; return node; }
        sibling = next ? node.firstChild : node.lastChild;
        if (result === FILTER_REJECT || sibling === null) sibling = next ? node.nextSibling : node.previousSibling;
      }
      node = node.parentNode;
      if (node === null || node === s.root) return null;
      if (filterNode(s, node) === FILTER_ACCEPT) return null;
    }
  }
  method(TreeWalker.prototype, 'firstChild', function firstChild() { return traverseChildren(stateOf(TRAVERSAL, this), true); });
  method(TreeWalker.prototype, 'lastChild', function lastChild() { return traverseChildren(stateOf(TRAVERSAL, this), false); });
  method(TreeWalker.prototype, 'nextSibling', function nextSibling() { return traverseSiblings(stateOf(TRAVERSAL, this), true); });
  method(TreeWalker.prototype, 'previousSibling', function previousSibling() { return traverseSiblings(stateOf(TRAVERSAL, this), false); });
  method(TreeWalker.prototype, 'previousNode', function previousNode() {
    var s = stateOf(TRAVERSAL, this);
    var node = s.current;
    while (node !== s.root) {
      var sibling = node.previousSibling;
      while (sibling !== null) {
        node = sibling;
        var result = filterNode(s, node);
        while (result !== FILTER_REJECT && node.lastChild !== null) {
          node = node.lastChild;
          result = filterNode(s, node);
        }
        if (result === FILTER_ACCEPT) { s.current = node; return node; }
        sibling = node.previousSibling;
      }
      if (node === s.root || node.parentNode === null) return null;
      node = node.parentNode;
      if (filterNode(s, node) === FILTER_ACCEPT) { s.current = node; return node; }
    }
    return null;
  });
  method(TreeWalker.prototype, 'nextNode', function nextNode() {
    var s = stateOf(TRAVERSAL, this);
    var node = s.current;
    var result = FILTER_ACCEPT;
    for (;;) {
      while (result !== FILTER_REJECT && node.firstChild !== null) {
        node = node.firstChild;
        result = filterNode(s, node);
        if (result === FILTER_ACCEPT) { s.current = node; return node; }
      }
      var sibling = null;
      for (var temporary = node; temporary !== null; temporary = temporary.parentNode) {
        if (temporary === s.root) return null;
        sibling = temporary.nextSibling;
        if (sibling !== null) { node = sibling; break; }
      }
      if (sibling === null) return null;
      result = filterNode(s, node);
      if (result === FILTER_ACCEPT) { s.current = node; return node; }
    }
  });

  function NodeIterator() { illegal(); }
  iface('NodeIterator', NodeIterator);
  traversalGetters(NodeIterator.prototype);
  getter(NodeIterator.prototype, 'referenceNode', function () { return stateOf(TRAVERSAL, this).reference; });
  getter(NodeIterator.prototype, 'pointerBeforeReferenceNode', function () { return stateOf(TRAVERSAL, this).before; });
  function following(node, root) {
    if (node.firstChild !== null) return node.firstChild;
    for (; node !== null && node !== root; node = node.parentNode) {
      if (node.nextSibling !== null) return node.nextSibling;
    }
    return null;
  }
  function preceding(node, root) {
    if (node === root) return null;
    var sibling = node.previousSibling;
    if (sibling === null) return node.parentNode;
    while (sibling.lastChild !== null) sibling = sibling.lastChild;
    return sibling;
  }
  function iterate(s, next) {
    var node = s.reference;
    var before = s.before;
    for (;;) {
      if (next) {
        if (!before) { node = following(node, s.root); if (node === null) return null; } else before = false;
      } else if (before) {
        node = preceding(node, s.root);
        if (node === null) return null;
      } else {
        before = true;
      }
      if (filterNode(s, node) === FILTER_ACCEPT) break;
    }
    s.reference = node;
    s.before = before;
    return node;
  }
  method(NodeIterator.prototype, 'nextNode', function nextNode() { return iterate(stateOf(TRAVERSAL, this), true); });
  method(NodeIterator.prototype, 'previousNode', function previousNode() { return iterate(stateOf(TRAVERSAL, this), false); });
  method(NodeIterator.prototype, 'detach', function detach() {});

  method(DP, 'createTreeWalker', function createTreeWalker(root, whatToShow, filter) {
    requireArgs(arguments, 1, "'createTreeWalker' on 'Document'");
    checkNode(root, "'createTreeWalker' on 'Document'");
    var w = Object.create(TreeWalker.prototype);
    traversalInit(w, root, whatToShow, filter);
    return w;
  });
  method(DP, 'createNodeIterator', function createNodeIterator(root, whatToShow, filter) {
    requireArgs(arguments, 1, "'createNodeIterator' on 'Document'");
    checkNode(root, "'createNodeIterator' on 'Document'");
    var it = Object.create(NodeIterator.prototype);
    traversalInit(it, root, whatToShow, filter);
    return it;
  });

  // ---------------------------------------------------------------------------
  // XMLHttpRequest (XHR Living Standard), asynchronous requests through fetch()
  // ---------------------------------------------------------------------------
  var XHR = new WeakMap();
  var UNSENT = 0, OPENED = 1, HEADERS_RECEIVED = 2, LOADING = 3, DONE = 4;
  var XHR_HANDLERS = ['onloadstart', 'onprogress', 'onabort', 'onerror', 'onload', 'ontimeout', 'onloadend'];
  var TOKEN = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;
  var FORBIDDEN_METHODS = ['CONNECT', 'TRACE', 'TRACK'];
  var NORMALIZED_METHODS = ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT'];

  function XMLHttpRequestEventTarget() { illegal(); }
  iface('XMLHttpRequestEventTarget', XMLHttpRequestEventTarget, EventTarget);
  events.defineHandlers(XMLHttpRequestEventTarget.prototype, XHR_HANDLERS);
  function XMLHttpRequestUpload() { illegal(); }
  iface('XMLHttpRequestUpload', XMLHttpRequestUpload, XMLHttpRequestEventTarget);

  function XMLHttpRequest() {
    requireNew(new.target, 'XMLHttpRequest');
    XHR.set(this, {
      state: UNSENT, method: 'GET', url: '', async: true, headers: [], send: false,
      timeout: 0, credentials: false, responseType: '', mime: null,
      upload: Object.create(XMLHttpRequestUpload.prototype), uploadDone: true,
      controller: null, timer: null, error: false,
      status: 0, statusText: '', responseURL: '', responseHeaders: [], chunks: [], loaded: 0,
      response: undefined
    });
  }
  iface('XMLHttpRequest', XMLHttpRequest, XMLHttpRequestEventTarget);
  events.defineHandlers(XMLHttpRequest.prototype, ['onreadystatechange']);
  [['UNSENT', UNSENT], ['OPENED', OPENED], ['HEADERS_RECEIVED', HEADERS_RECEIVED], ['LOADING', LOADING],
    ['DONE', DONE]].forEach(function (c) {
    constant(XMLHttpRequest, c[0], c[1]);
    constant(XMLHttpRequest.prototype, c[0], c[1]);
  });
  var XP = XMLHttpRequest.prototype;
  function xhr(obj) { return stateOf(XHR, obj); }

  function progress(target, type, loaded, total) {
    fire(target, new global.ProgressEvent(type, { lengthComputable: total > 0, loaded: loaded, total: total }));
  }
  function readyStateChange(x, s, state) {
    s.state = state;
    fire(x, new global.Event('readystatechange'));
  }
  function stopFetch(s) {
    var c = s.controller;
    s.controller = null;
    if (s.timer !== null) { clearTimeout_(s.timer); s.timer = null; }
    if (c) c.abort();
  }
  function resetResponse(s) {
    s.status = 0;
    s.statusText = '';
    s.responseURL = '';
    s.responseHeaders = [];
    s.chunks = [];
    s.loaded = 0;
    s.response = undefined;
    s.responseXML = undefined;
    s.error = false;
  }
  // "Request error steps": the request is done with a network error of kind `type`.
  function requestError(x, s, type) {
    s.send = false;
    resetResponse(s);
    s.error = true;
    readyStateChange(x, s, DONE);
    if (!s.uploadDone) {
      s.uploadDone = true;
      progress(s.upload, type, 0, 0);
      progress(s.upload, 'loadend', 0, 0);
    }
    progress(x, type, 0, 0);
    progress(x, 'loadend', 0, 0);
  }

  method(XP, 'open', function open(m, url, async) {
    var s = xhr(this);
    requireArgs(arguments, 2, "'open' on 'XMLHttpRequest'");
    m = String(m);
    if (!TOKEN.test(m)) throw domError('SyntaxError', "Failed to execute 'open' on 'XMLHttpRequest': '" + m + "' is not a valid HTTP method.");
    var upper = m.toUpperCase();
    if (FORBIDDEN_METHODS.indexOf(upper) >= 0) throw domError('SecurityError', "Failed to execute 'open' on 'XMLHttpRequest': '" + m + "' HTTP method is unsupported.");
    if (NORMALIZED_METHODS.indexOf(upper) >= 0) m = upper;
    var resolved = __axiom_resolveUrl(String(url));
    if (resolved === null) throw domError('SyntaxError', "Failed to execute 'open' on 'XMLHttpRequest': Invalid URL");
    var isAsync = arguments.length < 3 || !!async;
    if (!isAsync && (s.timeout !== 0 || s.responseType !== '')) {
      throw domError('InvalidAccessError', "Failed to execute 'open' on 'XMLHttpRequest': Synchronous requests from a document must not set a timeout or a response type.");
    }
    stopFetch(s);
    s.method = m;
    s.url = String(resolved);
    s.async = isAsync;
    s.headers = [];
    s.send = false;
    resetResponse(s);
    if (s.state !== OPENED) readyStateChange(this, s, OPENED);
  });
  method(XP, 'setRequestHeader', function setRequestHeader(name, value) {
    var s = xhr(this);
    requireArgs(arguments, 2, "'setRequestHeader' on 'XMLHttpRequest'");
    if (s.state !== OPENED || s.send) throw domError('InvalidStateError', "Failed to execute 'setRequestHeader' on 'XMLHttpRequest': The object's state must be OPENED.");
    name = String(name);
    value = String(value).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, '');
    if (!TOKEN.test(name)) throw domError('SyntaxError', "Failed to execute 'setRequestHeader' on 'XMLHttpRequest': '" + name + "' is not a valid HTTP header field name.");
    if (/[\0\r\n]/.test(value)) throw domError('SyntaxError', "Failed to execute 'setRequestHeader' on 'XMLHttpRequest': '" + value + "' is not a valid HTTP header field value.");
    var lower = name.toLowerCase();
    for (var i = 0; i < s.headers.length; i++) {
      if (s.headers[i][0].toLowerCase() === lower) { s.headers[i][1] += ', ' + value; return; }
    }
    s.headers.push([name, value]);
  });
  getter(XP, 'readyState', function () { return xhr(this).state; });
  getter(XP, 'timeout', function () { return xhr(this).timeout; }, function (v) {
    var s = xhr(this);
    if (!s.async && s.state === OPENED) throw domError('InvalidAccessError', "Failed to set the 'timeout' property on 'XMLHttpRequest': Timeouts cannot be set for synchronous requests made from a document.");
    s.timeout = Number(v) >>> 0;
  });
  getter(XP, 'withCredentials', function () { return xhr(this).credentials; }, function (v) {
    var s = xhr(this);
    if ((s.state !== UNSENT && s.state !== OPENED) || s.send) {
      throw domError('InvalidStateError', "Failed to set the 'withCredentials' property on 'XMLHttpRequest': The value may only be set if the object's state is UNSENT or OPENED.");
    }
    s.credentials = !!v;
  });
  getter(XP, 'upload', function () { return xhr(this).upload; });
  getter(XP, 'status', function () { return xhr(this).status; });
  getter(XP, 'statusText', function () { return xhr(this).statusText; });
  getter(XP, 'responseURL', function () { return xhr(this).responseURL; });
  getter(XP, 'responseType', function () { return xhr(this).responseType; }, function (v) {
    var s = xhr(this);
    v = String(v);
    if (['', 'arraybuffer', 'blob', 'document', 'json', 'text'].indexOf(v) < 0) return;
    if (s.state === LOADING || s.state === DONE) throw domError('InvalidStateError', "Failed to set the 'responseType' property on 'XMLHttpRequest': The response type cannot be set if the object's state is LOADING or DONE.");
    if (!s.async && s.state === OPENED) throw domError('InvalidAccessError', "Failed to set the 'responseType' property on 'XMLHttpRequest': The response type cannot be changed for synchronous requests made from a document.");
    s.responseType = v;
  });
  method(XP, 'overrideMimeType', function overrideMimeType(mime) {
    var s = xhr(this);
    requireArgs(arguments, 1, "'overrideMimeType' on 'XMLHttpRequest'");
    if (s.state === LOADING || s.state === DONE) throw domError('InvalidStateError', "Failed to execute 'overrideMimeType' on 'XMLHttpRequest': MimeType cannot be overridden when the state is LOADING or DONE.");
    s.mime = String(mime);
  });

  function bodyBytes(s) {
    var out = new Uint8Array(s.loaded);
    var offset = 0;
    s.chunks.forEach(function (c) { out.set(c, offset); offset += c.length; });
    return out;
  }
  function header(s, name) {
    var lower = name.toLowerCase();
    var values = s.responseHeaders.filter(function (h) { return h[0] === lower; }).map(function (h) { return h[1]; });
    return values.length ? values.join(', ') : null;
  }
  function charset(s) {
    var mime = s.mime !== null ? s.mime : header(s, 'content-type');
    var m = mime && /;\s*charset\s*=\s*"?([^";\s]+)/i.exec(mime);
    return m ? m[1] : 'utf-8';
  }
  function responseText(s) {
    if (s.error || s.state < LOADING) return '';
    var decoder;
    try { decoder = new global.TextDecoder(charset(s)); } catch (e) { decoder = new global.TextDecoder('utf-8'); }
    return decoder.decode(bodyBytes(s));
  }
  getter(XP, 'responseText', function () {
    var s = xhr(this);
    if (s.responseType !== '' && s.responseType !== 'text') {
      throw domError('InvalidStateError', "Failed to read the 'responseText' property from 'XMLHttpRequest': The value is only accessible if the object's 'responseType' is '' or 'text' (was '" + s.responseType + "').");
    }
    return responseText(s);
  });
  getter(XP, 'responseXML', function () {
    var s = xhr(this);
    if (s.responseType !== '' && s.responseType !== 'document') {
      throw domError('InvalidStateError', "Failed to read the 'responseXML' property from 'XMLHttpRequest': The value is only accessible if the object's 'responseType' is '' or 'document' (was '" + s.responseType + "').");
    }
    if (s.state !== DONE || s.error) return null;
    var mime = (s.mime !== null ? s.mime : (header(s, 'content-type') || ''))
      .split(';')[0].trim().toLowerCase();
    if (mime !== 'text/xml' && mime !== 'application/xml' &&
        mime !== 'application/xhtml+xml' && mime !== 'image/svg+xml') return null;
    if (s.responseXML !== undefined) return s.responseXML;
    try {
      s.responseXML = new global.DOMParser().parseFromString(responseText(s), mime);
    } catch (_) {
      s.responseXML = null;
    }
    return s.responseXML;
  });
  getter(XP, 'response', function () {
    var s = xhr(this);
    if (s.responseType === '' || s.responseType === 'text') return responseText(s);
    if (s.state !== DONE || s.error) return null;
    if (s.response !== undefined) return s.response;
    if (s.responseType === 'arraybuffer') {
      s.response = bodyBytes(s).buffer;
    } else if (s.responseType === 'blob') {
      var type = s.mime !== null ? s.mime : (header(s, 'content-type') || '');
      s.response = new global.Blob([bodyBytes(s)], { type: type });
    } else if (s.responseType === 'json') {
      try { s.response = JSON.parse(new global.TextDecoder('utf-8').decode(bodyBytes(s))); } catch (e) { s.response = null; }
    } else if (s.responseType === 'document') {
      var mime = (s.mime !== null ? s.mime : (header(s, 'content-type') || 'text/html'))
        .split(';')[0].trim().toLowerCase();
      if (mime === 'text/xml' || mime === 'application/xml' ||
          mime === 'application/xhtml+xml' || mime === 'image/svg+xml' || mime === 'text/html') {
        try { s.response = new global.DOMParser().parseFromString(responseText(s), mime); }
        catch (_) { s.response = null; }
      } else {
        s.response = null;
      }
    } else {
      s.response = null;
    }
    return s.response;
  });
  method(XP, 'getResponseHeader', function getResponseHeader(name) {
    var s = xhr(this);
    requireArgs(arguments, 1, "'getResponseHeader' on 'XMLHttpRequest'");
    if (s.state < HEADERS_RECEIVED || s.error) return null;
    return header(s, String(name));
  });
  method(XP, 'getAllResponseHeaders', function getAllResponseHeaders() {
    var s = xhr(this);
    if (s.state < HEADERS_RECEIVED || s.error) return '';
    var names = [];
    s.responseHeaders.forEach(function (h) { if (names.indexOf(h[0]) < 0) names.push(h[0]); });
    return names.sort().map(function (n) { return n + ': ' + header(s, n) + '\r\n'; }).join('');
  });

  method(XP, 'send', function send(body) {
    var x = this;
    var s = xhr(x);
    if (s.state !== OPENED) throw domError('InvalidStateError', "Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED.");
    if (s.send) throw domError('InvalidStateError', "Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED.");
    if (!s.async) throw domError('NetworkError', "Failed to execute 'send' on 'XMLHttpRequest': Synchronous requests are not supported.");
    if (s.method === 'GET' || s.method === 'HEAD' || body === undefined) body = null;
    s.uploadDone = body === null;
    s.send = true;
    progress(x, 'loadstart', 0, 0);
    if (!s.uploadDone) progress(s.upload, 'loadstart', 0, 0);
    if (s.state !== OPENED || !s.send) return;
    var controller = new global.AbortController();
    s.controller = controller;
    var init = {
      method: s.method, headers: s.headers.slice(), credentials: s.credentials ? 'include' : 'same-origin',
      signal: controller.signal
    };
    if (body !== null) init.body = body;
    if (s.timeout) {
      s.timer = setTimeout_(function () {
        if (s.controller !== controller) return;
        stopFetch(s);
        requestError(x, s, 'timeout');
      }, s.timeout);
    }
    var total = 0;
    fetch_(s.url, init).then(function (response) {
      if (s.controller !== controller) return;
      if (!s.uploadDone) {
        s.uploadDone = true;
        progress(s.upload, 'progress', 0, 0);
        progress(s.upload, 'load', 0, 0);
        progress(s.upload, 'loadend', 0, 0);
      }
      s.status = response.status;
      s.statusText = response.statusText;
      s.responseURL = response.url;
      response.headers.forEach(function (value, name) { s.responseHeaders.push([name, value]); });
      total = parseInt(header(s, 'content-length') || '0', 10) || 0;
      readyStateChange(x, s, HEADERS_RECEIVED);
      if (s.controller !== controller) return;
      if (!response.body) return finish();
      var reader = response.body.getReader();
      function pump() {
        return reader.read().then(function (r) {
          if (s.controller !== controller) { reader.cancel().catch(function () {}); return; }
          if (r.done) return finish();
          s.chunks.push(r.value);
          s.loaded += r.value.length;
          readyStateChange(x, s, LOADING);
          if (s.controller !== controller) return;
          progress(x, 'progress', s.loaded, total);
          return pump();
        });
      }
      return pump();
    }).catch(function () {
      if (s.controller !== controller) return;
      stopFetch(s);
      requestError(x, s, 'error');
    });
    function finish() {
      if (s.timer !== null) { clearTimeout_(s.timer); s.timer = null; }
      s.controller = null;
      s.send = false;
      readyStateChange(x, s, DONE);
      progress(x, 'load', s.loaded, total);
      progress(x, 'loadend', s.loaded, total);
    }
  });
  method(XP, 'abort', function abort() {
    var s = xhr(this);
    stopFetch(s);
    if ((s.state === OPENED && s.send) || s.state === HEADERS_RECEIVED || s.state === LOADING) {
      requestError(this, s, 'abort');
    }
    if (s.state === DONE) {
      s.state = UNSENT;
      resetResponse(s);
    }
  });

  // ---------------------------------------------------------------------------
  // History (HTML §7.2.2). Session history lives in the browsing context; this realm keeps
  // the structured clones of the states its document pushed, keyed by the id the entry
  // stores. pushState / replaceState change the URL at once; traversals are queued and
  // land as popstate (then hashchange) for entries of this document.
  // ---------------------------------------------------------------------------
  var STATES = new Map();
  var nextStateId = 1;
  function History() { illegal(); }
  var HP = History.prototype;
  Object.defineProperty(HP, Symbol.toStringTag, { value: 'History', configurable: true });
  var scrollRestoration = 'auto';
  function stateFor(id) { return id === null || !STATES.has(id) ? null : STATES.get(id); }
  getter(HP, 'length', function length() { return __axiom_historyLength() | 0; });
  getter(HP, 'state', function state() { return stateFor(__axiom_historyState()); });
  getter(HP, 'scrollRestoration', function () { return scrollRestoration; }, function (v) {
    v = String(v);
    if (v === 'auto' || v === 'manual') scrollRestoration = v;
  });
  function updateState(name, args, replace) {
    requireArgs(args, 2, "'" + name + "' on 'History'");
    var data = cloneValue(args[0], new Map());
    var url = args.length > 2 && args[2] !== undefined && args[2] !== null ? String(args[2]) : null;
    var previous = __axiom_historyState();
    var id = nextStateId++;
    STATES.set(id, data);
    var result = __axiom_historyPush(url, id, replace);
    if (result[0] === null) {
      STATES.delete(id);
      var message = result[1] === 'SecurityError'
        ? "Failed to execute '" + name + "' on 'History': A history state object with URL '" +
          url + "' cannot be created in a document with origin '" + global.location.origin + "'."
        : "Failed to execute '" + name + "' on 'History': Invalid URL '" + url + "'.";
      throw domError(result[1], message);
    }
    if (replace && previous !== null) STATES.delete(previous);
  }
  method(HP, 'pushState', function pushState(data, unused, url) { updateState('pushState', arguments, false); });
  method(HP, 'replaceState', function replaceState(data, unused, url) { updateState('replaceState', arguments, true); });
  method(HP, 'go', function go(delta) { __axiom_historyTraverse(delta === undefined ? 0 : (Number(delta) | 0)); });
  method(HP, 'back', function back() { __axiom_historyTraverse(-1); });
  method(HP, 'forward', function forward() { __axiom_historyTraverse(1); });
  expose('History', History);
  var history = Object.create(HP);
  getter(global, 'history', function () { return history; });

  global.__axiom_firePopstate = function (id, oldURL, newURL) {
    events.dispatchTrusted(global, new global.PopStateEvent('popstate', { state: stateFor(id) }));
    if (oldURL !== null) fireHashchange(oldURL, newURL);
  };
  function fireHashchange(oldURL, newURL) {
    setTimeout_(function () {
      events.dispatchTrusted(global, new global.HashChangeEvent('hashchange', { oldURL: oldURL, newURL: newURL }));
    }, 0);
  }
  global.__axiom_fireHashchange = fireHashchange;
})(this);
