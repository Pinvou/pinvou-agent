# 实施计划：Windows 兼容性问题扫描与修复

**分支**：`004-windows-compat-fixes` | **日期**：2026-06-16 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/004-windows-compat-fixes/spec.md` 的功能规格；补充约束：尽量保持现有代码不变，新增一层负责 OS 代码调度的抽象层，Linux 调用现有实现，Windows 调用重新实现的 Windows 专属代码。

## 概要

本 feature 面向 Windows 桌面迁移。实施策略从“在各业务调用点直接写平台条件分支”调整为“新增 OS 调度抽象层”：业务层只调用统一接口，Linux 实现尽量包住现有逻辑，Windows 实现在抽象层内提供对应行为或明确降级。这样既能减少对现有业务代码的修改，也能让后续 Windows 适配持续收敛到一个可维护边界。

当前工作区已经有一轮先行兼容性修复；后续 `/speckit-tasks` 应优先把这些散落的平台逻辑整理到 OS 抽象层中，而不是继续扩大调用点内的 `cfg` 分支。

## 技术上下文

**语言/版本**：Rust 1.88、Rust 2021、Tauri 2、PowerShell、Markdown 中文文档；DeepSeek-TUI workspace 使用 Rust 2024 / `rust-version = "1.88"`。

**主要依赖**：Tauri 2、DeepSeek-TUI submodule/fork、Windows Shell (`cmd /C start` / `where`)、Windows 用户目录环境变量 (`USERPROFILE`/`HOMEDRIVE`/`HOMEPATH`)、Linux `xdg-open`/`pkexec`/`apt-get`/`which`、系统工具 `nvidia-smi`、`pandoc`、LibreOffice、Poppler、Tesseract、7-Zip、Python。

**存储**：不新增运行时存储；继续使用 `~/.pinvou3`/`PINVOU3_HOME` 抽象路径；扫描记录和设计工件存入 `specs/004-windows-compat-fixes/`。

**测试**：`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`、OS 抽象层单元测试、定向源码扫描、Windows 手动冒烟、Linux 行为回归抽查。

**目标平台**：Windows 桌面为主；Linux 行为保持可用。pinvou3-app 本轮不新增 macOS 专属实现，DeepSeek-TUI 底座已有 macOS 代码保持不动。

**项目类型**：desktop-app / compatibility-fix / platform-abstraction。

**性能目标**：新增 OS 抽象层不得增加用户可感知启动耗时；监控采样仍按需执行；系统命令调度不引入后台轮询。

**约束**：中文文档优先；不重复实现 DeepSeek-TUI 底座能力；尽量不改现有业务逻辑；新增抽象层必须薄、明确、只承载 OS 差异；Linux 现有实现应优先移动/包装而非重写；Windows 实现不得伪装 Linux 能力可用。

**规模/范围**：新增或规划 `pinvou3-app/src-tauri/src/os/`（或等价平台抽象模块）；调用点涉及 `commands.rs`、`monitor.rs`、`bridge/paths.rs`、`super_permission.rs`、`file_ingest.rs`、`updater.rs`；不直接修改 DeepSeek-TUI 底座源码。

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。计划、研究、数据模型、契约、quickstart 和任务说明使用中文，英文仅保留技术名词、命令、路径和类型。
- **DeepSeek-TUI 底座优先**：PASS。OS 抽象层位于 pinvou3-app 桌面适配边界，不重写 Engine、ToolRegistry、Session、SkillRegistry、MCP、Hooks、Cycle、Compaction。
- **本地算力与数据边界**：PASS。不改变本地 vLLM 和用户数据目录边界；只处理系统命令、平台权限和工具探测。
- **小步高质量变更**：PASS。采用薄 OS 调度层减少业务调用点修改；Linux 现有实现优先包装；避免大范围格式化。
- **可测试性与可验证交付**：PASS。OS 抽象层可通过单测和静态扫描验证；运行时用 `cargo check` 和 Windows 冒烟补充。
- **可维护性与长期演进**：PASS。平台差异集中到固定模块，后续新增 Windows 行为有明确入口。

**门禁结果**：PASS，无需复杂度豁免。

## 项目结构

### 文档（本 feature）

```text
specs/004-windows-compat-fixes/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── compatibility-scan.md
├── checklists/
│   └── requirements.md
└── contracts/
    └── os-dispatch-contract.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
└── src-tauri/src/
    ├── os/
    │   ├── interface/
    │   │   ├── dependency.rs
    │   │   ├── mod.rs
    │   │   ├── path.rs
    │   │   ├── permission.rs
    │   │   ├── system.rs
    │   │   └── update.rs
    │   ├── mod.rs
    │   ├── linux/
    │   │   ├── linux_dependency.rs
    │   │   ├── linux_path.rs
    │   │   ├── linux_permission.rs
    │   │   ├── linux_system.rs
    │   │   ├── linux_update.rs
    │   │   └── mod.rs
    │   ├── windows/
    │   │   ├── windows_dependency.rs
    │   │   ├── windows_path.rs
    │   │   ├── windows_permission.rs
    │   │   ├── windows_system.rs
    │   │   ├── windows_update.rs
    │   │   └── mod.rs
    │   └── unsupported.rs
    ├── commands.rs
    ├── file_ingest.rs
    ├── monitor.rs
    ├── super_permission.rs
    ├── bridge/
    │   └── paths.rs
    └── updater.rs

AGENTS.md
.specify/feature.json
```

**结构决策**：新增 `os` 模块作为平台差异唯一入口。调用点只保留业务语义，例如“打开系统目标”“检测命令是否存在”“解析平台家目录/兼容路径”“启用超级权限”“检查更新”；统一接口按业务域拆分到 `os::interface::{system,path,permission,dependency,update}`，具体 OS 分支移入 `os::{linux,windows,unsupported}`，且系统独有实现文件名带系统名前缀。`bridge/paths.rs` 只保留 pinvou3 目录布局组合，不继续承担 Windows/Linux 路径分发。

## 复杂度追踪

| 违反项 | 为什么必要 | 拒绝的更简单替代方案 |
|---|---|---|
| 新增 OS 调度抽象层 | 用户明确要求尽量保持现有代码不变，并把 OS 差异集中调度；长期 Windows 适配需要固定边界 | 在每个调用点直接写 `cfg` 分支会让平台逻辑继续散落，后续难维护 |

## Phase 0：研究输出

研究文件：[research.md](./research.md)

研究目标：

- 确定 OS 调度层边界。
- 确定 Linux 现有实现如何保留或包装。
- 确定 Windows 实现与“不支持”降级的契约。
- 确定家目录、`PINVOU3_HOME` 和 `/tmp` 兼容路径如何收敛到 OS 调度层。
- 确定后续任务如何从散落修复迁移到抽象层。

## Phase 1：设计输出

- 数据模型：[data-model.md](./data-model.md)
- 契约：[contracts/os-dispatch-contract.md](./contracts/os-dispatch-contract.md)
- 快速开始：[quickstart.md](./quickstart.md)
- 扫描记录：[compatibility-scan.md](./compatibility-scan.md)

## Phase 1 后宪章复查

- **中文文档优先**：PASS。
- **DeepSeek-TUI 底座优先**：PASS。
- **本地算力与数据边界**：PASS。
- **小步高质量变更**：PASS。抽象层是为了减少业务层改动，不是重构底座。
- **可测试性与可验证交付**：PASS。契约和 quickstart 明确了静态扫描、编译检查和手动验收。
- **可维护性与长期演进**：PASS。平台差异有集中入口和后续扩展规则。

**复查结果**：PASS。
