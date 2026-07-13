# vendor/ — 本地化前端依赖（离线/内网可用）

前端使用 Vite 构建，但仍不依赖 CDN。这里仅保留必须在 `tauri-bridge.js` 之前加载的经典脚本；
React / ReactDOM 由 npm 依赖打包进 `dist/assets/`。

| 文件 | 来源 | 用途 |
|---|---|---|
| `tailwind.js` | cdn.tailwindcss.com (Play CDN, 自包含 JIT) | 运行时扫描 DOM 生成 Tailwind 样式 |
| `marked.min.js` / `purify.min.js` | （原有）| Markdown 渲染 + 消毒 |

## 刷新 / 升级

```bash
cd pinvou3-app/src/vendor
curl -fsSL -o tailwind.js              https://cdn.tailwindcss.com
```

## 上线前可做的优化（非必须）

- **预编译 Tailwind**：当前仍使用离线 runtime 扫描动态 class。后续可单独迁移到静态 CSS，
  但需要先覆盖运行时拼接 class 的页面，避免视觉回归。
