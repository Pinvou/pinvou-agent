# 完全体 Code 模式架构设计

> 关联文档：`docs/code-native-agent.md`（现状归档）、`docs/code-native-agent-安全审查问题清单.md`（安全急件，已修复合入）、`docs/code-native-agent-会话能力档案设计.md`（能力档案，本设计阶段一的核心组成）、`docs/code-native-agent-安全审查修复-改动说明.md`。
> 状态：设计提案，待评审。参考系：VSCode agent 模式（见 §2）。

## 1. 现状：code 模式与 chat 模式的依赖逻辑

两条链路在三个点汇合——`SessionStore`（统一存储）、`EnginePool`（统一引擎池）、各自的展示层；分叉靠一个布尔标志 `code_session` + 三个各自注入的谓词闭包完成。

### 1.1 结构性病灶（语义不同却被迫共用，按严重度排序）

1. **skill 机制是进程级全局，会话语义是按类型的**（病灶核心）。`skill_marketplace.rs:123-152` 只读 plain scope 推全局 `DISABLED_SKILLS`；代码会话只能整禁 `load_skill`（方案 D）且 catalogue 仍泄露路径。安全审查 #4/#5 的根源。
2. **会话类型真相源分散且无类型保证**。类型 = `SessionAgentStore.code_session` 标志 + sidecar + `AgentBackend::Deepseek` 枚举复用（Native 与 chat 共用一个枚举值）+ 三处谓词闭包注入（bridge `lib.rs:455`、remote_control `lib.rs:461`、SessionStore resolver `lib.rs:447`）。聊天会话在类型 store 里无记录。
3. **`codex_acp` / `CodexAcpView` 一名三义**。Rust 侧 5416 行模块同时承载 ACP 子进程托管与 Native store/恢复；前端 2747 行视图内 `isNativeAgent` 三元分流约 30 处。与 `docs/code-native-agent.md` §10 待决策项 1 互为印证。
4. **bridge 聊天状态机绑单一 activeSession，代码车道被迫平行重建**。`code-native-lane.js`（465 行）重实现事件消费/乐观气泡/回声去重；共享控件靠「显式会话态 props 优先、否则回落 bridge active」双模驱动（`composer-controls.jsx:1-7` 头注自认），每新增共享控件都要付这笔税。
5. **工具整形两层补丁机制**。`tool_policy` 全局闭包产初值（`lib.rs:423-435`）→ `shape_disallowed_tools` 按会话打补丁 → spawn 时覆盖。分歧靠运行期修补而非声明式 profile。
6. **会话列表互斥过滤**。`list_sessions` 反向排除 vs `list_codex_acp_sessions` 正向纳入，同源存储两端各写守卫，历史已串过一次列表。
7. **远程控制事件过滤是外挂谓词而非通道设计**，RPC 面未封（审查 #11）。
8. **chat 能力默认渗入 code 会话**：`kb_search` 常驻注入所有 engine 靠「为空则 disallowed」兜底；MCP/skills_dir/hooks 全进程共享。code 会话目前是「chat 全集 − 补丁减法」。

### 1.2 合理复用（应保留的共享底座）

- `SessionStore` + `SessionRoots` 双根（安全审查 #6 已收敛为单一入口）；
- `EnginePool` 共池 + per-session 隔离（锁/shell/模型修订），池不感知会话类型是正确的抽象层级；
- `ConnectorScope` 双 scope（`marketplace/mod.rs`）——唯一真正做到「按会话类型分型 + 各自持久化 + 安全默认」的通道，**是本设计的现成范本**；
- 提示词三层分层（shared/work/code + golden 测试）；
- 账本体系、标题命名、hooks 敏感目录防火墙、shell 后台任务、i18n、基础展示组件。

一句话总结：**code 模式现在不是一个「模式」，而是 chat 会话上的一个补丁层**——类型靠标志、能力靠减法、UI 靠分支。

## 2. 参考系：VSCode agent 模式

来源：[VS Code 官方文档](https://code.visualstudio.com/docs/copilot/chat/chat-agent-mode)。其架构要点：

1. **会话配置一等化**：每个会话声明式携带「agent 类型 / 自定义 agent / 权限级别 / 模型」，spawn 时定型——非全局开关，非运行时补丁。
2. **模式即能力集**：Ask（只读问答）/ Edit（定向编辑）/ Plan（只读规划）/ Agent（自主执行）是预定义能力档案；custom agents（`.agent.md`）是用户自定义档案（角色 + 工具 + 模型）。
3. **双表面**：Chat view（code-first，编辑器侧栏辅助当前工作区）+ Agents 窗口（agent-first，跨项目编排、并行/后台会话）。
4. **变更管理闭环**：inline diff 逐 edit keep/undo + checkpoints（关键点自动快照、可回滚）+ stage-to-accept。
5. **权限分级**：工具逐次批准 / 自动批准规则 / workspace trust——危险面与信任级别显式绑定。
6. **会话控制**：运行中消息 queue / steer / stop 三语义；并行会话与后台完成通知。
7. **可观测性**：Agent Logs（工具调用/请求事件流）+ Chat Debug（原始 prompt/context/tool payload）。
8. **扩展面**：MCP 工具 + instructions 文件 + prompt files，全部挂到会话配置上。

## 3. 差距分析

| VSCode 概念 | 品悟现状 | 差距 |
|---|---|---|
| 会话配置四元组 | 布尔标志 + 谓词闭包 | 无声明式会话描述 |
| 模式 = 能力档案 | chat 全集 − 补丁 | 仅连接器分型；skill/MCP/kb 无档案 |
| custom agents | bundle skills（进程级过滤） | 能力档案落地后自然获得 |
| 变更管理 | diff 预览 + 工作区基线指纹 | 无 checkpoint 回滚、无逐 edit keep/undo |
| 权限分级 | trust_mode + hooks 防火墙 + yolo/plan | plan 降级（§9 已知限制）；无项目目录信任级别 |
| queue/steer/stop | 停止轮次 | 无 queue/steer，运行中只能停 |
| 双表面/编排 | chat 页 + code 页并列 | 无跨项目编排面、无后台会话 |
| 可观测性 | fork 有 `dump_system_prompt` | 无会话级 agent log/debug 视图 |
| 远程面 | 事件过滤外挂谓词 | RPC 面未封（#11），远程不支持代码会话 |

## 4. 目标架构：SessionDescriptor

核心思想：把「会话类型」从布尔标志提升为声明式会话描述对象，把「能力」从运行时补丁提升为 spawn 时定型的档案——即 VSCode「session configuration」的对应物，也是会话能力档案设计的自然延伸。

```rust
SessionDescriptor {
    kind: SessionKind,               // Chat | CodeNative | CodeAcp | Scheduled
    roots: SessionRoots,             // 已有（安全审查 #6 已收敛入口）
    capabilities: CapabilityProfile, // tools / skills / connectors / prompt 层
    permissions: PermissionLevel,    // 审批策略 / 工作区信任 / 远程可见性
    model: ModelRef,                 // 会话级模型选择（已有雏形）
}
```

spawn 时一次性解析定型（经 `build_engine_config` 下发），之后所有能力通道只查本会话描述；热更走既有 Op 广播通道（`Op::SetDisallowedTools` 为现成范例）。**消灭谓词闭包穿线与「全集 − 补丁」**。

## 5. 分阶段演进

### 阶段一：类型与能力地基（两项互为表里，建议同 PR 或紧随其后）

**1. SessionKind 一等化**
- `SessionStore` 记录增加类型化 `kind` 字段，创建时定型、类型化查询；
- `code_session` 标志 + sidecar + `AgentBackend::Deepseek` 复用 + 三个谓词闭包收敛为一；
- 会话列表互斥守卫、远程过滤外挂谓词随之退役——串列表/误过滤类 bug 在编译期不可能；
- sidecar 保留为崩溃恢复用途，但不再是类型判定的并行真相源。

**2. CapabilityProfile 落地**（= `docs/code-native-agent-会话能力档案设计.md` 实施 + 推广）
- skill：catalogue 按会话过滤 + `load_skill` 按档案判定（根治 #4/#5 残留；方案 D 两组件退役，代码页开关恢复可交互）；替换语义已定稿（「plain 关、code 开」成立）；
- tools：收掉 `tool_policy` 全局闭包 + `shape_disallowed_tools` 补丁两层机制，兑现待决策项 3「代码专用工具 profile」；
- kb_search 等 chat 原生能力从「常驻 + 兜底」改为档案声明；
- connectors：沿用 ConnectorScope（范本），归入档案统一解析；
- prefix-cache：会话内档案恒定，落在 static 前缀区，分叉稳定可缓存（评估见能力档案设计 §7）。

### 阶段二：code 模式体验闭环（VSCode 对齐）

**3. CodeView 上提**（待决策项 1，阶段二地基）
- Rust：`codex_acp`（5416 行）拆为 `code_sessions` + ACP adapter + Native adapter；
- 前端：`CodexAcpView` 的 `isNativeAgent` 三元分流收进 adapter；
- bridge 状态机从「绑聊天 active」改为**会话作用域状态机**，`code-native-lane.js` 平行重建退役——消灭「双模驱动税」的唯一正解。

**4. 变更管理闭环**
- 工作区基线指纹已有一半；补 checkpoint（每轮工具执行前自动快照）+ 逐 edit keep/undo + 一键回滚；
- checkpoint 数据落账本根（与审计/产物同体系）。

**5. 权限分级**
- 绑定项目目录引入 workspace trust（首次绑定显式授权 + 已有敏感目录防火墙）；
- plan 模式完整化（§9 降级项：accept_plan 审批卡）；
- 审批策略入 `SessionDescriptor.permissions`。

**6. 会话控制与可观测**
- 运行中消息 queue / steer（引擎 Op 通道有热更先例）；
- 会话级 agent log 视图（`chat:*` 事件流已有，缺消费 UI）。

### 阶段三：编排面（远期，独立评估）

**7.** 跨项目 Agents 窗口、后台会话 + 完成通知、子代理/worktree 隔离（底座已有 subagent 继承机制可依托）。

**8.** 远程控制对代码会话从「过滤掉」改为「按 `SessionDescriptor.permissions` 声明可见性」的通道化设计（审查 #11 的终态解法）。

## 6. 路线与依赖

```
阶段一 2（能力档案）── 安全修复的根治，最先做；方案 D 已预留退役点
阶段一 1（SessionKind）─ 能力档案的解析入口正是 kind，同 PR 或紧随其后
阶段二 3（CodeView 上提）─ 阶段二其余项的地基
阶段二 4 → 5/6 ── 依序推进
阶段三 ── 独立评估，不阻塞
```

- fork 侧改动仅限能力档案设计 §2.1 已列范围（catalogue/load_skill 档案化），其余全在 app 侧，符合 fork 边界公约；
- 每阶段遵守：行为变更同步测试与文档、i18n 三语、fork-distinct 变更更新 fork-modifications.md + 指纹并跑 `fork-guard.sh --fast`、改动后跑 `architecture-guard.py`；
- plain/ACP 既有行为零变化需 golden 测试锁定（能力档案设计 §7 已列）。
