# 实施计划：Windows 内置 Tesseract OCR

**分支**：`015-bundle-windows-tesseract` | **日期**：2026-06-25 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/015-bundle-windows-tesseract/spec.md` 的功能规格

## 概要

Windows 安装版需要随 MSI 携带受控的 Tesseract OCR 运行时和简体中文、英文语言数据，使用户安装 pinvou 后无需手动安装 Tesseract 或配置 `PATH`，即可在无文字层 PDF 的兜底链路中完成 OCR。计划复用 Poppler/Pandoc 的 Windows 资源打包模式：将受控 Tesseract 运行时导入 `pinvou3-app/src-tauri/resources/windows/tesseract/`，安装后释放到 `{安装目录}/tesseract`，运行时由 OS 层优先解析 `{安装目录}/tesseract/tesseract.exe` 和 `tessdata`，`file_ingest` 只消费 OS 层提供的 OCR 命令与语言数据能力。Windows 依赖体检不再提示用户手动安装 Tesseract；Linux 继续保持现有系统包依赖提示。

## 技术上下文

**语言/版本**：Rust 1.88/Tauri 后端、JavaScript/HTML 前端单页 UI、Tauri 2 配置

**主要依赖**：Tauri 2 bundle/MSI、pinvou3-app Rust OS 分层、现有 `file_ingest` PDF 解析链路、已内置 Poppler、Tesseract OCR Windows runtime、`chi_sim.traineddata`、`eng.traineddata`

**存储**：仓库内新增 Windows Tesseract 资源目录；安装后释放到应用安装目录下的 `tesseract`

**测试**：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib`、`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_path --lib`、`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`、Windows MSI 构建与解包验证、未预装 Tesseract 的 Windows OCR 上传 smoke、依赖体检 UI 手动验收

**目标平台**：Windows 桌面安装版；Linux 依赖安装和体检行为保持现状

**项目类型**：desktop-app / packaging / file-ingest

**性能目标**：3 页以内中英文扫描件 PDF 从上传到 OCR 文本结果不超过 30 秒；OCR 路径探测不引入明显可感知等待

**约束**：不改 DeepSeek-TUI 底座；不重新实现 OCR 引擎；普通图片上传不恢复为 Tesseract 主路径；保留 PDF 文字层优先解析、空结果才 OCR 的顺序；Windows 优先使用安装目录内置 OCR，不依赖用户全局 PATH；Tesseract runtime 和语言数据必须包含许可证/来源说明

**规模/范围**：涉及 `pinvou3-app/src-tauri/tauri.conf.json`、Windows OS 工具解析、`file_ingest` 的 OCR 命令调用/依赖体检、前端依赖体检展示、Windows MSI 验收文档；不涉及 DeepSeek-TUI fork

## 宪章检查

- **中文文档优先**：PASS。计划、研究、数据模型、契约和 quickstart 均使用中文；保留必要英文命令、路径和 API 标识。
- **DeepSeek-TUI 底座优先**：PASS。变更限定在 Tauri app、OS 分层、打包资源与附件解析，不修改 Engine/SSE/Session/Compaction 等底座能力。
- **本地算力与数据边界**：PASS。OCR 在本地安装目录内执行，不引入远程模型、外部 API 或用户文件外发。
- **小步高质量变更**：PASS。按资源导入、打包配置、运行时路径解析、OCR 调用、依赖体检、验收测试分层推进。
- **可测试性与可验证交付**：PASS。定义 Rust 单测、cargo check、MSI 构建/解包、干净 Windows OCR 上传和依赖体检验收。
- **可维护性与长期演进**：PASS。通过 Spec Kit artifacts 记录 OCR runtime 组成、安装位置、运行时契约、许可证要求和验收步骤。

**闸门结果**：PASS。无需要豁免的宪章违反项。

## 项目结构

### 文档（本 feature）

```text
specs/015-bundle-windows-tesseract/
|-- plan.md
|-- research.md
|-- data-model.md
|-- quickstart.md
|-- contracts/
|   |-- windows-tesseract-runtime.md
|   `-- dependency-check-ui.md
`-- tasks.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
|-- src-tauri/
|   |-- resources/
|   |   `-- windows/
|   |       `-- tesseract/
|   |           |-- tesseract.exe
|   |           |-- tessdata/
|   |           |   |-- chi_sim.traineddata
|   |           |   `-- eng.traineddata
|   |           `-- LICENSE/NOTICE/source notes
|   |-- src/
|   |   |-- file_ingest.rs
|   |   `-- os/
|   |       |-- interface/
|   |       |-- linux/
|   |       `-- windows/
|   `-- tauri.conf.json
`-- src/
    |-- index.html
    `-- tauri-bridge.js
```

**结构决策**：Tesseract 属于 Windows 安装包运行时资源，放入 `pinvou3-app/src-tauri/resources/windows/tesseract/` 可与当前 Poppler/Pandoc 资源布局一致；命令路径、`tessdata` 路径和 Windows 特有降级策略进入 OS 层，`file_ingest` 只调用“可用 OCR 工具路径/参数”，避免 Windows 路径策略散落在业务解析逻辑中。

## 复杂度追踪

无。当前设计未违反宪章闸门，不需要复杂度豁免。
