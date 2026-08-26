// Baseline polyfills for Safari 14.0 (WKWebView of the first macOS 11 release).
/* eslint-disable unicorn/no-this-outside-of-class, unicorn/no-useless-undefined, unicorn/prefer-number-properties, no-empty -- polyfill implementation body: this is the prototype-method receiver; isFinite/undefined are exactly the APIs being polyfilled */
// Loaded synchronously as a classic script before tailwind.js in every window
// entry (index/pet/reader): this covers both the vendored Tailwind runtime
// (postcss uses .at() internally) and every module chunk that runs after it
// (bundlers downlevel syntax, not runtime APIs).
// Everything is feature-detected: zero overhead on modern engines, which keep
// the native implementations.
(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  'use strict';

  function define(owner, name, value) {
    if (owner[name] !== undefined) return;
    try {
      Object.defineProperty(owner, name, { value, writable: true, configurable: true });
    } catch {}
  }

  function toInteger(value) {
    const n = Number(value);
    // biome-ignore lint/suspicious/noGlobalIsFinite: shim implementation body; the global isFinite is exactly what is being polyfilled, see the file-header comment
    if (!isFinite(n)) return 0;
    return n < 0 ? Math.ceil(n) : Math.floor(n);
  }

  function relativeIndex(index, length) {
    const i = toInteger(index) + (Number(index) < 0 ? length : 0);
    return i < 0 || i >= length ? -1 : i;
  }

  // Array.prototype.at / String.prototype.at — Safari 15.4+
  define(Array.prototype, 'at', function at(index) {
    const i = relativeIndex(index, this.length >>> 0);
    return i < 0 ? undefined : this[i];
  });
  define(String.prototype, 'at', function at(index) {
    const s = String(this);
    const i = relativeIndex(index, s.length);
    return i < 0 ? undefined : s.charAt(i);
  });

  // Object.hasOwn — Safari 15.4+
  define(Object, 'hasOwn', function hasOwn(object, key) {
    if (object == null) throw new TypeError('Object.hasOwn called on non-object');
    // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor; Object.hasOwn is unavailable and this call is already the safe form
    return Object.prototype.hasOwnProperty.call(new Object(object), key);
  });
})();
/* eslint-enable unicorn/no-this-outside-of-class, unicorn/no-useless-undefined, unicorn/prefer-number-properties, no-empty */
