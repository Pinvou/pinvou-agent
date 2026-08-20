# vendor/ — 本地化前端依赖（离线/内网可用）

前端使用 Vite 构建，但仍不依赖 CDN。这里仅保留必须在 `tauri-bridge.js` 之前加载的经典脚本；
React / ReactDOM 由 npm 依赖打包进 `dist/assets/`。

| 文件 | 版本 | SHA-256 | 来源 / 许可证 | 用途 |
|---|---|---|---|---|
| `tailwind.js` | 3.4.17 + 本地补丁 | `e884d0030114ff1babdb43e9822ea7293c848debd9ce76a96cafc27105c3df6b` | Tailwind CSS Play CDN runtime / MIT | 运行时扫描 DOM 生成 Tailwind 样式 |

**本地补丁（2026-08，Safari 14 兼容）**：上游 `cdn.tailwindcss.com` 的构建把
`inset-*` 工具类发射为 `inset` 简写属性（Safari 14.1+ 才支持；macOS 11.0 初版
WKWebView 不识别，52 处 `fixed inset-0` 弹窗遮罩会整体错位）。补丁把发射表的
`["inset",["inset"]]` 展开为 `["inset",["top","right","bottom","left"]]`，
恢复 Tailwind 3.0 时代的物理属性输出。刷新上游版本后必须重打此补丁，
`tests/compat_audit.test.mjs` 有契约断言防止回归。

marked 与 DOMPurify 的 vendored 副本已移除（2026-08，Safari 14 兼容修复）：
React 主路径统一走 npm 依赖（`marked@14.1.4` / `dompurify`），由 Vite 按
`safari14` target 转译打包；bridge 兜底渲染在 `window.marked` 不存在时退回
`escapeHtml` 纯文本。全仓保持单一 marked 版本，避免 vendor 钉版本与 npm
版本漂移（该漂移曾把 Safari 15.4+ 语法带进产物，旧系统白屏）。
`tailwind.js` 内部（postcss）使用 `.at()`，由 `shared/legacy-polyfills.js`
在其加载前补齐 Safari 14 基线。

完整第三方归因见仓库根目录 `THIRD_PARTY_NOTICES.md`；Apache-2.0 全文随
`src-tauri/resources/common/bundle/dingtalk-skills/dws/LICENSE` 一并分发。

## 刷新 / 升级

```bash
cd pinvou3-app/src/vendor
curl -fsSL -o tailwind.js              https://cdn.tailwindcss.com
```

## 上线前可做的优化（非必须）

- **预编译 Tailwind**：当前仍使用离线 runtime 扫描动态 class。后续可单独迁移到静态 CSS，
  但需要先覆盖运行时拼接 class 的页面，避免视觉回归。
