# 契约：ASR Runtime 状态与下载

本契约描述前端与 Tauri 后端之间的现有交互，计划阶段不新增并行接口；如实现需要扩展，必须保持旧字段兼容。

## Command：`voice_asr_status`

**用途**：查询本机 ASR runtime、模型和依赖是否满足语音输入要求。

**返回**
```json
{
  "engine": true,
  "ffmpeg": true,
  "model": false,
  "ready": false,
  "installable": true,
  "missing": ["model"]
}
```

**规则**
- `ready = true` 时前端可以直接进入录音和转写流程。
- `ready = false` 且 `installable = true` 时前端展示安装/下载入口。
- `missing` 必须能区分 `model` 与 `engine`，避免把可下载模型缺失误报为必须重装应用。
- Windows 模型移出主包后，runtime 可用但模型缺失时应返回 `engine = true`、`model = false`、`installable = true`。

## Command：`install_voice_asr`

**用途**：在用户确认后补全本地 ASR 依赖。Linux 保持现有安装 ffmpeg + 下载模型流程；Windows 下载模型到用户目录。

**返回**
```json
{
  "engine": true,
  "ffmpeg": true,
  "model": true,
  "ready": true,
  "installable": true,
  "missing": []
}
```

**失败**
- 网络不可用：返回用户可理解的下载失败原因。
- 校验失败：删除或隔离 `.part`/损坏文件，返回可重试状态。
- runtime 缺失：提示修复安装或安装 runtime，不得假装模型下载可解决。
- 磁盘不足：返回磁盘空间不足提示，保留可恢复状态。

## Event：`voice_asr:progress`

**用途**：下载和安装过程中向前端推送进度。

**Payload**
```json
{
  "stage": "model",
  "downloaded": 10485760,
  "total": 254208320
}
```

**阶段**
- `start`：用户已确认，准备开始。
- `ffmpeg`：Linux 正在补全 ffmpeg。
- `model`：模型下载中。
- `verify`：模型校验中。
- `done`：状态已刷新，语音可用性以最终 status 为准。
- `failed`：流程失败，可重试。
- `cancelled`：用户取消，临时文件已清理或可安全覆盖。

## Command：`transcribe_voice_audio`

**用途**：已有语音转写入口，模型可选下载后不改变请求形态。

**规则**
- 如果 `voice_asr_status.ready = false`，前端应先展示安装/下载入口，不应直接开始录音。
- 后端仍需在转写前兜底检查 runtime 和模型，避免前端状态过期导致错误使用。
- 错误信息不得包含用户音频内容。
