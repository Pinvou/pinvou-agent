# PINVOU 麒麟 V11 适配要点

> 验证版本：PINVOU 0.5.9
> 验证环境：麒麟 V11 桌面版 x86_64、KVM/libvirt
> 验证结果：DEB 可安装，应用和 WebKit 主界面可正常启动。

## 一览

| 现象 | 根因 | 解决办法 |
|---|---|---|
| `apt` 不允许安装 | 麒麟软件包保护机制 | 开启维护模式后重启 |
| 提示缺少 `GLIBC_2.39` | 宿主机构建环境比麒麟新 | 在 Ubuntu 22.04 容器中构建（glibc 2.35，与仓库发布地板一致） |
| `pinvou3` 未找到 | 实际二进制名是 `pinvou3-tauri` | 统一命令名或提供启动脚本 |
| WebKit EGL 初始化失败并崩溃 | QXL 图形栈不兼容 | 虚拟显卡改为 VirtIO |
| 启动时寻找 `/dev/fgt340` | 麒麟同时加载多套厂商 EGL 驱动 | 虚拟机中只加载 Mesa EGL |
| 网页类任务可能无法运行 | 包内 esbuild 是 ARM64 | 换成 x86_64 esbuild 后再发布 |

## 1. 使用兼容的 Linux 构建基线

### 问题

在较新的宿主机直接构建，复制到麒麟后启动失败：

```text
/usr/bin/pinvou3-tauri: /lib/x86_64-linux-gnu/libc.so.6:
version `GLIBC_2.39' not found
```

麒麟 V11 使用 glibc 2.38。同为 x86_64 只代表 CPU 架构一致，不代表 Linux ABI 一定兼容。

### 处理

在 Ubuntu 22.04 容器中构建 amd64 DEB，不需要在麒麟中拉代码编译。选 22.04 而不是
更新的发行版：仓库的 Linux 发布地板就是 Ubuntu 22.04（glibc 2.35，见 `README.md` 与
`scripts/check-linux-glibc-floor.sh`，CI 的 deb 腿同样钉在 22.04），其 glibc 同时
低于麒麟的 2.38——一份包两端都满足。Debian 12（glibc 2.36）虽然也能在麒麟上运行，
但可能编出引用 2.36 符号的二进制，悄悄破坏 Ubuntu 22.04 地板：

```bash
docker run --rm -it \
  -v "$PWD:/workspace" \
  -v "$HOME/.cache/pinvou-jammy-cargo:/root/.cargo" \
  -v "$HOME/.cache/pinvou-jammy-target:/build-target" \
  -e CARGO_HOME=/root/.cargo \
  ubuntu:22.04 bash
```

容器内默认是 root，Cargo 主目录为 `/root/.cargo`。先装基础工具与 Tauri Linux 构建
依赖（与 CI 的 ubuntu-22.04 腿同列表），再装 Rust 与 Node——22.04 系统源的 rustc
过旧（仓库 MSRV 为 1.90，见 `src-tauri/Cargo.toml`），必须走 rustup：

```bash
apt-get update
apt-get install -y --no-install-recommends curl ca-certificates \
  libwebkit2gtk-4.1-dev libssl-dev libayatana-appindicator3-dev \
  libpipewire-0.3-dev libdrm-dev libgbm-dev libegl-dev \
  build-essential pkg-config cmake
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
apt-get install -y nodejs
```

然后执行：

```bash
export CARGO_TARGET_DIR=/build-target
cd /workspace/pinvou3-app
npm ci
npm run build
```

不要复用宿主机已有的 Cargo `target`，否则可能混入由宿主机 glibc 编译的产物。若
构建在 computer_use/Wayland 捕获相关 crate 上报 `spa_meta_region_is_valid` 之类的
`spa_sys` 符号错误：pipewire-rs 0.10 需要比 22.04 系统源（0.3.48）新的 SPA 头，
按 `scripts/ci-libpipewire-build.sh` 自建 PipeWire 到 `/usr/local`（CI 的 Linux 腿
就是这么做的）。

构建后必须检查最终二进制：

```bash
readelf --version-info /build-target/release/pinvou3-tauri \
  | grep -oE 'GLIBC_[0-9.]+' | sort -Vu | tail -n 1
```

通过标准：不高于麒麟的 2.38，也不高于仓库发布地板的 2.35（Ubuntu 22.04），两端都
满足。本次兼容包实测最高要求：

```text
GLIBC_2.34
```

## 2. 麒麟安装需要维护模式

麒麟的软件包保护机制会阻止普通状态下使用 `apt` 安装。安装前执行：

```bash
sudo mm-cli -o
sudo reboot
```

进入维护模式后，最小安装验证使用：

```bash
sudo apt install --no-install-recommends -y ./pinvou3_0.5.9_amd64.deb
```

`--no-install-recommends` 只验证主程序启动。需要 OCR、PDF、Office 等完整能力时，还要安装 DEB 中声明的推荐包。

完成安装后，应恢复麒麟的正常受保护状态，不要把长期维护模式作为运行条件。

## 3. 统一应用命令名

当前存在三个名称：

| 用途 | 名称 |
|---|---|
| DEB 包名 | `pinvou3` |
| 产品名 | PINVOU 智能助手 |
| 实际二进制 | `pinvou3-tauri` |

所以安装后直接输入 `pinvou3` 会提示未找到命令，实际命令是：

```bash
pinvou3-tauri
```

正式包应提供 `/usr/bin/pinvou3`，并让桌面入口也执行同一个命令，避免用户感知到内部 Tauri 二进制名。

## 4. 虚拟机图形适配

### QXL 不可用

QXL 显卡下 WebKitGTK 启动失败：

```text
DRI3: Screen seems not DRI3 capable
EGLDisplay Initialization failed: EGL_NOT_INITIALIZED
Cannot create EGL context: invalid display
```

随后应用在 `libwebkit2gtk-4.1.so.0` 中崩溃。

将虚拟显卡改为 VirtIO，关闭 3D 加速：

```bash
virt-xml <虚拟机名> -c qemu:///system --edit \
  --video model.type=virtio,model.acceleration.accel3d=no
```

### 麒麟 EGL 厂商驱动冲突

麒麟预装了多套国产 GPU EGL 驱动。虚拟机中启动时出现：

```text
not find "/dev/fgt340"
```

虚拟机没有对应的物理 GPU，但 GLVND 仍会枚举厂商驱动。虚拟机启动器中只加载 Mesa：

```bash
export __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json
export MESA_LOADER_DRIVER_OVERRIDE=virtio_gpu
exec /usr/bin/pinvou3-tauri "$@"
```

这两个环境变量是 **VirtIO 虚拟机专用处理**，不能无条件写入所有物理机安装包。

## 5. 当前仍需处理

### 高优先级

- `web-template` 内的 esbuild 是 ARM64 二进制，x86_64 上的网页类任务可能报 `Exec format error`：

  ```text
  pinvou3-app/src-tauri/resources/common/web-template/node_modules/esbuild/bin/esbuild
  ```

  （时点记录：该记录针对 0.5.9 时代的包；当前开源主干的 bundle 已不再携带
  `web-template` 资源，用户目录 `~/.pinvou3/web-template` 与用户生成内容按退役策略
  保留不删（见 `features/retirement.rs`），自建包如仍内嵌该资源才需处理本项。）

- 正式 DEB 应直接提供 `pinvou3` 命令。

### 完整适配验收

当前只证明“可安装、可启动、主界面可显示”。正式发布前还需验证：

- 模型配置、真实对话和流式输出；
- 会话保存与恢复；
- 本地文件和知识库；
- 工具调用、MCP 和工作流；
- OCR、PDF、Office 和压缩包处理；
- 网页类任务及 x86_64 esbuild；
- 升级、卸载、重装和非维护模式运行。

## 6. 发布前检查

- [ ] 使用 Ubuntu 22.04（glibc 2.35）兼容环境构建，同时满足麒麟 V11 与仓库发布地板。
- [ ] 最终主程序要求的 glibc 不高于麒麟版本，且不高于仓库发布地板的 2.35。
- [ ] DEB 内所有 ELF 文件均为 x86_64。
- [ ] `pinvou3` 命令与桌面入口一致。
- [ ] 物理机和 VirtIO 虚拟机分别验证。
- [ ] 完成模型、文件、知识库、工具和工作流回归。

## 结论

PINVOU 的 Tauri + Rust + WebKitGTK 技术路线可以运行在麒麟 V11 x86_64 上。适配重点不是在麒麟中重新开发或重新编译，而是固定旧 glibc 构建基线、处理图形栈差异，并保证包内所有本地二进制与目标架构一致。
