# Windows MSI 构建报告

## 1. Feature 上下文

- Feature：`002-windows-msi-package`
- Feature 目录：`specs/002-windows-msi-package`
- 当前目标：尽量不改变当前项目代码，生成 Windows `.msi` 安装包；如环境阻塞，则记录明确原因和补齐路径。
- 当前结论：Rust/Cargo 环境已补齐，MSI 已成功生成。

## 2. 当前项目配置检查

| 项 | 当前值 | 结论 |
|---|---|---|
| productName | `pinvou3` | 可用于 MSI 展示名 |
| app/package version | `0.4.3` | 与 Tauri/Cargo 版本一致 |
| tauri.conf version | `0.4.3` | 与 package/Cargo 版本一致 |
| Cargo package version | `0.4.3` | 与 package/Tauri 版本一致 |
| identifier | `com.pinvou.pinvou3` | 可追溯 |
| Rust edition | `2021` | 来自 `pinvou3-app/src-tauri/Cargo.toml` |
| bundle targets | `deb` | 当前只显式配置 Linux `.deb`；本次通过 CLI 参数临时指定 `msi` |
| Windows icon | `pinvou3-app/src-tauri/icons/icon.ico` | 存在 |

## 3. Windows 构建环境记录

| 检查项 | 结果 | 说明 |
|---|---|---|
| 操作系统 | Microsoft Windows 11 企业版 10.0.22631 64-bit | 当前构建机 |
| rustc | `rustc 1.96.0 (ac68faa20 2026-05-25)` | 可用，stable MSVC 工具链 |
| cargo | `cargo 1.96.0 (30a34c682 2026-05-25)` | 可用 |
| active toolchain | `stable-x86_64-pc-windows-msvc (default)` | 当前默认工具链 |
| Cargo registry | `sparse+https://rsproxy.cn/index/` | 写入用户级 `C:\Users\z27014\.cargo\config.toml`，用于缓解 crates.io 访问超时 |
| node | v26.0.0 | 可用 |
| npm | 11.12.1 | 可用 |
| Tauri CLI | tauri-cli 2.11.1 | `node_modules/.bin/tauri.cmd --version` 可用 |
| MSVC Build Tools | 已发现 | 本机存在 VS 2022 Enterprise / BuildTools 的 `vcvars64.bat` |
| WiX/MSI 链路 | 已通过 | Tauri 自动下载、校验并解压 WiX 3.14，完成 `candle` 和 `light` |
| `DeepSeek-TUI/` submodule | 已初始化 | `DeepSeek-TUI/crates/tui/Cargo.toml` 存在 |

## 4. Submodule / Path Dependency 检查

- 已运行 `git submodule update --init --recursive`。
- `git submodule status --recursive` 显示 `DeepSeek-TUI` 位于 `1ad4f27d95c8a492913bb3b61e14e09decc5ea64`。
- `DeepSeek-TUI/crates/tui/Cargo.toml` 当前存在。
- 结论：submodule/path dependency 前置条件已补齐。

## 5. 构建策略

本次保持项目配置最小变更，不修改 `tauri.conf.json` 的 bundle target，通过 CLI 参数指定 MSI：

```powershell
cd pinvou3-app
npm install
npm run tauri build -- --bundles msi
```

## 6. 构建执行记录

### 6.1 初始失败记录

首次执行 `npm run tauri build -- --bundles msi` 时失败，原因是 PowerShell PATH 中找不到 `cargo`：

```text
failed to run 'cargo metadata' command to get workspace directory:
failed to run command cargo metadata --no-deps --format-version 1: program not found
```

该问题已通过安装 Rust stable MSVC 工具链并修复 Cargo 可用性解决。

### 6.2 Rust 层检查记录

执行命令：

```powershell
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

结果：成功。

说明：首次下载依赖时 crates.io sparse 访问超时，已通过用户级 Cargo registry 配置切换到 `rsproxy.cn` 后复跑成功。检查过程中仅出现现有代码 warning，未出现编译错误。

### 6.3 MSI 构建记录

执行目录：

```text
pinvou3-app/
```

执行命令：

```powershell
npm run tauri build -- --bundles msi
```

结果：成功。

关键输出：

```text
Finished `release` profile [optimized] target(s) in 10m 03s
Built application at: E:\Pinvou\pinvou3\pinvou3-app\src-tauri\target\release\pinvou3-tauri.exe
Running candle for "...target\release\wix\x64\main.wxs"
Running light to produce E:\Pinvou\pinvou3\pinvou3-app\src-tauri\target\release\bundle\msi\pinvou3_0.4.3_x64_en-US.msi
Finished 1 bundle at:
E:\Pinvou\pinvou3\pinvou3-app\src-tauri\target\release\bundle\msi\pinvou3_0.4.3_x64_en-US.msi
```

编译 warning 摘要：

- `codewhale-tui` 产生 12 个现有 warning，主要为可见性和未使用代码提示。
- `pinvou3-tauri` 产生 6 个现有 warning，主要为未使用 import、未使用 `mut`、dead code 和 harness 字段未读。
- 本次未为消除 warning 修改运行时代码，以遵守“尽量不改变当前项目代码”的约束。

## 7. MSI 产物检查

| 项 | 值 |
|---|---|
| MSI 路径 | `pinvou3-app/src-tauri/target/release/bundle/msi/pinvou3_0.4.3_x64_en-US.msi` |
| 绝对路径 | `E:\Pinvou\pinvou3\pinvou3-app\src-tauri\target\release\bundle\msi\pinvou3_0.4.3_x64_en-US.msi` |
| 文件大小 | 15,798,272 bytes |
| 生成时间 | 2026-06-15 16:32:15 |
| SHA256 | `8F2FC142225601ED00E342596454126ECC126DCB5057C6955737C38772A23C6B` |

## 8. 安装 smoke

| 检查项 | 结果 | 说明 |
|---|---|---|
| 安装 | 未执行 | MSI 已生成；安装会修改当前 Windows 系统安装状态，暂未在未确认的情况下执行 |
| 启动 | 未执行 | 依赖安装 smoke |
| 配置入口 | 未执行 | 依赖启动 smoke |
| 卸载 | 未执行 | 依赖安装 smoke |
| 用户数据保留 | 未执行 | 依赖安装/卸载 smoke |

## 9. 已知限制

- MSI 已生成，但尚未执行真实安装、启动、卸载 smoke。
- 代码签名不在本 feature 范围。
- Windows 原生 updater 不在本 feature 范围；现有 Linux `.deb` updater 未迁移。
- Poppler、Tesseract、LibreOffice、7z 等附件外部工具的完整 Windows 安装和路径配置不在本 feature 范围。
- 企业分发策略、静默安装、组策略部署不在本 feature 范围。
- 模型服务不内置于 MSI；用户需自行配置本机、WSL、远程 GB10 或其他 OpenAI-compatible endpoint。

## 10. 复现步骤

在具备 Windows 构建条件的机器上：

```powershell
git submodule update --init --recursive
rustc --version
cargo --version
node --version
npm --version

cd pinvou3-app
npm install
npm run tauri build -- --bundles msi
```

产物检查：

```powershell
Get-ChildItem pinvou3-app\src-tauri\target\release\bundle\msi -Filter *.msi -Recurse
Get-FileHash -Algorithm SHA256 pinvou3-app\src-tauri\target\release\bundle\msi\pinvou3_0.4.3_x64_en-US.msi
```

安装 smoke 建议：

1. 安装生成的 MSI。
2. 从开始菜单或安装目录启动 pinvou3。
3. 打开设置或模型服务配置入口。
4. 通过 Windows 常规入口卸载。
5. 确认卸载或重复安装不会默认删除用户价值数据；如未执行，记录原因。
