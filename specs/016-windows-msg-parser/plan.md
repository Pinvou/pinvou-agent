# 实施计划：Windows MSG 邮件解析

**分支**：`016-windows-msg-parser` | **日期**：2026-06-25 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/016-windows-msg-parser/spec.md` 的功能规格

## 概要

本功能解决 Windows 版导入 Outlook `.msg` 邮件时依赖 Linux 专用 `libemail-outlook-message-perl/msgconvert` 的问题。实施路径是在 `pinvou3-app/src-tauri` 中引入 Rust 原生 MSG 解析能力，Windows `.msg` 直接解析为与 `.eml` 近似的可读邮件文本；`.eml` 保持当前 Python 标准库解析行为；Linux 保持现有 `msgconvert` 依赖提示和一键安装链路。

## 技术上下文

**语言/版本**：Rust 1.88，Rust edition 2021；Tauri 2；前端为现有静态 React/JS 页面

**主要依赖**：现有 `pinvou3-tauri`、Tauri 2.11.1、DeepSeek-TUI fork；新增 Windows MSG 解析依赖 `msg_parser` 0.3.x；保留现有 Python 标准库 `.eml` 解析路径

**存储**：不新增持久化存储；解析结果继续写入 `IngestResult.markdown`，临时文件行为仅保留给现有 Linux `.msg` 转换路径

**测试**：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib`；新增 Windows MSG 格式化单测；依赖体检项单测；手动导入真实 `.msg/.eml` 样本验证

**目标平台**：Windows 桌面为主；Linux 桌面回归保持现状

**项目类型**：desktop-app，Tauri 后端附件解析能力

**性能目标**：有效 `.msg` 在 Windows 上 5 秒内返回可读结果；解析过程无命令行弹窗；损坏文件 5 秒内返回明确 warning

**约束**：不得重写 DeepSeek-TUI 底座；Windows 不依赖 Perl、`msgconvert` 或 Linux 包；`.eml` 输出保持兼容；Linux 现有依赖安装能力不退化；文档和任务使用中文

**规模/范围**：涉及 `pinvou3-app/src-tauri/src/file_ingest.rs` 邮件解析分支、依赖体检逻辑、Windows/Linux OS 层策略、`Cargo.toml/Cargo.lock` 依赖、Spec Kit 文档和回归测试

## 宪章检查

*门槛：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：通过。计划、研究、数据模型、契约和 quickstart 均使用中文，保留必要英文 crate/命令名。
- **DeepSeek-TUI 底座优先**：通过。本功能仅修改 Tauri 附件解析能力，不触碰 Engine、ToolRegistry、Session、Commands、MCP、Hooks、Cycle 或 Compaction。
- **本地算力与数据边界**：通过。`.msg` 在本地解析，不引入外部 API，不上传用户邮件内容。
- **小步高质量变更**：通过。改动限定在邮件附件解析和依赖体检，不做无关重构。
- **可测试性与可验证交付**：通过。计划包含单测、依赖体检回归、真实样本手动验证和 Linux 回归。
- **可维护性与长期演进**：通过。将 Windows 原生 MSG 行为和 Linux 兼容策略记录在本 feature 文档和契约中。

**门槛结果**：PASS

## 项目结构

### 文档（本 feature）

```text
specs/016-windows-msg-parser/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── email-ingest-contract.md
│   └── dependency-check-ui.md
└── tasks.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── src-tauri/
│   ├── Cargo.toml
│   ├── Cargo.lock
│   └── src/
│       ├── file_ingest.rs
│       └── os/
│           ├── interface/
│           ├── linux/
│           ├── windows/
│           └── unsupported.rs
└── src/
    └── index.html
```

**结构决策**：沿用现有附件解析入口 `file_ingest.rs`，在邮件分支内拆分 `.eml` 与 `.msg` 路径；平台差异只放在 OS 层或显式平台条件中，避免把 Windows/Linux 依赖策略散落到前端。

## Phase 0：研究产物

见 [research.md](./research.md)。主要决策：

- Windows `.msg` 使用 `msg_parser` 直接解析，不再走 `msgconvert`。
- `.eml` 保持 Python 标准库解析，避免扩大风险面。
- Linux 保留现有 `msgconvert`/`libemail-outlook-message-perl` 路径，降低跨平台回归风险。
- 依赖体检按平台展示：Windows 不展示 Linux 包名，Linux 继续展示可安装包。

## Phase 1：设计产物

- 数据模型：[data-model.md](./data-model.md)
- 邮件导入契约：[contracts/email-ingest-contract.md](./contracts/email-ingest-contract.md)
- 依赖体检契约：[contracts/dependency-check-ui.md](./contracts/dependency-check-ui.md)
- 验证指南：[quickstart.md](./quickstart.md)

## Phase 1 宪章复查

- **中文文档优先**：PASS，所有新增设计产物为中文。
- **DeepSeek-TUI 底座优先**：PASS，不涉及底座能力重写。
- **本地算力与数据边界**：PASS，邮件内容仅在本地解析。
- **小步高质量变更**：PASS，设计限定在邮件解析与依赖体检。
- **可测试性与可验证交付**：PASS，契约和 quickstart 给出可执行验证。
- **可维护性与长期演进**：PASS，平台差异和依赖策略均记录。

**复查结果**：PASS

## 复杂度追踪

无宪章违背项；无需复杂度例外。
