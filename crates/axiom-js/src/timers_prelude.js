// Timers (HTML §8.6), queueMicrotask, animation frames (HTML §8.10), navigator
// (HTML §8.9.1) and matchMedia (CSSOM View §4.2).
// Timer and frame scheduling belongs to the host: script records handlers here and asks
// the host to run them. navigator describes Axiom itself; it does not pretend to be
// another browser.
(function (global) {
  'use strict';
  var hooks = global.__axiom_events;
  var report = global.__axiom_reportError;

  function describe(v) {
    if (v instanceof Error) return v.name + ': ' + v.message;
    try { return typeof v === 'string' ? v : String(v); } catch (_) { return '<value>'; }
  }
  // Runs `fn`, reporting an uncaught exception; returns its description or undefined.
  function guarded(fn, thisArg, args) {
    try {
      fn.apply(thisArg, args);
    } catch (e) {
      report(e);
      return 'Uncaught ' + describe(e);
    }
    return undefined;
  }
  function method(obj, name, fn) {
    Object.defineProperty(obj, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  function getter(obj, name, get) {
    Object.defineProperty(obj, name, { get: get, enumerable: true, configurable: true });
  }
  function illegalInterface(name, Parent) {
    var F = function () { throw new TypeError('Illegal constructor'); };
    Object.defineProperty(F, 'name', { value: name, configurable: true });
    F.prototype = Object.create(Parent ? Parent.prototype : Object.prototype);
    Object.defineProperty(F.prototype, 'constructor', { value: F, writable: true, configurable: true });
    Object.defineProperty(F.prototype, Symbol.toStringTag, { value: name, configurable: true });
    if (Parent) Object.setPrototypeOf(F, Parent);
    Object.defineProperty(global, name, { value: F, writable: true, configurable: true });
    return F;
  }

  // ---------------------------------------------------------------------------
  // setTimeout / setInterval: one id space; the host owns the clock.
  // ---------------------------------------------------------------------------
  var timers = new Map(); // id -> { handler, args, repeat, nesting }
  var nextTimer = 1;
  var runningNesting = 0; // timer nesting level of the running timer task (0 outside)

  function initializeTimer(handler, timeout, args, repeat) {
    if (typeof handler !== 'function') {
      var code = String(handler);
      handler = function () { (0, eval)(code); };
      args = [];
    }
    timeout = timeout | 0;
    if (timeout < 0) timeout = 0;
    var nesting = runningNesting;
    if (nesting > 5 && timeout < 4) timeout = 4;
    if (repeat && timeout < 4) timeout = 4;
    var id = nextTimer++;
    timers.set(id, { handler: handler, args: args, repeat: repeat, nesting: nesting + 1 });
    __axiom_scheduleTimer(id, timeout, repeat);
    return id;
  }
  function clearTimer(id) {
    id = id | 0;
    if (id > 0 && timers.delete(id)) __axiom_clearTimer(id);
  }
  method(global, 'setTimeout', function setTimeout(handler, timeout) {
    return initializeTimer(handler, timeout, Array.prototype.slice.call(arguments, 2), false);
  });
  method(global, 'setInterval', function setInterval(handler, timeout) {
    return initializeTimer(handler, timeout, Array.prototype.slice.call(arguments, 2), true);
  });
  method(global, 'clearTimeout', function clearTimeout(id) { clearTimer(id); });
  method(global, 'clearInterval', function clearInterval(id) { clearTimer(id); });

  global.__axiom_fireTimeout = function (id) {
    var t = timers.get(id);
    if (!t) return undefined;
    if (!t.repeat) timers.delete(id);
    var outer = runningNesting;
    runningNesting = t.nesting;
    if (t.repeat) t.nesting++;
    try {
      return guarded(t.handler, global, t.args);
    } finally {
      runningNesting = outer;
    }
  };

  method(global, 'queueMicrotask', function queueMicrotask(callback) {
    if (typeof callback !== 'function') {
      throw new TypeError("Failed to execute 'queueMicrotask': parameter 1 is not of type 'Function'.");
    }
    Promise.resolve().then(function () { guarded(callback, undefined, []); });
  });

  // ---------------------------------------------------------------------------
  // Animation frames: callbacks requested before a frame run together, in order,
  // with that frame's timestamp.
  // ---------------------------------------------------------------------------
  var frameCallbacks = new Map();
  var nextFrame = 0;
  method(global, 'requestAnimationFrame', function requestAnimationFrame(callback) {
    if (typeof callback !== 'function') {
      throw new TypeError("Failed to execute 'requestAnimationFrame': parameter 1 is not of type 'Function'.");
    }
    var id = ++nextFrame;
    frameCallbacks.set(id, callback);
    __axiom_requestFrame();
    return id;
  });
  method(global, 'cancelAnimationFrame', function cancelAnimationFrame(id) {
    frameCallbacks.delete(id | 0);
  });
  // The viewport scrolled (script or user): `scroll` then `scrollend` fire at the document
  // in the next frame's scroll steps, before its animation frame callbacks. Scrolls are
  // instant, so every scroll ends in the frame it is reported.
  var scrollPending = false;
  global.__axiom_notifyScroll = function () {
    scrollPending = true;
    __axiom_requestFrame();
  };
  global.__axiom_runAnimationFrames = function () {
    var now = global.performance.now();
    var errors = [];
    if (scrollPending) {
      scrollPending = false;
      var events = global.__axiom_events;
      events.fire(global.document, 'scroll', { bubbles: true });
      events.fire(global.document, 'scrollend', { bubbles: true });
    }
    Array.from(frameCallbacks.keys()).forEach(function (id) {
      var callback = frameCallbacks.get(id);
      if (!callback) return;
      frameCallbacks.delete(id);
      var err = guarded(callback, global, [now]);
      if (err !== undefined) errors.push(err);
    });
    if (typeof global.__axiom_updateObservations === 'function') {
      errors = errors.concat(global.__axiom_updateObservations());
    }
    return errors.join('\n');
  };

  // ---------------------------------------------------------------------------
  // navigator
  // ---------------------------------------------------------------------------
  var Navigator = illegalInterface('Navigator', null);
  var NP = Navigator.prototype;
  var languages = null;
  function currentLanguages() {
    var now = Array.prototype.slice.call(__axiom_languages()).map(String);
    if (!languages || languages.join(',') !== now.join(',')) languages = Object.freeze(now);
    return languages;
  }
  function userAgent() { return String(__axiom_userAgent()); }
  getter(NP, 'userAgent', userAgent);
  getter(NP, 'appCodeName', function () { return 'Mozilla'; });
  getter(NP, 'appName', function () { return 'Netscape'; });
  getter(NP, 'appVersion', function () {
    var ua = userAgent();
    var slash = ua.indexOf('/');
    return slash >= 0 ? ua.substring(slash + 1) : ua;
  });
  getter(NP, 'platform', function () { return String(__axiom_platformInfo()[0]); });
  getter(NP, 'product', function () { return 'Gecko'; });
  getter(NP, 'productSub', function () { return '20030107'; });
  getter(NP, 'vendor', function () { return ''; });
  getter(NP, 'vendorSub', function () { return ''; });
  getter(NP, 'language', function () { return currentLanguages()[0] || 'en-US'; });
  getter(NP, 'languages', currentLanguages);
  getter(NP, 'onLine', function () { return true; });
  getter(NP, 'cookieEnabled', function () { return !!__axiom_cookieEnabled(); });
  getter(NP, 'hardwareConcurrency', function () { return __axiom_platformInfo()[1] | 0; });
  getter(NP, 'maxTouchPoints', function () { return 0; });
  getter(NP, 'pdfViewerEnabled', function () { return false; });
  getter(NP, 'webdriver', function () { return false; });
  method(NP, 'javaEnabled', function javaEnabled() { return false; });
  var navigator = Object.create(NP);
  getter(global, 'navigator', function () { return navigator; });
  getter(global, 'clientInformation', function () { return navigator; });

  // ---------------------------------------------------------------------------
  // matchMedia: lists with change listeners are re-evaluated when the viewport changes.
  // ---------------------------------------------------------------------------
  var MediaQueryList = illegalInterface('MediaQueryList', global.EventTarget);
  var MQLP = MediaQueryList.prototype;
  var MQL = new WeakMap(); // list -> { media, matches }
  var live = new Set();
  function mql(obj) {
    var s = MQL.get(obj);
    if (!s) throw new TypeError('Illegal invocation');
    return s;
  }
  getter(MQLP, 'media', function () { return mql(this).media; });
  getter(MQLP, 'matches', function () { return !!__axiom_matchMedia(mql(this).media); });
  var baseAdd = global.EventTarget.prototype.addEventListener;
  var baseRemove = global.EventTarget.prototype.removeEventListener;
  method(MQLP, 'addEventListener', function addEventListener(type) {
    mql(this);
    live.add(this);
    return baseAdd.apply(this, arguments);
  });
  method(MQLP, 'addListener', function addListener(callback) {
    if (callback === null || callback === undefined) return;
    mql(this);
    live.add(this);
    baseAdd.call(this, 'change', callback);
  });
  method(MQLP, 'removeListener', function removeListener(callback) {
    if (callback === null || callback === undefined) return;
    baseRemove.call(this, 'change', callback);
  });
  hooks.defineHandlers(MQLP, ['onchange']);
  var onchange = Object.getOwnPropertyDescriptor(MQLP, 'onchange');
  Object.defineProperty(MQLP, 'onchange', {
    get: onchange.get,
    set: function (v) {
      mql(this);
      live.add(this);
      onchange.set.call(this, v);
    },
    enumerable: true,
    configurable: true
  });
  method(global, 'matchMedia', function matchMedia(query) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to execute 'matchMedia': 1 argument required, but only 0 present.");
    }
    var media = String(query).replace(/[\t\n\f\r ]+/g, ' ').replace(/^ | $/g, '');
    var list = Object.create(MQLP);
    MQL.set(list, { media: media, matches: !!__axiom_matchMedia(media) });
    return list;
  });
  global.__axiom_evaluateMediaQueries = function () {
    live.forEach(function (list) {
      var s = MQL.get(list);
      var now = !!__axiom_matchMedia(s.media);
      if (now === s.matches) return;
      s.matches = now;
      var ev = new global.MediaQueryListEvent('change', { media: s.media, matches: now });
      hooks.dispatchTrusted(list, ev);
    });
  };
})(this);
