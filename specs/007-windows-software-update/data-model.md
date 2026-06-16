# 数据模型：Windows 软件更新

## UpdateInfo

代表一次更新查询的用户可见结果。

**字段**

- `available`：是否存在适用于当前应用的更新。
- `current_version`：当前应用版本。
- `latest_version`：目标更新版本。
- `update_type`：更新类型，兼容 H3C `Silent`、`Force`、`Normal`。
- `notes`：更新说明，对应 H3C `updateInfo`。
- `package_url`：完整包下载地址。
- `package_md5`：完整包 MD5 校验值。
- `package_size`：包大小，未知时为 0。
- `software_id`：软件标识，Windows Pinvou3 预期为 `Pinvou3_Win`。
- `sn`：设备序列号。

**验证规则**

- `available = true` 时必须有 `latest_version`、`package_url` 和 `software_id`。
- `latest_version` 必须高于 `current_version` 才能触发更新。
- `package_url` 只允许 HTTP/HTTPS。

## DownloadedPackage

代表已下载到本地的完整更新 zip 包。

**字段**

- `path`：zip 文件绝对路径。
- `version`：目标版本。
- `md5`：实际或期望 MD5。
- `downloaded_bytes`：已下载字节数。
- `total_bytes`：总字节数，未知时为 0。
- `status`：`pending`、`downloading`、`completed`、`cancelled`、`failed`。

**验证规则**

- `path` 必须位于 `~/.pinvou3/updates/`。
- 下载完成后若存在期望 MD5，必须校验一致。
- 取消或失败时应删除半成品，除非明确保留用于调试。

## OtaPackageInfo

代表下载 zip 中 `OtaInfo.json` 的软件集合。

**字段**

- `softwareName`：集合或软件名称。
- `softwareId`：集合或软件标识。
- `softwareVersion`：集合或软件版本。
- `softwareType`：软件类型。
- `softwareInfos`：子软件列表。

**验证规则**

- 必须能找到 `softwareId = Pinvou3_Win` 或 `softwareType = Pinvou3` 的子软件信息。
- 子软件版本应与更新目标版本一致，或至少不低于当前版本。

## PackageSoftwareInfo

代表 `OtaInfo.json` 中的单个可安装软件。

**字段**

- `softwareName`：软件名称。
- `softwareId`：软件标识。
- `softwareVersion`：软件版本。
- `softwareType`：软件类型。
- `sourceDir`：安装资源目录，样例为 `Pinvou3`。
- `fileMetaInfos`：文件元信息列表。
- `attachData`：附加信息，可能包含字符串化 JSON，例如 `exeName`。

**验证规则**

- 目标软件必须能解析出一个 `.msi` 文件。
- `sourceDir` 和 `fileMetaInfos.filePath` 组合后的路径必须仍位于本次解压目录内。
- 若 `ignoreHash = false`，安装文件 hash 必须与清单一致。

## InstallerFile

代表准备启动的 Windows 安装文件。

**字段**

- `path`：`MSI` 绝对路径。
- `version`：目标版本。
- `source`：来源包和清单位置。
- `hash`：文件 hash。

**验证规则**

- `path` 必须位于 `~/.pinvou3/updates/` 的本次解压目录内。
- 扩展名必须为 `.msi`。
- 文件必须存在且可读。

## UpdateFeedbackRecord

代表跨进程保存的待反馈升级结果。

**字段**

- `software_identification`：软件标识。
- `sn`：设备序列号。
- `current_version`：升级前版本。
- `update_version`：目标版本。
- `update_result`：`REQUEST_UPGRADE`、`START_DOWNLOAD`、`DOWNLOAD_FAIL`、`DOWNLOAD_COMPLETED`、`START_INSTALL`、`INSTALL_COMPLETED`、`UPGRADE_SUCCEED`、`UPGRADE_FAILED` 或 `UNKNOWN`。
- `update_error_info`：失败或未知原因。
- `created_at`：记录创建时间。
- `last_attempt_at`：最近一次反馈时间。
- `attempts`：反馈尝试次数。
- `reported`：是否已成功反馈。

**状态转换**

```text
none
  -> query_available
  -> download_started
  -> download_completed | download_failed | cancelled
  -> install_started
  -> awaiting_result
  -> reported | report_failed_retryable
```

**验证规则**

- 启动安装器前必须写入 `START_INSTALL` 或 `REQUEST_UPGRADE` 类记录。
- 反馈成功后才能标记 `reported = true`。
- 反馈失败时必须保留记录和失败原因。
