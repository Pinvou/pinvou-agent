# 前端目录边界

- `app/`：主窗口、宠物窗口等应用入口和顶层视图编排。
- `features/`：按业务功能组织页面、状态、接口包装和功能内组件。
- `platform/`：Tauri、浏览器等宿主平台适配；不得包含具体业务页面。
- `components/`、`hooks/`、`shared/`：至少被两个功能复用的公共能力。
- `assets/`、`styles/`、`vendor/`：静态资源、全局样式和离线第三方运行时。

依赖方向为 `app -> features -> platform/shared`。功能模块不得反向引用 `app/`；前端不得通过 `navigator.userAgent` 扩散新的平台分支，新增平台能力应由 Tauri bridge 返回语义化 capability。

`platform/tauri/bridge.js` 暂时保留兼容的全局 `window.TauriBridge` 接口。后续拆分时，每次只迁移一个 feature 的 API 和状态，保持现有 invoke 命令名不变。
