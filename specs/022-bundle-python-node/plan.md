# 实施计划：Windows 安装包内置 Python 与 Node 运行时

**分支**：`022-bundle-python-node` | **日期**：2026-07-06 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/022-bundle-python-node/spec.md` 的功能规格

## 概要

Windows 安装包需要随包提供 Python 与 Node 运行时，避免用户机器没有真实 Python、只有 Microsoft Store 占位符时，`present_artifact`、天气、同花顺问财等 Python 型 MCP 稳定失败。实现规划采用“打包前校验并展开离线 zip 到 Windows 资源目录，Tauri 将资源打进安装包，NSIS/WiX 在安装后配置系统环境变量”的路径。

核心结果：

- 安装目录下稳定存在 `python\pythonw.exe` 与 `node\node.exe`。
- 系统环境变量 `PINVOU3_PYTHON` 指向 `$INSTDIR\python\pythonw.exe`。
- 系统 `PATH` 包含 `$INSTDIR\python` 与 `$INSTDIR\node`。
- 升级时刷新到当前安装目录，卸载时只清理由本应用安装目录管理的变量和 PATH 项。

## 技术上下文

**语言/版本**：Rust（Tauri 后端）、PowerShell（Windows 打包预处理）、NSIS 脚本、WiX XML fragment

**主要依赖**：Tauri 2 Windows bundler、现有 NSIS `installer-hooks.nsh`、现有 WiX environment fragment 模式、项目内置 7zip 资源、指定离线运行时 zip

**存储**：Windows 安装目录资源文件；系统环境变量注册表；构建期 `pinvou3-app/src-tauri/resources/windows/python/` 与 `pinvou3-app/src-tauri/resources/windows/node/` 资源目录

**测试**：`npm run build:nsis`、`cargo check`、NSIS 生成脚本检查、安装目录文件检查、系统环境变量检查、卸载清理检查；MSI 配置静态检查

**目标平台**：Windows 桌面安装包；Linux/macOS 不在本 feature 范围内

**项目类型**：desktop-app / Windows packaging

**性能目标**：安装完成后 10 秒内可验证 `pythonw.exe`、`node.exe`、`PINVOU3_PYTHON` 和 PATH 项；Python 型 MCP 不再出现 `Python was not found`

**约束**：不修改 DeepSeek-TUI 底座；保持本地运行时离线可用；不依赖用户机器已安装 Python/Node；只清理由本应用安装目录管理的环境变量项；文档中文优先

**规模/范围**：涉及 Windows 打包资源、预构建脚本、Tauri resource 映射、NSIS 安装/卸载 hook、WiX environment fragment、`paths::python_command()` 的 Windows 内置路径解析

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。计划、研究、数据模型、契约和 quickstart 均使用中文，英文保留为路径、命令、变量名和工具名。
- **DeepSeek-TUI 底座优先**：PASS。本计划只调整 Tauri app 的 Windows 打包与运行时定位，不改 Engine、ToolRegistry、MCP client 或 fork 行为。
- **本地算力与数据边界**：PASS。运行时资源来自本地离线 zip，安装后本地执行，不引入新的远程默认依赖。
- **小步高质量变更**：PASS。改动限制在 Windows 运行时资源、安装器环境变量和 Python 路径解析，不做无关重构。
- **可测试性与可验证交付**：PASS。定义了构建期校验、生成脚本检查、安装后文件/环境变量检查和卸载清理检查。
- **可维护性与长期演进**：PASS。通过脚本校验源包、固定资源目录和契约文档降低后续维护成本；不把用户系统 Python/Node 作为隐式前提。

**门禁结果**：PASS

## 项目结构

### 文档（本 feature）

```text
specs/022-bundle-python-node/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── windows-runtime-installation.md
└── tasks.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── package.json
├── scripts/
│   ├── clean-nsis-staging.ps1
│   └── prepare-windows-runtimes.ps1      # 计划新增
└── src-tauri/
    ├── tauri.conf.json
    ├── resources/windows/
    │   ├── nsis/installer-hooks.nsh
    │   ├── python/                       # 计划由 zip 展开生成/维护
    │   ├── node/                         # 计划由 zip 展开生成/维护
    │   ├── python-node-path.wxs          # 计划新增，用于 MSI 环境变量
    │   └── ...
    └── src/bridge/paths.rs
```

**结构决策**：运行时目录放在 `resources/windows/` 下，沿用 poppler、pandoc、tesseract、7zip 等 Windows 内置工具的资源模式；安装器环境变量沿用现有 NSIS hook 与 WiX fragment 模式；`paths.rs` 只负责运行时解析，不承担解压或安装逻辑。

## 复杂度追踪

无宪章违背项。

## Phase 0：研究产物

见 [research.md](./research.md)。

## Phase 1：设计产物

- 数据模型：[data-model.md](./data-model.md)
- 安装契约：[contracts/windows-runtime-installation.md](./contracts/windows-runtime-installation.md)
- 验证指南：[quickstart.md](./quickstart.md)

## Phase 1 后宪章复查

- **中文文档优先**：PASS。新增设计产物均为中文。
- **DeepSeek-TUI 底座优先**：PASS。无需改 DeepSeek-TUI。
- **本地算力与数据边界**：PASS。仅使用本地离线运行时资源。
- **小步高质量变更**：PASS。实现边界仍集中在 Windows packaging。
- **可测试性与可验证交付**：PASS。契约和 quickstart 覆盖构建、安装、运行、卸载。
- **可维护性与长期演进**：PASS。源包校验、目录契约和卸载保护均已纳入设计。

**复查结果**：PASS
