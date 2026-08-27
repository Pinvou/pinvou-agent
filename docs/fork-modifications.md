# CodeWhale Fork 修改清单

> 本文是 Pinvou 对 CodeWhale fork 的单一现状清单。
> 基线、主题边界、守护指纹和同步结论以本文与 `docs/fork-policy.md` 为准。
> English: [`docs/fork-modifications.en.md`](fork-modifications.en.md)

## 0. 当前状态（2026-09-01 · v0.9.5 r13 基线，父仓 gitlink 由 PR #370 接入）

| 项 | 当前值 |
|---|---|
| 上游基线 | tag `v0.9.5`，commit `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` |
| 公开维护分支 | `Pinvou/CodeWhale:pinvou3-clean`，r13 head `f853f8f1`（r12 + #32 GAIA 评测隔离扩展） |
| 已合并修复 | 既有 `#9`、`#11`、`#12`、`#13`、`#15`、`#16`、`#17`、`#19`，以及 r11 的 `#18`、`#21`、`#22`、`#25`、`#26`、`#27`、`#29`、`#30`，r12 的 `#33`、`#35`，r13 的 `#32` 均已合并 |
| 发布状态 | `pinvou3-clean` 与不可变 tag `pinvou-v0.9.5-r13` 均指向 `f853f8f1566c57e6be40d5439a222a932aa79ef5`；`r1` 至 `r13` 保持不可变；父仓 gitlink 由 PR #370 对齐 r13 |
| 旧基线备份 | tag `pinvou-v0.9.0-r4` + branch `backup/pinvou3-clean-v0.9.0-r4`，均指向 `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` |
| 组织方式 | 从 `v0.9.5` clean re-fork 的 4 个当前长期主题；专用编排主题由 PR #13 整体撤销 |
| drift | r13 基线合计 `110 files, +10895/-1195`（净增 9700 行）；r12→r13 为 `6 files, +1088/-1` |
| 守护 | r13 为 63 条 CodeWhale `forkguard_*` 行为测试（含 6 条 GAIA 评测隔离测试）+ 通用工具/路由兼容回归 + 父仓指纹/行为测试 |
| 父仓适配 | gitlink、`Cargo.lock`、`EngineConfig` v0.9.5 字段适配、拒绝编辑的终态/权威历史对账、压缩后用量即时刷新与持久化回填、严格直连模型大小写桥接回归、搜索源设置页引导文案，以及 operator-owned 未登记云端模型（自定义 openai-compatible 端点）的显式输出路由事实声明与官方端点 fail-closed 守护（承 PR #216） |

### r12 厂商原生搜索与免 key 兜底 Bing 化（已合入底座）

- CodeWhale PR #33（六提交 rebase 后以 `4f612e548` 汇入）：新增 DeepSeek Responses、Model Studio Token Plan（Qwen）、Moonshot/Kimi（K2.6 内建 `$web_search`、K3 官方 Formula 协议、Kimi Code `/search`）、Z.AI/智谱（全球 `search-prime` / 中国 `search_std`）、Xiaomi MiMo 的厂商原生搜索适配。能力按"厂商+模型+官方端点+产品面"四重精确匹配 fail-closed，K3 Formula 独立 180 秒预算与 8 次调用上限；评审发现的端点匹配宽松（整 URL 小写、无限剥尾斜杠）由收官提交 `4f612e548` 引入 `is_exact_url_route` 收紧。指纹锚点：`documented_server_side_web_search_for_route`、`WEB_SEARCH_FORMULA_URI`。
- CodeWhale PR #35（`9c5f4f19` 汇入）：API 后端（Tavily/Bocha/Metaso/Baidu/SearXNG/Volcengine/Sofya）失败后的免 key 链尾由 DuckDuckGo 换成 Bing（实测 DDG 在中国大陆 DNS 污染 + SNI 重置不可达，Bing 全球与国内端点均免 key 可达）；全链失败错误追加 API 后端配置建议；新增 `forkguard_api_provider_chain_tail_is_bing`（forkguard 总数 56→57）。
- r12 父仓配套（PR #375）：gitlink → `9c5f4f19`、设置页搜索源引导文案（i18n 三语）、`prefs/search.rs` 注释勘误与 `bridge.rs` 注入点注释/测试 docstring 同步勘误（底座默认仍为 DuckDuckGo，应用侧默认 Bing 由 bridge 构造 `EngineConfig` 时显式注入）。

### r11 Provider、MCP、steer 与平台边界（已发布）

- CodeWhale PR #18、#21、#22、#25、#26、#27、#29 与 #30 以 `0d89a31be016457c180501417dd2c0f34ce844a6` 汇入公开 r11。严格直连 provider 仅在唯一自有模型行可确认时容忍 wire model 大小写差异；父仓补充 GLM 小写保存值到 canonical `GLM-5.2` 路由的桥接回归。
- 显式 route 输出上限在请求预算层生效；Moonshot 对不兼容工具逐个省略，并对用户发出每轮一次的可见诊断，具名 `tool_choice` 指向被省略工具时明确拒绝。MCP 密钥由宿主 resolver 提供且不写进进程环境；被禁用 server 在 pool、catalog、直接调用、reload、子智能体继承等入口统一表现为不可见。
- `withdraw_steer` 返回 `SteerWithdrawal`，区分撤回、已提交和不存在；Windows Shell 输出使用跨 poll 的增量 UTF-8 解码，依赖更新修复 h2/lru 公告。r11 新增 15 条 `forkguard_*`，总数从 41 增至 56，未增加长期 fork 主题。
- r11 相对 r10 为 `48 files changed, +2242/-292`。其中 provider projection、MCP host policy、steer lifecycle 和跨平台 shell 解码均是可复用底座能力，后续继续以通用设计优先回馈上游。

### r11 steer 撤回 outcome（已发布）

- CodeWhale PR #30 以 `e6bc34769` 合入（维护者 follow-up `69ed3bfbd` 强化契约并纳入 forkguard）：`EngineHandle::withdraw_steer` 返回 `SteerWithdrawal::Retired`（撤回生效、永不注入、恰好一条 `SteerDropped`）/ `NotPending`（已结算或未知——**不是已投递证明**，宿主必须等对账终态事件，不确定时保留输入）。pinvou-agent#308 的 ⚡ 瞬发据此在撤回后决定是否重发，闭合「撤回成功/已注入」不可区分导致的重复投递。另含 `#[must_use]` 防宿主静默忽略返回值。
- 同期维护分支还合入 MCP 禁用工具旁路封闭（`e68a185c2`）与严格直连模型大小写匹配（`0d89a31be`）等修复，归入既有主题，无新增长期 fork 主题。

### r10 固定采样与压缩用量边界（已发布）

- CodeWhale PR #19 以 `feb8761aeda31749f3d54c6e1f8ef460540567a1` 发布。Kimi Code 会员路由仅对精确会员模型名单（`k3`、`k3-256k`、`kimi-for-coding`、`kimi-for-coding-highspeed`）剥离非 1 的固定采样字段；DeepSeek 仅对精确 `deepseek-v4-flash` Responses 路径保留兼容 shim，Chat 方言继续透传官方 0..=2 采样契约。
- K3 与 K3-256K 复用现有会员路由、reasoning dialect 和模型元数据入口；`k3-256k` 固定为 262,144 token 上下文，避免通用名称提示误解析成 256,000，裸 `k3` 仍独占现有 1M entitlement 路径。
- `CompactionCompleted.post_input_tokens` 使用引擎 canonical 估算覆盖压缩后的完整请求输入（system prompt 与合并摘要均包含）。父仓把该值即时写入用量 chip，并作为非 turn 的 `context_snapshot` 持久化，重新进入会话不会回退到压缩前数值。
- r10 相对 r9 增加 `17 files changed, +370/-31`；新增 4 条 `forkguard_*` 回归，总数从 37 增至 41。改动归入既有 T1 路由/宿主事件边界，没有增加长期 fork 主题。

### r9 对话插入与编辑边界（已发布）

- CodeWhale PR #16 以 `8aa5f77d35ac1d00d1f444193543307a7e9b391c` 发布可靠的 steer 生命周期：返回不透明 id，发出 `SteerCommitted` / `SteerDropped`，打断时按显式模式保留或丢弃未提交输入，并在取消路径确定性终止当前轮所属的前台 Shell。
- CodeWhale PR #17 以 `07d183e350ce4a1ed4f91bdfa1875c996e710d2b` 发布权威编辑目标分类。`EditLastTurnTarget` 区分可编辑文本、不支持的最新用户内容和缺失目标；工具结果、内部运行时信封及非权威来源不能充当编辑点，真实但不支持的最新用户内容也不能被跳过后回退到更早文本。
- 编辑预检失败使用稳定的 `edit_last_turn_*` 非恢复错误码，并发送单一 `TurnComplete(Failed)`。父仓据此跳过乐观落盘兜底、在 `chat:done.operation_rejected` 后重新读取权威历史，保证 Tauri 与 Web 都恢复到未修改的耐久会话。
- r9 相对 r8 增加 `18 files changed, +1998/-178`。增加量主要是跨中断 steer 所有权、Shell 终止和历史来源分类的状态/并发回归；这些语义必须位于 Engine 生命周期内，不能由 app 复制。后续以通用宿主 API 形式优先向上游贡献。

### r8 逐轮评测工具安全扩展（已发布）

> CodeWhale PR #15 的候选链 `1eca6103a` + 安全修复 `169c24cc5` + 只读分发与受限面收口 `21e5f661a` + 续轮/Shell 边界修复 `a647ed866` 已 squash 合并为 `d127aed113529dc93754d044b9f352e9746f6b83`；合并提交与已验证候选 head tree 完全一致，并已发布为不可变标签 `pinvou-v0.9.5-r8`。该扩展为嵌入宿主增加进程内逐轮工具安全策略、可信外部路径完全覆盖、最终执行前精确白名单门禁、只读 `File` action 投影与最终分发复检，并封闭排队控制操作、排队续轮/MCP reload、Hook 与日志旁路。受限轮结束后的子智能体完成、后台 Shell 唤醒和编辑重放会锁存到显式新消息安装替代权限；只读 `Bash` 使用 `ShellPolicy::ReadOnly` 的直接 argv 加固路径；受限审计只保留非私有身份字段。r8 发布时父仓 gitlink 与公开校验严格对齐该标签；当前公开基线以上方第 0 节为准，r8 标签保持不可变。

### r13 GAIA 评测隔离扩展（已发布）

- CodeWhale PR #32 已 squash 合并为 `f853f8f1566c57e6be40d5439a222a932aa79ef5`，直接建立在 r12 之上，并发布为不可变标签 `pinvou-v0.9.5-r13`。
- 新增 `benchmark-observability` 与 `benchmark-eval-controls` 两个空的、默认关闭 feature。首字延迟/请求耗时、工具预算后 final-only 以及无歧义只读参数修复仅在父仓 `benchmark-hooks` 显式启用时编译。
- Desktop 默认 feature 仍为 `local-embed`，不会编译上述评测分支；默认构建已单独通过。代理通配符信任、IPv6 fake-IP 特判和其他全局网络策略不在 r13 中。
- 新增 6 条 `forkguard_benchmark_*` 行为测试并登记精确发布指纹；父仓 fork guard 在 r13 公开基线下始终执行这些带 feature 的测试，不再保留未发布候选放行。

### 父仓 gitlink 同步勘误（2026-08-22 更正）

- 早期版本此处曾记载"PR #302 把父仓 gitlink 从 r6 (`3bbf8421`) 一次性 bump 到 r7"。
  事后按 git 历史勘误：该 bump 实际发生在 **PR #285**（commit `95502ac8`，`git ls-tree` 可证：`95502ac8^` 为 `3bbf8421`，`95502ac8` 起即为 `a36e6cd533…`）。
- PR #302 (`feat/plugin-protocol`) 起步于 #285 合并前的旧 main（起步时父仓 gitlink 停在 r6，与当时 `发布状态` 不一致，曾触发 `scripts/verify-public-submodule.sh` 与 `scripts/ci-fork-link-check.sh` 在 PR fast-gate 持续失败），合并时 gitlink 已在 main 上对齐 r7，因此 **#302 未改动 gitlink**（`git diff c75f2fb2^..c75f2fb2 -- CodeWhale` 为空）。
- 后续推进：PR #305 把公开基线推进到已发布的 r8；r7→r8 同步不改 `.gitmodules` 或底座主题组织方式。
- 勘误时点（2026-08-22）现状：父仓 gitlink、`Pinvou/CodeWhale:pinvou3-clean` HEAD 与 `pinvou-v0.9.5-r8` 标签均指向 `d127aed113529dc93754d044b9f352e9746f6b83`；当前基线以上方第 0 节为准。
- 另勘误 #302 的父仓侧改动范围：能力包统一模型（父仓 commit `c75f2fb2`）把开关存储收敛为单一 `disabled_bundles.json`（包 id × 模式禁用集 + `hidden_scopes`），取代原先分开的 `disabled_connectors.json` / `disabled_skills.json` 双文件（读到旧双文件即迁移不删）。

### 本次会话修复（已验证并发布）

- v0.9.5 的 `load_session` 会把无配对 `tool_use` 视为进程崩溃并立即补写失败结果；Pinvou 运行中持久化工具调用后再次读取同一会话时，这一假设并不成立。
- 底座修复已通过 `Pinvou/CodeWhale#11` 合入，公开 commit 为 `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`。
- T1 新增无修复副作用的 `load_session_snapshot` 与显式 `recover_session_for_resume`。Pinvou 的运行时读改写统一使用前者，仅在应用进程启动、任何 Engine 接管会话前执行后者，并把恢复结果原子落盘。
- 前端仅对真正的跨端回合保留 revision 对账门禁；本地 `chat:done` 直接释放下一轮发送，落盘读回异常不得阻塞普通本地对话，跨端未收敛提示按会话去重。
- 本次新增 2 条 CodeWhale `forkguard_*`、2 条父仓 `forkguard_*` 和 Tauri/Web 前端行为回归，分别锁定运行时无副作用读取、显式恢复可观测与幂等、二次 Store 打开安全、启动恢复落盘以及本地完成后连续发送。
- 本节改动已计入上方公开维护分支 head、drift 和固定标签 `pinvou-v0.9.5-r5`；CodeWhale required checks 与父仓自动测试均已通过。

### PR #13 退役发布

- **合并 commit**：`a36e6cd533024cfe5724bae21875aea42b2ed87a`；已通过 `Pinvou/CodeWhale#13` squash 合并并发布为 `pinvou-v0.9.5-r7`。
- 删除专用角色派发字段、结构化提交入口、文件完成闸和对应 TUI 投影，不再让产品协议进入通用 SubAgent 生命周期。
- 保留宿主取消所有运行中子智能体的窄操作，以及通用完成事件的 `failed` 终态；桌面停止/回收仍不会遗留后台子任务。
- 新增 `forkguard_host_bulk_cancel_stops_all_running_children_idempotently`，锁定批量取消和重复取消行为。
- 修复退役后两处通用兼容回归：MCP registry 提示恢复 canonical `Bash(action="run")` / `Web(action="fetch")`，Custom SubAgent allowlist 的旧 action alias 继续解析到已注册的 canonical family。

### 会话 steer 与确定性取消（CodeWhale#16 / pinvou-agent#308，已发布）

- **状态**：CodeWhale#16 已合并、`#30`（`withdraw_steer` 返回 `SteerWithdrawal` outcome）随后续修复合入，公开基线为 `pinvou-v0.9.5-r11`（`0d89a31be`）；本节内容已按约定并入 T1/T2 主题的 commits、公开基线 head 与 drift 登记，本节保留为发布记录。
- **T1（宿主嵌入与路由边界）新增**：
  - 会话 steer（mid-turn 注入）底座原语：`SteerMessage { id, content }` 经 steer channel 入队，`EngineHandle::steer` 入队时生成并返回 opaque `steer_id`；`Event::SteerCommitted` / `Event::SteerDropped` 携带 `steer_id` 供宿主关联排队占位消息，不使用内容哈希（非 ASCII 内容跨语言哈希无法实现一致）。
  - `EngineHandle::cancel_with_mode(reason, CancelMode)`（r10 起）：`InterruptKeepInbox`（⚡ 打断）把未注入 steer 跨轮 park，由下一轮 step 边界注入；`StopDropInbox`（⏹ 停止）在全部 Interrupted 出口统一 settle——`pending_steers` 与 steer channel 残留逐条发 `SteerDropped`。处置模式与 cancel token 在同一次句柄调用内原子发布，宿主没有独立的 cancel 前开关。
  - `Op::SyncSession` 与 `Op::Shutdown` 销毁前清场并逐条发 `SteerDropped`，杜绝跨会话注入；`Drop for Engine` 以 `try_send` best-effort 兜底，覆盖宿主 evict/reclaim 直接丢弃引擎的路径。
  - steer 撤回：`EngineHandle::withdraw_steer`（fire-and-forget）把 id 记入引擎共享撤回集合；所有收集/注入点过滤被撤回 id 并恰好发一条 `SteerDropped`，被撤回 steer 永不注入 transcript；撤回标记跨轮存活，`SyncSession`/`Shutdown` 清场时一并清除。宿主 UI 排队占位的 ✕ 由此在注入前真正生效。
- **T2（工具兼容与命令执行安全）新增**：
  - cancel 杀进程范围收敛：`ShellManager::kill_running_turn_foreground` 只杀本轮前台（`spawned_as_foreground` 且无 `owner_agent`）shell 进程组；用户转后台的任务与子智能体 background shell 不再被任何取消源连带 kill。
- **守护**：8 条新行为测试——`steer_lifecycle_rejects_idle_and_assigns_unique_ids`、`steer_lifecycle_capacity_wait_revalidates_target`、`steer_lifecycle_interrupt_keeps_input_for_next_turn`、`steer_lifecycle_stop_retires_reserved_late_send_once`、`steer_lifecycle_session_rejects_late_reserved_send`、`steer_lifecycle_withdrawal_is_bounded_and_prevents_commit`、`engine_drop_reports_unconsumed_steers_best_effort`（`core/engine/tests.rs`），`kill_running_turn_foreground_scopes_to_this_turns_unowned_foreground_shells`（`tools/shell/tests.rs`）；指纹见 `scripts/fork-guard.sh` T1/T2 steer 条目。
- **上游策略**：该能力按上游中性实现（英文注释、无 Pinvou 私有语境），后续按 §5 规则从 upstream main 建净分支回馈；回馈合入前以本登记为准。

### 软上限评估

净增量高于 1500 行软线，主要保留量来自逐轮工具安全、Automation 持久化、会话恢复、工具兼容和嵌入上下文密封：

- T2 r8 扩展 `+1370/-202`：逐轮权限必须同时覆盖 catalog、最终 dispatch、排队续轮、Hook、审计和只读 Shell/File 投影，并用行为回归锁住权限替换后的异步唤醒边界。
- T4 `+373/-24`：稳定 conversation/thread 关联、Pinvou 历史 schema 兼容、misfire/no-overlap 和终态级联清理必须与 Task/Automation 持久化原子完成。
- T3 `+253/-71`：嵌入宿主的静态指令、ambient context 和 Skill 单根来源必须在模型上下文生成前密封。

本轮不为压数字复制底座状态机到 app。后续减量顺序：T1 通用 embedding route API、T2 通用命令安全、T4 通用 Automation 生命周期；T3 的 Pinvou 产品语义继续留 fork。

## 1. 四个长期 fork 主题

### T1：宿主嵌入与路由边界

- **commits**：`331cb1594688c723d98499d9ca11f05af291b599`、`2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`（`#11`）、`a36e6cd533024cfe5724bae21875aea42b2ed87a`（`#13`）、`8aa5f77d35ac1d00d1f444193543307a7e9b391c`（`#16`）、`07d183e350ce4a1ed4f91bdfa1875c996e710d2b`（`#17`）、`feb8761aeda31749f3d54c6e1f8ef460540567a1`（`#19`）、`485884913308cdf7564bc60da2e416be637083b5`（`#21`）、`04e109af4b4786a0d49fbbeefdd77af15a9f495e`（`#22`）、`e6bc347694ef4229b84919c49fea54fc584377c4` + `69ed3bfbdb314f901d4cf4120f1caaaf0b6aa529`（`#30`，steer 撤回 outcome）、`0d89a31be016457c180501417dd2c0f34ce844a6`（`#18`）。
- **公开规模**：r8 前置规模为 10 文件、`+394/-31`；r9 至 r11 的增量按上节整体登记。
- **核心文件**：`crates/tui/src/lib.rs`、`core/{engine,events,ops}.rs`、`core/engine/{handle,turn_loop}.rs`、`runtime_handoff.rs`、`route_runtime.rs`、`runtime_threads.rs`、`automation_manager.rs`、`session_manager.rs`。
- **内容**：
  - 在 v0.9.5 原生 library target 上只公开 Pinvou 实际使用的模块和宿主类型，不恢复旧的全量 bin facade。
  - 以根级窄重导出公开 `FleetRoster` 与工作区角色目录常量，供嵌入宿主在写入角色文件后装配和热刷新名册；不公开整个 `fleet` 模块。
  - 提供只读持久化 worker 投影，供 live 宿主结合自身进程纪元判断状态；恢复入口仍按 v0.9.5 原语把孤儿 worker 收敛为 interrupted。
  - 提供 opaque resolved route、显式 route limits 和 embedding host route override。
  - 显式 route 输出上限参与请求预算；严格直连 provider 只在唯一自有模型行匹配时允许 wire model 大小写归一，避免把网关或歧义模型静默改写。
  - 保留宿主需要的 runtime thread / Automation 接口和 `EngineConfig` 注入边界。
  - 将无副作用的运行时 session snapshot 与已知进程重启后的显式 tool history recovery 分开，避免嵌入宿主把仍在执行的工具调用误判为崩溃。
  - 提供通用的宿主批量取消操作和失败终态标记，供会话停止与 Engine 回收安全收敛后台子智能体。
  - 为 steer 提供可关联 id、提交/丢弃事件和跨中断 keep-inbox 所有权，停止路径显式丢弃未提交输入，避免消息在 UI 与 Engine 之间静默消失或跨会话泄漏。
  - `withdraw_steer` 返回可区分撤回、已提交和不存在的 `SteerWithdrawal`，宿主无需从竞态错误文本推断结果。
  - `Op::EditLastTurn` 与宿主落盘兜底共用 `edit_last_turn_target`：工具结果与内部运行时信封同样以 `role = "user"` 持久化，裸 role 扫描会把截断落在 tool result 上；真实但不支持的最新用户内容必须拒绝，不能跳到更早文本。拒绝路径发送类型化错误与失败终态，不调用 provider，也不改变历史。
  - 固定采样路由剥离显式非 1 的 `temperature`（否则 400 "only 1 is allowed"）：Kimi Code 会员路由按会员模型名单精确匹配（`k3` / `k3-256k` / `kimi-for-coding` / `kimi-for-coding-highspeed`，Chat 方言 seam）；DeepSeek 侧仅在 Responses 方言对精确 `deepseek-v4-flash` 保留兼容 shim，Chat 方言按官方文档的 0..=2 契约透传（v4-pro 走 Chat 线，实测不受限）。网关与其他模型契约不动。修复 code 页手动压缩在 Kimi Code 路由必现 400。
  - `CompactionCompleted` 事件新增 `post_input_tokens`：压缩完成后完整请求的输入 token 保守估算（复用引擎 canonical 估算，含 system prompt 与压缩摘要），供宿主在压缩完成后立即刷新用量展示；TUI 与 runtime thread 持久化路径不消费。
  - `install_mcp_secret_resolver` 宿主钩子：mcp.json `${...}` 占位符、`env_headers`、`bearer_token_env_var` 解析先查宿主注册的进程内回调，未命中回落进程 env；嵌入宿主（品悟）据此把 MCP secret 承载改为 keyring + 进程内注册表，消除运行时进程 env 写。
- **边界**：不实现 Pinvou 产品工具策略或专用编排完成语义。
- **守护**：`forkguard_embedding_route_limits_preserve_wire_alias`、`forkguard_runtime_session_snapshot_preserves_in_flight_tool_call`、`forkguard_explicit_session_recovery_is_reported_and_idempotent_after_save`、`forkguard_host_bulk_cancel_stops_all_running_children_idempotently`、`forkguard_is_user_turn_prompt_separates_prompts_from_tool_results_and_envelopes`、`forkguard_edit_last_turn_cuts_at_user_prompt_before_tool_results`、`forkguard_edit_last_turn_without_user_prompt_errors_and_sends_nothing`、`forkguard_kimi_code_coding_plan_strips_non_one_temperature`、`forkguard_deepseek_v4_chat_preserves_documented_temperature`、`forkguard_deepseek_v4_flash_responses_drops_non_one_temperature`、`forkguard_compaction_completed_reports_complete_post_input_tokens`、`forkguard_mcp_secret_resolver_supplies_values_without_process_env_writes`，steer 生命周期/Shell 终止回归，以及父仓启动恢复、resolved-route、取消级联、compaction 合约、落盘编辑分类和双端拒绝回滚测试。

### T2：工具兼容与命令执行安全

- **commits**：`595adce47e2d1bcf895d7bfd6426c074eb969324`、`3bbf8421ebdb16bff71f83dac4d42c8fb65f0f02`（`#12`）、`a36e6cd533024cfe5724bae21875aea42b2ed87a`（`#13`）、`d127aed113529dc93754d044b9f352e9746f6b83`（`#15`）、`8aa5f77d35ac1d00d1f444193543307a7e9b391c`（`#16` 的 Shell 取消边界）、`44730dfe596b70f86ae2f928959877a3e3f494e4`（`#27`）、`665b46cd9e67326459223aa662931bd36d726004`（`#29`）、`04e109af4b4786a0d49fbbeefdd77af15a9f495e`（`#22`）、`4831c3797b76485a912b056c76a4cff22f0a2863`（`#25`）、`e68a185c2ba07f327bd8b63bbfea6a70a96f33ea`（`#26`）、`ecfd68acc056b95b06d98312753a712e4c0755db`、`603eeadcdab65d71d62a5ac32b6700207433fe5c`、`eb25a255a92f7385a3fde74f1f44626cdd068125`、`8c243e7ea7094fff189ab12582aea0460b655d06`、`8111f8150bc6b103da685f6abc3f26143b3bb207`、`4f612e548090616f8206154e37c9895404a8998b`（以上六项为 `#33` 厂商原生搜索）、`9c5f4f19b0acbc960889778a5873c7fb038b1378`（`#35` 免 key 链尾 Bing 化）、`f853f8f1566c57e6be40d5439a222a932aa79ef5`（`#32` GAIA 评测隔离扩展）。
- **核心文件**：`core/engine.rs`、`core/engine/tool_setup.rs`、`core/ops.rs`、`tools/file.rs`、`command_safety.rs`、`tools/shell.rs`、`docs/TOOL_SURFACE.md`。
- **内容**：
  - `EngineConfig.extra_tools` 让宿主工具在 Plan、Agent、Yolo 等 turn registry 中一致注册。
  - `SetDisallowedTools` 支持工具商店、知识库和会话策略在不重建 Engine 的情况下动态收窄工具面。
  - 复用 v0.9.5 原生 `allowed_tools` 作为硬白名单入口；Pinvou 名单由 app 构造，底座不维护产品 blocklist。
  - `File` 写入保持 64 KiB 单次内容上限，并在落盘前拒绝超限输入。
  - 多行 Shell 按 segment 检查；破坏性命令在自动批准模式下仍被阻断。
  - Engine 取消路径按 turn id 终止当前轮未被宿主接管的前台 Shell，不依赖工具 future drop；后台/宿主管理的任务保持原有所有权边界。
  - schema 约束的 JSON 容器兼容、工具续轮 provider 角色顺序和已知内部 runtime suffix 展示清理继续沿用 r6 行为。
  - Moonshot 工具 schema 按单个不兼容工具降级并发出一次用户可见诊断；具名选择不得指向已省略工具。宿主 MCP 密钥 resolver 不写进程环境，禁用 server 在 pool、catalog、直接调用、reload 与子智能体继承入口统一不可见。
  - Windows Shell 跨 poll 保留增量解码状态，避免拆分 UTF-8 序列被替换；h2/lru 安全更新不改变公开接口。
  - registry-first 提示只引用 canonical action；Custom SubAgent 的显式旧 action allowlist 通过 alias 映射解析到 canonical family，不扩大实际工具权限。
  - 当前工具面不恢复已退役的独立追加文件工具，也没有改动 `request_user_input`。
  - r8 在 catalog 与最终 dispatch 共用 exact allowlist；显式受限轮次可清空 trusted roots，并禁止 MCP 初始化、控制面 shell、动态工具和子 Agent。控制面限制保持到下一条消息出队，受限排队续轮（goal self-continuation）、编辑重放与 MCP reload 同样拒绝；轮后子智能体完成和后台 Shell 唤醒延迟到显式新消息安装替代权限。Hook 默认关闭；受限审计保留 event 与 tool_name 等非私有身份字段，输入/输出/路径固定脱敏；`None` 保持现有 GUI 行为。
  - 父仓 GAIA 集成在同一逐轮策略上显式启用 read-only dispatch：catalog 改用只读 `File` action schema，规划与最终执行前均再次拒绝写动作；`Bash` 同步投影为 `ShellPolicy::ReadOnly`，复用直接 argv 加固路径，不能只依赖 approval。
- **上游计划**：逐轮权限、可信根覆盖、只读 action 投影和最终 dispatch 门禁是通用嵌入能力。当前 fork 版本已随 r8 发布；后续从最新 `Hmbown/CodeWhale` main 提交独立上游 PR。Pinvou profile 名称与 GAIA 工具名单继续留在 app；上游接收后删除 fork 对应实现和本地指纹。
- **边界**：不包含 Skill 来源、Automation 或产品角色协议。
- **守护**：`forkguard_host_extra_tools_register_in_all_modes`、`forkguard_file_content_caps_reject_before_writing`、`forkguard_multiline_still_blocks_destructive_segments`、registry prompt、Custom allowlist alias、`forkguard_session_trusted_roots_override_persisted_workspace_trust`、`forkguard_dispatch_allowlist_rejects_forged_calls_before_all_dispatch_backends`、`forkguard_read_only_turn_rejects_write_action_at_final_dispatch`、`forkguard_restricted_agent_uses_read_only_file_schema`、`forkguard_queued_control_op_keeps_restricted_turn_authority`、`forkguard_queued_goal_continuation_and_mcp_reload_keeps_restricted_turn_authority`、`forkguard_restricted_turn_defers_idle_subagent_completion_until_new_message`、`forkguard_restricted_agent_uses_hardened_read_only_shell_context`、`forkguard_restricted_turn_defers_idle_shell_wake_until_new_message`、`forkguard_restricted_turn_hooks_require_explicit_host_opt_in`、`forkguard_restricted_tool_audit_redacts_private_sentinel`。

### T3：嵌入上下文与技能来源

- **commit**：`5a9f52941b83452c1e8b76c2d679bac315edcf70`
- **规模**：13 文件，`+253/-71`
- **核心文件**：`prompts.rs`、`project_context.rs`、`repo_law.rs`、`model_context/{fragment,world_state}.rs`、`skills/`、`tools/skill.rs`、`working_set.rs`。
- **内容**：
  - static prompt composer 由 app 接管时，停用 ambient project context 和 repo law，避免用户目录文件隐式进入系统上下文。
  - Skill 只从宿主显式 `skills_dir` 扫描；disabled Skill 同时从目录和 `load_skill` 消失。
  - `FragmentId::Permissions` 单独沿用 100 KiB instruction 上限，其他 WorldState fragment 保持 v0.9.5 的 40 KiB 上限，避免全局放宽。
  - 用户消息前置内部 `<system-reminder>` 不参与 Working Set 路径提取，历史原文保持不变。
- **边界**：app 负责生成和选择 bundle/会话 Skill 根；底座只保证显式来源与上下文不变量。
- **守护**：`forkguard_runtime_loader_ignores_ambient_project_authority`、`forkguard_instruction_fragment_preserves_content_beyond_default_cap`、`forkguard_disabled_skill_is_neither_rendered_nor_loadable`、`forkguard_working_set_ignores_leading_system_reminder_paths`。

### T4：定时任务与运行生命周期

- **commit**：`fc84f7d3e5dca0e3db404d43e218597764129f9b`
- **规模**：4 文件，`+373/-24`
- **核心文件**：`automation_manager.rs`、`task_manager.rs`、`tools/automation.rs`、`tui/automation_routing.rs`。
- **内容**：
  - Automation 透传选定 model，并以 automation id 建立稳定 conversation key。
  - 保持 v0.9.5 当前 task schema v2，同时兼容读取 Pinvou 历史 v3/v4，拒绝未知更新 schema；thread/turn 链接跨 worker 边界及时持久化。
  - HOURLY 调度保持创建时刻锚点；休眠/关机错过时段不补跑，存在 queued/running run 时不重叠执行。
  - 只清理终态 run/task，并级联删除相应 artifact；活动运行保持可恢复。
  - 强制审批不能被通用 auto-approve 绕过。
- **边界**：app 负责展示、通知和业务工作区；底座负责调度与耐久运行事实。
- **守护**：`forkguard_scheduler_skips_offline_misfires_without_backfill`、`forkguard_scheduler_does_not_overlap_active_automation_run`、`forkguard_conversation_key_and_created_thread_survive_worker_boundary`、`forkguard_accepts_pinvou_v4_tasks_but_rejects_unknown_newer_schema`。


## 2. 父仓能力与 fork 的分界

以下能力保留在 `pinvou3-app`，不进入 CodeWhale fork：

- `features/assistant/tool_policy.rs`：Pinvou canonical tools 白名单和 MCP namespace 策略。
- `disallowed_tools` 的会话/连接器动态取值与工具商店开关。
- bundle instructions、按会话 Skill 组合目录、用户 AGENTS 注入。
- UI、Tauri IPC、工作区与产物卡、Shell 输出观察和前端终态对账。
- 定时任务页面、通知和业务日志展示。

CodeWhale fork 只提供这些产品能力不可缺少的底座生命周期入口和原子不变量。

## 3. v0.9.5 同步结论

### 上游已有，不再维护

- v0.9.5 原生 library/runtime crate 边界：T1 只保留必要公开面。
- 原生 `allowed_tools`：Pinvou 白名单直接复用，不恢复 fork-only 第二套白名单字段。
- 通用 OAuth 取消、Fleet roster、Runtime API、MCP registry 和 session-tree：直接使用上游。
- canonical action 工具面：不恢复旧独立工具名。

### v0.9.5 新增适配

- `EngineConfig` 新增 `subagent_state_root`，父仓显式透传默认值。
- 已删除的旧 `hidden_tools` 字段不再恢复；Pinvou 原有动态隐藏行为本就通过 `disallowed_tools` 完成。
- v0.9.5 WorldState 40 KiB fragment cap 只对 Permissions 做 100 KiB 窄例外，其他 fragment 不变。
- v0.9.5 workspace crate 拆分引起父仓 `Cargo.lock` 重算，未增加 Pinvou 直接依赖。

## 4. 验证

CodeWhale 当前已通过：

```text
cargo fmt --all -- --check
cargo check / Pinvou fork CI
cargo test -p codewhale-tui --lib --locked forkguard_ -- --test-threads=1
57 passed / 0 failed
```

父仓当前已通过：

```text
cargo fmt --all -- --check
./scripts/fork-guard.sh
CodeWhale 默认 57 + benchmark 6 passed；pinvou3-app 默认 23 + benchmark-hooks 17 passed
cargo test --lib --locked forkguard_admitted_display_fallback -- --test-threads=1
2 passed / 0 failed
node --test pinvou3-app/tests/scheduled_tasks_unit.test.js
PASS
python3 scripts/architecture-guard.py
./scripts/verify-public-submodule.sh
pinvou-v0.9.5-r13 -> f853f8f1566c57e6be40d5439a222a932aa79ef5
```

完整结果见 `docs/codewhale-upgrade-0.9.0-to-0.9.5.md`。环境相关忽略/基线失败按实际验证披露；`scripts/verify-public-submodule.sh` 已锁定不可变标签 `pinvou-v0.9.5-r13` 与父仓 gitlink 一致。

## 5. 后续修改规则

- 修改任一主题时，同步更新本文、`scripts/fork-guard.sh` 和对应 `forkguard_*` 行为测试。
- 通用修复从 upstream main 建净分支贡献；不得把整个 Pinvou 主题直接提交上游。
- 发布后把本节状态更新为远端维护分支、不可变标签和实际 commit，并验证父仓 gitlink 一致。
