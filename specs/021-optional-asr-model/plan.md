# 实施计划：ASR 模型可选下载

**分支**：`021-optional-asr-model` | **日期**：2026-07-06 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/021-optional-asr-model/spec.md` 的功能规格
**说明**：本计划遵守 `.specify/memory/constitution.md` 与 `AGENTS.md` 中的项目约束，优先复用 Linux 已有 ASR 按需下载链路，避免重写语音识别能力。

## 概要

本 feature 将 Windows 主安装包中的大体积 ASR 模型改为按需下载，目标是显著降低安装包体积，同时保持语音输入入口、状态检测、下载进度和本地离线识别体验。实施策略是沿用当前 Linux 已有的 `voice_asr_status`、`install_voice_asr`、`voice_asr:progress` 和 `~/.pinvou3/asr/` 模型落点，最小化调整 Windows 平台分支：主包保留小体积 ASR wrapper/backend，移除 `sensevoice-small-q8.gguf`，缺模型时通过同一安装入口下载到用户目录。

## 技术上下文

**语言/版本**：Rust 2021（`pinvou3-app/src-tauri/Cargo.toml` 当前 `rust-version = "1.88"`）；前端为现有 JavaScript/Tauri invoke；不新增 DeepSeek-TUI 依赖

**主要依赖**：Tauri 2、现有 `voice_asr` 模块、`os::{linux,windows}` 平台分支、`reqwest`（已用于 Linux ASR/知识库模型下载）、现有 `tauri::Emitter` 进度事件、现有 `pinvou-asr.exe` / `llama-funasr-sensevoice.exe`

**存储**：ASR 模型下载到 `~/.pinvou3/asr/`（通过 `bridge::paths::pinvou3_home()`）；Windows 主包只保留小体积 runtime 文件，不再持久化大模型到安装目录

**测试**：
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml voice_asr --lib`
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml os::windows --lib`（或覆盖 Windows ASR path/status 的目标测试名）
- `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`
- Windows 手动 smoke：无模型全新环境点击语音入口、触发下载、下载完成后转写、重启后不重复下载
- 打包检查：Windows 主安装包不包含 `sensevoice-small-q8.gguf`，体积较当前基线减少至少 150 MB

**目标平台**：Windows 桌面为主要验收平台；Linux 保持现有按需下载行为不退化

**项目类型**：desktop-app

**性能目标**：
- 无模型时应用启动和非语音功能不被阻断
- 正常网络下模型下载进度至少按 1 MB 或完成事件更新，前端可感知
- 下载完成后无需重启应用即可进入语音可用状态
- Windows 主安装包体积相比含 ASR 模型基线减少至少 150 MB

**约束**：
- 不修改 DeepSeek-TUI Engine、ToolRegistry、Session、MCP client、Hooks、Cycle、Compaction
- 不引入远程语音识别服务；ASR 仍在本地执行
- 不把 pandoc、OCR、poppler 等其它资源纳入本 feature
- 优先复用 Linux 已有 ASR 下载模式；仅补充 Windows 所需的最小平台分支
- 不在错误、日志或诊断信息中暴露用户音频内容

**规模/范围**：
- 涉及 `pinvou3-app/src-tauri/tauri.conf.json`
- 涉及 `pinvou3-app/src-tauri/src/voice_asr.rs`
- 涉及 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs`
- 涉及 `pinvou3-app/src-tauri/src/os/windows/windows_path.rs`
- 可能涉及 `pinvou3-app/src-tauri/resources/windows/asr/README.md`
- 涉及前端现有语音安装弹窗文案/状态展示：`pinvou3-app/src/tauri-bridge.js`
- 不涉及 `DeepSeek-TUI/`

## 宪章检查
*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。计划、研究、数据模型、契约、quickstart 均使用中文；保留必要英文命令/API 名称。
- **DeepSeek-TUI 底座优先**：PASS。本 feature 只调整 Tauri app 的 ASR runtime/model 管理，不触碰 DeepSeek-TUI 底座能力。
- **本地算力与数据边界**：PASS。ASR 保持本地执行；网络仅用于用户确认后的模型获取，模型落在用户本地目录。
- **小步高质量变更**：PASS。复用 Linux 的状态/下载/进度链路，Windows 只补齐可选模型分支和资源打包差异。
- **可测试性与可验证交付**：PASS。计划包含 Rust 单测、cargo check、打包体积检查和 Windows 手动 smoke。
- **可维护性与长期演进**：PASS。通过 Spec Kit artifacts 记录模型状态、下载契约、打包边界和验收方式。

**门禁结果**：PASS，无需复杂度豁免。

## 项目结构

### 文档（本 feature）
```text
specs/021-optional-asr-model/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── asr-runtime-contract.md
└── checklists/
    └── requirements.md
```

### 源码（仓库根目录）
```text
pinvou3-app/
├── src/
│   └── tauri-bridge.js
└── src-tauri/
    ├── tauri.conf.json
    ├── resources/windows/asr/
    │   ├── pinvou-asr.exe
    │   ├── llama-funasr-sensevoice.exe
    │   └── models/sensevoice-small-q8.gguf
    └── src/
        ├── voice_asr.rs
        └── os/windows/
            ├── windows_system.rs
            └── windows_path.rs
```

**结构决策**：功能边界放在现有 Tauri app 平台层和 `voice_asr` 编排层。Linux 已有按需下载模式作为参考；Windows 资源打包和路径检测保留在 `os/windows`，避免把平台细节扩散到前端或 DeepSeek-TUI。

## 复杂度追踪
> 仅当宪章检查存在需要解释的违反项时填写。

| 违反项 | 为什么必要 | 拒绝的更简单替代方案 |
|---|---|---|
| 无 | N/A | N/A |

## Phase 0：研究结论

见 [research.md](./research.md)。核心决策：
- 复用 `voice_asr_status` / `install_voice_asr` / `voice_asr:progress` 作为跨平台用户契约。
- Windows 主包保留 wrapper/backend，移除大模型；模型下载到 `~/.pinvou3/asr/`。
- Windows 平台层提供与 Linux 等价的模型下载和状态判断，不新增并行语音链路。

## Phase 1：设计产物

- 数据模型：[data-model.md](./data-model.md)
- 契约：[contracts/asr-runtime-contract.md](./contracts/asr-runtime-contract.md)
- 验证指南：[quickstart.md](./quickstart.md)

## Phase 1 宪章复查

- **中文文档优先**：PASS。新增产物均为中文说明。
- **DeepSeek-TUI 底座优先**：PASS。设计不修改底座或 fork。
- **本地算力与数据边界**：PASS。下载仅获取模型文件，识别仍本地执行。
- **小步高质量变更**：PASS。设计以现有 Linux 代码路径和前端安装弹窗为基准，Windows 补最小平台差异。
- **可测试性与可验证交付**：PASS。契约与 quickstart 覆盖状态、下载、失败和打包体积。
- **可维护性与长期演进**：PASS。模型状态、下载任务和资源边界在本 feature 文档中可追踪。

**复查结果**：PASS。
