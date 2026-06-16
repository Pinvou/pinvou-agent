# 研究：OS 调度抽象层与 Windows 兼容性修复

## 决策 1：新增薄 OS 调度层，不在业务调用点继续扩散平台分支

**Decision**：新增 `pinvou3-app/src-tauri/src/os/` 模块，承载系统打开、命令查找、GPU 工具候选路径、平台家目录/兼容路径、超级权限、依赖安装、应用更新等 OS 差异。pinvou3-app 本轮只提供 Linux 与 Windows 平台实现，业务层只调用 `os::*` 统一接口。

**Rationale**：用户明确希望保持现有代码尽量不变，并通过一层 OS 调度代码决定 Linux/Windows 行为。集中抽象比散落 `cfg` 更容易审查、测试和后续扩展。

**Alternatives considered**：

- 在现有文件中直接加 `#[cfg]`：改动小但会让平台逻辑分散，后续 Windows 适配继续变脏。
- 大规模重构服务层：超出当前兼容性目标，风险高。

## 决策 1.1：路径兼容也进入 OS 调度层

**Decision**：`bridge/paths.rs` 不再直接维护 Windows/Linux 家目录选择、`/tmp` 兼容映射和路径平台分支；这些差异由 `os::user_home_dir` 与 `os::platform_compat_path` 提供，`bridge/paths.rs` 只组合 pinvou3 自身目录布局。

**Rationale**：此前路径兼容虽然改动小，但仍然通过 `cfg` 分散在业务路径模块中。路径是 Windows 迁移的核心风险之一，应与命令、权限、更新一样集中调度，避免后续继续在 `bridge` 层扩散平台判断。

**Alternatives considered**：

- 保留 `bridge/paths.rs` 内的 `cfg`：短期少迁移一个文件，但破坏“OS 差异集中入口”的架构目标。
- 引入独立 `paths_platform` 模块：职责与 `os` 调度层重叠，不如统一收敛。

## 决策 2：Linux 实现优先迁移现有逻辑，Windows 实现单独补齐

**Decision**：Linux 模块应尽量复用当前已存在逻辑，例如 `xdg-open`、`pkexec`、`apt-get`、`which`、`HOME`、`/usr/bin/nvidia-smi`；Windows 模块提供 `cmd /C start`、`where`、`USERPROFILE`/`HOMEDRIVE`/`HOMEPATH`、`/tmp` 兼容映射、Windows `nvidia-smi.exe` 候选路径，以及不支持 Linux 提权/`.deb` 更新的明确返回。macOS 暂不在 pinvou3-app 层新增专属实现；如构建到其他平台，走通用 unsupported fallback。

**Rationale**：迁移目标不是改变 Linux 行为，而是在 Windows 上提供正确行为。把 Linux 现有实现放入 `os/linux/` 能保留原语义，也让 review 更容易识别行为是否被改动。

**Alternatives considered**：

- 把 Linux 和 Windows 写在同一函数里：短期文件少，长期可读性差。
- 抽成 trait + dyn dispatch：当前是编译期平台差异，不需要运行时多态复杂度。

## 决策 3：OS 抽象层使用编译期分发

**Decision**：`os/mod.rs` 使用 `#[cfg(target_os = "...")]` re-export 平台实现，调用方不感知具体平台。

**Rationale**：平台行为由构建目标决定，编译期分发更简单、无运行时开销，也能避免在 Windows 二进制中误编译 Linux-only `Command::new("pkexec")` 等逻辑。

**Alternatives considered**：

- 运行时判断 `std::env::consts::OS`：仍可能编译进不适用代码，且错误更晚暴露。
- 到处使用条件编译：违背抽象层目标。

## 决策 3.1：`os/mod.rs` 只做装配，统一接口按业务域拆分

**Decision**：`os/mod.rs` 不承载全部统一接口实现；统一接口按业务域拆分到 `interface/` 目录，分别封装系统打开/命令检测、路径、权限、依赖安装、应用更新等能力，再由 `mod.rs` 统一 re-export。平台具体实现也按业务域拆分到 `linux/` 与 `windows/` 目录；系统独有实现文件名带系统名前缀，例如 `linux_system.rs`、`windows_path.rs`；`unsupported.rs` 保留为简单降级实现。

**Rationale**：随着路径、权限、更新等能力都收敛进 OS 调度层，单个 `mod.rs` 会变成混杂入口，不利于维护者按业务定位问题。业务域拆分能保持调用方 API 不变，同时让模块边界更清晰。

**Alternatives considered**：

- 继续把所有统一接口放在 `mod.rs`：文件短期少，但后续新增 OS 能力会让入口膨胀。
- 继续保留单文件 `linux.rs`、`windows.rs`：文件数量少，但路径、权限、更新逻辑会再次堆在单个大文件中，不符合后续维护诉求。

## 决策 4：不支持的 Windows 能力必须明确降级

**Decision**：Linux sudoers 超级权限、`apt-get` 依赖一键安装和 `.deb` 更新在 Windows 实现中返回清晰错误；不得静默成功或模拟开启。

**Rationale**：这些能力涉及权限和安装器语义。Windows 可行方案需要 UAC、MSI 更新、签名、安装器参数和进程退出策略，必须独立设计。

**Alternatives considered**：

- 用管理员 PowerShell 替代 `pkexec`：安全边界和产品语义不同，不能作为小修。
- 在 Windows 上直接返回成功：会误导用户和模型。

## 决策 5：扫描记录保留“先行修复”和“目标抽象层”的差异

**Decision**：`compatibility-scan.md` 继续记录已发现/已修复问题；后续任务应补充“迁移到 OS 抽象层”状态。

**Rationale**：当前工作区已有第一轮修复，这些证据仍有价值。计划调整后不应抹掉历史，而应把它们转成抽象层重整任务。

**Alternatives considered**：

- 删除扫描记录重写：会丢失已验证信息。
- 把扫描记录当最终架构：不符合用户新要求。
