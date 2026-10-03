// ECMA-402 (Intl) for the en-US locale, and the locale-sensitive Date / Number / BigInt /
// String methods built on it. Axiom carries no CLDR data: every formatter resolves to
// en-US (supportedLocalesOf only admits English tags) and follows Chrome's output for it,
// including U+202F before AM/PM. Time zones: UTC, fixed offsets (Etc/GMT±N, ±HH:MM) and
// the system zone; other IANA names are accepted and formatted in the system zone. The
// default zone is reported as UTC or its Etc/GMT offset name.
(function (global) {
  'use strict';
  var Intl = global.Intl;
  if (!Intl || typeof Intl !== 'object') {
    Intl = {};
    Object.defineProperty(global, 'Intl', { value: Intl, writable: true, configurable: true });
  }
  Object.defineProperty(Intl, Symbol.toStringTag, { value: 'Intl', configurable: true });
  var LOCALE = 'en-US';
  var STATE = new WeakMap();

  function define(obj, name, value) {
    Object.defineProperty(obj, name, { value: value, writable: true, configurable: true });
  }
  function getter(obj, name, get) {
    Object.defineProperty(obj, name, { get: get, configurable: true });
  }
  function slots(obj, kind) {
    var s = typeof obj === 'object' && obj !== null ? STATE.get(obj) : undefined;
    if (!s || s.kind !== kind) throw new TypeError('Method Intl.' + kind + ' called on incompatible receiver');
    return s;
  }
  function options_(options) {
    if (options === undefined) return Object.create(null);
    if (options === null) throw new TypeError('Cannot convert null to object');
    return Object(options);
  }
  function option(options, name, type, values, fallback) {
    var v = options[name];
    if (v === undefined) return fallback;
    v = type === 'boolean' ? Boolean(v) : String(v);
    if (values && values.indexOf(v) < 0) {
      throw new RangeError('Value ' + v + ' out of range for Intl options property ' + name);
    }
    return v;
  }
  function numberOption(options, name, min, max, fallback) {
    var v = options[name];
    if (v === undefined) return fallback;
    v = Number(v);
    if (isNaN(v) || v < min || v > max) throw new RangeError(name + ' value is out of range.');
    return Math.floor(v);
  }
  function repeat(s, n) { return n > 0 ? new Array(n + 1).join(s) : ''; }
  function pad2(n) { return (n < 10 ? '0' : '') + n; }
  function join(parts) {
    var s = '';
    for (var i = 0; i < parts.length; i++) s += parts[i].value;
    return s;
  }
  function pushPart(parts, type, value) {
    if (value === '') return;
    var last = parts[parts.length - 1];
    if (type === 'literal' && last && last.type === 'literal') last.value += value;
    else parts.push({ type: type, value: value });
  }
  function boundGetter(proto, name, method) {
    getter(proto, name, function () {
      var self = this;
      var s = STATE.get(self);
      if (!s) throw new TypeError('Method get ' + name + ' called on incompatible receiver');
      if (!s['bound_' + name]) {
        s['bound_' + name] = function (a, b) { return method.call(self, a, b); };
      }
      return s['bound_' + name];
    });
  }
  function service(name, ctor, methods) {
    define(ctor, 'supportedLocalesOf', function supportedLocalesOf(locales, options) {
      return supportedLocales(locales, options);
    });
    Object.defineProperty(ctor.prototype, Symbol.toStringTag, { value: 'Intl.' + name, configurable: true });
    for (var k in methods) define(ctor.prototype, k, methods[k]);
    define(Intl, name, ctor);
  }

  // ---------------------------------------------------------------------------
  // Locales
  // ---------------------------------------------------------------------------
  var TAG = /^([a-z]{2,3}|[a-z]{5,8})(-[a-z]{4})?(-(?:[a-z]{2}|\d{3}))?((?:-(?:[a-z0-9]{5,8}|\d[a-z0-9]{3}))*)((?:-[a-wy-z0-9](?:-[a-z0-9]{2,8})+)*)((?:-x(?:-[a-z0-9]{1,8})+)?)$/;
  function canonicalTag(tag) {
    if (typeof tag === 'object' && tag !== null && STATE.has(tag) && STATE.get(tag).kind === 'Locale') {
      return STATE.get(tag).tag;
    }
    if (typeof tag !== 'string' && (typeof tag !== 'object' || tag === null)) {
      throw new TypeError('Language ID should be string or object.');
    }
    var m = TAG.exec(String(tag).toLowerCase());
    if (!m) throw new RangeError('Incorrect locale information provided');
    var out = m[1];
    if (m[2]) out += '-' + m[2].charAt(1).toUpperCase() + m[2].substring(2);
    if (m[3]) out += '-' + m[3].substring(1).toUpperCase();
    return out + m[4] + m[5] + m[6];
  }
  function canonicalList(locales) {
    if (locales === undefined) return [];
    var list = typeof locales === 'string' || (STATE.has(Object(locales)) && STATE.get(locales).kind === 'Locale')
      ? [locales] : Object(locales);
    var len = list.length >>> 0, out = [];
    for (var i = 0; i < len; i++) {
      if (!(i in list)) continue;
      var t = canonicalTag(list[i]);
      if (out.indexOf(t) < 0) out.push(t);
    }
    return out;
  }
  function resolveLocale(locales, options) {
    canonicalList(locales);
    option(options, 'localeMatcher', 'string', ['lookup', 'best fit'], 'best fit');
    return LOCALE;
  }
  function supportedLocales(locales, options) {
    var list = canonicalList(locales);
    if (options !== undefined) option(options_(options), 'localeMatcher', 'string', ['lookup', 'best fit'], 'best fit');
    return list.filter(function (t) { return t === 'en' || t.indexOf('en-') === 0; });
  }
  define(Intl, 'getCanonicalLocales', function getCanonicalLocales(locales) { return canonicalList(locales); });

  // ---------------------------------------------------------------------------
  // Numbers: exact decimal arithmetic on the shortest round-trip digits (like ICU)
  // ---------------------------------------------------------------------------
  // A decimal is { d: significant digits without leading/trailing zeros, p: exponent }
  // meaning 0.d × 10^p; zero is { d: '', p: 0 }.
  function decimalFromDigits(digits, point) {
    var lead = 0;
    while (lead < digits.length && digits.charAt(lead) === '0') { lead++; point--; }
    digits = digits.substring(lead).replace(/0+$/, '');
    return digits ? { d: digits, p: point } : { d: '', p: 0 };
  }
  function decimalOf(x) {
    if (typeof x === 'bigint') {
      var b = (x < 0 ? -x : x).toString();
      return decimalFromDigits(b, b.length);
    }
    var s = Math.abs(x).toString(), e = 0, i = s.indexOf('e');
    if (i >= 0) { e = Number(s.substring(i + 1)); s = s.substring(0, i); }
    var dot = s.indexOf('.');
    var digits = dot < 0 ? s : s.substring(0, dot) + s.substring(dot + 1);
    return decimalFromDigits(digits, (dot < 0 ? s.length : dot) + e);
  }
  var DECIMAL_STRING = /^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i;
  function mathematicalValue(x) {
    if (typeof x === 'bigint') return { neg: x < 0, dec: decimalOf(x) };
    if (typeof x === 'string') {
      var t = x.replace(/^[\s\uFEFF\xA0]+|[\s\uFEFF\xA0]+$/g, '');
      if (DECIMAL_STRING.test(t)) {
        var neg = t.charAt(0) === '-';
        t = t.replace(/^[+-]/, '');
        var e = 0, i = t.search(/e/i);
        if (i >= 0) { e = Number(t.substring(i + 1)); t = t.substring(0, i); }
        var dot = t.indexOf('.');
        var digits = dot < 0 ? t : t.substring(0, dot) + t.substring(dot + 1);
        return { neg: neg, dec: decimalFromDigits(digits, (dot < 0 ? t.length : dot) + e) };
      }
    }
    var n = Number(x);
    if (n !== n) return { nan: true };
    return { neg: n < 0 || (n === 0 && 1 / n < 0), inf: !isFinite(n), dec: isFinite(n) ? decimalOf(n) : null };
  }
  // Keep `keep` leading digits of dec (keep may be <= 0), rounding by `mode`.
  function roundDecimal(dec, keep, mode, neg) {
    var d = dec.d;
    if (keep >= d.length) return dec;
    var up;
    if (keep < 0) {
      up = mode === 'expand' || (mode === 'ceil' && !neg) || (mode === 'floor' && neg);
      return up ? roundAway(dec, keep) : { d: '', p: 0 };
    }
    var first = d.charCodeAt(keep) - 48;
    var rest = /[1-9]/.test(d.substring(keep + 1));
    var half = first > 5 || (first === 5 && rest) ? 1 : first === 5 ? 0 : -1;
    var lastOdd = keep > 0 && (d.charCodeAt(keep - 1) - 48) % 2 === 1;
    switch (mode) {
      case 'ceil': up = !neg; break;
      case 'floor': up = neg; break;
      case 'expand': up = true; break;
      case 'trunc': up = false; break;
      case 'halfCeil': up = half > 0 || (half === 0 && !neg); break;
      case 'halfFloor': up = half > 0 || (half === 0 && neg); break;
      case 'halfTrunc': up = half > 0; break;
      case 'halfEven': up = half > 0 || (half === 0 && lastOdd); break;
      default: up = half >= 0;
    }
    var kept = d.substring(0, keep), p = dec.p;
    if (up) {
      var arr = kept.split(''), i = arr.length - 1;
      while (i >= 0 && arr[i] === '9') { arr[i] = '0'; i--; }
      if (i >= 0) arr[i] = String.fromCharCode(arr[i].charCodeAt(0) + 1);
      kept = arr.join('');
      if (i < 0) { kept = '1' + kept; p++; }
    }
    kept = kept.replace(/0+$/, '');
    return kept ? { d: kept, p: p } : { d: '', p: 0 };
  }
  // Rounding a nonzero value whose first digit lies past the rounding position away from
  // zero yields one unit of that position.
  function roundAway(dec, keep) { return { d: '1', p: dec.p - keep + 1 }; }
  function roundToFraction(dec, maxF, mode, neg) { return roundDecimal(dec, dec.p + maxF, mode, neg); }
  function roundToSignificant(dec, maxS, mode, neg) { return roundDecimal(dec, maxS, mode, neg); }
  function digitsOf(dec, minInt, minFrac) {
    var d = dec.d, p = dec.p, int, frac;
    if (d === '') { int = ''; frac = ''; }
    else if (p <= 0) { int = ''; frac = repeat('0', -p) + d; }
    else if (p >= d.length) { int = d + repeat('0', p - d.length); frac = ''; }
    else { int = d.substring(0, p); frac = d.substring(p); }
    while (int.length < minInt) int = '0' + int;
    while (frac.length < minFrac) frac += '0';
    return { int: int, frac: frac };
  }
  function shownSignificant(dec) {
    if (dec.d === '') return 1;
    return dec.p > 0 ? Math.max(dec.p, dec.d.length) : dec.d.length;
  }

  var CURRENCIES = {
    USD: ['$', '$', 'US dollar', 'US dollars'], EUR: ['\u20ac', '\u20ac', 'euro', 'euros'],
    GBP: ['\u00a3', '\u00a3', 'British pound', 'British pounds'], JPY: ['\u00a5', '\u00a5', 'Japanese yen', 'Japanese yen'],
    CAD: ['CA$', '$', 'Canadian dollar', 'Canadian dollars'], AUD: ['A$', '$', 'Australian dollar', 'Australian dollars'],
    CNY: ['CN\u00a5', '\u00a5', 'Chinese yuan', 'Chinese yuan'], INR: ['\u20b9', '\u20b9', 'Indian rupee', 'Indian rupees'],
    KRW: ['\u20a9', '\u20a9', 'South Korean won', 'South Korean won'], BRL: ['R$', 'R$', 'Brazilian real', 'Brazilian reals'],
    MXN: ['MX$', '$', 'Mexican peso', 'Mexican pesos'], CHF: ['CHF', 'CHF', 'Swiss franc', 'Swiss francs'],
    NZD: ['NZ$', '$', 'New Zealand dollar', 'New Zealand dollars'], HKD: ['HK$', '$', 'Hong Kong dollar', 'Hong Kong dollars'],
    TWD: ['NT$', '$', 'New Taiwan dollar', 'New Taiwan dollars'], ILS: ['\u20aa', '\u20aa', 'Israeli new shekel', 'Israeli new shekels'],
    VND: ['\u20ab', '\u20ab', 'Vietnamese dong', 'Vietnamese dong'], SEK: ['SEK', 'kr', 'Swedish krona', 'Swedish kronor'],
    NOK: ['NOK', 'kr', 'Norwegian krone', 'Norwegian kroner'], DKK: ['DKK', 'kr', 'Danish krone', 'Danish kroner'],
    PLN: ['PLN', 'z\u0142', 'Polish zloty', 'Polish zlotys'], RUB: ['RUB', '\u20bd', 'Russian ruble', 'Russian rubles'],
    TRY: ['TRY', '\u20ba', 'Turkish lira', 'Turkish Lira'], ZAR: ['ZAR', 'R', 'South African rand', 'South African rand'],
    SGD: ['SGD', '$', 'Singapore dollar', 'Singapore dollars']
  };
  var ZERO_DIGIT_CURRENCIES = ['JPY', 'KRW', 'VND', 'CLP', 'ISK', 'UGX', 'XAF', 'XOF', 'PYG'];

  // unit: [short one, short other, narrow, long one, long other, short joins without space]
  var UNITS = {
    acre: ['ac', 'ac', 'ac', 'acre', 'acres'], bit: ['bit', 'bit', 'bit', 'bit', 'bits'],
    byte: ['byte', 'byte', 'B', 'byte', 'bytes'], celsius: ['\u00b0C', '\u00b0C', '\u00b0C', 'degree Celsius', 'degrees Celsius', 1],
    centimeter: ['cm', 'cm', 'cm', 'centimeter', 'centimeters'], day: ['day', 'days', 'd', 'day', 'days'],
    degree: ['deg', 'deg', '\u00b0', 'degree', 'degrees'], fahrenheit: ['\u00b0F', '\u00b0F', '\u00b0', 'degree Fahrenheit', 'degrees Fahrenheit', 1],
    'fluid-ounce': ['fl oz', 'fl oz', 'fl oz', 'fluid ounce', 'fluid ounces'], foot: ['ft', 'ft', '\u2032', 'foot', 'feet'],
    gallon: ['gal', 'gal', 'gal', 'gallon', 'gallons'], gigabit: ['Gb', 'Gb', 'Gb', 'gigabit', 'gigabits'],
    gigabyte: ['GB', 'GB', 'GB', 'gigabyte', 'gigabytes'], gram: ['g', 'g', 'g', 'gram', 'grams'],
    hectare: ['ha', 'ha', 'ha', 'hectare', 'hectares'], hour: ['hr', 'hr', 'h', 'hour', 'hours'],
    inch: ['in', 'in', '\u2033', 'inch', 'inches'], kilobit: ['kb', 'kb', 'kb', 'kilobit', 'kilobits'],
    kilobyte: ['kB', 'kB', 'kB', 'kilobyte', 'kilobytes'], kilogram: ['kg', 'kg', 'kg', 'kilogram', 'kilograms'],
    kilometer: ['km', 'km', 'km', 'kilometer', 'kilometers'], liter: ['L', 'L', 'L', 'liter', 'liters'],
    megabit: ['Mb', 'Mb', 'Mb', 'megabit', 'megabits'], megabyte: ['MB', 'MB', 'MB', 'megabyte', 'megabytes'],
    meter: ['m', 'm', 'm', 'meter', 'meters'], microsecond: ['\u03bcs', '\u03bcs', '\u03bcs', 'microsecond', 'microseconds'],
    mile: ['mi', 'mi', 'mi', 'mile', 'miles'], 'mile-scandinavian': ['smi', 'smi', 'smi', 'mile-scandinavian', 'miles-scandinavian'],
    milliliter: ['mL', 'mL', 'mL', 'milliliter', 'milliliters'], millimeter: ['mm', 'mm', 'mm', 'millimeter', 'millimeters'],
    millisecond: ['ms', 'ms', 'ms', 'millisecond', 'milliseconds'], minute: ['min', 'min', 'm', 'minute', 'minutes'],
    month: ['mth', 'mths', 'm', 'month', 'months'], nanosecond: ['ns', 'ns', 'ns', 'nanosecond', 'nanoseconds'],
    ounce: ['oz', 'oz', 'oz', 'ounce', 'ounces'], percent: ['%', '%', '%', 'percent', 'percent', 1],
    petabyte: ['PB', 'PB', 'PB', 'petabyte', 'petabytes'], pound: ['lb', 'lb', 'lb', 'pound', 'pounds'],
    second: ['sec', 'sec', 's', 'second', 'seconds'], stone: ['st', 'st', 'st', 'stone', 'stones'],
    terabit: ['Tb', 'Tb', 'Tb', 'terabit', 'terabits'], terabyte: ['TB', 'TB', 'TB', 'terabyte', 'terabytes'],
    week: ['wk', 'wks', 'w', 'week', 'weeks'], yard: ['yd', 'yd', 'yd', 'yard', 'yards'],
    year: ['yr', 'yrs', 'y', 'year', 'years']
  };
  var PER_UNITS = { 'kilometer-per-hour': 'km/h', 'mile-per-hour': 'mph', 'mile-per-gallon': 'mpg', 'liter-per-kilometer': 'L/km' };
  function validUnit(unit) {
    if (UNITS[unit]) return true;
    var per = unit.split('-per-');
    return per.length === 2 && !!UNITS[per[0]] && !!UNITS[per[1]];
  }
  function unitLabel(unit, display, one) {
    var u = UNITS[unit];
    if (u) {
      if (display === 'long') return { text: one ? u[3] : u[4], tight: false };
      if (display === 'narrow') return { text: u[2], tight: true };
      return { text: one ? u[0] : u[1], tight: !!u[5] };
    }
    var per = unit.split('-per-'), a = UNITS[per[0]], b = UNITS[per[1]];
    if (display === 'long') return { text: (one ? a[3] : a[4]) + ' per ' + b[3], tight: false };
    return { text: PER_UNITS[unit] || (a[0] + '/' + b[0]), tight: display === 'narrow' };
  }

  function setDigitOptions(target, options, defMin, defMax, notation) {
    var minInt = numberOption(options, 'minimumIntegerDigits', 1, 21, 1);
    var mnfd = options.minimumFractionDigits, mxfd = options.maximumFractionDigits;
    var mnsd = options.minimumSignificantDigits, mxsd = options.maximumSignificantDigits;
    target.minimumIntegerDigits = minInt;
    var increment = numberOption(options, 'roundingIncrement', 1, 5000, 1);
    if (increment !== 1) throw new RangeError('roundingIncrement ' + increment + ' is not supported');
    target.roundingIncrement = 1;
    target.roundingMode = option(options, 'roundingMode', 'string',
      ['ceil', 'floor', 'expand', 'trunc', 'halfCeil', 'halfFloor', 'halfExpand', 'halfTrunc', 'halfEven'], 'halfExpand');
    target.roundingPriority = option(options, 'roundingPriority', 'string', ['auto', 'morePrecision', 'lessPrecision'], 'auto');
    target.trailingZeroDisplay = option(options, 'trailingZeroDisplay', 'string', ['auto', 'stripIfInteger'], 'auto');
    var hasSd = mnsd !== undefined || mxsd !== undefined;
    var hasFd = mnfd !== undefined || mxfd !== undefined;
    var needSd = true, needFd = true;
    if (target.roundingPriority === 'auto') {
      needSd = hasSd;
      if (needSd || (!hasFd && notation === 'compact')) needFd = false;
    }
    if (needSd) {
      if (hasSd) {
        target.minimumSignificantDigits = numberOption(options, 'minimumSignificantDigits', 1, 21, 1);
        target.maximumSignificantDigits = numberOption(options, 'maximumSignificantDigits', target.minimumSignificantDigits, 21, 21);
      } else {
        target.minimumSignificantDigits = 1;
        target.maximumSignificantDigits = 21;
      }
    }
    if (needFd) {
      if (hasFd) {
        var mn = numberOption(options, 'minimumFractionDigits', 0, 100, undefined);
        var mx = numberOption(options, 'maximumFractionDigits', 0, 100, undefined);
        if (mn === undefined) mn = Math.min(defMin, mx);
        else if (mx === undefined) mx = Math.max(defMax, mn);
        else if (mn > mx) throw new RangeError('maximumFractionDigits value is out of range.');
        target.minimumFractionDigits = mn;
        target.maximumFractionDigits = mx;
      } else {
        target.minimumFractionDigits = defMin;
        target.maximumFractionDigits = defMax;
      }
    }
    if (!needSd && !needFd) {
      // compact notation: the more precise of 0 fraction digits and 2 significant digits
      target.minimumFractionDigits = 0;
      target.maximumFractionDigits = 0;
      target.minimumSignificantDigits = 1;
      target.maximumSignificantDigits = 2;
      target.roundingType = 'morePrecision';
    } else if (needSd && needFd) {
      target.roundingType = target.roundingPriority;
    } else {
      target.roundingType = needSd ? 'significantDigits' : 'fractionDigits';
    }
  }

  function roundWith(f, dec, neg) {
    var bySig = function () { return roundToSignificant(dec, f.maximumSignificantDigits, f.roundingMode, neg); };
    var byFrac = function () { return roundToFraction(dec, f.maximumFractionDigits, f.roundingMode, neg); };
    var r, useSig;
    switch (f.roundingType) {
      case 'significantDigits': r = bySig(); useSig = true; break;
      case 'fractionDigits': r = byFrac(); useSig = false; break;
      default:
        var s = bySig(), fr = byFrac();
        var sigMag = dec.d === '' ? 0 : dec.p - f.maximumSignificantDigits;
        var fracMag = -f.maximumFractionDigits;
        useSig = f.roundingType === 'morePrecision' ? sigMag <= fracMag : sigMag > fracMag;
        r = useSig ? s : fr;
    }
    var minFrac;
    if (useSig) {
      var fracLen = Math.max(0, r.d.length - r.p);
      minFrac = fracLen + Math.max(0, f.minimumSignificantDigits - shownSignificant(r));
      if (r.d === '') minFrac = Math.max(0, f.minimumSignificantDigits - 1);
    } else {
      minFrac = f.minimumFractionDigits;
    }
    var digits = digitsOf(r, f.minimumIntegerDigits, minFrac);
    if (f.trailingZeroDisplay === 'stripIfInteger' && /^0*$/.test(digits.frac)) digits.frac = '';
    return { dec: r, int: digits.int, frac: digits.frac };
  }

  var COMPACT = [['', ''], ['K', ' thousand'], ['M', ' million'], ['B', ' billion'], ['T', ' trillion']];

  function formatNumberToParts(f, x) {
    var v = mathematicalValue(x), parts = [];
    var neg = !!v.neg, number = [], exponent = null, compactIndex = 0, one = false, zero = true;
    if (v.nan) {
      number.push({ type: 'nan', value: 'NaN' });
      neg = false;
    } else if (v.inf) {
      number.push({ type: 'infinity', value: '\u221e' });
      zero = false;
    } else {
      var dec = v.dec;
      if (f.style === 'percent') dec = dec.d === '' ? dec : { d: dec.d, p: dec.p + 2 };
      var rounded;
      if (f.notation === 'compact') {
        for (;;) {
          var mag = dec.d === '' || dec.p <= 3 ? 0 : Math.min(4, Math.floor((dec.p - 1) / 3));
          mag = Math.max(mag, compactIndex);
          var scaled = dec.d === '' ? dec : { d: dec.d, p: dec.p - mag * 3 };
          rounded = roundWith(f, scaled, neg);
          if (rounded.dec.p > 3 && mag < 4 && rounded.dec.d !== '') { compactIndex = mag + 1; continue; }
          compactIndex = mag;
          break;
        }
      } else if (f.notation === 'scientific' || f.notation === 'engineering') {
        var e = dec.d === '' ? 0 : dec.p - 1;
        if (f.notation === 'engineering') e = Math.floor(e / 3) * 3;
        rounded = roundWith(f, dec.d === '' ? dec : { d: dec.d, p: dec.p - e }, neg);
        if (rounded.dec.d !== '' && rounded.dec.p - 1 > (f.notation === 'engineering' ? 2 : 0)) {
          var bump = f.notation === 'engineering' ? 3 : 1;
          e += bump;
          rounded = roundWith(f, { d: dec.d, p: dec.p - e }, neg);
        }
        exponent = e;
      } else {
        rounded = roundWith(f, dec, neg);
      }
      zero = rounded.dec.d === '';
      one = rounded.int === '1' && rounded.frac === '';
      var groups = groupInteger(rounded.int, f.useGrouping);
      for (var g = 0; g < groups.length; g++) {
        if (g) number.push({ type: 'group', value: ',' });
        number.push({ type: 'integer', value: groups[g] });
      }
      if (rounded.frac) {
        number.push({ type: 'decimal', value: '.' });
        number.push({ type: 'fraction', value: rounded.frac });
      }
      if (exponent !== null) {
        number.push({ type: 'exponentSeparator', value: 'E' });
        if (exponent < 0) number.push({ type: 'exponentMinusSign', value: '-' });
        number.push({ type: 'exponentInteger', value: String(Math.abs(exponent)) });
      }
      if (compactIndex) {
        var c = COMPACT[compactIndex][f.compactDisplay === 'long' ? 1 : 0];
        if (f.compactDisplay === 'long') number.push({ type: 'literal', value: ' ' });
        number.push({ type: 'compact', value: c.replace(/^ /, '') });
      }
    }
    var sign = '';
    switch (f.signDisplay) {
      case 'always': sign = neg ? '-' : '+'; break;
      case 'exceptZero': sign = zero ? '' : neg ? '-' : '+'; break;
      case 'negative': sign = neg && !zero ? '-' : ''; break;
      case 'never': sign = ''; break;
      default: sign = neg ? '-' : '';
    }
    var signPart = sign ? { type: sign === '-' ? 'minusSign' : 'plusSign', value: sign } : null;
    var accounting = f.style === 'currency' && f.currencySign === 'accounting' && sign === '-';
    if (accounting) signPart = null;
    if (accounting) parts.push({ type: 'literal', value: '(' });
    if (f.style === 'currency') {
      var cur = CURRENCIES[f.currency];
      if (f.currencyDisplay === 'name') {
        if (signPart) parts.push(signPart);
        parts.push.apply(parts, number);
        parts.push({ type: 'literal', value: ' ' });
        parts.push({ type: 'currency', value: cur ? cur[one ? 2 : 3] : f.currency });
      } else {
        var symbol = f.currencyDisplay === 'code' || !cur ? f.currency
          : f.currencyDisplay === 'narrowSymbol' ? cur[1] : cur[0];
        if (signPart) parts.push(signPart);
        parts.push({ type: 'currency', value: symbol });
        if (/[A-Za-z]$/.test(symbol)) parts.push({ type: 'literal', value: '\u00a0' });
        parts.push.apply(parts, number);
      }
    } else {
      if (signPart) parts.push(signPart);
      parts.push.apply(parts, number);
      if (f.style === 'percent') parts.push({ type: 'percentSign', value: '%' });
      if (f.style === 'unit') {
        var label = unitLabel(f.unit, f.unitDisplay, one);
        if (!label.tight) parts.push({ type: 'literal', value: ' ' });
        parts.push({ type: 'unit', value: label.text });
      }
    }
    if (accounting) parts.push({ type: 'literal', value: ')' });
    return parts;
  }
  function groupInteger(int, mode) {
    if (mode === false || int.length < 4 || (mode === 'min2' && int.length < 5)) return [int];
    var groups = [], i = int.length;
    while (i > 3) { groups.unshift(int.substring(i - 3, i)); i -= 3; }
    groups.unshift(int.substring(0, i));
    return groups;
  }

  function createNumberFormat(locales, options) {
    options = options_(options);
    var f = { kind: 'NumberFormat', locale: resolveLocale(locales, options), numberingSystem: 'latn' };
    option(options, 'numberingSystem', 'string', null, undefined);
    f.style = option(options, 'style', 'string', ['decimal', 'percent', 'currency', 'unit'], 'decimal');
    var currency = option(options, 'currency', 'string', null, undefined);
    if (currency !== undefined && !/^[A-Za-z]{3}$/.test(currency)) throw new RangeError('Invalid currency code : ' + currency);
    var currencyDisplay = option(options, 'currencyDisplay', 'string', ['code', 'symbol', 'narrowSymbol', 'name'], 'symbol');
    var currencySign = option(options, 'currencySign', 'string', ['standard', 'accounting'], 'standard');
    var unit = option(options, 'unit', 'string', null, undefined);
    if (unit !== undefined && !validUnit(unit)) throw new RangeError('Invalid unit argument for Intl.NumberFormat() \'' + unit + '\'');
    var unitDisplay = option(options, 'unitDisplay', 'string', ['short', 'narrow', 'long'], 'short');
    if (f.style === 'currency') {
      if (currency === undefined) throw new TypeError('Currency code is required with currency style.');
      f.currency = currency.toUpperCase();
      f.currencyDisplay = currencyDisplay;
      f.currencySign = currencySign;
    }
    if (f.style === 'unit') {
      if (unit === undefined) throw new TypeError('Unit is required with unit style.');
      f.unit = unit;
      f.unitDisplay = unitDisplay;
    }
    f.notation = option(options, 'notation', 'string', ['standard', 'scientific', 'engineering', 'compact'], 'standard');
    var cDigits = f.style === 'currency' && ZERO_DIGIT_CURRENCIES.indexOf(f.currency) >= 0 ? 0 : 2;
    var defMin = f.style === 'currency' && f.notation === 'standard' ? cDigits : 0;
    var defMax = f.style === 'currency' && f.notation === 'standard' ? cDigits : f.style === 'percent' ? 0 : 3;
    setDigitOptions(f, options, defMin, defMax, f.notation);
    f.compactDisplay = option(options, 'compactDisplay', 'string', ['short', 'long'], 'short');
    var grouping = options.useGrouping;
    var defGrouping = f.notation === 'compact' ? 'min2' : 'auto';
    if (grouping === undefined || grouping === 'true' || grouping === 'false') f.useGrouping = defGrouping;
    else if (grouping === true) f.useGrouping = 'always';
    else if (!grouping) f.useGrouping = false;
    else f.useGrouping = option(options, 'useGrouping', 'string', ['min2', 'auto', 'always'], defGrouping);
    f.signDisplay = option(options, 'signDisplay', 'string', ['auto', 'never', 'always', 'exceptZero', 'negative'], 'auto');
    return f;
  }
  function numberArgument(x) {
    return typeof x === 'bigint' || typeof x === 'string' ? x : Number(x);
  }

  function NumberFormat(locales, options) {
    var self = this instanceof NumberFormat ? this : Object.create(NumberFormat.prototype);
    STATE.set(self, createNumberFormat(locales, options));
    return self;
  }
  service('NumberFormat', NumberFormat, {
    formatToParts: function formatToParts(x) {
      return formatNumberToParts(slots(this, 'NumberFormat'), numberArgument(x));
    },
    formatRange: function formatRange(a, b) { return join(numberRangeParts(this, a, b)); },
    formatRangeToParts: function formatRangeToParts(a, b) { return numberRangeParts(this, a, b); },
    resolvedOptions: function resolvedOptions() {
      var f = slots(this, 'NumberFormat'), out = {};
      ['locale', 'numberingSystem', 'style', 'currency', 'currencyDisplay', 'currencySign', 'unit', 'unitDisplay',
        'minimumIntegerDigits', 'minimumFractionDigits', 'maximumFractionDigits', 'minimumSignificantDigits',
        'maximumSignificantDigits', 'useGrouping', 'notation', 'compactDisplay', 'signDisplay', 'roundingIncrement',
        'roundingMode', 'roundingPriority', 'trailingZeroDisplay'].forEach(function (k) {
        if (k === 'compactDisplay' && f.notation !== 'compact') return;
        if (f[k] !== undefined) out[k] = f[k];
      });
      if (f.roundingType === 'morePrecision') out.roundingPriority = 'morePrecision';
      return out;
    }
  });
  boundGetter(NumberFormat.prototype, 'format', function (x) {
    return join(formatNumberToParts(slots(this, 'NumberFormat'), numberArgument(x)));
  });
  function numberRangeParts(nf, a, b) {
    var f = slots(nf, 'NumberFormat');
    if (a === undefined || b === undefined) throw new TypeError('start or end is undefined');
    var x = formatNumberToParts(f, numberArgument(a)), y = formatNumberToParts(f, numberArgument(b));
    if (join(x) === join(y)) {
      var approx = x.slice();
      approx.unshift({ type: 'approximatelySign', value: '~', source: 'shared' });
      return approx.map(function (p) { p.source = 'shared'; return p; });
    }
    var sep = f.style === 'currency' || f.style === 'unit' || f.style === 'percent' ? ' \u2013 ' : '\u2013';
    var out = x.map(function (p) { p.source = 'startRange'; return p; });
    out.push({ type: 'literal', value: sep, source: 'shared' });
    return out.concat(y.map(function (p) { p.source = 'endRange'; return p; }));
  }

  // ---------------------------------------------------------------------------
  // PluralRules (English)
  // ---------------------------------------------------------------------------
  function PluralRules(locales, options) {
    if (!(this instanceof PluralRules)) throw new TypeError("Constructor Intl.PluralRules requires 'new'");
    options = options_(options);
    var f = { kind: 'PluralRules', locale: resolveLocale(locales, options) };
    f.type = option(options, 'type', 'string', ['cardinal', 'ordinal'], 'cardinal');
    setDigitOptions(f, options, 0, 3, 'standard');
    f.useGrouping = false;
    f.style = 'decimal';
    f.signDisplay = 'never';
    STATE.set(this, f);
  }
  function pluralCategory(f, n) {
    n = Number(n);
    if (!isFinite(n)) return 'other';
    var text = join(formatNumberToParts(f, Math.abs(n)));
    var dot = text.indexOf('.');
    var int = dot < 0 ? text : text.substring(0, dot);
    var hasFraction = dot >= 0;
    if (f.type === 'ordinal') {
      if (hasFraction) return 'other';
      var i = Number(int), m10 = i % 10, m100 = i % 100;
      if (m10 === 1 && m100 !== 11) return 'one';
      if (m10 === 2 && m100 !== 12) return 'two';
      if (m10 === 3 && m100 !== 13) return 'few';
      return 'other';
    }
    return int === '1' && !hasFraction ? 'one' : 'other';
  }
  service('PluralRules', PluralRules, {
    select: function select(n) { return pluralCategory(slots(this, 'PluralRules'), n); },
    selectRange: function selectRange(a, b) {
      var f = slots(this, 'PluralRules');
      if (a === undefined || b === undefined) throw new TypeError('start or end is undefined');
      return pluralCategory(f, b);
    },
    resolvedOptions: function resolvedOptions() {
      var f = slots(this, 'PluralRules'), out = { locale: f.locale, type: f.type };
      ['minimumIntegerDigits', 'minimumFractionDigits', 'maximumFractionDigits', 'minimumSignificantDigits',
        'maximumSignificantDigits'].forEach(function (k) { if (f[k] !== undefined) out[k] = f[k]; });
      out.pluralCategories = f.type === 'ordinal' ? ['few', 'one', 'two', 'other'] : ['one', 'other'];
      out.roundingIncrement = 1;
      out.roundingMode = f.roundingMode;
      out.roundingPriority = f.roundingPriority;
      out.trailingZeroDisplay = f.trailingZeroDisplay;
      return out;
    }
  });

  // ---------------------------------------------------------------------------
  // DateTimeFormat
  // ---------------------------------------------------------------------------
  var MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September',
    'October', 'November', 'December'];
  var WEEKDAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
  var UTC_NAMES = ['UTC', 'ETC/UTC', 'GMT', 'ETC/GMT', 'UCT', 'ETC/UCT', 'ZULU', 'ETC/ZULU', 'UNIVERSAL',
    'ETC/UNIVERSAL', 'GREENWICH', 'ETC/GREENWICH', 'GMT0', 'ETC/GMT0', 'GMT+0', 'ETC/GMT+0', 'GMT-0', 'ETC/GMT-0'];

  function offsetZoneName(minutes) {
    if (minutes === 0) return 'UTC';
    if (minutes % 60 === 0 && Math.abs(minutes) <= 14 * 60) {
      return 'Etc/GMT' + (minutes > 0 ? '-' : '+') + Math.abs(minutes / 60);
    }
    var a = Math.abs(minutes);
    return (minutes < 0 ? '-' : '+') + pad2(Math.floor(a / 60)) + ':' + pad2(a % 60);
  }
  function resolveTimeZone(tz) {
    if (tz === undefined) return { name: offsetZoneName(-new Date().getTimezoneOffset()), system: true };
    var name = String(tz), upper = name.toUpperCase();
    if (UTC_NAMES.indexOf(upper) >= 0) return { name: 'UTC', offset: 0 };
    var m = /^ETC\/GMT([+-])(\d{1,2})$/.exec(upper);
    if (m && Number(m[2]) <= 14) {
      var hours = Number(m[2]);
      return { name: 'Etc/GMT' + m[1] + hours, offset: (m[1] === '+' ? -60 : 60) * hours };
    }
    m = /^([+-])(\d{2}):?(\d{2})$/.exec(name);
    if (m && Number(m[2]) <= 23 && Number(m[3]) <= 59) {
      var off = (Number(m[2]) * 60 + Number(m[3])) * (m[1] === '-' ? -1 : 1);
      return { name: m[1] + m[2] + ':' + m[3], offset: off };
    }
    if (/^[A-Za-z][A-Za-z0-9_+\-]*(\/[A-Za-z0-9_+\-]+)*$/.test(name)) return { name: name, system: true };
    throw new RangeError('Invalid time zone specified: ' + name);
  }
  function dateFields(t, zone) {
    var off = zone.system ? -new Date(t).getTimezoneOffset() : zone.offset;
    var s = new Date(t + off * 60000);
    return {
      year: s.getUTCFullYear(), month: s.getUTCMonth(), day: s.getUTCDate(), weekday: s.getUTCDay(),
      hour: s.getUTCHours(), minute: s.getUTCMinutes(), second: s.getUTCSeconds(), ms: s.getUTCMilliseconds(),
      offset: off
    };
  }
  function gmtOffset(minutes, long) {
    if (minutes === 0) return 'GMT';
    var a = Math.abs(minutes), h = Math.floor(a / 60), m = a % 60, sign = minutes < 0 ? '-' : '+';
    if (long) return 'GMT' + sign + pad2(h) + ':' + pad2(m);
    return 'GMT' + sign + h + (m ? ':' + pad2(m) : '');
  }
  function zoneLabel(style, zone, offset) {
    var utc = zone.name === 'UTC';
    switch (style) {
      case 'long': case 'longGeneric': return utc ? 'Coordinated Universal Time' : gmtOffset(offset, true);
      case 'shortOffset': return gmtOffset(offset, false);
      case 'longOffset': return gmtOffset(offset, true);
      default: return utc ? 'UTC' : gmtOffset(offset, false);
    }
  }

  var DATE_FIELDS = ['weekday', 'era', 'year', 'month', 'day', 'dayPeriod', 'hour', 'minute', 'second',
    'fractionalSecondDigits', 'timeZoneName'];
  function createDateTimeFormat(locales, options, required, defaults) {
    options = options_(options);
    var f = { kind: 'DateTimeFormat', locale: resolveLocale(locales, options), calendar: 'gregory', numberingSystem: 'latn' };
    var calendar = option(options, 'calendar', 'string', null, undefined);
    if (calendar !== undefined && !/^[a-z0-9]{3,8}(-[a-z0-9]{3,8})*$/i.test(calendar)) throw new RangeError('Invalid calendar : ' + calendar);
    option(options, 'numberingSystem', 'string', null, undefined);
    var hour12 = option(options, 'hour12', 'boolean', null, undefined);
    var hourCycle = option(options, 'hourCycle', 'string', ['h11', 'h12', 'h23', 'h24'], undefined);
    f.zone = resolveTimeZone(options.timeZone);
    f.weekday = option(options, 'weekday', 'string', ['narrow', 'short', 'long'], undefined);
    f.era = option(options, 'era', 'string', ['narrow', 'short', 'long'], undefined);
    f.year = option(options, 'year', 'string', ['2-digit', 'numeric'], undefined);
    f.month = option(options, 'month', 'string', ['2-digit', 'numeric', 'narrow', 'short', 'long'], undefined);
    f.day = option(options, 'day', 'string', ['2-digit', 'numeric'], undefined);
    f.dayPeriod = option(options, 'dayPeriod', 'string', ['narrow', 'short', 'long'], undefined);
    f.hour = option(options, 'hour', 'string', ['2-digit', 'numeric'], undefined);
    f.minute = option(options, 'minute', 'string', ['2-digit', 'numeric'], undefined);
    f.second = option(options, 'second', 'string', ['2-digit', 'numeric'], undefined);
    f.fractionalSecondDigits = numberOption(options, 'fractionalSecondDigits', 1, 3, undefined);
    f.timeZoneName = option(options, 'timeZoneName', 'string',
      ['short', 'long', 'shortOffset', 'longOffset', 'shortGeneric', 'longGeneric'], undefined);
    option(options, 'formatMatcher', 'string', ['basic', 'best fit'], 'best fit');
    f.dateStyle = option(options, 'dateStyle', 'string', ['full', 'long', 'medium', 'short'], undefined);
    f.timeStyle = option(options, 'timeStyle', 'string', ['full', 'long', 'medium', 'short'], undefined);
    if (f.dateStyle || f.timeStyle) {
      for (var i = 0; i < DATE_FIELDS.length; i++) {
        if (f[DATE_FIELDS[i]] !== undefined) {
          throw new TypeError("Can't set option " + DATE_FIELDS[i] + ' when ' + (f.dateStyle ? 'dateStyle' : 'timeStyle') + ' is used');
        }
      }
      if (required === 'date' && !f.dateStyle) throw new TypeError('Invalid option : timeStyle');
      if (required === 'time' && !f.timeStyle) throw new TypeError('Invalid option : dateStyle');
    } else {
      var need = true;
      if ((required === 'date' || required === 'any') && (f.weekday || f.year || f.month || f.day)) need = false;
      if ((required === 'time' || required === 'any') &&
        (f.dayPeriod || f.hour || f.minute || f.second || f.fractionalSecondDigits)) need = false;
      if (need && (defaults === 'date' || defaults === 'all')) f.year = f.month = f.day = 'numeric';
      if (need && (defaults === 'time' || defaults === 'all')) f.hour = f.minute = f.second = 'numeric';
    }
    if (f.hour !== undefined || f.timeStyle !== undefined) {
      f.hourCycle = hour12 !== undefined ? (hour12 ? 'h12' : 'h23') : hourCycle || 'h12';
      f.hour12 = f.hourCycle === 'h11' || f.hourCycle === 'h12';
    }
    return f;
  }

  function dateTokens(f) {
    switch (f.dateStyle) {
      case 'full': return [['weekday', 'long'], ', ', ['month', 'long'], ' ', ['day', 'numeric'], ', ', ['year', 'numeric']];
      case 'long': return [['month', 'long'], ' ', ['day', 'numeric'], ', ', ['year', 'numeric']];
      case 'medium': return [['month', 'short'], ' ', ['day', 'numeric'], ', ', ['year', 'numeric']];
      case 'short': return [['month', 'numeric'], '/', ['day', 'numeric'], '/', ['year', '2-digit']];
    }
    var out = [], m = f.month;
    if (m === 'long' || m === 'short' || m === 'narrow') {
      if (f.weekday) out.push(['weekday', f.weekday], ', ');
      out.push(['month', m]);
      if (f.day) out.push(' ', ['day', f.day]);
      if (f.year) out.push(f.day ? ', ' : ' ', ['year', f.year]);
    } else if (m || f.day || f.year) {
      if (f.weekday) out.push(['weekday', f.weekday], ', ');
      var nums = [];
      if (m) nums.push(['month', m]);
      if (f.day) nums.push(['day', f.day]);
      if (f.year) nums.push(['year', f.year]);
      for (var i = 0; i < nums.length; i++) {
        if (i) out.push('/');
        out.push(nums[i]);
      }
    } else if (f.weekday) {
      out.push(['weekday', f.weekday]);
    }
    if (f.era) out.push(' ', ['era', f.era]);
    return out;
  }
  function timeTokens(f) {
    var out = [];
    if (f.timeStyle) {
      out.push(['hour', 'numeric'], ':', ['minute', '2-digit']);
      if (f.timeStyle !== 'short') out.push(':', ['second', '2-digit']);
      if (f.hour12) out.push('\u202f', ['dayPeriod', 'short']);
      if (f.timeStyle === 'full') out.push(' ', ['timeZoneName', 'long']);
      if (f.timeStyle === 'long') out.push(' ', ['timeZoneName', 'short']);
      return out;
    }
    var hms = [];
    if (f.hour) hms.push(['hour', f.hour]);
    if (f.minute) hms.push(['minute', f.hour || f.second ? '2-digit' : f.minute]);
    if (f.second) hms.push(['second', f.hour || f.minute ? '2-digit' : f.second]);
    for (var i = 0; i < hms.length; i++) {
      if (i) out.push(':');
      out.push(hms[i]);
    }
    if (f.fractionalSecondDigits) {
      if (hms.length) out.push('.');
      out.push(['fractionalSecond', f.fractionalSecondDigits]);
    }
    if (f.hour && f.hour12) out.push('\u202f', ['dayPeriod', f.dayPeriod || 'short']);
    else if (f.dayPeriod && !f.hour) out.push(out.length ? ' ' : '', ['flexibleDayPeriod', f.dayPeriod]);
    if (f.timeZoneName) out.push(out.length ? ' ' : '', ['timeZoneName', f.timeZoneName]);
    return out;
  }
  function dateFieldValue(type, style, x, f) {
    switch (type) {
      case 'weekday': {
        var w = WEEKDAYS[x.weekday];
        return style === 'long' ? w : style === 'short' ? w.substring(0, 3) : w.charAt(0);
      }
      case 'era':
        if (x.year > 0) return style === 'long' ? 'Anno Domini' : style === 'short' ? 'AD' : 'A';
        return style === 'long' ? 'Before Christ' : style === 'short' ? 'BC' : 'B';
      case 'year': {
        var y = f.era && x.year <= 0 ? 1 - x.year : x.year;
        return style === '2-digit' ? pad2(Math.abs(y) % 100) : String(y);
      }
      case 'month': {
        if (style === 'numeric') return String(x.month + 1);
        if (style === '2-digit') return pad2(x.month + 1);
        var mo = MONTHS[x.month];
        return style === 'long' ? mo : style === 'short' ? mo.substring(0, 3) : mo.charAt(0);
      }
      case 'day': return style === '2-digit' ? pad2(x.day) : String(x.day);
      case 'hour': {
        var h = x.hour;
        if (f.hourCycle === 'h12') h = h % 12 || 12;
        else if (f.hourCycle === 'h11') h = h % 12;
        else if (f.hourCycle === 'h24') h = h || 24;
        return style === '2-digit' ? pad2(h) : String(h);
      }
      case 'minute': return style === '2-digit' ? pad2(x.minute) : String(x.minute);
      case 'second': return style === '2-digit' ? pad2(x.second) : String(x.second);
      case 'fractionalSecond': return String(1000 + x.ms).substring(1, 1 + style);
      case 'dayPeriod':
        return style === 'narrow' ? (x.hour < 12 ? 'a' : 'p') : x.hour < 12 ? 'AM' : 'PM';
      case 'flexibleDayPeriod': {
        var hr = x.hour;
        if (hr === 12 && x.minute === 0) return 'noon';
        if (hr >= 6 && hr < 12) return 'in the morning';
        if (hr >= 12 && hr < 18) return 'in the afternoon';
        if (hr >= 18 && hr < 21) return 'in the evening';
        return 'at night';
      }
      case 'timeZoneName': return zoneLabel(style, f.zone, x.offset);
    }
    return '';
  }
  function tokensToParts(tokens, f, t) {
    var x = dateFields(t, f.zone), parts = [];
    for (var i = 0; i < tokens.length; i++) {
      var tok = tokens[i];
      if (typeof tok === 'string') pushPart(parts, 'literal', tok);
      else pushPart(parts, tok[0] === 'flexibleDayPeriod' ? 'dayPeriod' : tok[0], dateFieldValue(tok[0], tok[1], x, f));
    }
    return parts;
  }
  function patternTokens(f) {
    var d = dateTokens(f), t = timeTokens(f);
    if (!d.length) return t;
    if (!t.length) return d;
    return d.concat([f.dateStyle === 'full' || f.dateStyle === 'long' ? ' at ' : ', '], t);
  }
  function timeValue(date) {
    var t = date === undefined ? Date.now() : Number(date);
    if (!isFinite(t)) throw new RangeError('Invalid time value');
    return t;
  }
  function formatDateToParts(f, date) { return tokensToParts(patternTokens(f), f, timeValue(date)); }

  function DateTimeFormat(locales, options) {
    var self = this instanceof DateTimeFormat ? this : Object.create(DateTimeFormat.prototype);
    STATE.set(self, createDateTimeFormat(locales, options, 'any', 'date'));
    return self;
  }
  function dateRangeParts(dtf, a, b) {
    var f = slots(dtf, 'DateTimeFormat');
    if (a === undefined || b === undefined) throw new TypeError('startDate or endDate is undefined');
    var x = formatDateToParts(f, a), y = formatDateToParts(f, b);
    if (join(x) === join(y)) return x.map(function (p) { p.source = 'shared'; return p; });
    var out = x.map(function (p) { p.source = 'startRange'; return p; });
    out.push({ type: 'literal', value: ' \u2013 ', source: 'shared' });
    return out.concat(y.map(function (p) { p.source = 'endRange'; return p; }));
  }
  service('DateTimeFormat', DateTimeFormat, {
    formatToParts: function formatToParts(date) { return formatDateToParts(slots(this, 'DateTimeFormat'), date); },
    formatRange: function formatRange(a, b) { return join(dateRangeParts(this, a, b)); },
    formatRangeToParts: function formatRangeToParts(a, b) { return dateRangeParts(this, a, b); },
    resolvedOptions: function resolvedOptions() {
      var f = slots(this, 'DateTimeFormat');
      var out = { locale: f.locale, calendar: f.calendar, numberingSystem: f.numberingSystem, timeZone: f.zone.name };
      if (f.hourCycle !== undefined) { out.hourCycle = f.hourCycle; out.hour12 = f.hour12; }
      if (!f.dateStyle && !f.timeStyle) {
        DATE_FIELDS.forEach(function (k) {
          if (f[k] === undefined) return;
          var v = f[k];
          if ((k === 'minute' || k === 'second') && f.hour !== undefined) v = '2-digit';
          out[k] = v;
        });
      }
      if (f.dateStyle) out.dateStyle = f.dateStyle;
      if (f.timeStyle) out.timeStyle = f.timeStyle;
      return out;
    }
  });
  boundGetter(DateTimeFormat.prototype, 'format', function (date) {
    return join(formatDateToParts(slots(this, 'DateTimeFormat'), date));
  });

  // ---------------------------------------------------------------------------
  // RelativeTimeFormat (English)
  // ---------------------------------------------------------------------------
  var RELATIVE_UNITS = ['second', 'minute', 'hour', 'day', 'week', 'month', 'quarter', 'year'];
  var RELATIVE_SHORT = { second: ['sec.', 'sec.'], minute: ['min.', 'min.'], hour: ['hr.', 'hr.'], day: ['day', 'days'],
    week: ['wk.', 'wk.'], month: ['mo.', 'mo.'], quarter: ['qtr.', 'qtrs.'], year: ['yr.', 'yr.'] };
  var RELATIVE_NARROW = { second: 's', minute: 'm', hour: 'h', day: 'd', week: 'w', month: 'mo', quarter: 'q', year: 'y' };
  var RELATIVE_AUTO = {
    day: { '-1': 'yesterday', '0': 'today', '1': 'tomorrow' },
    second: { '0': 'now' }
  };
  function relativeAuto(unit, n, style) {
    if (RELATIVE_AUTO[unit]) return RELATIVE_AUTO[unit][String(n)];
    if (n !== -1 && n !== 0 && n !== 1) return undefined;
    if (unit === 'minute' || unit === 'hour') return n === 0 ? 'this ' + unit : undefined;
    var name = style === 'long' ? unit : { week: 'wk.', month: 'mo.', quarter: 'qtr.', year: 'yr.' }[unit];
    return (n < 0 ? 'last ' : n > 0 ? 'next ' : 'this ') + name;
  }
  function RelativeTimeFormat(locales, options) {
    if (!(this instanceof RelativeTimeFormat)) throw new TypeError("Constructor Intl.RelativeTimeFormat requires 'new'");
    options = options_(options);
    var f = { kind: 'RelativeTimeFormat', locale: resolveLocale(locales, options), numberingSystem: 'latn' };
    option(options, 'numberingSystem', 'string', null, undefined);
    f.style = option(options, 'style', 'string', ['long', 'short', 'narrow'], 'long');
    f.numeric = option(options, 'numeric', 'string', ['always', 'auto'], 'always');
    f.number = createNumberFormat(undefined, undefined);
    STATE.set(this, f);
  }
  function relativeParts(rtf, value, unit) {
    var f = slots(rtf, 'RelativeTimeFormat');
    value = Number(value);
    if (!isFinite(value)) throw new RangeError('Invalid value ' + value);
    unit = String(unit);
    var singular = unit.replace(/s$/, '');
    if (RELATIVE_UNITS.indexOf(singular) < 0) throw new RangeError('Invalid unit argument for format() \'' + unit + '\'');
    if (f.numeric === 'auto') {
      var word = relativeAuto(singular, value, f.style);
      if (word !== undefined && !(value === 0 && 1 / value < 0 && singular !== 'second' && singular !== 'day')) {
        return [{ type: 'literal', value: word }];
      }
    }
    var past = value < 0 || (value === 0 && 1 / value < 0);
    var numberParts = formatNumberToParts(f.number, Math.abs(value)).map(function (p) { p.unit = singular; return p; });
    var one = join(numberParts) === '1';
    var label;
    if (f.style === 'long') label = ' ' + singular + (one ? '' : 's');
    else if (f.style === 'short') label = ' ' + RELATIVE_SHORT[singular][one ? 0 : 1];
    else label = RELATIVE_NARROW[singular];
    var out = [];
    if (!past) out.push({ type: 'literal', value: 'in ' });
    out = out.concat(numberParts);
    out.push({ type: 'literal', value: label + (past ? ' ago' : '') });
    return out;
  }
  service('RelativeTimeFormat', RelativeTimeFormat, {
    format: function format(value, unit) { return join(relativeParts(this, value, unit)); },
    formatToParts: function formatToParts(value, unit) { return relativeParts(this, value, unit); },
    resolvedOptions: function resolvedOptions() {
      var f = slots(this, 'RelativeTimeFormat');
      return { locale: f.locale, style: f.style, numeric: f.numeric, numberingSystem: f.numberingSystem };
    }
  });

  // ---------------------------------------------------------------------------
  // ListFormat (English)
  // ---------------------------------------------------------------------------
  function ListFormat(locales, options) {
    if (!(this instanceof ListFormat)) throw new TypeError("Constructor Intl.ListFormat requires 'new'");
    options = options_(options);
    var f = { kind: 'ListFormat', locale: resolveLocale(locales, options) };
    f.type = option(options, 'type', 'string', ['conjunction', 'disjunction', 'unit'], 'conjunction');
    f.style = option(options, 'style', 'string', ['long', 'short', 'narrow'], 'long');
    STATE.set(this, f);
  }
  function listParts(lf, list) {
    var f = slots(lf, 'ListFormat'), items = [];
    if (list !== undefined) {
      var it = list[Symbol.iterator]();
      for (var step = it.next(); !step.done; step = it.next()) {
        if (typeof step.value !== 'string') throw new TypeError('Iterable yielded ' + step.value + ' which is not a string');
        items.push(step.value);
      }
    }
    var pair, last, middle = ', ';
    if (f.type === 'disjunction') { pair = ' or '; last = ', or '; }
    else if (f.type === 'unit') {
      if (f.style === 'narrow') { pair = ' '; last = ' '; middle = ' '; } else { pair = ', '; last = ', '; }
    } else if (f.style === 'long') { pair = ' and '; last = ', and '; }
    else if (f.style === 'short') { pair = ' & '; last = ', & '; }
    else { pair = ', '; last = ', '; }
    var out = [];
    for (var i = 0; i < items.length; i++) {
      if (i > 0) {
        var sep = items.length === 2 ? pair : i === items.length - 1 ? last : middle;
        out.push({ type: 'literal', value: sep });
      }
      out.push({ type: 'element', value: items[i] });
    }
    return out;
  }
  service('ListFormat', ListFormat, {
    format: function format(list) { return join(listParts(this, list)); },
    formatToParts: function formatToParts(list) { return listParts(this, list); },
    resolvedOptions: function resolvedOptions() {
      var f = slots(this, 'ListFormat');
      return { locale: f.locale, type: f.type, style: f.style };
    }
  });

  // ---------------------------------------------------------------------------
  // Collator: primary (base letters), secondary (accents), tertiary (case, lower first)
  // levels over NFD text; punctuation < symbols < digits < letters, like the root order.
  // ---------------------------------------------------------------------------
  var MARKS = /[\u0300-\u036f\u1ab0-\u1aff\u1dc0-\u1dff\u20d0-\u20ff\ufe20-\ufe2f]/g;
  var PUNCTUATION = /[\s!-\/:-@\[-`{-~\u00a1-\u00bf\u2000-\u206f\u3000-\u303f]/g;
  function charClass(c) {
    if (c <= 32 || (c >= 0x2000 && c <= 0x200b) || c === 0x3000 || c === 0xa0) return 0;
    if ((c >= 33 && c <= 47) || (c >= 58 && c <= 64) || (c >= 91 && c <= 96) || (c >= 123 && c <= 126) ||
      (c >= 0xa1 && c <= 0xbf) || (c >= 0x2010 && c <= 0x205e)) return 1;
    if (c >= 48 && c <= 57) return 3;
    return 4;
  }
  function comparePrimary(a, b, numeric) {
    var i = 0, j = 0;
    while (i < a.length && j < b.length) {
      var ca = a.charCodeAt(i), cb = b.charCodeAt(j);
      if (numeric && ca >= 48 && ca <= 57 && cb >= 48 && cb <= 57) {
        var ea = i, eb = j;
        while (ea < a.length && a.charCodeAt(ea) >= 48 && a.charCodeAt(ea) <= 57) ea++;
        while (eb < b.length && b.charCodeAt(eb) >= 48 && b.charCodeAt(eb) <= 57) eb++;
        var na = a.substring(i, ea).replace(/^0+(?=\d)/, ''), nb = b.substring(j, eb).replace(/^0+(?=\d)/, '');
        if (na.length !== nb.length) return na.length < nb.length ? -1 : 1;
        if (na !== nb) return na < nb ? -1 : 1;
        i = ea; j = eb;
        continue;
      }
      if (ca !== cb) {
        var ka = charClass(ca), kb = charClass(cb);
        if (ka !== kb) return ka < kb ? -1 : 1;
        return ca < cb ? -1 : 1;
      }
      i++; j++;
    }
    return a.length - i === b.length - j ? 0 : a.length - i < b.length - j ? -1 : 1;
  }
  function compareCase(a, b, upperFirst) {
    for (var i = 0; i < Math.min(a.length, b.length); i++) {
      var x = a.charAt(i), y = b.charAt(i);
      if (x === y) continue;
      var xl = x === x.toLowerCase();
      if (xl === (y === y.toLowerCase())) continue;
      return (xl ? -1 : 1) * (upperFirst ? -1 : 1);
    }
    return 0;
  }
  function collate(c, a, b) {
    a = String(a).normalize('NFD');
    b = String(b).normalize('NFD');
    if (c.ignorePunctuation) { a = a.replace(PUNCTUATION, ''); b = b.replace(PUNCTUATION, ''); }
    var baseA = a.replace(MARKS, ''), baseB = b.replace(MARKS, '');
    var r = comparePrimary(baseA.toLowerCase(), baseB.toLowerCase(), c.numeric);
    if (r || c.sensitivity === 'base') return r;
    if (c.sensitivity !== 'case') {
      var accentA = a.toLowerCase(), accentB = b.toLowerCase();
      if (accentA !== accentB) return accentA < accentB ? -1 : 1;
      if (c.sensitivity === 'accent') return 0;
    }
    return compareCase(baseA, baseB, c.caseFirst === 'upper');
  }
  function createCollator(locales, options) {
    options = options_(options);
    var c = { kind: 'Collator' };
    c.usage = option(options, 'usage', 'string', ['sort', 'search'], 'sort');
    c.locale = resolveLocale(locales, options);
    option(options, 'collation', 'string', null, undefined);
    c.numeric = option(options, 'numeric', 'boolean', null, false);
    c.caseFirst = option(options, 'caseFirst', 'string', ['upper', 'lower', 'false'], 'false');
    c.sensitivity = option(options, 'sensitivity', 'string', ['base', 'accent', 'case', 'variant'], 'variant');
    c.ignorePunctuation = option(options, 'ignorePunctuation', 'boolean', null, false);
    return c;
  }
  function Collator(locales, options) {
    var self = this instanceof Collator ? this : Object.create(Collator.prototype);
    STATE.set(self, createCollator(locales, options));
    return self;
  }
  service('Collator', Collator, {
    resolvedOptions: function resolvedOptions() {
      var c = slots(this, 'Collator');
      return {
        locale: c.locale, usage: c.usage, sensitivity: c.sensitivity, ignorePunctuation: c.ignorePunctuation,
        collation: 'default', numeric: c.numeric, caseFirst: c.caseFirst
      };
    }
  });
  boundGetter(Collator.prototype, 'compare', function (a, b) { return collate(slots(this, 'Collator'), a, b); });

  // ---------------------------------------------------------------------------
  // Segmenter: extended grapheme clusters (combining marks, ZWJ sequences, emoji
  // modifiers, regional indicator pairs), words and sentences
  // ---------------------------------------------------------------------------
  var EXTEND = /^(?:[\u0300-\u036f\u0483-\u0489\u0591-\u05bd\u0610-\u061a\u064b-\u065f\u0900-\u0903\u093a-\u094f\u1ab0-\u1aff\u1dc0-\u1dff\u200c\u20d0-\u20ff\ufe00-\ufe0f\ufe20-\ufe2f]|\udb40[\udc20-\udc7f]|\ud83c[\udffb-\udfff])/;
  var REGIONAL = /^\ud83c[\udde6-\uddff]$/;
  function codePointAtIndex(s, i) {
    var c = s.charCodeAt(i);
    if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
      var d = s.charCodeAt(i + 1);
      if (d >= 0xdc00 && d <= 0xdfff) return s.substring(i, i + 2);
    }
    return s.charAt(i);
  }
  function graphemes(s) {
    var out = [], i = 0;
    while (i < s.length) {
      var start = i, cp = codePointAtIndex(s, i);
      i += cp.length;
      if (cp === '\r' && s.charAt(i) === '\n') i++;
      else if (REGIONAL.test(cp) && REGIONAL.test(codePointAtIndex(s, i))) i += 2;
      for (;;) {
        var rest = s.substring(i, i + 2);
        var m = EXTEND.exec(rest);
        if (m) { i += m[0].length; continue; }
        if (s.charAt(i) === '\u200d' && i + 1 < s.length) { i += 1; i += codePointAtIndex(s, i).length; continue; }
        break;
      }
      out.push([start, s.substring(start, i)]);
    }
    return out;
  }
  var WORD_CHAR = /[0-9A-Za-z_\u00aa\u00b5\u00ba\u00c0-\u00d6\u00d8-\u00f6\u00f8-\u02ff\u0370-\u1fff\u2c00-\u2dff\ua000-\ud7ff\uf900-\ufaff\ufb00-\ufdff\ufe70-\ufeff\uff10-\uff19\uff21-\uff3a\uff41-\uff5a\ud800-\udfff]/;
  var IDEOGRAPH = /[\u3040-\u30ff\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]/;
  function words(s) {
    var clusters = graphemes(s), out = [], i = 0;
    while (i < clusters.length) {
      var start = clusters[i][0], text = clusters[i][1], ch = text.charAt(0);
      i++;
      var wordLike = false;
      if (IDEOGRAPH.test(ch)) {
        wordLike = true;
      } else if (WORD_CHAR.test(ch)) {
        wordLike = true;
        while (i < clusters.length) {
          var next = clusters[i][1].charAt(0);
          if (WORD_CHAR.test(next) && !IDEOGRAPH.test(next)) { text += clusters[i][1]; i++; continue; }
          if ((next === "'" || next === '\u2019' || next === '.' || next === ',') && i + 1 < clusters.length &&
            WORD_CHAR.test(clusters[i + 1][1].charAt(0)) &&
            (next === "'" || next === '\u2019' || /\d/.test(text.slice(-1)))) {
            text += clusters[i][1] + clusters[i + 1][1];
            i += 2;
            continue;
          }
          break;
        }
      } else if (/\s/.test(ch)) {
        while (i < clusters.length && /\s/.test(clusters[i][1]) && clusters[i][1] !== '\n' && ch !== '\n') {
          text += clusters[i][1];
          i++;
        }
      }
      out.push([start, text, wordLike]);
    }
    return out;
  }
  function sentences(s) {
    var out = [], re = /[^.!?\u3002]*(?:[.!?\u3002]+["'\u201d\u2019)\]]*|$)\s*/g, m;
    while ((m = re.exec(s)) && m[0] !== '') out.push([m.index, m[0]]);
    return out;
  }
  function Segmenter(locales, options) {
    if (!(this instanceof Segmenter)) throw new TypeError("Constructor Intl.Segmenter requires 'new'");
    options = options_(options);
    var f = { kind: 'Segmenter', locale: resolveLocale(locales, options) };
    f.granularity = option(options, 'granularity', 'string', ['grapheme', 'word', 'sentence'], 'grapheme');
    STATE.set(this, f);
  }
  function segmentsOf(f, s) {
    var list = f.granularity === 'word' ? words(s) : f.granularity === 'sentence' ? sentences(s) : graphemes(s);
    return list.map(function (seg) {
      var data = { segment: seg[1], index: seg[0], input: s };
      if (f.granularity === 'word') data.isWordLike = seg[2];
      return data;
    });
  }
  function Segments() { throw new TypeError('Illegal constructor'); }
  define(Segments.prototype, 'containing', function containing(index) {
    var s = STATE.get(this);
    var n = index === undefined ? 0 : Math.trunc(Number(index)) || 0;
    if (n < 0 || n >= s.input.length) return undefined;
    var list = s.list();
    for (var i = 0; i < list.length; i++) {
      if (n >= list[i].index && n < list[i].index + list[i].segment.length) return list[i];
    }
    return undefined;
  });
  define(Segments.prototype, Symbol.iterator, function () {
    var list = STATE.get(this).list(), i = 0;
    var it = Object.create(SegmentIterator.prototype);
    STATE.set(it, { next: function () { return i < list.length ? { value: list[i++], done: false } : { value: undefined, done: true }; } });
    return it;
  });
  function SegmentIterator() { throw new TypeError('Illegal constructor'); }
  define(SegmentIterator.prototype, 'next', function next() { return STATE.get(this).next(); });
  define(SegmentIterator.prototype, Symbol.iterator, function () { return this; });
  Object.defineProperty(SegmentIterator.prototype, Symbol.toStringTag, { value: 'Segmenter String Iterator', configurable: true });
  service('Segmenter', Segmenter, {
    segment: function segment(input) {
      var f = slots(this, 'Segmenter'), s = String(input), cache = null;
      var segments = Object.create(Segments.prototype);
      STATE.set(segments, { input: s, list: function () { return cache || (cache = segmentsOf(f, s)); } });
      return segments;
    },
    resolvedOptions: function resolvedOptions() {
      var f = slots(this, 'Segmenter');
      return { locale: f.locale, granularity: f.granularity };
    }
  });

  // ---------------------------------------------------------------------------
  // DisplayNames (a small English table) and Locale
  // ---------------------------------------------------------------------------
  var LANGUAGES = { ar: 'Arabic', bn: 'Bangla', cs: 'Czech', da: 'Danish', de: 'German', el: 'Greek', en: 'English',
    es: 'Spanish', fa: 'Persian', fi: 'Finnish', fr: 'French', he: 'Hebrew', hi: 'Hindi', hu: 'Hungarian',
    id: 'Indonesian', it: 'Italian', ja: 'Japanese', ko: 'Korean', ms: 'Malay', nl: 'Dutch', no: 'Norwegian',
    nb: 'Norwegian Bokm\u00e5l', pl: 'Polish', pt: 'Portuguese', ro: 'Romanian', ru: 'Russian', sv: 'Swedish',
    th: 'Thai', tr: 'Turkish', uk: 'Ukrainian', vi: 'Vietnamese', zh: 'Chinese' };
  var DIALECTS = { 'en-US': 'American English', 'en-GB': 'British English', 'en-AU': 'Australian English',
    'en-CA': 'Canadian English', 'es-MX': 'Mexican Spanish', 'fr-CA': 'Canadian French', 'pt-BR': 'Brazilian Portuguese',
    'pt-PT': 'European Portuguese', 'zh-Hans': 'Simplified Chinese', 'zh-Hant': 'Traditional Chinese' };
  var REGIONS = { AR: 'Argentina', AT: 'Austria', AU: 'Australia', BE: 'Belgium', BR: 'Brazil', CA: 'Canada',
    CH: 'Switzerland', CN: 'China', DE: 'Germany', DK: 'Denmark', EG: 'Egypt', ES: 'Spain', FI: 'Finland',
    FR: 'France', GB: 'United Kingdom', HK: 'Hong Kong SAR China', IE: 'Ireland', IL: 'Israel', IN: 'India',
    IT: 'Italy', JP: 'Japan', KR: 'South Korea', MX: 'Mexico', NL: 'Netherlands', NO: 'Norway', NZ: 'New Zealand',
    PL: 'Poland', PT: 'Portugal', RU: 'Russia', SE: 'Sweden', SG: 'Singapore', TR: 'T\u00fcrkiye', TW: 'Taiwan',
    UA: 'Ukraine', US: 'United States', ZA: 'South Africa', '001': 'world', '150': 'Europe', '419': 'Latin America' };
  var SCRIPTS = { Arab: 'Arabic', Cyrl: 'Cyrillic', Deva: 'Devanagari', Grek: 'Greek', Hans: 'Simplified Han',
    Hant: 'Traditional Han', Hebr: 'Hebrew', Jpan: 'Japanese', Kore: 'Korean', Latn: 'Latin', Thai: 'Thai' };
  var FIELDS = { era: 'era', year: 'year', quarter: 'quarter', month: 'month', weekOfYear: 'week', weekday: 'day of the week',
    day: 'day', dayPeriod: 'AM/PM', hour: 'hour', minute: 'minute', second: 'second', timeZoneName: 'time zone' };
  var CALENDARS = { gregory: 'Gregorian Calendar', iso8601: 'ISO-8601 Calendar', buddhist: 'Buddhist Calendar',
    chinese: 'Chinese Calendar', hebrew: 'Hebrew Calendar', islamic: 'Hijri Calendar', japanese: 'Japanese Calendar' };
  function DisplayNames(locales, options) {
    if (!(this instanceof DisplayNames)) throw new TypeError("Constructor Intl.DisplayNames requires 'new'");
    if (options === undefined) throw new TypeError('Required option type is missing');
    options = options_(options);
    var f = { kind: 'DisplayNames', locale: resolveLocale(locales, options) };
    f.style = option(options, 'style', 'string', ['narrow', 'short', 'long'], 'long');
    f.type = option(options, 'type', 'string', ['language', 'region', 'script', 'currency', 'calendar', 'dateTimeField'], undefined);
    if (f.type === undefined) throw new TypeError('Required option type is missing');
    f.fallback = option(options, 'fallback', 'string', ['code', 'none'], 'code');
    f.languageDisplay = option(options, 'languageDisplay', 'string', ['dialect', 'standard'], 'dialect');
    STATE.set(this, f);
  }
  function displayName(f, code) {
    code = String(code);
    switch (f.type) {
      case 'language': {
        var tag = canonicalTag(code);
        if (f.languageDisplay === 'dialect' && DIALECTS[tag]) return { code: tag, name: DIALECTS[tag] };
        var parts = tag.split('-'), lang = LANGUAGES[parts[0]];
        if (!lang) return { code: tag };
        var extra = [];
        for (var i = 1; i < parts.length; i++) {
          var p = parts[i];
          if (p.length === 4 && SCRIPTS[p]) extra.push(SCRIPTS[p]);
          else if (REGIONS[p]) extra.push(REGIONS[p]);
        }
        return { code: tag, name: extra.length ? lang + ' (' + extra.join(', ') + ')' : lang };
      }
      case 'region': {
        if (!/^([a-z]{2}|\d{3})$/i.test(code)) throw new RangeError('invalid_argument');
        var r = code.toUpperCase();
        return { code: r, name: REGIONS[r] };
      }
      case 'script': {
        if (!/^[a-z]{4}$/i.test(code)) throw new RangeError('invalid_argument');
        var sc = code.charAt(0).toUpperCase() + code.substring(1).toLowerCase();
        return { code: sc, name: SCRIPTS[sc] };
      }
      case 'currency': {
        if (!/^[a-z]{3}$/i.test(code)) throw new RangeError('invalid_argument');
        var cu = code.toUpperCase();
        return { code: cu, name: CURRENCIES[cu] && CURRENCIES[cu][2].replace(/^./, function (c) { return c.toUpperCase(); }) };
      }
      case 'calendar': {
        if (!/^[a-z0-9]{3,8}(-[a-z0-9]{3,8})*$/i.test(code)) throw new RangeError('invalid_argument');
        return { code: code.toLowerCase(), name: CALENDARS[code.toLowerCase()] };
      }
      default: {
        if (!FIELDS.hasOwnProperty(code)) throw new RangeError('invalid_argument');
        return { code: code, name: FIELDS[code] };
      }
    }
  }
  service('DisplayNames', DisplayNames, {
    of: function of(code) {
      var f = slots(this, 'DisplayNames');
      if (code === undefined) throw new TypeError('invalid_argument');
      var r = displayName(f, code);
      return r.name !== undefined ? r.name : f.fallback === 'code' ? r.code : undefined;
    },
    resolvedOptions: function resolvedOptions() {
      var f = slots(this, 'DisplayNames');
      var out = { locale: f.locale, style: f.style, type: f.type, fallback: f.fallback };
      if (f.type === 'language') out.languageDisplay = f.languageDisplay;
      return out;
    }
  });

  var LIKELY = { en: ['Latn', 'US'], es: ['Latn', 'ES'], fr: ['Latn', 'FR'], de: ['Latn', 'DE'], it: ['Latn', 'IT'],
    pt: ['Latn', 'BR'], ja: ['Jpan', 'JP'], zh: ['Hans', 'CN'], ko: ['Kore', 'KR'], ru: ['Cyrl', 'RU'],
    ar: ['Arab', 'EG'], hi: ['Deva', 'IN'], nl: ['Latn', 'NL'], sv: ['Latn', 'SE'], pl: ['Latn', 'PL'],
    tr: ['Latn', 'TR'], he: ['Hebr', 'IL'], uk: ['Cyrl', 'UA'] };
  function Locale(tag, options) {
    if (!(this instanceof Locale)) throw new TypeError("Constructor Intl.Locale requires 'new'");
    if (typeof tag !== 'string' && (typeof tag !== 'object' || tag === null)) {
      throw new TypeError('First argument to Intl.Locale constructor can\'t be empty or missing');
    }
    var canonical = canonicalTag(tag);
    options = options_(options);
    var m = /^([a-z]+)(?:-([A-Z][a-z]{3}))?(?:-([A-Z]{2}|\d{3}))?((?:-[a-z0-9]{5,8}|-\d[a-z0-9]{3})*)(.*)$/.exec(canonical);
    var f = { kind: 'Locale', language: m[1], script: m[2], region: m[3], variants: m[4], extensions: m[5] };
    var lang = option(options, 'language', 'string', null, undefined);
    if (lang !== undefined) {
      if (!/^[a-z]{2,3}$|^[a-z]{5,8}$/i.test(lang)) throw new RangeError('Incorrect locale information provided');
      f.language = lang.toLowerCase();
    }
    var script = option(options, 'script', 'string', null, undefined);
    if (script !== undefined) {
      if (!/^[a-z]{4}$/i.test(script)) throw new RangeError('Incorrect locale information provided');
      f.script = script.charAt(0).toUpperCase() + script.substring(1).toLowerCase();
    }
    var region = option(options, 'region', 'string', null, undefined);
    if (region !== undefined) {
      if (!/^([a-z]{2}|\d{3})$/i.test(region)) throw new RangeError('Incorrect locale information provided');
      f.region = region.toUpperCase();
    }
    var ext = {};
    var u = /-u((?:-[a-z0-9]{2,8})+)/.exec(f.extensions);
    if (u) {
      var keys = u[1].substring(1).split('-'), key = null;
      for (var i = 0; i < keys.length; i++) {
        if (keys[i].length === 2) { key = keys[i]; ext[key] = ''; }
        else if (key) ext[key] = ext[key] ? ext[key] + '-' + keys[i] : keys[i];
      }
    }
    var keyOptions = { calendar: 'ca', collation: 'co', hourCycle: 'hc', caseFirst: 'kf', numeric: 'kn', numberingSystem: 'nu' };
    for (var k in keyOptions) {
      var v = options[k];
      if (v !== undefined) ext[keyOptions[k]] = k === 'numeric' ? String(Boolean(v)) : String(v);
    }
    if (ext.hc !== undefined && ['h11', 'h12', 'h23', 'h24'].indexOf(ext.hc) < 0) throw new RangeError('Incorrect locale information provided');
    f.ext = ext;
    f.baseName = f.language + (f.script ? '-' + f.script : '') + (f.region ? '-' + f.region : '') + f.variants;
    var extKeys = Object.keys(ext).sort();
    f.tag = f.baseName + (extKeys.length ? '-u' + extKeys.map(function (key) {
      var val = ext[key];
      return '-' + key + (val && val !== 'true' ? '-' + val : '');
    }).join('') : '') + f.extensions.replace(/-u(?:-[a-z0-9]{2,8})+/, '');
    STATE.set(this, f);
  }
  function localeGetter(name, read) {
    getter(Locale.prototype, name, function () { return read(slots(this, 'Locale')); });
  }
  localeGetter('baseName', function (f) { return f.baseName; });
  localeGetter('language', function (f) { return f.language; });
  localeGetter('script', function (f) { return f.script; });
  localeGetter('region', function (f) { return f.region; });
  localeGetter('calendar', function (f) { return f.ext.ca; });
  localeGetter('collation', function (f) { return f.ext.co; });
  localeGetter('hourCycle', function (f) { return f.ext.hc; });
  localeGetter('caseFirst', function (f) { return f.ext.kf; });
  localeGetter('numberingSystem', function (f) { return f.ext.nu; });
  localeGetter('numeric', function (f) { return f.ext.kn === '' || f.ext.kn === 'true'; });
  service('Locale', Locale, {
    toString: function toString() { return slots(this, 'Locale').tag; },
    toJSON: function toJSON() { return slots(this, 'Locale').tag; },
    maximize: function maximize() {
      var f = slots(this, 'Locale'), likely = LIKELY[f.language];
      if (!likely) return new Locale(f.tag);
      return new Locale(f.tag, { script: f.script || likely[0], region: f.region || likely[1] });
    },
    minimize: function minimize() {
      var f = slots(this, 'Locale'), likely = LIKELY[f.language];
      if (!likely) return new Locale(f.tag);
      var tag = f.language + (f.script && f.script !== likely[0] ? '-' + f.script : '') +
        (f.region && f.region !== likely[1] ? '-' + f.region : '') + f.variants;
      var rest = f.tag.substring(f.baseName.length);
      return new Locale(tag + rest);
    },
    getCalendars: function getCalendars() { var f = slots(this, 'Locale'); return [f.ext.ca || 'gregory']; },
    getCollations: function getCollations() { slots(this, 'Locale'); return ['emoji', 'eor']; },
    getHourCycles: function getHourCycles() { var f = slots(this, 'Locale'); return [f.ext.hc || 'h12']; },
    getNumberingSystems: function getNumberingSystems() { var f = slots(this, 'Locale'); return [f.ext.nu || 'latn']; },
    getTextInfo: function getTextInfo() {
      var f = slots(this, 'Locale');
      return { direction: ['ar', 'he', 'fa', 'ur'].indexOf(f.language) >= 0 ? 'rtl' : 'ltr' };
    },
    getWeekInfo: function getWeekInfo() {
      slots(this, 'Locale');
      return { firstDay: 7, weekend: [6, 7], minimalDays: 1 };
    }
  });
  delete Locale.supportedLocalesOf;

  define(Intl, 'supportedValuesOf', function supportedValuesOf(key) {
    switch (String(key)) {
      case 'calendar': return ['gregory'];
      case 'collation': return ['default'];
      case 'currency': return Object.keys(CURRENCIES).sort();
      case 'numberingSystem': return ['latn'];
      case 'timeZone': return ['UTC'];
      case 'unit': return Object.keys(UNITS).sort();
      default: throw new RangeError('Invalid key : ' + key);
    }
  });

  // ---------------------------------------------------------------------------
  // Locale-sensitive methods of the built-ins
  // ---------------------------------------------------------------------------
  var dateTime = Date.prototype.getTime;
  function dateLocaleMethod(name, required, defaults) {
    define(Date.prototype, name, {
      [name]: function (locales, options) {
        var t = dateTime.call(this);
        if (isNaN(t)) return 'Invalid Date';
        return join(formatDateToParts(createDateTimeFormat(locales, options, required, defaults), t));
      }
    }[name]);
  }
  dateLocaleMethod('toLocaleString', 'any', 'all');
  dateLocaleMethod('toLocaleDateString', 'date', 'date');
  dateLocaleMethod('toLocaleTimeString', 'time', 'time');

  var numberValue = Number.prototype.valueOf;
  define(Number.prototype, 'toLocaleString', function toLocaleString(locales, options) {
    return join(formatNumberToParts(createNumberFormat(locales, options), numberValue.call(this)));
  });
  if (typeof BigInt === 'function') {
    var bigintValue = BigInt.prototype.valueOf;
    define(BigInt.prototype, 'toLocaleString', function toLocaleString(locales, options) {
      return join(formatNumberToParts(createNumberFormat(locales, options), bigintValue.call(this)));
    });
  }
  var defaultCollator = null;
  define(String.prototype, 'localeCompare', function localeCompare(that, locales, options) {
    if (this === undefined || this === null) throw new TypeError('String.prototype.localeCompare called on null or undefined');
    var c;
    if (locales === undefined && options === undefined) c = defaultCollator || (defaultCollator = createCollator());
    else c = createCollator(locales, options);
    return collate(c, String(this), String(that));
  });
})(this);
