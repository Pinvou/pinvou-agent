# Windows 兼容性扫描记录

**日期**：2026-06-16

**分支**：`004-windows-compat-fixes`

## 扫描范围

- `pinvou3-app/src-tauri/src/`
- `pinvou3-app/src-tauri/tests/`
- `pinvou3-app/` 开发脚本和安装说明
- `docs/` 与既有 Spec Kit 文档中的系统命令示例
- `DeepSeek-TUI/` 仅做边界识别，不在本轮直接修改底座源码

## 架构调整

当前 feature 已从“在调用点直接补 `cfg`”调整为“新增 OS 调度抽象层”。

新增目标边界：

- `pinvou3-app/src-tauri/src/os/mod.rs`：只负责模块装配、平台实现选择和统一接口 re-export。
- `pinvou3-app/src-tauri/src/os/interface/`：按系统打开/命令检测、路径、权限、依赖安装、更新等业务域提供统一接口薄封装。
- `pinvou3-app/src-tauri/src/os/linux/`：按业务域承载 Linux 现有行为，包括 `xdg-open`、`which`、`HOME`、`pkexec`、`apt-get`、`.deb` 安装；系统独有实现文件名使用 `linux_` 前缀。
- `pinvou3-app/src-tauri/src/os/windows/`：按业务域承载 Windows 行为，包括 `cmd /C start`、`where`、`USERPROFILE`/`HOMEDRIVE`/`HOMEPATH`、`/tmp` 兼容映射；系统独有实现文件名使用 `windows_` 前缀。
- `pinvou3-app/src-tauri/src/os/unsupported.rs`：承载非 Linux/Windows 平台的清晰降级。
- pinvou3-app 本轮不新增 `os/macos.rs`。

业务调用点原则：

- `commands.rs` 只调用 `crate::os::open_target`。
- `monitor.rs` 只调用 `crate::os::nvidia_smi_candidates`。
- `file_ingest.rs` 只调用 `crate::os::command_exists` 与 `crate::os::install_dependencies`。
- `super_permission.rs` 只调用 `crate::os::super_permission_*`。
- `updater.rs` 只调用 `crate::os::check_update_platform_support` 与 `crate::os::install_update_package`。
- `bridge/paths.rs` 只组合 pinvou3 业务目录，平台家目录和 `/tmp` 兼容由 `crate::os` 处理。

## 已修复并迁入 OS 抽象层

| 编号 | 类别 | 原位置 | 问题 | 当前处理 |
|---|---|---|---|---|
| WIN-001 | 系统打开命令 | `commands.rs` | `open_external_url`、`open_in_system`、`open_containing_folder` 曾固定调用或分散维护系统打开命令 | `commands.rs` 委托 `crate::os::open_target`；Linux 用 `xdg-open`，Windows 用 `cmd /C start ""`，其他平台返回不支持 |
| WIN-002 | GPU 监控 | `monitor.rs` | `nvidia-smi` 候选路径曾在监控模块内维护 | `monitor.rs` 委托 `crate::os::nvidia_smi_candidates`；Windows/Linux 候选集中在 OS 模块 |
| WIN-003 | Linux sudo 权限 | `super_permission.rs` | 超级权限开关固定依赖 `/etc/sudoers.d`、`pkexec`、`bash`、`rm` | `super_permission.rs` 委托 `crate::os::super_permission_*`；Linux 保持原行为，Windows 和其他平台清晰降级 |
| WIN-004 | 外部工具检测 | `file_ingest.rs` | 依赖检测曾固定或分散使用 `which`/`where` | `file_ingest.rs` 委托 `crate::os::command_exists`；Linux 用 `which`，Windows 用 `where` |
| WIN-005 | Linux 依赖一键安装 | `file_ingest.rs` | 一键安装固定走 `pkexec sh -c apt-get install`，Windows 上不可用 | `file_ingest.rs` 委托 `crate::os::install_dependencies`；Linux 保留白名单和 `apt-get`，Windows 和其他平台返回不支持 |
| WIN-006 | deb 更新流程 | `updater.rs` | 应用内更新固定下载 `.deb` 并用 `pkexec apt-get` 安装，Windows MSI 场景不适用 | `updater.rs` 先调用 `crate::os::check_update_platform_support`；安装委托 `crate::os::install_update_package`；Windows 和其他平台不执行 `.deb` 流程 |
| WIN-007 | 平台路径解析 | `bridge/paths.rs` | 家目录选择和 Windows `/tmp` 兼容映射曾直接写在路径模块内 | `bridge/paths.rs` 委托 `crate::os::user_home_dir` 与 `crate::os::platform_compat_path`，自身只保留目录布局组合 |

## 已识别但本轮暂不修复

| 编号 | 类别 | 位置 | 现象 | 暂不修复原因 |
|---|---|---|---|---|
| OBS-001 | 测试临时目录 | `pinvou3-app/src-tauri/src/**/*`、`pinvou3-app/src-tauri/tests/*` | 多个测试夹具使用 `/tmp/pinvou3-*` | 主要影响 Windows 跑测试，不是运行时路径；建议后续集中替换为 `std::env::temp_dir()` 或 OS 层 `platform_compat_path` |
| OBS-002 | Linux 安装文档 | `pinvou3-app/INSTALL.md`、`deploy/nginx-pinvou3-updates.conf`、`docs/*` | 文档中包含 `bash`、`export`、`rm -rf`、`/var/www`、`.deb` 等 Linux 示例 | 属于文档/部署说明，需单独产出 Windows 安装与维护说明，避免和运行时代码修复混在一起 |
| OBS-003 | 文件预处理外部工具 | `pinvou3-app/src-tauri/src/file_ingest.rs` | `pdftotext`、`pandoc`、`soffice`、`tesseract`、`7z`、`msgconvert` 在 Windows 上安装方式不同 | 本轮先修复检测命令和一键安装降级；完整 Windows 依赖安装指引需要产品/安装包层设计 |
| OBS-004 | Python 命令名 | `harness.rs`、`bridge/marketplace.rs`、`bridge/bundle.rs` | 部分路径仍通过 `cfg!(target_os = "windows")` 选择 `python`/`python3` | 影响 workflow 和脚本启动策略，需结合 Windows 开发/运行环境统一设计，避免只改一处造成行为不一致 |
| OBS-005 | Linux-only dev 脚本 | `pinvou3-app/run-dev.sh`、`pinvou3-app/src-tauri/scripts/prerm.sh` | 开发启动和 deb 卸载脚本是 shell 脚本 | 需要新增 Windows 开发启动脚本或文档，不应直接改掉 Linux 脚本 |
| OBS-006 | DeepSeek-TUI 底座平台逻辑 | `DeepSeek-TUI/crates/tui/src/utils.rs`、`DeepSeek-TUI/crates/tui/src/command_safety.rs` 等 | 底座已有自身的跨平台打开和命令安全逻辑 | DeepSeek-TUI 是底座，本轮不重复实现也不直接修改；仅记录边界 |

## 静态扫描结果

已执行：

```powershell
rg -n -F 'xdg-open' pinvou3-app/src-tauri/src
rg -n -F 'pkexec' pinvou3-app/src-tauri/src
rg -n -F 'apt-get' pinvou3-app/src-tauri/src
rg -n 'Command::new\("which"\)' pinvou3-app/src-tauri/src
rg -n 'cfg\((windows|not\(windows\)|target_os)' pinvou3-app/src-tauri/src/bridge/paths.rs
```

结果：

- `xdg-open` 实际调用仅在 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs`。
- `apt-get` 实际安装命令仅在 `pinvou3-app/src-tauri/src/os/linux/linux_dependency.rs` 与 `pinvou3-app/src-tauri/src/os/linux/linux_update.rs`。
- `Command::new("which")` 未在业务调用点命中。
- `bridge/paths.rs` 未命中平台路径调度 `cfg`。
- `pkexec` 命中包含部分注释和文案；实际命令调用集中在 `pinvou3-app/src-tauri/src/os/linux/`。

## DeepSeek-TUI 边界扫描

已执行：

```powershell
rg -n -F 'xdg-open' DeepSeek-TUI
rg -n -F 'pkexec' DeepSeek-TUI
rg -n -F 'apt-get' DeepSeek-TUI
rg -n 'Command::new\("which"\)|Command::new\("where"\)' DeepSeek-TUI
```

结果：

- `DeepSeek-TUI/crates/tui/src/utils.rs` 已有底座自己的跨平台打开逻辑。
- `DeepSeek-TUI/crates/tui/src/command_safety.rs` 中的 `pkexec` 属于安全规则模式。
- `apt-get` 主要出现在 Dockerfile、安装文档、远程 smoke 脚本和 benchmark 脚本。
- 未发现需要 pinvou3 本轮直接修改的 DeepSeek-TUI 运行时阻断项。

## 验证

- 已执行：`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`
- 结果：通过。
- 备注：检查输出仍包含项目既有 warning，主要来自 `DeepSeek-TUI` 和 pinvou3 既有未使用项；本轮未做无关清理。

补充验证：

- 尝试执行：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml os:: --lib`
- 结果：180 秒超时，已停止残留 `cargo/rustc` 进程。
- 处理：本轮以 `cargo check`、OS 抽象层编译、静态扫描和文档化风险作为交付验证；后续如需完整测试，建议在更长超时时间或 CI 环境中运行。

## 最终未提交文件范围

执行 `git status --short` 时，本 feature 相关变更包括：

- `.specify/feature.json`
- `AGENTS.md`
- `pinvou3-app/src-tauri/src/commands.rs`
- `pinvou3-app/src-tauri/src/file_ingest.rs`
- `pinvou3-app/src-tauri/src/monitor.rs`
- `pinvou3-app/src-tauri/src/super_permission.rs`
- `pinvou3-app/src-tauri/src/updater.rs`
- `pinvou3-app/src-tauri/src/bridge/paths.rs`
- `pinvou3-app/src-tauri/src/lib.rs`
- `pinvou3-app/src-tauri/src/os/`，不包含 `os/macos.rs`
- `specs/004-windows-compat-fixes/`

## 建议下一步

1. 为 Windows 开发新增 `run-dev.ps1` 或在文档中明确 PowerShell 启动步骤。
2. 集中替换测试中的 `/tmp/pinvou3-*` 为 `std::env::temp_dir()` 或 OS 层 `platform_compat_path`，确保 Windows 能跑相关单测。
3. 设计 Windows 下附件解析依赖的安装/探测策略，尤其是 `pandoc`、LibreOffice、Poppler、Tesseract、7-Zip、Python。
4. 为 MSI 更新设计 Windows 专用更新路径，不复用 `.deb` 更新流程。
