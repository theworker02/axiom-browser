// DOM events (DOM Standard §2) and HTML event handlers (HTML §8.1.8): EventTarget with
// the spec listener model (capture, once, passive, signal, handleEvent), the dispatch
// algorithm (capture / target / bubble over "get the parent"), Event and its subclasses,
// window.event, event handler IDL and content attributes, "report an exception" as an
// ErrorEvent at the window, and the Window interface.
// Shadow trees retarget target / relatedTarget per path entry and composedPath() hides
// closed trees (hooks.shadow, from the DOM prelude). Activation behavior (links, checkboxes, labels) is not run for script-dispatched
// events; engine clicks run their default actions only when the event was not canceled.
// Node-specific parts ("get the parent", content attributes) are hooks the DOM prelude
// fills in.
(function (global) {
  'use strict';

  var EV = new WeakMap(); // event -> internal state
  var TARGETS = new WeakMap(); // event target -> { listeners, handlers }
  var hooks = {
    getParent: function () { return null; },
    shadow: null,
    passiveByDefault: function () { return false; },
    scan: function () {},
    added: function () {},
    addAbortAlgorithm: null
  };
  Object.defineProperty(global, '__axiom_events', { value: hooks });

  function now() { return __axiom_now(); }
  function describe(v) {
    if (v instanceof Error) return v.name + ': ' + v.message;
    try { return typeof v === 'string' ? v : String(v); } catch (_) { return '<value>'; }
  }
  function domError(name, message) { return new global.DOMException(message || name, name); }
  function isObject(v) { return v !== null && (typeof v === 'object' || typeof v === 'function'); }
  function requireArgs(args, n, where) {
    if (args.length < n) {
      throw new TypeError("Failed to execute '" + where + "': " + n + ' argument(s) required, but only ' +
        args.length + ' present.');
    }
  }
  function getter(obj, name, get, set) {
    Object.defineProperty(obj, name, { get: get, set: set, enumerable: true, configurable: true });
  }
  function method(obj, name, fn) {
    Object.defineProperty(obj, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  function expose(name, F) {
    Object.defineProperty(global, name, { value: F, writable: true, configurable: true });
  }
  function finishInterface(name, F, Parent) {
    Object.defineProperty(F, 'name', { value: name, configurable: true });
    F.prototype = Object.create(Parent ? Parent.prototype : Object.prototype);
    Object.defineProperty(F.prototype, 'constructor', { value: F, writable: true, configurable: true });
    Object.defineProperty(F.prototype, Symbol.toStringTag, { value: name, configurable: true });
    if (Parent) Object.setPrototypeOf(F, Parent);
    expose(name, F);
  }
  function constants(F, table) {
    Object.keys(table).forEach(function (k) {
      Object.defineProperty(F, k, { value: table[k], enumerable: true });
      Object.defineProperty(F.prototype, k, { value: table[k], enumerable: true });
    });
  }

  // ---------------------------------------------------------------------------
  // WebIDL conversions for event init dictionaries
  // ---------------------------------------------------------------------------
  function toInt(v, bits, signed) {
    var x = Number(v);
    if (!isFinite(x)) return 0;
    x = x < 0 ? Math.ceil(x) : Math.floor(x);
    var m = Math.pow(2, bits);
    x = ((x % m) + m) % m;
    if (signed && x >= m / 2) x -= m;
    return x;
  }
  var conv = {
    boolean: function (v) { return !!v; },
    long: function (v) { return toInt(v, 32, true); },
    ulong: function (v) { return toInt(v, 32, false); },
    short: function (v) { return toInt(v, 16, true); },
    ushort: function (v) { return toInt(v, 16, false); },
    ulonglong: function (v) { var x = Number(v); return isFinite(x) ? Math.max(0, Math.floor(x)) : 0; },
    double: function (v) {
      var n = Number(v);
      if (!isFinite(n)) throw new TypeError('The provided double value is non-finite.');
      return n;
    },
    ndouble: function (v) { return v === null ? null : conv.double(v); },
    string: function (v) { return String(v); },
    nstring: function (v) { return v === null ? null : String(v); },
    any: function (v) { return v; },
    object: function (v) {
      if (!isObject(v)) throw new TypeError('The provided value is not an object.');
      return v;
    },
    target: function (v) {
      if (v === null) return null;
      if (!isTarget(v)) throw new TypeError("The provided value is not of type 'EventTarget'.");
      return v;
    },
    window: function (v) {
      if (v === null) return null;
      if (v !== global) throw new TypeError("The provided value is not of type 'Window'.");
      return v;
    },
    element: function (v) {
      if (v === null) return null;
      if (!(v instanceof global.HTMLElement)) throw new TypeError("The provided value is not of type 'HTMLElement'.");
      return v;
    },
    frozenArray: function (v) {
      if (!isObject(v) || typeof v[Symbol.iterator] !== 'function') {
        throw new TypeError('The provided value cannot be converted to a sequence.');
      }
      return Object.freeze(Array.from(v));
    }
  };
  function dictionary(init) {
    if (init === undefined || init === null) return {};
    if (!isObject(init)) throw new TypeError("The provided value is not of type 'EventInit'.");
    return init;
  }

  // ---------------------------------------------------------------------------
  // Event
  // ---------------------------------------------------------------------------
  function state(e) {
    var s = isObject(e) ? EV.get(e) : undefined;
    if (!s) throw new TypeError('Illegal invocation');
    return s;
  }
  function isEvent(v) { return isObject(v) && EV.has(v); }
  var isTrustedGetter = Object.getOwnPropertyDescriptor({
    get isTrusted() { return state(this).trusted; }
  }, 'isTrusted').get;
  // Set while document.createEvent builds an event (also allows interfaces without a
  // constructor, such as TextEvent).
  var creating = false;

  function Event(type) {
    if (new.target === undefined) throw new TypeError("Failed to construct 'Event': Please use the 'new' operator.");
    requireArgs(arguments, 1, 'Event');
    var t = String(type);
    var init = dictionary(arguments[1]);
    var s = {
      type: t, bubbles: false, cancelable: false, composed: false, initialized: true,
      dispatch: false, stop: false, stopImmediate: false, canceled: false, inPassive: false,
      target: null, currentTarget: null, phase: 0, path: [], trusted: false,
      timeStamp: now(), m: Object.create(null)
    };
    var v;
    if ((v = init.bubbles) !== undefined) s.bubbles = !!v;
    if ((v = init.cancelable) !== undefined) s.cancelable = !!v;
    if ((v = init.composed) !== undefined) s.composed = !!v;
    EV.set(this, s);
    Object.defineProperty(this, 'isTrusted', { get: isTrustedGetter, enumerable: true, configurable: false });
  }
  finishInterface('Event', Event, null);
  constants(Event, { NONE: 0, CAPTURING_PHASE: 1, AT_TARGET: 2, BUBBLING_PHASE: 3 });
  var EP = Event.prototype;
  function setCanceled(s) { if (s.cancelable && !s.inPassive) s.canceled = true; }
  function initialize(s, type, bubbles, cancelable) {
    s.initialized = true;
    s.stop = s.stopImmediate = s.canceled = false;
    s.trusted = false;
    s.target = null;
    s.type = type;
    s.bubbles = bubbles;
    s.cancelable = cancelable;
  }
  getter(EP, 'type', function () { return state(this).type; });
  getter(EP, 'target', function () { return state(this).target; });
  getter(EP, 'srcElement', function () { return state(this).target; });
  getter(EP, 'currentTarget', function () { return state(this).currentTarget; });
  getter(EP, 'eventPhase', function () { return state(this).phase; });
  getter(EP, 'bubbles', function () { return state(this).bubbles; });
  getter(EP, 'cancelable', function () { return state(this).cancelable; });
  getter(EP, 'composed', function () { return state(this).composed; });
  getter(EP, 'defaultPrevented', function () { return state(this).canceled; });
  getter(EP, 'timeStamp', function () { return state(this).timeStamp; });
  getter(EP, 'returnValue', function () { return !state(this).canceled; }, function (v) {
    var s = state(this);
    if (!v) setCanceled(s);
  });
  getter(EP, 'cancelBubble', function () { return state(this).stop; }, function (v) {
    var s = state(this);
    if (v) s.stop = true;
  });
  // DOM §2.2 composedPath(): the path, minus closed shadow trees the current target is
  // outside of.
  method(EP, 'composedPath', function composedPath() {
    var s = state(this);
    var path = s.path;
    if (!path.length) return [];
    var out = [s.currentTarget];
    var targetIndex = 0, targetLevel = 0, index;
    for (index = path.length - 1; index >= 0; index--) {
      if (path[index].rootOfClosedTree) targetLevel++;
      if (path[index].item === s.currentTarget) { targetIndex = index; break; }
      if (path[index].slotInClosedTree) targetLevel--;
    }
    var level = targetLevel, maxLevel = targetLevel;
    for (index = targetIndex - 1; index >= 0; index--) {
      if (path[index].rootOfClosedTree) level++;
      if (level <= maxLevel) out.unshift(path[index].item);
      if (path[index].slotInClosedTree) {
        level--;
        if (level < maxLevel) maxLevel = level;
      }
    }
    level = maxLevel = targetLevel;
    for (index = targetIndex + 1; index < path.length; index++) {
      if (path[index].slotInClosedTree) level++;
      if (level <= maxLevel) out.push(path[index].item);
      if (path[index].rootOfClosedTree) {
        level--;
        if (level < maxLevel) maxLevel = level;
      }
    }
    return out;
  });
  method(EP, 'stopPropagation', function stopPropagation() { state(this).stop = true; });
  method(EP, 'stopImmediatePropagation', function stopImmediatePropagation() {
    var s = state(this);
    s.stop = s.stopImmediate = true;
  });
  method(EP, 'preventDefault', function preventDefault() { setCanceled(state(this)); });
  method(EP, 'initEvent', function initEvent(type) {
    var s = state(this);
    requireArgs(arguments, 1, 'initEvent');
    if (s.dispatch) return;
    initialize(s, String(type), !!arguments[1], !!arguments[2]);
  });

  // An Event subclass. `groups` are the init dictionary levels, base first; each member
  // is [name, conversion, default, required]. Members are read in WebIDL order (each
  // level sorted by name) and exposed as getters.
  function defineEvent(name, Parent, groups, opts) {
    opts = opts || {};
    groups = groups.map(function (g) {
      return g.slice().sort(function (a, b) { return a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0; });
    });
    var F = function (type) {
      if (new.target === undefined) {
        throw new TypeError("Failed to construct '" + name + "': Please use the 'new' operator.");
      }
      if (opts.noConstructor && !creating) throw new TypeError('Illegal constructor');
      requireArgs(arguments, 1, name);
      var init = arguments[1];
      var self = Reflect.construct(Parent, [type, init], new.target);
      var d = dictionary(init);
      var m = EV.get(self).m;
      groups.forEach(function (g) {
        g.forEach(function (member) {
          var v = d[member[0]];
          if (v === undefined) {
            if (member[3]) {
              throw new TypeError("Failed to construct '" + name + "': required member " + member[0] +
                ' is undefined.');
            }
            m[member[0]] = member[2];
          } else {
            m[member[0]] = member[1](v);
          }
        });
      });
      return self;
    };
    finishInterface(name, F, Parent);
    Object.defineProperty(F, 'length', { value: 1, configurable: true });
    groups.forEach(function (g) {
      g.forEach(function (member) {
        var key = member[0];
        if (opts.hidden && opts.hidden.indexOf(key) >= 0) return;
        getter(F.prototype, key, function () { return state(this).m[key]; });
      });
    });
    if (opts.constants) constants(F, opts.constants);
    if (opts.methods) {
      Object.keys(opts.methods).forEach(function (k) { method(F.prototype, k, opts.methods[k]); });
    }
    return F;
  }

  function initUI(s, args) {
    initialize(s, String(args[0]), !!args[1], !!args[2]);
    s.m.view = args[3] === undefined ? null : conv.window(args[3]);
  }
  var UIEvent = defineEvent('UIEvent', Event, [
    [['view', conv.window, null], ['detail', conv.long, 0], ['which', conv.ulong, 0]]
  ], {
    methods: {
      initUIEvent: function initUIEvent(type) {
        var s = state(this);
        requireArgs(arguments, 1, 'initUIEvent');
        if (s.dispatch) return;
        initUI(s, arguments);
        s.m.detail = arguments[4] === undefined ? 0 : conv.long(arguments[4]);
      }
    }
  });
  defineEvent('FocusEvent', UIEvent, [[['relatedTarget', conv.target, null]]]);

  var MODIFIERS = [
    ['ctrlKey', conv.boolean, false], ['shiftKey', conv.boolean, false], ['altKey', conv.boolean, false],
    ['metaKey', conv.boolean, false], ['modifierAltGraph', conv.boolean, false],
    ['modifierCapsLock', conv.boolean, false], ['modifierFn', conv.boolean, false],
    ['modifierFnLock', conv.boolean, false], ['modifierHyper', conv.boolean, false],
    ['modifierNumLock', conv.boolean, false], ['modifierScrollLock', conv.boolean, false],
    ['modifierSuper', conv.boolean, false], ['modifierSymbol', conv.boolean, false],
    ['modifierSymbolLock', conv.boolean, false]
  ];
  var MODIFIER_KEYS = {
    Alt: 'altKey', Control: 'ctrlKey', Shift: 'shiftKey', Meta: 'metaKey', AltGraph: 'modifierAltGraph',
    CapsLock: 'modifierCapsLock', Fn: 'modifierFn', FnLock: 'modifierFnLock', Hyper: 'modifierHyper',
    NumLock: 'modifierNumLock', ScrollLock: 'modifierScrollLock', Super: 'modifierSuper',
    Symbol: 'modifierSymbol', SymbolLock: 'modifierSymbolLock'
  };
  var HIDDEN_MODIFIERS = MODIFIERS.slice(4).map(function (m) { return m[0]; });
  function getModifierState(key) {
    var s = state(this);
    requireArgs(arguments, 1, 'getModifierState');
    var k = MODIFIER_KEYS[String(key)];
    return k ? !!s.m[k] : false;
  }

  var MouseEvent = defineEvent('MouseEvent', UIEvent, [MODIFIERS, [
    ['screenX', conv.double, 0], ['screenY', conv.double, 0], ['clientX', conv.double, 0],
    ['clientY', conv.double, 0], ['button', conv.short, 0], ['buttons', conv.ushort, 0],
    ['relatedTarget', conv.target, null], ['movementX', conv.double, 0], ['movementY', conv.double, 0]
  ]], {
    hidden: HIDDEN_MODIFIERS,
    methods: {
      getModifierState: getModifierState,
      initMouseEvent: function initMouseEvent(type) {
        var s = state(this);
        requireArgs(arguments, 1, 'initMouseEvent');
        if (s.dispatch) return;
        var a = arguments;
        initUI(s, a);
        s.m.detail = a[4] === undefined ? 0 : conv.long(a[4]);
        s.m.screenX = a[5] === undefined ? 0 : conv.long(a[5]);
        s.m.screenY = a[6] === undefined ? 0 : conv.long(a[6]);
        s.m.clientX = a[7] === undefined ? 0 : conv.long(a[7]);
        s.m.clientY = a[8] === undefined ? 0 : conv.long(a[8]);
        s.m.ctrlKey = !!a[9];
        s.m.altKey = !!a[10];
        s.m.shiftKey = !!a[11];
        s.m.metaKey = !!a[12];
        s.m.button = a[13] === undefined ? 0 : conv.short(a[13]);
        s.m.relatedTarget = a[14] === undefined ? null : conv.target(a[14]);
      }
    }
  });
  var MP = MouseEvent.prototype;
  ['pageX', 'x'].forEach(function (k) { getter(MP, k, function () { return state(this).m.clientX; }); });
  ['pageY', 'y'].forEach(function (k) { getter(MP, k, function () { return state(this).m.clientY; }); });
  getter(MP, 'offsetX', function () { return state(this).m.clientX; });
  getter(MP, 'offsetY', function () { return state(this).m.clientY; });

  defineEvent('WheelEvent', MouseEvent, [[
    ['deltaX', conv.double, 0], ['deltaY', conv.double, 0], ['deltaZ', conv.double, 0],
    ['deltaMode', conv.ulong, 0]
  ]], { constants: { DOM_DELTA_PIXEL: 0, DOM_DELTA_LINE: 1, DOM_DELTA_PAGE: 2 } });
  defineEvent('PointerEvent', MouseEvent, [[
    ['pointerId', conv.long, 0], ['width', conv.double, 1], ['height', conv.double, 1],
    ['pressure', conv.double, 0], ['tangentialPressure', conv.double, 0], ['tiltX', conv.long, 0],
    ['tiltY', conv.long, 0], ['twist', conv.long, 0], ['altitudeAngle', conv.double, Math.PI / 2],
    ['azimuthAngle', conv.double, 0], ['pointerType', conv.string, ''], ['isPrimary', conv.boolean, false],
    ['persistentDeviceId', conv.long, 0]
  ]], {
    methods: {
      getCoalescedEvents: function getCoalescedEvents() { state(this); return []; },
      getPredictedEvents: function getPredictedEvents() { state(this); return []; }
    }
  });
  defineEvent('DragEvent', MouseEvent, [[['dataTransfer', conv.any, null]]]);
  defineEvent('KeyboardEvent', UIEvent, [MODIFIERS, [
    ['key', conv.string, ''], ['code', conv.string, ''], ['location', conv.ulong, 0],
    ['repeat', conv.boolean, false], ['isComposing', conv.boolean, false],
    ['charCode', conv.ulong, 0], ['keyCode', conv.ulong, 0]
  ]], {
    hidden: HIDDEN_MODIFIERS,
    constants: {
      DOM_KEY_LOCATION_STANDARD: 0, DOM_KEY_LOCATION_LEFT: 1, DOM_KEY_LOCATION_RIGHT: 2,
      DOM_KEY_LOCATION_NUMPAD: 3
    },
    methods: {
      getModifierState: getModifierState,
      initKeyboardEvent: function initKeyboardEvent(type) {
        var s = state(this);
        requireArgs(arguments, 1, 'initKeyboardEvent');
        if (s.dispatch) return;
        var a = arguments;
        initUI(s, a);
        s.m.key = a[4] === undefined ? '' : String(a[4]);
        s.m.location = a[5] === undefined ? 0 : conv.ulong(a[5]);
        s.m.ctrlKey = !!a[6];
        s.m.altKey = !!a[7];
        s.m.shiftKey = !!a[8];
        s.m.metaKey = !!a[9];
      }
    }
  });
  function initWithData(name) {
    return function (type) {
      var s = state(this);
      requireArgs(arguments, 1, name);
      if (s.dispatch) return;
      initUI(s, arguments);
      s.m.data = arguments[4] === undefined ? '' : String(arguments[4]);
    };
  }
  defineEvent('CompositionEvent', UIEvent, [[['data', conv.string, '']]], {
    methods: { initCompositionEvent: initWithData('initCompositionEvent') }
  });
  defineEvent('InputEvent', UIEvent, [[
    ['data', conv.nstring, null], ['isComposing', conv.boolean, false], ['inputType', conv.string, ''],
    ['dataTransfer', conv.any, null]
  ]], { methods: { getTargetRanges: function getTargetRanges() { state(this); return []; } } });
  defineEvent('TextEvent', UIEvent, [[['data', conv.string, '']]], {
    noConstructor: true,
    methods: { initTextEvent: initWithData('initTextEvent') }
  });

  defineEvent('CustomEvent', Event, [[['detail', conv.any, null]]], {
    methods: {
      initCustomEvent: function initCustomEvent(type) {
        var s = state(this);
        requireArgs(arguments, 1, 'initCustomEvent');
        if (s.dispatch) return;
        initialize(s, String(type), !!arguments[1], !!arguments[2]);
        s.m.detail = arguments[3] === undefined ? null : arguments[3];
      }
    }
  });
  var ErrorEvent = defineEvent('ErrorEvent', Event, [[
    ['message', conv.string, ''], ['filename', conv.string, ''], ['lineno', conv.ulong, 0],
    ['colno', conv.ulong, 0], ['error', conv.any, undefined]
  ]]);
  defineEvent('SecurityPolicyViolationEvent', Event, [[
    ['documentURI', conv.string, ''], ['referrer', conv.string, ''], ['blockedURI', conv.string, ''],
    ['violatedDirective', conv.string, ''], ['effectiveDirective', conv.string, ''],
    ['originalPolicy', conv.string, ''], ['sourceFile', conv.string, ''], ['sample', conv.string, ''],
    ['disposition', conv.string, 'enforce'], ['statusCode', conv.ulong, 0],
    ['lineNumber', conv.ulong, 0], ['columnNumber', conv.ulong, 0]
  ]]);
  defineEvent('HashChangeEvent', Event, [[['oldURL', conv.string, ''], ['newURL', conv.string, '']]]);
  defineEvent('PopStateEvent', Event, [[
    ['state', conv.any, null], ['hasUAVisualTransition', conv.boolean, false]
  ]]);
  defineEvent('PageTransitionEvent', Event, [[['persisted', conv.boolean, false]]]);
  defineEvent('MessageEvent', Event, [[
    ['data', conv.any, null], ['origin', conv.string, ''], ['lastEventId', conv.string, ''],
    ['source', conv.any, null], ['ports', conv.frozenArray, Object.freeze([])]
  ]], {
    methods: {
      initMessageEvent: function initMessageEvent(type) {
        var s = state(this);
        requireArgs(arguments, 1, 'initMessageEvent');
        if (s.dispatch) return;
        var a = arguments;
        initialize(s, String(type), !!a[1], !!a[2]);
        s.m.data = a[3] === undefined ? null : a[3];
        s.m.origin = a[4] === undefined ? '' : String(a[4]);
        s.m.lastEventId = a[5] === undefined ? '' : String(a[5]);
        s.m.source = a[6] === undefined ? null : a[6];
        s.m.ports = a[7] === undefined ? Object.freeze([]) : conv.frozenArray(a[7]);
      }
    }
  });
  defineEvent('StorageEvent', Event, [[
    ['key', conv.nstring, null], ['oldValue', conv.nstring, null], ['newValue', conv.nstring, null],
    ['url', conv.string, ''], ['storageArea', conv.any, null]
  ]], {
    methods: {
      initStorageEvent: function initStorageEvent(type) {
        var s = state(this);
        requireArgs(arguments, 1, 'initStorageEvent');
        if (s.dispatch) return;
        var a = arguments;
        initialize(s, String(type), !!a[1], !!a[2]);
        s.m.key = a[3] === undefined ? null : conv.nstring(a[3]);
        s.m.oldValue = a[4] === undefined ? null : conv.nstring(a[4]);
        s.m.newValue = a[5] === undefined ? null : conv.nstring(a[5]);
        s.m.url = a[6] === undefined ? '' : String(a[6]);
        s.m.storageArea = a[7] === undefined ? null : a[7];
      }
    }
  });
  var BeforeUnloadEvent = defineEvent('BeforeUnloadEvent', Event, [], { noConstructor: true });
  getter(BeforeUnloadEvent.prototype, 'returnValue', function () {
    var v = state(this).m.returnValue;
    return v === undefined ? '' : v;
  }, function (v) {
    state(this).m.returnValue = String(v);
  });
  defineEvent('DeviceMotionEvent', Event, [[
    ['acceleration', conv.any, null], ['accelerationIncludingGravity', conv.any, null],
    ['rotationRate', conv.any, null], ['interval', conv.double, 0]
  ]]);
  defineEvent('DeviceOrientationEvent', Event, [[
    ['alpha', conv.ndouble, null], ['beta', conv.ndouble, null], ['gamma', conv.ndouble, null],
    ['absolute', conv.boolean, false]
  ]]);
  defineEvent('ProgressEvent', Event, [[
    ['lengthComputable', conv.boolean, false], ['loaded', conv.ulonglong, 0], ['total', conv.ulonglong, 0]
  ]]);
  defineEvent('PromiseRejectionEvent', Event, [[
    ['promise', conv.object, undefined, true], ['reason', conv.any, undefined]
  ]]);
  defineEvent('AnimationEvent', Event, [[
    ['animationName', conv.string, ''], ['elapsedTime', conv.double, 0], ['pseudoElement', conv.string, '']
  ]]);
  defineEvent('TransitionEvent', Event, [[
    ['propertyName', conv.string, ''], ['elapsedTime', conv.double, 0], ['pseudoElement', conv.string, '']
  ]]);
  defineEvent('SubmitEvent', Event, [[['submitter', conv.element, null]]]);
  defineEvent('CloseEvent', Event, [[
    ['wasClean', conv.boolean, false], ['code', conv.ushort, 0], ['reason', conv.string, '']
  ]]);
  defineEvent('ToggleEvent', Event, [[['oldState', conv.string, ''], ['newState', conv.string, '']]]);
  defineEvent('MediaQueryListEvent', Event, [[['media', conv.string, ''], ['matches', conv.boolean, false]]]);

  // An uninitialized event of interface F (document.createEvent).
  hooks.createEvent = function (F) {
    creating = true;
    var e;
    try { e = new F(''); } finally { creating = false; }
    EV.get(e).initialized = false;
    return e;
  };

  // ---------------------------------------------------------------------------
  // EventTarget
  // ---------------------------------------------------------------------------
  function EventTarget() {
    if (new.target === undefined) {
      throw new TypeError("Failed to construct 'EventTarget': Please use the 'new' operator.");
    }
  }
  finishInterface('EventTarget', EventTarget, null);
  var ETP = EventTarget.prototype;
  function isTarget(v) { return v === global || v instanceof EventTarget; }
  function thisTarget(t) {
    if (t === undefined || t === null) return global;
    if (!isTarget(t)) throw new TypeError('Illegal invocation');
    return t;
  }
  function targetState(t) {
    var ts = TARGETS.get(t);
    if (!ts) {
      ts = { listeners: [], handlers: Object.create(null) };
      TARGETS.set(t, ts);
      hooks.scan(t);
    } else if (t === global) {
      hooks.scan(t);
    }
    return ts;
  }
  function listenerCallback(cb) {
    if (cb === undefined || cb === null) return null;
    if (!isObject(cb)) throw new TypeError("The provided value is not of type 'EventListener'.");
    return cb;
  }
  function flatten(options) {
    if (options === undefined || options === null) return false;
    if (isObject(options)) return !!options.capture;
    return !!options;
  }
  function removeListener(t, l) {
    l.removed = true;
    var list = targetState(t).listeners;
    var i = list.indexOf(l);
    if (i >= 0) list.splice(i, 1);
  }
  function passiveByDefault(t, type) {
    if (type !== 'touchstart' && type !== 'touchmove' && type !== 'wheel' && type !== 'mousewheel') {
      return false;
    }
    return t === global || hooks.passiveByDefault(t);
  }
  method(ETP, 'addEventListener', function addEventListener(type, callback) {
    var t = thisTarget(this);
    requireArgs(arguments, 2, 'addEventListener');
    type = String(type);
    callback = listenerCallback(callback);
    var options = arguments[2];
    var capture = false, once = false, passive = null, signal = null;
    if (isObject(options)) {
      capture = !!options.capture;
      once = !!options.once;
      var p = options.passive;
      if (p !== undefined) passive = !!p;
      var sg = options.signal;
      if (sg !== undefined) {
        if (!(global.AbortSignal && sg instanceof global.AbortSignal)) {
          throw new TypeError("Failed to read the 'signal' property: not of type 'AbortSignal'.");
        }
        signal = sg;
      }
    } else if (options !== undefined && options !== null) {
      capture = !!options;
    }
    if (signal && signal.aborted) return;
    if (callback === null) return;
    if (passive === null) passive = passiveByDefault(t, type);
    var list = targetState(t).listeners;
    for (var i = 0; i < list.length; i++) {
      var l = list[i];
      if (!l.handler && l.type === type && l.callback === callback && l.capture === capture) return;
    }
    var listener = {
      type: type, callback: callback, capture: capture, passive: passive, once: once, removed: false,
      handler: null
    };
    list.push(listener);
    if (signal && hooks.addAbortAlgorithm) {
      hooks.addAbortAlgorithm(signal, function () { removeListener(t, listener); });
    }
    hooks.added(t, type);
  });
  method(ETP, 'removeEventListener', function removeEventListener(type, callback) {
    var t = thisTarget(this);
    requireArgs(arguments, 2, 'removeEventListener');
    type = String(type);
    callback = listenerCallback(callback);
    var capture = flatten(arguments[2]);
    var list = targetState(t).listeners;
    for (var i = 0; i < list.length; i++) {
      var l = list[i];
      if (!l.handler && l.type === type && l.callback === callback && l.capture === capture) {
        removeListener(t, l);
        return;
      }
    }
  });
  method(ETP, 'dispatchEvent', function dispatchEvent(event) {
    var t = thisTarget(this);
    requireArgs(arguments, 1, 'dispatchEvent');
    if (!isEvent(event)) throw new TypeError("Failed to execute 'dispatchEvent': parameter 1 is not of type 'Event'.");
    var s = EV.get(event);
    if (s.dispatch || !s.initialized) throw domError('InvalidStateError', 'The event is already being dispatched or was not initialized.');
    s.trusted = false;
    return dispatch(t, event, false);
  });

  // ---------------------------------------------------------------------------
  // Dispatch (DOM §2.9)
  // ---------------------------------------------------------------------------
  var currentEvent;
  var LEGACY_TYPES = {
    animationend: 'webkitAnimationEnd', animationiteration: 'webkitAnimationIteration',
    animationstart: 'webkitAnimationStart', transitionend: 'webkitTransitionEnd'
  };

  // Shadow tree queries (DOM prelude), set while a path is built; null when no shadow
  // tree exists, so every node is in one tree.
  var sh = null;
  function treeRoot(n) { return sh ? sh.root(n) : null; }
  function retarget(a, b) { return a && sh ? sh.retarget(a, b) : a; }
  function isClosedShadowRoot(n) { return !!sh && sh.isClosedShadowRoot(n); }
  function inShadowTree(n) { return !!sh && sh.isShadowRoot(treeRoot(n)); }

  function dispatch(target, event, legacyTargetOverride) {
    var s = EV.get(event);
    s.dispatch = true;
    sh = hooks.shadow && hooks.shadow.active() ? hooks.shadow : null;
    var hasRelated = 'relatedTarget' in s.m;
    var originalRelated = hasRelated ? s.m.relatedTarget : null;
    var path = [];
    s.path = path;
    function append(item, adjusted, related, slotInClosedTree) {
      path.push({
        item: item, adjusted: adjusted, related: related,
        rootOfClosedTree: isClosedShadowRoot(item), slotInClosedTree: slotInClosedTree
      });
    }
    var related = retarget(originalRelated, target);
    if (target !== related || target === originalRelated) {
      append(target, legacyTargetOverride ? global.document : target, related, false);
      var slottable = sh && sh.isAssigned(target) ? target : null;
      var slotInClosedTree = false;
      var parent = hooks.getParent(target, s.type, s);
      while (parent) {
        if (slottable) {
          slottable = null;
          if (isClosedShadowRoot(treeRoot(parent))) slotInClosedTree = true;
        }
        if (sh && sh.isAssigned(parent)) slottable = parent;
        related = retarget(originalRelated, parent);
        if (!sh || !sh.isNode(parent) || sh.contains(treeRoot(target), parent)) {
          append(parent, null, related, slotInClosedTree);
        } else if (parent === related) {
          parent = null;
        } else {
          target = parent;
          append(parent, target, related, slotInClosedTree);
        }
        if (parent) parent = hooks.getParent(parent, s.type, s);
        slotInClosedTree = false;
      }
    }
    var clearTargets = false;
    for (var c = path.length - 1; c >= 0; c--) {
      if (path[c].adjusted === null) continue;
      clearTargets = inShadowTree(path[c].adjusted) || (!!path[c].related && inShadowTree(path[c].related));
      break;
    }
    sh = null;
    var i;
    for (i = path.length - 1; i >= 0; i--) {
      s.phase = path[i].adjusted !== null ? 2 : 1;
      invoke(path, i, event, true);
    }
    for (i = 0; i < path.length; i++) {
      if (path[i].adjusted !== null) {
        s.phase = 2;
      } else {
        if (!s.bubbles) continue;
        s.phase = 3;
      }
      invoke(path, i, event, false);
    }
    s.phase = 0;
    s.currentTarget = null;
    s.path = [];
    s.dispatch = s.stop = s.stopImmediate = false;
    if (clearTargets) {
      s.target = null;
      if (hasRelated) s.m.relatedTarget = null;
    }
    return !s.canceled;
  }

  function invoke(path, i, event, capturing) {
    var s = EV.get(event);
    for (var j = i; j >= 0; j--) {
      if (path[j].adjusted !== null) { s.target = path[j].adjusted; break; }
    }
    if ('relatedTarget' in s.m) s.m.relatedTarget = path[i].related;
    if (s.stop) return;
    var item = path[i].item;
    s.currentTarget = item;
    var listeners = targetState(item).listeners.slice();
    var found = innerInvoke(event, listeners, capturing, item);
    if (!found && s.trusted && LEGACY_TYPES[s.type]) {
      var original = s.type;
      s.type = LEGACY_TYPES[original];
      innerInvoke(event, listeners, capturing, item);
      s.type = original;
    }
  }

  function innerInvoke(event, listeners, capturing, item) {
    var s = EV.get(event);
    var found = false;
    for (var k = 0; k < listeners.length; k++) {
      var l = listeners[k];
      if (l.removed || l.type !== s.type) continue;
      found = true;
      if (capturing !== l.capture) continue;
      if (l.once) removeListener(item, l);
      var previous = currentEvent;
      currentEvent = event;
      if (l.passive) s.inPassive = true;
      try {
        if (l.handler) runHandler(item, l.handler, event);
        else callListener(l.callback, event, s.currentTarget);
      } catch (e) {
        reportException(e);
      }
      s.inPassive = false;
      currentEvent = previous;
      if (s.stopImmediate) break;
    }
    return found;
  }

  function callListener(cb, event, thisArg) {
    if (typeof cb === 'function') return cb.call(thisArg, event);
    var handleEvent = cb.handleEvent;
    if (typeof handleEvent !== 'function') throw new TypeError("'handleEvent' is not a function");
    return handleEvent.call(cb, event);
  }

  Object.defineProperty(global, 'event', {
    get: function () { return currentEvent; },
    set: function (v) {
      Object.defineProperty(global, 'event', { value: v, writable: true, enumerable: true, configurable: true });
    },
    enumerable: true,
    configurable: true
  });

  // "Report an exception": an ErrorEvent at the window; logged unless a listener
  // cancels it. Errors raised while reporting are only logged.
  var reporting = false;
  function reportException(e) {
    if (e instanceof Error && typeof e.stack === 'string') __axiom_debugStack(e.stack);
    if (reporting) { __axiom_log('Uncaught', describe(e)); return; }
    reporting = true;
    try {
      var ev = new ErrorEvent('error', { cancelable: true, message: 'Uncaught ' + describe(e), error: e });
      EV.get(ev).trusted = true;
      if (dispatch(global, ev, false)) __axiom_log('Uncaught', describe(e));
    } catch (inner) {
      __axiom_log('Uncaught', describe(e));
    } finally {
      reporting = false;
    }
  }

  // Trusted events fired by the platform.
  function dispatchTrusted(target, event, legacyTargetOverride) {
    EV.get(event).trusted = true;
    return dispatch(target, event, !!legacyTargetOverride);
  }
  function fire(target, type, init) {
    return dispatchTrusted(target, new Event(type, init));
  }
  hooks.dispatchTrusted = dispatchTrusted;
  hooks.fire = fire;

  // ---------------------------------------------------------------------------
  // Event handlers (HTML §8.1.8)
  // ---------------------------------------------------------------------------
  function handlerSlot(t, name) {
    var hs = targetState(t).handlers;
    return hs[name] || (hs[name] = { name: name, value: null, listener: null });
  }
  function deactivate(t, h) {
    h.value = null;
    if (h.listener) {
      removeListener(t, h.listener);
      h.listener = null;
    }
  }
  function activate(t, h) {
    if (h.listener) return;
    var type = h.name.substring(2);
    var l = {
      type: type, callback: null, capture: false, passive: passiveByDefault(t, type), once: false,
      removed: false, handler: h
    };
    targetState(t).listeners.push(l);
    h.listener = l;
    hooks.added(t, type);
  }
  // Uncompiled handlers are { source, element }; compiling happens on first use. The
  // host applies the document's Content Security Policy (a blocked handler stays null).
  var compileHandler = global.__axiom_compileHandler;
  function compile(t, h) {
    var v = h.value;
    var params = h.name === 'onerror' && t === global ? 'event, source, lineno, colno, error'
      : (v.element && v.element instanceof global.SVGElement ? 'evt' : 'event');
    try {
      var scopes = [global.document];
      if (v.element) scopes.push(v.element.form || {}, v.element);
      var fn = compileHandler(v.element ? v.element.__id : -1, params, v.source, scopes);
      if (fn !== null) Object.defineProperty(fn, 'name', { value: h.name, configurable: true });
      h.value = fn;
    } catch (e) {
      h.value = null;
      reportException(e);
    }
    return h.value;
  }
  function currentValue(t, h) {
    if (h.value !== null && typeof h.value === 'object' && h.value.__uncompiled === true) return compile(t, h);
    return h.value;
  }
  function runHandler(t, h, event) {
    var cb = currentValue(t, h);
    if (typeof cb !== 'function') return;
    var s = EV.get(event);
    var special = h.name === 'onerror' && t === global && event instanceof ErrorEvent && s.type === 'error';
    var ret;
    if (special) {
      ret = cb.call(t, s.m.message, s.m.filename, s.m.lineno, s.m.colno, s.m.error);
      if (ret === true) setCanceled(s);
    } else {
      ret = cb.call(t, event);
      if (event instanceof BeforeUnloadEvent && s.type === 'beforeunload') {
        if (ret !== undefined && ret !== null) {
          setCanceled(s);
          if (!s.m.returnValue) s.m.returnValue = String(ret);
        }
      } else if (ret === false) {
        setCanceled(s);
      }
    }
  }
  // Content attribute change for handler `name` on target `t` (element or window);
  // `source` null removes it.
  hooks.contentAttributeChanged = function (t, name, source, element) {
    var h = handlerSlot(t, name);
    if (source === null) { deactivate(t, h); return; }
    h.value = { __uncompiled: true, source: String(source), element: element };
    activate(t, h);
  };
  // IDL attributes `names` on `proto`; `resolve(obj)` maps the object to the target
  // that owns the handler (body / frameset forward to the window) or null.
  hooks.defineHandlers = function (proto, names, resolve) {
    names.forEach(function (name) {
      getter(proto, name, function () {
        var t = resolve ? resolve(this) : thisTarget(this);
        return t ? currentValue(t, handlerSlot(t, name)) : null;
      }, function (v) {
        var t = resolve ? resolve(this) : thisTarget(this);
        if (!t) return;
        var h = handlerSlot(t, name);
        if (!isObject(v)) { deactivate(t, h); return; }
        h.value = v;
        activate(t, h);
      });
    });
  };

  var GLOBAL_HANDLERS = ('onabort onauxclick onbeforeinput onbeforematch onbeforetoggle onblur oncancel ' +
    'oncanplay oncanplaythrough onchange onclick onclose oncommand oncontextlost oncontextmenu ' +
    'oncontextrestored oncopy oncuechange oncut ondblclick ondrag ondragend ondragenter ondragleave ' +
    'ondragover ondragstart ondrop ondurationchange onemptied onended onerror onfocus onformdata ' +
    'oninput oninvalid onkeydown onkeypress onkeyup onload onloadeddata onloadedmetadata onloadstart ' +
    'onmousedown onmouseenter onmouseleave onmousemove onmouseout onmouseover onmouseup onpaste ' +
    'onpause onplay onplaying onprogress onratechange onreset onresize onscroll onscrollend ' +
    'onsecuritypolicyviolation onseeked onseeking onselect onslotchange onstalled onsubmit onsuspend ' +
    'ontimeupdate ontoggle onvolumechange onwaiting onwebkitanimationend onwebkitanimationiteration ' +
    'onwebkitanimationstart onwebkittransitionend onwheel onanimationstart onanimationiteration ' +
    'onanimationend onanimationcancel ontransitionrun ontransitionstart ontransitionend ' +
    'ontransitioncancel onpointerover onpointerenter onpointerdown onpointermove onpointerup ' +
    'onpointercancel onpointerout onpointerleave ongotpointercapture onlostpointercapture ' +
    'onselectstart onselectionchange').split(' ');
  var WINDOW_HANDLERS = ('onafterprint onbeforeprint onbeforeunload onhashchange onlanguagechange ' +
    'onmessage onmessageerror onoffline ononline onpagehide onpagereveal onpageshow onpageswap ' +
    'onpopstate onrejectionhandled onstorage onunhandledrejection onunload').split(' ');
  // Body and frameset handlers that forward to the window.
  var WINDOW_REFLECTING = WINDOW_HANDLERS.concat(['onblur', 'onerror', 'onfocus', 'onload', 'onresize', 'onscroll']);
  hooks.GLOBAL_HANDLERS = GLOBAL_HANDLERS;
  hooks.WINDOW_REFLECTING = WINDOW_REFLECTING;

  // ---------------------------------------------------------------------------
  // Window
  // ---------------------------------------------------------------------------
  function Window() { throw new TypeError('Illegal constructor'); }
  finishInterface('Window', Window, EventTarget);
  Object.setPrototypeOf(global, Window.prototype);
  hooks.defineHandlers(Window.prototype, GLOBAL_HANDLERS.concat(WINDOW_HANDLERS), function (obj) {
    return obj === undefined || obj === null || obj === global ? global : null;
  });

  // Engine-driven UI event at the last node of `path` (root first). Returns false when
  // a listener canceled it, so the engine skips the default action.
  global.__axiom_dispatchJsEvent = function (path, type, clientX, clientY) {
    if (!path || !path.length) return true;
    var target = new ElementRef(path[path.length - 1] | 0);
    var ev = new MouseEvent(String(type), {
      bubbles: true, cancelable: true, composed: true, view: global, detail: 1,
      clientX: +clientX || 0, clientY: +clientY || 0, screenX: +clientX || 0, screenY: +clientY || 0
    });
    return dispatchTrusted(target, ev, false);
  };
  global.__axiom_reportError = reportException;
})(this);
