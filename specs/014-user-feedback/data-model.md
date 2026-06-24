# 数据模型：我要反馈

## FeedbackDraft（反馈草稿）

用户在前端填写但尚未提交的内容。

**字段**：
- `type`: 反馈类型。枚举：`issue`、`suggestion`。
- `title`: 可选标题，最多 120 个字符。
- `description`: 必填文字说明，1 到 5000 个字符。
- `attachments`: `FeedbackAttachmentDraft[]`，最多 5 个。
- `entry_point`: 入口来源，例如 `settings`、`error_banner`。
- `error_summary`: 可选错误摘要，仅来自用户触发的上下文入口。

**验证规则**：
- `description` 去除空白后不能为空。
- `type` 必须为已知枚举值。
- 存在未提交内容时，关闭反馈界面需要确认。

## FeedbackAttachmentDraft（附件草稿）

用户选择的本地图片或短视频。

**字段**：
- `path`: 本地文件路径，仅传给 Tauri 命令，不写入最终 `manifest.json` 的完整原始路径。
- `name`: 文件名。
- `media_type`: `image` 或 `video`。
- `mime`: 识别到的 MIME 类型。
- `size_bytes`: 文件大小。

**验证规则**：
- 图片扩展名：`png`、`jpg`、`jpeg`、`gif`、`webp`。
- 视频扩展名：`mp4`、`mov`、`webm`。
- 单图片不超过 10 MB。
- 单视频不超过 50 MB。
- 全部附件合计不超过 80 MB。
- 不允许目录、快捷方式或不存在的文件。

## FeedbackPackage（反馈包）

Tauri 后端生成的待上传目录。

**字段**：
- `feedback_id`: 本地生成的提交 ID，建议格式 `fb-YYYYMMDD-HHMMSS-<short-random>`。
- `created_at`: ISO 8601 本地提交时间。
- `type`: 反馈类型。
- `title`: 可选标题。
- `description`: 用户填写的文字说明。
- `entry_point`: 入口来源。
- `app_context`: `AppContext`。
- `attachments`: `FeedbackAttachment[]`。
- `privacy_notice_version`: 用户看到的隐私提示版本。

**目录结构**：
```text
<feedback_id>/
├── manifest.json
├── description.txt
└── attachments/
    ├── 001-image.png
    └── 002-video.mp4
```

**验证规则**：
- `manifest.json` 不保存附件原始绝对路径。
- `description.txt` 与 `manifest.json.description` 内容一致或为其纯文本副本。
- 只复制用户选择的附件，不扫描目录。

## FeedbackAttachment（反馈附件）

反馈包内的附件条目。

**字段**：
- `id`: 包内附件 ID，例如 `att-001`。
- `original_name`: 原文件名，不含完整路径。
- `package_name`: 包内文件名。
- `media_type`: `image` 或 `video`。
- `mime`: MIME 类型。
- `size_bytes`: 文件大小。
- `sha256`: 文件内容摘要，用于团队侧核对。

**验证规则**：
- `package_name` 必须位于 `attachments/` 下。
- 文件大小与 `manifest.json` 中记录一致。

## AppContext（应用环境摘要）

帮助定位问题的非敏感上下文。

**字段**：
- `app_version`: pinvou app 版本。
- `os`: 操作系统名称和版本。
- `arch`: CPU 架构。
- `language`: UI 语言。
- `entry_point`: 提交入口。
- `error_summary`: 可选错误摘要。
- `timestamp`: 生成摘要时间。

**隐私边界**：
- 不包含聊天正文。
- 不包含用户上传给聊天的文件正文。
- 不包含用户 home 目录完整文件路径。
- 不包含模型 API key、搜索 API key 或其他凭据。

## H3CUploadEnvelope（H3C 上传封装）

由反馈包目录生成的上传文件与请求元数据。

**字段**：
- `source_dir`: 反馈包目录。
- `tar_gz_path`: 临时 `tar.gz` 路径。
- `dbg_path`: XOR 后 `.dbg` 路径。
- `gw_sn`: 设备序列号或配置覆盖值。
- `file_name`: 上传文件名。
- `check_code`: 按 H3CLogCollector 方式计算的校验值。

**状态流转**：
```text
Draft -> Validated -> Packaged -> Uploading -> Submitted
                                  └──────────> FailedRetryable
Validated -> FailedValidation
Uploading -> FailedPermanent
```

**清理规则**：
- `Submitted` 后删除附件副本和 `.dbg` 临时文件，仅保留 `receipt.json` 与必要摘要。
- `FailedRetryable` 保留反馈包目录，供用户在当前界面或后续重试。
- `FailedValidation` 不生成上传文件。

## FeedbackReceipt（提交回执）

提交命令返回给前端的结果。

**字段**：
- `feedback_id`: 本地提交 ID。
- `submitted_at`: 成功提交时间。
- `status`: `submitted`、`failed_retryable` 或 `failed_validation`。
- `message`: 用户可读提示。
- `retryable`: 是否可重试。

**验证规则**：
- 成功时必须有 `submitted_at`。
- 失败时必须有用户可理解的 `message`。
