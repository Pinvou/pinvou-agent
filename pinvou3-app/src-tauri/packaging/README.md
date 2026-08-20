# 平台打包目录

这里仅保存社区构建需要的可审查安装资源；大型运行时不得直接提交到本目录。

```text
packaging/
├─ linux/
│  ├─ deb/                 # desktop、维护脚本、user units、ASR drop-in、显式 canary profile
│  ├─ descriptor/          # 固定 app / ASR descriptor v1
│  └─ supervisor/          # 独立 pinvou-supervisor 源码；构建产物仍进 target/
├─ macos/                  # 社区版打包配置
└─ windows/
   ├─ nsis/                # 最小化 NSIS 安装 hook
   └─ runtime/             # 独立 runtime 的锁校验、descriptor 与原子 staging 脚本
```

公共构建编排位于 `../../scripts/tauri/`：

- `platform-config.js`：只负责选择当前平台 overlay。
- `build.js`：组合平台配置并启动 Tauri CLI；Linux deb 构建还会以
  `umask 0022` 把固定 `deb.files` allowlist 复制到 `src-tauri/target/` 临时
  staging，并在构建后核对包内唯一路径、`root/root`、预期 mode 与 SHA-256；
  任一 symlink、staging / 包内 hardlink、路径、mode 或 hash 异常都 fail closed。
- `windows-runtime.js`：读取 runtime descriptor，不感知安装器细节。
- `windows-installer.js`：按 bundle 目标准备 NSIS 专属资源。

工具市场不在构建期注入共享 API Key。社区构建从公开、锁定的独立仓库取得 Windows 运行时，
不包含私有凭据或官方签名工具。

平台脚本不得修改其他平台的资源树；所有生成物必须写入 `src-tauri/target/`。Windows
安装包通过 `private-runtimes/windows` 的锁定 gitlink 合入运行时，协议和初始化方式见
[`docs/windows-private-runtime-submodule.md`](../../../docs/windows-private-runtime-submodule.md)。

## Linux 资源边界

当前工作树的 Linux bundle 已包含独立 `pinvou-supervisor`、socket-activated user
service、固定 app / ASR descriptor、受监督 app unit、ASR cgroup drop-in、显式
MegaBook canary profile、专用 launcher 和只接受 `activate / deactivate / status`
的 profile helper。helper 对完整 effective unit / resource / restart policy 和受信 Supervisor
`Status + Reconciled` 回执 fail closed，并冻结 v1 profile / desktop / marker 的路径、
字节与 hash cleanup ABI。仓库还已实现固定 E2E harness 与 deb 固定 payload
mode/hash 门禁；验收脚本在执行任何已安装 helper / Supervisor 前，以 deb SHA-256、
完整 maintainer control 成员/跟踪的安装行为字段、生成 `.list`、control `md5sums`、`dpkg --verify` 和
12 条关键安装路径证明安装行为与未变基线 deb 等价，但不声称能由 dpkg 状态重建原始
压缩 archive 字节。这些是已实现的
发行候选资源与验收结构，不是
MegaBook 已部署且验收的事实：真实 deb 安装、High、OOM 与 purge 尚未
执行。旧 f24
direct transient canary 只验证过当时的直接临时 unit，不能作为新 Supervisor
或 HostWork 链路的验收证据。

普通 `pinvou3.desktop` 仍直接启动 `/usr/bin/pinvou3-tauri`，因此 generic desktop
不受 app cgroup 硬限保护。只有经受审部署流程显式激活
`megabook-canary.conf`，再使用专用 canary launcher 时，才进入固定 app service
链路。这两项 canary 资源默认都是 inert：不安装为全局默认，也不自动把
canary desktop 暴露到普通应用列表。MegaBook v1 是唯一已声明的 app 绝对内存
profile：`MemoryHigh=4G`、`MemoryMax=8G`、`MemorySwapMax=2G`；不能推广为
其他设备的默认值。App 的完整 policy owner 是 base app unit 加该显式
MegaBook canary drop-in；base unit 持有 restart / StartLimit，profile 持有绝对
memory 线。ASR 的完整 policy owner 则是 ASR base unit 加 Supervisor package
drop-in；base unit 持有 restart / StartLimit，drop-in 声明 20% / 35% / 2G 与
其余 cgroup 保护。该 drop-in 随包安装并 daemon-reload，但 postinst 不停止
或重启已运行 ASR，该实例要到之后的有效重启才会应用新 cgroup property。

Supervisor 的安装边界如下：

- descriptor v1 只列编译期固定的 app `status/launch` 与 ASR `status/stop`；wire
  protocol v2 不接受 PID、任意 unit、cgroup 路径、property、命令或 shell；
- app 的 restart contract 是 `on-failure / 15s / 300s / 3`，ASR 是
  `on-failure / 3s / 60s / 3`。Daemon 从 effective systemd property 读取并
  精确核对；缺失、无效或被 drop-in 改写都使 Status fail closed 为
  `outcome_unknown`；
- ASR 20% / 35% 还要用物理 RAM 换算成页对齐字节上限；页舍入或更严格值
  可以通过，更宽的策略不可以，且 systemd effective 值必须与实际 cgroup
  文件相等；
- app Launch 只在初始与紧邻 action 的两次受信 preflight 都是
  `Inactive | Failed` 且 `MainPID=None` 时 start。`Active` 视为已满足；
  `Activating / Deactivating / Unknown`、任何带 MainPID 的状态、不可用或不可信 Status
  都无副作用拒绝。start 后无法验证时，还需两次 preflight 都可归因且 start
  返回成功，才能认定所有权并 rollback stop；不会误停未知、既有、过渡中
  或启动失败的 app；
- ASR Stop 前必须从受信 Status 得到当前 systemd `InvocationID`，并把它作为精确
  instance generation 前置条件；app HostWork 为 `essential + non-governable`
  status-only，Governor 不会经它自停 app / WebKit；
- socket 限于同 UID，daemon 对 ASR Stop 再核验调用方 PID 是当前 app
  `MainPID`，client 反向核验回包 UID 与 Supervisor `MainPID`。这是固定动作与
  进程身份约束，不是对已获得同一 Unix UID 的恶意 shell 的强隔离；
- Supervisor 位于 app service cgroup 之外，用 `control-v1.jsonl` 持久化
  `Pending` 与终态 tombstone，用独立滚动 `observations-v1.jsonl` 保留状态证据；
- 基础 app unit 故意不写绝对 `MemoryHigh / MemoryMax / MemorySwapMax`，设备值只能由
  显式、受审查 profile 提供；模型、Renderer 和远程输入都不能修改；
- 维护脚本不 source 用户 home 配置，不创建全局或用户永久 enable symlink；
  只为当时在线会话启动固定 socket，卸载前移除历史不安全
  `/etc/sudoers.d/pinvou3`；
- Linux 构建只产出当前主机的 native release companion，并核对 ELF machine 与
  deb architecture；Intel/AMD x86-64 在这里使用 Rust `x86_64-unknown-linux-gnu`
  和 Debian `amd64`，不在 Linux bundle 步骤冒充交叉编译。固定 payload
  从 allowlist 进入 target staging，最终 deb 必须对每个固定目标证明恰好一份、
  `root/root`、预期 mode 且内容 hash 与源字节一致；这是构建门禁，不是
  MegaBook 安装证据。

开发期 `npm run dev` 不是日常运行、内存恢复或 watchdog 边界。打包和
运维也不得把“Resource Agent 正在采样”、“Supervisor 资源已随包”或“旧 canary
曾经稳定”写成“当前 MegaBook 已受新链路保护”。

资源治理唯一权威见
[`ADR-0009`](../../../docs/adr/0009-PinvouOS-资源治理与Host-Supervisor.md)。在 MegaBook 真实
deb 安装、High、OOM 与 purge E2E 通过前，本文只声明已入库的安装资源、
验收 harness、构建门禁和安全边界，不发布未验证的恢复手册或默认激活步骤。
