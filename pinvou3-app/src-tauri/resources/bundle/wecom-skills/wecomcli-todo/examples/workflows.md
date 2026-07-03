# 典型工作流

### 工作流 1：查看待办列表（标准三步流程）

**用户意图**："看看我最近的待办" / "我有哪些待办事项？" / "我还有多少事要做？"

**步骤：**

1. **获取待办列表**（只有 ID 和状态，没有内容）：
```bash
wecom-cli todo get_todo_list '{}'
```

2. **用返回的 todo_id 列表获取完整详情**（禁止跳过！）：
```bash
wecom-cli todo get_todo_detail '{"todo_id_list": ["返回的TODO_ID_1", "返回的TODO_ID_2"]}'
```

3. **通过 wecomcli-contact 获取通讯录**，将 follower_id / creator_id 转为姓名：
```bash
wecom-cli contact get_userlist '{}'
```
用返回的 userlist 中的 userid 匹配 follower_id 和 creator_id，取 name 字段作为展示姓名。

4. **检查分页**：检查第一步返回的 `has_more` 字段。如果为 `true`，在展示结果时必须提醒用户："以上是部分待办，还有更多待办未显示，需要我继续查看吗？"

**展示格式（分派人和创建人必须显示为姓名，不是 ID）：**

```
📋 您当前的待办事项（共 3 项）

1. 🔵 完成Q2规划文档
   - 待办状态：进行中 | 我的状态：已接受
   - 提醒时间：2025-06-01 09:00
   - 分派人：张三、李四
   - 创建时间：2025-01-15

2. 🔵 提交周报
   - 待办状态：进行中 | 我的状态：已接受
   - 提醒时间：2025-03-17 18:00
   - 创建时间：2025-03-10

3. ☑️ 代码评审
   - 待办状态：已完成 | 我的状态：已完成
   - 创建时间：2025-03-01
```

---

### 工作流 2：按时间范围查询待办

**用户意图**："这个月创建的待办有哪些？"

**步骤：**

1. **按时间过滤获取列表**：
```bash
wecom-cli todo get_todo_list '{"create_begin_time": "2025-03-01 00:00:00", "create_end_time": "2025-03-31 23:59:59"}'
```

2. **获取详情**（同工作流 1 的第 2、3 步）。

---

### 工作流 3：创建待办并分派给同事

**用户意图**："帮我创建一个待办，让张三下周一前完成需求文档"

**步骤：**

1. **通讯录查询**：通过 wecomcli-contact 技能查询张三的 userid，在返回结果中筛选姓名为"张三"的成员，获取其 userid。
```bash
wecom-cli contact get_userlist '{}'
```

2. **创建待办并分派**：
```bash
wecom-cli todo create_todo '{"content": "<待办的内容>", "follower_list": {"followers": [{"follower_id": "zhangsan", "follower_status": 1}]}, "remind_time": "2025-03-24 09:00:00"}'
```

3. **展示结果**：
```
✅ 待办创建成功！

📝 内容：完成需求文档
👥 分派人：张三
⏰ 提醒时间：2025-03-24 09:00
```

> `follower_id` 必须来自 `wecomcli-contact` 技能的 `get_userlist` 接口返回的 `userid`，禁止自行猜测。若搜索结果有多个同名人员，需展示候选列表让用户确认。

---

### 工作流 4：标记待办完成

需要区分两种场景：**标记待办本身完成**（改 `todo_status`）和**标记我的参与状态为完成**（改 `user_status`）。

#### 场景 A：标记待办本身完成

**用户意图**："把'完成Q2规划文档'这个待办标记为完成" / "关闭这个待办"

1. **查找目标待办**：
```bash
wecom-cli todo get_todo_list '{}'
```
2. **获取详情确认**：
```bash
wecom-cli todo get_todo_detail '{"todo_id_list": ["TODO_ID"]}'
```
3. **将待办状态改为已完成**：
```bash
wecom-cli todo update_todo '{"todo_id": "TODO_ID", "todo_status": 0}'
```

#### 场景 B：标记我的参与状态为完成

**用户意图**："我已经完成了这个待办" / "标记我的部分为完成"

1. **查找目标待办**：
```bash
wecom-cli todo get_todo_list '{}'
```
2. **获取详情确认**：
```bash
wecom-cli todo get_todo_detail '{"todo_id_list": ["TODO_ID"]}'
```
3. **变更当前用户的参与状态为已完成**：
```bash
wecom-cli todo change_todo_user_status '{"todo_id": "TODO_ID", "user_status": 2}'
```

> **如何判断用户意图：** 如果用户说"标记完成"且该待办是自己创建的、没有其他分派人，通常指场景 A（标记待办本身完成）。如果该待办有多个参与人，用户可能只是想标记自己那部分完成（场景 B）。不确定时应向用户确认。

> 用户提供的是待办内容描述而非 ID，所以需要先通过 `get_todo_list` 和 `get_todo_detail` 查找再匹配。匹配到多个相似待办时，列出候选项让用户确认。

---

### 工作流 5：更新待办内容或提醒时间

**用户意图**："把那个需求文档的待办提醒时间改到下周五"

**步骤：**

1. **查找目标待办**：
```bash
wecom-cli todo get_todo_list '{}'
```
再查详情：
```bash
wecom-cli todo get_todo_detail '{"todo_id_list": ["TODO_ID_1", "TODO_ID_2"]}'
```

2. **确认目标后更新**：
```bash
wecom-cli todo update_todo '{"todo_id": "TODO_ID", "remind_time": "2025-03-28 09:00:00"}'
```

3. **展示结果**：
```
✅ 待办已更新！

📝 内容：完成需求文档
⏰ 新提醒时间：2025-03-28 09:00
```

---

### 工作流 6：删除待办

**用户意图**："删掉'代码评审'那个待办"

**步骤：**

1. **查找目标待办**：
```bash
wecom-cli todo get_todo_list '{}'
```
再查详情：
```bash
wecom-cli todo get_todo_detail '{"todo_id_list": ["TODO_ID"]}'
```

2. **向用户确认后删除**：
```bash
wecom-cli todo delete_todo '{"todo_id": "TODO_ID"}'
```

3. **展示结果**：
```
✅ 待办已删除：代码评审
```

> 删除前必须向用户确认，确认措辞示例："确认删除待办'代码评审'吗？删除后不可恢复。"

---

### 工作流 7：分页获取大量待办

当待办数量超过单页上限时，通过 `cursor` 循环分页拉取：

- 首次请求（不传 cursor）：
```bash
wecom-cli todo get_todo_list '{"limit": 20}'
```
如果没有拉取完，还有更多的待办，会返回 has_more=true, next_cursor="CURSOR_1"

- 第二次请求（传入上次的 next_cursor）：
```bash
wecom-cli todo get_todo_list '{"limit": 20, "cursor": "CURSOR_1"}'
```
返回 has_more=false，拉取完毕

**分页规则：**
- 首次请求不传 `cursor`
- `has_more` 为 `true` 时，将 `next_cursor` 作为下次请求的 `cursor` 传入
- `has_more` 为 `false` 时停止请求
- 分页过程中时间过滤参数保持不变
- **如果选择不继续翻页（比如当前页数据已经够用），必须告诉用户还有更多待办未显示，问用户是否需要继续查看**
