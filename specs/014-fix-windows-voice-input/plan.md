# 实施计划：修复 Windows 语音输入

**分支**：`014-fix-windows-voice-input` | **日期**：2026-06-24 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/014-fix-windows-voice-input/spec.md` 的功能规格

## 概要

本 feature 修复 Windows 桌面版 pinvou 的语音输入不可用问题。用户在聊天输入区触发语音输入后，应能完成麦克风授权、录音、识别文本回填，并在权限、设备或识别失败时看到明确提示。

实现路径以诊断优先：先查明当前 Windows 下语音输入入口、Tauri WebView 权限、DeepSeek-TUI voice capture 命令和 pinvou bridge 之间的断点；再做最小修复。优先复用 DeepSeek-TUI 已有 `/voice`、`AppAction::VoiceCapture`、voice capture/transcribe 状态机和 pinvou3 现有 UI/bridge，不重新实现 Engine、Session、Commands 或语音识别底座。

## 技术上下文

**语言/版本**：Rust（pinvou3 Tauri 后端与 DeepSeek-TUI fork）、JavaScript/React-in-HTML（pinvou3 前端）、Tauri 2 桌面运行时。

**主要依赖**：`pinvou3-app/`、Tauri 2、DeepSeek-TUI submodule/fork、当前本地模型/endpoint 配置、Windows WebView2 系统权限与麦克风设备。

**存储**：不新增持久业务数据。仅允许必要的诊断日志、UI 临时状态、会话输入框草稿状态和现有 session/workspace 机制。

**测试**：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml` 中聚焦语音/bridge 相关单测；必要时运行 DeepSeek-TUI voice command 相关测试；Windows 手动 smoke 验证麦克风授权、录音、识别、失败提示和会话切换保护。

**目标平台**：Windows 桌面版为主；Linux/其他平台行为不作为本 feature 的修改目标。

**项目类型**：desktop-app。

**性能目标**：用户触发语音输入后 2 秒内看到录音、权限或失败状态；正常短句识别完成后文本进入当前输入区，不阻塞普通文本输入和发送。

**约束**：中文文档；不重写 DeepSeek-TUI 底座；本地算力与用户配置优先；不引入默认外部云语音服务；不扩大到音频附件转写、长音频转写或说话人分离。

**规模/范围**：涉及 `pinvou3-app/src/index.html`、`pinvou3-app/src/tauri-bridge.js`、`pinvou3-app/src-tauri/src/bridge/*`、`pinvou3-app/src-tauri/src/lib.rs`、Tauri 权限/capabilities 配置，以及必要时 DeepSeek-TUI voice capture 相关小修。

## 宪章检查

- **中文文档优先**：PASS。本计划、研究、数据模型、契约和 quickstart 使用中文；保留必要英文标识符和命令。
- **DeepSeek-TUI 底座优先**：PASS。计划复用现有 `/voice`、voice capture、Commands 和 Session，不重新实现底座能力。
- **本地算力与数据边界**：PASS。不新增默认外部语音服务；若识别依赖现有配置，必须沿用用户显式配置。
- **小步高质量变更**：PASS。先诊断再最小修复，范围限定在 Windows 语音输入入口、权限和桥接链路。
- **可测试性与可验证交付**：PASS。包含单测、命令测试和 Windows 手动 smoke。
- **可维护性与长期演进**：PASS。新增契约记录 UI/状态/诊断边界；保留回滚思路。

**门禁结果**：PASS。

## 项目结构

### 文档（本 feature）

```text
specs/014-fix-windows-voice-input/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── voice-input-ui.md
│   └── voice-diagnostics.md
└── tasks.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── src/
│   ├── index.html
│   └── tauri-bridge.js
└── src-tauri/
    ├── capabilities/
    ├── src/
    │   ├── bridge/
    │   ├── commands.rs
    │   └── lib.rs
    └── tauri.conf.json

DeepSeek-TUI/
└── crates/tui/src/commands/groups/core/voice*.rs
```

**结构决策**：优先在 `pinvou3-app` 修复 Windows 桌面集成和 UI 可见状态；只有确认底座 voice capture 在 Windows 本身存在通用 bug 时，才在 `DeepSeek-TUI` fork 做小补丁并记录原因。

## 复杂度追踪

无宪章违背项。
