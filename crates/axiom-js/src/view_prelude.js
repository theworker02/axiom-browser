// CSSOM View (geometry and scrolling) and the Image constructor: DOMRect /
// DOMRectReadOnly / DOMRectList, getBoundingClientRect / getClientRects, offset* /
// client* / scroll* metrics, window scrollX / scrollY / scrollTo / scrollBy,
// scrollIntoView and document.scrollingElement. Every geometry read flushes style and
// layout in the host.
// Not implemented: element scroll containers (scrollTop / scrollLeft of elements other
// than the scrolling element stay 0), smooth scrolling (behavior is ignored), and scroll
// events.
(function (global) {
  'use strict';
  var doc = global.document;

  function getter(obj, name, get, set) {
    Object.defineProperty(obj, name, { get: get, set: set, enumerable: true, configurable: true });
  }
  function method(obj, name, fn) {
    Object.defineProperty(obj, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  function expose(name, F) {
    Object.defineProperty(global, name, { value: F, writable: true, configurable: true });
  }
  function num(v) { v = Number(v); return v === v ? v : 0; }
  function unrestricted(v) { return v === undefined ? 0 : Number(v); }

  // --- DOMRectReadOnly / DOMRect -------------------------------------------------
  var RECT = new WeakMap();
  function rectOf(r) {
    var s = RECT.get(r);
    if (!s) throw new TypeError('Illegal invocation');
    return s;
  }
  function DOMRectReadOnly(x, y, width, height) {
    if (!(this instanceof DOMRectReadOnly)) throw new TypeError("Failed to construct 'DOMRectReadOnly': Please use the 'new' operator.");
    RECT.set(this, { x: unrestricted(x), y: unrestricted(y), width: unrestricted(width), height: unrestricted(height) });
  }
  var RRO = DOMRectReadOnly.prototype;
  ['x', 'y', 'width', 'height'].forEach(function (k) {
    getter(RRO, k, function () { return rectOf(this)[k]; });
  });
  getter(RRO, 'top', function () { var s = rectOf(this); return Math.min(s.y, s.y + s.height); });
  getter(RRO, 'bottom', function () { var s = rectOf(this); return Math.max(s.y, s.y + s.height); });
  getter(RRO, 'left', function () { var s = rectOf(this); return Math.min(s.x, s.x + s.width); });
  getter(RRO, 'right', function () { var s = rectOf(this); return Math.max(s.x, s.x + s.width); });
  method(RRO, 'toJSON', function toJSON() {
    var r = this;
    var out = {};
    ['x', 'y', 'width', 'height', 'top', 'right', 'bottom', 'left'].forEach(function (k) { out[k] = r[k]; });
    return out;
  });
  Object.defineProperty(RRO, Symbol.toStringTag, { value: 'DOMRectReadOnly', configurable: true });
  method(DOMRectReadOnly, 'fromRect', function fromRect(o) {
    o = o || {};
    return new DOMRectReadOnly(o.x, o.y, o.width, o.height);
  });
  expose('DOMRectReadOnly', DOMRectReadOnly);

  function DOMRect(x, y, width, height) {
    if (!(this instanceof DOMRect)) throw new TypeError("Failed to construct 'DOMRect': Please use the 'new' operator.");
    DOMRectReadOnly.call(this, x, y, width, height);
  }
  DOMRect.prototype = Object.create(RRO);
  Object.setPrototypeOf(DOMRect, DOMRectReadOnly);
  Object.defineProperty(DOMRect.prototype, 'constructor', { value: DOMRect, writable: true, configurable: true });
  Object.defineProperty(DOMRect.prototype, Symbol.toStringTag, { value: 'DOMRect', configurable: true });
  ['x', 'y', 'width', 'height'].forEach(function (k) {
    getter(DOMRect.prototype, k, function () { return rectOf(this)[k]; }, function (v) { rectOf(this)[k] = Number(v); });
  });
  method(DOMRect, 'fromRect', function fromRect(o) {
    o = o || {};
    return new DOMRect(o.x, o.y, o.width, o.height);
  });
  expose('DOMRect', DOMRect);

  function DOMRectList() { throw new TypeError('Illegal constructor'); }
  method(DOMRectList.prototype, 'item', function item(i) {
    i = Number(i) >>> 0;
    return i < this.length ? this[i] : null;
  });
  DOMRectList.prototype[Symbol.iterator] = Array.prototype[Symbol.iterator];
  Object.defineProperty(DOMRectList.prototype, Symbol.toStringTag, { value: 'DOMRectList', configurable: true });
  expose('DOMRectList', DOMRectList);
  function rectList(rects) {
    var list = Object.create(DOMRectList.prototype);
    rects.forEach(function (r, i) {
      Object.defineProperty(list, i, { value: r, enumerable: true });
    });
    Object.defineProperty(list, 'length', { value: rects.length });
    return list;
  }

  // --- Element geometry ------------------------------------------------------------
  // Geometry arrays: [offsetParent, offsetLeft, offsetTop, offsetWidth, offsetHeight,
  // clientLeft, clientTop, clientWidth, clientHeight, scrollWidth, scrollHeight,
  // then x, y, width, height per client rect].
  function geometry(el) {
    var id = el.__id;
    if (typeof id !== 'number') throw new TypeError('Illegal invocation');
    return __axiom_elementGeometry(id);
  }
  function clientRects(g) {
    var out = [];
    if (!g) return out;
    for (var i = 11; i + 3 < g.length; i += 4) out.push(new DOMRect(g[i], g[i + 1], g[i + 2], g[i + 3]));
    return out;
  }
  var EP = global.Element.prototype;
  var HP = global.HTMLElement.prototype;

  method(EP, 'getClientRects', function getClientRects() {
    return rectList(clientRects(geometry(this)));
  });
  method(EP, 'getBoundingClientRect', function getBoundingClientRect() {
    var rects = clientRects(geometry(this));
    if (!rects.length) return new DOMRect(0, 0, 0, 0);
    var nonEmpty = rects.filter(function (r) { return r.width !== 0 || r.height !== 0; });
    if (nonEmpty.length) rects = nonEmpty;
    var left = Infinity, top = Infinity, right = -Infinity, bottom = -Infinity;
    rects.forEach(function (r) {
      left = Math.min(left, r.left);
      top = Math.min(top, r.top);
      right = Math.max(right, r.right);
      bottom = Math.max(bottom, r.bottom);
    });
    return new DOMRect(left, top, right - left, bottom - top);
  });

  function metric(index, round) {
    return function () {
      var g = geometry(this);
      if (!g) return 0;
      return round ? Math.round(g[index]) : g[index];
    };
  }
  getter(HP, 'offsetParent', function () {
    var g = geometry(this);
    return g && g[0] >= 0 ? new ElementRef(g[0]) : null;
  });
  getter(HP, 'offsetLeft', metric(1, true));
  getter(HP, 'offsetTop', metric(2, true));
  getter(HP, 'offsetWidth', metric(3, true));
  getter(HP, 'offsetHeight', metric(4, true));
  getter(EP, 'clientLeft', metric(5, true));
  getter(EP, 'clientTop', metric(6, true));
  getter(EP, 'clientWidth', metric(7, true));
  getter(EP, 'clientHeight', metric(8, true));
  getter(EP, 'scrollWidth', metric(9, true));
  getter(EP, 'scrollHeight', metric(10, true));

  // --- Viewport scrolling ------------------------------------------------------------
  function scrollX() { return __axiom_scrollPosition()[0]; }
  function scrollY() { return __axiom_scrollPosition()[1]; }
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
  replaceable('scrollX', scrollX);
  replaceable('scrollY', scrollY);
  replaceable('pageXOffset', scrollX);
  replaceable('pageYOffset', scrollY);

  // (x, y) or a ScrollToOptions dictionary; missing members keep the current position.
  function target(args, relative) {
    var x = relative ? 0 : scrollX();
    var y = relative ? 0 : scrollY();
    if (args.length >= 2) return [num(args[0]), num(args[1])];
    var o = args[0];
    if (o !== undefined && o !== null && typeof o === 'object') {
      if (o.left !== undefined) x = num(o.left);
      if (o.top !== undefined) y = num(o.top);
    }
    return [x, y];
  }
  function scrollWindowTo(args) {
    var t = target(args, false);
    __axiom_scrollTo(t[0], t[1]);
  }
  function scrollWindowBy(args) {
    var t = target(args, true);
    __axiom_scrollTo(scrollX() + t[0], scrollY() + t[1]);
  }
  method(global, 'scrollTo', function scrollTo() { scrollWindowTo(arguments); });
  method(global, 'scroll', function scroll() { scrollWindowTo(arguments); });
  method(global, 'scrollBy', function scrollBy() { scrollWindowBy(arguments); });

  getter(doc, 'scrollingElement', function () { return doc.documentElement; });
  function isScrollingElement(el) {
    var root = doc.documentElement;
    return root !== null && el.__id === root.__id;
  }
  getter(EP, 'scrollTop', function () {
    return isScrollingElement(this) ? scrollY() : 0;
  }, function (v) {
    if (isScrollingElement(this)) __axiom_scrollTo(scrollX(), num(v));
  });
  getter(EP, 'scrollLeft', function () {
    return isScrollingElement(this) ? scrollX() : 0;
  }, function (v) {
    if (isScrollingElement(this)) __axiom_scrollTo(num(v), scrollY());
  });
  method(EP, 'scrollTo', function scrollTo() { if (isScrollingElement(this)) scrollWindowTo(arguments); });
  method(EP, 'scroll', function scroll() { if (isScrollingElement(this)) scrollWindowTo(arguments); });
  method(EP, 'scrollBy', function scrollBy() { if (isScrollingElement(this)) scrollWindowBy(arguments); });

  // scrollIntoView: aligns the element's border box in the viewport's block axis.
  method(EP, 'scrollIntoView', function scrollIntoView(arg) {
    var block = 'start';
    if (arg === false) block = 'end';
    else if (arg !== null && typeof arg === 'object' && arg.block !== undefined) block = String(arg.block);
    var r = this.getBoundingClientRect();
    var vh = global.innerHeight;
    var y = scrollY();
    if (block === 'end') y += r.bottom - vh;
    else if (block === 'center') y += r.top + r.height / 2 - vh / 2;
    else if (block === 'nearest') {
      if (r.top < 0) y += r.top;
      else if (r.bottom > vh) y += Math.min(r.top, r.bottom - vh);
    } else y += r.top;
    __axiom_scrollTo(scrollX(), y);
  });

  // --- HTMLImageElement ----------------------------------------------------------------
  var IMG = global.HTMLImageElement.prototype;
  function natural(el) { return __axiom_imageNaturalSize(el.__id); }
  getter(IMG, 'naturalWidth', function () { var n = natural(this); return n ? n[0] : 0; });
  getter(IMG, 'naturalHeight', function () { var n = natural(this); return n ? n[1] : 0; });
  getter(IMG, 'complete', function () {
    var src = __axiom_getAttr(this.__id, 'src');
    return src === null || String(src) === '' || natural(this) !== null;
  });
  // width / height: the rendered content box when laid out, else the attribute, else the
  // natural size.
  function dimension(name, index) {
    getter(IMG, name, function () {
      var g = geometry(this);
      if (g && g.length > 11) return Math.round(g[7 + index]);
      var v = parseInt(__axiom_getAttr(this.__id, name), 10);
      if (v >= 0) return v;
      var n = natural(this);
      return n ? n[index] : 0;
    }, function (v) {
      __axiom_setAttr(this.__id, name, String(Number(v) >>> 0));
    });
  }
  dimension('width', 0);
  dimension('height', 1);
  method(IMG, 'decode', function decode() {
    var img = this;
    if (img.complete) return Promise.resolve();
    return new Promise(function (resolve, reject) {
      img.addEventListener('load', function () { resolve(); }, { once: true });
      img.addEventListener('error', function () {
        reject(new DOMException('The source image cannot be decoded.', 'EncodingError'));
      }, { once: true });
    });
  });

  function Image(width, height) {
    if (!(this instanceof Image)) throw new TypeError("Failed to construct 'Image': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
    var img = doc.createElement('img');
    if (width !== undefined) img.setAttribute('width', String(Number(width) >>> 0));
    if (height !== undefined) img.setAttribute('height', String(Number(height) >>> 0));
    return img;
  }
  Image.prototype = IMG;
  expose('Image', Image);

  // screen: Axiom reports the viewport as the screen (no multi-monitor or window
  // placement information is exposed).
  function Screen() { throw new TypeError('Illegal constructor'); }
  var SP = Screen.prototype;
  getter(SP, 'width', function () { return global.innerWidth; });
  getter(SP, 'height', function () { return global.innerHeight; });
  getter(SP, 'availWidth', function () { return global.innerWidth; });
  getter(SP, 'availHeight', function () { return global.innerHeight; });
  getter(SP, 'availLeft', function () { return 0; });
  getter(SP, 'availTop', function () { return 0; });
  getter(SP, 'colorDepth', function () { return 24; });
  getter(SP, 'pixelDepth', function () { return 24; });
  Object.defineProperty(SP, Symbol.toStringTag, { value: 'Screen', configurable: true });
  expose('Screen', Screen);
  var screen = Object.create(SP);
  replaceable('screen', function () { return screen; });
})(this);
