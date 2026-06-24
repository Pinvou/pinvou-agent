# 数据模型：修复 Windows 语音输入

## VoiceInputSession（语音输入会话）

表示一次从用户触发语音输入到完成、取消或失败的交互。

**字段**
- `id`：本次语音输入的临时标识。
- `session_id`：启动语音输入时所在的聊天会话。
- `draft_before_start`：启动前输入框已有文本，用于取消或失败时保留。
- `state`：当前状态，见状态流转。
- `started_at`：启动时间。
- `completed_at`：完成、取消或失败时间。
- `error`：失败时的错误类别和用户可见说明。

**状态流转**
- `idle` → `requesting_permission`
- `requesting_permission` → `recording`
- `requesting_permission` → `failed`
- `recording` → `transcribing`
- `recording` → `cancelled`
- `transcribing` → `completed`
- `transcribing` → `failed`
- 任意活动状态 → `cancelled`

**验证规则**
- 活动状态同一时间最多存在一个。
- `completed` 必须包含非空或明确为空的识别结果。
- 结果回填前必须校验 `session_id` 仍匹配目标上下文，或以安全方式提示用户结果未自动写入。

## MicrophonePermissionState（麦克风权限状态）

表示 Windows/Tauri/WebView 对录音设备访问的授权状态。

**字段**
- `state`：`unknown`、`allowed`、`denied`、`unavailable`。
- `message`：用户可见提示。
- `next_action`：建议操作，例如开启系统麦克风权限、检查录音设备、重试。

**验证规则**
- `denied` 和 `unavailable` 必须提供 `next_action`。
- 权限状态未知时不得静默失败，必须进入请求或诊断流程。

## VoiceRecognitionResult（语音识别结果）

表示一次语音转文本输出。

**字段**
- `session_id`：关联的启动会话。
- `text`：识别文本。
- `is_empty`：是否为空文本。
- `source`：结果来源，用于诊断，例如现有底座 voice capture 或系统录音链路。
- `diagnostics`：可选诊断信息，供调试和日志使用。

**验证规则**
- 成功结果如果为空，必须提示用户未识别到内容，而不是当作无响应。
- 结果不得跨会话自动写入。

## VoiceDiagnosticEvent（语音诊断事件）

用于定位 Windows 语音输入问题的轻量诊断事件。

**字段**
- `stage`：`permission`、`device`、`recording`、`transcribing`、`writeback`。
- `level`：`info`、`warn`、`error`。
- `message`：面向开发者的简短诊断说明。
- `user_message`：需要展示给用户时的本地化提示。

**验证规则**
- 错误事件必须能映射到用户可见失败提示。
- 不记录原始音频、密钥、完整路径或敏感会话内容。
