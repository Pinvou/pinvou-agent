# vendor/ — 本地化前端依赖（离线/内网可用）

前端不走构建步骤（`frontendDist: ../src` 直接服务），所有运行时依赖以单文件 JS vendor 在此，
**不依赖任何 CDN** —— GB10 本地/内网环境可直接运行。

| 文件 | 来源 | 用途 |
|---|---|---|
| `react.development.js` | unpkg.com/react@18/umd/react.development.js | React 运行时 |
| `react-dom.development.js` | unpkg.com/react-dom@18/umd/react-dom.development.js | React DOM 渲染 |
| `babel.min.js` | unpkg.com/@babel/standalone/babel.min.js | 浏览器内编译 `<script type="text/babel">` |
| `tailwind.js` | cdn.tailwindcss.com (Play CDN, 自包含 JIT) | 运行时扫描 DOM 生成 Tailwind 样式 |
| `marked.min.js` / `purify.min.js` | （原有）| Markdown 渲染 + 消毒 |

## 刷新 / 升级

```bash
cd pinvou3-app/src/vendor
curl -fsSL -o react.development.js     https://unpkg.com/react@18/umd/react.development.js
curl -fsSL -o react-dom.development.js https://unpkg.com/react-dom@18/umd/react-dom.development.js
curl -fsSL -o babel.min.js             https://unpkg.com/@babel/standalone/babel.min.js
curl -fsSL -o tailwind.js              https://cdn.tailwindcss.com
```

## 上线前可做的优化（非必须）

- **React 生产版**：`react.development.js` / `react-dom.development.js`（~1.2MB）可换成
  `react.production.min.js` / `react-dom.production.min.js`（~130KB）减体积、提速，代价是丢开发期警告。
- **去掉浏览器内 Babel**：`babel.min.js`（3MB）每次启动都在浏览器里编译 JSX。若引入构建步骤
  （Tailwind CLI 出静态 CSS + 预编译 JSX），可同时去掉 `babel.min.js` 和 `tailwind.js` 的运行时开销。
  当前刻意保持「无构建」以贴合同事原架构。
