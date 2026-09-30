# Fork 会话功能实施计划

- 分支：`feat/fork-chat`（基线 `c0618f7d0`）
- 状态：设计定稿，待实施
- 依赖：PR #484（单入口工作区 / 会话钥匙串，`feat/workspace-entry`，**尚未合并**）——后端实施堆叠在其之后
- 边界：**不修改 CodeWhale 子模块**，纯 `pinvou3-app` 层组合，不触发 fork-guard 登记流程

---

## 1. 背景与目标

用户在长对话中常产生"回到第 N 轮、换个方向再试"的需求。现状只有两个糟糕选项：开新会话手动重建上下文，或用代码模式 rewind（破坏性截断，原路丢失）。fork 提供第三条路：**把会话前缀复制为新会话，原会话原封不动**。

### 目标

1. 会话级 fork（v1）：从侧边栏一键复制会话为新会话并跳转。
2. 消息级 fork（v2）：从指定用户 turn 末尾分叉，只保留前缀。
3. 工作区隔离（按根选择）：支持"共享全部根"或"逐根隔离（git 工作树 / 目录复制）"。
4. 删除行为完全统一：删会话永远只删会话数据，永不触碰磁盘工作区。

### 非目标（v1 明确不做）

- 会话谱系展示 / 同源会话分组（依赖被裁剪的 lineage 元数据，留作后续）
- 会话内分支切换（CodeWhale `/branch` 形态；pinvou3-app 渲染管线为线性，改造量大）
- web / 远程端（`web_access_*` 系列变体）
- CodeWhale 侧任何改动

---

## 2. 功能定义

### 2.1 两档粒度

| 粒度 | 行为 | 入口 | 版本 |
|---|---|---|---|
| 会话级 | 复制全部对话为新会话 | 侧边栏会话行菜单"fork 会话" | v1 |
| 消息级 | 复制前 N 个用户 turn 的前缀 | 消息/turn 悬浮动作"从这里分叉" | v2 |

后端 API 从 v1 起即接受 `keep_turns: Option<u32>`（None = 全量），v2 只补 UI，不动后端。

### 2.2 分叉点口径

统一复用 `deepseek_tui::is_user_turn_prompt` 谓词定位"第 N 个用户 turn"，与 rewind（`rewind.rs:153`）、checkpoints 的 `count_user_turns` 同口径，保证快照 / 截断 / UI 三处一致。切分点总落在用户 turn prompt 边界，天然不产生悬空的 `tool_use`/`tool_result` 断对（前提：源会话自身一致且非生成中，见 §8.2）。

### 2.3 工作区处理（依赖 PR484 钥匙串模型）

会话钥匙串 = 主根（引擎 cwd，slot 0）+ 附加根（`workspace_roots`，见 PR484 `SessionWorkspaceSidecar`）。fork 时逐根选择：

```
Fork 会话
├─ ○ 仅复制会话（共享全部工作区）
└─ ● 隔离工作区
       ├─ ☑ ~/projects/bugfix        （主根）→ git 工作树
       ├─ ☐ ~/docs/api-spec          （附加根）→ 共享
       └─ ☑ ~/projects/shared-lib    （附加根）→ 目录复制
```

- 默认值：主根 = 隔离，附加根 = 共享（主根是 agent 写文件的地方，附加根多为参考目录）。
- 隔离方式自动选择：git 仓库 → 工作树；非 git → 递归复制。不把该选择暴露给用户。
- 新会话钥匙串：隔离根写新路径，共享根保留原路径（路径翻译参考 PR484 rebind 的 keychain translation）。
- 兼容：源会话无钥匙串（PR484 之前的旧 sidecar）= 单根语义，退化为"主根一个开关"。

---

## 3. 交互设计

### 3.1 入口与流程（v1）

1. 侧边栏会话行右键/更多菜单 → "fork 会话"（`NavigationComponents.jsx` `RecentItem` 菜单，与重命名/导出并列；菜单高度常量 +36px/项）。
2. 弹出 fork 对话框：分叉范围（v1 整会话）+ 工作区档位与逐根开关（§2.3）。
3. 选择"隔离工作区"时展示**创建时告知**（一次性、逐根合并展示）：

   > 将创建工作区副本：`~/projects/bugfix-fork-a3f2/`（git 工作树，含未提交改动）等 2 项。
   > **该目录不会随会话删除而自动清理**，请自行管理。
   > 〔取消〕〔创建〕

4. 执行成功 → `session:list_changed` → **立即切换到新会话**（已定）。
5. 失败 → toast 报错，原会话与磁盘状态不受影响。

### 3.2 新会话标识

- 标题：`<原标题>（分叉2）`，后缀数字取当前未被占用的最小值（避免"（分叉）（分叉）"套娃）。
- 不写血缘元数据（`parent_session_id` 等已裁剪，无任何消费方；将来做谱系展示时再以最小 sidecar 形式加回）。

### 3.3 删除行为（统一，已定）

删除任何会话 = 只删会话数据（对话 / 附件 / sidecar / 绑定记录）。隔离工作区在创建时经确认框完成归属告知，删除流程**零改动**、无 fork 特殊分支。保留上限（每类 50）静默清理产生的孤儿副本同样被创建时告知覆盖。

### 3.4 归属标记（可选但建议保留）

隔离根目录内写 `.pinvou-fork-workspace.json`（内容：创建时间、来源路径）；git 工作树同时把该文件名加入本工作树 `.git/info/exclude`（本地排除，不污染 `git status`）。用途：将来"存储管理"页识别 fork 副本。成本一次文件写入，不阻塞任何功能。

---

## 4. 技术设计

### 4.1 复制内容清单

**复制**：
- `messages` 线性前缀（`load_session_snapshot` 返回值即活跃分支投影；journal 死分支/多根**不**随行——线性前缀方案天然免疫，已核实 `ensure_journal` 投影语义）
- `system_prompt`（压缩摘要随行继承）
- `context_references`
- artifacts 元数据（`SavedSession.artifacts`）+ 会话私有 ledger 目录中的 artifact 内容（已定：随 fork 复制）
- 模型 / 模式（`_session_models.json`、mode state sidecar）
- 工作区绑定（钥匙串，按 §2.3 翻译）

**不复制**：置顶状态、steered messages、turn timeline、血缘元数据、cost 快照（新会话独立计费从零开始）。

### 4.2 落点（分层）

| 层 | 位置 | 内容 |
|---|---|---|
| 存储 | `src-tauri/src/features/sessions/fork.rs`（新） | `SessionStore::fork_session(&self, id, keep_turns: Option<u32>, workspace_plan: ForkWorkspacePlan) -> Result<ForkOutcome>`：load 快照 → turn 前缀切分 → `create_saved_session_with_id_and_mode` 新 id 落盘（仿 `create_new` 失败回滚）→ 填充 §4.1 清单 → `persist_then_reconcile` 显式路径（绕开 `update_messages` 的 `looks_like_truncating_overwrite` 守卫，守卫本身不动）→ 写新绑定 sidecar |
| OS 原语 | `src-tauri/src/platform/`（新 adapter，接口 + 各 OS 实现） | `is_git_repo`、`create_worktree`（含未提交改动同步：`git diff HEAD` 补丁 + 未跟踪文件复制）、`copy_dir_recursive`（进度回调）、git 可用性探测与降级（无 git → 复制） |
| 命令 | `src-tauri/src/app/commands/sessions.rs` + `lib.rs` `generate_handler!` 注册 | `fork_session(source_id, keep_turns, workspace_plan)`；异步执行（复制可能耗时），进度经 Tauri 事件 `fork:progress` 上报；完成 emit `session:list_changed` |
| 桥接 | `src/platform/tauri/bridge/sessions.js` + `src/shared/bridge-shared-helpers.js` | `forkSession(id, keepTurns, plan)`：invoke → 监听进度 → 成功后 `refreshHistoryList` + `switchToSession(newId)`；防重复提交（进行中置灰） |
| UI | `NavigationComponents.jsx`（菜单项）、`main.jsx`（`handleForkSession` stable callback，仿 2279-2342 现有模式）、fork 对话框组件（新，建议复用 PR484 工作区选择器的根列表样式） | 入口 + 对话框 + 进度 + 结果 |
| i18n | `src/shared/i18n/{zh,en,ja}.js` | `forkSession`、`forkDialog*`、`forkWorkspace*`、`forkProgress*`、`forkSuccess/Failed`、`forkTitleSuffix` 等三语同步 |
| 测试 | `features/sessions/tests.rs`（Rust）+ `tests/session_fork_*.test.mjs`（node:test） | 见 §6 |

### 4.3 引擎衔接

新会话落盘后无需专用引擎通道：`switchToSession` → 后续 chat 时 `EnginePool::get_or_spawn` lazy 注水持久化 messages，与普通会话切换一致。隔离根的路径提示以普通用户消息形态注入新会话开头（不进 system prompt）：

> （系统注入）本会话由 fork 创建，隔离工作区映射：`~/projects/bugfix` → `~/projects/bugfix-fork-a3f2`。历史记录中的路径属于原工作区。

**不做**历史消息内绝对路径的字符串替换（误伤风险高，靠提示 + agent 重新 `ls` 对齐）。

### 4.4 防护与排除

- 生成中（该会话 engine 有活跃 turn）拒绝 fork：命令层校验，前端同时置灰入口。
- scheduled-run 会话拒绝 fork（与 rewind 同规则）。
- 新路径过 `validate_user_workspace_path` 校验。
- 副本位置约定：源根同目录 `<name>-fork-<新会话id前4位>`；工作树分支名 `pinvou-fork/<新会话id前8位>`。
- git 属外部命令：按社区版规范在确认框文案中告知。

### 4.5 与 PR484 的堆叠关系

两者同基（`c0618f7d0`），但 #484 改的文件（`workspace_bindings.rs`、sessions 命令层、创建流程）与 fork 后端高度重叠。**后端实现必须基于 #484 合并后的 main（或直接堆叠在 `feat/workspace-entry` 上开发）**，且钥匙串模型从第一天纳入设计（本计划 §2.3/§4.1 已按多根编写，不返工）。可先行：桥接层骨架、菜单入口、i18n、本计划评审。

---

## 5. 决策记录（已定，含理由）

| # | 决策 | 理由 |
|---|---|---|
| D1 | 不写血缘元数据，用标题后缀（分叉N） | 无任何消费方；标题可见性更好；将来加回仅一行成本 |
| D2 | 线性前缀复制，非 journal 整树复制 | pinvou3-app 全链路线性；避免死分支随行膨胀文件；多根结构天然免疫 |
| D3 | 分叉口径 = `is_user_turn_prompt` | 与 rewind / checkpoints 三处一致 |
| D4 | 隔离按根逐个选择，默认主根隔离、附加根共享 | 主根是写入面；附加根多为参考 |
| D5 | git 根用工作树（含未提交改动 + 未跟踪文件同步），非 git 根整目录复制；自动选择不暴露给用户 | worktree 省 disk 且 git 原生易比较；裸 worktree 会丢未提交状态，必须补同步 |
| D6 | 创建时一次性告知"副本不随会话删除"；删除流程零改动 | 删除行为全局统一，无 fork 特殊分支；归属在创建时移交 |
| D7 | 副本目录内留 `.pinvou-fork-workspace.json` 标记（可选建议） | 将来存储管理可识别；成本一次写入 |
| D8 | fork 成功后立即跳转新会话 | 符合主流程预期 |
| D9 | artifacts 随 fork 复制（元数据 + ledger 内容） | 历史 artifact 卡片引用需保持可打开 |
| D10 | 隔离时注入路径映射提示消息，不改写历史路径 | 字符串替换误伤风险高 |
| D11 | 不改 CodeWhale | fork 原语已公开（`create_saved_session_with_id_and_mode` 等），app 层组合即可；不触发 fork-guard |
| D12 | 生成中 / scheduled 会话拒绝 fork | 残缺快照与运行记录无 fork 语义 |

---

## 6. 测试标准

### 6.1 Rust 单元测试（`features/sessions/tests.rs`，`isolated_store()` 模式）

**核心语义**（全部必须存在，命名即验收项）：

1. `fork_session_copies_full_history` — 全量 fork：消息逐条相等、`message_count` 一致、原会话文件字节不变。
2. `fork_session_keep_turns_cuts_at_user_turn_prompt` — `keep_turns=2` 时恰保留前 2 轮（含第 2 轮的 assistant/tool_result），第 3 轮用户 prompt 为界。
3. `fork_session_keep_turns_beyond_total_fails` — 超界报错，不落任何文件。
4. `fork_session_preserves_tool_use_result_pairing` — 前缀内 `tool_use`/`tool_result` 逐对完整（含 `tool_use_id` 一致）。
5. `fork_session_inherits_system_prompt_and_compaction_summary` — 含压缩摘要的 system_prompt 原样继承；压缩会话（含死分支 journal）fork 后新会话 `journal` 单根单链、活跃投影正确。
6. `fork_session_copies_artifacts_and_they_open` — artifact 元数据 + ledger 内容复制后，新会话 artifact 可解析、内容可读。
7. `fork_session_rejects_scheduled_session`。
8. `fork_session_rejects_while_generating` — 活跃 turn 期间调用报错、零写入。
9. `fork_session_new_title_uses_next_free_suffix_number` — 重复 fork 不套娃、不重号。
10. `fork_session_source_untouched_after_isolation_failure` — 隔离根创建失败时整体回滚：不留半成品会话、不留半成品副本目录/工作树。

**工作区隔离**（platform adapter 测试 + 组合）：

11. `worktree_creation_carries_uncommitted_and_untracked` — 脏工作树 fork 后：未提交修改与未跟踪文件均存在于新工作树。
12. `worktree_fallback_to_copy_when_git_missing` — git 不可用时降级复制，结果等价（内容一致）。
13. `fork_translates_keychain_only_for_isolated_roots` — 隔离根指向新路径、共享根保留原路径；无钥匙串旧 sidecar 退化为单根。
14. `copy_dir_reports_progress_and_aborts_cleanly` — 进度回调单调递增；中途失败不残留部分目录。
15. `fork_marker_file_written_and_excluded_in_worktree` — 标记文件存在；工作树内 `.git/info/exclude` 含其文件名。

### 6.2 前端测试（node:test，`tests/session_fork_*.test.mjs`，仿 `session_buffer_eviction.test.mjs` 的工厂驱动风格）

1. 桥接：`forkSession` 正确 invoke 参数、成功后刷新列表 + 切换新会话、失败不切换不破坏当前 buffer。
2. 进行中防重复：fork 未完成时再次调用被拒。
3. 对话框逻辑（纯逻辑测试，仿 `session_management_logic.test.mjs`）：默认开关值、逐根覆盖、确认框文案含"不会随会话删除"提示、后缀编号计算。
4. 菜单项渲染与禁用态（生成中置灰）。

### 6.3 i18n 与静态检查

- `tests/ui_language_coverage.test.mjs`（三语 parity）、`tests/i18n_no_zh_literals.test.mjs`（组件无中文直量）必须通过。
- `python3 scripts/architecture-guard.py`：OS 原语落 `platform/`、依赖方向 `app → features → platform` 不违规。
- `./scripts/fork-guard.sh --fast`：预期无变化（CodeWhale 零改动），运行以确认登记数不变。

### 6.4 验收前置命令

```bash
cd pinvou3-app && npm test          # node:test 全量
cd pinvou3-app/src-tauri && cargo test
python3 scripts/architecture-guard.py
./scripts/fork-guard.sh --fast
```

---

## 7. 验收矩阵

自动化（A）/ 手动（M）双轨；"结果"列为验收断言。前置 DB = 会话钥匙串（PR484 合并后）；前置 LG = 生成中；前置 SC = scheduled 会话；前置 CP = 已压缩会话；前置 GR = git 仓库根；前置 NR = 非 git 根。

| # | 场景 | 前置 | 操作 | 预期结果 | 覆盖 |
|---|---|---|---|---|---|
| 1 | 基础会话级 fork（共享根） | 单根 | 菜单 fork，选"仅复制会话" | 新会话消息全量相等；原会话只读未动；立即跳转；标题 `（分叉2）` | A(1,10)/M |
| 2 | 消息级 fork 前缀 | ≥3 轮 | `keep_turns=2` | 恰保留前 2 轮，断点为第 3 轮用户 prompt | A(2) |
| 3 | 超界/非法 keep_turns | ≥1 轮 | 传 total 或更大 | 报错，零写入 | A(3) |
| 4 | tool 调用完整性 | 含工具轮 | 全量 fork | `tool_use`/`tool_result` 成对、id 一致，新会话续聊无孤儿工具消息 | A(4)/M |
| 5 | 压缩会话 fork | CP | 全量 fork | 摘要继承；新会话单根单链；续聊上下文连贯 | A(5)/M |
| 6 | artifacts 随 fork | 含 artifact | 全量 fork | 新会话历史卡片可打开、内容一致 | A(6)/M |
| 7 | scheduled 拒绝 | SC | 任意 fork 入口 | 明确报错，无写入 | A(7)/M |
| 8 | 生成中拒绝 | LG | 菜单入口 | 入口置灰；强行调命令报错 | A(8)/M |
| 9 | git 根隔离（干净树） | GR | 主根隔离 | 工作树建立、HEAD 一致、分支命名符合约定 | A(11)/M |
| 10 | git 根隔离（脏树） | GR+未提交+未跟踪 | 主根隔离 | 修改与未跟踪文件均出现在新工作树 | A(11)/M |
| 11 | 无 git 降级 | NR | 隔离 | 整目录复制，内容一致，有进度提示 | A(12)/M |
| 12 | 多根逐根选择 | DB=1主+2附 | 主隔离、附共享 | 钥匙串按 §2.3 翻译；路径提示消息列出映射 | A(13)/M |
| 13 | 旧版单根 sidecar | PR484 前数据 | 隔离 | 退化为主根单开关，行为正确 | A(13)/M |
| 14 | 隔离失败回滚 | 模拟复制失败 | 隔离 | 原会话完好；无半成品会话/目录/工作树 | A(10,14)/M |
| 15 | 删除 fork 会话 | 已隔离 | 删除 | 只删会话数据；副本目录与工作树原样保留 | M |
| 16 | 创建时告知 | 隔离 | 打开确认框 | 文案含副本路径与"不随会话删除"声明；取消则零动作 | A(§6.2-3)/M |
| 17 | 保留上限淘汰 | 50+ 会话 | 连续 fork | 最老会话被清理时功能不崩；孤儿副本可被标记文件识别 | M |
| 18 | fork 之 fork | 已是分叉会话 | 再 fork | 标题编号递增不套娃 | A(9)/M |
| 19 | 三语 UI | 切 zh/en/ja | 全流程 | 无缺 key、无中文直量、无回退假象 | A(§6.3)/M |
| 20 | 隔离根绝对路径 | 已隔离 | 续聊 | 新会话开头有映射提示；agent 引用新路径工作 | M |
| 21 | 并发/重复点击 | LG 或慢复制 | 连点 fork | 单次执行，进行中入口禁用 | A(§6.2-2)/M |
| 22 | web/远程端 | web 打开 | fork 入口 | v1 不展示入口（远程能力显式 unsupported，不静默失败） | M |

**发布验收门槛**：矩阵 #1–#14、#16、#19、#21 自动化全覆盖且通过；#15、#17、#18、#20、#22 手动清单逐项签字。任一"原会话被改动"即一票否决（#1、#8、#14 为最高优先级回归项）。

---

## 8. 风险与注意事项

### 8.1 分支与流程

- **PR484 堆叠是硬依赖**：后端开工前确认其合并状态；冲突热点 `workspace_bindings.rs` / sessions 命令层。同步 main 按 CONTRIBUTING 执行，勿因 main 推进反复 rebase。
- 自查清单（提 PR 前）：根因与关联场景、实现边界、对现有功能影响（尤其 rewind / checkpoints / 删除 / 保留清理）、异常态、验证充分性。
- 本文档为设计文档，实施 PR 的标题与描述用英文（CONTRIBUTING §PR）。

### 8.2 数据一致性

- fork 读的是 `load_session_snapshot` 的活跃线性投影，**绝不**直写会话 JSON（CodeWhale 单写者约束：`LIVE_SESSIONS` + `.session_boot_owners.json`，必须经 `manager.save_session`，store 现有方法已满足）。
- 生成中快照可能含悬空 `tool_use`：命令层校验 + 前端置灰双保险，不依赖单侧。
- 大目录复制是长任务：命令必须异步 + `fork:progress` 事件，前端不可假同步完成；中途取消要干净（无部分目录）。

### 8.3 工作树与复制边缘

- 嵌套 git 仓库 / submodule：v1 按"根独立处理、不递归特殊化"（submodule 在工作树中为空目录属 git 原生行为，文档明示即可）；异常时降级整目录复制。
- git 版本过旧不支持某 worktree 参数：探测降级。
- 磁盘空间不足：预检（副本根分区可用空间 vs 源体积估算），不足即前置报错。
- 符号链接、权限异常文件：复制失败按根隔离失败处理（整体回滚该根），错误信息含具体路径。
- 同一 git 仓库被多个根引用（主根与附加根同 repo 不同子目录）：各根独立建工作树，v1 不做去重合并。

### 8.4 安全与社区版约束

- fork 全程本地完成，无网络行为；git 为外部命令，确认框已告知（社区版规范）。
- 新副本路径必须过 `validate_user_workspace_path`；标记文件不包含敏感信息（仅时间戳与来源路径，来源路径本就属于用户本机数据）。
- 不在日志中输出完整消息内容。

### 8.5 已知取舍（明示记录，不视为缺陷）

- 每次前缀复制带来磁盘冗余（会话 JSON 通常几百 KB 量级，接受）。
- 历史中的旧绝对路径靠提示消息 + agent 自行对齐，非改写。
- 同源会话在列表中扁平展示，无分组（v1 接受，谱系留后续）。
- 保留上限触发时孤儿副本静默存在（创建时已告知归属；标记文件可供将来清理工具识别）。

---

## 9. 实施批次

| 批次 | 内容 | 前置 | 交付判据 |
|---|---|---|---|
| 0 | 本计划评审通过；桥接骨架 / 菜单入口 / i18n 三语 key（不接线） | 无 | §6.3 静态检查通过 |
| 1 | 后端：`fork_session` 存储语义 + platform adapter（worktree/复制）+ Rust 测试 §6.1 全量 | PR484 合并或堆叠 | §6.1 #1–#13 通过；cargo test 全绿 |
| 2 | 命令注册 + 桥接 + 对话框 + 进度 + 前端测试 §6.2 | 批次 1 | 验收矩阵 #1、#9–#11、#16、#19、#21 通过 |
| 3 | 消息级入口（turn 悬浮动作）+ artifacts 卡片回归 + 手动清单补全 | 批次 2 | 矩阵全量（含 #15、#17、#18、#20、#22 手动项）签字 |

批次 1 与 2 可并行开发、串行合并（2 依赖 1 的命令签名）。
