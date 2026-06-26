# 实施计划：平板语音输入强化

**分支**：`018-tablet-voice-input` | **日期**：2026-06-26 | **规格**：`specs/018-tablet-voice-input/spec.md`

**输入**：来自 `specs/018-tablet-voice-input/spec.md` 的功能规格

## 概要

本功能面向带触摸屏的 Windows 平板、二合一设备和类似平板尺寸窗口，目标是在不破坏现有聊天输入区的前提下，新增更醒目的语音输入入口，并在输入框存在内容时显示更适合触控的发送与清除操作。

实现路径限定在 `pinvou3-app/src/index.html` 的 `ChatView` 输入区交互与样式层：复用现有 `tauri-bridge.js` 中的 `voiceInput` 状态机、`startVoiceInput`、`cancelVoiceInput`、`clearVoiceInput`、`appendVoiceText` 和既有发送流程，不新增语音识别引擎、不修改 DeepSeek-TUI 底座、不改变 Rust/Tauri 命令契约。

## 技术上下文

**语言/版本**：JavaScript/React 单文件 JSX、CSS、Tauri 2 WebView；Rust 仅用于现有命令构建校验。

**主要依赖**：Tauri 2、现有前端 `ChatView`、现有 `tauri-bridge.js` 语音输入桥接；不新增 npm、Cargo 或系统依赖。

**存储**：N/A。本功能只涉及运行时 UI 状态，不新增持久化数据。

**测试**：实现后执行 `cargo check --manifest-path pinvou3-app\src-tauri\Cargo.toml`，并按 `quickstart.md` 执行触屏/平板、桌面回归、语音状态、发送/清除和可访问性 smoke。

**目标平台**：Windows 桌面应用，重点覆盖带触摸屏的 Windows 平板、二合一设备和平板尺寸窗口；桌面键鼠场景保持回归稳定。

**项目类型**：desktop-app，Tauri 前端 UI 调整。

**性能目标**：新增入口和操作按钮不得造成聊天输入区明显卡顿；布局切换应随视口变化即时响应；不增加语音识别处理耗时。

**约束**：必须保持现有输入框、附件、模型选择、工具菜单、小麦克风和发送流程可用；不得重写 DeepSeek-TUI 的 Engine、Session、Commands、Hooks、Compaction 等底座能力；不得引入远端模型或外部 API；不得格式化无关代码。

**规模/范围**：主要修改 `pinvou3-app/src/index.html`；`pinvou3-app/src/tauri-bridge.js` 作为既有语音状态与桥接契约参考，除非实现中发现必要缺口，否则不修改。

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。规格、计划、研究、数据模型、契约、quickstart 均使用中文描述，保留必要英文代码/API 名称。
- **DeepSeek-TUI 底座优先**：PASS。本功能为 Tauri UI 层增强，不触碰 DeepSeek-TUI Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle 或 Compaction。
- **本地算力与数据边界**：PASS。不新增远端模型、外部 API、网络请求或持久化数据；语音识别继续沿用既有本地/已配置路径。
- **小步高质量变更**：PASS。变更限定在聊天输入区 UI 与状态派生，不做无关重构，不格式化原有代码。
- **可测试性与可验证交付**：PASS。定义了静态构建检查、平板触控 smoke、语音流程 smoke、桌面回归和可访问性检查。
- **可维护性与长期演进**：PASS。通过 Spec Kit artifacts 记录设计边界、数据模型、UI 契约和验证路径，后续可按 tasks 独立实施。

**门禁结果**：PASS，无需复杂度豁免。

## 项目结构

### 文档（本 feature）

```text
specs/018-tablet-voice-input/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── tablet-voice-input-ui.md
└── checklists/
    └── requirements.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── src/
│   ├── index.html          # ChatView、输入区、语音按钮、发送/清除按钮样式与交互
│   └── tauri-bridge.js     # 现有语音输入状态机与桥接契约，计划复用
└── src-tauri/
    └── src/
        └── commands.rs     # 现有 Tauri 命令注册，计划不修改
```

**结构决策**：当前语音输入入口、输入框、发送逻辑和附件状态集中在 `pinvou3-app/src/index.html` 的 `ChatView` 中；语音录制与识别桥接已经由 `pinvou3-app/src/tauri-bridge.js` 提供。因此本 feature 以局部 UI 增强为主，保持实现边界清晰，避免将触屏 UI 需求扩散到 Rust 或 DeepSeek-TUI 层。

## Phase 0：研究产物

已生成 `specs/018-tablet-voice-input/research.md`，关键决策如下：

- 新增主语音按钮，不替换现有小麦克风。
- 触屏/平板体验采用设备能力与视口尺寸的保守判断。
- 主语音按钮放在输入框附近的独立触控区。
- 输入框有非空内容时显示发送与清除按钮；空白字符视为空内容。
- 不更换或扩展语音识别引擎。
- 验证以触屏手动 smoke、桌面回归和静态构建检查为主。

## Phase 1：设计产物

已生成以下设计产物：

- `specs/018-tablet-voice-input/data-model.md`：定义 `VoicePrimaryAction`、`ComposerDraft`、`ComposerActions`、`DeviceExperienceMode`、`VoiceFeedbackNotice`。
- `specs/018-tablet-voice-input/contracts/tablet-voice-input-ui.md`：定义桌面/平板入口可见性、语音状态、输入状态、清除行为、发送行为、可访问性和布局验收契约。
- `specs/018-tablet-voice-input/quickstart.md`：定义实现后的构建检查与手动 smoke 验证步骤。
- `AGENTS.md`：当前 Spec Kit 引用已指向 `specs/018-tablet-voice-input/plan.md`。

## Phase 1 复查

- **中文文档优先**：PASS。新增文档均为中文，代码/API 名称保留英文。
- **DeepSeek-TUI 底座优先**：PASS。设计不涉及底座重写或 fork 变更。
- **本地算力与数据边界**：PASS。不引入外部服务或新数据存储。
- **小步高质量变更**：PASS。实施边界仍限定在前端输入区，复用现有桥接状态。
- **可测试性与可验证交付**：PASS。quickstart 覆盖核心用户故事和边界情况。
- **可维护性与长期演进**：PASS。契约与数据模型可直接支撑后续 `tasks.md` 拆分。

**门禁结果**：PASS，无复杂度豁免。

## 复杂度追踪

无。当前设计不包含宪章违背项，不需要复杂度豁免。
