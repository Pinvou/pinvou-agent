# vendor/ — 本地化前端依赖（离线/内网可用）

前端使用 Vite 构建，但仍不依赖 CDN。这里仅保留必须在 `tauri-bridge.js` 之前加载的经典脚本；
React / ReactDOM 由 npm 依赖打包进 `dist/assets/`。

| 文件 | 版本 | SHA-256 | 来源 / 许可证 | 用途 |
|---|---|---|---|---|
| `tailwind.js` | 3.4.17 | `176e894661aa9cdc9a5cba6c720044cbbf7b8bd80d1c9a142a7c24b1b6c50d15` | Tailwind CSS Play CDN runtime / MIT | 运行时扫描 DOM 生成 Tailwind 样式 |
| `marked.min.js` | 13.0.3 | `5adea7d8ee41a700fccc14bb9d503104f0470cc17a84ad3e167d3f5251eae0da` | marked / MIT | Markdown 渲染 |
| `purify.min.js` | 3.4.14 | `c2f26ea4fc0d88141c9aa430eb515ac86fce59418ceebd85fa475b87a8d6c3e6` | DOMPurify / Apache-2.0 OR MPL-2.0 | HTML 消毒 |

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
