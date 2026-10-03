// IntersectionObserver, ResizeObserver and requestIdleCallback. Observations update in
// the rendering steps after animation frame callbacks; a frame runs when a target is
// observed, when the viewport scrolls and when layout changes while observers exist.
// Not implemented: clipping by ancestor overflow (intersections test the root only),
// scrollMargin, trackVisibility / delay, and ResizeObserver depth limits (a callback
// that keeps resizing its target is observed again in the next frame).
(function (global) {
  'use strict';

  function method(obj, name, fn) {
    Object.defineProperty(obj, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  function getter(obj, name, get) {
    Object.defineProperty(obj, name, { get: get, enumerable: true, configurable: true });
  }
  function expose(name, F) {
    Object.defineProperty(global, name, { value: F, writable: true, configurable: true });
  }
  function report(e) { if (typeof __axiom_reportError === 'function') __axiom_reportError(e); }
  function requireElement(t, where) {
    if (!(t instanceof global.Element)) {
      throw new TypeError("Failed to execute '" + where + "': parameter 1 is not of type 'Element'.");
    }
  }
  var STATE = typeof Symbol === 'function' ? Symbol('state') : '__state';
  var observers = [];
  function activate(o) {
    if (observers.indexOf(o) < 0) observers.push(o);
    __axiom_observersActive();
    __axiom_requestFrame();
  }
  function deactivate(o) {
    var i = observers.indexOf(o);
    if (i >= 0) observers.splice(i, 1);
  }

  // --- IntersectionObserver ------------------------------------------------------------
  function parseMargin(text) {
    var parts = String(text === undefined ? '0px' : text).trim().split(/\s+/);
    if (parts.length > 4 || parts[0] === '') throw new DOMException("Failed to construct 'IntersectionObserver': rootMargin must be specified in pixels or percent.", 'SyntaxError');
    var values = parts.map(function (p) {
      var m = /^(-?\d*\.?\d+)(px|%)$/.exec(p) || (p === '0' ? ['0', '0', 'px'] : null);
      if (!m) throw new DOMException("Failed to construct 'IntersectionObserver': rootMargin must be specified in pixels or percent.", 'SyntaxError');
      return { value: Number(m[1]), unit: m[2] };
    });
    var a = values[0], b = values[1] || a, c = values[2] || a, d = values[3] || b;
    return [a, b, c, d];
  }
  function marginText(m) { return m.map(function (v) { return v.value + v.unit; }).join(' '); }
  function resolve(v, base) { return v.unit === '%' ? base * v.value / 100 : v.value; }

  function IntersectionObserverEntry(init) {
    this[STATE] = init;
  }
  ['time', 'rootBounds', 'boundingClientRect', 'intersectionRect', 'isIntersecting',
    'intersectionRatio', 'target', 'isVisible'].forEach(function (k) {
    getter(IntersectionObserverEntry.prototype, k, function () { return this[STATE][k]; });
  });
  expose('IntersectionObserverEntry', IntersectionObserverEntry);

  function IntersectionObserver(callback, options) {
    if (!(this instanceof IntersectionObserver)) throw new TypeError("Failed to construct 'IntersectionObserver': Please use the 'new' operator.");
    if (typeof callback !== 'function') throw new TypeError("Failed to construct 'IntersectionObserver': The callback provided as parameter 1 is not a function.");
    options = options || {};
    var thresholds = options.threshold === undefined ? [0] : [].concat(options.threshold).map(Number);
    thresholds.forEach(function (t) {
      if (!(t >= 0 && t <= 1)) throw new RangeError("Failed to construct 'IntersectionObserver': Threshold values must be numbers between 0 and 1");
    });
    thresholds.sort(function (a, b) { return a - b; });
    if (!thresholds.length) thresholds = [0];
    var root = options.root === undefined ? null : options.root;
    if (root !== null && !(root instanceof global.Element) && root !== global.document) {
      throw new TypeError("Failed to construct 'IntersectionObserver': Failed to read the 'root' property: The provided value is not of type '(Document or Element)'.");
    }
    this[STATE] = {
      callback: callback, root: root, margin: parseMargin(options.rootMargin),
      thresholds: Object.freeze(thresholds), targets: [], queue: []
    };
  }
  var IOP = IntersectionObserver.prototype;
  getter(IOP, 'root', function () { return this[STATE].root; });
  getter(IOP, 'rootMargin', function () { return marginText(this[STATE].margin); });
  getter(IOP, 'thresholds', function () { return this[STATE].thresholds; });
  method(IOP, 'observe', function observe(target) {
    requireElement(target, 'observe');
    var st = this[STATE];
    for (var i = 0; i < st.targets.length; i++) if (st.targets[i].el === target) return;
    st.targets.push({ el: target, index: -1, intersecting: false });
    activate(this);
  });
  method(IOP, 'unobserve', function unobserve(target) {
    var st = this[STATE];
    st.targets = st.targets.filter(function (t) { return t.el !== target; });
    if (!st.targets.length) deactivate(this);
  });
  method(IOP, 'disconnect', function disconnect() {
    this[STATE].targets = [];
    this[STATE].queue = [];
    deactivate(this);
  });
  method(IOP, 'takeRecords', function takeRecords() {
    var q = this[STATE].queue;
    this[STATE].queue = [];
    return q;
  });
  expose('IntersectionObserver', IntersectionObserver);

  function rootRect(st) {
    var r;
    if (st.root === null || st.root === global.document) {
      r = { left: 0, top: 0, right: global.innerWidth, bottom: global.innerHeight };
    } else {
      var b = st.root.getBoundingClientRect();
      r = { left: b.left, top: b.top, right: b.right, bottom: b.bottom };
    }
    var w = r.right - r.left, h = r.bottom - r.top, m = st.margin;
    return {
      left: r.left - resolve(m[3], w), top: r.top - resolve(m[0], h),
      right: r.right + resolve(m[1], w), bottom: r.bottom + resolve(m[2], h)
    };
  }
  function updateIntersections(o, now) {
    var st = o[STATE];
    var root = rootRect(st);
    var rootInDoc = st.root === null || st.root === global.document || st.root.isConnected;
    st.targets.forEach(function (t) {
      var b = t.el.getBoundingClientRect();
      var left = Math.max(b.left, root.left), top = Math.max(b.top, root.top);
      var right = Math.min(b.right, root.right), bottom = Math.min(b.bottom, root.bottom);
      var contained = st.root === null || st.root === global.document || st.root.contains(t.el);
      var intersecting = rootInDoc && contained && t.el.isConnected && right >= left && bottom >= top &&
        (b.width > 0 || b.height > 0 || (b.left >= root.left && b.top >= root.top));
      var area = b.width * b.height;
      var inter = intersecting ? Math.max(0, right - left) * Math.max(0, bottom - top) : 0;
      var ratio = intersecting ? (area > 0 ? inter / area : 1) : 0;
      var index = 0;
      if (intersecting) st.thresholds.forEach(function (x) { if (ratio >= x) index++; });
      if (index === t.index && intersecting === t.intersecting) return;
      t.index = index;
      t.intersecting = intersecting;
      st.queue.push(new IntersectionObserverEntry({
        time: now,
        rootBounds: new DOMRectReadOnly(root.left, root.top, root.right - root.left, root.bottom - root.top),
        boundingClientRect: DOMRectReadOnly.fromRect(b),
        intersectionRect: intersecting ? new DOMRectReadOnly(left, top, right - left, bottom - top) : new DOMRectReadOnly(0, 0, 0, 0),
        isIntersecting: intersecting,
        intersectionRatio: ratio,
        target: t.el,
        isVisible: false
      }));
    });
  }

  // --- ResizeObserver ------------------------------------------------------------------
  function ResizeObserverSize(inline, block) {
    this[STATE] = { inlineSize: inline, blockSize: block };
  }
  getter(ResizeObserverSize.prototype, 'inlineSize', function () { return this[STATE].inlineSize; });
  getter(ResizeObserverSize.prototype, 'blockSize', function () { return this[STATE].blockSize; });
  expose('ResizeObserverSize', ResizeObserverSize);
  function ResizeObserverEntry(init) { this[STATE] = init; }
  ['target', 'contentRect', 'borderBoxSize', 'contentBoxSize', 'devicePixelContentBoxSize'].forEach(function (k) {
    getter(ResizeObserverEntry.prototype, k, function () { return this[STATE][k]; });
  });
  expose('ResizeObserverEntry', ResizeObserverEntry);

  function ResizeObserver(callback) {
    if (!(this instanceof ResizeObserver)) throw new TypeError("Failed to construct 'ResizeObserver': Please use the 'new' operator.");
    if (typeof callback !== 'function') throw new TypeError("Failed to construct 'ResizeObserver': The callback provided as parameter 1 is not a function.");
    this[STATE] = { callback: callback, targets: [] };
  }
  var ROP = ResizeObserver.prototype;
  method(ROP, 'observe', function observe(target, options) {
    requireElement(target, 'observe');
    var box = options && options.box !== undefined ? String(options.box) : 'content-box';
    var st = this[STATE];
    st.targets = st.targets.filter(function (t) { return t.el !== target; });
    st.targets.push({ el: target, box: box, last: null });
    activate(this);
  });
  method(ROP, 'unobserve', function unobserve(target) {
    var st = this[STATE];
    st.targets = st.targets.filter(function (t) { return t.el !== target; });
    if (!st.targets.length) deactivate(this);
  });
  method(ROP, 'disconnect', function disconnect() {
    this[STATE].targets = [];
    deactivate(this);
  });
  expose('ResizeObserver', ResizeObserver);

  function px(v) { var n = parseFloat(v); return n === n ? n : 0; }
  function boxes(el) {
    var cs = global.getComputedStyle(el);
    var borderW = el.getBoundingClientRect().width, borderH = el.getBoundingClientRect().height;
    var padL = px(cs.paddingLeft), padT = px(cs.paddingTop);
    var bl = px(cs.borderLeftWidth), br = px(cs.borderRightWidth), bt = px(cs.borderTopWidth), bb = px(cs.borderBottomWidth);
    var contentW = Math.max(0, borderW - bl - br - padL - px(cs.paddingRight));
    var contentH = Math.max(0, borderH - bt - bb - padT - px(cs.paddingBottom));
    if (cs.display === 'inline' || cs.display === 'none' || !el.isConnected) {
      borderW = borderH = contentW = contentH = 0;
    }
    return { borderW: borderW, borderH: borderH, contentW: contentW, contentH: contentH, padL: padL, padT: padT };
  }
  function updateSizes(o) {
    var st = o[STATE];
    var entries = [];
    st.targets.forEach(function (t) {
      var b = boxes(t.el);
      var size = t.box === 'border-box' ? [b.borderW, b.borderH] : [b.contentW, b.contentH];
      if (t.last && t.last[0] === size[0] && t.last[1] === size[1]) return;
      if (!t.last && size[0] === 0 && size[1] === 0 && !t.el.isConnected) return;
      t.last = size;
      var content = Object.freeze([new ResizeObserverSize(b.contentW, b.contentH)]);
      entries.push(new ResizeObserverEntry({
        target: t.el,
        contentRect: new DOMRectReadOnly(b.padL, b.padT, b.contentW, b.contentH),
        borderBoxSize: Object.freeze([new ResizeObserverSize(b.borderW, b.borderH)]),
        contentBoxSize: content,
        devicePixelContentBoxSize: content
      }));
    });
    return entries;
  }

  // Rendering steps: resize observations, then intersection observations.
  global.__axiom_updateObservations = function () {
    var now = global.performance.now();
    var errors = [];
    observers.slice().forEach(function (o) {
      var st = o[STATE];
      var entries;
      if (o instanceof ResizeObserver) {
        entries = updateSizes(o);
      } else {
        updateIntersections(o, now);
        entries = st.queue;
        st.queue = [];
      }
      if (!entries.length) return;
      try { st.callback.call(o, entries, o); } catch (e) { report(e); errors.push(String(e)); }
    });
    return errors;
  };

  // --- requestIdleCallback ------------------------------------------------------------
  // Idle periods are the event loop's idle time after a task; each gets a 50 ms budget.
  var idleIds = new Map();
  var nextIdle = 0;
  method(global, 'requestIdleCallback', function requestIdleCallback(callback, options) {
    if (typeof callback !== 'function') {
      throw new TypeError("Failed to execute 'requestIdleCallback' on 'Window': The callback provided as parameter 1 is not a function.");
    }
    var id = ++nextIdle;
    var timeout = options && options.timeout > 0 ? Number(options.timeout) : 0;
    var timer = global.setTimeout(function () {
      idleIds.delete(id);
      var start = global.performance.now();
      callback({
        didTimeout: false,
        timeRemaining: function () { return Math.max(0, 50 - (global.performance.now() - start)); }
      });
    }, timeout > 0 ? Math.min(timeout, 1) : 1);
    idleIds.set(id, timer);
    return id;
  });
  method(global, 'cancelIdleCallback', function cancelIdleCallback(id) {
    var timer = idleIds.get(id);
    if (timer !== undefined) {
      global.clearTimeout(timer);
      idleIds.delete(id);
    }
  });
})(this);
