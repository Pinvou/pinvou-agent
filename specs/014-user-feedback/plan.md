# 实施计划：我要反馈

**分支**：`015-user-feedback` | **日期**：2026-06-24 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/014-user-feedback/spec.md` 的功能规格

**说明**：本计划遵守 `.specify/memory/constitution.md` 中的项目宪章。

## 概要

在 pinvou app 内提供“我要反馈”入口，用户可提交问题和建议，并可附带图片和短视频。主入口放在现有设置页内的帮助/支持区域，关键错误提示可追加上下文入口。应用端将用户输入、附件和非敏感环境摘要组织为反馈包，由 Tauri 后端复用 H3CLogCollector 的上传方式：目录打包为 `tar.gz`、按 `0x55` 做字节 XOR 生成 `.dbg`、获取校验 token 后以二进制流上传到既有 H3C 接收通道。本 feature 不新建后台管理环境，不修改 DeepSeek-TUI 底座。

## 技术上下文

**语言/版本**：Rust 1.88（Tauri 后端）、JavaScript/HTML（现有单页 UI）

**主要依赖**：Tauri 2.11、现有 `reqwest`/`md5`，新增最小压缩打包依赖用于生成 H3CLogCollector 兼容的 `tar.gz`；DeepSeek-TUI 仅作为既有底座，不参与本功能实现

**存储**：提交前的临时反馈包目录位于 `~/.pinvou3/feedback/pending/<feedback_id>/`；上传完成后保留 `receipt.json` 和必要摘要，清理原始附件副本与 `.dbg` 暂存文件；失败时保留待重试目录

**测试**：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml feedback --lib`、`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`、前端手动 smoke（设置页入口、附件校验、提交成功/失败）、上传契约单测（tar 条目、XOR、checkCode）

**目标平台**：Windows 桌面安装版优先；Linux 桌面保留 UI 可用性，但上传通道以 H3CLogCollector 兼容性验证为准

**项目类型**：desktop-app / feedback-upload / Tauri bridge

**性能目标**：纯文字反馈 5 秒内返回成功或失败；正常网络下图片/短视频反馈 1 分钟内送达既有接收通道；附件校验在选择后即时反馈

**约束**：不自建后台环境；不外发聊天内容、文件正文或无关敏感数据；上传通道属于用户明确指定的外部接收能力；不修改 DeepSeek-TUI Engine/SSE/Session/ToolRegistry 等底座；文档与用户可见文案中文优先

**规模/范围**：涉及 `pinvou3-app/src/index.html` 设置页 UI 与 i18n、`pinvou3-app/src/tauri-bridge.js` bridge 方法、`pinvou3-app/src-tauri/src/commands.rs` 命令注册/参数类型、`pinvou3-app/src-tauri/src/feedback.rs` 新模块、`pinvou3-app/src-tauri/src/bridge/paths.rs` 反馈目录路径、`pinvou3-app/src-tauri/Cargo.toml` 最小依赖、Spec Kit 文档与验收说明

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。本计划、研究、数据模型、契约和 quickstart 使用中文，英文仅保留命令、路径、字段和协议名。
- **DeepSeek-TUI 底座优先**：PASS。本功能是 Tauri UI + Rust wrapper 能力，不重写底座 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle 或 Compaction。
- **本地算力与数据边界**：PASS。不引入远端模型；唯一外发数据是用户主动提交的反馈包，且通过用户指定的既有 H3C 上传通道。
- **小步高质量变更**：PASS。改动限定在 pinvou3-app 和文档；新增 `feedback` 模块隔离上传逻辑，避免散落到聊天或底座逻辑。
- **可测试性与可验证交付**：PASS。定义 Rust 单测、cargo check、前端 smoke 和上传契约验证。
- **可维护性与长期演进**：PASS。上传协议、反馈包结构、路径与限制记录在本 feature artifacts 中；后续若 H3C 通道变更，可从 contracts 定位影响面。

**门禁结果**：PASS。无需要豁免的宪章违反项。

## 项目结构

### 文档（本 feature）

```text
specs/014-user-feedback/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── feedback-tauri-command.md
│   └── h3c-upload-package.md
└── tasks.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── src/
│   ├── index.html
│   └── tauri-bridge.js
└── src-tauri/
    ├── Cargo.toml
    └── src/
        ├── lib.rs
        ├── commands.rs
        ├── feedback.rs
        └── bridge/
            └── paths.rs
```

**结构决策**：反馈入口属于 pinvou app 用户体验，放在 `pinvou3-app/src/index.html` 的 SettingsView 内最贴近现有“设置/依赖体检/更新”组织方式；上传、打包、校验和隐私边界属于 Tauri 后端能力，放入新的 `feedback.rs`，由 `commands.rs` 暴露单一提交命令；路径集中到 `bridge/paths.rs`，延续 `~/.pinvou3` 目录约定。

## 复杂度追踪

无。当前设计未违反宪章门禁，不需要复杂度豁免。

## Phase 0 研究结果

详见 [research.md](./research.md)。关键结论：复用 H3CLogCollector 上传语义而非复用其 C# 项目；反馈包采用目录结构；附件限制在前后端双重校验；设备序列号使用 Windows 优先采集、配置覆盖和明确失败提示的策略。

## Phase 1 设计结果

- 数据模型：[data-model.md](./data-model.md)
- Tauri 命令契约：[contracts/feedback-tauri-command.md](./contracts/feedback-tauri-command.md)
- H3C 上传包契约：[contracts/h3c-upload-package.md](./contracts/h3c-upload-package.md)
- 验收与开发指引：[quickstart.md](./quickstart.md)

## 宪章复查（设计后）

- **中文文档优先**：PASS。新增 artifacts 均为中文。
- **DeepSeek-TUI 底座优先**：PASS。设计不触碰 `DeepSeek-TUI/`。
- **本地算力与数据边界**：PASS。用户主动上传；反馈包白名单字段；不采集聊天正文和用户文件正文。
- **小步高质量变更**：PASS。UI、bridge、命令、上传模块边界清晰。
- **可测试性与可验证交付**：PASS。契约文件给出可单测字段、包结构、上传步骤和错误场景。
- **可维护性与长期演进**：PASS。H3CLogCollector 兼容流程已在 contracts 中固化，后续 tasks 可追踪。

**复查结果**：PASS。可进入 `/speckit-tasks`。
