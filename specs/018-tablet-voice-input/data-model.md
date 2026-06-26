# 数据模型：平板语音输入强化

## VoicePrimaryAction（主语音入口）

表示触屏/平板体验中新增的醒目语音输入按钮。

- `visible`：是否显示主语音入口。
- `mode`：入口模式，取值为 `compact`、`tablet_primary`。
- `status`：关联语音输入状态，取值为 `idle`、`requesting_permission`、`recording`、`transcribing`、`completed`、`cancelled`、`failed`。
- `label`：可访问名称和悬浮说明，应能表达开始录音、结束录音、处理中或重试。
- `touch_target`：触控目标尺寸是否满足平板可点按要求。
- `disabled_reason`：不可用原因，例如正在识别、设备不可用或 bridge 不可用。

### 状态规则

- `idle` 时点击主语音入口应开始语音输入。
- `recording` 时点击主语音入口应结束录音并进入识别流程。
- `requesting_permission` 或 `transcribing` 时主入口应显示忙碌状态，避免重复触发。
- `failed` 时主入口应允许重试，且错误提示不能阻塞文本输入。

## ComposerDraft（输入草稿）

表示输入框当前可编辑内容。

- `text`：输入框文本。
- `trimmed_text`：去除首尾空白后的文本，用于判断是否可发送。
- `has_text`：`trimmed_text` 是否非空。
- `has_ready_attachment`：是否存在已解析完成、可随消息发送的附件。
- `source`：草稿来源，取值为 `typed`、`voice_result`、`prefill`、`mixed`。

### 验证规则

- 仅空白字符视为空草稿。
- 语音识别结果写回后必须保持可编辑。
- 清除草稿只清空 `text`，不得移除附件或中断语音入口。

## ComposerActions（输入区操作）

表示输入框附近的发送、清除、附件、语音等操作集合。

- `send_visible`：发送按钮是否显示。
- `send_enabled`：发送按钮是否可用。
- `clear_visible`：清除按钮是否显示。
- `clear_enabled`：清除按钮是否可用。
- `voice_visible`：语音入口是否显示。
- `voice_enabled`：语音入口是否可用。

### 派生规则

- 当 `has_text` 或 `has_ready_attachment` 为真时，发送按钮可见且可用。
- 当 `has_text` 为真时，清除按钮可见且可用。
- 当 `has_text` 为假时，清除按钮不应占据主要操作区域。
- 当语音正在录音或识别时，发送和清除按钮的状态应避免造成文本丢失或并发录音。

## DeviceExperienceMode（设备体验模式）

表示当前 UI 是否应启用平板语音强化。

- `is_touch_capable`：当前环境是否表现为触控可用。
- `is_tablet_sized`：当前窗口是否接近平板尺寸或平板布局。
- `orientation`：横屏或竖屏。
- `effective_mode`：最终体验模式，取值为 `desktop` 或 `tablet_touch`。

### 派生规则

- `tablet_touch` 模式应显示主语音入口。
- `desktop` 模式应优先保持现有紧凑输入区，不得强制改变桌面工作流。
- 模式变化时不得清空输入草稿或取消正在进行的语音输入，除非用户主动取消。

## VoiceFeedbackNotice（语音反馈提示）

表示录音、识别、失败或完成时的用户反馈。

- `visible`：是否显示提示。
- `tone`：提示类型，取值为 `info`、`active`、`success`、`error`。
- `message`：用户可读文案。
- `actions`：提示中可用动作，例如取消、重试、关闭。

### 状态规则

- `recording` 必须有可见录音中反馈。
- `transcribing` 必须有可见处理中反馈。
- `failed` 必须显示可恢复说明，并允许文本输入继续使用。
- `completed` 可以短暂提示识别文本已写入输入框。
