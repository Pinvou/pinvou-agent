# vendor/ — 本地化前端依赖（离线/内网可用）

前端使用 Vite 构建，但仍不依赖 CDN。这里仅保留必须在 `tauri-bridge.js` 之前加载的经典脚本；
React / ReactDOM 由 npm 依赖打包进 `dist/assets/`。

| 文件 | 版本 | SHA-256 | 来源 / 许可证 | 用途 |
|---|---|---|---|---|
| `tailwind.js` | 3.4.17 + local patch | `e884d0030114ff1babdb43e9822ea7293c848debd9ce76a96cafc27105c3df6b` | Tailwind CSS Play CDN runtime / MIT | Scans the DOM at runtime to generate Tailwind styles |

**Local patch (2026-08, Safari 14 compatibility)**: the upstream
`cdn.tailwindcss.com` build emits the `inset-*` utilities through the `inset`
shorthand property (supported from Safari 14.1 only; the WKWebView of the
initial macOS 11.0 release does not parse it, which breaks all 52
`fixed inset-0` dialog overlays at once). The patch expands the emission-table
entry `["inset",["inset"]]` into `["inset",["top","right","bottom","left"]]`,
restoring the physical-property output of the Tailwind 3.0 era. The patch must
be reapplied after refreshing the upstream file, and the SHA-256 above refers
to the patched bytes; `tests/compat_audit.test.mjs` pins the emission table
with a contract assertion.

The vendored marked and DOMPurify copies were removed (2026-08, Safari 14
compatibility fix): the React path now uniformly uses the npm dependencies
(`marked@14.1.4` / `dompurify@3.4.14`), transpiled and bundled by Vite for the
`safari14` target; the bridge fallback renderer degrades to plain text via
`escapeHtml` when `window.marked` is unavailable. The repository keeps a
single marked version to avoid vendor-pin vs npm version drift (that drift
once shipped Safari 15.4+ syntax into the bundle and blanked the app on old
systems). `tailwind.js` internally (postcss) uses `.at()`, which
`shared/legacy-polyfills.js` installs before it loads to meet the Safari 14
baseline. The vendor set is intentionally a single file (Tailwind only);
`tests/vendor_asset_integrity.test.js` keeps enforcing that every shipped
`.js` asset stays registered with its exact SHA-256.

完整第三方归因见仓库根目录 `THIRD_PARTY_NOTICES.md`；Apache-2.0 全文随
`src-tauri/resources/common/bundle/dingtalk-skills/dws/LICENSE` 一并分发。

## 刷新 / 升级

```bash
cd pinvou3-app/src/vendor
curl -fsSL -o tailwind.js              https://cdn.tailwindcss.com
```

After refreshing any file, update its version and SHA-256 in the table above in
the same change (and `THIRD_PARTY_NOTICES.md` when the version changes). The
integrity contract in `npm test` (`tests/vendor_asset_integrity.test.js`)
verifies every registered hash and rejects unregistered `.js` assets. These
files are pinned to LF in `.gitattributes`; do not commit bytes rewritten by a
local `core.autocrlf` checkout.

## 上线前可做的优化（非必须）

- **预编译 Tailwind**：当前仍使用离线 runtime 扫描动态 class。后续可单独迁移到静态 CSS，
  但需要先覆盖运行时拼接 class 的页面，避免视觉回归。
