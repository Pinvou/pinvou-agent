# Pinvou 夜间反思模块 Spec

> 状态：评审收敛稿 v2
> 初稿日期：2026-07-16
> 最近更新：2026-07-21
> 目标：让 Pinvou 从历史 session 的成功/失败轨迹中离线提炼可复用经验，在类似任务中减少无效探索、错误工具调用和重复验证。
> PR 关系：#184、#189 均关闭，不予上线；后续实现必须基于本文从 P0 重新立项，不继承两者的完成态判断。

---

## 1. 背景与结论

Pinvou 当前已有用户记忆、品悟召唤式评审、session 持久化、context compaction、技能系统和 MCP 工具链，但还没有“任务执行反思”闭环。

现有用户记忆解决的是“用户是谁、偏好什么、最近在做什么”。夜间反思要解决的是另一类问题：

```text
这类任务以前怎么做慢了？
哪些工具尝试是绕路？
最后真正有效的路径是什么？
下次遇到类似任务，应该先走哪条路线？
```

结论：

- 夜间反思不是把历史聊天继续塞进上下文。
- 夜间反思不是自动品悟，也不应恢复常驻自动审查。
- 夜间反思不是重做 DeepSeek-TUI 的 Session / Engine / Compaction。
- 夜间反思是 app 层的“批量 Memory Review + Experience Compiler”。
- 产物应分为三层：`episodic` 任务案例、`procedural` 执行经验、`runtime` 短注入。
- 运行时只注入与当前任务相关的 1-3 条经验，硬控 token 预算。
- 最终产品形态是“夜间自动分析、白天人工确认、确认后才按需应用”，绝不自动激活模型生成的规则。
- 夜间运行只生成 `proposed` 候选；任何会改变后续工具选择或对话行为的经验，都必须允许用户查看证据、编辑、应用、暂停和删除。
- 第一阶段必须先证明候选质量，再接运行时注入；不能用“CI 通过”代替真实任务回放和效果验证。

---

## 2. 外部依据

### 2.1 Memory 是 write-manage-read 闭环

2026 年综述 *Memory for Autonomous LLM Agents* 将 agent memory 形式化为 `write -> manage -> read`，并强调写入不是 append，而要做摘要、去重、优先级、矛盾处理、删除和治理。论文也把记忆分为 working / episodic / semantic / procedural，并指出程序性记忆是“可复用技能和计划”。

Pinvou 启发：夜间反思应有明确的 `collect -> reflect -> gate -> promote -> retrieve` 流程，不能只生成“今日总结”。

来源：https://arxiv.org/html/2603.07670v1

### 2.2 从 Storage 到 Reflection 到 Experience

2026 年综述 *From Storage to Experience* 把 agent memory 演进分为三阶段：

- Storage：忠实保留轨迹。
- Reflection：对轨迹做评价和纠错，提炼高质量洞察。
- Experience：跨轨迹抽象，形成可指导未来行为的规则和策略。

Pinvou 启发：`~/.pinvou3/sessions` 是 storage；夜间任务做 reflection；稳定规则进入 experience。

来源：https://arxiv.org/html/2605.06716

### 2.3 Record & Replay 适合“类似任务变快”

2025 年 *Agent Record & Replay* 提出：记录 agent 与环境交互轨迹，将其摘要为结构化 experience，并在后续相似任务中 replay，用 check functions 保证完整性和安全。

Pinvou 启发：不要只记“上次错了”，而要记录“适用条件 + 推荐路线 + 检查函数”。

来源：https://arxiv.org/html/2505.17716

### 2.4 Procedural Memory 能减少重复探索

2025 年 *Memp: Exploring Agent Procedural Memory* 指出，类似任务在同一环境中反复执行时，agent 会重复花费 token 和时间理解环境；程序性记忆通过构建、检索、更新可复用过程来减少重复探索。实验中 proceduralization 在 TravelPlanner、ALFWorld 等任务上减少步骤或提高任务指标。

Pinvou 启发：对飞书会议、H3C PPT、内部知识查询、历史 session 总结等高频任务，要沉淀“怎么做”的步骤，而不是保存完整失败轨迹。

来源：https://arxiv.org/html/2508.06433v2

### 2.5 程序性记忆需要评估和适用边界

2026 年 *Managing Procedural Memory in LLM Agents* 指出，程序性记忆在企业工作流中能带来稳定收益，但部分技能会过拟合角色或任务，迁移时失效。

Pinvou 启发：每条经验必须有 `scope`、`confidence`、`last_verified_at`、`negative_evidence`，不能无条件全局生效。

来源：https://arxiv.org/abs/2606.23127

### 2.6 背景写入优于热路径写入

LangGraph / LangChain memory 文档区分 hot path 与 background 写入：热路径能即时生效但增加延迟、分散 agent 注意力；后台写入能隔离应用逻辑和记忆管理，适合 cron / idle 触发。

Pinvou 启发：用户提出的“夜深人静时执行一次”是正确方向。Pinvou 本地 Qwen 推理慢，反思必须离线批处理，不在用户等待路径里做。

来源：https://docs.langchain.com/oss/python/concepts/memory

### 2.7 记忆分层与作用域隔离

Mem0 文档将记忆分为 conversation / session / user / organizational，并明确短期与长期的区别。长期记忆包含 factual / episodic / semantic，且建议按 `user_id`、`run_id`、metadata 控制范围。

Pinvou 启发：夜间反思不要污染用户画像；执行经验应单独建 experience store，按用户、workspace、task_type 和 skill 作用域隔离。

来源：https://docs.mem0.ai/core-concepts/memory-types

### 2.8 记忆治理与漂移防控

2026 年 *Governing Evolving Memory in LLM Agents* 提出受治理的记忆更新：保留不可变 raw ledger，用 gated writing 防错误固化，用周期 reconciliation 防漂移。

Pinvou 启发：反思产物必须可追溯到原 session/tool_result；高影响规则默认进入候选，不直接改 instructions / skill。

来源：https://arxiv.org/html/2603.11768v1

---

## 3. Pinvou 当前样本观察

基于本机 `~/.pinvou3/sessions` 的粗扫描：

- 顶层 session JSON 约 52 个。
- 工具调用统计里 `exec_shell` 约 765 次，远高于 `read_file` 76 次、`web_search` 64 次。
- 多个慢任务的主要绕路不是模型“不知道答案”，而是没有先路由到现成 skill / MCP / 工作流。

高价值经验样本：

| 任务 | 观察到的绕路 | 应沉淀经验 |
|---|---|---|
| 飞书会议 | 先 shell 搜索 CLI，多轮探索后才完成 | 飞书/日历/会议室任务先走 `lark-contact` + `lark-calendar`，不要先 shell 找 CLI |
| H3C PPT | 临时造导出/截图脚本，反复试错 | H3C deck 任务直接走 `H3C-PPT` skill 的 audit/build |
| H3C 内部资料 | zhidao 登录/浏览器控制绕路 | 内部资料优先 zhidao；认证失败一次后请求用户登录，不继续控制浏览器 |
| 用户文件读取 | 把 `~/Documents` 读成 session workspace 下的相对路径 | 读取真实用户文件时先展开 `~`，产物写相对路径是另一条规则 |
| session 总结 | 假设存在 `daily/session-index` 等不存在路径 | 历史 session 总结直接读 `~/.pinvou3/sessions/*.json` metadata/messages |

这些经验应作为首批 `procedural_candidates`，由人确认后转为 active。

---

## 4. 设计目标

### 4.1 一等目标

让 Pinvou 在类似任务中更快进入有效路径：

- 减少无效工具调用。
- 减少重复环境探索。
- 减少错误工具路由。
- 减少用户重复纠正。
- 保持上下文注入极短。

### 4.2 非目标

- 不做新的通用 Session 系统。
- 不做新的 Compaction 系统。
- 不把所有历史聊天变成长期记忆。
- 不自动修改 `instructions.md`、`AGENTS.md`、`SKILL.md`。
- 不把品悟 v4 改回常驻审查。
- MVP 不上知识图谱。

---

## 5. 总体架构

```text
用户启用夜间反思 / 手动触发
  ↓
Idle + Model Busy Gate
仅在应用空闲、本地模型空闲且未超过资源预算时运行
  ↓
Incremental Session Scanner
只读取 fingerprint 发生变化的 session
  ↓
读取 sessions / artifacts / pinvou_reviews / timing_events
  ↓
Run Extractor
确定性抽取工具序列、规范化失败类型、完成状态、耗时、产物
  ↓
Reflection Classifier
本地 LLM 归因：route_miss / auth_blocked / path_error / over_search / tool_misuse / build_loop
  ↓
Experience Compiler
生成 episodic_memory / procedural_candidate / memory_review_candidate
  ↓
Gate
敏感过滤、证据检查、去重、作用域、置信度、TTL
  ↓
Experience Store
候选一律以 proposed 状态落盘，不自动生效
  ↓
Candidate Review UI
用户查看证据 → 编辑并应用 / 忽略 / 永不建议
  ↓（只有用户明确应用）
Active Procedure Store
  ↓
Runtime Retriever
下一次任务开始前检索 1-3 条相关经验
  ↓
Short Injection
注入 ≤400/800 token 的 task_experience block
```

---

## 6. 数据来源

### 6.1 必读

| 来源 | 用途 |
|---|---|
| `~/.pinvou3/sessions/*.json` | session metadata、messages、artifacts |
| `~/.pinvou3/sessions/<sid>/timing_events.jsonl` | turn 耗时、完成状态 |
| `~/.pinvou3/sessions/<sid>/pinvou_reviews.json` | 品悟审查结果和 resolution |
| `~/.pinvou3/sessions/<sid>/workspace/` | 产物线索：文件名、大小、mtime |
| `~/.pinvou3/user/memory/` | 用户记忆事实源，必要时复用 gate |

### 6.2 读取原则

- 默认不读取完整产物正文。
- 默认不把原始 messages 写入长期经验库。
- 对工具结果只保存摘要、错误类型、命令类别，不保存敏感 stdout 全文。
- 读入口优先走 app 内路径 API / SessionStore；离线 fallback 才直接扫文件。

---

## 7. 存储设计

MVP 数据量有硬上限，不需要先引入 SQLite。使用版本化 JSON + 每次运行独立审计文件即可；所有写入必须原子替换，后续只有在查询或并发规模证明 JSON 不够时才迁移 SQLite。

```text
~/.pinvou3/experience/
  procedures.json
  ignored_fingerprints.json
  scanner_state.json
  reflection_runs/
    reflection-20260716023000.json
  runtime/
    <session_id>.md
```

容量与保留策略：

- `active` 经验最多 80 条。
- `proposed + active + paused` 最多 160 条。
- 达到上限时先清理过期候选和已归档项目，不得静默覆盖 active 经验。
- 已忽略/归档记录保留 90 天；`reflection_runs` 保留最近 30 天。
- `never` 不保留完整规则正文，只保存归一化指纹和用户选择原因，防止反复建议同类规则。

### 7.1 `task_runs`

```json
{
  "id": "run_...",
  "session_id": "rceq67h8szjd0",
  "title": "利用我的飞书CLI，在下周二到下周三两天...",
  "task_type": "lark_calendar",
  "started_at": "2026-07-16T06:30:14Z",
  "ended_at": "2026-07-16T07:52:15Z",
  "turn_count": 6,
  "tool_call_count": 63,
  "failed_tool_count": 15,
  "artifacts_count": 1,
  "status": "completed",
  "signals": ["high_tool_count", "route_miss", "auth_or_cli_discovery"]
}
```

### 7.2 `episodic_memories`

存“某次任务发生了什么”，用于少量案例召回和证据追溯。

```json
{
  "id": "epi_...",
  "task_type": "lark_calendar",
  "scope": {
    "user_id": "local",
    "workspace": null,
    "skill": "lark-calendar"
  },
  "observation": "用户要求创建飞书会议，agent 前期用 shell 搜索 CLI，产生多轮探索。",
  "action": "最终通过飞书相关 CLI/接口创建日程并确认参会人。",
  "result": "completed",
  "lesson": "飞书会议任务应优先走 lark-contact + lark-calendar，不先 shell 搜索 CLI。",
  "evidence": {
    "session_id": "rceq67h8szjd0",
    "tool_calls": 63,
    "failed_tools": 15
  },
  "confidence": 0.78,
  "created_at": "2026-07-16T23:30:00+08:00"
}
```

### 7.3 `procedural_candidates`

存“下次怎么做”的候选规则。默认不直接生效。

```json
{
  "id": "proc_...",
  "status": "proposed",
  "kind": "route_rule",
  "task_pattern": "飞书|日历|会议|会议室|参会人",
  "scope": {
    "user_id": "local",
    "workspace": "*",
    "skills": ["lark-contact", "lark-calendar"]
  },
  "rule": "飞书/日历/会议室任务：先用 lark-contact 解析人员，再用 lark-calendar 查询会议室并创建日程；不要先用 shell 搜索 feishu/lark CLI。",
  "rationale": "历史 session 显示先 shell 探索 CLI 造成多轮绕路。",
  "evidence": ["rceq67h8szjd0"],
  "confidence": 0.82,
  "impact": "high",
  "risk": "medium",
  "requires_user_approval": true,
  "last_verified_at": null,
  "negative_evidence": []
}
```

### 7.4 `active_procedures`

只存已批准、低风险、可注入的短规则。

```json
{
  "id": "proc_...",
  "task_pattern": "H3C.*PPT|deck|送审|客户汇报",
  "rule": "H3C deck 任务直接走 H3C-PPT skill 的 audit/build，不临时手写截图或导出脚本。",
  "max_injection_tokens": 80,
  "confidence": 0.9,
  "valid_until": null
}
```

### 7.5 `reflection_runs`

审计日志，不参与模型注入。

```json
{
  "date": "2026-07-16",
  "started_at": "2026-07-16T02:30:00+08:00",
  "ended_at": "2026-07-16T02:33:12+08:00",
  "sessions_scanned": 18,
  "task_runs_written": 7,
  "episodic_written": 4,
  "procedural_candidates_written": 3,
  "skipped": [
    {
      "session_id": "xxx",
      "reason": "no_user_task"
    }
  ],
  "model_calls": 5,
  "token_estimate": 18400
}
```

---

## 8. 反思分类

MVP 先用有限枚举，避免 LLM 自由发挥：

| 分类 | 含义 | 例子 |
|---|---|---|
| `route_miss` | 没有先走合适 skill / MCP / workflow | 飞书会议先 shell 找 CLI |
| `auth_blocked` | 卡在登录/授权，重复尝试 | snap Firefox 无法传 URL |
| `path_error` | 路径语义错误 | `workspace/~/Documents` |
| `over_search` | 搜索范围过宽或重复 | 简单任务全仓 find |
| `tool_misuse` | 工具参数或工具选择错误 | zhidao 不支持 `--limit` |
| `build_loop` | 产物构建/截图/导出反复试错 | PPT 临时导出脚本 |
| `success_recipe` | 有明确成功路径可复用 | H3C-PPT skill 审计链路 |
| `user_preference` | 用户表达稳定偏好 | 进入现有 memory 候选 |

---

## 9. Gate 规则

### 9.1 自动写入

满足以下条件可自动写入 `episodic_memories`：

- 低敏。
- 有明确 session evidence。
- 不包含密钥、token、私人身份信息。
- 只是“案例记录”，不直接改变未来行为。

### 9.2 候选确认

以下内容进入 `procedural_candidates`，默认不激活：

- 会改变工具路由的规则。
- 会影响跨任务行为的 guard。
- 需要写入 skill / instructions 的建议。
- 置信度 < 0.85 但潜在收益高的经验。

### 9.3 必须跳过

- 原始工具 stdout 含 credential、token、session_id、cookie。
- 用户明确隐私内容。
- 只发生一次且没有成功/失败反馈的推测。
- 对外部事实的结论性规则，例如“某政策不存在”。
- pinvou 当前运行环境、临时路径、模型配置，除非用户正在长期开发 Pinvou 且明确相关。

### 9.4 候选生命周期与用户控制

状态流转：

```text
proposed ──用户应用──> active ──暂停──> paused
   │                       │              │
   ├──忽略──> archived     ├──删除        └──重新启用
   └──永不建议──> never fingerprint
```

- 候选支持“编辑后应用、直接应用、忽略、永不建议”。
- Active 经验支持“编辑、暂停、删除”；Paused 经验支持“重新启用、删除”。
- 用户手动编辑视为一次明确批准，但必须保留版本号、修改时间和来源 evidence。
- 模型重新生成或改写已有规则时，不得覆盖用户已批准版本，必须生成新的 proposed revision 重新确认。
- “删除”允许未来基于新证据再次建议；“永不建议”用于阻止同类规则反复出现。
- 删除规则正文后仍可保留不含正文的最小审计 tombstone；用户选择彻底清除时应一并删除。

---

## 10. Runtime 注入策略

经验库可以变大，但每次上下文注入必须小。

预算：

| 任务 | 注入条数 | token 上限 |
|---|---:|---:|
| 普通任务 | 3 | 400 |
| 复杂任务 | 3 | 600 |
| 高风险任务 | 3 | 800 |

注入模板：

```markdown
<pinvou_task_experience>
- 飞书/日历/会议室任务：先用 lark-contact 解析人员，再用 lark-calendar 查会议室并创建日程；不要先用 shell 搜索 feishu/lark CLI。
- H3C deck 任务：直接走 H3C-PPT skill 的 audit/build，不临时手写截图或导出脚本。
</pinvou_task_experience>
```

排序公式 MVP：

```text
score =
  task_pattern_match * 0.35
+ recency * 0.10
+ confidence * 0.20
+ impact * 0.20
+ evidence_count * 0.10
+ last_success_bonus * 0.05
```

冲突处理：

- 当前用户指令 > 当前工具输出/文件内容 > active procedure > episodic memory。
- 经验提示必须标注“可过期、遇冲突以实时工具为准”。
- 同一 task_type 最多注入 2 条经验，防止单领域经验霸占上下文。
- 单次任务最多注入 3 条；用户可以查看“本轮使用了哪些经验”，并可从该提示直接暂停或删除。
- 未经用户确认的 proposed 候选永远不进入 runtime prompt。

---

## 11. 调度策略

触发：

- 首次默认关闭；用户明确启用后，每天 02:30 本地时间尝试运行。
- 或应用空闲 30 分钟后。
- 或用户手动点击“复盘最近任务”。
- 夜间运行只生成候选，不自动激活、不自动修改 `instructions.md` / `SKILL.md`。

限额：

- 每次最多扫描最近 24 小时内发生变化的 20 个 session。
- 每次最多尝试 4 次 LLM 调用；失败和超时同样消耗预算，不能只统计成功调用。
- 单个 session 输入给反思模型的材料 ≤ 6000 字符。
- 总耗时硬上限 5 分钟，支持取消；达到 deadline 后保存已完成结果并停止。
- GB10 忙、正在聊天、正在执行前台任务或 vLLM 不可用时延后，写 reflection_runs skip 记录，不做高频重试。
- 同一时间只允许一个反思 run；进程重启后不得重复执行当天已经完成的 run。

增量：

- 以 `session_id + updated_at + message_count + artifacts_count` 做 fingerprint。
- fingerprint 未变化则不重复反思。
- scanner state 与候选写入必须原子持久化；模型调用失败时不能把 session 错误标记为已完成。

---

## 12. 与现有模块关系

### 12.1 与用户记忆

夜间反思不替代用户记忆。它只在发现稳定用户偏好、当前关注、近期动态时，调用现有 Memory Review / Gate 写入 `~/.pinvou3/user/memory/`。

执行经验默认写入 `~/.pinvou3/experience/`。

### 12.2 与品悟 v4

夜间反思可以读取 `pinvou_reviews.json` 判断产物是否被审过、哪些问题反复出现，但不自动触发品悟、不自动转交主 AI、不弹窗。

### 12.3 与 compaction

夜间反思离线批处理，不能增加每轮上下文压力。runtime 注入必须短，且不参与 compaction 触发线计算之外的特殊逻辑。

### 12.4 与 SkillRegistry

高频稳定经验应升级为 skill 说明或 workflow，而不是永久 runtime 注入。

升级条件：

- 同一 rule 命中 ≥ 3 次。
- 最近 2 次使用后任务耗时/失败工具数下降。
- 没有 negative_evidence。
- 人确认。

---

## 13. MVP 范围

### P0：离线报告，不影响运行时

- 手动扫描最近 session。
- 确定性抽取工具顺序、规范化失败、完成状态、耗时、产物与 review 结果。
- 生成 `reflection_runs/reflection-<timestamp>.json`。
- 生成 `procedural_candidates`。
- 不注入、不自动激活。
- 提供最小报告界面，让用户看见“发现了什么、依据是什么”，但本阶段不提供 active/runtime 行为。

验收：

- 能识别飞书会议、H3C PPT、路径错误、session 总结四类经验。
- 所有候选可追溯到 session evidence。
- 缺少成功路径或完成证据时只能生成问题报告，不能生成会改变未来行为的规则。
- 原始用户/助手消息、工具 stdout、密钥、私人路径和身份信息既不发送给模型，也不写入长期经验记录。

### P1：夜间候选 + 人工确认 + 短注入

- 用户明确启用后，夜间/空闲时增量生成候选。
- UI 展示候选规则。
- 用户可编辑并应用、忽略、稍后或永不建议。
- 批准后进入 `active_procedures`。
- 新任务开始前检索并注入 ≤400 token。
- UI 对用户透明显示本轮命中的经验，并支持暂停/删除。

验收：

- 飞书会议类任务首轮不再 shell 搜索 CLI。
- H3C PPT 类任务首轮加载 H3C-PPT skill。

### P2：效果评估

- 记录经验命中与任务结果。
- 对比命中前后：
  - tool_call_count
  - failed_tool_count
  - elapsed_sec
  - user correction count
  - task completion status

验收：

- 至少 5 个高频任务类型有基线与改进数据。
- 能自动降权无效经验。

### P3：经验升级

- 把稳定经验转为 skill patch 建议。
- 生成 PR checklist 或候选 diff。
- 仍需人工确认。

---

## 14. 首批回放样本与候选模板

以下内容用于真实 session 回放和验收示例，不作为内置规则发布，也不直接写入 `instructions.md`。实际候选必须由当前设备上的 evidence 生成：

```json
[
  {
    "task_pattern": "飞书|日历|会议|会议室|参会人",
    "rule": "先用 lark-contact 解析人员，再用 lark-calendar 查询会议室并创建日程；不要先用 shell 搜索 feishu/lark CLI。",
    "evidence": ["session_hash_lark_01"],
    "impact": "high"
  },
  {
    "task_pattern": "H3C.*PPT|deck|送审|客户汇报",
    "rule": "直接走 H3C-PPT skill 的 audit/build，不临时手写截图或导出脚本。",
    "evidence": ["session_hash_ppt_01", "session_hash_ppt_02"],
    "impact": "high"
  },
  {
    "task_pattern": "H3C.*内部资料|产品|解决方案|发文|制度",
    "rule": "优先 zhidao skill；认证未完成时请求用户登录，不反复控制浏览器。",
    "evidence": ["session_hash_knowledge_01"],
    "impact": "medium"
  },
  {
    "task_pattern": "读取用户文件|Documents|下载|桌面",
    "rule": "读取真实用户文件时先按当前系统用户目录解析 ~；不要把 ~/Documents 当成 session workspace 下相对路径。",
    "evidence": ["session_hash_path_01"],
    "impact": "medium"
  },
  {
    "task_pattern": "今天.*session|历史 session|其他 session",
    "rule": "直接读取 ~/.pinvou3/sessions/*.json 的 metadata/messages；不要假设存在 daily/session-index。",
    "evidence": ["session_hash_history_01", "session_hash_history_02"],
    "impact": "medium"
  }
]
```

---

## 15. 已收敛决策与开放问题

- 数据语义已收敛：保留独立 `experience.rs` / experience store，不把程序性经验混进用户画像；模型 JSON review、provider dialect、原子存储和候选卡片交互尽量复用现有 memory 公共能力。
- UI 归属已收敛：候选和 active procedures 放在现有记忆管理区域的“任务经验”分区，不重新开放或混入通用 Scheduled Tasks 创建入口。
- 透明度已收敛：经验命中后在当前任务中轻提示，并可进入设置页查看、暂停或删除。

仍待实现前用原型验证：

- runtime 注入放在 session instructions、system-reminder，还是工具路由前置层？
- 是否允许用户设置“只反思今天，不读取旧 session”？
- 当用户编辑规则的 task pattern 或 scope 时，是否要求二次风险确认？

---

## 16. 建议下一步

1. 保持 #184 关闭，只保留本 Spec 中仍成立的产品目标和设计依据。
2. 关闭 #189，不在原分支继续叠加实现；后续新开 P0 PR，只实现有证据的增量抽取、候选报告、预算/取消/保留期和最小查看界面，不保留没有调用方的 active/preview 假闭环。
3. 用当前本机 sessions 跑一遍，人工审核候选质量，并做飞书会议、H3C PPT、路径错误、session 总结、内部资料五类回放。
4. 候选质量达标后，再用独立 PR 实现 P1 的夜间调度、人工确认和透明短注入。
5. P2 验证命中前后的工具调用、失败率、耗时和用户纠正；只有稳定收益的经验才进入 P3 skill/workflow 升级建议。

---

## 17. PR #184 / #189 评审结论与实现约束

### 17.1 #184 为什么不能恢复

#184 的产品闭环较完整，但实现将“生成候选”和“改变未来行为”绑在一起，存在以下阻断：

- 默认启用、自动激活并直接注入后续对话，没有人工 Gate。
- 读取并向模型发送用户/助手原文片段，关键词过滤不足以构成隐私边界。
- 内置规则包含特定用户目录，不能跨设备或跨用户使用。
- 启动即运行后台循环，缺少可靠的空闲检测、取消、当天幂等和资源隔离。
- 同一 PR 重新开放定时任务 UI，混入与反思内核无关的既有产品问题。

因此 #184 继续保持关闭，不以 reopen 或局部修补方式恢复。

### 17.2 #189 哪些方向正确

#189 对 #184 做了必要的安全收缩：

- 默认只生成 proposed 候选，不自动激活。
- 不启动后台 loop，不接入 chat 注入。
- 模型默认只允许 loopback 本地端点。
- 发给模型的是结构化遥测，不包含消息原文和工具 stdout。
- experience store 与用户画像记忆分离。

这些边界应保留。

### 17.3 #189 当前为什么仍不能合入

1. **证据不足**：模型只看到工具直方图、失败数量、少量 task hints 和粗粒度 signals，看不到工具顺序、具体规范化失败、最终成功路径、任务完成状态、耗时和 review 结果，无法可靠区分“正确使用 Shell”和“Shell 是绕路”。
2. **未达到 P0 验收**：当前无法从输入中可靠识别 `path_error`、`tool_misuse`、`build_loop` 或 `success_recipe`，也没有五类真实样本回放。
3. **调用预算 bug**：现实现只在请求成功后增加 `model_calls`；连续失败或超时不会消耗预算，最多可能依次等待 20 个 session × 180 秒。
4. **无增量和保留期**：每次重复扫描最近 session；procedures 与 reflection runs 没有容量和清理策略。
5. **候选合并不完整**：重复候选只增加 evidence 和提高 confidence，不处理规则修订、冲突、negative evidence 或用户已批准版本。
6. **模型兼容性不足**：反思请求单独直连 `/chat/completions`，没有复用现有 memory review 的 provider/model dialect、reasoning control、取消与 JSON 解析公共能力。
7. **隐私仍有尾部风险**：模型请求不发送标题是正确的，但本地 run 记录仍保存弱脱敏标题；中文姓名、地址等可能长期残留。
8. **没有用户入口**：只有 Rust/Tauri command，没有 JS bridge 和界面；用户无法运行、查看、编辑、应用或删除，active/preview 也没有实际 chat 调用方。

### 17.4 P0 必须采用的 evidence schema

模型输入只允许使用本地确定性抽取后的 allowlist 字段，例如：

```json
{
  "anonymous_run_id": "run_hash",
  "task_type": "lark_calendar",
  "completion": "completed",
  "elapsed_bucket": "10m-30m",
  "tool_sequence": [
    {"tool": "exec_shell", "outcome": "failed", "error_kind": "command_not_found"},
    {"tool": "lark_contact", "outcome": "success"},
    {"tool": "lark_calendar", "outcome": "success"}
  ],
  "artifact_result": "not_applicable",
  "review_result": "accepted",
  "signals": ["route_miss", "repeated_failure"]
}
```

约束：

- 模型不接收原始 session id、标题、message、工具参数、stdout、文件路径或身份信息。
- 本地 evidence map 单独保存匿名 run id 到 session 的映射，只用于用户查看来源和审计。
- error kind、task type、completion 和 review result 必须是有限枚举，不允许把任意模型文本当作事实。
- 没有成功证据时，模型只能输出诊断候选，不能输出 active-capable procedure。

### 17.5 候选确认界面

候选卡片至少展示：

- 发现的重复问题。
- 涉及的任务数量。
- 失败路径与最终成功路径。
- 建议规则及适用范围。
- 风险、置信度、最近证据时间。
- 应用后会影响哪些任务。

不能只展示一句模型生成的规则让用户盲目确认。

### 17.6 验证矩阵

合入 P0 前至少覆盖：

| 场景 | 必须证明 |
|---|---|
| 飞书会议 | 能识别先 Shell 探索、后联系人/日历成功的 route miss |
| H3C PPT | 能识别临时脚本绕路和既有 Skill 成功路径 |
| 路径错误 | 能识别路径语义错误，但不泄露真实路径 |
| Session 总结 | 能识别不存在索引的重复探索和正确数据源 |
| 证据不足 | 只生成问题报告，不生成可应用规则 |
| 隐私 | 请求和持久化文件均不包含原始消息、stdout、路径或身份信息 |
| 预算 | 成功、失败、超时均消耗尝试次数，总运行不超过 deadline |
| 增量 | fingerprint 未变不重复调用，失败任务不会被错误标记完成 |
| 生命周期 | 编辑、暂停、删除、永不建议、过期和容量清理均有测试 |
| 模型兼容 | 使用 fake server 验证实际请求，并覆盖本地 Qwen/vLLM reasoning 参数 |

### 17.7 最终决策

- **功能有意义**：它补充的是“任务怎么做”的程序性经验，不替代用户记忆、Skill、品悟产物审查或 DeepSeek-TUI compaction。
- **最终产品值得做**：高频重复任务、本地小模型工具路由和跨任务复用都能从中受益。
- **价值必须用效果证明**：至少在 5 类高频任务上比较命中前后的工具调用数、失败工具数、耗时和用户纠正。
- **#189 不可合入**：安全边界方向正确，但还是不可达、证据不足且生命周期不完整的后端草案，设计不完善，不予上线。
- **推荐处理**：#184、#189 均保持关闭；未来基于本文新开 P0 实现 PR，真实回放通过后再开发 P1。
