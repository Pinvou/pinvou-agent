# 最小变更清单：Windows MSI 安装包构建

## 1. 目标

本清单用于证明 MSI 打包工作遵守“尽量不改变当前项目代码”的约束。默认不触碰 DeepSeek-TUI 底座、聊天主链路、session、工具循环、MCP、workflow agent 编排和 Linux-only updater 逻辑。

## 2. 变更记录

| changed_file | change_type | reason | risk | verification | out_of_scope_note |
|---|---|---|---|---|---|
| `.specify/feature.json` | config | 指向当前 Spec Kit feature `specs/002-windows-msi-package` | 仅影响 Spec Kit 当前上下文 | `Get-Content .specify/feature.json` | 不影响应用运行时代码 |
| `AGENTS.md` | docs | Spec Kit 指针切换到 MSI 打包计划 | 仅影响 Codex/Agent 上下文 | 人工检查 SPECKIT block | 不影响应用运行时代码 |
| `specs/002-windows-msi-package/spec.md` | docs | 记录 MSI 打包需求规格 | 无运行时风险 | checklist 16/16 | 不影响应用运行时代码 |
| `specs/002-windows-msi-package/plan.md` | docs | 记录实施计划和宪章检查 | 无运行时风险 | plan 审查 | 不影响应用运行时代码 |
| `specs/002-windows-msi-package/research.md` | docs | 记录 Tauri/MSI 技术决策 | 无运行时风险 | research 审查 | 不影响应用运行时代码 |
| `specs/002-windows-msi-package/data-model.md` | docs | 定义构建报告和最小变更记录实体 | 无运行时风险 | data model 审查 | 不影响应用运行时代码 |
| `specs/002-windows-msi-package/contracts/windows-msi-contract.md` | docs | 定义 MSI 构建与验收契约 | 无运行时风险 | contract 审查 | 不影响应用运行时代码 |
| `specs/002-windows-msi-package/quickstart.md` | docs | 记录 MSI 构建快速入口与当前成功产物 | 无运行时风险 | quickstart 审查 | 不影响应用运行时代码 |
| `specs/002-windows-msi-package/tasks.md` | docs | 记录实现任务和完成状态 | 无运行时风险 | tasks 格式校验 | 不影响应用运行时代码 |
| `specs/002-windows-msi-package/msi-build-report.md` | validation | 记录 Windows 构建环境、命令、产物和 smoke 结果 | 无运行时风险 | 本报告复核 | 不影响应用运行时代码 |
| `specs/002-windows-msi-package/minimal-change-record.md` | validation | 记录最小变更边界和未触碰区域 | 无运行时风险 | 本清单复核 | 不影响应用运行时代码 |
| `pinvou3-app/package-lock.json` | config | `npm install` 将 lockfile 根版本从 `0.4.1` 同步为 `0.4.3`，与 `package.json` 当前版本一致 | 依赖树未新增额外包；仅 package metadata 版本同步 | `npm install` 成功且 0 vulnerabilities | 不改变应用运行时代码 |

## 3. 环境侧变更记录

以下变更发生在当前 Windows 用户环境，不属于仓库源码变更：

| 环境项 | change_type | reason | verification |
|---|---|---|---|
| Rust stable MSVC toolchain | local environment | 为 Tauri Windows/MSI 构建提供 `rustc`、`cargo`、MSVC target | `rustc --version`、`cargo --version`、`rustup show active-toolchain` |
| `C:\Users\z27014\.cargo\config.toml` | local environment | 将 crates.io registry 临时切换为 `rsproxy.cn` sparse 源，缓解依赖下载超时 | `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 成功 |
| Tauri/WiX 缓存 | local build cache | Tauri 构建 MSI 时自动下载、校验并解压 WiX 3.14 | `npm run tauri build -- --bundles msi` 成功 |

## 4. 底座边界检查

- 未修改 `DeepSeek-TUI/`。
- 未重写 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle 或 Compaction。
- MSI 打包只处理桌面打包、构建记录和安装验收，不改变 agent 底座职责。

## 5. Windows 暂不纳入项

- Windows 原生自动更新器：不在本 feature 范围。
- Linux `.deb` updater、`apt`、`pkexec`：不伪装为 Windows 可用能力。
- Poppler、Tesseract、LibreOffice、7z 等附件外部工具的完整 Windows 安装体验：不在本 feature 范围。
- 代码签名、企业分发策略、静默安装策略：不在本 feature 范围。

## 6. 运行时代码状态

当前记录：尚未触碰 `pinvou3-app/src-tauri/src/lib.rs`、`pinvou3-app/src-tauri/src/updater.rs`、`pinvou3-app/src-tauri/src/file_ingest.rs` 或 `DeepSeek-TUI/` 下文件。

`pinvou3-app/src-tauri/Cargo.toml` 曾在 `git status` 中显示修改，但 `git diff -- pinvou3-app/src-tauri/Cargo.toml` 无文本差异；当前 `git diff --name-only` 已不再显示该文件。

## 7. 审查结论

- `git diff --name-only` 当前显示 `.specify/feature.json`、`AGENTS.md`、`pinvou3-app/package-lock.json` 以及新增 `specs/002-windows-msi-package/`。
- 没有修改 DeepSeek-TUI submodule 内容。
- 没有修改聊天主链路、session、工具执行、MCP、workflow agent 编排或附件解析运行时代码。
- 没有把 Linux `.deb` updater、`apt`、`pkexec` 描述为 Windows 已完成能力；这些项仍被记录为本 feature 暂不纳入范围。
- MSI 已通过 `npm run tauri build -- --bundles msi` 生成，产物路径记录在 `msi-build-report.md`。

## 8. 收尾契约复核

- `msi-build-report.md` 已覆盖构建命令、初始失败摘要、环境补齐路径、成功产物检查和安装 smoke 状态。
- 当前已生成 MSI；未执行安装 smoke 的原因是该动作会修改当前 Windows 系统安装状态。
- `minimal-change-record.md` 已覆盖实际变更文件、环境侧变更、变更原因、风险、验证方式和范围排除项。
- 底座关键词扫描命中的是“未修改/未重写”的否定性说明，不代表发生底座重写。
