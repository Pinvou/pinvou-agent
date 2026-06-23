# 实施计划：Windows 内置 Poppler 安装

**分支**：`010-bundle-windows-poppler` | **日期**：2026-06-23 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/010-bundle-windows-poppler/spec.md` 的功能规格

## 概要

Windows 安装版需要随 MSI 携带受控 Poppler 运行时，使用户安装 pinvou 后无需手动安装 Poppler 或配置 PATH 即可上传并解析文字层 PDF。计划将 `C:\Users\z27014\Downloads\poppler-26.02.0` 作为受控源内容复制到 `pinvou3-app/src-tauri/resources/windows/poppler/`，通过 Tauri Windows bundle 将资源安装到应用安装目录下的 `poppler` 文件夹，并在 Windows 运行时优先使用该目录解析 `pdftotext`/`pdftoppm`。依赖体检在 Windows 上不再展示 Poppler/PDF 文本提取为用户需补全项，但 Linux 仍保持原有依赖提示。

## 技术上下文

**语言/版本**：Rust（Tauri 后端）、JavaScript/HTML（前端单页 UI）、Tauri 2 配置

**主要依赖**：Tauri 2 bundle/MSI、pinvou3-app Rust OS 分层、现有 `file_ingest` PDF 解析链路、Poppler 26.02.0 Windows 运行时

**存储**：仓库内新增 Windows Poppler 资源目录；安装后释放到应用安装目录下的 `poppler`

**测试**：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib`、`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`、Windows MSI 构建与干净环境安装 smoke、依赖体检 UI 手动验收

**目标平台**：Windows 桌面安装版；Linux 依赖体检和 deb 推荐依赖保持原状

**项目类型**：desktop-app / packaging / file-ingest

**性能目标**：Windows 安装后首次 PDF 上传无需用户额外配置；PDF 文本提取启动延迟不因路径探测引入明显可感知等待

**约束**：不改 DeepSeek-TUI 底座；不重新实现 PDF 解析引擎；Poppler 源目录根部直接包含 `pdftotext.exe` 与依赖 DLL，因此安装目标为 `{安装目录}/poppler`；Windows 依赖体检只隐藏 Poppler/PDF 文本提取项，不影响其他依赖项

**规模/范围**：涉及 `pinvou3-app/src-tauri/tauri.conf.json`、Windows OS 工具解析、`file_ingest` 的 PDF 命令调用/依赖体检、前端依赖体检展示、Windows MSI 验收文档；不涉及 DeepSeek-TUI fork

## 宪章检查

- **中文文档优先**：PASS。计划、研究、数据模型、契约和 quickstart 均使用中文；保留必要英文命令、路径和字段。
- **DeepSeek-TUI 底座优先**：PASS。变更限定在 Tauri app、OS 分层、打包资源与附件解析，不修改 Engine/SSE/Session/Compaction 等底座能力。
- **本地算力与数据边界**：PASS。功能仅处理本地安装包资源和本地 PDF 解析工具，不引入远程模型或外部 API。
- **小步高质量变更**：PASS。按资源引入、打包配置、运行时命令解析、依赖体检展示、验收测试分层推进。
- **可测试性与可验证交付**：PASS。定义 Rust 单测、cargo check、MSI 安装 smoke、干净 Windows PDF 上传验收和依赖体检验收。
- **可维护性与长期演进**：PASS。通过 Spec Kit artifacts 记录 Poppler 来源、安装位置、运行时契约和验证步骤。

**门禁结果**：PASS。无需要豁免的宪章违反项。

## 项目结构

### 文档（本 feature）

```text
specs/010-bundle-windows-poppler/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── windows-poppler-runtime.md
│   └── dependency-check-ui.md
└── tasks.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── src-tauri/
│   ├── resources/
│   │   └── windows/
│   │       └── poppler/
│   ├── src/
│   │   ├── file_ingest.rs
│   │   └── os/
│   │       ├── interface/
│   │       ├── linux/
│   │       └── windows/
│   └── tauri.conf.json
└── src/
    ├── index.html
    └── tauri-bridge.js
```

**结构决策**：Poppler 属于 Windows 安装包运行时资源，放入 `pinvou3-app/src-tauri/resources/windows/poppler/` 可与现有 Tauri app 资源边界一致；命令解析进入 OS 层，`file_ingest` 只消费“可用的 PDF 工具路径/命令”，避免把 Windows 路径策略散落在业务解析逻辑里。

## 复杂度追踪

无。当前设计未违反宪章门禁，不需要复杂度豁免。
