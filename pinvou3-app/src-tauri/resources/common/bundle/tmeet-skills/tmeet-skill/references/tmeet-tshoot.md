# tshoot — 问题排查

## `tshoot log` — 导出本地日志

将本地日志打包为 zip 文件，输出到 `~/tmeet_ts_{datetime}.zip`，可用于问题排查。支持按时间范围过滤，不传时间参数则导出全部日志。

```bash
tmeet tshoot log [选项]
```

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|:----:|--------|------|
| `--start` | string | 与 `--end` 同时使用 | — | 日志查询开始时间，ISO 8601，如 `2026-03-12T14:00+08:00` |
| `--end` | string | 与 `--start` 同时使用 | — | 日志查询结束时间，ISO 8601，如 `2026-03-12T15:00+08:00` |
| `--upload` | bool | 否 | `false` | 上传日志到服务器，需要登录 |

> `--start` 和 `--end` 必须同时传入或同时不传。

### 示例

```bash
# 导出全部日志
tmeet tshoot log

# 导出指定时间范围内的日志
tmeet tshoot log \
  --start "2026-04-10T00:00+08:00" \
  --end "2026-04-10T23:59+08:00"

# 导出日志并上传至服务器（需要登录）
tmeet tshoot log --upload
```

### 输出示例

```
output log saved to: ~/tmeet_ts_20260410_153000.zip
```

### 说明

- 日志文件存储在 `~/.tmeet/logs/` 目录下
- 输出的 zip 文件保存在用户主目录（`~/`）
- 若指定时间范围内无日志，输出提示 `choose time range has no log`
- `--upload` 用于将日志上传到服务器，需要先完成 `tmeet auth login` 登录授权

---

## `tshoot feedback` — 上报问题排查反馈

将 Agent 在使用 tmeet CLI 过程中遇到的问题或建议上报至平台。

> 何时调用 / 调用准则请参阅 [SKILL.md](../SKILL.md) «自动反馈规则»一节；本文仅提供命令语法、参数与示例。

```bash
tmeet tshoot feedback --category <分类> --intent <原始意图> [选项]
```

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|:----:|--------|------|
| `--category` | string | ✅ | — | 反馈分类，枚举值见下表 |
| `--intent` | string | ✅ | — | Agent 的原始意图（用户想做什么），最多 **200 字符** |
| `--actions-tried` | string | — | — | Agent 已尝试过的动作（调用了哪些命令、做了哪些尝试），最多 **500 字符** |
| `--result` | string | — | — | 已尝试动作的结果或阻塞点（错误信息、为何无法继续），最多 **500 字符** |
| `--tool-name` | string | — | — | 涉及的 tmeet 子命令名（如 `record address`） |
| `--error-code` | string | — | — | 工具返回的业务错误码（若有） |

### `--category` 枚举值

| 取值 | 含义 | 适用场景 |
|------|------|---------|
| `tool_not_found` | 找不到匹配的工具 | 想做某事，但 tmeet 没有对应命令 |
| `tool_error` | 工具调用返回错误 | 命令执行失败，返回错误码或错误信息 |
| `tool_inadequate` | 工具能力/参数不足 | 命令存在但无法满足当前诉求 |
| `unexpected_result` | 结果未达预期 | 命令成功但返回结果不符合期望 |
| `suggestion` | 一般性建议 | 对 tmeet 的改进建议、新增能力提议 |

### 示例

```bash
# 1) tool_not_found：用户想批量导出某时间段所有会议的智能纪要，但当前没有批量命令
tmeet tshoot feedback \
  --category "tool_not_found" \
  --intent "批量导出 2026-04 整月所有会议的智能纪要为 markdown" \
  --actions-tried "查阅了 record list / record smart-minutes 子命令" \
  --result "未找到批量导出命令，仅能逐个文件查询"

# 2) tool_error：调用 record address 返回错误码
tmeet tshoot feedback \
  --category "tool_error" \
  --intent "获取某录制文件的下载地址" \
  --tool-name "record address" \
  --error-code "200003" \
  --actions-tried "tmeet record address --meeting-record-id record_abc123" \
  --result "接口返回权限不足"

# 3) tool_inadequate：能力/参数不足
tmeet tshoot feedback \
  --category "tool_inadequate" \
  --intent "按主题模糊搜索历史会议" \
  --tool-name "meeting list-ended" \
  --result "list-ended 仅支持时间范围过滤，不支持按 subject 模糊搜索"

# 4) unexpected_result：结果不符合预期
tmeet tshoot feedback \
  --category "unexpected_result" \
  --intent "查询近 7 天的所有录制" \
  --tool-name "record list" \
  --result "返回的录制条数明显少于实际录制数"

# 5) suggestion：一般性建议
tmeet tshoot feedback \
  --category "suggestion" \
  --intent "希望 meeting list 支持按主题关键字过滤"
```

### 说明

- 字符长度超限（`intent>200` / `actions-tried>500` / `result>500`）会在客户端直接报错，不会发起请求；
- 反馈成功后输出 `feedback_id`。
