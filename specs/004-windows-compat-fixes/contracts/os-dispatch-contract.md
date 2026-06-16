# 契约：OS 调度抽象层

## 目标

新增一层负责 OS 代码调度的抽象层，让 pinvou3-app 的业务调用点不直接散落 Linux/Windows 分支。本轮 pinvou3-app 暂不新增 macOS 专属实现，其他平台走通用 unsupported fallback。

## 目标源码结构

建议结构：

```text
pinvou3-app/src-tauri/src/os/
├── interface/
│   ├── dependency.rs
│   ├── mod.rs
│   ├── path.rs
│   ├── permission.rs
│   ├── system.rs
│   └── update.rs
├── mod.rs
├── linux/
│   ├── linux_dependency.rs
│   ├── linux_path.rs
│   ├── linux_permission.rs
│   ├── linux_system.rs
│   ├── linux_update.rs
│   └── mod.rs
├── windows/
│   ├── windows_dependency.rs
│   ├── windows_path.rs
│   ├── windows_permission.rs
│   ├── windows_system.rs
│   ├── windows_update.rs
│   └── mod.rs
└── unsupported.rs
```

`mod.rs` 只负责模块装配、平台实现选择和统一接口 re-export；`interface/` 按业务域提供统一接口薄封装；`linux/`、`windows/` 按相同业务域承载具体平台行为，且系统独有实现文件名必须带系统名前缀；`unsupported.rs` 承载通用降级。

## 必备接口契约

### 1. 系统打开

接口语义：

```text
open_target(target, label) -> Result<(), String>
```

要求：

- 调用方负责 URL allowlist 或用户路径校验。
- Windows 使用系统默认打开方式。
- Linux 保留 `xdg-open` 行为。
- 其他平台返回不支持。
- 失败时返回包含 `label` 的错误。

### 2. 命令存在性检测

接口语义：

```text
command_exists(command) -> bool
```

要求：

- Windows 使用 Windows 命令查找机制。
- Linux 使用现有命令查找机制。
- 其他平台返回 `false`。
- 检测失败返回 `false`，不得 panic。

### 3. GPU 工具候选

接口语义：

```text
nvidia_smi_candidates() -> Vec<&'static str>
```

要求：

- 所有平台先尝试 `nvidia-smi`。
- Windows 增加常见 `nvidia-smi.exe` 绝对路径。
- Linux 增加 `/usr/bin/nvidia-smi` 和 `/usr/local/bin/nvidia-smi`。

### 4. 平台路径解析

接口语义：

```text
user_home_dir() -> PathBuf
platform_compat_path(value) -> PathBuf
```

要求：

- Windows 使用 `USERPROFILE`，必要时回退 `HOMEDRIVE` + `HOMEPATH`，最后才兼容 `HOME`。
- Linux 使用 `HOME`，保持现有缺省行为。
- 其他平台使用 fallback，不新增专属路径规则。
- Windows 下兼容类 Unix 测试或配置中出现的 `/tmp/...`，映射到 `std::env::temp_dir()` 下的等价路径。
- `bridge/paths.rs` 只保留 pinvou3 目录布局组合逻辑，不直接维护 `#[cfg(windows)]`、`#[cfg(not(windows))]` 或 Windows 专属路径转换。
- 该接口只处理平台路径差异，不改变 `~/.pinvou3/`、sessions、workflows、bundle 等业务目录布局。

### 5. 超级权限

接口语义：

```text
super_permission_is_enabled() -> bool
enable_super_permission() -> Result<(), String>
disable_super_permission() -> Result<(), String>
super_permission_turn_reminder() -> &'static str
```

要求：

- Linux 保留现有 sudoers + `pkexec` 机制。
- Windows 和其他平台返回清晰不支持信息。
- 非 Linux 不得尝试 `/etc/sudoers.d`、`sudo`、`apt`、`systemctl`、`pkexec`。

### 6. 依赖一键安装

接口语义：

```text
install_dependencies(packages) -> Result<(), String>
```

要求：

- Linux 保留包名白名单和 `pkexec apt-get`。
- Windows 和其他平台返回不支持，并提示按本系统方式安装。

### 7. 应用内更新

接口语义：

```text
check_update_platform_support() -> Result<(), String>
install_update_package(path) -> Result<(), String>
```

要求：

- Linux 保留 `.deb` 更新流程。
- Windows 不复用 `.deb`；MSI 更新另行设计。
- 不支持时不得下载或执行 Linux 安装命令。

## 调用点契约

完成抽象层迁移后，以下业务文件不应直接调用平台命令：

- `commands.rs` 不直接调用 `xdg-open`、`cmd /C start`、`open`。
- `file_ingest.rs` 不直接调用 `which` 或 `where`。
- `monitor.rs` 不直接维护 OS 专属 `nvidia-smi` 路径。
- `bridge/paths.rs` 不直接维护 OS 专属家目录选择、`/tmp` 兼容转换或路径 `cfg` 分支；应委托给 `os`。
- `super_permission.rs` 不直接混写 Windows/Linux 行为；可整体委托给 `os`。
- `updater.rs` 不直接在非 Linux 分支中处理 Linux 安装命令。

## 验证契约

最低验证：

```powershell
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

静态扫描建议：

```powershell
rg -n -F "xdg-open" pinvou3-app/src-tauri/src
rg -n -F "pkexec" pinvou3-app/src-tauri/src
rg -n -F "apt-get" pinvou3-app/src-tauri/src
rg -n -F "Command::new(\"which\")" pinvou3-app/src-tauri/src
rg -n "cfg\\((windows|not\\(windows\\)|target_os)" pinvou3-app/src-tauri/src/bridge/paths.rs
```

预期：

- 命中主要集中在 `src/os/linux/` 或注释/文档。
- 业务调用点通过 `os` 模块调用统一接口。
- `bridge/paths.rs` 不再出现平台路径调度用的 `cfg`。

## 非目标

- 不在本 feature 中重写 DeepSeek-TUI 底座。
- 不在本 feature 中完整实现 MSI 自更新。
- 不在本 feature 中批量重构所有测试夹具。
- 不删除 Linux `.deb` 更新能力。
