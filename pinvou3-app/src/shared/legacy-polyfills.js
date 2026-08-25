// Baseline polyfills for Safari 14.0 (WKWebView of the first macOS 11 release).
// Loaded synchronously as a classic script before tailwind.js in every window
// entry (index/pet/reader): this covers both the vendored Tailwind runtime
// (postcss uses .at() internally) and every module chunk that runs after it
// (bundlers downlevel syntax, not runtime APIs).
// Everything is feature-detected: zero overhead on modern engines, which keep
// the native implementations.
(function () {
  'use strict';

  function define(owner, name, value) {
    if (typeof owner[name] !== 'undefined') return;
    try {
      Object.defineProperty(owner, name, { value: value, writable: true, configurable: true });
    } catch (_) {}
  }

  function toInteger(value) {
    var n = Number(value);
    if (!isFinite(n)) return 0;
    return n < 0 ? Math.ceil(n) : Math.floor(n);
  }

  function relativeIndex(index, length) {
    var i = toInteger(index) + (Number(index) < 0 ? length : 0);
    return i < 0 || i >= length ? -1 : i;
  }

  // Array.prototype.at / String.prototype.at — Safari 15.4+
  define(Array.prototype, 'at', function at(index) {
    var i = relativeIndex(index, this.length >>> 0);
    return i < 0 ? undefined : this[i];
  });
  define(String.prototype, 'at', function at(index) {
    var s = String(this);
    var i = relativeIndex(index, s.length);
    return i < 0 ? undefined : s.charAt(i);
  });

  // Object.hasOwn — Safari 15.4+
  define(Object, 'hasOwn', function hasOwn(object, key) {
    if (object == null) throw new TypeError('Object.hasOwn called on non-object');
    return Object.prototype.hasOwnProperty.call(Object(object), key);
  });
})();
