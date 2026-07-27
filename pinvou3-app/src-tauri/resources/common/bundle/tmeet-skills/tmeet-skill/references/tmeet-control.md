# tmeet control — 会中控制

> **前置条件：** 先执行 `tmeet auth login` 完成登录授权，且会议必须处于进行中状态。

会中控制命令用于在会议进行过程中对参会成员进行管理，包括呼叫成员入会、将成员踢出会议。成员通过用户 `open_id` 指定，可通过 `contact search` 命令查询获得。

---

## call — 呼叫成员入会

> ⚠️ **写操作，执行前请确认用户意图。被呼叫成员的腾讯会议客户端会收到入会呼叫通知。**

会中邀请呼叫：向指定成员发起入会呼叫，邀请其加入当前正在进行的会议。

```bash
# 通过英文逗号分隔传入多个 open_id
tmeet control call \
  --meeting-id "100000000" \
  --users "open_id1,open_id2"

# 重复传入 --users 参数
tmeet control call \
  --meeting-id "100000000" \
  --users "open_id1" \
  --users "open_id2"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--meeting-id <id>` | ✅ | — | 会议 ID |
| `--users <list>` | ✅ | — | 待呼叫成员 `open_id` 列表，支持英文逗号分隔或重复传入该参数，最多 20 个 |

---

## kick — 踢出会议成员

> ⚠️ **高风险写操作：被踢出的成员会立即离开会议。默认允许被踢成员重新入会（`--allow-rejoin` 默认 `true`）；如需不允许重新入会，可显式传 `--allow-rejoin=false`。执行前必须向用户明确列出将被踢出的成员。**

> 🔒 **成员来源约束：`kick` 的目标必须是当前会议中的实际参会人，其 `open_id` / `ms_open_id` 必须从 [`tmeet report participants`](tmeet-report.md#participants--获取参会人列表) 的返回结果中获取，严禁使用 `tmeet contact search` 通讯录查询的结果作为来源。**

> 原因：通讯录查询返回的是组织成员名录，并不代表他们已加入当前会议；而踢人操作仅对会中成员有效，且需要区分普通成员、Sip、Pstn 三类设备身份，这些信息只有 `report participants` 才能准确提供。

会中踢人：将指定成员从当前正在进行的会议中移除。支持踢出三类对象（身份类型以 `report participants` 返回的字段为准）：
- **普通成员**：通过 `--users` 传入参会人 `open_id`（不包含 Sip/Pstn 设备）
- **Sip 设备**：通过 `--sip-users` 传入设备 `ms_open_id`
- **Pstn 设备**：通过 `--pstn-users` 传入设备 `ms_open_id`

```bash
# 踢出普通成员（默认允许重新入会）
tmeet control kick \
  --meeting-id "100000000" \
  --users "open_id1,open_id2"

# 重复传入 --users 参数
tmeet control kick \
  --meeting-id "100000000" \
  --users "open_id1" \
  --users "open_id2"

# 同时踢出普通成员、Sip 设备、Pstn 设备
tmeet control kick \
  --meeting-id "100000000" \
  --users "open_id1" \
  --sip-users "ms_open_id_sip1" \
  --pstn-users "ms_open_id_pstn1"

# 不允许被踢成员重新入会
tmeet control kick \
  --meeting-id "100000000" \
  --allow-rejoin=false \
  --users "open_id1,open_id2"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--meeting-id <id>` | ✅ | — | 会议 ID |
| `--users <list>` | 三选一 | — | 待踢出的普通成员 `open_id` 列表（不含 Sip/Pstn 设备），支持英文逗号分隔或重复传入该参数 |
| `--sip-users <list>` | 三选一 | — | 待踢出的 Sip 设备 `ms_open_id` 列表，支持英文逗号分隔或重复传入该参数 |
| `--pstn-users <list>` | 三选一 | — | 待踢出的 Pstn 设备 `ms_open_id` 列表，支持英文逗号分隔或重复传入该参数 |
| `--allow-rejoin` | ❌ | `true` | 被踢出的成员是否允许重新加入会议；不传则默认 `true`（允许重新入会），传 `--allow-rejoin=false` 不允许重新入会 |

> `--users` / `--sip-users` / `--pstn-users` **至少必填一种**，且**三者总数合计最多 20 个**。

---

## 常见错误

| 错误现象 | 原因 | 解决方案 |
|---------|------|---------|
| `--meeting-id is required` | 缺少必填参数 | 补充 `--meeting-id` |
| `--users/--sip-users/--pstn-users, at least one of them is required` | 三个参数一个都未传 | 至少传入其中一个 |
| `the total number of --users/--sip-users/--pstn-users is too long, max is 20` | 三者合计超过 20 | 减少传入的 `open_id` / `ms_open_id` 数量，使三者合计不超过 20 |

## 参考

- [tmeet](../SKILL.md) — 全部命令概览
- [tmeet-report](tmeet-report.md) — 会议报告（**`kick` 的成员 `open_id` / `ms_open_id` 必须来自 `report participants`**）
- [tmeet-contact](tmeet-contact.md) — 通讯录搜索（仅用于 `call` 邀请会外成员入会获取 `open_id`，不用于 `kick`）
- [tmeet-meeting](tmeet-meeting.md) — 会议管理（受邀者管理）
