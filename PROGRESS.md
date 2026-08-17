# 开发进度追踪（feat/eval-and-perf）

> 本文件是可持续开发 checkpoint。会话中断后从这里继续。

## 当前状态

- **分支**：`feat/eval-and-perf`
- **Worktree**：`D:/Worksapce/SourceCode/Task/pinvou-agent-eval`
- **相对 origin/main**：运行 `git rev-list --count origin/main..HEAD` 获取实时数量
- **当前任务**：Product smoke 已具备 JSONL 事实报告、确定性 Product Score、产品问题诊断和
  可选独立 Judge Markdown 分析报告；T5b BFCL official-compatible adapter 仍待开发

## 任务列表

| ID | 任务 | 状态 | Commit |
|----|------|------|--------|
| T1 | 缓存修复 | ✅ | `5f2de373` |
| T2 | 分段计时基础设施 | ✅ | `90d0697b`、`4970e198`、`74baad7c` |
| T3 | ProductChatRuntime seam | ✅ | `a76d2c58`、`4a5d7cb1` |
| T4 | PinvouChatRunner + Mock + 测试 | ✅ | `178b636e`、`49de4a3e`、`6ebca911` |
| T4b | PLEP smoke 任务集 + Markdown | ✅ | `0637504a` |
| T5a | Product smoke 共享 runner、JSONL、无窗口 CLI | ✅（实跑通过） | `435fe02d`、`713c9b31`、`695cae97`、`6583ed1e`、`dbc8fd1c`、`96d93521`、`2587b23d`、`402fdba3` |
| T5a-Analysis | Markdown 分析报告、确定性规则、独立 Judge | ✅（源码完成，等待本轮统一编译与实跑） | `415f2ce8` 至 `46a26359` |
| T5a-Score | 确定性产品健康评分与可执行诊断摘要 | ✅（实跑通过） | `1d7428a6` 起 |
| T5b | BFCL official-compatible adapter / scorer | ⬜ | — |
| T6 | MCPEval PoC | ⬜ | — |
| G1-G9 | GAIA 官方评测适配器 | 🔧 G1-G7 已提交，G8 部分完成，G9 待办 | `97008045` 等 |

## 当前评测数据流

```text
cases::smoke_cases()                    5 条 PLEP smoke case
    ↓
run_eval_suite<R: ProductChatRuntime>   顺序执行、失败不中断、逐 case callback
    ↓
PinvouChatRunner<EnginePoolRuntime>     真实产品链路、唯一临时 session、结束后清理
    ↓
EvalRecord                              status / usage / elapsed / milestones / error
    ├─ analyze_rules()                  确定性问题诊断
    ├─ calculate_product_score()        五维确定性产品健康评分（不依赖 Judge）
    ├─ 独立 Judge（可选）               六维评分与 AI 建议；失败时安全降级
    ├─ summarize_product_problems()     产品问题、改进动作与可量化验收标准
    ├─ EvalReportWriter                 增量 .tmp → complete 后原子发布 .jsonl
    └─ write_markdown_report()          同 basename 原子发布 .md
```

GUI `run_eval_smoke` 和无窗口 `eval_smoke` 已统一使用上述共享路径。JSONL 默认写入
`~/.pinvou3/eval/`，包含非敏感的版本、模式、case-set、provider/model 标识、逐 case
结果、token usage 和已采集的 timing milestones。

## Eval 与 Product 的区别

- **Eval** 是评测流程：用固定 case 执行、采集结果、应用确定性规则、可选调用独立 Judge，
  最后生成可复查的 JSONL 和 Markdown 报告。
- **Product**（有时口头写作 produce）是当前 eval 的运行模式：case 走 Pinvou 的真实产品会话链路，
  包含产品系统提示词、Skills、Memory、工具策略和 Engine 行为。它不是另一个评分器，也不是
  BFCL 的同义词。
- 因此，Product eval 回答的是“当前 Pinvou 产品链路在这组 smoke case 上表现如何”，适合
  内部模型比较、版本回归、延迟与成本基线；它不自动获得公开 benchmark 的可比性。

## 运行方式

在 `pinvou3-app/src-tauri/` 下执行：

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo run --bin eval_smoke --features dev-tools -- --mode product
rustup run stable-x86_64-pc-windows-msvc cargo run --bin eval_smoke --features dev-tools -- --mode product --judge-model-id <saved-model-id>
```

- 第一条命令运行**规则模式**：不配置 Judge，Markdown 中 Judge 状态为 `not_configured`，
  确定性规则与总结建议仍会完整生成。
- 第二条命令运行**规则 + 独立 Judge 模式**。`<saved-model-id>` 必须是本机 Pinvou 设置中
  已保存模型的 ID，且其规范化后的 provider/model 身份必须与本批被测模型不同；相同模型会
  以 `skipped_same_model` 降级到规则报告，未找到 ID 或 Judge 调用/解析失败会以 `failed`
  降级。上述 Judge 降级本身不会把成功的 eval case 改成失败。
- 默认模式也是 `product`；标准输出打印 Markdown 内容、JSONL/Markdown 绝对路径、Product
  Score（含公式版本）和 Judge 状态，便于脚本统一采集。
- 全部 case 为 `Completed` 且报告成功落盘时退出码为 0；任一 case 失败/超时、启动失败或
  Markdown 写入失败时退出码为 1。Judge 失败但 case 和报告写入成功时，退出码仍为 0。
- `--mode official-compatible` 当前明确返回 unsupported，避免误生成伪 BFCL 成绩。

## 报告文件与解读

JSONL 默认写入 `~/.pinvou3/eval/`，Markdown 与它位于同一目录并共享 basename，例如：

```text
plep-smoke-...jsonl
plep-smoke-...md
```

- JSONL 是机器可读的运行事实源，保存版本、模式、case-set、非敏感 provider/model 标识、
  case 状态、token usage、timing milestones 和最终分析状态。
- Markdown 是给人阅读的分析报告，固定包含 10 节：运行结论、产品问题与改进方向、产品健康
  评分、关键指标、逐用例诊断、工具与性能观察、确定性规则发现、独立 Judge 质量评分、
  P0/P1/P2 改进建议、评测限制与可比性说明。
- `[规则事实]` 可复现自状态、耗时、token 与工具事件；`[AI 推断]` 是独立 Judge 基于本次
  内存材料的判断，应结合证据与置信度复核。
- **P0** 表示执行或工具失败等会使结果不可信的问题，应先修复；**P1** 表示明显违反工具
  约束或较大的效率/缓存问题，应进入近期计划；**P2** 表示重复调用、相对延迟异常等优化项。
- “评测限制与可比性说明”必须与结论一起阅读。当前 smoke 集少于 10 条时不能据单次结果
  推断趋势；Judge 分数也不是客观真值，模型偏差与短样本都会影响结果。

## 隐私边界

- 原始用户 prompt、助手完整回答、工具输入和工具输出仅在本次进程内用于分析，不写入
  JSONL 或 Markdown。
- `EvalRecord.error` 仅保留在进程内供规则和 Markdown 安全归纳使用，JSONL 不序列化原始
  错误；case callback 自身失败时只写固定 `case_execution_failed` 类别，原始 `Err` 仍沿调用栈返回。
- 持久化报告只保留 case ID、状态、聚合指标、规范化工具名/失败标志，以及经过长度限制和
  敏感模式检查的诊断文本；认证头、API key、Cookie、Token 等命中检测时 Markdown 拒绝落盘。
- Judge prompt 同样不包含凭据、session 文件路径、完整工具输入或完整工具输出。报告中
  Judge 失败原因隐藏，避免 provider 错误详情带出敏感信息。

## 评分与公开榜单边界

- **Pinvou Product Score** 使用 `pinvou-product-score/v1` 公式输出 `0..100` 整数，由任务完成
  （35%）、工具可靠性（25%）、约束遵循（15%）、性能效率（15%）和运行稳定性（10%）组成。
  它只读取确定性运行记录和规则 finding；Judge 未配置、失败或输出变化都不会改变该分数。
- Product Score 只适合在**相同 case 集、case-set 版本、模型、产品配置、运行环境和评分公式
  版本**之间做内部趋势比较。少于 10 条样本属于低置信；建议同一条件至少运行 3 次并比较
  中位数，不能根据单次 5-case smoke 宣称稳定提升。
- JSONL `complete` 记录可选保存 `product_score` 与 `product_score_version`；空运行不写这两个
  字段，旧消费者可继续忽略新增字段。

- **Eval Framework** 是运行、采集和报告基础设施，不等于某个公开 benchmark。
- **Product mode** 使用 Pinvou 的系统提示词、Skills、Memory、工具策略和会话逻辑，适合做
  Pinvou 内部模型比较、版本回归、延迟与成本基线。
- Product mode / PLEP 的完成率、规则结论和 Judge 分数**不能直接与 BFCL 或其他网上榜单的
  分数、名次比较**；Product Score 和当前 Judge 六维评分都服务于本项目诊断，不是公开榜单
  评分器，也不能换算成 BFCL Accuracy。
- 要建立有意义的可比结果，必须锁定同一官方数据集及版本、官方推理/工具调用协议、工具定义、
  scorer/evaluator 版本、模型与采样参数、运行环境，并进行多次运行后报告均值、方差或置信区间。
- 只有未来的 **official-compatible mode** 满足上述条件，结果才可作为对应版本榜单的社区
  复现结果近似比较；仍应明确环境差异和统计误差，不能把单次 Product smoke 当作榜单成绩。

详细设计见：

- `docs/superpowers/specs/2026-08-11-eval-smoke-runner-design.md`；
- `docs/superpowers/specs/2026-08-12-eval-markdown-analysis-report-design.md`。
- `docs/superpowers/specs/2026-08-12-eval-product-score-and-diagnosis-design.md`。

## 2026-08-12 Markdown 分析报告静态验证记录

本轮 Task 7 已完成：

- `git diff --check`（仅有 Windows 工作区 LF/CRLF 转换提示，无空白错误）；
- `python scripts/architecture-guard.py`，结果为 `no architecture debt increased`；
- 对实现逐项静态审计：同 basename JSONL/Markdown、确定性规则、整批被测模型快照、独立
  Judge 与同模型拒绝、Judge 安全降级、Markdown 写失败返回错误、分析材料 `serde(skip)`、
  P0/P1/P2 证据与建议、公开榜单声明限制、Judge session/模型快照清理均有对应实现和测试。

按本轮“先完成功能、跳过耗时编译自测”的约定，Task 7 文档提交不单独运行 Cargo；完整的
`cargo check`、测试编译与真实规则/Judge smoke 由随后一次统一验证执行，结果不得预先视为通过。

## 2026-08-12 验证记录

已通过：

- 本轮涉及 Rust 文件的定向 `rustfmt --check`；
- `python scripts/architecture-guard.py`；
- `cargo metadata --no-deps --format-version 1`，确认 `eval_smoke` target 及
  `required-features = ["dev-tools"]`；
- 使用 `stable-x86_64-pc-windows-msvc` 执行
  `cargo check --bin eval_smoke --features dev-tools`，项目源码检查通过；
- 使用真实 `deepseek-v4-pro` 执行 Product smoke：5/5 case 为 `Completed`、24 条
  milestone、`all_succeeded=true`、进程退出码 0；报告位于
  `C:/Users/c24894/.pinvou3/eval/plep-smoke-20260812T024802173Z-product-20260812T024802173Z-82332.jsonl`；
- 实跑结束后对应临时 session 为 0，评测目录遗留 `.tmp` 为 0；
- `git diff --check`（各原子提交前执行）。

尚未通过/尚未执行：

- 仓库固定的 GNU 1.97.1 工具链仍因本机缺少 `dlltool.exe` 无法链接；本轮使用可用的
  stable MSVC 工具链完成编译和实跑；
- `cargo test --lib features::assistant::eval` 的测试代码已完成编译，但测试可执行文件启动时
  返回 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)`，因此不得宣称 Rust 单测已实际运行；
- 全仓 `cargo fmt --all -- --check`：分支既有的 `engine.rs`、`assistant/mod.rs`、
  `platform/bridge.rs` 存在格式差异；本轮文件的定向格式检查通过；
- GitNexus `detect_changes`：当前 worktree 无可用 GitNexus MCP/CLI，改用 Git diff 范围审计。

因此 T5a 已完成源码检查和真实 provider smoke；Rust 单测运行仍等待本机测试 DLL/manifest
环境修复或由 CI 验证。

## 下一步

1. 开发 T5b BFCL adapter：扩展 case 工具定义、锁定官方数据版本与推理参数、接入官方 scorer；
2. 在 CI 或修复后的 Windows 测试环境运行 Mock/Rust 定向测试；
3. PR 使用 Mock 测试；真实 provider smoke 放人工或 nightly 门禁。

## 2026-08-12 Product Score 验证记录

- `pinvou-product-score/v1` 已接入 CLI、GUI、JSONL complete 与 Markdown；JSONL 空运行会省略
  可选分数字段，公开 outcome 只暴露总分和公式版本。
- GNU 1.97.1 定向 `rustfmt`、`git diff --check`、`python scripts/architecture-guard.py`、
  `cargo check --bin eval_smoke --features dev-tools`、`cargo test --lib eval_ --no-run` 和
  `cargo test --bin eval_smoke --features dev-tools --no-run` 均通过。
- CLI 摘要定向单测实际执行通过；lib 测试可执行文件仍受本机
  `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` 影响，测试代码已完成编译。
- 真实 `deepseek-v4-pro` rules-only Product smoke 为 5/5 Completed，Product Score 为 79/100
  （良好、小样本）；报告明确列出工具链 P0、约束遵循 P1、性能 P2 及其动作和连续 3 次验收标准。
- 本次 JSONL/Markdown 隐私模式扫描命中 0，评测目录遗留 `.tmp` 为 0，临时 eval session 为 0。

## 2026-08-14 GAIA 官方评测适配器进度

GAIA validation Level 1 适配器（`adapter-gaia` crate + CLI 接线）开发计划见
`docs/superpowers/plans/2026-08-13-gaia-official-adapter.md`。

- **G1-G4**（score DTO + dataset + fetch + private inputs + scorer）：已提交。
- **G5**（收紧下载与权限边界）：已提交 `2f94e14c` 及后续加固。流式下载大小限制、Windows
  受保护 DACL（当前用户、SYSTEM、Administrators）、reparse/symlink 检测。
- **G6**（submission JSONL 导出）：已提交 `1705bf24` + 加固。原子 hard_link no-clobber 发布、
  数据集行序、覆盖校验、`validate_parent` 拒绝不安全路径。测试 18/18 GREEN。
- **G7**（CLI 接线）：已提交，包含 `1db2dae2`、`ac6278d4`、`f3cda0d4`、`2ab8fa6e` 和
  `ea3a134e`。registry/parser/execute/product-backend 链路已接通；raw `verify --source` 使用固定
  官方附件摘要做只读校验，run/resume/score/submission 只消费摘要绑定的 ready dataset，
  但当前 Windows 产品 CLI 可执行文件仍以 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` 退出，不能据此
  宣称产品链路已真机跑通；Windows headless 附件还固定返回
  `attachments_platform_security_unsupported`。
- **G8**（文档与真实 gated 验证）：部分完成。固定 revision 的 gated snapshot 曾在旧 ready
  marker 契约下完成 fetch/verify；`530c1739` 至 `d3ef2dc3`、`6c545220` 的完整性加固固定了
  11 个官方 Level 1 附件摘要，新增 `.pinvou-gaia-integrity-v1`，让 ready marker 绑定清单摘要，
  并在运行时冻结附件后再次复核内容。旧快照会 fail closed，必须重新 fetch 或从符合固定摘要的
  可信源 import 才能成为当前版本的 ready 快照。本次文档更新没有重新执行联网下载或 Tauri 真机验证。
  真实 product run、score、submission、真实 Python scorer 逐题 cross-check、输出隐私审计和
  临时数据清理验证均未完成。
- **G9**（最终复审）：待 G8 GREEN 后执行。

固定版本：dataset `682dd723...`、scorer `1349a179...`、adapter `pinvou-gaia-adapter/v1`、
runtime `hf-spaces-python-3.10-unicode-13.0`。

## 注意事项

- 每个任务原子提交，commit message 使用 `<type>(<scope>): <中文>` 并带 Signed-off-by；
- CodeWhale 改动需同步 fork 文档、指纹和 guard；
- 不修改与当前任务无关的脏文件。
