// Inline style (CSSOM §6.7): element.style is a CSSStyleDeclaration over the element's
// style attribute. Declarations are kept as written (shorthands are not expanded) and
// values are only checked for balanced brackets; the cascade parses them for real.
(function (global) {
  'use strict';

  var PROPERTIES = (
    'accent-color align-content align-items align-self all animation animation-delay ' +
    'animation-direction animation-duration animation-fill-mode animation-iteration-count ' +
    'animation-name animation-play-state animation-timing-function appearance aspect-ratio ' +
    'backdrop-filter backface-visibility background background-attachment background-blend-mode ' +
    'background-clip background-color background-image background-origin background-position ' +
    'background-position-x background-position-y background-repeat background-size block-size ' +
    'border border-block border-block-color border-block-end border-block-start border-block-style ' +
    'border-block-width border-bottom border-bottom-color border-bottom-left-radius ' +
    'border-bottom-right-radius border-bottom-style border-bottom-width border-collapse border-color ' +
    'border-image border-inline border-inline-end border-inline-start border-left border-left-color ' +
    'border-left-style border-left-width border-radius border-right border-right-color ' +
    'border-right-style border-right-width border-spacing border-style border-top border-top-color ' +
    'border-top-left-radius border-top-right-radius border-top-style border-top-width border-width ' +
    'bottom box-shadow box-sizing break-after break-before break-inside caption-side caret-color ' +
    'clear clip clip-path color color-scheme column-count column-gap column-rule column-span ' +
    'column-width columns contain container content content-visibility counter-increment ' +
    'counter-reset cursor direction display empty-cells fill filter flex flex-basis flex-direction ' +
    'flex-flow flex-grow flex-shrink flex-wrap float font font-family font-feature-settings ' +
    'font-kerning font-size font-stretch font-style font-variant font-variant-numeric font-weight ' +
    'gap grid grid-area grid-auto-columns grid-auto-flow grid-auto-rows grid-column grid-column-end ' +
    'grid-column-start grid-row grid-row-end grid-row-start grid-template grid-template-areas ' +
    'grid-template-columns grid-template-rows height hyphens image-rendering inline-size inset ' +
    'inset-block inset-inline isolation justify-content justify-items justify-self left ' +
    'letter-spacing line-break line-height list-style list-style-image list-style-position ' +
    'list-style-type margin margin-block margin-block-end margin-block-start margin-bottom ' +
    'margin-inline margin-inline-end margin-inline-start margin-left margin-right margin-top mask ' +
    'max-block-size max-height max-inline-size max-width min-block-size min-height min-inline-size ' +
    'min-width mix-blend-mode object-fit object-position opacity order orphans outline ' +
    'outline-color outline-offset outline-style outline-width overflow overflow-anchor ' +
    'overflow-wrap overflow-x overflow-y overscroll-behavior padding padding-block ' +
    'padding-block-end padding-block-start padding-bottom padding-inline padding-inline-end ' +
    'padding-inline-start padding-left padding-right padding-top page-break-after ' +
    'page-break-before page-break-inside perspective perspective-origin place-content place-items ' +
    'place-self pointer-events position quotes resize right rotate row-gap scale scroll-behavior ' +
    'scroll-margin scroll-padding scroll-snap-align scroll-snap-type scrollbar-color ' +
    'scrollbar-gutter scrollbar-width shape-outside stroke stroke-width tab-size table-layout ' +
    'text-align text-align-last text-decoration text-decoration-color text-decoration-line ' +
    'text-decoration-style text-decoration-thickness text-emphasis text-indent text-orientation ' +
    'text-overflow text-rendering text-shadow text-size-adjust text-transform text-underline-offset ' +
    'text-underline-position text-wrap top touch-action transform transform-origin transform-style ' +
    'transition transition-delay transition-duration transition-property ' +
    'transition-timing-function translate unicode-bidi user-select vertical-align visibility ' +
    'white-space widows width will-change word-break word-spacing word-wrap writing-mode z-index ' +
    'zoom -webkit-appearance -webkit-box-orient -webkit-font-smoothing -webkit-line-clamp ' +
    '-webkit-mask -webkit-tap-highlight-color -webkit-text-fill-color -webkit-text-size-adjust ' +
    '-webkit-text-stroke -webkit-transform -webkit-transition -webkit-user-select'
  ).split(' ');
  var SUPPORTED = Object.create(null);
  PROPERTIES.forEach(function (p) { SUPPORTED[p] = true; });

  function asciiLower(s) { return s.replace(/[A-Z]/g, function (c) { return c.toLowerCase(); }); }
  function canonical(name) {
    name = String(name);
    return name.substring(0, 2) === '--' ? name : asciiLower(name);
  }
  function supported(name) { return name.substring(0, 2) === '--' ? name.length > 2 : !!SUPPORTED[name]; }

  // Splits `text` at top-level `sep` (outside quotes, brackets and comments).
  function splitTopLevel(text, sep) {
    var out = [];
    var depth = 0;
    var quote = null;
    var start = 0;
    for (var i = 0; i < text.length; i++) {
      var c = text.charAt(i);
      if (quote) {
        if (c === '\\') i++;
        else if (c === quote) quote = null;
      } else if (c === '"' || c === "'") {
        quote = c;
      } else if (c === '/' && text.charAt(i + 1) === '*') {
        var end = text.indexOf('*/', i + 2);
        i = end < 0 ? text.length : end + 1;
      } else if (c === '(' || c === '[' || c === '{') {
        depth++;
      } else if (c === ')' || c === ']' || c === '}') {
        depth = Math.max(0, depth - 1);
      } else if (c === sep && depth === 0) {
        out.push(text.substring(start, i));
        start = i + 1;
      }
    }
    out.push(text.substring(start));
    return out;
  }
  function balanced(value) {
    var stack = [];
    var quote = null;
    for (var i = 0; i < value.length; i++) {
      var c = value.charAt(i);
      if (quote) {
        if (c === '\\') i++;
        else if (c === quote) quote = null;
      } else if (c === '"' || c === "'") {
        quote = c;
      } else if (c === '(' || c === '[' || c === '{') {
        stack.push(c === '(' ? ')' : c === '[' ? ']' : '}');
      } else if (c === ')' || c === ']' || c === '}') {
        if (stack.pop() !== c) return false;
      } else if (c === ';' && stack.length === 0) {
        return false;
      }
    }
    return stack.length === 0 && quote === null;
  }
  // "name: value [!important]" declarations; later duplicates replace earlier ones.
  function parseDeclarations(text) {
    var decls = [];
    splitTopLevel(String(text), ';').forEach(function (part) {
      var colon = part.indexOf(':');
      if (colon < 0) return;
      var name = canonical(part.substring(0, colon).replace(/\/\*[\s\S]*?\*\//g, '').trim());
      var value = part.substring(colon + 1).trim();
      var important = false;
      var m = /!\s*important\s*$/i.exec(value);
      if (m) {
        important = true;
        value = value.substring(0, m.index).trim();
      }
      if (!name || value === '' || !supported(name)) return;
      setDecl(decls, name, value, important);
    });
    return decls;
  }
  function setDecl(decls, name, value, important) {
    for (var i = 0; i < decls.length; i++) {
      if (decls[i].name === name) {
        decls[i].value = value;
        decls[i].important = important;
        return;
      }
    }
    decls.push({ name: name, value: value, important: important });
  }
  function serialize(decls) {
    return decls.map(function (d) {
      return d.name + ': ' + d.value + (d.important ? ' !important' : '') + ';';
    }).join(' ');
  }

  function CSSStyleDeclaration() { throw new TypeError('Illegal constructor'); }
  Object.defineProperty(CSSStyleDeclaration.prototype, Symbol.toStringTag, { value: 'CSSStyleDeclaration', configurable: true });
  Object.defineProperty(global, 'CSSStyleDeclaration', { value: CSSStyleDeclaration, writable: true, configurable: true });
  var CSDP = CSSStyleDeclaration.prototype;
  var OWNER = new WeakMap(); // declaration -> element id
  var COMPUTED = new WeakMap(); // computed declaration -> { id, pseudo }
  var computedNames = null;

  function names() {
    if (!computedNames) computedNames = __axiom_computedNames().map(String);
    return computedNames;
  }
  function readOnly(what) {
    throw new DOMException("Failed to execute '" + what + "' on 'CSSStyleDeclaration': These styles are computed, and therefore read-only.", 'NoModificationAllowedError');
  }
  function computedValue(c, name) {
    var v = __axiom_computedValue(c.id, c.pseudo, name);
    return v === null ? '' : String(v);
  }

  function ownerId(decl) {
    var id = OWNER.get(decl);
    if (id === undefined) throw new TypeError('Illegal invocation');
    return id;
  }
  function read(decl) {
    var v = __axiom_getAttr(ownerId(decl), 'style');
    return v === null ? [] : parseDeclarations(v);
  }
  function write(decl, decls) {
    var id = ownerId(decl);
    if (decls.length === 0 && __axiom_getAttr(id, 'style') === null) return;
    __axiom_setAttr(id, 'style', serialize(decls));
  }
  function method(name, fn) {
    Object.defineProperty(CSDP, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  function accessor(name, get, set) {
    Object.defineProperty(CSDP, name, { get: get, set: set, enumerable: true, configurable: true });
  }

  accessor('cssText', function () {
    return COMPUTED.has(this) ? '' : serialize(read(this));
  }, function (v) {
    if (COMPUTED.has(this)) readOnly('cssText');
    write(this, parseDeclarations(v === null ? '' : v));
  });
  accessor('length', function () {
    var c = COMPUTED.get(this);
    if (c) return c.pseudo === '' ? names().length : 0;
    return read(this).length;
  });
  accessor('parentRule', function () {
    if (!COMPUTED.has(this)) ownerId(this);
    return null;
  });
  method('item', function item(index) {
    index = Number(index) >>> 0;
    if (COMPUTED.has(this)) {
      var all = this.length ? names() : [];
      return index < all.length ? all[index] : '';
    }
    var decls = read(this);
    return index < decls.length ? decls[index].name : '';
  });
  method('getPropertyValue', function getPropertyValue(property) {
    var name = canonical(property);
    var c = COMPUTED.get(this);
    if (c) return computedValue(c, name);
    var decls = read(this);
    for (var i = 0; i < decls.length; i++) if (decls[i].name === name) return decls[i].value;
    return '';
  });
  method('getPropertyPriority', function getPropertyPriority(property) {
    var name = canonical(property);
    if (COMPUTED.has(this)) return '';
    var decls = read(this);
    for (var i = 0; i < decls.length; i++) if (decls[i].name === name) return decls[i].important ? 'important' : '';
    return '';
  });
  function removeProperty(decl, name) {
    var decls = read(decl);
    var old = '';
    var kept = decls.filter(function (d) {
      if (d.name !== name) return true;
      old = d.value;
      return false;
    });
    if (kept.length !== decls.length) write(decl, kept);
    return old;
  }
  function setProperty(decl, name, value, priority) {
    if (COMPUTED.has(decl)) readOnly('setProperty');
    if (!supported(name)) return;
    value = value === null ? '' : String(value).trim();
    if (value === '') { removeProperty(decl, name); return; }
    priority = priority === undefined || priority === null ? '' : asciiLower(String(priority));
    if (priority !== '' && priority !== 'important') return;
    if (!balanced(value)) return;
    var decls = read(decl);
    setDecl(decls, name, value, priority === 'important');
    write(decl, decls);
  }
  method('setProperty', function setProperty_(property, value, priority) {
    if (arguments.length < 2) {
      throw new TypeError("Failed to execute 'setProperty': 2 arguments required.");
    }
    setProperty(this, canonical(property), value, priority);
  });
  method('removeProperty', function removeProperty_(property) {
    if (COMPUTED.has(this)) readOnly('removeProperty');
    return removeProperty(this, canonical(property));
  });

  function camel(name) {
    return name.replace(/-([a-z])/g, function (_, c) { return c.toUpperCase(); });
  }
  function defineProperty(attr, name) {
    if (Object.prototype.hasOwnProperty.call(CSDP, attr)) return;
    accessor(attr, function () { return this.getPropertyValue(name); }, function (v) {
      setProperty(this, name, v, '');
    });
  }
  PROPERTIES.forEach(function (name) {
    defineProperty(name, name);
    if (name.charAt(0) === '-') {
      var webkit = camel(name.substring(1));
      defineProperty(webkit, name);
      defineProperty(webkit.charAt(0).toUpperCase() + webkit.substring(1), name);
    } else {
      defineProperty(camel(name), name);
    }
  });
  defineProperty('cssFloat', 'float');

  // ElementCSSInlineStyle: one declaration object per element; assigning sets cssText.
  var declarations = new Map();
  function styleOf(el) {
    var id = el.__id;
    if (typeof id !== 'number') throw new TypeError('Illegal invocation');
    var decl = declarations.get(id);
    if (!decl) {
      decl = Object.create(CSDP);
      OWNER.set(decl, id);
      declarations.set(id, decl);
    }
    return decl;
  }
  [global.HTMLElement, global.SVGElement, global.MathMLElement].forEach(function (I) {
    if (typeof I !== 'function') return;
    Object.defineProperty(I.prototype, 'style', {
      get: function () { return styleOf(this); },
      set: function (v) { styleOf(this).cssText = v; },
      enumerable: true,
      configurable: true
    });
  });

  // getComputedStyle (CSSOM §9): a live, read-only declaration of resolved values; each
  // read flushes style and layout. Pseudo-element styles are not computed, so their
  // declarations are empty.
  var PSEUDOS = ['before', 'after', 'marker', 'placeholder', 'first-line', 'first-letter', 'selection', 'backdrop'];
  function normalizePseudo(p) {
    if (p === undefined || p === null) return '';
    p = asciiLower(String(p));
    if (p === '' || p.charAt(0) !== ':') return '';
    var name = p.replace(/^::?/, '');
    return PSEUDOS.indexOf(name) >= 0 ? '::' + name : null;
  }
  Object.defineProperty(global, 'getComputedStyle', {
    value: function getComputedStyle(elt, pseudoElt) {
      if (!(elt instanceof global.Element)) {
        throw new TypeError("Failed to execute 'getComputedStyle' on 'Window': parameter 1 is not of type 'Element'.");
      }
      var pseudo = normalizePseudo(pseudoElt);
      var decl = Object.create(CSDP);
      COMPUTED.set(decl, { id: elt.__id, pseudo: pseudo === null ? ':invalid' : pseudo });
      return decl;
    },
    writable: true,
    enumerable: true,
    configurable: true
  });
})(this);
