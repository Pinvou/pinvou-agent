---
name: wecomcli-media
version: 1.0.0
description: 何时用:仅当用户明确指向企业微信 media_id 与本地文件互转时使用;泛指本地文件操作默认走本地工具。基于 media_id 下载媒体文件到本地,或上传本地文件换取 media_id 供其他企微技能使用;本技能不解析/识别文件内容,仅负责文件搬运。
metadata:
  requires:
    bins: ["wecom-cli"]
  cliHelp: "wecom-cli media --help"
---

# 企业微信媒体文件

> 执行任何 `wecom-cli` 命令前，必须先读取并完成 `wecomcli-shared` 技能的公共前置检查。

资源型 skill，负责基于 `media_id` 下载媒体文件到本地，以及把本地文件上传为 `media_id`。是其他技能（微盘、邮件等）处理 `media_id` 相关操作的基础依赖：`upload` 会产出新的 `media_id`，但本 skill 不负责搜索/发现其他业务场景中已存在的 `media_id`（如邮件附件、微盘文件的 `media_id` 由对应业务技能产出），也不解析文件内容。

## 适用范围

### 适用

- 根据其他技能或用户提供的 `media_id` 下载媒体文件到本地
- 上传本地文件（本地路径已知）获取 `media_id`，供其他技能后续使用（如微盘上传素材）

### 不适用

- 解析/识别文件内容（正文提取、OCR、看图问答、PDF/Word/Excel 解析等） → 本 skill 只负责把文件下载到本地拿 `file_path`，如需查看内容请直接通过 `file_path` 读取该本地文件
- 搜索/发现其他业务场景中已存在的 `media_id`（如邮件附件、微盘文件列表/搜索等） → 由对应业务技能负责产出并返回 `media_id`，本 skill 只接收已有的 `media_id` 做下载；本地文件转`media_id` 的场景仍走本 skill 的 `upload`
- 编造或猜测 `media_id` / 本地文件路径 → 两者必须来自其他技能返回或用户明确提供，禁止自行构造

## 接口详述

### 下载媒体文件

根据 `media_id` 下载媒体文件到本地，返回本地文件路径。

**命令**

```bash
wecom-cli media download --json '{"media_id": "MEDIA_ID"}'
```

**入参**

| 字段 | 类型 | 必填 | 说明 |
|---|---|:----:|---|
| `media_id` | string | 是 | 文件的 `media_id`，由上传文件后获得，或由其他技能（邮件附件/内嵌图片等）返回 |

**返回**

| 字段 | 类型 | 说明 |
|---|---|---|
| `file_path` | string | 下载成功后的本地文件路径 |

**使用规则**

- 下载完成后如需查看文件内容，直接通过 `file_path` 读取该本地文件。
- 下载失败时返回错误码和错误信息。
- **`media_id` 必须是真正的 media_id，不接受任何形式的 URL**：若拿到的是一个链接（如 `attach_url`、正文里的图片/附件链接），**不要**把这个 URL 当作 `media_id` 传入本接口，会直接报错。尤其是命中 `work.weixin.qq.com/filepreview/security/` 特征的防泄漏加密链接，属于加密的、与用户身份绑定的资源，本接口**无法下载或解密**，应直接告知用户该文件受防泄漏策略保护，引导其点击链接、在企业微信客户端内打开查看/保存，不要尝试用本接口或其他手段绕过。

### 上传媒体文件

将本地文件上传，获取 `media_id`。

**命令**

```bash
wecom-cli media upload --json '{"file_path": "/tmp/example.pdf"}'
```

**入参**

| 字段 | 类型 | 必填 | 说明 |
|---|---|:----:|---|
| `file_path` | string | 是 | 需要上传的文件的本地路径 |
| `type` | string | 否 | 媒体类型：`image`(图片)/`voice`(语音)/`video`(视频)/`file`(文件)，可选传入；发送消息时以上传响应返回的 `type` 为准 |

**返回**

| 字段 | 类型 | 说明 |
|---|---|---|
| `type` | string | 媒体类型：`image`(图片)/`voice`(语音)/`video`(视频)/`file`(文件) |
| `media_id` | string | 上传后的 `media_id`，供其他技能后续使用（如微盘`upload` 的 `file_content_media`） |
| `created_at` | string | 创建时间，格式：`YYYY-MM-DD HH:mm:ss`|

## 关键约束

- **`media_id` / `file_path` 不得编造**：`media_id` 必须来自上传结果、其他技能返回或用户明确提供；`file_path` 必须是真实存在的本地路径。两者都没有时用自然语言追问，禁止靠猜测凑一个。
- **不做内容解析**：本 skill 只负责文件的下载落地与上传，`download` 拿到 `file_path` 后如需查看内容，直接通过 `file_path` 读取，不在本 skill 职责范围内。
- **内部 ID 不外露**：`media_id` 仅用于后续接口调用，禁止直接展示给用户；下载后的本地 `file_path` 同样不展示给用户。
- **CLI 报错原样转达**：命令返回明确错误码时如实告知用户并给替代建议，禁止用 curl / python 等通用手段绕过 CLI 强行完成。

## 跨技能依赖

| 依赖场景 | 说明 |
|---|---|
| `wecomcli-email` | 邮件附件/内嵌图片的 `media_id`，使用本 skill 的 `download` 下载到本地后通过 `file_path` 读取 |
| `wecomcli-disk` | 上传文件到微盘时若已有 `media_id`，直接作为 `disk files upload` 的 `file_content_media` 使用，无需再走本 skill；若只有本地路径且需要先转成 `media_id`，可用本 skill 的 `upload` |

> 参数缺失 / 意图不明确时，用自然语言追问让用户明确，不要瞎猜。
