# Quickstart：Windows OTA 域名引导验证

## 前置条件

- 在 Windows 环境运行 pinvou3。
- 当前仓库位于 `E:\Pinvou\pinvou3`。
- 准备一个可控的域名引导 mock 服务或正式可访问的 `https://bootstrap.magic.h3c.com`。
- 准备一个可控 OTA mock 服务，至少支持更新查询、下载信息和升级结果反馈接口。

## 配置文件

域名引导配置文件：

```text
~/.pinvou3/windows-ota-bootstrap.json
```

Windows 默认展开路径通常为：

```text
%USERPROFILE%\.pinvou3\windows-ota-bootstrap.json
```

默认内容格式：

```json
{
  "bootstrapHost": "https://bootstrap.magic.h3c.com"
}
```

验证点：

- 删除该文件后检查更新，应使用默认 `https://bootstrap.magic.h3c.com`。
- 写入自定义地址后检查更新，应请求自定义地址。
- 写入非法地址后检查更新，应忽略该值并使用默认 `https://bootstrap.magic.h3c.com`。
- 升级或重装应用后，已修改文件不应被主动覆盖。

联调时可用 `PINVOU3_HOME` 指向临时用户目录，避免污染真实 `~/.pinvou3`：

```powershell
$env:PINVOU3_HOME = "E:\Pinvou\pinvou3\.tmp\ota-bootstrap-home"
New-Item -ItemType Directory -Force -Path "$env:PINVOU3_HOME" | Out-Null
@'
{
  "bootstrapHost": "http://127.0.0.1:8788"
}
'@ | Set-Content -Path "$env:PINVOU3_HOME\windows-ota-bootstrap.json" -Encoding UTF8
```

## Mock 服务期望

域名引导 mock：

```http
POST /v2/bootstrap
```

返回：

```json
{
  "code": 0,
  "data": {
    "smarthubOta": "http://127.0.0.1:8787"
  }
}
```

OTA mock 至少提供：

- `POST /ota/pkg/package/upgrade/check`
- `POST /ota/pkg/package/upgrade/getDownloadInfo`
- `POST /ota/pkg/package/updateLog`
- `GET` 完整包 zip 下载地址

## 静态检查

```powershell
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

## 单元测试

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_update --lib
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_domain_bootstrap --lib
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml updater --lib
```

重点覆盖：

- 配置文件缺失、为空、合法自定义地址和非法地址回退默认地址。
- BIOS SN 以 `2198`、`2199`、其他前缀、空值、读取失败时的有效 SN 选择。
- 域名引导签名在固定 timestamp 下与 C# 参考算法一致。
- 域名引导响应 `smarthubOta` 大小写不敏感匹配。
- 域名引导失败时不继续 OTA 查询。
- 查询、下载信息和升级结果反馈使用同一个 `ota_host`。
- 旧 `update-feedback.json` 缺少 `ota_host` 时仍有兼容处理。
- 前端 `updateInfo` 原样透传给 `install_update`，当前无需 UI 读取或展示 `ota_host`。

## 手动 smoke

```powershell
cd pinvou3-app
npm run dev
```

验证步骤：

1. 打开“版本与更新”。
2. 点击“检查更新”。
3. 观察域名引导 mock 收到 `/v2/bootstrap`，请求体包含 `device_id`、`product_id`、`timestamp`、`sign`、`sign_type`。
4. 观察 OTA mock 收到 `/ota/pkg/package/upgrade/check`，host 为 `smarthubOta` 返回值。
5. mock 返回无更新时，页面显示“已是最新版本”。
6. mock 返回新版本时，页面显示目标版本和更新说明。
7. 点击下载并安装，确认下载、解压、MSI 定位和 MSI 提权启动流程仍正常。
8. MSI 启动前检查 `~/.pinvou3/updates/update-feedback.json`，确认包含 `ota_host`。
9. 升级后重新启动应用，观察反馈请求发送到记录中的 `ota_host`。

## 失败场景

- 域名引导服务不可达：手动检查更新应在 15 秒内提示更新服务暂不可用，应用不崩溃。
- 域名引导返回缺少 `data`：不访问 OTA 后台。
- 域名引导返回缺少 `smarthubOta`：不访问 OTA 后台。
- `smarthubOta` 为非法 URL：不访问 OTA 后台。
- BIOS SN 不符合前缀：请求使用固定 SN。

## 非 Windows 回归

在 Linux 或非 Windows 构建环境执行：

```powershell
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml updater --lib
```

期望结果：

- Linux `.deb` 更新链路不改变。
- 非 Windows 平台不编译或调用 Windows 域名引导模块。
- 跨平台 `updater.rs` 命令接口保持兼容。

## 本轮实现备注

- 配置文件由应用读取但不主动创建或覆盖。
- 配置文件不存在、为空、JSON 非法、缺少 `bootstrapHost` 或 `bootstrapHost` 非 HTTP/HTTPS URL 时，使用默认 `https://bootstrap.magic.h3c.com`。
- 合法自定义 `bootstrapHost` 会移除末尾 `/` 后参与 `/v2/bootstrap` 拼接。
- `tauri-bridge.js` 和 `index.html` 当前不需要修改；`ota_host` 作为后端字段随 `updateInfo` 传回安装命令，用于写入升级反馈记录。

## 本轮验证结果

2026-06-17 已执行：

- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml paths --lib`：通过，7 passed。
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_update --lib`：通过，13 passed。
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_domain_bootstrap --lib`：通过，9 passed。
- `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml updater --lib`：命令通过；当前过滤条件未匹配到 updater 专用单测，0 tests。
- `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`：通过；仍有既有 DeepSeek-TUI/private_interfaces 与 pinvou3 bridge unused/dead_code 警告，本 feature 未改动 `DeepSeek-TUI/`。
- `git status --short DeepSeek-TUI`：无输出，确认底座未修改。

未执行：

- Windows UI/mock 手动 smoke：需要启动可控域名引导 mock、OTA mock 与真实应用窗口，本轮未自动执行；按“手动 smoke”步骤补验。
