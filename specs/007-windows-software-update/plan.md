# 实施计划：Windows 软件更新

**分支**：`009-windows-software-update` | **日期**：2026-06-16 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/007-windows-software-update/spec.md` 的功能规格

**说明**：本计划遵守 `.specify/memory/constitution.md` 中的项目宪章。

## 概要

本 feature 将现有“版本与更新”能力从 Linux `.deb` 静态更新源扩展为 Windows OTA 更新闭环。Windows 路径按四阶段执行：查询 H3C OTA 更新信息、下载 zip 更新包、解压并根据包内 `OtaInfo.json` 定位 `MSI`、启动 Windows 安装器后退出当前 pinvou 进程，并在升级后的后续运行中反馈升级结果。

实现边界放在 `pinvou3-app` 的 Tauri 后端、Windows OS 分支和现有前端更新桥接中；不改 DeepSeek-TUI 底座，不改变 Linux `.deb` 更新行为。C# `H3C.Updater` 作为协议参考；实际 Windows 下载 zip 即完整包内容，包含 `OtaInfo.json` 与 `Files/Pinvou3/*.msi`，解析逻辑按该结构定位 MSI。

## 技术上下文

**语言/版本**：Rust 2021（`pinvou3-app/src-tauri/Cargo.toml` 当前 `rust-version = "1.88"`）、JavaScript 单页前端

**主要依赖**：Tauri 2、现有 `updater.rs`、`os/interface/update.rs`、`os/windows/windows_update.rs`、`reqwest` HTTP、`sha2` 校验；Windows OTA 协议、包结构、安装器和反馈记录的数据结构放在 `os/windows/windows_update.rs`；Windows 更新包解析预计需要新增 zip 解压依赖；DeepSeek-TUI 仅作为底座存在，不参与更新实现

**存储**：`~/.pinvou3/updates/` 用于下载、解压和安装文件暂存；新增或扩展待反馈状态文件用于跨进程/跨版本保留升级反馈记录；继续支持 `PINVOU3_HOME` 测试隔离

**测试**：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml updater --lib`、`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_update --lib`、`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`、前端更新面板手动 smoke、Windows 样例包解析验收、安装器启动手动验收

**目标平台**：Windows 桌面为本 feature 主目标；Linux 桌面必须保持既有 `.deb` 更新链路；非 Windows 平台不得出现新 Windows 更新副作用

**项目类型**：desktop-app

**性能目标**：更新查询 5 秒内给出有更新/无更新/失败状态；常规桌面更新包在下载完成后 10 秒内完成包结构解析和 `MSI` 定位；安装器成功启动后 10 秒内退出当前 pinvou 进程；升级后首次运行 30 秒内尝试反馈结果

**约束**：中文文档优先；尽量只改 Windows 更新相关代码；复用现有下载进度和前端更新 UI 状态；不重写 DeepSeek-TUI 底座；更新 API 外发仅限用户明确触发或产品更新场景；解压必须防路径穿越；安装文件必须来自受控更新目录

**规模/范围**：预计涉及 `pinvou3-app/src-tauri/src/updater.rs`、`pinvou3-app/src-tauri/src/os/interface/update.rs`、`pinvou3-app/src-tauri/src/os/windows/windows_update.rs`、`pinvou3-app/src-tauri/src/os/linux/linux_update.rs`（保持兼容）、`pinvou3-app/src-tauri/src/bridge/paths.rs`、`pinvou3-app/src-tauri/Cargo.toml`、`pinvou3-app/src/tauri-bridge.js`、`pinvou3-app/src/index.html` 及本 feature 文档

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。本计划、研究、数据模型、契约和 quickstart 使用中文；仅保留 `MSI`、`zip`、`OtaInfo.json`、API 路径、命令和字段名等必要术语。
- **DeepSeek-TUI 底座优先**：PASS。更新能力属于 Tauri app/Windows wrapper 层，不改 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle 或 Compaction。
- **本地算力与数据边界**：PASS。更新查询和反馈是产品更新场景下明确外发；不引入模型、LLM 或额外隐式外部能力。下载和状态文件落在 `~/.pinvou3/updates/`。
- **小步高质量变更**：PASS。沿用现有 `updater.rs` 命令、下载事件和 OS 分支；Windows 行为集中在 Windows 更新实现和前端状态适配。
- **可测试性与可验证交付**：PASS。计划包含协议解析单测、路径安全单测、包结构解析测试、Linux 兼容检查、前端 smoke 和 Windows 安装器手动验收。
- **可维护性与长期演进**：PASS。研究和契约记录 H3C OTA 字段、样例包结构、状态持久化和反馈重试边界，后续任务可追踪。

**门禁结果**：PASS，无需复杂度豁免。

## 项目结构

### 文档（本 feature）

```text
specs/007-windows-software-update/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── ota-service-contract.md
│   └── update-ui-command-contract.md
└── checklists/
    └── requirements.md
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
        ├── updater.rs
        ├── bridge/
        │   └── paths.rs
        └── os/
            ├── interface/
            │   └── update.rs
            ├── linux/
            │   └── linux_update.rs
            └── windows/
                └── windows_update.rs
```

**结构决策**：`updater.rs` 已经承载 Tauri 更新命令、下载进度事件和前端状态契约，应保持为跨平台薄编排层；`os/windows/windows_update.rs` 是当前 Windows 更新占位点，适合承载 Windows OTA 请求/响应结构、更新包结构、安装器启动、路径校验和反馈记录。保留 Linux `.deb` 实现，新增 Windows 包解析和反馈状态时使用 `cfg` 或平台分支隔离。

## 复杂度追踪

无需填写。当前方案符合既有 app 层职责和宪章边界。

## Phase 0：研究结论

见 [research.md](./research.md)。

## Phase 1：设计产物

- 数据模型：[data-model.md](./data-model.md)
- OTA 服务与包契约：[contracts/ota-service-contract.md](./contracts/ota-service-contract.md)
- 前端/Tauri 命令契约：[contracts/update-ui-command-contract.md](./contracts/update-ui-command-contract.md)
- 验证指南：[quickstart.md](./quickstart.md)

## Phase 1 复查

- **中文文档优先**：PASS。所有新增设计文档为中文。
- **DeepSeek-TUI 底座优先**：PASS。设计产物没有要求修改底座。
- **本地算力与数据边界**：PASS。外发边界限于 OTA 查询、下载和反馈；状态落盘路径明确。
- **小步高质量变更**：PASS。任务可按查询、下载解析、安装启动、反馈、UI 状态和测试分段交付。
- **可测试性与可验证交付**：PASS。契约列出成功、失败、取消、包异常、路径越界和非 Windows 兼容场景。
- **可维护性与长期演进**：PASS。样例包与 C# 参考差异已记录，避免后续实现误判 `OtaInfo.json` 路径。
