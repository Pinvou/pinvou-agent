# event — 实时事件订阅

通过本机后台 **bus 守护进程**（per-host daemon）订阅腾讯会议的实时事件（如 `meeting.started`、`meeting.end`等）。所有 `tmeet event consume` 消费者复用同一条 WSS 长连接，由 bus 进程统一管理握手 / 心跳 / 自动重连。

> 主 SKILL 仅保留功能说明与订阅入口；何时使用、调用流程、subprocess 契约、红线、bus 残留自愈流程等**全部收敛在本文档**。

## 目录

- [通用约定](#通用约定)
- [何时使用 `event consume`（vs 快照查询）](#何时使用-event-consumevs-快照查询) · [红线（硬约束）](#红线硬约束)
- [`event list` — 列出可订阅的 EventKey](#event-list--列出可订阅的-eventkey)
- [`event schema <EventKey>` — 查看 EventKey 的完整契约](#event-schema-eventkey--查看-eventkey-的完整契约)
- [`event consume` — 订阅事件并按 NDJSON 流式输出](#event-consume---event-id-eventkey--订阅事件并按-ndjson-流式输出)
  - [事件流的常识约束](#事件流的常识约束) · [运行模式](#运行模式) · [stdin EOF 与后台运行契约](#stdin-eof-与后台运行契约) · [stderr 控制面诊断](#stderr-控制面诊断) · [错误处理](#错误处理)
- [`event status` — 查看本机 bus 守护进程状态](#event-status--查看本机-bus-守护进程状态)
- [`event stop` — 停止本机 bus 守护进程](#event-stop--停止本机-bus-守护进程)
- [EventKey 字段参考（payload 内部结构）](#eventkey-字段参考payload-内部结构)
- [典型故障速查](#典型故障速查)

## 通用约定

- **stdout / stderr 分离**：业务事件只写 stdout（NDJSON 每行一条），所有诊断 / 状态 / 告警仅写 stderr。
- **bare JSON，无信封**：`event` 子命令族输出的均为 **bare JSON**，**不包含**主 SKILL `--format` 章节描述的 `{trace_id, message, data}` 信封；也**不走 `--compact` 中间件**，加 `--compact` 不会产生任何精简效果。需要精简事件字段请用 `event consume --jq` 投影。
- **ready 标记**（subprocess 契约）：握手完成并就绪后，stderr 输出一行：
  ```text
  [event] ready event_key=<key>
  ```
  即使开启 `--quiet` 也不会被屏蔽。
- **退出标记**：consume 退出时 stderr 输出一行汇总：
  ```text
  [event] exited — received <N> event(s) in <duration> (reason: <reason>)
  ```
  `reason` 取值：`limit` / `timeout` / `signal` / `shutdown`。
- **退出码约定**：
  - `0` — 正常退出（达到 `--max-events` / `--timeout` / 收到 SIGINT/SIGTERM / bus 主动关闭）。
  - `1` — 致命错误（Hello 被拒、未知 EventKey、IO 错误、订阅失败等）。
  - `2` — 仅 `event status --fail-on-orphan` 与 `event stop` 在 `refused` / `errored` 状态时返回。
- **`event _bus`** 为隐藏子命令（由 `event consume` 自动拉起），**Agent 不得直接调用**。
- **stdin EOF 行为**：`tmeet event consume` **完全不读 stdin**——关闭 stdin / `< /dev/null` / `nohup` / `setsid` 在**有界（`--max-events > 0` 或 `--timeout > 0`）与无界（两者都为 0）两种模式下均不会触发退出**。详见下文 «stdin EOF 与后台运行契约»，其中有针对 Agent 场景踩坑（尤其从其它 CLI 迁移过来的用户）的完整说明。

---

## 何时使用 `event consume`（vs 快照查询）

- ✅ 用户表达「监听 / 订阅 / 一开始就记录 / 结束后补播」等**被动触发**意图
- ✅ 长任务 Agent 需要在会议状态变化后（用户下次开口时）被动驱动后续动作
- ⚠️ 用户说「实时通知我」「一有事件就推送」时，**不要默认 Agent 能做到**——需主动告知用户：Agent 在你静默期无法主动说话，只能后台落盘、等用户下次发消息时再补播
- ❌ 用户只想知道「现在有哪些会议在开」——用 `meeting list`，**不要**用 `event consume`
- ❌ 用户想查「历史已结束的会」——用 `meeting list-ended`，event 流**不回放历史事件**，只交付订阅后新产生的事件

### 调用流程（强约束顺序）

```
1) tmeet event list                # 查可用 EventKey 与 domain
2) tmeet event schema <key>        # 看 params_schema / jq_root_path / payload schema
3) tmeet event consume --event-id <key> ...   # 真正订阅
```

> 跳过 (2) 直接拼 `--jq` / `--param` 是常见错误源：`jq_root_path` 错了**不会报错只会静默丢事件**（jq 返回 null/无结果即丢弃，仅在 stderr 留一行 WARN）。

### 运行模式选择

| 用户意图 | 选择 |
|---------|------|
| 「等下一场会一开始就告诉我」 | `--max-events 1` |
| 「盯 30 分钟，结束后汇总」 | `--timeout 30m` |
| 「持续监听，直到我喊停」 | 两者都不传；明确告知用户用 `Ctrl-C` 或 `tmeet event stop` 退出 |

---

## 红线（硬约束）

- **登录前置**：`event consume` 必须登录（owner_hash 与 bus 绑定）；`event list` / `schema` / `status` / `stop` 不依赖登录，可在 `auth logout` 后用于残留排查。
- **写 `--jq` 前先看 schema**：当前 `meeting.started` / `meeting.end` 的 `jq_root_path` 是 `.payload`，**且 `payload` 本身是数组**（服务端契约保证长度恒为 1）。jq 必须先用 `.[0]` 取首元素再下钻字段，例如 `.[0].meeting_info.subject`；**不能**写成 `.payload.subject` 或 `.meeting_info.subject`。
- **不要 `kill -9` consume 进程**：强杀不会清理 bus 残留；若误杀导致 `tmeet event status` 出现 `orphan` / `stale_owner`，必须 `tmeet event stop --force` 清理（属写操作，需二次确认）。
- **多账号切换**：bus 与 OpenID 绑定。`auth logout` 后再换账号登录，若 `status` 显示 `stale_owner`，需先 `tmeet event stop --force` 再 `consume`。
- **`--output-dir` 路径**：仅允许相对路径，不允许 `..` 段；CLI 已硬校验，**不要**尝试绕过传绝对路径。
- **隐私输出**：事件 payload 含会议主题 / 参会人姓名等敏感字段；展示给用户时仍遵循主 SKILL «响应处理规则»——只展示关键字段，会议标识统一用 `meeting_code` 而非 `meeting_id`。
- **`event _bus` 是隐藏命令**：由 `event consume` 自动 fork，Agent **严禁**直接调用。

### bus 残留自愈流程

```
1) tmeet event status                          # 看 buses[].state
2) state=running 且 is_active_login=true       # 正常，无需处理
3) state=stale_owner                           # 与原账号确认后 → tmeet event stop --force
4) state=orphan                                # 直接 tmeet event stop --force（仍属写操作，需二次确认）
```

---

## `event list` — 列出可订阅的 EventKey

读取本地内置注册表，**不依赖登录**，也不发起任何远程调用。

```bash
tmeet event list [选项]
```

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|:----:|--------|------|
| `--domain` | string | — | — | 仅展示该 domain 下的 EventKey（如 `meeting`）；未知 domain 会以退出码 1 返回并提示已知 domain 列表 |

### 输出字段

| 字段 | 说明 |
|------|------|
| `key` | EventKey 名称，例如 `meeting.started` |
| `domain` | 所属 domain，例如 `meeting` |
| `description` | 简短描述 |

### 示例

```bash
# 列出全部 EventKey
tmeet event list

# 仅展示 meeting 域下的 EventKey，并以缩进格式输出
tmeet event list --domain meeting --format json-pretty
```

---

## `event schema <EventKey>` — 查看 EventKey 的完整契约

输出指定 EventKey 的参数 schema（`--param` 可用的 key）、事件 payload 的 JSON Schema、以及 jq 表达式使用的根路径。本地注册表查询，**不依赖登录**。

```bash
tmeet event schema <EventKey>
```

### 参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|:----:|------|
| `<EventKey>` | string | ✅ | 位置参数，要查询的 EventKey；未知 key 会以退出码 1 返回并提示用 `event list` 查询 |

### 输出字段

| 字段 | 说明 |
|------|------|
| `key` | EventKey 名称 |
| `domain` | 所属 domain |
| `jq_root_path` | `--jq` 表达式的根路径，取值为 `.`（整包络）或 `.payload`（仅 payload） |
| `params_schema` | `--param key=value` 可接受的参数定义（map），含 `type` / `required` / `description` |
| `resolved_output_schema` | 事件 payload 的 JSON Schema |

### `jq_root_path` 解读

`--jq` 表达式拿到的"输入根 `.`"由该字段决定：

| 取值 | 含义 | jq 写法示例 |
|------|------|-------------|
| `"."` | jq 看到整个包络 `{event, trace_id, payload}` | `.payload.meeting_info.subject` |
| `".payload"` | jq 看到的就是 `payload` 本身 | 见下一行（注意数组形态） |

> **强约束**：当前 `meeting.started` / `meeting.end` 两个 EventKey 的 `jq_root_path` 均为 `.payload`，**且 `payload` 本身是数组**（服务端契约保证长度恒为 1）。在 jq 中**必须先 `.[0]` 取首元素再下钻**，例如 `.[0].meeting_info.subject`，**不能**写成 `.payload.subject` 或 `.meeting_info.subject`。

### 示例

```bash
tmeet event schema meeting.started --format json-pretty
```

---

## `event consume --event-id <EventKey>` — 订阅事件并按 NDJSON 流式输出

订阅指定 EventKey 的事件流，每条事件以一行 NDJSON 写入 stdout。底层 bus 未运行时会自动 fork 一个出来。

```bash
tmeet event consume --event-id <EventKey> [选项]
```

> 该命令**要求已登录**（用 OpenID 计算 owner_hash 与 bus 绑定）。未登录请先执行 `tmeet auth login`。

> **单 EventKey 限制**：`--event-id` 只接受单个 EventKey。

### 事件流的常识约束

`event consume` 是**长跑 + 异步**命令，无论调用方以何种方式使用订阅数据，都需满足以下约束：

- **`trace_id` 不是去重键**：仅用于单次推送的链路追踪；服务端已做事件去重，Agent **不需**再自行在端内去重。
- **事件流不回放历史**：consume 重启后不会补送过去的事件。查"现在哪些会在开"请用 `meeting list`（快照），查"历史已结束会议"请用 `meeting list-ended`，**不要**用 `event consume` 替代。
- **时间字段单位混用陷阱**：`payload[0].meeting_info.start_time` / `end_time` 为**秒级** Unix 时间戳；`operate_time` 为**毫秒级**——单位不同，比较 / 排序前**必须**统一，且展示时转成用户时区可读字符串。
- **异常事件的处理口径**：
  - stderr 出现 `[source] ...: auth_expired` → 该 consume 会以退出码 1 退出；告诉用户"登录态已过期，请重新 `tmeet auth login`"，**不要**把 `auth_expired` 字面抛给用户；
  - stderr 出现 `[event] WARN dropped <N> event(s) ...` → 提示用户丢弃条数 + 可能原因（上游抖动），询问是否重订；
  - stderr 出现 `[event] WARN subscribe failed key=... code=...` 后退出码 1 → **不要静默重试**，把 `code` 呈现给用户并询问下一步。

### 运行模式

| 模式 | 触发 | 退出 |
|------|------|------|
| 有界（批处理） | `--max-events` 或 `--timeout` 任一为正 | 首次满足条件即退出（退出码 0，`reason: limit` / `timeout`） |
| 无界（常驻） | 两者都不传或都为 0 | 仅在收到 SIGINT/SIGTERM 或 `tmeet event stop` 关闭 bus 时退出 |

### stdin EOF 与后台运行契约

> **一句话**：`tmeet event consume` **完全不读 stdin**——**有界（`--max-events` / `--timeout` 任一 > 0）和无界（两者都为 0）两种模式下，stdin EOF 都不会触发退出**。`nohup`、`</dev/null`、`setsid`、脱离终端、SSH 断链均**安全**，不需要用 `< <(tail -f /dev/null)` 之类的保活 trick。

| 模式 | stdin EOF 行为 |
|------|---------------|
| 有界（bounded）| stdin EOF → **忽略**，按 `--max-events` / `--timeout` 达标退出 |
| 无界（unbounded）| stdin EOF → **忽略**，仅 SIGINT/SIGTERM 或 `tmeet event stop` 才终止 |

**无界模式的正确退出手段**：SIGINT/SIGTERM（`kill <pid>` 默认发 SIGTERM，consume 会走完 Bye 帧后 `reason: signal` 退出）或 `tmeet event stop`（可能同时影响其他 consumer，需按红线走确认）。**不要 `kill -9`**（见 «红线»）。

#### Agent 父进程契约（拉起 `event consume` 的调用方须知）

Agent 或脚本把 `event consume` 作为 subprocess 拉起时，**判断"订阅是否真的就绪"必须走 stderr 阻塞读，不要 sleep**：

- **正确**：父进程按行读取 subprocess 的 stderr，**阻塞**直到读到 `[event] ready event_key=<key>`（`--quiet` 也不会屏蔽这一行）——ready 标记之后才认为订阅生效，可以继续下一步。若 stderr 提前吐出 `[event] handshake failed` / `[event] bus rejected` / `[event] WARN subscribe failed` 或子进程直接退出，说明订阅失败，按 «错误处理» 处置。
- **错误**：拉起后 `sleep 2` 就认为订阅生效——bus 冷启动、握手、`bus rejected` / `subscribe failed` 都可能让"就绪"晚于任何固定睡眠时长，事件流会有静默丢失窗口。

**推荐后台落盘模板**（Agent 需要事件在后台归档、之后由调用方自行决定何时/如何呈现给用户）：

```bash
# 1) 后台拉起 consume：stdout 落文件 (--output-dir)，stderr 落到独立日志
nohup tmeet event consume --event-id <key> --output-dir ./meeting_events --quiet \
  >/dev/null 2>./meeting_events.stderr.log &
echo $! > ./meeting_events.pid

# 2) 阻塞等 ready 标记，最多 10 秒；就绪后再返回给用户 / 继续后续动作
( timeout 10 tail -n +1 -F ./meeting_events.stderr.log 2>/dev/null | \
  grep -m1 -E '^\[event\] ready ' ) || {
    echo "consume 未在 10s 内就绪，查看 ./meeting_events.stderr.log" >&2
    exit 1
  }

# 3) 收尾：停止订阅（发 SIGTERM，consume 会走 Bye 后按 reason: signal 退出）
kill "$(cat ./meeting_events.pid)"
```

- `--quiet` 不会屏蔽 `[event] ready` / `[event] WARN ...` / `[event] exited` 三种关键行，其他 info 会被压掉，正好适合后台归档。
- 同一 owner 下所有 consume 结束后，bus 会在 idle-timeout 后自行退出，**不需**额外 `event stop`。

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|:----:|--------|------|
| `--event-id` | string | ✅ | — | 要订阅的 EventKey（必须为 `event list` 中已注册的 key）；只接受单个 EventKey |
| `--param` | strings | — | — | 形如 `key=value` 的订阅过滤参数，可重复传入；可用 key 由 `event schema <key>.params_schema` 给出，未知 key 会以退出码 1 拒绝 |
| `--max-events` | int | — | `0` | 累计接收到 N 条事件后退出，`0` 表示不限制；负数被拒绝 |
| `--timeout` | duration | — | `0` | 自 ready 标记起 N 时长后退出，`0` 表示不限制（如 `30s`、`5m`、`1h`）；负数被拒绝 |
| `--quiet` | bool | — | `false` | 抑制信息型 stderr 输出；ready / exit / WARN / 握手失败仍会输出 |
| `--output-dir` | string | — | — | 额外把每条事件写入 `<output-dir>/<trace_id>.json`（**每事件一个文件、多行缩进 JSON**，shape 同 stdout；**不受 `--jq` 影响**、始终为完整事件，适合审计与后台归档）；**仅允许相对路径**，不允许 `..` 段；目录不存在会自动创建（权限 0700）；单条事件写入失败仅 stderr `WARN`，不中断订阅 |
| `--jq` | string | — | — | gojq 表达式：返回 null / 无结果则丢弃该事件，否则其输出替换默认的 NDJSON 行；编译期失败会在 fork bus 之前拒绝（不会打印 ready 标记） |

> **关于 `--param` 的现状提醒**：当前所有 EventKey（`meeting.started` / `meeting.end`）的 `params_schema` **只接受 `meeting_id` 一个 key**，许多其它可能的筛选维度（`subject` / 创建人 / 会议类型 等）**不可用 `--param`**。需下推筛选请用 `--jq`。

### stdout 默认输出形态

```json
{"event":"meeting.started","trace_id":"<id>","payload":[{...}]}
```

> `payload` 为数组（长度恒为 1）；写 `--jq` 时务必 `.[0]` 取首元素，详见 «`event schema` → jq_root_path 解读»。

### stderr 控制面诊断

```text
[event] starting consume key=<key>
[event] bus not running, forked daemon                      # 仅在自动拉起 bus 时输出
[event] bus owner mismatch (bus=<bus_hash> consume=<self_hash>); run 'tmeet event stop --force' to clean up
[event] bus rejected EventKey "<key>" (detail: ...)         # 未知 EventKey / 底层订阅被拒
[event] handshake failed: <error> (<detail>)                # 其它 Hello 失败
[event] handshake ok bus_version=<version>
[event] ready event_key=<key>                               # 就绪标记，--quiet 也不屏蔽
[event] received trace_id=<id>                              # 每条事件一行
[source] <source>: <state> (<detail>)                       # 上游 source 状态变化
[event] WARN dropped <N> event(s) for key=<key> since unix=<ts>
[event] WARN subscribe failed key=<key> code=<code> (<detail>)
[event] WARN jq error trace_id=<id>: <err>                  # 单条 jq 运行期错，不中断订阅
[event] WARN output-dir write failed trace_id=<id>: <err>   # 落盘失败，不中断订阅
[event] read error: <err>                                   # socket 读异常，随后以退出码 1 退出
[event] exited — received <N> event(s) in <duration> (reason: <reason>)
```

### 示例

```bash
# 仅取一条：下一场会一开就退出，jq 投影出会议号 / 主题 / 开始时间
tmeet event consume --event-id meeting.started --max-events 1 --quiet \
  --jq '.[0].meeting_info | {meeting_code, subject, start_time}'

# 有界批处理：仅消费 3 条事件后退出
tmeet event consume --event-id meeting.started --max-events 3

# 有界批处理：30 秒内若没事件也退出
tmeet event consume --event-id meeting.end --timeout 30s

# 用 --param 缩小订阅范围到指定 meeting_id（当前唯一可用的 --param key）
tmeet event consume --event-id meeting.started --param meeting_id=6953553464429888300

# 用 jq 投影只输出 meeting_id 和 subject
# 注意：meeting.started / meeting.end 的 jq_root_path 是 .payload，
# 即 jq 的输入根 . 已经是 payload 数组本身（长度恒为 1），
# 必须用 .[0] 取首元素后再下钻字段。
tmeet event consume --event-id meeting.started \
  --jq '.[0].meeting_info | {meeting_id, subject}'

# 后台归档（无界，脱离当前会话）：事件按 <trace_id>.json 落盘。
# 注意：如需父进程在拉起后确认订阅已生效，请按 «stdin EOF 与后台运行契约 →
# Agent 父进程契约» 的骨架阻塞读 stderr 等 `[event] ready`，不要 sleep。
nohup tmeet event consume --event-id meeting.started \
  --output-dir ./meeting_events \
  --quiet >/dev/null 2>./meeting_events.stderr.log &
echo $! > ./meeting_events.pid
```

### 错误处理

| 现象 | 退出码 | 典型原因 | 处置 |
|------|:------:|---------|------|
| `unknown EventKey "xxx"` | 1 | EventKey 拼写错误或未注册 | `tmeet event list` 查可用 key |
| `unknown --param key=...` | 1 | `--param` key 不在当前 EventKey 的 schema 中 | `tmeet event schema <key>` 查可用 param |
| `duplicate --param key "xxx"` | 1 | 同一个 `--param key` 重复传入 | 仅保留一次 |
| `invalid --param "xxx": expected key=value with non-empty key` | 1 | `--param` 格式错（缺 `=` / key 空） | 补齐为 `key=value` |
| `--param <key>: expected integer/boolean, got "xxx"` | 1 | 值不符合 schema 声明的类型 | 根据 `event schema` 给出的 type 修正 |
| `value "xxx" is not in allowed set [...]` | 1 | 值不在 schema 枚举集中 | 改用枚举集内的取值 |
| `--output-dir must be relative` | 1 | 传入了绝对路径 | 改为相对路径 |
| `--output-dir must not contain '..' segments` | 1 | 路径含 `..` | 去掉 `..` 段 |
| `user config is empty` | 1 | 未登录 | `tmeet auth login` |
| stderr 出现 `[event] WARN subscribe failed key=<key> code=<code>` 后立即退出 | 1 | 网关拒绝订阅（鉴权 / 配额 / 后端错误） | 据 `<code>` 排查；常见为账号未开通该事件订阅权限，需在腾讯会议管理后台启用 |
| ready 标记一直不出现 | — | 握手失败 / token 过期 / WSS 链路异常 | `tmeet auth status` 检查；过期则重登；网络异常时观察 stderr 的 `[source]` 状态行 |

---

## `event status` — 查看本机 bus 守护进程状态

报告本机 bus 守护进程的状态。输出 schema 始终包含一个长度为 0 或 1 的 `buses` 数组（每台主机最多一个 bus 实例）。仅读取本机 bus 目录与 IPC，**不依赖登录**；常用于 `auth logout` 后排查残留状态。

```bash
tmeet event status [选项]
```

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|:----:|--------|------|
| `--fail-on-orphan` | bool | — | `false` | 当存在 `orphan` 或 `stale_owner` 状态的 bus 时，以退出码 `2` 返回（默认 0），方便健康检查脚本分支处理 |

### `buses[].state` 状态机

| 状态 | 含义 | 建议操作 |
|------|------|---------|
| `running` | bus 存活且绑定到当前登录用户 | 无需处理；`consumer_count=0` 也是正常状态（所有消费者退出后 bus 会在 idle-timeout 后自行退出，不需手动 stop） |
| `stale_owner` | bus 存活但绑定到其他用户，或本机未登录 | 与原用户确认后执行 `tmeet event stop --force`，或重新以原账户登录 |
| `orphan` | bus 已退出但残留了 `bus.pid` / `bus.meta` 等磁盘文件 | 执行 `tmeet event stop --force` 清理残留 |

### 输出 `buses[]` 主要字段

| 字段 | 说明 |
|------|------|
| `state` | 状态枚举，见上表 |
| `openid_hash` | bus 绑定的 OpenID 哈希 |
| `is_active_login` | 该 bus 的 owner 是否为当前登录用户 |
| `pid` | bus 进程 PID |
| `started_at` | bus 启动时间（本地时区 RFC3339） |
| `sock` | bus 监听的 unix socket 路径 |
| `consumer_count` | 当前挂接的消费者数量（仅 `running` 时有意义） |
| `subscribed_keys` | 当前订阅的 EventKey 列表 |
| `wss.state` | 底层 WSS 链路状态（`connecting` / `steady` / `reconnecting` / `auth_failed` / `auth_expired` / `disconnected`） |
| `wss.connected_at` | WSS 建链时间（本地时区 RFC3339） |
| `wss.reconnect_count` | WSS 累计重连次数 |
| `hint` | 异常状态下的处理建议 |

### 示例

```bash
# 普通查询
tmeet event status --format json-pretty

# 健康检查脚本：发现 orphan / stale_owner 时退出码 2
tmeet event status --fail-on-orphan
```

---

## `event stop` — 停止本机 bus 守护进程

请求 bus 守护进程退出。默认走优雅关闭，必要时通过 `--force` 强制清理。**不依赖登录**；常用于 `auth logout` 之后清理残留状态。

> ⚠️ **写操作 / 高风险**：`event stop --force` 会**驱逐所有活跃消费者**并清理磁盘残留，调用前必须先向用户展示影响（当前 `consumer_count` 与受影响的 EventKey）并获得用户确认。

```bash
tmeet event stop [选项]
```

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|:----:|--------|------|
| `--force` | bool | — | `false` | 跳过"还有活跃消费者"的拒绝保护；强制清理 `orphan` / `stale_owner` 状态；并在磁盘上清除残留的 `bus.pid` / `bus.meta` / `bus.sock` |
| `--timeout` | duration | — | `10s` | 等待 bus 优雅退出的最长时间，超时后若加 `--force` 会自动转入清理 |

### `results[].state` 状态与退出码

| 状态 | 含义 | 退出码 |
|------|------|:------:|
| `stopped` | bus 已退出（优雅 / 强制清理均归此类） | `0` |
| `no_bus` | 磁盘与运行时均无 bus，相当于 no-op | `0` |
| `refused` | 有活跃消费者 / 检测到 `stale_owner` / `orphan` 且未加 `--force` | `2` |
| `errored` | 优雅关闭超时且未加 `--force`，或强制清理失败 | `2` |

### 输出 `results[]` 主要字段

| 字段 | 说明 |
|------|------|
| `state` | 见上表 |
| `openid_hash` | 被操作 bus 的 owner 哈希 |
| `pid` | bus 进程 PID |
| `consumers_evicted` | 退出时被驱逐的消费者数量（`stopped` 时有意义） |
| `consumer_count` | 拒绝时的活跃消费者数量（`refused` 时有意义） |
| `forced` | 是否走了 `--force` 分支 |
| `socket_cleaned` | 是否清理了 `bus.sock` |
| `elapsed_ms` | 优雅关闭耗时（毫秒） |
| `hint` | 建议的后续操作 |

### 示例

```bash
# 优雅关闭：有活跃消费者时会拒绝（退出码 2）
tmeet event stop

# 强制关闭：驱逐活跃消费者；或清理 orphan / stale_owner
tmeet event stop --force

# 自定义优雅等待时长
tmeet event stop --timeout 5s
```

---

## EventKey 字段参考（payload 内部结构）

> 完整 JSON Schema 由 `tmeet event schema <key>` 给出；下表仅为日常写 `--jq` / `--param` 时的速查表。`jq_root_path` 均为 `.payload`，jq 输入根 `.` 即 payload 数组本身，**必须 `.[0]` 取首元素**。

### `meeting.started` — 会议开始事件

| 字段路径（payload 内）| 类型 | 说明 |
|------|------|------|
| `operate_time` | integer | 毫秒级事件操作时间戳 |
| `operator.userid` / `open_id` / `uuid` / `user_name` / `ms_open_id` / `instance_id` | string | 操作者身份信息（同企业用户为 `userid`，OAuth 用户为 `open_id`，rooms 为 `roomsId`） |
| `meeting_info.meeting_id` | string | 会议 ID（内部标识，不展示给用户） |
| `meeting_info.meeting_code` | string | 会议号（展示给用户用这个） |
| `meeting_info.subject` | string | 会议主题 |
| `meeting_info.creator.*` | string | 创建者身份信息，字段同 operator |
| `meeting_info.meeting_type` | integer | 0:一次性 / 1:周期性 / 2:微信专属 / 4:rooms 投屏 / 5:个人会议号 |
| `meeting_info.start_time` / `end_time` | integer | **秒级**时间戳 |
| `meeting_info.meeting_create_mode` | integer | 0:普通 / 1:快速 |
| `meeting_info.meeting_create_from` | integer | 0:空 / 1:客户端 / 2:web / 3:企微 / 4:微信 / 5:outlook / 6:restapi / 7:腾讯文档 / 8:Rooms 智能录制 |

**支持的 `--param`**：

| key | 类型 | 必填 | 说明 |
|------|------|:----:|------|
| `meeting_id` | string | — | 仅推送该 `meeting_id` 的事件；不传则接收本账号名下所有会议 |

### `meeting.end` — 会议结束事件

字段同 `meeting.started`，**额外**包含：

| 字段路径（payload 内）| 类型 | 说明 |
|------|------|------|
| `meeting_end_type` | integer | 0:主动结束 / 1:最后一人离开且超时 / 2:无人且超时 / 3:无人且未到结束时间 |

**支持的 `--param`**：同 `meeting.started`（仅 `meeting_id`）。

---

## 典型故障速查

| 现象 | `status` 应看哪个字段 | 行动 |
|------|----------------------|------|
| consume 启动后无事件 | `wss.state` | `reconnecting` 等待自愈；`auth_failed` / `auth_expired` 需重登；`disconnected` 观察 stderr `[source]` |
| `event stop` 返回 `refused` | `results[].consumer_count` | 有活跃消费者：先 Ctrl-C 关闭所有 `consume`，或加 `--force`（先与用户确认） |
| `status` 显示 `orphan` | `buses[].state` | `tmeet event stop --force` 清理 |
| 换账号后 `status` 显示 `stale_owner` | `buses[].openid_hash` vs 当前登录 hash | 与原账号确认后 `tmeet event stop --force`，再以新账号 `consume` |
| ready 标记不出现且 stderr 提示 handshake 失败 | — | 登录态/网络/bus 版本不匹配；`tmeet auth status` → 必要时 `event stop --force` 再重新 `consume` |
