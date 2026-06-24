# 契约：反馈 Tauri 命令

## 命令

`submit_feedback`

## 调用方

`pinvou3-app/src/tauri-bridge.js` 从设置页反馈表单调用。

## 请求

```json
{
  "type": "issue",
  "title": "上传附件后没有响应",
  "description": "我点击提交后按钮一直转圈，没有看到成功或失败提示。",
  "entry_point": "settings",
  "error_summary": null,
  "attachments": [
    {
      "path": "C:\\Users\\me\\Pictures\\feedback.png",
      "name": "feedback.png",
      "media_type": "image",
      "mime": "image/png",
      "size_bytes": 482901
    }
  ],
  "privacy_notice_version": "2026-06-24"
}
```

## 字段规则

- `type` 必须是 `issue`、`suggestion` 之一。
- `description` 必填，去除空白后长度 1 到 5000。
- `title` 可选，最长 120。
- `entry_point` 必填，首版使用 `settings` 或 `error_banner`。
- `attachments` 最多 5 个。
- `attachments[].path` 只用于后端读取和复制文件，不写入最终反馈包 manifest。
- 前端可传 `mime` 和 `size_bytes` 用于即时显示；后端必须重新验证真实文件。

## 成功响应

```json
{
  "feedback_id": "fb-20260624-153012-a8f3",
  "status": "submitted",
  "submitted_at": "2026-06-24T15:30:18+08:00",
  "message": "反馈已提交，感谢你的帮助。",
  "retryable": false
}
```

## 可重试失败响应

命令返回 `Ok`，但状态为 `failed_retryable`，便于前端保留表单并展示重试按钮。

```json
{
  "feedback_id": "fb-20260624-153012-a8f3",
  "status": "failed_retryable",
  "submitted_at": null,
  "message": "当前无法连接反馈接收通道，请稍后重试。",
  "retryable": true
}
```

## 校验失败

命令返回 `Err(String)`，前端保持表单内容并展示错误。

示例：

```text
视频文件超过 50 MB，请压缩后再上传。
```

## 前端状态

- `idle`: 初始状态。
- `validating`: 本地附件校验中。
- `submitting`: 后端打包和上传中，提交按钮禁用。
- `submitted`: 展示成功回执。
- `failed_retryable`: 展示失败原因和重试按钮。
- `failed_validation`: 高亮需要用户修正的字段。

## UI 落点

- 主入口：设置页内新增“帮助与反馈”区域，按钮文案“我要反馈”。
- 补充入口：关键错误提示中可显示“反馈此问题”，并预填 `entry_point = "error_banner"` 与 `error_summary`。
- 表单控件：反馈类型分段控件、标题输入、说明文本框、附件选择/删除、隐私提示、提交/取消按钮。

## 隐私要求

- 提交前必须显示简短提示：反馈会发送用户填写内容、所选附件和非敏感环境摘要。
- 不自动附带聊天记录、用户文件正文、模型密钥、搜索密钥或完整敏感路径。
