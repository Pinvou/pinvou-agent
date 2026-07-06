# 数据模型：ASR 模型可选下载

## ASR 模型包

代表语音识别所需的大体积模型文件。

**字段**
- `id`：模型标识，例如 `sensevoice-q8`。
- `filename`：模型文件名，例如 `sensevoice-small-q8.gguf`。
- `version`：模型版本或发布批次，用于后续更新判断。
- `expected_size`：期望文件大小，用于快速校验和展示下载量。
- `sha256`：期望摘要，用于完整性校验。
- `download_url`：模型下载来源，可由环境变量覆盖以支持内网镜像。
- `local_path`：本机落点，默认位于 `~/.pinvou3/asr/`。
- `status`：当前可用状态。

**验证规则**
- `filename` 必须与当前平台支持的 ASR runtime 兼容。
- 下载完成后必须通过大小和摘要校验才能进入 `available`。
- 校验失败的文件不得被启用。

## ASR Runtime 组件

代表本地执行语音识别需要的小体积程序文件。

**字段**
- `wrapper_path`：固定命令入口，例如 Windows 的 `pinvou-asr.exe`。
- `backend_path`：实际后端程序，例如 `llama-funasr-sensevoice.exe`。
- `bundled`：是否来自安装包资源。
- `available`：wrapper/backend 是否满足当前平台运行条件。

**验证规则**
- Windows 主包允许 runtime 存在而模型缺失。
- runtime 缺失时不得触发模型下载后直接标记 ready，应提示修复或重新安装 runtime。
- Linux 保持现有 `sense-voice-main` resource fallback 与用户目录优先级。

## 模型获取任务

代表一次由用户确认触发的模型下载/校验流程。

**字段**
- `task_id`：本次下载任务标识。
- `stage`：`start`、`ffmpeg`、`model`、`verify`、`done`、`failed`、`cancelled`。
- `downloaded`：已下载字节数。
- `total`：预计总字节数。
- `temp_path`：下载中的 `.part` 文件。
- `final_path`：校验通过后的模型路径。
- `error`：失败原因，面向用户展示时不得包含音频内容。

**状态转换**
```text
not_installed -> downloading -> verifying -> available
downloading -> failed -> not_installed
downloading -> cancelled -> not_installed
available -> invalid -> not_installed
available -> removed -> not_installed
```

## 模型状态

前端通过状态决定是否展示安装框、进度和语音入口可用性。

**字段**
- `engine`：runtime 是否可用。
- `ffmpeg`：当前平台是否需要且具备音频转码能力。
- `model`：模型是否存在且通过校验。
- `ready`：语音能力是否可直接使用。
- `installable`：当前状态是否允许应用内补全依赖。
- `missing`：缺失项列表，例如 `model`、`engine`、`ffmpeg`。

**约束**
- `ready = engine && model && 平台所需转码能力满足`。
- 缺模型但 runtime 可用时，Windows 应返回 `installable = true`。
- 缺 runtime 时，Windows 可以继续提示修复/重装或安装离线 runtime 包，不应误报为仅缺模型。
