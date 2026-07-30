# 设计模式交付说明

更新时间：2026-07-28
目标 PR：`Pinvou/pinvou-agent#68`
状态：准备正式评审

## 当前范围

本 PR 将对话输入区收敛为 `工作`、`设计`、`代码` 三个主入口，并补齐设计产物预览、元素编辑、AI 调整和场景路由的主要体验。

当前正式可用场景：

- 工作：`公文写作`
- 设计：`海报`、`数据可视化`
- 代码：保留入口，不在本 PR 扩展执行链路

`PPT设计` 暂时保留为置灰入口，提示 `PPT 生成能力修复中`。本 PR 不包含 PPT 设计生成路由，也不会触发 `pptx` MCP 安装或 `mcp_pptx_make_pptx` 调用。

## 关键行为

- 新会话展示完整场景入口；进入具体场景后收敛为当前场景标签，减少输入区噪声。
- `产物与代码` 会直接打开最新产物预览，产物列表收敛为顶部 iOS 风格切换菜单。
- 设计预览右上角使用 `编辑模式` / `退出编辑` 胶囊按钮，进入后支持右侧 inspector、全屏预览、缩放控制、元素选中框、尺寸标签和基础样式编辑。
- 全屏预览保留窗口操作栏，并提供独立 AI 输入框；退出再进入全屏后保留 AI 调整状态。
- 海报场景 prompt 要求优先使用贴合主题的真实图片，并在生成前做海报自检。
- 公文写作强制路由到 `government-writing` / `gongwen`，数据可视化强制路由到 `visualizer`。

## 关键文件

- `src/features/chat/ChatView.jsx`：主入口、场景标签、场景路由、产物预览打开逻辑、全屏 AI 输入框。
- `src/features/chat/work-scene-routes.js`：公文写作和数据可视化场景 payload。
- `src/features/chat/visual-poster-scene.js`：视觉海报场景 payload 和真实图片约束。
- `src/features/chat/scene-capabilities.js`：场景所需能力的安装和校验。
- `src/features/artifacts/ArtifactsPanel.jsx`：产物预览、全屏、缩放、产物切换、inspector 容器。
- `src/features/artifacts/DesignInspectorPanel.jsx`：右侧 iOS 风格元素编辑面板。
- `src/features/artifacts/design-runtime.js`：iframe 内元素选中、编辑和覆盖层 runtime。
- `tests/design_mode_entry_smoke.js`：设计模式端到端 smoke，覆盖入口、路由、全屏、缩放、元素编辑和 PPT 置灰。

## PPT 设计后续处理

本机排查发现 `pptx` MCP 在 Windows runtime 下启动失败，原因是 `python-pptx` 依赖未安装，且当前 Windows marketplace 安装逻辑会跳过 Python 依赖安装。因此本 PR 不把 `PPT设计` 作为可用功能。

后续单独 PR 需要完成：

- Windows runtime 预置或可靠安装 `python-pptx`。
- 安装完成后通过 MCP `tools/list` 健康检查确认 `mcp_pptx_make_pptx` 可见。
- 健康检查失败时工具市场显示运行异常，而不是只依据 `installed.json` 显示已安装。
- 健康检查通过后再打开 `PPT设计` 场景入口和对应生成路由。

## 验证

本 PR 至少需要通过：

```powershell
cd pinvou3-app
npm run build:ui
$env:CHROME='C:\Program Files\Google\Chrome\Application\chrome.exe'; npm run test:design-mode-entry-smoke
npm run lint:ui
```

当前设计模式 smoke 期望 `28/28` 通过，并额外确认：

- 设计子场景顺序为 `海报`、`数据可视化`、`PPT设计`。
- `PPT设计` 为 disabled，点击后不会发送消息，也不会切换到 `design:ppt`。
