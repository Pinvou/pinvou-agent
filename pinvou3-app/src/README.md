# 前端目录约定

前端由 Vite 构建，`index.html` 只负责加载离线运行时脚本和 `app/main.jsx`；另有 `pet.html`、`reader.html` 两个入口，分别加载 `app/pet-main.jsx`、`app/reader-main.jsx`；业务界面按功能放在 `features/`。

## 目录职责

- `app/main.jsx`：应用壳、顶层状态和视图编排，不在这里新增完整业务页面。
- `features/<name>/`：一个业务功能的页面、子组件和功能内工具。
- `components/`：至少被两个 feature 使用的无业务 UI 组件和图标。
- `hooks/`：跨 feature 的 React hooks。
- `shared/`：无 UI 的常量和纯函数。
- `styles/`：全局样式；功能专属样式优先放回对应 feature。

主依赖方向为 `app/main → features → platform/components/hooks/shared`，任何模块都不得反向引用 `app/main.jsx`。
迁移期允许 feature 之间复用已有组件，但必须保持单向、无环；被多个 feature 稳定复用后，应下沉到 `components/`、`hooks/` 或 `shared/`，不要继续扩大跨 feature 耦合。

## 修改与验证

```bash
npm ci
npm run lint:ui
npm run build:ui
npm test
```

浏览器 smoke 测试加载 `dist/`，运行前需先执行 `npm run build:ui`。

## 测试分层

- `npm test`：运行无需浏览器和外部服务的确定性测试，并校验桌宠资源。
- `npm run test:node`：只运行 Node 测试；`tests/*.test.js` 和
  `tests/*.test.mjs` 会由 Node 测试运行器自动发现，并以固定 4 并发执行，无需再修改测试清单。
- `npm run test:browser-smoke`：运行完整浏览器 smoke 集合；本地需提供
  `CHROME`，并先在 `pinvou3-app/` 和 `remote-control-relay/` 分别执行 `npm ci`。
  CI 在 Ready PR 中按改动选择 smoke，在 Merge Queue 中运行完整集合。
- `npm run test:user-journey`：运行跨前端、Relay 和 MCP 的用户旅程检查。

新增确定性测试统一命名为 `*.test.js` 或 `*.test.mjs`。需要浏览器的测试统一命名为
`*_smoke.js` 或 `*_smoke.mjs`，并登记到 `scripts/select-frontend-smokes.mjs`；平台专属
runtime smoke 保留独立入口。`test_suite_contract.test.mjs` 会阻止未分类测试被静默遗漏。
