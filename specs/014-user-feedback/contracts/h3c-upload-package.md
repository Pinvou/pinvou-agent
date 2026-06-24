# 契约：H3CLogCollector 兼容上传包

## 来源

兼容参考：`D:\0_Projects\01_全家桶\H3CLogCollector\LogCollector.Business\WebService\LogWebService.cs`

## 反馈包目录

```text
~/.pinvou3/feedback/pending/<feedback_id>/
├── manifest.json
├── description.txt
└── attachments/
    ├── 001-image.png
    └── 002-video.mp4
```

## manifest.json

```json
{
  "schema_version": "1.0",
  "feedback_id": "fb-20260624-153012-a8f3",
  "created_at": "2026-06-24T15:30:12+08:00",
  "type": "issue",
  "title": "上传附件后没有响应",
  "description": "我点击提交后按钮一直转圈，没有看到成功或失败提示。",
  "entry_point": "settings",
  "privacy_notice_version": "2026-06-24",
  "app_context": {
    "app_version": "0.4.9",
    "os": "Windows 11",
    "arch": "x86_64",
    "language": "zh-Hans",
    "entry_point": "settings",
    "error_summary": null,
    "timestamp": "2026-06-24T15:30:12+08:00"
  },
  "attachments": [
    {
      "id": "att-001",
      "original_name": "feedback.png",
      "package_name": "attachments/001-image.png",
      "media_type": "image",
      "mime": "image/png",
      "size_bytes": 482901,
      "sha256": "..."
    }
  ]
}
```

## 打包流程

1. 将反馈目录打包为 `<feedback_id>.tar.gz`。
2. 读取 `tar.gz` 的全部字节。
3. 对每个字节执行 XOR `0x55`。
4. 写入 `<feedback_id>.dbg`。
5. 上传成功后删除 `tar.gz` 和 `.dbg`。

## 获取 token

请求：

```http
POST http://sohord10.h3c.com/rest/ihomers/uploadRequest
Content-Type: application/json

{"gwSn":"<device-serial-number>"}
```

响应成功条件：

```json
{
  "retCode": 0,
  "retString": "<token>"
}
```

## checkCode

按 H3CLogCollector 现有逻辑计算：

1. 将设备序列号按相邻字符交换。例如 `abcdef` 变为 `badcfe`；奇数长度时最后一个字符保持在其 pair 处理结果中。
2. 拼接 `token + swapped_sn`。
3. 对拼接结果计算 MD5。
4. 转成小写十六进制字符串。

## 上传请求

```http
PUT https://magic.h3c.com/rest/ihomers/uploadSysinfoFile
GwSn: <device-serial-number>
FileName: <feedback_id>.dbg
checkCode: <md5-token-check>
Content-Type: application/octet-stream

<dbg binary bytes>
```

响应成功条件：

```json
{
  "retCode": 0
}
```

## 错误映射

- token 请求失败：`failed_retryable`，提示接收通道暂不可用。
- 设备序列号缺失：`failed_validation`，提示当前设备缺少上传通道所需标识。
- 上传返回非 `retCode = 0`：`failed_retryable`。
- 本地文件读取、复制、打包失败：`failed_validation` 或 `failed_retryable`，按是否需要用户修正区分。

## 安全与清理

- `.dbg` 仅作为上传暂存，不在成功后保留。
- `manifest.json` 不保存附件原始绝对路径。
- 失败待重试目录留在 `~/.pinvou3/feedback/pending/`，用户主动重试或取消后再清理。
