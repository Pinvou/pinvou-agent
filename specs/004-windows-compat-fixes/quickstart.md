# 快速开始：验证 OS 调度抽象层

## 1. 确认当前 feature

```powershell
git branch --show-current
Get-Content .specify/feature.json
```

预期：

- 分支为 `004-windows-compat-fixes`。
- `feature_directory` 指向 `specs/004-windows-compat-fixes`。

## 2. 阅读计划与契约

```powershell
Get-Content specs/004-windows-compat-fixes/plan.md
Get-Content specs/004-windows-compat-fixes/contracts/os-dispatch-contract.md
```

预期：

- 计划明确采用 OS 调度抽象层。
- 契约列出系统打开、命令检测、GPU 工具候选、平台路径解析、超级权限、依赖安装、应用更新等接口。

## 2.1 确认实现边界

```powershell
Get-ChildItem pinvou3-app/src-tauri/src/os
rg -n "crate::os::" pinvou3-app/src-tauri/src/commands.rs pinvou3-app/src-tauri/src/monitor.rs pinvou3-app/src-tauri/src/file_ingest.rs pinvou3-app/src-tauri/src/bridge/paths.rs pinvou3-app/src-tauri/src/super_permission.rs pinvou3-app/src-tauri/src/updater.rs
```

预期：

- `os/mod.rs` 仅做模块装配和 re-export；`os/interface/` 按业务域提供统一门面；`os/linux/`、`os/windows/` 承载按业务域拆分的平台实现，且系统独有实现文件名带 `linux_` 或 `windows_` 前缀；`os/unsupported.rs` 承载通用降级；pinvou3-app 本轮不新增 `os/macos.rs`。
- 主要业务调用点通过 `crate::os::*` 委托平台行为。
- `bridge/paths.rs` 只组合 pinvou3 业务目录，不直接维护 Windows/Linux 路径分支。

## 3. 检查扫描记录

```powershell
Get-Content specs/004-windows-compat-fixes/compatibility-scan.md
```

预期：

- 能看到已修复项和暂不修复项。
- 后续任务应把已修复项迁移/收敛到 OS 抽象层。

## 4. 实施后编译检查

```powershell
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

预期：

- 命令成功完成。
- 允许存在既有 warning，但不得出现 error。

## 5. 实施后静态扫描

```powershell
rg -n -F "xdg-open" pinvou3-app/src-tauri/src
rg -n -F "pkexec" pinvou3-app/src-tauri/src
rg -n -F "apt-get" pinvou3-app/src-tauri/src
rg -n -F "Command::new(\"which\")" pinvou3-app/src-tauri/src
rg -n "cfg\\((windows|not\\(windows\\)|target_os)" pinvou3-app/src-tauri/src/bridge/paths.rs
```

预期：

- Linux-only 命令主要集中在 `pinvou3-app/src-tauri/src/os/linux/`。
- 业务文件不再继续扩散新的 OS 专属命令。
- `pinvou3-app/src-tauri/src/bridge/paths.rs` 不再直接维护路径平台分支，只组合 pinvou3 业务目录。

## 6. Windows 手动冒烟

在 Windows 上运行应用后检查：

- 设置页外部链接可通过默认浏览器打开。
- 打开文件和打开所在目录不依赖 `xdg-open`。
- 依赖体检不会因缺少 `which` 全部误报。
- `PINVOU3_HOME=/tmp/...` 这类兼容路径会映射到 Windows 临时目录下的等价路径。
- 超级权限开关返回清晰“不支持 Linux sudo 超级权限开关”。
- `.deb` 更新流程不在 Windows 上执行。

## 7. Linux 回归抽查

在 Linux 上抽查：

- 系统打开仍可使用 `xdg-open`。
- `PINVOU3_HOME`、`HOME` 和 `~/.pinvou3` 目录布局保持原行为。
- 超级权限仍走原 `pkexec` + sudoers 机制。
- 依赖一键安装仍保留包名白名单。
- `.deb` 更新流程仍可校验路径和 sha256。
