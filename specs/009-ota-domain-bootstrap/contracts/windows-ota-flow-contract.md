# 契约：Windows OTA 域名引导后的更新流程

## 总体流程

```text
读取 bootstrap 配置
  -> 读取/选择有效 SN
  -> POST bootstrap /v2/bootstrap
  -> 从 data.smarthubOta 得到 ota_host
  -> POST ota_host /ota/pkg/package/upgrade/check
  -> 必要时 POST ota_host /ota/pkg/package/upgrade/getDownloadInfo
  -> HTTP 下载 pkgUrl
  -> 解压 zip 并定位 MSI
  -> 启动 MSI 前写入 update-feedback.json，其中包含 ota_host
  -> 升级后首次运行 POST ota_host /ota/pkg/package/updateLog
```

## 更新查询

Windows `check_for_update` 必须先完成域名引导。成功后使用解析出的 `ota_host` 调用既有 OTA 查询接口。

```http
POST {ota_host}/ota/pkg/package/upgrade/check
Content-Type: application/json
```

```json
{
  "sn": "有效 SN",
  "softwareId": "Pinvou3_Win",
  "version": "0.4.6",
  "hardwareInfo": null
}
```

行为约定：

- `ota_host` 必须来自本次域名引导成功结果。
- 无可用更新仍是正常查询结果，不应提示为检查失败。
- 查询失败不得触发下载。
- 返回给前端的 Windows 更新信息应携带 `ota_host`，用于后续安装反馈记录。

## 下载信息

当更新查询响应未包含完整包下载地址时，使用同一个 `ota_host` 调用下载信息接口。

```http
POST {ota_host}/ota/pkg/package/upgrade/getDownloadInfo
Content-Type: application/json
```

行为约定：

- 不能重新域名引导后切换 OTA 来源。
- 下载 URL 仍由 OTA 后台响应中的 `pkgUrl` 决定。
- 下载、MD5 校验、zip 解压和 MSI 定位沿用既有 Windows OTA 约定。

## 安装前反馈记录

启动 MSI 前写入：

```json
{
  "software_identification": "Pinvou3_Win",
  "sn": "有效 SN",
  "current_version": "0.4.6",
  "update_version": "0.4.7",
  "update_result": "START_INSTALL",
  "update_error_info": "",
  "installer_path": "C:\\Users\\...\\.pinvou3\\updates\\...\\pinvou3.msi",
  "ota_host": "https://api.intcloud.h3c.com",
  "created_at": "2026-06-17T00:00:00Z",
  "last_attempt_at": null,
  "attempts": 0,
  "reported": false
}
```

行为约定：

- `ota_host` 必须来自更新查询阶段的域名引导结果。
- 旧记录缺少 `ota_host` 时，反馈阶段可重新域名引导以兼容既有已安装版本。

## 升级结果反馈

升级后首次运行读取待反馈记录，优先使用记录中的 `ota_host`：

```http
POST {ota_host}/ota/pkg/package/updateLog
Content-Type: application/json
```

```json
{
  "softwareIdentification": "Pinvou3_Win",
  "sn": "有效 SN",
  "currentVersion": "0.4.6",
  "updateVersion": "0.4.7",
  "updateErrorInfo": "",
  "updateResult": "UPGRADE_SUCCEED"
}
```

行为约定：

- 当前版本达到或高于目标版本时反馈 `UPGRADE_SUCCEED`。
- 当前版本低于目标版本且无法确认安装结果时反馈或保留 `UNKNOWN`，错误信息说明当前版本未达到目标版本。
- 反馈成功后清理本地记录。
- 反馈失败时保留记录并增加重试计数。

## 失败和隐私

- 域名引导失败时停止本次 OTA 检查，错误文案避免暴露完整 SN。
- 手动检查需要友好提示；静默检查仍由前端吞掉错误，不打扰用户。
- 普通日志可记录配置来源、host 是否来自默认/文件、SN 来源和脱敏 SN 后缀，避免完整 SN。
