# 平台打包目录

这里仅保存社区构建需要的可审查安装资源；大型运行时不得直接提交到本目录。

```text
packaging/
├─ linux/
│  └─ deb/                 # desktop、postinst、prerm
├─ macos/                  # 社区版打包配置
└─ windows/
   ├─ nsis/                # 最小化 NSIS 安装 hook
   └─ runtime/             # 独立 runtime 的锁校验、descriptor 与原子 staging 脚本
```

公共构建编排位于 `../../scripts/tauri/`：

- `platform-config.js`：只负责选择当前平台 overlay。
- `build.js`：组合平台配置并启动 Tauri CLI。
- `windows-runtime.js`：读取 runtime descriptor，不感知安装器细节。
- `windows-installer.js`：按 bundle 目标准备 NSIS 专属资源。

工具市场不在构建期注入共享 API Key。社区构建从公开、锁定的独立仓库取得 Windows 运行时，
不包含私有凭据或官方签名工具。

平台脚本不得修改其他平台的资源树；所有生成物必须写入 `src-tauri/target/`。Windows
安装包通过 `private-runtimes/windows` 的锁定 gitlink 合入运行时，协议和初始化方式见
[`docs/windows-private-runtime-submodule.md`](../../../docs/windows-private-runtime-submodule.md)。

## Linux 资源边界

当前 Linux bundle 只安装 desktop / deb 资源，desktop 直接启动 `pinvou3-tauri`；没有
独立 `pinvou-supervisor`、app cgroup、`MemoryHigh / MemoryMax / OOMPolicy` 或 Resource
Control Adapter。开发期 `npm run dev` 也不是日常运行或内存恢复边界。打包和运维不得把
“Resource Agent 正在采样”写成“应用已有 watchdog”。

目标 packaging 必须以可审查静态资源安装同 UID Supervisor 与固定 HostWork descriptor：

- Supervisor 位于 app cgroup 外，应用和 WebKit 子进程使用独立 cgroup；
- `MemoryHigh`、`MemoryMax`、`OOMPolicy=kill`、`KillMode=control-group`、`TasksMax`、
  重启退避和日志保留由部署 descriptor 决定，不能由模型或 Renderer 修改；
- descriptor 只能列编译期认可的工作与动作，不接受 PID、任意 unit、命令或 shell；
- 不得复用 `NOPASSWD:ALL`、通用 sudo 或任意 `systemctl`；未来系统级动作必须使用另行
  评审的固定 action + descriptor helper / polkit；
- 生成的 unit、descriptor、临时 staging 和签名产物仍只能写入 `target/`，经测试后由
  installer 安装；不得把机器地址、凭据或本机绝对路径写进仓库。

资源治理唯一权威见
[`ADR-0009`](../../../docs/adr/0009-PinvouOS-资源治理与Host-Supervisor.md)。在 Supervisor
代码和 unit 尚未落地前，本文只冻结安装边界，不发布虚构的启停命令或恢复手册。
