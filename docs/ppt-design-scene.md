# PPT 设计场景

在「设计」中选择「PPT设计」，输入主题或修改要求，也可附上参考材料。该场景使用现有的本地 `pptx` 工具及配套技能；先确认大纲，再生成可编辑的 `.pptx`，通过产物卡交付。

- 桌面端复用已有能力准备流程，缺少组件时显示准备进度；失败时保留输入并提示失败，不静默发送普通聊天。
- Web 或不支持依赖安装的宿主不会调用桌面安装接口，但仍携带强制场景元数据：要求模型在缺少能力时明确提示，不用 HTML 或在线文档代替本地 PPT。
- 生成工具在本地运行；内容规划仍由用户已配置的模型处理，不能据此理解为模型调用也完全离线。首次准备沿用现有依赖安装和权限规则，没有新增下载源或在线服务。
- 场景选择与消息标签分别保存；Tauri、Web 和 Rust 后端使用一致的 `design:ppt` 白名单，旧场景不变。
- 复用现有的 `scene_triggered` 受控场景选择统计，遵循已有埋点开关，不新增对话内容采集。

验证入口：`test:pinvou-mode-state`、`test:scene-capabilities`、`test:pinvou-scene-sidecar`、`test:ui-language` 与构建后的 `test:scene-cards-smoke`。

实现位置：场景路由 `pinvou3-app/src/features/chat/work-scene-routes.js`；场景能力 `pinvou3-app/src/features/chat/scene-capabilities.js`。
