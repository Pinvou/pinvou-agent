# 会话能力档案（Session Capability Profile）设计

> 关联文档：`docs/code-native-agent-安全审查问题清单.md` 问题 #4 / #5 / #9。
> 状态：设计提案，待评审。建议独立于安全急件修复 PR 推进。

## 1. 背景与问题

原生代码会话（品悟）引入了「按会话模式隔离能力」的诉求：代码会话与普通聊天会话应有各自的工具 / 技能 / 连接器开关。当前实现把能力边界拆存在两套机制里：

| 通道 | 机制 | 粒度 |
|---|---|---|
| 连接器工具（`mcp_pinvou3_*`） | 每会话 `disallowed_tools`（spawn 时整形下发） | ✅ 按会话 |
| 内置工具（present_artifact 等） | 同上 | ✅ 按会话 |
| skill catalogue（`## Skills` 提示词区块） | 进程级全局 `DISABLED_SKILLS`（fork 私有，`CodeWhale/crates/tui/src/skills/mod.rs:876`） | ❌ 全局 |
| `load_skill` 工具 | 查同一个全局集合 | ❌ 全局 |

全局态不知道「当前是哪个会话」，由此产生一组结构性问题（详见审查清单）：

- **假开关**（#4）：code scope 的 skill 开关落盘成功但无人消费，UI 显示已关闭、实际生效为零；
- **companion skills 绕过**（#5）：code scope 默认全禁连接器，其 companion skills 仍经 catalogue / `load_skill` 触达模型；
- **read_file 侧路**：catalogue 把 skill 的磁盘路径明文印进提示词，即使禁用 `load_skill` 工具，模型仍可直接 `read_file` 读取 SKILL.md——「不列出」是唯一真正的封口点，而全局态做不到按会话不列出。

这些是同一架构病的症状：**能力边界存在进程全局态里**。只修症状（禁工具、UI 只读化）无法根治。

## 2. 设计：会话能力档案

仿照 `SessionRoots { execution, ledger }` 对「两个根」的收敛，在会话 spawn 时**一次性解析出该会话的完整能力档案**，随 `EngineConfig` 下发；之后所有能力通道只查本会话档案：

```rust
SessionCapabilities {
    roots:  SessionRoots { execution, ledger },  // 已有
    tools:  HashSet<ToolName>,                    // 已有（disallowed_tools 整形）
    skills: SkillPolicy,                          // ← 新增：本设计核心
    rules:  Vec<PathBuf>,                         // 已有雏形（AGENTS.md 注入）
}
```

`SkillPolicy` 由 app 侧在 `build_engine_config` 时按 scope 解析：

- plain 会话：`已装 − plain 禁用集`（现状语义不变）
- 代码会话：`已装 − code 禁用集 − code 禁用连接器的 companion skills`

语义采用**替换**而非减法（评审已敲定）：code scope 的 skill 禁用集独立于 plain，与连接器工具现有的 scope 语义（`shape_disallowed_tools` 替换式）对齐——「plain 关、code 开」成立，code 侧开关不反向污染 plain。安全默认由「code 未初始化时连接器（含 companion skills）全禁」承担，不依赖 plain 打底。code scope 中普通 skill 的初始默认：未操作过即为启用（`skill:` 条目只在用户显式关闭时落盘）。

### 2.1 fork 侧改动（均为 fork 私有代码，无上游同步负担）

1. **catalogue 按档案过滤**：`render_skills_block` 接受会话级禁用集（经 EngineConfig / session context 携带），会话未列出的 skill 不渲染进 `## Skills`。模型不知路径，`read_file` 侧路自然封死——**这是封口点**。
2. **`load_skill` 按档案判定**：`ToolContext` 携带会话上下文，执行时查会话档案而非全局 RwLock（纵深防御，防路径被猜测）。
3. **全局 `DISABLED_SKILLS` 退役为默认值**：无档案的会话（ACP、既有链路）维持现状语义，兼容不破坏。有档案的会话**以会话禁用集为准（替换全局，非并集）**——若取并集，plain 禁用集会穿透回代码会话，「plain 关、code 开」不成立（§2 替换语义）。

### 2.2 app 侧改动

1. `bridge.build_engine_config` 按 scope 解析 `SkillPolicy`（复用 `ConnectorScope` 的 scope 判定）。
2. `refresh_disabled_skills` 由「推全局」改为「按会话刷新」（spawn 注入 + 热更）。
3. 代码页 skill 开关真正生效后，恢复为可交互（过渡期为只读，见 §4）。

## 3. 示例

装了「周报写作」skill（weekly-report），用户在代码页将其关闭：

```
开普通会话 A：SessionCapabilities(A).skills = 含 weekly-report
  → A 的提示词 ## Skills 列出 weekly-report（含路径），load_skill 可用

开代码会话 B：SessionCapabilities(B).skills = 不含 weekly-report
  → B 的提示词 ## Skills 无此行；load_skill("weekly-report") 按 B 档案拒绝；
    read_file 因路径未出现在 B 的提示词而无从得知
```

恶意仓库 prompt injection 引导读取 skill 文件时：B 中两条通道（load_skill / read_file）均不可达；A 中行为不受影响。代码页开关终于只影响代码会话。

## 4. 与过渡方案的关系

安全急件修复 PR（`fix/code-native-agent-review-issues`）中，skill 维度采用过渡方案 D：代码会话禁用 `load_skill` 工具 + 代码页 skill 分组只读/隐藏。该方案**提高门槛但不封口**（catalogue 仍印路径，read_file 可读），文档需如实登记残留。

本设计落地后，过渡方案的两个组件自然退役：代码页 skill 开关恢复可交互，`load_skill` 禁用由档案判定取代。

## 5. 第二阶段（纵深防御，独立 PR）

catalogue 过滤后的理论残留：`~/.pinvou3/bundle/skills/` 路径形态可预测，被 prompt injection 引导的模型可**猜路径** `read_file`。补法：文件工具工作区围栏——代码会话的 `read_file` 等文件工具默认限制在执行根（项目目录）内，越界绝对路径需显式授权。该层有独立价值（防项目外文件被乱读），工作量独立，不阻塞本设计。

## 6. 收益

- 新增能力通道（新工具类型、新 skill 来源）只需接能力档案解析一处；
- 新增会话类型（如将来的远程代码会话）只需声明 scope，行为自动正确；
- 消除「两套真相」（全局态 vs 每会话配置）整类 bug；
- 顺带完整兑现审阅者「代码专用工具 profile」的诉求（审查清单 #9）。

## 7. 实施约束（项目公约）

- fork-distinct 行为变更：同一 PR 更新 `docs/fork-modifications.md` + 指纹 + 行为测试，跑 `./scripts/fork-guard.sh --fast`；
- prefix-cache 影响需评估：按会话不同的 skills 区块会使不同 scope 会话的提示词前缀分叉（现全局方案下 toggle 也有一次 miss，差异是按会话稳定后各自可缓存）；
- fork 注释中「全局对齐连接器 chip」的设计理由（skills/mod.rs:870-875）需正式推翻并改写；
- 普通会话与 ACP 会话行为零变化，需 golden 测试锁定 plain scope 的提示词渲染结果。
