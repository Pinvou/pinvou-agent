# 底座替换评估报告：DeepSeek-TUI → Kimi Code

> 评估日期 2026-05-31 · 范围聚焦 tools / system prompt / pinvou3 踩坑与 fork commit
> 结论：**不推荐替换**（低可行性 / 高代价 / 低收益）。详见 §5。

---

## 0. 必须先澄清：链接指向两个"同名异体"

`MoonshotAI` 名下存在两套实现，命名高度混淆，直接影响结论：

| 仓库 | 语言 | 形态 | 信号 |
|---|---|---|---|
| **`MoonshotAI/kimi-code`**（评估链接） | **TypeScript 97.6%** | pnpm monorepo（`apps/` `packages/` `plugins/`），单二进制、无需 Node | 1.5k star / 131 fork / 121 commits，"The Starting Point for Next-Gen Agents" |
| `MoonshotAI/kimi-cli` | **Python 3.12+** | `src/kimi_cli/`，Typer + uv + pytest | 文档站 `moonshotai.github.io/kimi-cli` 完整，工具/agent 机制最详尽 |
| `MoonshotAI/kimi-agent-sdk` | — | kimi-cli 的编程接口封装 | — |

两者 README 第一句都是 "Kimi Code CLI is an AI coding agent that runs in your terminal"。
**kimi-code(TS) 是新的下一代重写，kimi-cli(Python) 是更成熟、文档更全的现存实现。** 本报告凡引用 agent/skills/tools 机制细节均来自 kimi-cli 文档站（TS 版尚未公开同等文档，但产品理念一致：MCP 原生 + coder/explore/plan 子代理 + skills 注入）。

> 这个分叉本身是第一个风险点：要替换得先确定锁定哪个；TS 版文档稀薄、Python 版与评估链接不是同一仓库。

---

## 1. 最硬约束：语言/运行时与架构边界

pinvou3 宪法是「**pinvou-platform 是编排层，DeepSeek-TUI 是主体**」，整条集成链是 **Rust**：

```
pinvou3-app (Tauri 2.0 + Rust) ──EngineHandle wrapper──> DeepSeek-TUI (Rust crates)
   ├─ bridge/mod.rs（engine 池、session 隔离、事件转发）
   ├─ build.rs（deny hook 内容哈希校验）
   ├─ super_permission.rs（per-turn reminder 注入）
   └─ install_prompt_overrides()（OnceLock 注入底座 prompt）
```

| 维度 | DeepSeek-TUI（现底座） | kimi-code(TS) | kimi-cli(Python) |
|---|---|---|---|
| 语言 | **Rust** | TypeScript | Python |
| 集成方式 | crate 级 in-process（`EngineHandle`） | 跨语言（需 Node sidecar/IPC） | 跨语言（需 Python sidecar/IPC） |
| 与 Tauri 同进程 | ✅ 是 | ❌ 必须子进程/RPC | ❌ 必须子进程/RPC |

**根本性障碍**：DeepSeek-TUI 作为 Rust crate 与 Tauri 后端**同进程**，pinvou3 直接拿到 `EngineHandle`、共享内存里的 `EngineConfig`、`OnceLock` override、session 池。换成 TS/Python 底座意味着：

- 整个 `bridge/`、`engine_pool.rs`、事件转发、session 隔离要**推倒重做**为跨进程 RPC；
- per-turn `<system-reminder>` 注入、prompt override、deny hook 哈希校验等机制失去 in-process 抓手；
- 多 session 并发架构（engine 池 + per-session instructions 隔离，已 GUI 冒烟通过）需重新设计进程模型。

> 一句话：这不是"换底座"，是"换语言栈 + 重写整个编排层与 IPC"。CLAUDE.md「复用优先 / 不重复造轮子」恰恰指向反面——现有 Rust 集成是最大的已沉淀资产。

---

## 2. Tools 对比

### 2.1 DeepSeek-TUI（现状）
- **~48 个内置工具**，Rust trait `ToolSpec`（async_trait）+ JSON Schema 输入，`ToolRegistry`（HashMap + Arc）。
- 调用协议：**Claude/Anthropic `tool_use` / `tool_result` content block 风格**（非 OpenAI function_calling），见 `crates/tui/src/models.rs:74-119`。
- 关键特性：prefix-cache 稳定（按名排序 + `OnceLock` memoize）、5 层名称解析容错（`ReadFile`→`read_file`，对抗 Qwen3.6 截断）、大输出路由到 workshop 变量、`ApprovalRequirement` + `SandboxPolicy` + Hook `deny` 三层权限。
- pinvou3 核心依赖：`exec_shell` / `request_user_input` / `read|write|edit_file` / `list_dir|file_search|grep_files` / `git_*`；并有 fork 专用 `pinvou3_hidden_tool_blocklist_check` + `defer_loading` 隐藏工具。

### 2.2 Kimi（kimi-cli 文档）
- 内置工具约 16 个：`agent / shell / file / web / todo / background / dmail / think / plan`，YAML 里以 `"kimi_cli.tools.shell:Shell"` 模块:类名 分配，支持 `exclude_tools`。
- **MCP 原生**（`fastmcp` 加载，`/mcp-config` 对话式添加）——卖点，但 DeepSeek-TUI 已有 MCP client。
- 子代理 `coder/explore/plan` 三种、禁止嵌套、隔离上下文。
- approval 机制覆盖 shell/写文件/MCP。

### 2.3 差距评估
| 能力 | 现底座 | Kimi 是否更强 | 结论 |
|---|---|---|---|
| 读写/shell/搜索/git | ✅ 全有且更细 | ❌ 更少（16 vs 48） | 现底座更全 |
| MCP | ✅ 有 | ≈ 同等 | 平 |
| 子代理 | ✅ 有；但 pinvou3 已**定论 fan-out 在本地 Qwen3.6 废弃** | coder/explore/plan 设计更清晰 | Kimi 更干净，但 pinvou3 不用多 agent，价值有限 |
| 弱模型容错/prefix-cache | ✅ 专为 Qwen3.6 做 5 层解析 + cache 稳定 | 未见同等工程 | **现底座为本地小模型量身打磨，Kimi 没有** |
| 调用协议 | Claude content-block | 未公开明确（推测 function_calling） | 迁移需重映射，vLLM 模板适配重做 |

**关键点**：现底座工具层**已被 pinvou3 针对 Qwen3.6/vLLM 反复调优**（截断容错、64KB 硬上限防 SSE timeout、truncated_args_hint、bing 实体解码、fake-ip CIDR 信任……）。Kimi 为 Kimi-K2.5 这类强云端模型设计，这些"弱模型/本地 vLLM 适配"积累**全部归零**。

---

## 3. System Prompt 对比

### 3.1 DeepSeek-TUI（现状）
- base.md ~28KB / 297 行，宪法式分层（Constitution/Statutes/Regulations/Evidence，Tier 1-9）。
- 组装遵循 **Volatile-Content-Last** 不变量（静态可缓存段在前，env/instructions/memory 在后），prefix-cache 友好。
- **已建成 override hook 工程**（`set_base_prompt_override` 等 3 个 OnceLock，上游 PR #2356 OPEN）：app 层 `install_prompt_overrides()` 注入 pinvou3 品牌版，submodule base.md 已回退上游 0 diff。
- **动态状态走 per-turn `<system-reminder>`**（超级权限/sudo 开关），静态 prompt 刷不动这条坑已用 reminder 解决。
- 正在做 prompt 减肥（38.4K→20.1K，-48%）。

### 3.2 Kimi
- agent 用 **YAML 定义**（`version/name/system_prompt_path/system_prompt_args/tools/exclude_tools/subagents/extend`）。
- system prompt 是 **Markdown 模板 + `${VAR}` 变量 + Jinja2 `{% include %}`**：`${KIMI_NOW}` `${KIMI_WORK_DIR}` `${KIMI_WORK_DIR_LS}` `${KIMI_AGENTS_MD}` `${KIMI_SKILLS}`。
- skills 发现路径**与 Claude/Codex 兼容**（`~/.kimi/skills` `~/.claude/skills` `~/.agents/skills` 等），启动时把 name/path/description 注入 prompt——**与 pinvou3 现用 SkillRegistry 理念一致**。

### 3.3 差距评估
- Kimi 的 **YAML agent + Jinja 模板 + `${VAR}`** 比 DeepSeek-TUI 的"编译时常量 + OnceLock override"**更适合外部化定制**——这是 Kimi 唯一对 pinvou3 有实质吸引力的点。pinvou3 当前改 prompt 还得动 bundle 文件（阶段 2 遗留缺点），Kimi 模板天生支持。
- 但 pinvou3 **已用 override hook + per-session instructions 把这件事做到能用**，且解决了静态 prompt 刷不动、并发地雷等坑。Kimi 模板是否原生支持 **per-turn 动态注入**（sudo 状态实时刷新）未见文档证据——这条核心坑（`dynamic_state_per_turn_injection`）在 Kimi 上**很可能要重踩**。

---

## 4. pinvou3 的坑与 fork commit：迁移 = 资产清零

fork drift ~2200 行、~40 文件、~38 个活跃 patch，由 `fork-guard.sh`（29 条指纹）+ ~15 个 `forkguard_*` 回归测试守护。**几乎全是"为本地 Qwen3.6 + vLLM + GUI 场景"打的补丁**，换底座=全部作废重做：

| Fork patch 组 | 解决的坑 | 换 Kimi 后 |
|---|---|---|
| subagent steps/elapsed 上限 + 截断容错 + 继承父 model（#1/#2/#4/#7） | 弱模型死磕、Qwen3.6 截断 agent_id、硬编码 deepseek-v4-flash | Kimi 子代理另一套，全部重写 |
| 联网工具 bing 实体解码 + fake-ip CIDR 信任（#10/#13/#18） | Clash/TUN fake-ip 被 SSRF 误杀、bing 恒返 0 | TS/Python 网络栈重写 |
| file 64KB 硬上限 + truncated_args_hint（#14/#15） | 大产物 >240s → SSE timeout 流截断 | 重新发现并修复同类坑 |
| SSE idle/open timeout 加长 + recoverable 分流（#20/#21/#22） | 本地 vLLM 首 token 慢、瞬态错误被当 turn 结束 | TS/Python SSE 客户端重写 |
| subagent/timeout 调大（#16，120→300s） | 本地 vLLM 慢推理 | 重新调参 |
| skills union + InstructionSource::Inline + base-prompt-override | skills 路径泄漏、并发地雷、prompt drift | Kimi YAML/Jinja 另一范式 |

**已踩并已解决的"架构级"坑**（换底座会复发）：
1. **Fork patch 合并静默丢失**——任何 fork 都有，守护工具要重写。
2. **重建还原实时气泡**——pinvou3-app 前端 + 事件路由耦合，与底座语言无关，但事件协议要重对接。
3. **多 session 并发 / 全局 instructions 并发地雷**——现方案是 in-process engine 池 + per-session 文件隔离；跨进程 Kimi 底座要重新设计进程/会话模型，**最重的重写**。
4. **动态状态 per-turn 注入 + 关闭态 sudo 硬拦**——依赖 in-process `build_send_message_op` 插 reminder + build.rs 哈希校验 deny hook；跨进程底座要找新注入点，可能根本没有对等机制。

**已合入上游的 12 个 PR**（#2245/#2311/#2313/#2354/#2355 等）——pinvou3 反哺 DeepSeek-TUI 生态的沉淀，换底座后**对 Kimi 一文不值**，且失去"修上游 bug 还能 PR 回去"的协作通路。

---

## 5. 结论与建议

### 5.1 可行性评级：**不推荐**（低可行性 / 高代价 / 低收益）

| 评估项 | 结论 |
|---|---|
| 技术可行性 | 理论可行，但等于**换语言栈(Rust→TS/Python) + 重写整个 Rust 编排层与 IPC**，不是"换底座" |
| 工作量 | 极大。in-process EngineHandle 集成、engine 池、session 隔离、prompt override、per-turn 注入、deny hook、fork-guard 全部重做 |
| 资产损失 | ~2200 行 fork patch + ~38 个针对 Qwen3.6/vLLM 的调优 + 12 个上游 PR 通路 全部清零 |
| 收益 | 唯一实质收益：Kimi 的 YAML agent + Jinja prompt 模板**更易外部化定制**；MCP/子代理/skills 现底座**都已有** |
| 风险 | Kimi 为强云端模型(K2.5)设计，**缺少 pinvou3 已做的弱模型/本地 vLLM 适配**；且 kimi-code(TS) 文档稀薄，与文档全的 kimi-cli(Python) 不是同一仓库 |

### 5.2 符合项目宪法的替代路径

CLAUDE.md 明确「复用优先 / 不重复造轮子 / 扩展按场景选层」。Kimi 真正值得借鉴的只有 **prompt 模板化**，完全可在**不换底座**前提下吸收：

1. **借鉴而非替换**：把 Kimi 的 `${VAR}` + Jinja `{% include %}` 模板思路落到现有 override hook 上（阶段 3：让 `set_base_prompt_override` 从外部模板文件读取），即可拿到"易外部化定制"收益，代价是几十行 Rust。正好接上 process.md 的 **base-prompt-override 阶段 3** 和 **prompt 减肥二期(C 类)** 待办。
2. **MCP 已有**：需要 Kimi 那种工具扩展，直接用 DeepSeek-TUI 的 MCP client 接外部 server。
3. **若看中 Kimi 模型能力**：走模型层而非底座层——OpenAI 兼容端点换 Kimi 模型即可，不动 Rust 底座（但与"只用本地 GB10+Qwen3.6"约束 2 冲突，需另行决策）。

### 5.3 若仍坚持评估替换，先回答三个问题
- 锁定哪个仓库？kimi-code(TS，文档少) 还是 kimi-cli(Python，文档全)？
- 跨进程 IPC 后，**多 session 并发 + per-turn 动态状态注入**怎么实现？（现架构最深的两条护城河）
- 愿意接受**弱模型适配从零重做**吗？（截断容错/SSE timeout/fake-ip 等坑要重踩）

---

## 6. 一句话总结

Kimi Code 是优秀的、面向强云端模型的 TS/Python agent CLI，但 pinvou3 的全部价值沉淀都在「Rust in-process 集成 + 针对本地 Qwen3.6/vLLM 的 ~2200 行调优」上。替换 = 换栈重写、资产清零，而 Kimi 唯一的实质优势（prompt 模板化）能以几十行成本在现底座上借鉴实现。**建议：不替换，选择性吸收 prompt 模板思路，推进 override 阶段 3。**

---

## 附：信息来源
- https://github.com/MoonshotAI/kimi-code
- https://github.com/MoonshotAI/kimi-cli
- https://moonshotai.github.io/kimi-cli/en/customization/agents.html
- https://moonshotai.github.io/kimi-cli/en/customization/skills.html
- 本地：`docs/fork-policy.md`、`docs/fork-modifications.md`、`process.md`、`scripts/fork-guard.sh`、`DeepSeek-TUI/crates/tui/src/{prompts.rs,models.rs,tools/registry.rs,core/engine.rs}`

> 说明：§2 工具层结论部分基于 kimi-cli(Python) 文档外推，kimi-code(TS) 的真实工具/协议细节文档稀薄，如需定稿建议实地核对该仓库源码。
