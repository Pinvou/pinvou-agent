// Baseline polyfills for Safari 14.0 (WKWebView of the first macOS 11 release).
/* eslint-disable unicorn/no-this-outside-of-class, unicorn/no-useless-undefined, unicorn/prefer-number-properties, no-empty -- polyfill 实现体:this 是原型方法接收者,isFinite/undefined 正是被垫底的 API */
// Loaded synchronously as a classic script before tailwind.js in every window
// entry (index/pet/reader): this covers both the vendored Tailwind runtime
// (postcss uses .at() internally) and every module chunk that runs after it
// (bundlers downlevel syntax, not runtime APIs).
// Everything is feature-detected: zero overhead on modern engines, which keep
// the native implementations.
(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: classic script 直拷产物,严格模式是载荷
  'use strict';

  function define(owner, name, value) {
    if (owner[name] !== undefined) return;
    try {
      Object.defineProperty(owner, name, { value, writable: true, configurable: true });
    } catch {}
  }

  function toInteger(value) {
    const n = Number(value);
    // biome-ignore lint/suspicious/noGlobalIsFinite: 垫片实现体,垫的正是全局 isFinite,见文件头注释
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
    // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 下限,Object.hasOwn 不可用,本调用已是安全形态
    return Object.prototype.hasOwnProperty.call(new Object(object), key);
  });
})();
/* eslint-enable unicorn/no-this-outside-of-class, unicorn/no-useless-undefined, unicorn/prefer-number-properties, no-empty */
