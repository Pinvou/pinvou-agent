# Eval Smoke Runner 设计

## 背景

当前评测分支已经具备 `ProductChatRuntime`、`PinvouChatRunner`、`MockRuntime`、5 条 PLEP smoke case、Markdown 报告格式化和 App 内 `run_eval_smoke` 命令。不过，App 命令仍重复实现执行循环，真实 `EnginePoolRuntime` 没有通过通用 Runner 入口运行，也没有可直接从命令行启动的无窗口 runner。

本设计完成 T5 的 runner binary 闭环。BFCL adapter 和 MCPEval 不属于本轮实现，但本轮产物必须为后续官方兼容评测保留清晰边界和可复现数据。

## 概念与评分边界

### Eval Framework

Eval Framework 是公共评测基础设施，负责读取测试用例、运行 Agent、采集事件、形成记录并输出报告。它本身不代表某一种评分口径。

### Product mode

Product mode 通过完整 Pinvou 产品链路运行评测，包括现有 provider、模型、系统提示词、Skills、Memory、工具策略和会话逻辑。它衡量真实产品表现，适合：

- 比较不同模型在 Pinvou 中的效果、延迟和成本；
- 检测同一模型在产品版本迭代中的回归；
- 建立 PLEP 内部性能与稳定性基线。

Product mode 的分数不能直接作为 BFCL 或其他公开榜单成绩。

### Official-compatible mode

Official-compatible mode 是后续 BFCL adapter 使用的严格评测模式。它复用 Pinvou 的 provider 和模型配置，但禁用会改变公开评测口径的 Pinvou 增强层，并使用固定版本的完整官方数据、推理协议和官方 evaluator。

只有在数据版本、类别、推理参数、工具定义、评分器和运行环境均与公开评测口径一致时，结果才可作为同版本榜单的社区复现结果进行近似比较。部分数据运行、PLEP 结果或 Product mode 结果不得包装成公开榜单成绩。

本轮实现 `Product` 模式，并在类型和报告元数据中为未来的 `OfficialCompatible` 模式预留扩展位置。

## 范围

### 本轮包含

- 抽取共享的批量评测入口；
- 让 GUI 命令和 CLI 都使用 `PinvouChatRunner<EnginePoolRuntime>`；
- 新增无窗口 `eval_smoke` Tauri binary；
- 复用 `~/.pinvou3/` 中已有的 provider、模型和 bundle 配置；
- 终端输出 Markdown 摘要；
- 将运行元数据和逐 case 结果持久化为 JSONL；
- 明确批次级与 case 级失败语义；
- 增加单元、集成和编译验证。

### 本轮不包含

- BFCL 数据适配和官方评分器；
- MCPEval 或 MCP-AgentBench 适配；
- 前端评测页面；
- CI 中调用真实付费 provider；
- PLEP full 任务集或跨运行趋势图。

## 架构

### 共享评测入口

新增共享 `run_eval_suite<R: ProductChatRuntime>`。它接收 runtime、case 集合和运行上下文，逐条调用 `PinvouChatRunner`，返回完整批次结果。该入口只负责执行和记录，不负责 provider 初始化，也不内置特定 benchmark 的评分逻辑。

现有 Tauri `run_eval_smoke` 命令改为：

1. 从受管的 `EnginePool` 构造 `EnginePoolRuntime`；
2. 读取 PLEP smoke cases；
3. 调用共享 suite；
4. 返回 Markdown 摘要。

删除命令层当前重复的 `run_case` 和 `wait_for_completion`，确保 GUI、CLI 和 Mock 测试通过同一 Runner 路径。

### 无窗口 CLI

新增 `src/bin/eval_smoke.rs`，并在 `Cargo.toml` 中以 `dev-tools` feature 注册。binary 创建正常 Tauri runtime，但在应用构建前清空窗口配置，不创建 WebView。

在 Tauri `setup` 阶段，binary 按生产路径初始化运行所需的最小状态：

- Pinvou 运行环境和资源目录；
- `SessionStore`；
- `EnginePool` 及现有工具策略；
- `EnginePoolRuntime`；
- PLEP smoke suite。

完成后打印报告并退出事件循环。不得启动主窗口，也不得要求前端或开发者控制台参与。

如果验证发现 Tauri 配置无法可靠地在构建前移除窗口，则停止扩展范围，改用独立的最小 Tauri context；不重构生产 `EnginePool` 的 AppHandle 依赖。

### 评测模式

引入可序列化的 `EvalMode`：

- `Product`：本轮实现；
- `OfficialCompatible`：仅作为报告与接口的保留值，实际调用在 BFCL adapter 实现前返回明确的 unsupported 错误。

禁止用布尔参数表示模式，避免后续加入 benchmark 模式时产生含义不明的组合。

## 数据流

1. CLI 读取现有 Pinvou 配置并启动无窗口 runtime。
2. 创建唯一 `run_id` 和开始时间。
3. 写入批次元数据，包括模式、case-set 名称与版本、模型、provider、时间和应用版本。
4. 顺序执行 PLEP smoke cases，避免并发改变 provider 限流和产品时序语义。
5. 每条结果完成后立即追加 JSONL。
6. 全部 case 结束后关闭临时 writer，并把临时报告安全重命名为正式文件。
7. 在终端打印 Markdown 摘要和报告绝对路径。

## 报告格式

报告目录为 `~/.pinvou3/eval/`。文件名使用 UTC 时间和 `run_id`，例如：

```text
plep-smoke-20260811T120102Z-<run-id>.jsonl
```

写入期间使用同名 `.tmp` 文件；只有批次完成后才重命名为 `.jsonl`。进程异常中断时保留 `.tmp`，使部分结果可恢复且不会被误认为完整报告。

JSONL 第一行为批次元数据，后续每行是一条 case 结果。元数据至少包含：

- schema version；
- run ID；
- eval mode；
- case-set 名称和版本；
- Pinvou 版本；
- provider 和模型标识；
- started/finished 时间；
- completion 状态。

case 记录至少包含：

- case ID、session ID、turn ID；
- status 和 error；
- elapsed time；
- input、output 和 cache-hit token；
- 已采集的 timing milestones；
- schema version 和 run ID。

报告不得包含 API key、Cookie、Token、完整私有配置或未脱敏的凭据。

## 错误与退出语义

### 批次级错误

以下错误立即终止，输出可操作错误并返回非零退出码：

- 配置或模型不可用；
- Tauri runtime、SessionStore 或 EnginePool 初始化失败；
- 报告目录或临时文件无法创建；
- 未实现的 eval mode 被请求。

批次初始化失败不得生成带 `completed` 状态的正式报告。

### Case 级错误

单条 case 的 provider 错误、runner 错误、超时或取消会被写入 `EvalRecord`，后续 case 继续执行。批次结束后：

- 全部 case 成功：退出码 0；
- 任一 case 失败、超时或取消：报告正常落盘，但退出码非零。

## 测试策略

实现遵循测试先行。

### 共享 suite

使用 `MockRuntime` 先添加失败测试，覆盖：

- 成功、超时和错误均被保留；
- 单 case 失败不阻断后续 case；
- 输出顺序与输入 case 顺序一致；
- 批次汇总能区分全部成功和部分失败。

### JSONL writer

使用临时目录添加失败测试，覆盖：

- 元数据和 case 记录可反序列化；
- 换行、引号和 Unicode 被正确转义；
- 每条 case 立即追加；
- 完成后 `.tmp` 安全重命名；
- 写入失败不产生伪完成报告；
- 敏感配置不会进入报告。

### GUI 命令与 binary

- GUI 命令只适配 state 和返回值，不再拥有执行循环；
- binary 通过编译检查；
- 无 provider 消耗的启动/参数错误路径可以自动测试；
- 真实 provider smoke 作为人工或 nightly 验证，不进入普通 PR 测试。

### 完成门禁

- `cargo fmt --all -- --check`；
- 定向 Rust 测试；
- `cargo check --bin eval_smoke --features dev-tools`；
- `python scripts/architecture-guard.py`；
- GitNexus `detect_changes`（工具可用时）；
- 一次真实 provider PLEP smoke，并保留报告路径和已知成本。

当前本机 Rust 环境缺少 `dlltool.exe`。实施前先定位并修复工具链；如果无法恢复，必须把编译和测试列为未验证项，不能宣称通过。

## 验收标准

- `cargo run --bin eval_smoke --features dev-tools` 无需打开桌面窗口即可运行 PLEP smoke；
- GUI 和 CLI 都使用共享 Runner 路径；
- 终端显示 Markdown 摘要和 JSONL 绝对路径；
- JSONL 包含足够的非敏感元数据支持复现和后续评分；
- 单 case 失败不会丢失其他结果；
- 批次退出码准确反映完整成功或部分失败；
- 文档明确区分 Eval Framework、Product mode 和 Official-compatible mode；
- PLEP 报告不宣称可直接对比公开榜单。
