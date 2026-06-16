# 契约：H3C OTA 服务与更新包

## 更新查询

**请求**

```http
POST /ota/pkg/package/upgrade/check
Content-Type: application/json
```

```json
{
  "sn": "设备序列号",
  "softwareId": "Pinvou3_Win",
  "version": "0.4.3",
  "hardwareInfo": null
}
```

**成功响应**

```json
{
  "success": true,
  "code": 200,
  "msg": "OK",
  "data": {
    "updateInfo": "更新说明",
    "updateType": 2,
    "updateVersion": "0.4.4.0",
    "pkgMd5": "BFBE3F51BEF0B52D3882B2E6A7B41B38",
    "incrPkgMd5": ""
  }
}
```

**行为约定**

- `success = true` 且 `code = 200` 视为查询成功。
- `data.updateVersion` 高于当前版本时视为有更新。
- 本 feature 优先使用完整包；增量包信息可保留但不执行。
- 查询失败不得触发下载。

## 下载信息

C# 项目中存在下载信息响应：

```json
{
  "success": true,
  "code": 200,
  "msg": "OK",
  "data": {
    "updateInfo": "更新说明",
    "updateType": 2,
    "updateVersion": "0.4.4.0",
    "pkgMd5": "BFBE3F51BEF0B52D3882B2E6A7B41B38",
    "incrPkgMd5": "",
    "pkgUrl": "https://example/pinvou3.zip",
    "incrPkgUrl": ""
  }
}
```

**行为约定**

- 若查询响应不含下载 URL，实现需要按 C# `GetDownloadInfoAsync` 对应接口获取 `pkgUrl`。
- `pkgUrl` 必须是完整包 zip 地址。
- `pkgMd5` 用于下载 zip 完成后的校验。

## 升级结果反馈

**请求**

```http
POST /ota/pkg/package/updateLog
Content-Type: application/json
```

```json
{
  "softwareIdentification": "Pinvou3_Win",
  "sn": "设备序列号",
  "currentVersion": "0.4.3",
  "updateVersion": "0.4.4.0",
  "updateErrorInfo": "",
  "updateResult": "UPGRADE_SUCCEED"
}
```

**成功响应**

```json
{
  "success": true,
  "code": 200,
  "msg": "OK"
}
```

**行为约定**

- `success = true` 且 `code = 200` 视为反馈成功。
- 反馈失败时保留本地待反馈记录。
- `updateResult` 至少覆盖：`REQUEST_UPGRADE`、`START_DOWNLOAD`、`DOWNLOAD_FAIL`、`DOWNLOAD_COMPLETED`、`START_INSTALL`、`INSTALL_COMPLETED`、`UPGRADE_SUCCEED`、`UPGRADE_FAILED`、`UNKNOWN`。

## 更新包

下载 zip 条目：

```text
OtaInfo.json
Files/Pinvou3/pinvou3_0.4.4_x64_en-US.msi
```

`OtaInfo.json` 路径：

- 当前服务端包结构使用根目录 `OtaInfo.json`。
- 代码仍可读取 `FullPack/OtaInfo.json` 路径，但下载 zip 需要直接包含 OTA 清单和安装文件。

`OtaInfo.json` 关键结构：

```json
{
  "softwareName": "Pinvou3",
  "softwareId": "Pinvou3_Win",
  "softwareVersion": "0.4.4.0",
  "softwareType": "SoftwareCollection",
  "softwareInfos": [
    {
      "softwareName": "Pinvou3",
      "softwareId": "Pinvou3_Win",
      "softwareVersion": "0.4.4.0",
      "attachData": "{\"version\":\"0.4.4.0\",\"exeName\":\"pinvou3_0.4.4_x64_en-US.msi\"}",
      "sourceDir": "Pinvou3",
      "softwareType": "Pinvou3",
      "fileMetaInfos": [
        {
          "fileName": "pinvou3_0.4.4_x64_en-US.msi",
          "filePath": "pinvou3_0.4.4_x64_en-US.msi",
          "hash": "107A415212B9E943084BB4F609E7E0C0",
          "ignoreHash": false
        }
      ]
    }
  ]
}
```

**包安全约定**

- 所有解压目标必须位于 `~/.pinvou3/updates/` 的本次更新目录内。
- 拒绝绝对路径、`..` 路径穿越、空文件名和非 UTF-8 关键路径。
- 目标安装文件必须从 `OtaInfo.json` 定位，不能仅扫描第一个 `.msi`。
- `ignoreHash = false` 时必须校验文件 hash。
