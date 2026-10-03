// HTML forms (HTML §4.10): form control IDL attributes (value, checked, selected,
// selectedIndex, form, validity, ...), HTMLFormElement (elements, submit, requestSubmit,
// reset, checkValidity), the Option constructor, document.forms, activation behavior of
// checkboxes, radio buttons, submit and reset buttons and labels, implicit submission and
// FormData(form). Control state lives in the host, shared with layout and user input;
// the host plans the navigation of a submission.
// Not implemented: form named properties (form.fieldName; use form.elements), RadioNodeList,
// labels, text selection APIs, valueAsNumber / valueAsDate and the formdata event.
(function (global) {
  'use strict';
  var events = global.__axiom_events;
  var doc = global.document;
  var HTML_NS = 'http://www.w3.org/1999/xhtml';

  function getter(obj, name, get, set) {
    Object.defineProperty(obj, name, { get: get, set: set, enumerable: true, configurable: true });
  }
  function method(obj, name, fn) {
    Object.defineProperty(obj, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  function asciiLower(s) { return String(s).replace(/[A-Z]+/g, function (m) { return m.toLowerCase(); }); }
  function wrap(id) { id = id | 0; return id < 0 ? null : new ElementRef(id); }
  function attr(id, name) { return __axiom_getAttr(id, name); }
  function hasAttr(id, name) { return attr(id, name) !== null; }
  function tag(id) { return __axiom_namespaceURI(id) === HTML_NS ? String(__axiom_tagName(id)) : ''; }
  function parentOf(id) { return __axiom_parentNode(id) | 0; }
  function isConnected(id) { var w = wrap(id); return w !== null && w.isConnected; }
  function chainOf(id) {
    var out = [];
    while (id >= 0) { out.unshift(id); id = parentOf(id); }
    return out;
  }

  var INPUT_TYPES = ['hidden', 'text', 'search', 'tel', 'url', 'email', 'password', 'date', 'month',
    'week', 'time', 'datetime-local', 'number', 'range', 'color', 'checkbox', 'radio', 'file',
    'submit', 'image', 'reset', 'button'];
  var BLOCKS_IMPLICIT = ['text', 'search', 'url', 'tel', 'email', 'password', 'date', 'month', 'week',
    'time', 'datetime-local', 'number'];

  function inputType(id) {
    var t = attr(id, 'type');
    t = t === null ? '' : asciiLower(String(t).trim());
    return INPUT_TYPES.indexOf(t) >= 0 ? t : 'text';
  }
  function buttonType(id) {
    var t = attr(id, 'type');
    t = t === null ? '' : asciiLower(String(t).trim());
    return t === 'reset' || t === 'button' ? t : 'submit';
  }
  function isSubmitButton(id) {
    var t = tag(id);
    if (t === 'button') return buttonType(id) === 'submit';
    return t === 'input' && (inputType(id) === 'submit' || inputType(id) === 'image');
  }
  function isResetButton(id) {
    var t = tag(id);
    return (t === 'button' && buttonType(id) === 'reset') || (t === 'input' && inputType(id) === 'reset');
  }
  // Disabled: the attribute, or inside a disabled fieldset but not in its first legend.
  function isDisabled(id) {
    var t = tag(id);
    if (['button', 'input', 'select', 'textarea', 'fieldset'].indexOf(t) >= 0 && hasAttr(id, 'disabled')) return true;
    var child = id;
    for (var p = parentOf(id); p >= 0; child = p, p = parentOf(p)) {
      if (tag(p) !== 'fieldset' || !hasAttr(p, 'disabled')) continue;
      var legend = __axiom_childNodes(p).filter(function (c) { return tag(c) === 'legend'; })[0];
      if (legend === undefined || legend !== child) return true;
    }
    return false;
  }

  // Reflection (HTML §2.6.1).
  function reflect(protos, prop, name) {
    protos.forEach(function (p) {
      getter(p, prop, function () { var v = attr(this.__id, name); return v === null ? '' : String(v); },
        function (v) { __axiom_setAttr(this.__id, name, String(v)); });
    });
  }
  function reflectBool(protos, prop, name) {
    protos.forEach(function (p) {
      getter(p, prop, function () { return hasAttr(this.__id, name); }, function (v) {
        if (v) __axiom_setAttr(this.__id, name, '');
        else __axiom_removeAttr(this.__id, name);
      });
    });
  }
  function reflectEnum(protos, prop, name, values, missing, invalid) {
    protos.forEach(function (p) {
      getter(p, prop, function () {
        var v = attr(this.__id, name);
        if (v === null) return missing;
        v = asciiLower(v);
        return values.indexOf(v) >= 0 ? v : invalid;
      }, function (v) { __axiom_setAttr(this.__id, name, String(v)); });
    });
  }
  function reflectLong(protos, prop, name, fallback, min) {
    protos.forEach(function (p) {
      getter(p, prop, function () {
        var v = attr(this.__id, name);
        var n = v === null ? NaN : parseInt(String(v).trim(), 10);
        return isNaN(n) || n < min ? fallback : n;
      }, function (v) { __axiom_setAttr(this.__id, name, String(v | 0)); });
    });
  }
  function resolved(id, name, fallbackToDocument) {
    var v = attr(id, name);
    if (v === null || (fallbackToDocument && String(v).trim() === '')) {
      return fallbackToDocument ? String(__axiom_resolveUrl('')) : '';
    }
    var r = __axiom_resolveUrl(String(v));
    return r === null ? String(v) : String(r);
  }
  function reflectUrl(protos, prop, name, fallbackToDocument) {
    protos.forEach(function (p) {
      getter(p, prop, function () { return resolved(this.__id, name, fallbackToDocument); },
        function (v) { __axiom_setAttr(this.__id, name, String(v)); });
    });
  }

  var FORM = global.HTMLFormElement.prototype;
  var INPUT = global.HTMLInputElement.prototype;
  var BUTTON = global.HTMLButtonElement.prototype;
  var SELECT = global.HTMLSelectElement.prototype;
  var TEXTAREA = global.HTMLTextAreaElement.prototype;
  var OPTION = global.HTMLOptionElement.prototype;
  var FIELDSET = global.HTMLFieldSetElement.prototype;
  var OUTPUT = global.HTMLOutputElement.prototype;
  var LABEL = global.HTMLLabelElement.prototype;
  var LEGEND = global.HTMLLegendElement.prototype;
  var OPTGROUP = global.HTMLOptGroupElement.prototype;
  var SUBMITTABLE = [INPUT, BUTTON, SELECT, TEXTAREA];
  var LISTED = SUBMITTABLE.concat([FIELDSET, OUTPUT, global.HTMLObjectElement.prototype]);

  reflect(LISTED, 'name', 'name');
  reflectBool([INPUT, BUTTON, SELECT, TEXTAREA], 'autofocus', 'autofocus');
  reflectBool([INPUT, SELECT, TEXTAREA], 'required', 'required');
  reflectBool([INPUT, TEXTAREA], 'readOnly', 'readonly');
  reflectBool([INPUT, SELECT], 'multiple', 'multiple');
  reflect([INPUT, TEXTAREA], 'placeholder', 'placeholder');
  reflect([INPUT, TEXTAREA], 'dirName', 'dirname');
  reflect([INPUT, SELECT, TEXTAREA, FORM], 'autocomplete', 'autocomplete');
  reflectLong([INPUT, TEXTAREA], 'maxLength', 'maxlength', -1, 0);
  reflectLong([INPUT, TEXTAREA], 'minLength', 'minlength', -1, 0);
  ['min', 'max', 'step', 'pattern', 'accept', 'alt', 'inputMode', 'list'].forEach(function (p) {
    reflect([INPUT], p, asciiLower(p));
  });
  reflectLong([INPUT], 'size', 'size', 20, 1);
  reflectUrl([INPUT], 'src', 'src', false);
  reflectBool([INPUT], 'defaultChecked', 'checked');
  reflect([INPUT, BUTTON], 'defaultValue', 'value');
  reflect([BUTTON], 'value', 'value');
  reflectLong([SELECT], 'size', 'size', 0, 0);
  reflectLong([TEXTAREA], 'rows', 'rows', 2, 1);
  reflectLong([TEXTAREA], 'cols', 'cols', 20, 1);
  reflect([TEXTAREA], 'wrap', 'wrap');
  reflectBool([OPTION], 'defaultSelected', 'selected');
  reflect([OPTGROUP], 'label', 'label');
  reflect([LABEL], 'htmlFor', 'for');
  reflect([FORM], 'name', 'name');
  reflect([FORM], 'target', 'target');
  reflect([FORM], 'acceptCharset', 'accept-charset');
  reflect([FORM], 'rel', 'rel');
  reflectBool([FORM], 'noValidate', 'novalidate');
  reflectUrl([FORM], 'action', 'action', true);
  var ENCTYPES = ['application/x-www-form-urlencoded', 'multipart/form-data', 'text/plain'];
  reflectEnum([FORM], 'enctype', 'enctype', ENCTYPES, ENCTYPES[0], ENCTYPES[0]);
  reflectEnum([FORM], 'encoding', 'enctype', ENCTYPES, ENCTYPES[0], ENCTYPES[0]);
  reflectEnum([FORM], 'method', 'method', ['get', 'post', 'dialog'], 'get', 'get');
  // Submitter overrides: missing means "use the form's".
  reflectUrl([INPUT, BUTTON], 'formAction', 'formaction', true);
  reflectEnum([INPUT, BUTTON], 'formEnctype', 'formenctype', ENCTYPES, '', ENCTYPES[0]);
  reflectEnum([INPUT, BUTTON], 'formMethod', 'formmethod', ['get', 'post', 'dialog'], '', 'get');
  reflect([INPUT, BUTTON], 'formTarget', 'formtarget');
  reflectBool([INPUT, BUTTON], 'formNoValidate', 'formnovalidate');

  getter(INPUT, 'type', function () { return inputType(this.__id); },
    function (v) { __axiom_setAttr(this.__id, 'type', String(v)); });
  getter(BUTTON, 'type', function () { return buttonType(this.__id); },
    function (v) { __axiom_setAttr(this.__id, 'type', String(v)); });
  getter(SELECT, 'type', function () { return hasAttr(this.__id, 'multiple') ? 'select-multiple' : 'select-one'; });
  getter(TEXTAREA, 'type', function () { return 'textarea'; });
  getter(FIELDSET, 'type', function () { return 'fieldset'; });
  getter(OUTPUT, 'type', function () { return 'output'; });

  function nullToEmpty(v) { return v === null ? '' : String(v); }
  getterValue([INPUT, SELECT, TEXTAREA]);
  function getterValue(protos) {
    protos.forEach(function (p) {
      getter(p, 'value', function () { return String(__axiom_controlValue(this.__id)); },
        function (v) { __axiom_setControlValue(this.__id, nullToEmpty(v)); });
    });
  }
  getter(TEXTAREA, 'defaultValue', function () { return this.textContent; },
    function (v) { this.textContent = String(v); });
  getter(TEXTAREA, 'textLength', function () { return this.value.length; });
  getter(OUTPUT, 'value', function () { return this.textContent; },
    function (v) { this.textContent = String(v); });
  getter(OUTPUT, 'defaultValue', function () { return this.textContent; },
    function (v) { this.textContent = String(v); });

  getter(INPUT, 'checked', function () { return !!__axiom_controlChecked(this.__id); },
    function (v) { __axiom_setControlChecked(this.__id, !!v); });
  var indeterminate = Object.create(null);
  getter(INPUT, 'indeterminate', function () { return !!indeterminate[this.__id]; },
    function (v) { if (v) indeterminate[this.__id] = true; else delete indeterminate[this.__id]; });

  // Options.
  function collapse(s) { return String(s).replace(/[\t\n\f\r ]+/g, ' ').replace(/^ | $/g, ''); }
  getter(OPTION, 'value', function () { var v = attr(this.__id, 'value'); return v === null ? collapse(this.textContent) : String(v); },
    function (v) { __axiom_setAttr(this.__id, 'value', String(v)); });
  getter(OPTION, 'text', function () { return collapse(this.textContent); },
    function (v) { this.textContent = String(v); });
  getter(OPTION, 'label', function () { var v = attr(this.__id, 'label'); return v === null ? this.text : String(v); },
    function (v) { __axiom_setAttr(this.__id, 'label', String(v)); });
  getter(OPTION, 'selected', function () { return !!__axiom_controlChecked(this.__id); },
    function (v) { __axiom_setControlChecked(this.__id, !!v); });
  function optionSelect(id) {
    var p = parentOf(id);
    if (p >= 0 && tag(p) === 'optgroup') p = parentOf(p);
    return p >= 0 && tag(p) === 'select' ? p : -1;
  }
  getter(OPTION, 'index', function () {
    var s = optionSelect(this.__id);
    if (s < 0) return 0;
    return Array.prototype.indexOf.call(wrap(s).options, this);
  });
  getter(OPTION, 'form', function () { var s = optionSelect(this.__id); return s < 0 ? null : wrap(__axiom_formOwner(s)); });

  function Option(text, value, defaultSelected, selected) {
    if (!(this instanceof Option)) throw new TypeError("Failed to construct 'Option': Please use the 'new' operator.");
    var o = doc.createElement('option');
    if (text !== undefined && String(text) !== '') o.text = String(text);
    if (value !== undefined) o.value = String(value);
    if (defaultSelected) o.defaultSelected = true;
    if (selected) o.selected = true;
    return o;
  }
  Option.prototype = OPTION;
  Object.defineProperty(global, 'Option', { value: Option, writable: true, configurable: true });

  // Selects.
  getter(SELECT, 'options', function () { return this.getElementsByTagName('option'); });
  getter(SELECT, 'length', function () { return this.options.length; });
  getter(SELECT, 'selectedIndex', function () { return __axiom_selectedIndex(this.__id) | 0; }, function (v) {
    var opts = this.options;
    var i = v | 0;
    for (var k = 0; k < opts.length; k++) if (opts[k].selected && k !== i) opts[k].selected = false;
    if (i >= 0 && i < opts.length) opts[i].selected = true;
  });
  getter(SELECT, 'selectedOptions', function () {
    return Array.prototype.filter.call(this.options, function (o) { return o.selected; });
  });
  method(SELECT, 'item', function (i) { return this.options.item(i); });
  method(SELECT, 'namedItem', function (name) { return this.options.namedItem(name); });
  method(SELECT, 'add', function (element, before) {
    var ref = null;
    if (typeof before === 'number') ref = this.options[before] || null;
    else if (before !== undefined && before !== null) ref = before;
    var parent = ref !== null ? ref.parentNode : this;
    parent.insertBefore(element, ref);
  });
  var childRemove = global.Element.prototype.remove;
  method(SELECT, 'remove', function (index) {
    if (arguments.length === 0) return childRemove.call(this);
    var o = this.options[index | 0];
    if (o) o.remove();
  });

  // Form owners.
  getter(INPUT, 'form', formOf);
  [BUTTON, SELECT, TEXTAREA, FIELDSET, OUTPUT, global.HTMLObjectElement.prototype].forEach(function (p) {
    getter(p, 'form', formOf);
  });
  function formOf() { return wrap(__axiom_formOwner(this.__id)); }
  function labeledControl(id) {
    var forId = attr(id, 'for');
    var labelable = ['button', 'input', 'meter', 'output', 'progress', 'select', 'textarea'];
    var ok = function (c) { return c >= 0 && labelable.indexOf(tag(c)) >= 0 && !(tag(c) === 'input' && inputType(c) === 'hidden'); };
    if (forId !== null) {
      var t = __axiom_getElementById(String(forId)) | 0;
      return ok(t) ? t : -1;
    }
    var stack = __axiom_childNodes(id).slice();
    while (stack.length) {
      var c = stack.shift();
      if (ok(c)) return c;
      stack = __axiom_childNodes(c).concat(stack);
    }
    return -1;
  }
  getter(LABEL, 'control', function () { return wrap(labeledControl(this.__id)); });
  getter(LABEL, 'form', function () { var c = labeledControl(this.__id); return c < 0 ? null : wrap(__axiom_formOwner(c)); });
  getter(LEGEND, 'form', function () {
    var p = parentOf(this.__id);
    return p >= 0 && tag(p) === 'fieldset' ? wrap(__axiom_formOwner(p)) : null;
  });

  // HTMLFormControlsCollection (a snapshot: indexed and named properties are plain).
  function FormControls() { throw new TypeError('Illegal constructor'); }
  Object.defineProperty(global, 'HTMLFormControlsCollection', { value: FormControls, writable: true, configurable: true });
  method(FormControls.prototype, 'item', function (i) { return this[i >>> 0] || null; });
  method(FormControls.prototype, 'namedItem', function (name) {
    name = String(name);
    for (var i = 0; i < this.length; i++) {
      var id = this[i].__id;
      if (attr(id, 'id') === name || attr(id, 'name') === name) return this[i];
    }
    return null;
  });
  FormControls.prototype[Symbol.iterator] = Array.prototype[Symbol.iterator];
  function controlsCollection(ids) {
    var c = Object.create(FormControls.prototype);
    ids.forEach(function (id, i) { c[i] = wrap(id); });
    Object.defineProperty(c, 'length', { value: ids.length });
    ids.forEach(function (id) {
      [attr(id, 'id'), attr(id, 'name')].forEach(function (n) {
        if (n && !/^\d+$/.test(n) && !(n in c)) Object.defineProperty(c, n, { value: wrap(id) });
      });
    });
    return c;
  }
  function formElements(formId) {
    return __axiom_formControls(formId).filter(function (id) {
      return !(tag(id) === 'input' && inputType(id) === 'image');
    });
  }
  getter(FORM, 'elements', function () { return controlsCollection(formElements(this.__id)); });
  getter(FORM, 'length', function () { return formElements(this.__id).length; });
  getter(FIELDSET, 'elements', function () {
    var listed = ['button', 'fieldset', 'input', 'object', 'output', 'select', 'textarea'];
    return controlsCollection(Array.prototype.filter.call(this.getElementsByTagName('*'), function (e) {
      return listed.indexOf(tag(e.__id)) >= 0;
    }).map(function (e) { return e.__id; }));
  });
  Object.defineProperty(doc, 'forms', {
    get: function () { return doc.getElementsByTagName('form'); },
    enumerable: true,
    configurable: true
  });

  // Constraint validation (HTML §4.10.20).
  var FLAGS = ['valueMissing', 'typeMismatch', 'patternMismatch', 'tooLong', 'tooShort',
    'rangeUnderflow', 'rangeOverflow', 'stepMismatch', 'badInput', 'customError'];
  function ValidityState() { throw new TypeError('Illegal constructor'); }
  Object.defineProperty(global, 'ValidityState', { value: ValidityState, writable: true, configurable: true });
  FLAGS.forEach(function (flag) {
    getter(ValidityState.prototype, flag, function () { return __axiom_controlValidity(this.__control)[1] === flag; });
  });
  getter(ValidityState.prototype, 'valid', function () { return __axiom_controlValidity(this.__control)[1] === ''; });
  function checkValidity(el) {
    if (__axiom_controlValidity(el.__id)[1] === '') return true;
    events.fire(el, 'invalid', { cancelable: true });
    return false;
  }
  var VALIDATED = SUBMITTABLE.concat([FIELDSET, OUTPUT]);
  VALIDATED.forEach(function (p) {
    getter(p, 'validity', function () {
      var v = Object.create(ValidityState.prototype);
      Object.defineProperty(v, '__control', { value: this.__id });
      return v;
    });
    getter(p, 'willValidate', function () { return !!__axiom_controlValidity(this.__id)[0]; });
    getter(p, 'validationMessage', function () { return String(__axiom_controlValidity(this.__id)[2]); });
    method(p, 'checkValidity', function () { return checkValidity(this); });
    method(p, 'reportValidity', function () {
      var ok = checkValidity(this);
      if (!ok) __axiom_log('invalid form field: ' + this.validationMessage);
      return ok;
    });
    method(p, 'setCustomValidity', function (message) {
      if (arguments.length < 1) throw new TypeError("Failed to execute 'setCustomValidity': 1 argument required, but only 0 present.");
      __axiom_setCustomValidity(this.__id, String(message));
    });
  });
  function formValidity(form, report) {
    var invalid = __axiom_invalidControls(form.__id);
    var unhandled = [];
    invalid.forEach(function (id) {
      if (events.fire(wrap(id), 'invalid', { cancelable: true })) unhandled.push(id);
    });
    if (report) {
      unhandled.forEach(function (id) {
        __axiom_log('invalid form field ' + (attr(id, 'name') || tag(id)) + ': ' +
          String(__axiom_controlValidity(id)[2]));
      });
    }
    return invalid.length === 0;
  }
  method(FORM, 'checkValidity', function () { return formValidity(this, false); });
  method(FORM, 'reportValidity', function () { return formValidity(this, true); });

  // Submission (HTML §4.10.21.3) and reset (§4.10.22).
  var firing = Object.create(null);
  function submitForm(formId, submitterId, fromSubmitMethod) {
    if (!isConnected(formId)) return;
    if (!fromSubmitMethod) {
      if (firing[formId]) return;
      var noValidate = hasAttr(formId, 'novalidate') ||
        (submitterId >= 0 && hasAttr(submitterId, 'formnovalidate'));
      if (!noValidate && !formValidity(wrap(formId), true)) return;
      firing[formId] = true;
      var ok;
      try {
        var ev = new global.SubmitEvent('submit', { bubbles: true, cancelable: true, submitter: wrap(submitterId) });
        ok = events.dispatchTrusted(wrap(formId), ev);
      } finally {
        delete firing[formId];
      }
      if (!ok || !isConnected(formId)) return;
    }
    var err = __axiom_submitForm(formId, submitterId);
    if (err !== null) __axiom_log('form not submitted: ' + err);
  }
  function resetForm(formId) {
    if (events.fire(wrap(formId), 'reset', { bubbles: true, cancelable: true })) __axiom_resetForm(formId);
  }
  method(FORM, 'submit', function () { submitForm(this.__id, -1, true); });
  method(FORM, 'requestSubmit', function (submitter) {
    var sid = -1;
    if (submitter !== undefined && submitter !== null) {
      if (!(submitter instanceof global.HTMLElement) || !isSubmitButton(submitter.__id)) {
        throw new TypeError("Failed to execute 'requestSubmit' on 'HTMLFormElement': The specified element is not a submit button.");
      }
      if ((__axiom_formOwner(submitter.__id) | 0) !== this.__id) {
        throw new global.DOMException("Failed to execute 'requestSubmit' on 'HTMLFormElement': The specified element is not owned by this form element.", 'NotFoundError');
      }
      sid = submitter.__id;
    }
    submitForm(this.__id, sid, false);
  });
  method(FORM, 'reset', function () { resetForm(this.__id); });

  global.__axiom_formEntryList = function (form, submitter) {
    if (!(form instanceof global.HTMLFormElement)) return null;
    var sid = -1;
    if (submitter !== undefined && submitter !== null) {
      if (!(submitter instanceof global.HTMLElement) || !isSubmitButton(submitter.__id)) {
        throw new TypeError("Failed to construct 'FormData': The specified element is not a submit button.");
      }
      if ((__axiom_formOwner(submitter.__id) | 0) !== form.__id) {
        throw new global.DOMException("Failed to construct 'FormData': The specified element is not owned by this form element.", 'NotFoundError');
      }
      sid = submitter.__id;
    }
    return __axiom_formEntries(form.__id, sid);
  };

  // Activation behavior (HTML §6.5.1 / DOM "activation behavior"): the nearest element on
  // the event path that has one. Checkboxes and radio buttons change state before
  // dispatch (legacy-pre-activation) and restore it when the click is canceled.
  function hasActivation(id) {
    var t = tag(id);
    if (t === 'button' || t === 'label') return true;
    return t === 'input' && ['checkbox', 'radio', 'submit', 'image', 'reset'].indexOf(inputType(id)) >= 0;
  }
  function radioGroupChecked(id) {
    var name = attr(id, 'name');
    if (!name) return -1;
    var owner = __axiom_formOwner(id) | 0;
    var all = doc.getElementsByTagName('input');
    for (var i = 0; i < all.length; i++) {
      var r = all[i].__id;
      if (r !== id && inputType(r) === 'radio' && attr(r, 'name') === name &&
          (__axiom_formOwner(r) | 0) === owner && __axiom_controlChecked(r)) return r;
    }
    return -1;
  }
  var activation = {
    pre: function (path) {
      var target = -1;
      for (var i = path.length - 1; i >= 0; i--) {
        if (hasActivation(path[i])) { target = path[i]; break; }
      }
      if (target < 0 || isDisabled(target)) return null;
      var state = { target: target, clicked: path[path.length - 1] };
      if (tag(target) === 'input') {
        var type = inputType(target);
        if (type === 'checkbox') {
          state.old = !!__axiom_controlChecked(target);
          __axiom_setControlChecked(target, !state.old);
        } else if (type === 'radio') {
          state.old = !!__axiom_controlChecked(target);
          state.previous = radioGroupChecked(target);
          __axiom_setControlChecked(target, true);
        }
      }
      return state;
    },
    post: function (state, notCanceled) {
      if (state === null) return;
      var id = state.target;
      var type = tag(id) === 'input' ? inputType(id) : '';
      if (!notCanceled) {
        if (type === 'checkbox') __axiom_setControlChecked(id, state.old);
        else if (type === 'radio') {
          if (state.previous >= 0) __axiom_setControlChecked(state.previous, true);
          else __axiom_setControlChecked(id, state.old);
        }
        return;
      }
      if (type === 'checkbox' || (type === 'radio' && !state.old)) {
        if (isConnected(id)) {
          events.fire(wrap(id), 'input', { bubbles: true, composed: true });
          events.fire(wrap(id), 'change', { bubbles: true });
        }
        return;
      }
      if (tag(id) === 'label') {
        var control = labeledControl(id);
        if (control >= 0 && chainOf(state.clicked).indexOf(control) < 0) wrap(control).click();
        return;
      }
      var form = __axiom_formOwner(id) | 0;
      if (form < 0 || !isConnected(id)) return;
      if (isSubmitButton(id)) submitForm(form, id, false);
      else if (isResetButton(id)) resetForm(form);
    }
  };
  global.__axiom_activation = { pre: function (id) { return activation.pre(chainOf(id)); }, post: activation.post };

  // Engine-driven user click with activation; false when a listener canceled it.
  global.__axiom_userClick = function (path, clientX, clientY) {
    var state = activation.pre(path);
    var ok = global.__axiom_dispatchJsEvent(path, 'click', clientX, clientY);
    activation.post(state, ok);
    return ok;
  };
  global.__axiom_fireFormEvent = function (id, type) {
    events.fire(wrap(id), String(type), { bubbles: true, composed: type === 'input' });
  };
  // Enter in a text control: click the form's default button, or submit when at most one
  // field blocks implicit submission (HTML §4.10.21.2).
  global.__axiom_implicitSubmit = function (id) {
    var form = __axiom_formOwner(id) | 0;
    if (form < 0) return;
    var controls = __axiom_formControls(form);
    for (var i = 0; i < controls.length; i++) {
      if (isSubmitButton(controls[i])) {
        if (!isDisabled(controls[i])) wrap(controls[i]).click();
        return;
      }
    }
    var blocking = controls.filter(function (c) {
      return tag(c) === 'input' && BLOCKS_IMPLICIT.indexOf(inputType(c)) >= 0;
    }).length;
    if (blocking <= 1) submitForm(form, -1, false);
  };
})(this);
