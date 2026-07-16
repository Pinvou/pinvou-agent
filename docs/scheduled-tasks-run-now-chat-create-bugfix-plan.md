# 定时任务开发方案与自动化测试方案

## 背景

当前已定位两个未完成修复的问题：

1. 点击定时任务详情里的“立即运行”可能报错。
2. 在聊天中创建的任务没有出现在定时任务列表。

这两个问题表面都发生在定时任务模块，但根因不同：

- “立即运行”是模型绑定数据不稳定导致运行时无法唯一解析模型。
- “聊天创建任务不显示”是聊天流程绕过了 pinvou3 内部定时任务接口，创建成了 Windows 系统任务。

本方案目标是让定时任务全链路都走 pinvou3 自有 `create_scheduled_task` / `run_scheduled_task_now` / automation 存储，避免系统任务和模型配置歧义。

维护约定：

- 定时任务模块只维护这一份方案文档。
- 后续定时任务相关 UI、后端、测试方案都追加或修订到本文档，不再新建平行方案文档。
- 已完成项保留作为回归依据，新增需求按“问题/目标/实现/测试”结构补充。

## 问题 1：立即运行报错

### 已定位现象

前端定时任务编辑弹窗的 AI 模型列表来自：

- `pinvou3-app/src/features/scheduled/ScheduledTasksView.jsx`
- `appState.savedModels`

当前代码用 `model.model` 作为下拉 value：

```js
const modelOptions = (appState.savedModels || []).map(model => ({
  value: model.model,
  label: model.name || model.model,
}));
```

如果用户保存了多个模型配置，但它们的 wire model name 相同，例如：

```text
id=deepseek-a, model=deepseek-v4-flash
id=deepseek-b, model=deepseek-v4-flash
```

前端会出现重复选项，React 也会报重复 key 警告：

```text
Encountered two children with the same key, `deepseek-v4-flash`
```

后端立即运行时，定时任务只持久化了 `model`，没有稳定 `model_id`。运行链路为：

```text
run_scheduled_task_now
  -> ScheduledTaskState::run_task_now
  -> AutomationManager::run_now
  -> TaskManager add_task_with_conversation_key
  -> ScheduledChatExecutor::execute
  -> ScheduledRunProfile { model, model_id: None }
  -> EnginePool::resolve_scheduled_model
```

`resolve_scheduled_model` 如果没有 `model_id`，只能按 `model` 反查。当同名模型超过一个时，会报：

```text
Scheduled model '<model>' is ambiguous without a stable model id
```

### 根因

定时任务持久数据没有稳定保存模型配置 ID，只保存了模型 wire name。

### 修复目标

- 模型下拉不再用 wire name 作为唯一值。
- 创建、编辑、立即运行都使用稳定模型 ID 绑定模型配置。
- 已有只保存 `model` 的旧任务仍能运行，不能直接破坏历史数据。
- 用户删除或修改模型配置后，错误提示必须可读。

### 推荐实现

#### 1. 后端增加定时任务模型绑定存储

为了尽量减少 DeepSeek-TUI fork 改动，优先在 app 层新增 sidecar 存储，而不是直接扩展底座 `AutomationRecord`。

新增存储：

```text
~/.pinvou3/automations/model-bindings.json
```

结构建议：

```json
{
  "schema_version": 1,
  "tasks": {
    "<automation_id>": {
      "model_id": "deepseek-a",
      "model": "deepseek-v4-flash",
      "updated_at": "2026-07-15T..."
    }
  }
}
```

落点：

- `pinvou3-app/src-tauri/src/scheduled_tasks.rs`
- 新增 `ScheduledTaskModelBindingStore`
- `ScheduledTaskState` 持有该 store

行为：

- 创建任务时，如果输入含 `model_id`，保存 `{automation_id -> model_id, model}`。
- 更新任务模型时，同步更新 binding。
- 删除任务时删除 binding。
- 列表/详情 DTO 返回 `model_id`，供前端回显。

#### 2. 扩展 Tauri 输入/DTO

扩展：

```rust
pub struct ScheduledTaskDto {
    pub model: Option<String>,
    pub model_id: Option<String>,
}

pub struct CreateScheduledTaskInput {
    pub model: Option<String>,
    pub model_id: Option<String>,
}

pub struct UpdateScheduledTaskInput {
    pub model: Option<String>,
    pub model_id: Option<String>,
}
```

注意：

- 传给底座 `AutomationManager` 的仍然是 `model` wire name。
- `model_id` 只用于 app 层稳定绑定。

#### 3. ScheduledChatExecutor 使用稳定模型 ID

当前 `ScheduledChatExecutor::execute` 只能从 `ExecutionTask` 拿到 `task.model()` 和 `task.conversation_key()`。

改造建议：

- 给 `ScheduledChatExecutor` 注入一个 model binding resolver。
- resolver 输入 `automation_id` 和 `model`，输出 `Option<model_id>`。
- `conversation_key()` 当前就是 automation id，可以用于查 binding。

伪代码：

```rust
let automation_id = task.conversation_key();
let model_id = automation_id
    .and_then(|id| self.runtime.model_id_for_automation(id, task.model()));

let profile = ScheduledRunProfile {
    task_id: automation_id.unwrap_or_else(|| task.id()).to_string(),
    model: task.model().to_string(),
    model_id,
    ...
};
```

保留兼容：

- 如果 binding 缺失，继续走现有 `bind_profile_model_id` fallback。
- 如果 fallback 仍歧义，返回明确错误，提示用户重新选择模型并保存任务。

#### 4. 前端模型下拉改为使用 model.id

`ScheduledTasksView.jsx`：

- 下拉 value 使用 `model.id`。
- label 使用 `model.name`，必要时附加 wire name 作为辅助文案。
- 编辑 form 内部保存 `modelId` 和 `model`。

建议结构：

```js
const modelOptions = uniqueSavedModels.map(model => ({
  value: model.id,
  label: model.name || model.model,
  model: model.model,
}));
```

选择时：

```js
const selectedModel = savedModels.find(item => item.id === value);
editImmediateField('model', selectedModel.model, { modelId: selectedModel.id });
```

创建时：

```js
model: activeModel.model,
modelId: activeModel.id,
```

需要同步 bridge 字段白名单：

```js
var SCHEDULED_TASK_WRITABLE_FIELDS = ["name", "prompt", "rrule", "model", "modelId", "paused"];
```

Tauri 参数会自动转成 `model_id`，或 bridge 手动映射。

#### 5. 错误提示优化

当运行报错是模型歧义或模型缺失时，前端应该显示明确动作：

```text
此任务绑定的 AI 模型配置已失效或不唯一，请重新选择 AI 模型并保存。
```

不要只显示底层英文报错。

## 问题 2：聊天创建任务不显示在定时任务列表

### 已定位现象

会话：

```text
C:\Users\123\.pinvou3\sessions\we06tk78syjd0.json
```

实际流程是：

- AI 写了 PowerShell 脚本。
- AI 通过 Windows `schtasks` 创建了系统计划任务。
- 任务名：`AI News Push`
- 频率：每小时。

这个任务存在于 Windows 任务计划程序，不存在于：

```text
C:\Users\123\.pinvou3\automations\automations\*.json
```

所以 pinvou3 定时任务模块不会显示它。

### 根因

聊天创建没有强制走 pinvou3 内部定时任务创建协议。AI 可以绕过模块，直接调用系统定时器。

### 修复目标

- 从 pinvou3 的“AI 聊天创建”入口创建任务时，必须调用 `create_scheduled_task`。
- 不允许在该流程中通过 `schtasks`、Windows Task Scheduler、cron、系统服务等方式创建任务。
- 创建成功后任务必须立刻出现在定时任务列表。
- 对不支持的时间规则，例如“每 5 分钟”，必须询问用户改成支持的模式，而不是自行创建系统任务。

### 推荐实现

#### 1. 收紧 `SCHEDULED_TASK_CHAT_PROMPT`

文件：

```text
pinvou3-app/src-tauri/src/scheduled_tasks.rs
```

更新 `SCHEDULED_TASK_CHAT_PROMPT`，明确要求：

- 只能收集定时任务参数。
- 只能输出前端可解析的定时任务 JSON 草稿。
- 不得写脚本。
- 不得调用 `schtasks`、Task Scheduler、cron、systemd timer。
- 不得创建系统级计划任务。
- 不得要求用户选择工作目录、权限、Shell、信任模式。
- 支持时间规则仅限：
  - 每 N 小时
  - 每天指定时间
  - 每周指定星期和时间
- 遇到分钟级需求必须询问用户改成哪个支持规则。

提示词中要显式包含反例：

```text
错误做法：使用 schtasks 创建 Windows 任务。
正确做法：返回 JSON 草稿，由 pinvou3 调用 create_scheduled_task。
```

#### 2. bridge 层强制创建路径

文件：

```text
pinvou3-app/src/tauri-bridge.js
```

现有相关函数：

- `startScheduledTaskChat`
- `normalizeScheduledTaskDraft`
- `autoCreateScheduledTaskDraft`
- `confirmScheduledTaskDraft`
- `createScheduledTask`

需要强化：

- 从 `startScheduledTaskChat` 进入的会话设置 `scheduledTaskCreationSessionId`。
- 该会话中一旦解析出合法 draft，必须调用 `createScheduledTask`。
- 创建成功后清除 draft，刷新 `loadScheduledTasks()`。
- 创建失败时显示定时任务模块错误。

#### 3. 工具权限约束

如果当前引擎支持 per-session 工具控制，应在定时任务创建会话里禁用高风险工具：

- `exec_shell`
- `write_file`
- `edit_file`
- 与系统计划任务相关的系统命令

如果当前没有 per-session 工具控制，先做两层防线：

1. prompt 层明确禁止。
2. bridge 层只承认解析出的 scheduled task draft；不会把脚本或系统任务纳入定时任务模块。

后续增强项：

- 在定时任务创建会话里检测 assistant/tool 输出中出现 `schtasks`、`Task Scheduler`、`cron`、`systemd timer`，显示警告：

```text
该操作创建的是系统任务，不会进入 pinvou3 定时任务模块。请改用 pinvou3 定时任务创建。
```

#### 4. 普通聊天中的“创建定时任务”意图

当前问题发生在会话里，用户可能不是从定时任务页的“AI 聊天创建”入口进入，而是在普通聊天里说“创建一个定时任务”。

建议分两期：

第一期：

- 只保证“AI 聊天创建”入口严格走 pinvou3 内部创建。

第二期：

- 普通聊天中检测到“创建定时任务 / 每天 / 每小时 / 定时推送”等意图时，引导用户切换到定时任务创建流程。
- 不让普通聊天直接用 `schtasks` 完成任务。

#### 5. 已有 Windows 任务处理

已有 `AI News Push` 不会自动出现在 pinvou3 定时任务里。

可选处理：

- 提供一次性“导入 Windows 任务”功能，不作为本次修复必需范围。
- 或提示用户用 pinvou3 定时任务模块重新创建。

## 问题 3：定时任务记录缺少一致的任务操作能力

### 当前结论

定时任务模块里有两类对象，操作归属必须分清（完整归属表见文末「交接说明」§二，那里是单一真相源）：

- 定时任务定义：主界面里的任务配置，负责名称、执行内容、模型、重复规则、时间、启停、立即运行、置顶任务、打开任务工作间、删除定时任务定义。
- 定时运行对话：左侧栏“定时任务记录”里的每次运行会话，负责打开并继续追问、重命名、置顶/取消置顶、收纳、删除本次运行、打开共享工作间。

记录级的重命名、置顶、收纳、删除入口放在左侧栏“定时任务记录”，不放在定时任务主列表——因为它们作用的对象是运行会话，不是任务定义。这是**对象归属**问题，不是“任务列表做不了置顶/删除”。

### 开发方案

1. 左侧栏定时任务记录复用现有 `RecentItem` 会话项交互。
   - hover 显示置顶、重命名、删除、更多按钮。
   - 右键记录打开同一个更多菜单。
   - 菜单内包含置顶/取消置顶、重命名、删除、打开文件夹。
   - 定时任务记录保留时钟图标和运行状态副标题。
   - 更多菜单必须使用 `fixed + portal` 渲染到 `document.body`，不能作为行内 `absolute` 子元素挂在侧边栏滚动容器内，避免被侧边栏右边界或滚动区域裁剪。

2. 定时任务记录操作走**普通 session 命令**，后端按 `SessionKind` 分发（前端不分叉）。
   - 重命名调用 `rename_session`，复用 Session 元数据，只改本次运行标题。
   - 置顶调用 `set_session_pinned`，记录进入左侧栏“置顶任务”分组。
   - 收纳调用 `set_session_archived`，记录移出侧边栏进设置页“任务收纳”，可还原。
   - 删除调用 `delete_session`；后端对 `ScheduledRun` 分发到 `delete_run_for_session`，联动删除该次 Session + Run + 底座 Task，保留任务定义、共享工作间和其他运行记录。
   - 不调用 `delete_scheduled_task`，避免误删定时任务定义。

3. 置顶后的定时任务记录仍按定时运行上下文打开。
   - 如果置顶项对应 scheduled run session，点击时调用 `openScheduledRunChat`。
   - 不退化为普通 `switchToSession`，避免丢失定时任务返回路径和运行上下文。

4. 主定时任务列表当前的入口布局。
   - 行级不显示置顶按钮和更多菜单（避免与左侧栏记录菜单混淆）。
   - 不按置顶/普通任务分组。
   - 详情弹窗提供编辑、启停、立即运行、打开工作间和删除任务定义；任务置顶后端能力已存在，但当前 UI 没有入口。

5. bridge 行为调整。
   - `renameSession` / `toggleSessionPinned` / `archiveSession` / `deleteSession` 对定时运行会话与普通会话**发同一条命令**，分发在后端。
   - `deleteSession` 删除后同步移除 `scheduledTaskRecentRuns` 与 `scheduledTaskRuns` 里的对应记录。
   - 定时运行正文、附件、工作集仍由后端托管，不走普通前端持久化路径。

6. 侧边栏空间策略。
   - 已撤回“缩减主导航项高度/间距”的改动，主导航项高度、字号、图标间距继续沿用原有侧边栏规范。
   - 后续如果要给“置顶任务 / 任务 / 定时任务记录”释放更多空间，不通过压缩主导航项解决。
   - 当前仍存在一个待优化问题：侧边栏里其他主导航模块占用的垂直空间偏多，导致“置顶任务 / 任务 / 定时任务记录”的实际展示区域较小，任务记录可见数量不足。
   - 该问题需要在保持主导航视觉规范一致的前提下优化，不能简单缩小主导航项高度、字号或图标间距。
   - 可选方向：
     - 优化历史分组默认展开策略。
     - 减少非当前分组同时展开的内容。
     - 对记录区做更明确的剩余高度分配。
     - 控制单个分组展示数量并提供进入完整列表的入口。

### 自动化测试补充

1. 静态测试：
   - 断言主定时任务列表不包含 `scheduled-task-pin`、`scheduled-task-actions`、`scheduled-task-action-menu`、`scheduled-detail-actions`。
   - 断言左侧栏“定时任务记录”通过 `RecentItem` 渲染。
   - 断言 `RecentItem` 支持右键菜单、置顶、重命名、删除。

2. bridge 行为测试（`scheduledRunRecordSessionActionsBehavior`，已实现）：
   - 加载一条定时任务运行记录。
   - `renameSession` → 断言调用 `rename_session`（不是任何 `*_scheduled_run_record` 平行命令）。
   - `toggleSessionPinned` → 断言调用 `set_session_pinned`。
   - `archiveSession` → 断言调用 `set_session_archived`，记录离开侧边栏；重新 load 后仍不回流（`archived` 由后端 DTO 携带）；`restoreArchivedSession` 后回到侧边栏。
   - `deleteSession` → 断言调用 `delete_session`、侧边栏记录缓存被移除、**没有**调用 `delete_scheduled_task`。

3. 后端命令测试（`scheduled_session_metadata_dispatch_supports_rename_pin_archive`，`commands.rs`，已实现）：
   - 真实 `SessionStore` 创建 `sched-*` 会话，断言 `session_kind` 为 `ScheduledRun`。
   - `set_title` → reload 后标题落盘生效（`rename_session` 的真实路径）。
   - `set_pinned` → `is_pinned` / `pinned_at` 生效（`set_session_pinned` 的真实路径）。
   - `set_hidden` → `is_hidden` 生效、`list_scheduled()` 能列出、收起强制取消置顶（`set_session_archived` 的真实路径）。
   - `store.delete()` 仍拒绝 `sched-*`（删除只能走 automation 联动，不能当普通会话直删）。

4. 冒烟测试（已实现）：
   - 展开左侧栏“定时任务记录”，打开某条记录的更多菜单。
   - 验证菜单包含重命名、置顶、删除、收纳，且 `fixed + portal` 完整显示不被侧边栏裁剪。
   - 执行重命名 → 断言 `rename_session` 带 `id: 'sched-run-1'`，标题更新。
   - 执行置顶 → 断言 `set_session_pinned` 带 `id: 'sched-run-1'`，进入“置顶任务”分组，且**没有**调用 `set_scheduled_task_pinned`。
   - 执行删除并取消 → 断言未调用 `delete_session` / `delete_scheduled_task`，记录仍在。

### 已废弃方案

一个已撤回方向：通过缩减侧边栏主导航项高度、字号、图标间距来释放历史记录空间。该方向会破坏侧边栏主导航与其他模块的一致性，已从代码中回退。保留的修复仅包括：定时任务记录菜单使用 `fixed + portal`，解决菜单显示不全。

另一个**已废弃的实现草稿**（本次修复中删除）：为定时运行记录新增 `rename_scheduled_run_record` / `set_scheduled_run_record_pinned` / `delete_scheduled_run_record` / `list_recent_scheduled_runs` 四个平行 Tauri 命令，前端按记录类型分叉调用。废弃原因：那等于给 `sched-*` 会话建了一套平行 Session 系统，与「复用现有 Session/Run/RecentItem」冲突。正确做法是让普通 Session 命令在后端按 `SessionKind` 分发。

## 问题 4：新建/编辑任务弹窗内嵌设置组外线框不统一

### 当前结论

新建任务、基于模板创建任务、编辑任务弹窗里的“重复/日期/时间”“AI 模型/重复/日期/时间”等内嵌设置组，当前使用了浅灰底加外层 `border`。这会和上方“任务名称”“执行内容”输入框的无边框浅灰块不一致，视觉上像套了另一种控件系统。

这类设置组属于弹窗内部的嵌入式表单区域，不是独立浮层，不需要外描边。应保持 iOS 风格的浅灰底圆角容器，只保留内部行分割线。

### 开发方案

1. 新增/明确样式语义：
   - `iosInsetSurface`：弹窗内部嵌入式浅灰块背景。
   - `iosInsetSeparator`：嵌入式设置组内部行分割线。
   - `iosFloatingBorder`：浮层菜单/时间滚轮/删除确认等独立层的边框。

2. 去掉以下嵌入式容器的外层 `border`：
   - 新建任务/基于模板创建任务的计划设置组。
   - 编辑任务的 `scheduled-detail-settings` 设置组。
   - 编辑任务里的“立即运行 / 打开文件夹”操作组。
   - 编辑任务里的执行历史列表容器。

3. 保留内部行分割线：
   - “重复 / 日期 / 时间”行之间继续用 `border-b`。
   - “AI 模型 / 重复 / 日期 / 时间”行之间继续用 `border-b`。
   - “立即运行 / 打开文件夹”之间继续用单条分割线。
   - 执行历史记录之间继续用 `divide-y`。

4. 保留真正浮层的边框和阴影：
   - AI 模型、重复、日期下拉 popover。
   - 时间滚轮 popover。
   - 左侧栏定时任务记录更多菜单。
   - 删除确认弹窗。
   - 定时任务主列表外层卡片。

5. 增加稳定测试锚点：
   - `scheduled-create-settings`
   - `scheduled-detail-settings`
   - `scheduled-detail-actions-group`
   - `scheduled-run-history-list`

### 自动化测试补充

1. 静态测试：
   - 断言上述四个嵌入式容器不再带外层 `border`。
   - 断言 `ScheduledSelect`、`ScheduledTimeWheel`、删除确认弹窗仍保留边框。

2. 冒烟测试：
   - 打开新建任务弹窗，检查 `scheduled-create-settings` 外层边框宽度为 `0px`。
   - 打开编辑任务弹窗，检查 `scheduled-detail-settings`、`scheduled-detail-actions-group`、`scheduled-run-history-list` 外层边框宽度为 `0px`。
   - 打开下拉/时间滚轮，确认浮层仍有边框或阴影，不因本次修改丢失层级感。

### 已定位现象

当前定时任务列表主要支持：

- 点击任务行打开编辑弹窗。
- 在详情弹窗中编辑字段。
- 在详情弹窗中删除任务。
- 开关任务启停。
- 点击“立即运行”。

但它还没有与左侧近期任务/会话列表一致的任务操作体验，例如：

- 任务行无法直接置顶/取消置顶。
- 删除入口只在详情底部，用户需要先进入编辑弹窗。
- 行级操作没有统一的 `...` 菜单。
- 置顶任务与普通任务没有分区或稳定排序。
- 移动端/非 hover 场景下，操作入口不够稳定。

### 目标

- 定时任务支持与现有会话任务一致的操作模型：置顶、取消置顶、删除、编辑、启停、立即运行等。
- 任务行、详情弹窗、删除确认弹窗使用同一套状态和命令，不出现两个入口行为不同的问题。
- 置顶状态持久化，重启 app 后仍保留。
- 删除后列表、详情弹窗、侧边栏未读状态、运行记录状态都正确刷新。
- 交互保持 iOS/macOS 风格：行级轻操作、`...` 菜单承载更多动作、危险操作二次确认。
- 不破坏已有自动创建、立即运行、模型绑定逻辑。

### 交互设计

#### 1. 任务列表行级操作

定时任务行保持“点击整行打开编辑弹窗”的主路径，同时在右侧增加操作区。

桌面端：

- 默认显示任务状态开关。
- hover 或 keyboard focus 时显示：
  - 置顶/取消置顶图标按钮。
  - `...` 更多菜单按钮。
- 已置顶任务始终显示置顶标识，避免用户看不出排序原因。

移动端/窄屏：

- 不依赖 hover。
- 右侧始终显示 `...` 更多菜单。
- 置顶动作放入菜单中，不额外挤占空间。

推荐行级按钮：

```text
[PinIcon/PinOffIcon]  [MoreHorizontal]  [Switch]
```

按钮行为：

- 置顶/取消置顶：不打开编辑弹窗，`event.stopPropagation()`。
- 更多菜单：不打开编辑弹窗，`event.stopPropagation()`。
- 开关：沿用当前启停逻辑，`event.stopPropagation()`。
- 点击任务主体：打开编辑弹窗。

#### 2. 更多菜单内容

任务行 `...` 菜单建议项：

```text
编辑
立即运行
暂停 / 恢复
置顶 / 取消置顶
打开任务工作间
删除
```

规则：

- `立即运行` 在任务字段无效、任务正在保存、或全局 busyAction 时禁用。
- `暂停 / 恢复` 根据当前 `status` 动态切换。
- `打开任务工作间` 仅在后端支持 `openScheduledTaskFolder` 时显示或可用。
- `删除` 使用红色危险样式，并触发统一删除确认弹窗。
- 菜单支持点击外部关闭、`Escape` 关闭、滚动/窗口变化重算位置。

#### 3. 置顶分组与排序

“我的任务”区建议分两组：

```text
置顶任务
普通任务
```

如果没有置顶任务，不显示“置顶任务”标题，避免空分组占空间。

排序规则：

1. 置顶任务按 `pinnedAt` 倒序。
2. 普通任务按：
   - active/running 优先；
   - `nextRunAt` 升序；
   - 再按 `updatedAt` 或 `createdAt` 倒序兜底。
3. 筛选器“全部 / 已开启 / 已暂停”仍作用于两个分组。

说明：

- 不建议把置顶任务混在普通任务里只靠图标区分，用户无法稳定预期排序。
- 不建议置顶改变 schedule 本身，置顶只属于 UI 元数据。

#### 4. 详情编辑弹窗操作入口

编辑弹窗顶部保留标题和关闭按钮。

新增顶部右侧或标题旁的 `...` 操作菜单，菜单项与行级菜单保持一致：

```text
立即运行
暂停 / 恢复
置顶 / 取消置顶
打开任务工作间
删除
```

详情弹窗底部不再重复放多个危险/导航按钮，避免用户之前反馈的“按钮在底部不可见”和“取消与关闭重复”问题。推荐：

- 文本字段继续自动保存。
- 顶部关闭按钮负责退出。
- 关键操作进 `...` 菜单。
- 删除进入 iOS 确认弹窗。

如果短期不重构底部按钮，也必须保证：

- 行级删除和详情删除触发同一个确认组件。
- 删除确认文案一致。
- 删除成功后的状态清理一致。

#### 5. 删除确认

删除确认沿用 iOS alert 样式：

```text
删除定时任务？
“每日早报”将被删除，此操作无法撤销。

取消 | 删除
```

行为：

- 点击背景不建议直接删除，只关闭确认或无操作。
- `Escape` 等同取消。
- 删除按钮红色。
- 删除中禁用所有确认按钮。
- 删除成功：
  - 关闭确认弹窗。
  - 如果当前任务详情弹窗打开，则关闭详情。
  - 从列表移除任务。
  - 清理选中任务 ID。
  - 刷新任务列表、任务详情、运行记录、侧边栏未读聚合。

#### 6. 空态与筛选状态

新增分组和置顶后，空态要区分：

- 没有任何任务：显示“没有匹配的定时任务”或当前空态。
- 当前筛选无结果：显示“没有已开启任务 / 没有已暂停任务”。
- 有置顶任务但筛选不匹配：不显示置顶分组。

### 后端数据设计

#### 1. 新增定时任务 UI 元数据 sidecar

不建议为置顶直接改 DeepSeek-TUI 的 `AutomationRecord`，避免扩大 fork 差异。推荐新增 app 层 sidecar：

```text
~/.pinvou3/automations/task-ui-metadata.json
```

结构：

```json
{
  "schema_version": 1,
  "tasks": {
    "<automation_id>": {
      "pinned": true,
      "pinned_at": "2026-07-15T10:00:00+08:00",
      "updated_at": "2026-07-15T10:00:00+08:00"
    }
  }
}
```

说明：

- `pinned=false` 的任务可以直接从 sidecar 中删除，减少冗余。
- list/read 时发现 automation 已不存在，应 compact 清理孤儿 metadata。
- 删除任务时同步删除 metadata。
- 该 sidecar 与已有 `model-bindings.json` 职责分离：
  - `model-bindings.json` 管运行时模型解析。
  - `task-ui-metadata.json` 管 UI 行为。

#### 2. DTO 扩展

扩展 `ScheduledTaskDto`：

```rust
pub struct ScheduledTaskDto {
    pub pinned: bool,
    pub pinned_at: Option<String>,
    // existing fields...
}
```

前端字段使用 camelCase：

```js
task.pinned
task.pinnedAt
```

#### 3. Tauri 命令

推荐新增明确命令：

```rust
#[tauri::command]
pub async fn pin_scheduled_task(id: String) -> Result<ScheduledTaskDto, String>

#[tauri::command]
pub async fn unpin_scheduled_task(id: String) -> Result<ScheduledTaskDto, String>
```

也可以用统一命令：

```rust
#[tauri::command]
pub async fn set_scheduled_task_pinned(id: String, pinned: bool) -> Result<ScheduledTaskDto, String>
```

推荐统一命令，原因：

- bridge 只需要一个 `toggleScheduledTaskPinned(id, pinned)`。
- 前端 optimistic update 更简单。
- 后续如果增加 `favorite`、`hidden` 等 UI 元数据，可以复用 metadata store。

#### 4. 删除命令补齐清理职责

现有 `delete_scheduled_task` 需要保证清理：

- automation record。
- model binding。
- task UI metadata。
- 选中任务详情。
- 与该 automation 相关的 run read-state。
- 最近运行 shortcut 中该任务的聚合状态。

如果运行记录本身需要保留用于审计，则不要删除 run history；但列表聚合必须重新计算，避免侧边栏残留未读红点。

#### 5. 并发与失败处理

- pin/unpin 是轻操作，可 optimistic update，但后端失败必须回滚并显示错误。
- 删除不能 optimistic 移除后静默失败；失败时应保留任务并显示错误。
- 如果任务正在保存文本字段，pin/delete/立即运行前必须先 `flushBeforeAction()`，与当前详情编辑逻辑一致。
- 如果任务刚被另一个窗口删除，pin/unpin/delete 返回“任务不存在”时，前端应刷新列表并关闭相关弹窗。

### 前端实现方案

#### 1. bridge API

文件：

```text
pinvou3-app/src/tauri-bridge.js
```

新增方法：

```js
async function toggleScheduledTaskPinned(id, pinned) {
  state.scheduledTaskBusyAction = pinned ? 'pin' : 'unpin';
  notify();
  try {
    const updated = await invoke('set_scheduled_task_pinned', { id, pinned });
    mergeScheduledTask(updated);
    if (state.scheduledTaskDetail && state.scheduledTaskDetail.id === id) {
      state.scheduledTaskDetail = updated;
    }
    return updated;
  } finally {
    state.scheduledTaskBusyAction = null;
    notify();
  }
}
```

注意：

- `mergeScheduledTask` 应保留任务列表排序由 view 层完成，避免 bridge 和 UI 各排一次。
- `loadScheduledTasks()` 仍是最终一致性兜底。
- 删除成功后如果 `selectedScheduledTaskId === id`，需要置空并清空 detail/runs。

#### 2. ScheduledTasksView 状态与排序

文件：

```text
pinvou3-app/src/features/scheduled/ScheduledTasksView.jsx
```

新增 helper：

```js
function sortScheduledTasks(tasks) {
  return [...tasks].sort((a, b) => {
    if (!!a.pinned !== !!b.pinned) return a.pinned ? -1 : 1;
    if (a.pinned && b.pinned) return String(b.pinnedAt || '').localeCompare(String(a.pinnedAt || ''));
    // active/running + nextRunAt + updatedAt fallback
  });
}
```

分组：

```js
const filteredTasks = sortScheduledTasks(tasks).filter(...);
const pinnedTasks = filteredTasks.filter(task => task.pinned);
const regularTasks = filteredTasks.filter(task => !task.pinned);
```

渲染：

```jsx
{pinnedTasks.length > 0 && <TaskGroup title="置顶任务" tasks={pinnedTasks} />}
<TaskGroup title={pinnedTasks.length ? "普通任务" : null} tasks={regularTasks} />
```

#### 3. 行级菜单组件

建议在 `ScheduledTasksView.jsx` 内部先实现局部组件，不急着抽全局组件：

```jsx
const ScheduledTaskActionMenu = ({ task, anchor, onEdit, onRunNow, onTogglePaused, onTogglePinned, onOpenFolder, onDelete }) => ...
```

要求：

- 使用 `createPortal` 渲染到 `document.body`，避免被列表 overflow 裁剪。
- 与 `ScheduledSelect` 一样支持：
  - 外部点击关闭。
  - `Escape` 关闭。
  - resize/scroll 重算。
- 菜单项高度、圆角、阴影与现有 iOS select/menu 保持一致。
- 所有 action 都要 `stopPropagation()`。

#### 4. 复用删除确认

当前已有：

```js
deleteConfirmId
requestDeleteTask
cancelDeleteTask
confirmDeleteTask
```

改造为：

```js
deleteConfirmTask
requestDeleteTask(event, task)
```

原因：

- 确认弹窗展示需要 `task.name`。
- 删除时如果任务已从列表刷新掉，仍能展示用户刚点击的任务名。

#### 5. 详情菜单

在 `DetailTaskDialog` header 中加入更多按钮：

```jsx
<button data-testid="scheduled-detail-actions">
  <MoreHorizontal />
</button>
```

菜单复用行级 action builder，区别：

- `编辑` 项在详情内隐藏。
- `立即运行`、`暂停/恢复`、`置顶/取消置顶`、`打开任务工作间`、`删除` 保留。

#### 6. 图标与视觉

使用现有图标：

- `PinIcon`
- `PinOffIcon`
- `MoreHorizontal`
- `Trash2`
- `Play`
- `FolderOpen`
- `Pause` 或开关文案

不要新增手写 SVG。

视觉约束：

- 卡片圆角沿用当前定时任务列表。
- 不把操作按钮做成大块文字胶囊；优先使用图标按钮 + tooltip/title。
- 删除菜单项红色。
- 置顶标识使用小图标或小圆点，不增加大标签导致行高变化。

### 范围边界

本次包含：

- 置顶/取消置顶。
- 行级更多菜单。
- 详情更多菜单。
- 删除入口统一。
- DTO/bridge/sidecar 持久化。
- 自动化测试覆盖。

本次不包含：

- 拖拽排序。
- 批量选择/批量删除。
- 归档定时任务。
- 导入 Windows 任务计划程序任务。
- 删除历史运行会话的二次策略变更。

## 自动化测试方案

### 一、前端静态回归测试

文件：

```text
pinvou3-app/tests/scheduled_tasks_unit.js
```

新增断言：

1. 模型下拉使用 `model.id`，不是 `model.model`。
2. 创建任务 payload 包含 `modelId`。
3. 更新任务模型 payload 包含 `modelId`。
4. 不再出现重复 key 风险。
5. `SCHEDULED_TASK_CHAT_PROMPT` 包含：
   - `create_scheduled_task`
   - 禁止 `schtasks`
   - 禁止 Windows Task Scheduler
   - 禁止 cron/systemd timer
   - 不支持分钟级时必须询问

示例断言：

```js
assert.ok(/value:\s*model\.id/.test(indexHtml));
assert.ok(/modelId:\s*activeModel\.id/.test(indexHtml));
assert.ok(/不得.*schtasks/.test(scheduledTaskPromptRust));
```

### 二、bridge 行为测试

文件：

```text
pinvou3-app/tests/scheduled_tasks_unit.js
```

新增 bridge harness 测试：

#### 1. 创建任务不自动弹编辑

已部分覆盖，继续保留：

- `selectAfterCreate: false` 时不调用 `selectScheduledTask(created.id)`。
- 任务进入 `state.scheduledTasks`。
- `state.scheduledTaskDetail` 不被新任务覆盖。

#### 2. 创建任务保留 modelId

新增：

```js
await bridge.createScheduledTask({
  name: "模型绑定任务",
  prompt: "run",
  rrule: "FREQ=HOURLY;INTERVAL=1",
  model: "deepseek-v4-flash",
  modelId: "deepseek-a",
});

assert.deepStrictEqual(invokeArgs.input, {
  name: "...",
  prompt: "...",
  rrule: "...",
  model: "deepseek-v4-flash",
  model_id: "deepseek-a",
  mode: "yolo",
});
```

#### 3. 聊天创建必须调用 createScheduledTask

模拟：

- `startScheduledTaskChat()`
- 用户发送“每小时推送 AI 新闻”
- mocked assistant 返回合法 draft
- bridge 自动调用 `createScheduledTask`

断言：

- 调用了 `create_scheduled_task`
- 没有调用任何 shell/system scheduler 模拟接口
- 创建后 `loadScheduledTasks()` 能看到任务

### 三、Rust 单元测试

文件：

```text
pinvou3-app/src-tauri/src/scheduled_tasks.rs
pinvou3-app/src-tauri/src/scheduled_executor.rs
pinvou3-app/src-tauri/src/engine_pool.rs
```

新增测试：

#### 1. 创建任务保存模型绑定

输入：

```rust
CreateScheduledTaskInput {
    model: Some("deepseek-v4-flash".into()),
    model_id: Some("deepseek-a".into()),
    ...
}
```

断言：

- automation record 的 `model` 是 `deepseek-v4-flash`
- model binding store 里保存 `deepseek-a`
- DTO 返回 `model_id = Some("deepseek-a")`

#### 2. 更新模型同步 binding

输入：

```rust
UpdateScheduledTaskInput {
    model: Some("qwen-max".into()),
    model_id: Some("qwen-prod".into()),
}
```

断言：

- automation record 更新 model
- binding 更新 model_id
- DTO 返回新绑定

#### 3. 删除任务清理 binding

删除任务后：

- automation 文件删除
- model binding 也删除

#### 4. 运行时使用稳定模型 ID

构造两个同名模型：

```rust
SavedModel { id: "one", model: "deepseek-v4-flash" }
SavedModel { id: "two", model: "deepseek-v4-flash" }
```

创建任务绑定 `model_id = "two"`。

断言：

- `ScheduledRunProfile.model_id == Some("two")`
- `resolve_scheduled_model` 不报 ambiguous

#### 5. 旧任务兼容

无 model_id 的旧任务：

- 如果 wire name 唯一，可以运行。
- 如果 wire name 不唯一，返回明确错误。

错误文案要包含：

```text
模型配置不唯一，请重新选择 AI 模型并保存任务
```

### 四、Smoke 测试

文件：

```text
pinvou3-app/tests/scheduled_tasks_smoke.js
```

新增或更新场景：

#### 场景 A：重复模型名不导致立即运行报错

Mock `list_models` 返回：

```js
[
  { id: "deepseek-a", name: "DeepSeek A", model: "deepseek-v4-flash" },
  { id: "deepseek-b", name: "DeepSeek B", model: "deepseek-v4-flash" }
]
```

步骤：

1. 打开定时任务页。
2. 新建任务。
3. 选择 `DeepSeek B`。
4. 保存。
5. 打开编辑任务。
6. 点击立即运行。

断言：

- invoke `create_scheduled_task` input 包含 `model_id: "deepseek-b"`。
- invoke `run_scheduled_task_now` 成功。
- 页面不显示 ambiguous 错误。
- 运行历史出现 queued/running 记录。

#### 场景 B：聊天创建任务进入定时任务列表

步骤：

1. 点击定时任务页 `AI 聊天创建`。
2. 输入“每小时推送 AI 新闻”。
3. mock assistant 返回合法 draft。
4. bridge 调用 `create_scheduled_task`。
5. 返回定时任务页。

断言：

- 列表中出现“AI 新闻推送”。
- `.scheduledTasks` state 包含该任务。
- 没有出现 `schtasks` 调用。
- 没有 Windows 任务计划程序相关文案作为成功结果。

#### 场景 C：分钟级需求必须询问

输入：

```text
每5分钟推送 AI 新闻
```

断言：

- 不调用 `create_scheduled_task`。
- 聊天回复要求用户改成支持模式，例如每 1 小时。
- 不调用 shell。

### 五、任务操作功能自动化测试

#### 1. 前端静态回归测试

文件：

```text
pinvou3-app/tests/scheduled_tasks_unit.js
```

新增断言：

1. 定时任务 DTO/前端使用 `pinned` 和 `pinnedAt`。
2. `ScheduledTasksView.jsx` 包含：
   - `data-testid="scheduled-task-pin"`
   - `data-testid="scheduled-task-actions"`
   - `data-testid="scheduled-task-action-menu"`
   - `data-testid="scheduled-detail-actions"`
3. 行级操作按钮都调用 `event.stopPropagation()`，避免误打开编辑弹窗。
4. 删除确认组件被行级菜单和详情菜单复用，而不是两套删除弹窗。
5. `tauri-bridge.js` 暴露：
   - `toggleScheduledTaskPinned`
   - `deleteScheduledTask`
   - `set_scheduled_task_pinned`
6. Rust 源码包含：
   - `task-ui-metadata.json`
   - `ScheduledTaskUiMetadataStore`
   - `pub pinned: bool`
   - `pub pinned_at: Option<String>`

示例断言：

```js
assert.ok(/data-testid="scheduled-task-pin"/.test(indexHtml));
assert.ok(/data-testid="scheduled-task-actions"/.test(indexHtml));
assert.ok(/function toggleScheduledTaskPinned/.test(tauriBridge));
assert.ok(/set_scheduled_task_pinned/.test(tauriBridge));
assert.ok(/task-ui-metadata\.json/.test(scheduledTasksRust));
```

#### 2. bridge 行为测试

文件：

```text
pinvou3-app/tests/scheduled_tasks_unit.js
```

新增 harness 测试：

##### A. 置顶任务

模拟：

```js
await bridge.toggleScheduledTaskPinned("task-a", true);
```

断言：

- 调用 `set_scheduled_task_pinned`。
- invoke 参数为 `{ id: "task-a", pinned: true }`。
- `state.scheduledTasks` 中对应任务变为 `pinned: true`。
- 如果当前详情是该任务，`state.scheduledTaskDetail.pinned === true`。
- `scheduledTaskBusyAction` 最终恢复为 `null`。

##### B. 取消置顶任务

模拟：

```js
await bridge.toggleScheduledTaskPinned("task-a", false);
```

断言：

- 调用 `set_scheduled_task_pinned`。
- 对应任务 `pinned: false`。
- `pinnedAt` 清空或为 `null`。

##### C. 置顶失败回滚

模拟后端 throw：

```js
invoke("set_scheduled_task_pinned") -> throw new Error("disk failed")
```

断言：

- UI 不永久显示 busy。
- `scheduledTaskError` 有可读错误。
- 已有任务数据不被错误覆盖。

##### D. 删除选中任务

模拟当前选中任务：

```js
state.selectedScheduledTaskId = "task-a";
state.scheduledTaskDetail = { id: "task-a", name: "A" };
state.scheduledTaskRuns = [{ automationId: "task-a" }];
await bridge.deleteScheduledTask("task-a");
```

断言：

- 调用 `delete_scheduled_task`。
- `selectedScheduledTaskId === null`。
- `scheduledTaskDetail === null`。
- `scheduledTaskRuns` 清空。
- `scheduledTasks` 中不再包含该任务。
- 侧边栏未读聚合重新计算。

##### E. 删除非选中任务

断言：

- 只从列表移除该任务。
- 当前选中详情不变。
- 不清空无关运行记录。

#### 3. React smoke 测试

文件：

```text
pinvou3-app/tests/scheduled_tasks_smoke.js
```

新增场景：

##### 场景 D：行级置顶与排序

Mock 初始任务：

```js
[
  { id: "task-a", name: "普通任务 A", pinned: false, nextRunAt: "..." },
  { id: "task-b", name: "普通任务 B", pinned: false, nextRunAt: "..." }
]
```

步骤：

1. 打开定时任务页。
2. hover 或 focus `task-b` 行。
3. 点击置顶按钮。
4. mock 返回 `task-b.pinned=true, pinnedAt=now`。

断言：

- 调用 `set_scheduled_task_pinned`。
- 出现“置顶任务”分组。
- `task-b` 出现在 `task-a` 前面。
- 点击置顶按钮不会打开编辑弹窗。

##### 场景 E：行级更多菜单

步骤：

1. 点击任务行右侧 `...`。
2. 菜单出现。
3. 点击“立即运行”。

断言：

- 菜单包含：
  - 编辑
  - 立即运行
  - 暂停/恢复
  - 置顶/取消置顶
  - 打开任务工作间
  - 删除
- 点击菜单不触发行点击。
- 点击立即运行调用 `run_scheduled_task_now`。
- `Escape` 能关闭菜单。
- 点击外部能关闭菜单。

##### 场景 F：行级删除

步骤：

1. 打开任务行 `...`。
2. 点击“删除”。
3. 出现 iOS 删除确认。
4. 点击取消。
5. 再次打开并确认删除。

断言：

- 取消后任务仍在列表。
- 确认后调用 `delete_scheduled_task`。
- 删除确认关闭。
- 任务从列表消失。
- 如果删除的是当前选中任务，详情弹窗关闭。

##### 场景 G：详情菜单操作一致

步骤：

1. 点击任务行打开编辑弹窗。
2. 点击详情弹窗右上角 `...`。
3. 点击置顶。
4. 再点击删除。

断言：

- 置顶调用与行级相同的 bridge 方法。
- 删除复用同一个确认弹窗 `data-testid="scheduled-detail-delete-confirmation"`。
- 详情菜单中不出现重复“编辑”项。
- 顶部关闭按钮仍可关闭弹窗。

##### 场景 H：移动端菜单可用

使用 viewport：

```js
await page.setViewportSize({ width: 390, height: 844 });
```

断言：

- 任务行右侧 `...` 可见。
- 菜单不被屏幕边缘裁剪。
- 菜单项文字不溢出。
- 删除确认完整覆盖 app 蒙层。

#### 4. Rust 单元测试

文件：

```text
pinvou3-app/src-tauri/src/scheduled_tasks.rs
```

新增测试：

##### A. UI metadata 保存与读取

步骤：

1. 创建临时 automation id。
2. 调用 `set_scheduled_task_pinned(id, true)`。
3. 重新加载 `ScheduledTaskUiMetadataStore`。

断言：

- `pinned == true`。
- `pinned_at.is_some()`。
- DTO 返回 `pinned=true`。

##### B. 取消置顶清理 metadata

步骤：

1. 先置顶。
2. 再取消置顶。
3. 重新读取 sidecar。

断言：

- DTO 返回 `pinned=false`。
- sidecar 中没有该任务，或该任务 `pinned=false` 且不会影响排序。

##### C. 删除任务清理 metadata

步骤：

1. 创建任务。
2. 置顶任务。
3. 删除任务。
4. 重新读取 sidecar。

断言：

- automation 删除。
- model binding 删除。
- UI metadata 删除。
- list DTO 不再返回该任务。

##### D. 孤儿 metadata compact

构造 sidecar：

```json
{
  "tasks": {
    "missing-task": { "pinned": true, "pinned_at": "..." },
    "live-task": { "pinned": true, "pinned_at": "..." }
  }
}
```

只创建 `live-task` automation。

断言：

- list/read 后 `missing-task` metadata 被清理。
- `live-task` 仍置顶。

##### E. 任务不存在错误

调用：

```rust
set_scheduled_task_pinned("missing", true)
```

断言：

- 返回错误。
- 错误文案可读，例如“定时任务不存在或已被删除”。
- 不写入 sidecar。

#### 5. 视觉与可访问性检查

Smoke 或 Playwright 检查：

- 行级按钮有 `aria-label`：
  - `置顶任务：<name>`
  - `取消置顶任务：<name>`
  - `打开任务操作菜单：<name>`
- 菜单使用 `role="menu"`。
- 菜单项使用 `role="menuitem"`。
- 删除确认使用 `role="alertdialog"` 和 `aria-modal="true"`。
- 键盘：
  - `Tab` 可聚焦行级菜单按钮。
  - `Enter/Space` 可打开菜单。
  - `Escape` 关闭菜单或确认弹窗。

### 六、手工验证清单

修复后按顺序验证：

1. 启动 app。
2. 打开设置，准备两个同名 wire model 的模型配置。
3. 打开定时任务。
4. 新建任务，选择第二个同名模型。
5. 保存后不自动弹编辑。
6. 打开该任务编辑弹窗。
7. 点击立即运行。
8. 不报模型歧义错误。
9. 运行历史新增一条记录。
10. 点击 AI 聊天创建。
11. 让 AI 创建“每小时推送 AI 新闻”。
12. 任务出现在定时任务列表。
13. Windows 任务计划程序中不应新增由该流程创建的 `schtasks` 任务。
14. 在定时任务列表中置顶一个任务。
15. 重启 app，确认置顶仍保留。
16. 取消置顶，确认任务回到普通分组。
17. 通过任务行 `...` 删除任务，确认不需要先打开编辑弹窗。
18. 通过详情弹窗 `...` 删除任务，确认与行级删除弹窗一致。
19. 在 390px 宽度下检查菜单和删除确认不被裁剪、不溢出。

### 七、验证命令

最小本地验证：

```powershell
cd C:\Users\123\pinvou3\pinvou3-app
npm run test:scheduled
npm run lint:ui
npm run build:ui
```

Smoke：

```powershell
cd C:\Users\123\pinvou3\pinvou3-app
node tests/scheduled_tasks_smoke.js
```

Rust 编译验证：

```powershell
cd C:\Users\123\pinvou3\pinvou3-app\src-tauri
cargo test -p pinvou3-tauri scheduled_tasks --no-run
cargo test -p pinvou3-tauri scheduled_executor --no-run
cargo test -p pinvou3-tauri engine_pool --no-run
```

说明：

- 当前本机 Rust 测试二进制曾出现 `STATUS_ENTRYPOINT_NOT_FOUND`，因此至少要求 `--no-run` 编译通过。
- CI 或修复本机动态库环境后，再要求实际 Rust 单测运行通过。

## 实施顺序

建议分 4 个小 PR 或 4 个提交：

1. 模型下拉与 bridge payload 改造。
2. 后端 model binding sidecar 与 DTO 扩展。
3. ScheduledChatExecutor 运行时稳定 model_id 注入。
4. 聊天创建 prompt/bridge/smoke 测试强化。

每步都必须保持：

- `npm run test:scheduled` 通过。
- `npm run lint:ui` 通过。
- `npm run build:ui` 通过。

## 风险与回滚

### 风险 1：旧任务没有 model_id

处理：

- 保留旧逻辑 fallback。
- 只有同名模型歧义时才要求用户重新选择模型。

### 风险 2：sidecar 与 automation 文件不一致

处理：

- list/read 时如果 automation 不存在，清理 binding。
- delete 时删除 binding。
- update 时覆盖 binding。

### 风险 3：普通聊天仍可创建系统任务

第一期只保证定时任务模块的 AI 创建入口。普通聊天全局禁止 `schtasks` 属于更大范围的工具策略问题，建议二期做意图识别和工具权限控制。

## 完成标准

本修复完成后应满足：

- 模型下拉不再出现重复 key 警告。
- 同名 wire model 配置下，“立即运行”不再因模型歧义失败。
- 从定时任务页“AI 聊天创建”创建的任务一定进入 pinvou3 定时任务列表。
- 该创建流程不会创建 Windows `schtasks` 任务。
- 不支持的分钟级定时需求会询问用户改成支持规则。
- 所有新增行为均有自动化测试覆盖。

## 交接说明

### 一、当前交接结论

定时任务模块按“任务定义”和“运行对话”两类对象交接。两类对象的能力归属必须分清；“当前 UI 没有入口”不等于底层做不了：

- **定时任务定义**：定时任务页面里的任务配置。当前 UI 提供编辑名称/执行内容/模型/重复规则/时间、启停、立即运行、打开任务工作间和删除任务定义。后端另有任务置顶能力，但当前详情页没有入口。
- **定时运行对话**：左侧栏“定时任务记录”里的每次运行会话。归属它的操作有：打开并继续追问、重命名、置顶/取消置顶、收纳（归档）、删除本次运行、打开该任务的共享工作间。

> 历史文档曾写过“定时任务列表里的置顶/删除/打开文件夹不可能做”。**该结论是错的，已删除**。这些操作在两侧都成立，只是各自作用的对象不同：任务列表上的置顶/删除作用于**任务定义**（`set_scheduled_task_pinned` / `delete_scheduled_task`，sidecar 见 `task-ui-metadata.json`），侧边栏记录上的置顶/删除作用于**该次运行对话**（`set_session_pinned` / `delete_session`）。两者互不覆盖，不是二选一。

### 二、功能归属

#### 1. 定时任务定义（定时任务页面）

作用于任务定义本身，走 automation 命令：

- 编辑字段 / 启停：`update_scheduled_task`、`pause_scheduled_task`、`resume_scheduled_task`
- 立即运行：`run_scheduled_task_now`
- 置顶任务：`set_scheduled_task_pinned`（sidecar `task-ui-metadata.json`）
- 打开任务工作间：`open_scheduled_task_folder`（工作间由 automation_id 稳定派生，同一任务的所有运行共享）
- 删除任务定义：`delete_scheduled_task`（连带删除该任务的**所有**运行对话、read-state、model binding、UI metadata）

行级/详情菜单是否呈现这些入口，是**交互布局**问题，不是能力问题。当前详情弹窗提供编辑、启停、立即运行、打开工作间和删除；`set_scheduled_task_pinned` 已实现但 UI 暂未暴露。要不要增加入口可以再定，但不要写成“不可能做”。

#### 2. 定时运行对话（左侧栏“定时任务记录”）

作用于某一次运行的会话，复用 `RecentItem` 与**普通 Session 命令**。后端 `SessionKind` 分发（`commands.rs`），前端不做命令分叉：

| 操作 | 命令 | ScheduledRun 的行为 |
|---|---|---|
| 打开 / 继续追问 | `openScheduledRunChat` → `chat` | 与普通对话同路，engine 复用会话 profile |
| 重命名 | `rename_session` | 复用 Session 元数据 `metadata.title` |
| 置顶 | `set_session_pinned` | 复用置顶表 `_pinned_sessions.json` |
| 收纳（归档） | `set_session_archived` | 复用收起表 `_hidden_sessions.json`；归档后离开侧边栏，进设置页“任务收纳”，可还原 |
| 删除 | `delete_session` | 按 SessionKind 分发到 `ScheduledTaskState::delete_run_for_session`：**联动删除该次 Session + Run + 底座 Task**；**保留任务定义、共享工作间和其他运行记录** |
| 打开工作间 | `reveal_session_folder` | 定时会话没有独立 runtime 目录，分发到所属任务的共享工作间 |

关键约束：

- 侧边栏记录的删除**只删这一次运行**，绝不调用 `delete_scheduled_task`。
- 运行中/排队中的运行记录不允许删除（后端返回“正在运行的定时任务记录不能删除”）。
- 定时运行的 transcript 由 Engine 独占持久化，`save_session_messages` / `save_session_artifacts` 仍拒绝 `sched-*`（`ensure_chat_session` 现在只守卫这两个覆盖类命令）。

#### 3. 侧边栏“定时任务记录”的范围

侧边栏列出**所有任务的所有现存运行对话**：`loadScheduledTaskRecentRuns()` 一次调用 `list_scheduled_runs`，后端只执行一次 reconcile 和一次 Session 元数据扫描；前端过滤无 sessionId 和已归档的记录并按时间倒序。

- 不再有“最多 8 条 / 最多 12 个任务 / 每任务最多 3 条”的截断。
- 不新增二级索引或汇总存储：条数上限由后端既有 retention（`MAX_TERMINAL_RUNS_PER_AUTOMATION = 50`，按 automation 生效）兜底。
- `ScheduledRunDto` 直接携带 `sessionTitle` / `pinned` / `pinnedAt` / `archived`，因为 `list_sessions` 刻意把 `sched-*` 隔离在普通历史之外，前端拿不到这些会话的元数据。

#### 4. 立即运行 → 侧边栏

`run_scheduled_task_now` 返回时 run 还没有 `sessionId`（会话由 executor 稍后创建并 ThreadLinked）。前端 `refreshScheduledRunShortcutUntilLinked(automationId, runId)`：

- **只轮询当前任务**（`list_scheduled_task_runs`）：前 15 次每秒一次，此后每 5 秒一次，最长 30 分钟；run 进入终态且仍无会话时也会停止。
- 匹配到该 runId 拿到 `sessionId` 后并入侧边栏并**立即停止轮询**。
- 独立于定时任务页面生命周期：run 完就切走页面也不会漏掉这条记录。
- 后台自动调度不依赖这条“立即运行”轮询：Session 文件监听会发送轻量 `scheduled_task:run_updated` 事件，前端防抖后刷新任务摘要与聚合运行列表。

#### 5. 聊天创建任务

`createScheduledTask` 只有在 `create_scheduled_task` 返回**真实 id** 时才算成功；返回空/无 id 直接抛错，不写入列表。创建成功后立即 `await loadScheduledTasks()` 重拉列表——新的 request stamp 会使创建前仍在途的旧 `list_scheduled_tasks` 响应失效，避免旧结果把新任务覆盖掉。模型只在文字里声称创建成功、没有产出可解析的 `scheduled-task-draft`，不会触发创建，也不显示成功。

#### 6. 立即运行后的执行约束

任务 prompt 原文本来就作为用户消息传入会话（`ScheduledChatExecutor::execute` → `task.prompt()` → `EnginePool::run_scheduled_turn` → `AppEngine::send_scheduled_message`），链路无缺失。为防模型把目标改写成别的事，`engine.rs` 的 `SCHEDULED_TURN_REMINDER` 只加**一条**短约束（走既有 per-turn `<system-reminder>` 通道，复用 persona reminder 参数位）：

```
本轮是定时任务的自动执行：直接执行用户消息里的任务，不要改写、替换或扩展任务目标。
```

不要在这里继续堆叠长提示词。

### 三、已完成的 UI 修复

1. 定时任务记录菜单显示不全：
   - 已改为 `fixed + portal` 渲染到 `document.body`。
   - 不再作为行内 `absolute` 子元素挂在侧边栏滚动容器里。
   - 目的：避免菜单被侧边栏右边界或滚动区域裁剪。

2. 新建/编辑任务弹窗线框问题：
   - 已去掉嵌入式设置组外层线框。
   - 保留内部细分割线。
   - 浮层菜单、时间滚轮、删除确认弹窗仍保留边框。

3. 已撤回的 UI 改动：
   - 缩减侧边栏主导航项高度和间距的改动已经撤回。
   - 不要通过压缩主导航项来释放记录区空间。

4. 仍需后续优化的侧边栏空间问题：
   - 当前侧边栏其他模块占用空间过多，导致“置顶任务 / 任务 / 定时任务记录”的展示区域偏小。
   - 后续优化重点应放在分组展开策略、记录区剩余高度分配、非当前分组折叠策略或完整列表入口上。
   - 不能再次通过压缩主导航项高度、字号、图标间距来解决，否则会破坏侧边栏与其他模块的一致性。

### 四、关键文件

前端：

```text
pinvou3-app/src/main.jsx
pinvou3-app/src/components/layout/NavigationComponents.jsx
pinvou3-app/src/features/scheduled/ScheduledTasksView.jsx
pinvou3-app/src/tauri-bridge.js
```

测试：

```text
pinvou3-app/tests/scheduled_tasks_unit.js
pinvou3-app/tests/scheduled_tasks_smoke.js
```

文档：

```text
docs/scheduled-tasks-run-now-chat-create-bugfix-plan.md
```

### 五、验证命令

交接前必须通过（路径按各自 checkout 调整）：

```powershell
cd <repo>\pinvou3-app
npm run test:scheduled
npm run lint:ui
npm run build:ui
node tests\scheduled_tasks_smoke.js
cd src-tauri
cargo check --locked
```

说明：`cargo check --locked` 不编译 `#[cfg(test)]`，要覆盖新增的后端命令测试需 `cargo check --locked --tests`。本机实际运行 Rust 测试二进制可能撞 `STATUS_ENTRYPOINT_NOT_FOUND`，见 `docs/fork-modifications.md` §4。

### 六、交接风险

1. 不要把作用于**运行对话**的操作（重命名/置顶/收纳/删除本次运行）搬到定时任务列表上——它们的对象是会话，不是任务定义。反过来也一样：任务定义的置顶/删除不要塞进侧边栏记录菜单。
2. 不要把运行记录删除误实现为删除定时任务定义。删除一次运行只清 Session + Run + Task，任务定义、共享工作间、其他运行必须保留。
3. 不要再给 `sched-*` 会话新增平行 Session 命令。会话元数据（标题/置顶/收起）就用普通 Session 命令，差异在后端 `SessionKind` 分发处收敛。
4. 不要给侧边栏记录列表加二级索引或汇总存储。条数上限由后端既有 retention 兜底；前端只做「逐任务 `list_scheduled_task_runs` → 过滤 → 倒序」。
5. 不要在 `SCHEDULED_TURN_REMINDER` 上继续堆提示词。任务 prompt 原文已完整传入，那里只允许一条防目标改写的短约束。
6. 不要新增第二份定时任务方案文档；后续修改继续写入本文档。
7. 如果后续要释放侧边栏空间，优先优化分组展开策略或记录区高度分配，不要压缩主导航项。侧边栏空间偏小的问题尚未解决（现在记录不再截断，这个问题会更明显），需作为下一轮 UI 优化项跟进。
