# 快速开始：Windows MSI 构建计划验证

## 1. 确认当前 feature

```powershell
Get-Content .specify\feature.json
git branch --show-current
```

预期：

- `feature_directory` 指向 `specs/002-windows-msi-package`。
- 当前分支为 `002-windows-msi-package`。

## 2. 准备 Windows 构建前置条件

在 Windows 构建机上确认：

```powershell
git submodule update --init --recursive
rustc --version
cargo --version
node --version
npm --version
```

如果 `DeepSeek-TUI/` 未初始化，先修复 submodule；否则 Cargo path dependency 会失败。

当前构建机已确认：

- `rustc 1.96.0 (ac68faa20 2026-05-25)`
- `cargo 1.96.0 (30a34c682 2026-05-25)`
- `stable-x86_64-pc-windows-msvc (default)`
- `node v26.0.0`
- `npm 11.12.1`
- `tauri-cli 2.11.1`

如遇 crates.io 下载超时，可在用户级 Cargo 配置中临时使用国内 sparse 源：

```toml
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"

[net]
git-fetch-with-cli = true
```

## 3. 安装前端/Tauri 依赖

```powershell
cd pinvou3-app
npm install
```

## 4. 构建 MSI

本项目当前没有修改 `tauri.conf.json` 的 bundle target，而是通过 Tauri CLI 参数显式指定 MSI：

```powershell
cd pinvou3-app
npm run tauri build -- --bundles msi
```

本轮已确认 `@tauri-apps/cli` 2.11.1 支持 `--bundles msi`，并且该命令已成功生成 MSI。

## 5. 查找产物

优先检查：

```powershell
Get-ChildItem pinvou3-app\src-tauri\target\release\bundle\msi -Filter *.msi -Recurse
```

当前产物：

```text
pinvou3-app/src-tauri/target/release/bundle/msi/pinvou3_0.4.3_x64_en-US.msi
```

校验哈希：

```powershell
Get-FileHash -Algorithm SHA256 pinvou3-app\src-tauri\target\release\bundle\msi\pinvou3_0.4.3_x64_en-US.msi
```

当前 SHA256：

```text
8F2FC142225601ED00E342596454126ECC126DCB5057C6955737C38772A23C6B
```

## 6. 安装 smoke

在 Windows 机器上执行：

1. 双击 MSI 或通过系统安装入口安装。
2. 从开始菜单或安装目录启动 pinvou3。
3. 打开设置或模型服务配置入口。
4. 退出应用后通过 Windows 常规入口卸载。
5. 确认用户价值数据目录没有被默认删除，或记录未执行原因。

当前状态：MSI 已生成；安装 smoke 尚未执行，因为安装会修改当前 Windows 系统安装状态。

## 7. 记录结果

本轮记录文件：

```text
specs/002-windows-msi-package/msi-build-report.md
specs/002-windows-msi-package/minimal-change-record.md
```
