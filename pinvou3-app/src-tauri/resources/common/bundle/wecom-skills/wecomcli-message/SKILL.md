---
name: wecomcli-message
description: 何时用:仅当用户明确指向企业微信(给企微同事/群发消息、查企微会话)时使用;泛指发消息默认走本地通讯工具,邮件走 wecomcli-email。查询可发消息的聊天会话范围,向单聊/群聊发送文本、Markdown、图片、文件、语音、视频消息。
metadata:
  requires:
    bins: ["wecom-cli"]
---

# 企业微信发送消息

> 执行任何 `wecom-cli` 命令前，必须先读取并完成 `wecomcli-shared` 技能的公共前置检查。

1. 可以向授权人发送消息。
2. 可以向授权人以外的、机器人最近有消息往来的聊天会话（单聊和群聊）发送消息。

## 适用范围

### 适用

- 适用于给授权人发消息，使用 `wecom-cli identity whoami` 获取授权人ID，可作为 `chat_id` 使用，无需调用 `sessions list`。
- 适用于查询当前有权限发送消息的聊天会话范围并给这些范围中的成员或群聊发送 Markdown 消息、图片、文件、AMR 语音或视频

### 不适用

- 发送对象不是授权人且不在本次 `sessions list` 返回结果中 → 告知用户当前只能向最近活跃的会话或授权人发送

## 技能依赖

调用依赖技能前，必须先完整读取对应 `SKILL.md`。

| 依赖技能 | 触发场景 | 数据流向 |
|---|---|---|
| `wecomcli-media` | 发送图片、文件、语音或视频时只有本地文件路径，没有可直接复用的 `media_id` | 包含媒体上传接口，如没有已有的 `media_id`，必须先阅读该技能获取 `media_id`。`media upload` 以本地 `file_path` 上传，`type` 为可选入参（`image`/`voice`/`video`/`file`）；发送时以上传响应返回的 `type` 对齐 `msg_type` |

## 获取能发送消息的会话列表

### 命令

```bash
wecom-cli message aibot sessions list
```

### 返回

| 字段 | 类型 | 说明 |
|---|---|---|
| `sessions` | array | 会话列表，按最后一条消息时间从新到旧排序，具体数量以实际回包为准 |
| `sessions[].chat_id` | string | 会话 ID |
| `sessions[].chat_name` | string | 群名称或单聊名称 |
| `sessions[].chat_type` | string | `single` 单聊或 `group` 群聊 |
| `sessions[].last_msg_time` | string | 最后一条消息时间，格式 `YYYY-MM-DD HH:MM:SS` |
| `sessions_count` | integer | `sessions` 数组元素数量 |

### `chat_id` 来源

向授权人以外的用户发送消息，调用 `wecom-cli message aibot send` 前，需要先调用一次 `sessions list`，然后从本次返回的 `sessions[]` 中选定目标项，把该项的 `chat_id` 原样复制到 `send.chat_id`。

以下值都不能直接作为 `send.chat_id`：

- 用户输入的 ID
- 之前轮次或历史上下文保存的 `chat_id`
- `wecomcli-contact` 返回的 `userid`
- 根据姓名、群名或其他字段自行构造的值

这些值最多只能作为匹配线索；最终发送参数必须重新取自本次 `sessions list` 的匹配项。

### 目标会话匹配

- **聊天名称**：在本次 `sessions[]` 中按非空 `chat_name` 精确匹配；不能精确匹配需要向用户反问确认发送目标，唯一命中时从匹配项复制 `chat_id`。
- **最近第一个/最近某个会话**：按 `sessions[]` 原始顺序选择用户明确指定的项。
- **用户提供 ID**：只能与本次 `sessions[].chat_id` 做完全相等校验；命中后仍从匹配项复制 `chat_id`，不能直接复用用户输入值。

匹配结果处理：

- 唯一匹配时继续发送。
- 多个聊天会话候选时，按返回顺序展示聊天名和最后消息时间，让用户选择。
- 用户完成选择后，必须重新调用 `sessions list`，再用选定对象匹配当次返回值。
- 无匹配时停止发送，如实告知目标不在本次返回的最近会话中；不要接受外部 `chat_id` 绕过限制。
- `sessions_count=0` 时停止发送，告知当前没有可发送的最近会话。
- 展示会话列表时保持接口原始顺序；展示名称和时间，不展示内部 `chat_id`。

## 发送消息

### 前置条件

调用本接口前必须完成以下步骤：

1. 根据发送对象选择调用 `wecom-cli message aibot sessions list`获取 `chat_id` 或 `wecom-cli identity whoami` 获取授权人ID。
2. 在本次列表中唯一匹配目标。
3. 如果发送授权人以外的对象，从列表中匹配项原样复制 `sessions[].chat_id`。
4. 目标是媒体消息时，再准备对应的 `media_id`。

在目标会话匹配成功前，不上传媒体，也不调用 `send`。

### 命令

```bash
wecom-cli message aibot send --json '<JSON 参数>'
```

### 公共参数

| 字段 | 类型 | 必填 | 说明 |
|---|---|:---:|---|
| `chat_id` | string | 是 | 必须取自 `wecom-cli identity whoami` 或当前发送流程中刚调用的 `sessions list` 返回的目标 `sessions[].chat_id` |
| `msg_type` | string | 是 | `markdown` / `image` / `file` / `voice` / `video` |
| `markdown` | object | 条件必填 | 仅 `msg_type="markdown"` 时传 |
| `image` | object | 条件必填 | 仅 `msg_type="image"` 时传 |
| `file` | object | 条件必填 | 仅 `msg_type="file"` 时传 |
| `voice` | object | 条件必填 | 仅 `msg_type="voice"` 时传 |
| `video` | object | 条件必填 | 仅 `msg_type="video"` 时传 |

每次请求必须且只能携带一个与 `msg_type` 同名的内容对象。不要传空对象，也不要同时传多个消息对象。

### Markdown 消息

`markdown.content` 必填，最长 20480 UTF-8 字节。普通文本也按 Markdown 发送。

```bash
wecom-cli message aibot send --json '{
  "chat_id": "<本次 sessions[].chat_id>",
  "msg_type": "markdown",
  "markdown": {
    "content": "<markdown 消息内容>"
  }
}'
```

### 图片消息

`image.media_id` 必填，必须来自媒体上传接口上传后返回 `type=image` 的结果。

```bash
wecom-cli message aibot send --json '{
  "chat_id": "<本次 sessions[].chat_id>",
  "msg_type": "image",
  "image": {
    "media_id": "<media_id>"
  }
}'
```

### 文件消息

`file.media_id` 必填，必须来自媒体上传接口上传后返回 `type=file` 的结果；文件名取上传时的原始文件名。

```bash
wecom-cli message aibot send --json '{
  "chat_id": "<本次 sessions[].chat_id>",
  "msg_type": "file",
  "file": {
    "media_id": "<media_id>"
  }
}'
```

### 语音消息

`voice.media_id` 必填，必须来自媒体上传接口上传后返回 `type=voice` 的结果；源文件仅支持 AMR 格式，不能只改扩展名冒充 AMR。

```bash
wecom-cli message aibot send --json '{
  "chat_id": "<本次 sessions[].chat_id>",
  "msg_type": "voice",
  "voice": {
    "media_id": "<media_id>"
  }
}'
```

### 视频消息

| 字段 | 必填 | 说明 |
|---|:---:|---|
| `video.media_id` | 是 | 来自媒体上传接口上传后返回 `type=video` 的结果 |
| `video.title` | 否 | 最长 128 UTF-8 字节；省略时使用上传时的原始文件名 |
| `video.description` | 否 | 最长 512 UTF-8 字节；省略时不展示描述 |

```bash
wecom-cli message aibot send --json '{
  "chat_id": "<本次 sessions[].chat_id>",
  "msg_type": "video",
  "video": {
    "media_id": "<media_id>",
    "title": "产品演示",
    "description": "本周版本的核心功能演示"
  }
}'
```

用户没有提供视频标题或描述时直接省略对应字段，不传空字符串，也不追问非必填字段。

## 关键约束

- 用户明确要求发送且目标与内容完整时直接执行，不重复追问确认；缺少目标、内容或本地文件时只追问缺失项。
- 连续发送多条时，不用每次 `send` 前都重新调用 `sessions list` 或 `wecom-cli identity whoami`，但连续发送中途上下文发生压缩时重新调用确保 `chat_id` 正确。
- `chat_id`、`userid`、`media_id` 都是内部调用值，禁止面向用户展示。
- Markdown 正文、视频标题和描述限制按 UTF-8 字节数计算；超限时不静默截断，请用户缩短或明确同意拆分。
- 发送成功后只说明目标和消息类型，不编造消息 ID。
- 接口失败时如实转达错误，不使用 curl / Python 等方式绕过 `wecom-cli`。
