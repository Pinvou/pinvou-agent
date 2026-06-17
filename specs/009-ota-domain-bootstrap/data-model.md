# 数据模型：Windows OTA 域名引导

## WindowsOtaBootstrapConfig

表示用户可编辑的域名引导配置。

**字段**

- `bootstrapHost`：域名引导后台根地址，示例 `https://bootstrap.magic.h3c.com`。
- `source`：配置来源，取值为 `default` 或 `file`。
- `path`：配置文件路径，预期为 `~/.pinvou3/windows-ota-bootstrap.json`。

**验证规则**

- 文件不存在、为空、缺少 `bootstrapHost` 或 `bootstrapHost` 非 HTTP/HTTPS URL 时，使用默认地址。
- 文件中的 `bootstrapHost` 只有是 HTTP/HTTPS URL 时才作为自定义地址生效。
- 有效地址在参与拼接前移除末尾 `/`。
- 应用升级或重装不得主动覆盖用户修改后的文件。

## WindowsBootstrapIdentity

表示域名引导请求使用的设备身份。

**字段**

- `raw_bios_sn`：读取到的原始 BIOS SN，可为空。
- `effective_sn`：实际发送到域名引导后台的 SN。
- `source`：`bios` 或 `fallback`。
- `matched_prefix`：是否命中 `2198` 或 `2199` 前缀。

**验证规则**

- 判断前缀前必须 trim 原始 SN。
- trim 后以 `2198` 或 `2199` 开头时，`effective_sn = raw_bios_sn.trim()`。
- 其他前缀、空值、读取失败时，`effective_sn = 219904A17T4257W00018`。
- 普通错误提示不展示完整 `raw_bios_sn` 或 `effective_sn`。

## DomainBootstrapRequest

表示发送给域名引导后台的请求体。

**字段**

- `device_id`：`WindowsBootstrapIdentity.effective_sn`。
- `product_id`：固定值 `61de63cd22271b82ccd9e1bc258b55e0`。
- `timestamp`：当前 Unix 毫秒时间戳字符串。
- `sign_type`：固定值 `0`。
- `sign`：签名字符串。

**验证规则**

- `timestamp` 参与签名，签名和请求体必须使用同一个时间戳。
- `sign` 为指定拼接串的 UTF-8 MD5 小写十六进制值。
- 请求路径固定为 `/v2/bootstrap`。

## DomainBootstrapResult

表示域名引导后台返回的服务地址集合。

**字段**

- `success`：后台是否成功。
- `code`：后台状态码。
- `message`：后台消息，兼容 `msg` 或 `message`。
- `urls`：服务 key 到 URL 的字典。
- `smarthub_ota`：从 `urls` 中大小写不敏感查找到的 OTA 后台地址。

**验证规则**

- 响应失败、`data` 缺失或 `data` 不是对象时，域名引导失败。
- 成功状态兼容正式服务返回的 `code = 0`，也兼容测试 mock 或旧约定中的 `code = 200`；`success = false` 始终按失败处理。
- `smarthubOta` 缺失或为空时，域名引导失败。
- `smarthubOta` 必须是 HTTP/HTTPS URL。
- 查找 key 时大小写不敏感，但输出保持调用方使用的标准 key 语义。

## WindowsOtaConfig

表示 Windows OTA 流程实际使用的后端配置。

**字段**

- `ota_host`：域名引导解析出的 OTA 后台根地址。
- `software_id`：默认 `Pinvou3_Win`。
- `sn`：本次 OTA 查询使用的有效 SN。
- `current_version`：当前应用版本。

**验证规则**

- `ota_host` 必须来自成功的域名引导结果。
- `check`、`getDownloadInfo` 和 `updateLog` endpoint 均基于 `ota_host` 拼接。
- `ota_host` 不应回退到旧固定 OTA 默认地址。

## UpdateInfo 扩展

表示前端已有更新信息结构中需要新增或透传的 Windows OTA 来源信息。

**字段**

- `ota_host`：Windows OTA 查询解析出的 OTA host。非 Windows 可为空。
- 既有字段：`available`、`current_version`、`latest_version`、`url`、`package_md5`、`software_id`、`sn`、`update_type`、`platform`。

**验证规则**

- Windows `available = true` 时，`ota_host` 必须非空且有效。
- `available = false` 但查询成功时，`ota_host` 可保留用于诊断或本次流程上下文。
- 非 Windows 平台序列化兼容，新增字段必须有默认值。

## UpdateFeedbackRecord 扩展

表示 MSI 启动前写入、升级后读取并反馈的持久化记录。

**字段**

- `ota_host`：本次升级查询时使用的 OTA 后台地址。
- 既有字段：`software_identification`、`sn`、`current_version`、`update_version`、`update_result`、`update_error_info`、`installer_path`、`created_at`、`last_attempt_at`、`attempts`、`reported`。

**状态转换**

```text
check_available
  -> download_prepared
  -> install_started
  -> pending_report
  -> reported | report_failed_retryable
```

**验证规则**

- 启动 MSI 前必须写入 `ota_host`。
- 升级后反馈优先使用记录中的 `ota_host`。
- 旧版本记录缺少 `ota_host` 时，可重新执行域名引导作为兼容路径。
- 反馈成功后清理记录；反馈失败保留记录和重试计数。
