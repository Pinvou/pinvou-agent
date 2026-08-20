// Safari 14.0（macOS 11 初版 WKWebView）基线 polyfill。
// 以经典脚本在每个窗口入口(index/pet/reader)的 tailwind.js 之前同步加载：
// 既覆盖 vendored Tailwind 运行时(postcss 内部用 .at())，也覆盖其后执行的
// 所有 module chunk（打包器只降语法、不补运行时 API）。
// 全部特性检测：现代引擎零开销，直接走原生实现。
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
