# 前端目录约定

前端由 Vite 构建，`index.html` 只负责加载离线运行时脚本和 `main.jsx`；业务界面按功能放在 `features/`。

## 目录职责

- `main.jsx`：应用壳、顶层状态和视图编排，不在这里新增完整业务页面。
- `features/<name>/`：一个业务功能的页面、子组件和功能内工具。
- `components/`：至少被两个 feature 使用的无业务 UI 组件和图标。
- `hooks/`：跨 feature 的 React hooks。
- `shared/`：无 UI 的常量和纯函数。
- `styles/`：全局样式；功能专属样式优先放回对应 feature。

主依赖方向为 `main → features → components/hooks/shared`，任何模块都不得反向引用 `main.jsx`。
迁移期允许 feature 之间复用已有组件，但必须保持单向、无环；被多个 feature 稳定复用后，应下沉到 `components/`、`hooks/` 或 `shared/`，不要继续扩大跨 feature 耦合。

## 修改与验证

```bash
npm ci
npm run lint:ui
npm run build:ui
npm test
```

浏览器 smoke 测试加载 `dist/`，运行前需先执行 `npm run build:ui`。
