# 实施计划：Windows MSI 安装包构建

**分支**：`002-windows-msi-package` | **日期**：2026-06-15 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/002-windows-msi-package/spec.md` 的功能规格

**说明**：本计划服务于“尽量不改变当前项目代码”的 Windows MSI 打包目标。实现阶段应先验证现有 Tauri 2 打包能力和 Windows 构建环境，再只做必要的配置、脚本、文档或启动兼容调整。

## 概要

本 feature 的核心价值是把当前 pinvou3 桌面应用构建为 Windows `.msi` 安装包，并形成可重复的 Windows 打包与安装验收路径。技术路径优先复用 Tauri 2 官方 Windows installer 能力，在 Windows 构建机上生成 MSI；代码改动默认限制在打包配置、Windows 构建说明、验证记录和极少量启动兼容修正内。DeepSeek-TUI 底座、聊天主链路、session、工具循环、MCP、workflow agent 编排均不作为本 feature 的重写对象。

## 技术上下文

**语言/版本**：Rust 1.88；JavaScript 静态前端；Python 脚本作为既有 workflow/辅助能力；项目使用 Tauri 2。

**主要依赖**：Tauri 2.11.1、tauri-build 2.6.1、`@tauri-apps/cli` 2.x、DeepSeek-TUI path dependency、Windows WebView2、Windows MSI/WiX 打包链路、Node/npm。

**存储**：本 feature 不新增业务存储；需验证安装/卸载不会默认破坏 `~/.pinvou3/` 下 settings、sessions、workspace、artifacts、workflows、bundle/user 数据。

**测试**：Windows 构建前置检查、`npm install`、Tauri build/bundle 命令、`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`、`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib -- --test-threads=1`、MSI 安装/启动/卸载人工 smoke、用户数据保留检查。

**目标平台**：Windows 桌面，优先 x64 Windows 10/11；MSI 产物必须在 Windows 构建环境中生成。

**项目类型**：desktop-app packaging。

**性能目标**：满足前置条件后，工程师 30 分钟内能得到 `.msi` 产物或明确阻塞原因；安装后 30 秒内能启动主窗口或给出可定位失败原因。

**约束**：中文文档优先；不重写 DeepSeek-TUI 底座；不把 Linux-only 的 `.deb`、`apt`、`pkexec` 更新/依赖安装机制伪装成 Windows 可用；MSI 不内置大模型；模型服务由用户配置本机、WSL、远程 GB10 或 OpenAI-compatible endpoint；尽量不修改现有项目代码。

**规模/范围**：涉及 `pinvou3-app/src-tauri/tauri.conf.json` 的打包目标或 Windows 覆盖配置、Windows 构建说明、安装验收记录、可能的环境前置检查脚本；不涉及 DeepSeek-TUI fork 改动、聊天主链路重写、Windows 原生 updater 完整迁移、代码签名、企业分发策略或附件外部工具全量适配。

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。计划、研究、数据模型、契约、quickstart 和后续任务使用中文；英文保留为命令、文件名、API 字段和官方术语。
- **DeepSeek-TUI 底座优先**：PASS。本 feature 只处理桌面打包与 Windows 安装验收，不重写 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle 或 Compaction。
- **本地算力与数据边界**：PASS。MSI 不内置模型；模型 endpoint 仍由用户配置；安装/卸载验收包含用户数据目录保留预期。
- **小步高质量变更**：PASS。优先使用 Tauri 既有打包机制和配置覆盖；只有实际构建阻塞时才做最小代码修正。
- **可测试性与可验证交付**：PASS。计划包含构建命令、产物位置、安装/启动/卸载 smoke、数据保留检查和未生成产物时的阻塞记录。
- **可维护性与长期演进**：PASS。Windows 打包限制、暂不解决项和复现步骤写入 Spec Kit artifacts，后续任务可追踪。

**门禁结果**：PASS。无需要解释的宪章违反项。

## 项目结构

### 文档（本 feature）

```text
specs/002-windows-msi-package/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── windows-msi-contract.md
└── tasks.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── package.json
├── src/
│   └── 静态前端资源
└── src-tauri/
    ├── tauri.conf.json
    ├── Cargo.toml
    ├── icons/
    └── src/
        ├── lib.rs
        ├── updater.rs
        └── bridge/

DeepSeek-TUI/
└── crates/tui/                # path dependency；本 feature 不改底座

docs/
└── Windows迁移与维护接手手册.md
```

**结构决策**：本 feature 是打包交付，不新增业务模块。实现阶段优先在 `pinvou3-app/src-tauri/tauri.conf.json` 或 Windows 专用配置文件中处理 MSI target；如需文档，放在 `specs/002-windows-msi-package/quickstart.md` 或后续 `docs/` 说明。`DeepSeek-TUI/` 只作为依赖存在，不进入改动范围。

## 复杂度追踪

无宪章违反项，当前不需要复杂度豁免。

## 设计后宪章复查

- Phase 0 研究已确认 MSI 生成必须在 Windows 构建环境完成，计划未承诺 Linux/macOS 交叉生成 MSI。
- Phase 1 设计将构建产物、构建环境、安装验收记录和最小变更清单建模，契约可直接驱动任务拆解。
- 计划仍满足“中文文档优先、底座不重写、用户数据边界清晰、小步变更、可验证交付、可维护记录”六项原则。
