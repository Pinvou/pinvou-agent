# 会议工作流(查询/取消/成员更新)

### 工作流 3: 查询会议列表

**示例**: 用户说 "帮我查一下本周有哪些会议"

**步骤:**

1. **确定时间范围**: 根据当前日期计算本周的起止时间。
2. **查询会议 ID 列表**:

```bash
wecom-cli meeting list_user_meetings '{"begin_datetime": "2026-03-16 00:00", "end_datetime": "2026-03-22 23:59", "limit": 100}'
```

3. **逐个查询会议详情** (对返回的每个 meetingid):

```bash
wecom-cli meeting get_meeting_info '{"meetingid": "<会议id1>"}'
```
```bash
wecom-cli meeting get_meeting_info '{"meetingid": "<会议id2>"}'
```

4. **汇总展示**:

```
📋 本周会议列表 (共 3 场):

1. 📅 技术方案评审
   🕐 2026-03-17 10:00 - 11:00
   👥 张三，李四，王五

2. 📅 产品需求沟通
   🕐 2026-03-18 14:00 - 15:00
   👥 赵六，钱七

3. 📅 周五周会
   🕐 2026-03-21 09:00 - 10:00
   👥 全组成员
```

> **分页处理**: 如果 `next_cursor` 不为空，使用 `cursor` 参数继续拉取下一页。

---

### 工作流 4: 获取会议详情

**示例**: 用户说 "帮我看下技术方案评审会议的详情"

**步骤:**

1. **定位会议**: 先通过会议列表查询找到目标会议的 meetingid (按关键词匹配)。
2. **查询详情**:

```bash
wecom-cli meeting get_meeting_info '{"meetingid": "<target_meetingid>"}'
```

3. **展示结果**:

#会议号: <会议号>

```
📅 <会议标题>

🕐 时间: <开始时间>，时长 <时长>
📍 地点: <会议地点>
📝 描述: <会议描述>
👤 创建者: <创建者姓名>
👥 参与者: <参与者姓名列表>
🔗 会议链接: <会议链接>
```

---

### 工作流 5: 根据关键词查找会议

**示例**: 用户说 "技术评审会议是什么时候?"

**查询策略:**

1. **确定查询范围**: 默认查当日前后 30 天 (接口限制范围)。
2. **拉取会议列表**:

```bash
wecom-cli meeting list_user_meetings '{"begin_datetime": "2026-02-15 00:00", "end_datetime": "2026-04-16 23:59", "limit": 100}'
```

3. **逐个查询详情并匹配标题关键词**。
4. **找到匹配后停止查询，展示结果**:

#会议号: <会议号>

```
✅ 找到会议: "<会议标题>"

📅 时间: <开始时间>，时长 <时长>
📍 地点: <会议地点>
👥 参与者: <参与者姓名列表>
🔗 会议链接: <会议链接>
```

5. **未找到处理**: 告知用户在前后 30 天范围内未找到匹配会议，请确认会议名称。

---

### 工作流 6: 取消会议

**示例**: 用户说 "帮我取消明天的技术方案评审会议"

**步骤:**

1. **定位会议**: 通过 `list_user_meetings` + `get_meeting_info` 查询会议列表 + 关键词匹配找到目标会议。
2. **直接执行取消**:

```bash
wecom-cli meeting cancel_meeting '{"meetingid": "<target_meetingid>"}'
```

3. **展示结果**:

```
✅ 会议已取消: 技术方案评审
```

---

### 工作流 7: 更新会议成员

**示例**: 用户说 "把王五加到技术方案评审会议里"

**步骤:**

1. **定位会议**: 通过 `list_user_meetings` + `get_meeting_info` 查询会议列表 + 匹配找到目标会议。
2. **获取当前受邀成员**: `set_invite_meeting_members` 为全量覆盖，必须先通过 `get_meeting_info` 获取会议详情，获取现有成员后再合并。
3. **通讯录查询**: 调用 `wecomcli-contact` 技能获取通讯录成员，按姓名筛选出王五的 userid。

```bash
wecom-cli contact get_userlist '{}'
```

在返回的 `userlist` 中筛选 `name` 包含 "王五" 的成员，获取其 `userid`。

4. **合并成员列表**: 将现有成员 + 新增成员合并 (全量覆盖)。
5. **执行更新**:

```bash
wecom-cli meeting set_invite_meeting_members '{"meetingid": "<target_meetingid>", "invitees": [{"userid": "zhangsan"}, {"userid": "lisi"}, {"userid": "wangwu"}]}'
```

6. **展示结果**:

```
✅ 会议成员已更新: 技术方案评审
👥 当前成员: 张三，李四，王五
```
