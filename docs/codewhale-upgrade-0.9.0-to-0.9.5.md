# CodeWhale 0.9.0 → 0.9.5 底座升级报告

> 日期：2026-08-10
> 状态：CodeWhale 重建和父仓编译适配已完成，公开维护分支 `pinvou3-clean` 与固定标签 `pinvou-v0.9.5-r3` 已发布；`r1`/`r2` 保留为父仓最新主线兼容补齐过程中的不可变候选。

## 1. 结论

本次从官方 `v0.9.5` tag clean re-fork，没有把旧 fork 整包 merge。仍需留在底座生命周期的 Pinvou 差异被收敛为 5 个长期主题：

1. 宿主嵌入与路由边界
2. 工具兼容与命令执行安全
3. 嵌入上下文与技能来源
4. 定时任务与运行生命周期
5. 三省六部编排与完成闸

相对官方 v0.9.5，候选 fork 为 45 文件、`+1743/-263`；父仓代码层只需适配 `EngineConfig` 字段、窄 Fleet roster/worker 宿主入口和重算 lockfile。Pinvou 工具白名单继续在 app 层维护，直接复用 CodeWhale 原生 `allowed_tools`；工具商店和会话开关继续通过动态 `disallowed_tools` 收窄，不重建第二套底座策略。

## 2. 版本基线

| 版本 | commit | 日期 |
|---|---|---:|
| 0.9.0 | `d167c07c96282411956ea7f35ddb8227afa1402f` | 2026-07-16 |
| 0.9.1 | `d9fdee8aec469915cfdc07ab40aba5c40e9e9de4` | 2026-07-21 |
| 0.9.2 | `2778fd38efc749fc859ef47c088ea32647d4f28b` | 2026-07-30 |
| 0.9.3 | `c98648b1c0f2a82ddaf7d2d82f8212e065db9b65` | 2026-07-31 |
| 0.9.4 | `c20386d29c7a26802ae1976134b18888dbdabfae` | 2026-08-07 |
| 0.9.5 | `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` | 2026-08-08 |

0.9.0 到 0.9.5 共 2181 个提交、1299 个文件变化，约 `+512K/-127K`；其中包含测试、文档、网站和生成物，不能直接当作运行时代码规模。0.9.4 到 0.9.5 只有一天，但仍有 131 个提交、294 个文件变化，属于值得同步的正式运行时版本，而不是纯版本号更新。

## 3. 0.9.0 到 0.9.5 最大变化

### 3.1 工具面统一为 canonical action 家族

模型可见面从许多独立工具名转向 `Bash`、`File`、`Run` 等工具家族及 action；工具发现、权限、审批、sandbox、历史回放和预算围绕同一 canonical 身份组织。`tool_search` 和 MCP Registry 提供渐进披露，避免把所有工具 schema 永久塞进首轮上下文。

对 Pinvou 的影响：

- app 的白名单必须声明 canonical family，而不是继续维护旧工具别名。
- `allowed_tools` 同时约束目录、搜索和 dispatch；`disallowed_tools` 只负责动态进一步收窄，deny 优先。
- MCP 工具仍按 `mcp_*` namespace 进入目录，工具商店不受“统一 action 家族”破坏。
- 文件写入统一由 `File` action 承担，不恢复已退役的独立追加文件工具。

### 3.2 Fleet/SubAgent 从工具调用演进为耐久运行实体

角色、roster、任务图、并发/预算、checkpoint、handoff、receipt、终态和恢复逐步成为底座能力。v0.9.5 又统一 built-in dispatch posture，并移除会在生产性工具轮尚未结束时提前中断的隐藏 continuation backstop。

对 Pinvou 的影响：

- 不再在 app 重写通用 Agent 生命周期；优先消费 CodeWhale 的 route、task、receipt 和 terminal facts。
- 三省六部 fork 只保留业务特有的角色工具范围、最大步数、schema/file 产物和完成闸。
- “模型声称完成”不能替代结构化数据或文件真实落盘；完成条件必须由底座生命周期内的结果校验保证。

### 3.3 Runtime API、Session tree、Task 与 Automation 耐久化

0.9.1 起的 Runtime API 在后续版本补齐 goal、memory、MCP server、Skill lifecycle、Fleet receipt；0.9.5 增加 append-only session tree、`/tree`、`/branch`、`/fork`、`/resume` 和 `/rc` remote control。Task/Automation 同时强化 thread/turn 关联、恢复、misfire 和 no-overlap。

对 Pinvou 的影响：

- 定时任务复用底座调度和耐久状态，不在 Tauri 再复制一套状态机。
- Pinvou 负责业务工作区、展示、通知与产物；底座负责 run/thread/turn 的权威事实。
- 未来移动端控制、团队协作和跨设备接力应尽量建立在 Runtime API/receipt 上，减少私有旁路协议。

### 3.4 路由与预算成为可审计运行事实

Provider/model inventory、Auto 模型选择、route limits、输出上限、reasoning、请求预览和 resolved route receipt 逐步完善。v0.9.5 支持 `model = "auto"`，并让未知模型上下文上限显式失败，而不是静默猜测。

对 Pinvou 的影响：

- UI、token usage、compaction 和故障归因使用 resolved route，不根据用户输入的 model alias 猜测。
- 自定义 OpenAI-compatible 模型的上下文/输出限制应由 SavedModel 或 probe 明确声明。
- Pinvou 保留 embedding host 的显式 route limits API，但不复制上游模型目录和 Auto 路由器。

### 3.5 Prompt、Skills、Plugin、Memory 强调单一权威

新会话 prompt 更小，项目上下文按需披露；Skill 有 catalog、disabled 和生命周期 API；Plugin/MCP 成为独立能力的正式扩展面；Memory 支持修改、退役和审计。

对 Pinvou 的影响：

- 嵌入模式必须关闭 ambient project/repo authority，避免宿主指令与用户目录文件静默混合。
- Skill 只从 app 显式组合根进入模型上下文；disabled Skill 在列表和加载路径都必须消失。
- Pinvou Memory 与 CodeWhale native Memory 不能同时无边界默认注入；正式整合前应选择唯一来源并记录 provenance。
- 新业务能力优先 Skill、MCP、connector 或 plugin，而不是继续扩底座 fork。

### 3.6 v0.9.5 的重点：单一 runtime 与运行可达性

v0.9.5 把 CLI/TUI 合并为一个编译 runtime，`codewhale` 与 `codew` 指向同一字节；依赖图也随 workspace crate 边界重整。它同时改善后台工作指示、完整错误查看、OAuth 新凭据即时采用、MCP registry 后台刷新，以及 productive turn/goal continuation 的可达性。

对 Pinvou 的直接价值：

- library/runtime 边界更正式，旧的全量 bin facade 可以缩减为最小宿主公开面。
- lockfile 依赖发生明显重排，但 Pinvou 无需新增直接依赖。
- 去掉隐藏 continuation ceiling 对长工具链、三省六部和定时任务更安全；仍保留用户显式预算和真正 stuck-loop 防线。

## 4. 本次五主题实现

| 主题 | commit | 主要职责 |
|---|---|---|
| T1 宿主嵌入与路由边界 | `331cb1594` | 最小 library 公开面、窄 Fleet roster/worker API、opaque route、显式 limits、runtime host API |
| T2 工具兼容与命令执行安全 | `595adce47` | extra tools、动态禁用、File 上限、多行危险命令 fail-closed |
| T3 嵌入上下文与技能来源 | `5a9f52941` | static composer 密封、Skill 单根/disabled、Permissions 100 KiB 窄例外、Working Set 隔离 |
| T4 定时任务与运行生命周期 | `fc84f7d3e` | model/conversation、历史 schema、thread/turn、misfire/no-overlap、终态清理 |
| T5 三省六部编排与完成闸 | `3782a78d4` | role/tool/steps、产物级 write claim、schema/file 产出、取消、权威失败终态 |

主题边界、文件与指纹详见 `docs/fork-modifications.md`。

## 5. Pinvou 父仓兼容迁移

- submodule 指向本地 v0.9.5 五主题候选。
- `Cargo.lock` 对齐 v0.9.5 workspace crate 和依赖图。
- `EngineConfig` 删除已不存在的旧 `hidden_tools` 字段引用，显式透传新增 `subagent_state_root`。
- 会话工具隐藏继续走 `shape_disallowed_tools`，产品语义不变。
- app 工具白名单继续使用 `allowed_tool_names()` 同源注入 Engine/turn。
- 三省六部把角色登记的具体产物文件转换为 v0.9.5 有界 write claim，工作区外路径继续拒绝。
- 没有扩大 request-user-input、MCP 工具商店、定时任务或三省六部的产品权限。

## 6. 验证结果

### CodeWhale

- `cargo fmt --all -- --check`：通过。
- `cargo check -p codewhale-tui --lib --locked`：通过。
- `cargo test -p codewhale-tui --lib --locked forkguard_ -- --test-threads=1`：18 passed / 0 failed。

### Pinvou 父仓

- `cargo fmt --all -- --check`：通过。
- `cargo check --locked`：通过。
- `cargo test --locked --lib -- --test-threads=1`：1026 passed / 0 failed / 12 ignored；ignored 项依赖真实模型、外部工具或专用 fixture。
- `./scripts/fork-guard.sh`：CodeWhale 18 passed；pinvou3-app 16 passed。
- `cargo build --locked --no-default-features --features local-embed --bin pinvou3-tauri`：实际桌面二进制链接通过。
- `python3 scripts/architecture-guard.py`：通过，无新增架构债务。
- `npm test`、`npm run lint:ui`、`npm run build:ui`、`npm run build:web`：全部通过；仅有既有非 module script 和大 chunk warning。

未运行真实云模型、外部 API 凭据和各硬件部署的在线推理，因此“功能未丢失”最终仍需用户做 GUI、MCP/OAuth、定时任务和三省六部端到端签收。

## 7. 风险与回滚

- CodeWhale library 公开部分模块会产生 19 条 `private_interfaces` warning；不影响编译和运行。进一步封装成更窄 wrapper 会显著扩大本次改动，故暂不处理。
- `Cargo.lock` diff 较大，原因是 v0.9.5 workspace crate 拆分与依赖裁剪；应依靠 locked build 和父仓全量 lib tests 验证，而不是手工缩小 lockfile。
- 升级前 `pinvou3-clean` 已双重备份：tag `pinvou-v0.9.0-r4` 和 branch `backup/pinvou3-clean-v0.9.0-r4` 均指向 `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624`；需要回滚时恢复该 commit 并同步父仓 gitlink。

## 8. 给 Pinvou 能力演进的启示

1. **运行事实归底座**：route、usage、task、agent、receipt 和 terminal state 由 CodeWhale 生成，Pinvou UI 只消费，不再猜测。
2. **产品策略留 app**：工具白名单、连接器开关、工作区、页面和产物规则不进入通用底座。
3. **扩展优先 Skill/MCP/plugin**：只有需要参与 Engine/Task/SubAgent 原子生命周期的语义才进入 fork。
4. **三省六部保持独立主题**：它的完成闸与普通 Fleet 能力分开维护，避免业务协议污染通用宿主接口。
5. **耐久工作流围绕 receipt 设计**：定时任务、团队接力、后台运行和远程控制统一依赖可恢复、可审计终态。
6. **用契约测试应对快速上游**：重点锁住 canonical tools、route limits、Skill 来源、Automation lifecycle、structured output 和文件完成闸。

## 9. 当前交付状态

- CodeWhale 分支：`Pinvou/CodeWhale:pinvou3-clean`
- CodeWhale HEAD：`3782a78d4e11d1fb65042cf9c82231b9d644c20a`
- 父仓分支：`upgrade/codewhale-v0.9.5`
- 远端状态：`pinvou3-clean` 与 `pinvou-v0.9.5-r3` 已发布；`r1`/`r2` 保留为不可变候选，旧 v0.9.0 分支已双重备份
- 下一步：通过父仓升级 PR 合入 gitlink、适配代码和维护登记。
