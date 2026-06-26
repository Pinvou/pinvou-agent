# 实施计划：Windows 内置 7z

**分支**：`017-bundle-windows-7z` | **日期**：2026-06-25 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/017-bundle-windows-7z/spec.md` 的功能规格

**说明**：本模板由 `/speckit-plan` 填充。计划必须遵守 `.specify/memory/constitution.md` 中的项目宪章。

## 概要

Windows 用户上传 zip、rar、7z 压缩包时，不应再依赖系统预装 `7z`。本 feature 计划沿用 Poppler/Pandoc/Tesseract 的 Windows 内置资源模式：使用 `C:\Program Files\7-Zip` 作为源目录，裁剪为压缩包解析必需的 `7z.exe`、`7z.dll` 和许可证/说明文件后放入 `pinvou3-app/src-tauri/resources/windows/7zip/`，打包进 MSI，运行时由 OS 层提供 `archive_tool_path()` 和 `archive_tool_exists()`，业务层继续复用现有压缩包预检、解压、递归 ingest 和安全限制。

研究阶段已验证 `C:\Program Files\7-Zip\7z.exe i` 输出包含 `zip`、`7z`、`Rar` 和 `Rar5`，并列出 `Rar1/2/3/5` 解码器。另已将 `7z.exe` 和 `7z.dll` 单独复制到临时目录执行 `7z.exe i`，确认最小运行文件集仍可加载本地 `7z.dll` 并保留 RAR/RAR5 支持。因此实现阶段应裁剪掉 GUI、帮助文档、SFX、卸载器和语言包等非运行必需文件。

## 技术上下文

**语言/版本**：Rust（Tauri 2 后端）、JSON（Tauri bundle 配置）、WiX XML（Windows MSI PATH 片段）、Markdown（Spec Kit 产物）

**主要依赖**：Tauri 2 bundle resources、现有 OS 抽象层、现有 `file_ingest.rs` 压缩包解析流程、裁剪后的 Windows 7-Zip CLI 运行时资源

**存储**：Windows 安装目录下的 `7zip/` 子目录；不新增用户数据、settings、session 或缓存持久化

**测试**：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml archive`、`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`、`7z.exe i` 源包能力验证、Windows MSI 安装后手动上传 zip/7z/rar 样本、无系统级 7z 环境的依赖体检检查

**目标平台**：Windows 桌面为主；Linux 桌面行为保持现状

**项目类型**：desktop-app

**性能目标**：损坏、空包、密码保护和超限压缩包在 5 秒内返回可理解结果；正常小型样本压缩包解析结果与系统级 7z 可用时一致

**约束**：中文文档优先；不改 DeepSeek-TUI 底座；Windows 内置资源不引入远端下载；只打包 7-Zip CLI 必需运行文件和许可证/说明文件；避免业务层直接写平台判断；不扩大压缩包递归范围；不做无关格式化

**规模/范围**：涉及 Windows 资源目录、Tauri bundle 配置、OS interface/windows/linux/unsupported 层、`file_ingest.rs` 中 `7z` 调用路径、依赖体检展示、Windows MSI 打包验证和本 feature 文档

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。本计划、研究、数据模型、契约、quickstart 使用中文，英文仅保留命令、路径和专有名词。
- **DeepSeek-TUI 底座优先**：PASS。需求只触碰 Tauri app 的文件导入和 OS 工具路径，不修改 Engine、ToolRegistry、Session、Hooks 等底座能力。
- **本地算力与数据边界**：PASS。内置 7z 是本地运行时资源，不引入远端下载或外部 API，不改变用户数据生命周期。
- **小步高质量变更**：PASS。沿用现有 Poppler/Pandoc/Tesseract 资源模式和 OS 层抽象；业务层只从硬编码 `7z` 切换为 OS 提供的压缩包工具路径。
- **可测试性与可验证交付**：PASS。定义单测、静态检查、源包能力验证、MSI 安装后手动验收。已验证新源目录支持 RAR。
- **可维护性与长期演进**：PASS。新增 feature 文档和契约；计划要求资源 README/license 记录来源和能力边界。

**门禁结果**：PASS；无宪章违反项。裁剪后的 `7z.exe` + `7z.dll` 已验证支持 zip、7z、RAR/RAR5，满足规格范围。

## 项目结构

### 文档（本 feature）

```text
specs/017-bundle-windows-7z/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── archive-ingest-contract.md
│   ├── dependency-check-ui.md
│   └── windows-7z-runtime.md
└── tasks.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── src-tauri/
│   ├── tauri.conf.json
│   ├── resources/windows/
│   │   ├── 7zip/
│   │   └── 7zip-path.wxs
│   └── src/
│       ├── file_ingest.rs
│       └── os/
│           ├── interface/
│           ├── linux/
│           ├── unsupported.rs
│           └── windows/
└── src/index.html
```

**结构决策**：Windows 运行时资源放入 `resources/windows/7zip/`，与 `poppler/`、`pandoc/`、`tesseract/`、`asr/` 保持一致；压缩包工具路径和依赖检查策略放到 OS 层，保持 `file_ingest.rs` 只表达业务流程。

## 复杂度追踪

> 仅当宪章检查存在需要解释的违反项时填写。

无。
