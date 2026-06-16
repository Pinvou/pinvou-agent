# Quickstart：Windows 软件更新验证

## 前置条件

- 在 Windows 环境运行。
- 可访问 H3C OTA 查询和反馈服务，或有本地 mock 服务。
- 准备样例包：`C:\Users\z27014\Downloads\Megabook2_BIOS_12.0.0.0\Pinvou3_0.4.4.0.zip`。
- 当前仓库位于 `E:\Pinvou\pinvou3`。

## 实现配置

Windows OTA 默认连接 H3C 升级服务，环境变量仅用于联调或测试覆盖。

```powershell
# 可选：覆盖默认 https://api.intcloud.h3c.com，指向本地 mock
$env:PINVOU3_OTA_HOST = "http://127.0.0.1:8787"
$env:PINVOU3_OTA_SN = "mock-sn"
$env:PINVOU3_OTA_SOFTWARE_ID = "Pinvou3_Win"
```

- `PINVOU3_OTA_HOST`：可选，默认 `https://api.intcloud.h3c.com`；代码会调用 `/ota/pkg/package/upgrade/check`、`/ota/pkg/package/upgrade/getDownloadInfo` 和 `/ota/pkg/package/updateLog`。
- `PINVOU3_OTA_SN`：可选，默认读取 `COMPUTERNAME`，仍为空时使用 `UNKNOWN`。
- `PINVOU3_OTA_SOFTWARE_ID`：可选，默认 `Pinvou3_Win`。
- `PINVOU3_HOME`：可选，用于重定位 `~/.pinvou3`，联调时可指向临时目录。

更新 zip、解压目录、MSI 和待反馈记录均落在 `~/.pinvou3/updates/`；反馈记录文件为 `update-feedback.json`。

## Mock 服务

本 feature 未新增专用 mock server。联调时可使用任意 HTTP mock 工具启动 `PINVOU3_OTA_HOST`，至少提供：

- `POST /ota/pkg/package/upgrade/check`：返回 `success=true`、`code=200`，`data.updateVersion` 大于当前版本，`data.pkgUrl` 指向可下载 zip，`data.pkgMd5` 为下载 zip MD5。
- `POST /ota/pkg/package/upgrade/getDownloadInfo`：当 check 响应未带 `pkgUrl` 时返回下载地址和 MD5。
- `POST /ota/pkg/package/updateLog`：接收 `softwareIdentification`、`sn`、`currentVersion`、`updateVersion`、`updateErrorInfo`、`updateResult`。

## 静态检查

```powershell
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

## 单元测试

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml updater --lib
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_update --lib
```

重点覆盖：

- H3C OTA 查询响应解析。
- H3C OTA 反馈请求序列化。
- 下载 zip 安全解压与 `OtaInfo.json` 解析。
- `OtaInfo.json` 在 `OtaInfo.json` 和 `FullPack/OtaInfo.json` 两种路径下都可识别。
- 通过 `softwareId = Pinvou3_Win` 或 `softwareType = Pinvou3` 定位 `MSI`。
- 拒绝路径穿越、绝对路径、非 `.msi` 文件和清单缺失。
- Linux `.deb` 现有测试继续通过。

## 样例包解析 smoke

使用样例包验证：

```text
下载 zip:
- OtaInfo.json
- Files/Pinvou3/pinvou3_0.4.4_x64_en-US.msi
```

期望结果：

- 能读取 `OtaInfo.json`。
- 能定位 `Files/Pinvou3/pinvou3_0.4.4_x64_en-US.msi`。
- 若 hash 校验开启，能校验 `fileMetaInfos[0].hash`。

## 前端 smoke

启动应用：

```powershell
# 可选：使用 mock 服务时覆盖默认 H3C 后台地址
$env:PINVOU3_OTA_HOST = "http://127.0.0.1:8787"
cd pinvou3-app
npm run dev
```

验证步骤：

1. 打开“版本与更新”。
2. 点击“检查更新”。
3. mock 服务返回无更新时，页面显示“已是最新版本”，不下载。
4. mock 服务返回有效更新时，页面显示目标版本和更新说明。
5. 点击“下载并安装”，下载进度递增。
6. 下载完成后进入解析和准备安装阶段。
7. 安装器成功启动后，当前 pinvou 进程退出。

## 反馈 smoke

1. 启动安装器前确认本地生成待反馈记录。
2. 完成升级后重新启动 pinvou3。
3. 应用启动或更新面板初始化后触发反馈。
4. 服务端收到 `/ota/pkg/package/updateLog` 请求。
5. 反馈成功后本地待反馈记录标记为已反馈或被清理。
6. 模拟网络失败时，记录保留并在下次启动后重试。

## 本轮验证结果

2026-06-16 已执行：

- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml updater --lib`：通过，6 passed。
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_update --lib`：通过，10 passed。
- `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`：通过。输出仍包含既有 DeepSeek-TUI/private_interfaces 与 pinvou3 bridge unused 警告，本 feature 未改动 `DeepSeek-TUI/`。
- `npm run dev` 前端 smoke：需真实窗口、mock/正式 OTA 服务和 MSI 启动动作，本轮未自动执行，按上方步骤手动验收。

## 非 Windows 回归

在非 Windows 或 Linux 环境执行：

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml updater --lib
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

期望结果：

- Linux `.deb` 更新链路不改变。
- Windows `MSI` 逻辑不会在非 Windows 平台触发。
- 非 Windows 平台不会因为缺少 Windows 安装能力而影响应用启动。
