# Pinvou Benchmark Platform 设计

## 1. 背景与目标

Pinvou 当前已有一套可运行的 Product smoke 评测底座：真实产品 Agent 无窗口执行、单
case Session、整批模型固定、超时和清理、工具与性能事件、JSONL/Markdown 报告、独立
Judge 以及隐私保护。该能力可用于快速健康检查，但当前只有 5 条 PLEP smoke case，且不
校验标准答案，不能作为 GAIA、BFCL 或其他公开 benchmark 的成绩。

本设计建立一个正式、统一、可扩展的 Pinvou Benchmark Platform，并交付一个用户可长期
使用的 `pinvou` CLI。近期明确支持三类 benchmark：

- GAIA：多步骤问答、联网检索、多模态附件与最终答案评分；
- BFCL：结构化函数调用与工具约束评分；
- Tencent WorkBuddy Bench：Docker/Harbor 沙箱中的真实工作区、patch、artifact 与测试
  套件评分。

目标是让三个 adapter 能够并行开发，同时保证：

1. 评测的是完整 Pinvou Agent，而不是只调用底层模型 API；
2. 官方数据版本、执行参数和 scorer 版本可复现；
3. 官方 benchmark 分数与内部 Smoke Health Score 永久分离；
4. benchmark 通过窄接口和只读 hook 观察产品，不改变现有 GUI、聊天、Session 或工具
   行为；
5. gated 数据、ground truth、Token、Cookie 和私人输入不进入仓库或公开报告；
6. 用户只需要一个正式 `pinvou` 命令，不再为每个 benchmark 开发独立 CLI。

## 2. 非目标

首个版本不做以下事情：

- 不把 Product smoke 分数换算为 GAIA/BFCL/WorkBuddy 分数；
- 不在本地伪造 GAIA private test 成绩；
- 不支持运行时动态加载任意第三方 adapter 二进制或脚本；
- 不重写 CodeWhale 的模型、工具循环、Session、Compaction 或 MCP 能力；
- 不重构现有 GUI 或普通聊天路径；
- 不让 benchmark hook 修改模型输入输出、工具参数、权限或执行结果；
- 不在首个里程碑实现 GAIA Level 2/3、BFCL 或 WorkBuddy 的完整运行；这些在通用契约
  冻结后作为独立 adapter 阶段开发。

## 3. 已锁定的官方来源

### 3.1 GAIA 数据

- Hugging Face 仓库：`gaia-benchmark/GAIA`
- 数据基准语义：GAIA 2023
- 官方数据格式：2025-10 Parquet 更新格式
- 固定 revision：`682dd723ee1e1697e00360edccf2366dc8418dd9`
- 本机私人快照目录：
  `~/.pinvou3/eval/datasets/gaia/682dd723ee1e1697e00360edccf2366dc8418dd9`
- 下载快照包含 8 个 Parquet 和 PDF、图片、音频、Excel、CSV、PPTX、DOCX、ZIP 等附件；
  Parquet 文件头尾魔数已验证。

GAIA 是 gated dataset。用户必须在 Hugging Face 接受条款并在本机认证。Token 只允许从
Hugging Face credential store 或环境变量读取，不进入 CLI 参数、Git、manifest 或报告。
validation/test 数据不得重新发布到公开仓库。

### 3.2 GAIA scorer

- Hugging Face Space：`gaia-benchmark/leaderboard`
- 固定 revision：`9f133d71362e77b3539f1514f31b9c101a545fec`
- 官方入口：`question_scorer`

官方 scorer 按 ground truth 类型处理数值、逗号/分号列表和普通字符串，并执行对应的
数值、空白、大小写和标点规范化。生产 CLI 使用 Rust 等价移植；CI 使用固定 revision 的
官方 Python scorer 运行 parity fixtures。任一结果不一致都阻断发布。不得增加 LLM/Judge
评分或更宽松的匹配逻辑。

### 3.3 WorkBuddy Bench

- 官方仓库：`Tencent/workbuddy-bench`
- 运行框架：Docker + Harbor
- 近期官方子集：Code、Web、Office、Security

WorkBuddy 不是单轮问答。它将 Agent CLI 放入沙箱工作区，采集 patch、artifact、trajectory、
测试和效率数据。因此它使用 External Harness Runner，不能强行塞入 GAIA/BFCL 的
Native Turn case。

## 4. 仓库与模块边界

新能力主体位于独立目录。第一阶段先完成通用核心并迁移现有 smoke；现有 Rust 业务代码只
允许新增一个窄 `headless_bridge` 和删除已被新平台替代的旧 eval 编排，不重构其他业务：

```text
pinvou-agent/
├── pinvou-cli/                         # 独立 Cargo workspace
│   ├── Cargo.toml
│   └── crates/
│       ├── cli/
│       ├── benchmark-core/
│       ├── agent-backend-api/
│       ├── pinvou-product-backend/
│       ├── adapter-smoke/
│       ├── adapter-gaia/
│       ├── adapter-bfcl/
│       └── adapter-workbuddy/
├── pinvou3-app/
└── CodeWhale/
```

职责：

- `cli`：参数解析、用户输出、退出码；不实现 benchmark 业务；
- `benchmark-core`：descriptor、plan、manifest、runner 调度、事件、恢复、outcome、score
  envelope、submission 和报告安全契约；
- `agent-backend-api`：完整 Pinvou Agent 的无头执行接口和只读 observer；
- `pinvou-product-backend`：唯一可依赖 `pinvou3_lib::headless_bridge` 的组合层，实现完整
  Pinvou Agent 后端；不得访问 GUI/Tauri command 或其他业务内部模块；
- `adapter-smoke`：承接现有 smoke cases、规则分析、Product Score、诊断和可选独立 Judge；
- `adapter-*`：官方数据验证、任务映射、官方 scorer 包装与 submission；
- `pinvou3-app`：GUI/Tauri 宿主。只有连接真实产品时才新增一个窄 `headless_bridge`；
- `CodeWhale`：继续作为模型与 Agent 底座，不加入 Pinvou benchmark 语义。

依赖只能向内：

```text
adapter-* ───────────────▶ benchmark-core
cli ─────────────────────▶ benchmark-core
cli ─────────────────────▶ agent-backend-api
cli ─────────────────────▶ pinvou-product-backend
pinvou-product-backend ───▶ pinvou3_lib::headless_bridge
pinvou3-app headless bridge ─▶ agent-backend-api
```

`benchmark-core` 不依赖 Tauri、具体 adapter 或官方数据集 SDK。adapter 之间互不依赖。

## 5. 用户入口

最终只发布一个正式 `pinvou` CLI：

```text
pinvou
├── chat
├── agent
│   └── run --headless
├── benchmark
│   ├── list
│   ├── fetch
│   ├── verify
│   ├── run
│   ├── status
│   ├── resume
│   ├── score
│   ├── report
│   └── submission
├── config
└── version
```

示例：

```powershell
pinvou benchmark fetch gaia --revision 682dd723ee1e1697e00360edccf2366dc8418dd9
pinvou benchmark verify gaia
pinvou benchmark run gaia --split validation --level 1 --pass 1 --concurrency 1
pinvou benchmark resume <run-id>
pinvou benchmark score <run-id>
pinvou benchmark report <run-id>
```

GUI 直接调用 `benchmark-core`，不解析 CLI 文本。CLI 和 GUI 使用相同 manifest、runner、
scorer 和报告实现。

现有 `eval_smoke.exe` 只在迁移期间作为结果、退出码、JSONL/Markdown、隐私和 Session 清理的
对照基线。`pinvou benchmark run smoke` 通过等价测试后，在同一迁移里程碑删除
`eval_smoke.exe`、旧 `eval_cli` 入口以及已迁入新平台的重复编排代码，不保留永久兼容壳。
最终唯一用户入口是 `pinvou benchmark ...`。

### 5.1 旧 eval 迁移边界

迁移遵循“通用能力进 core，Smoke 语义进 adapter，产品运行时留在 App”：

| 现有能力 | 新位置 | 处理方式 |
|---|---|---|
| suite 编排、超时、运行记录 | `benchmark-core` | 泛化为 runner、manifest、event、resume |
| JSONL/Markdown 安全写入 | `benchmark-core` | 统一官方与内部报告 envelope |
| smoke cases、ToolExpectation | `adapter-smoke` | 保持 case ID 与行为契约 |
| 规则分析、Product Score、诊断 | `adapter-smoke` | 仅作为 Smoke Health，不进入官方成绩 |
| 独立 Judge | `adapter-smoke` | 可选分析，不代替官方 scorer |
| EnginePool、模型固定、Session、工具循环 | `pinvou3-app` | 不搬迁，通过 `headless_bridge` 暴露窄接口 |
| Tauri GUI eval command | `pinvou3-app` | 后续直接调用相同 core service，不解析 CLI 文本 |
| `eval_smoke.exe` / 旧 eval CLI | 删除 | 新 smoke 等价验证通过后移除 |

迁移完成前不得让新旧两套报告、隐私或恢复逻辑继续独立演进。迁移完成后旧 eval 目录中若仍有
GUI 专用薄适配，只能调用新 core/adapter，不得保留第二套评分或编排实现。

## 6. 核心架构

```text
Benchmark CLI / GUI
        │
        ▼
Benchmark Registry
        │
        ├── GAIA Adapter
        ├── BFCL Adapter
        └── WorkBuddy Adapter
        │
        ▼
Task Planning Core
revision / split / filter / resume / run manifest
        │
        ├── NativeTurnRunner
        │     ├── GAIA
        │     └── BFCL
        │
        └── ExternalHarnessRunner
              └── WorkBuddy + Pinvou Headless CLI + Docker/Harbor
        │
        ▼
TaskOutcome
prediction / artifacts / status / usage / timing / trajectory refs
        │
        ▼
Official Scorer Adapter
        │
        ▼
JSONL + score.json + submission + Markdown
```

通用层不理解 GAIA 答案、BFCL 函数调用或 WorkBuddy patch 的正确性。每个 adapter 对自己
的 typed prediction、官方 scorer 和 submission 负责。

## 7. 稳定数据契约

### 7.1 BenchmarkDescriptor

```rust
pub struct BenchmarkDescriptor {
    pub id: BenchmarkId,
    pub adapter_version: String,
    pub dataset_revision: String,
    pub scorer_revision: String,
    pub supported_splits: Vec<Split>,
    pub execution_kind: ExecutionKind,
}

pub enum ExecutionKind {
    NativeTurn,
    ExternalHarness,
}
```

revision 必须是不可变 SHA、版本与 checksum，禁止只记录 `main` 或 `latest`。

### 7.2 BenchmarkTask

```rust
pub struct BenchmarkTask {
    pub task_id: String,
    pub category: Option<String>,
    pub level: Option<String>,
    pub execution: ExecutionRequest,
    pub reference_handle: Option<ReferenceHandle>,
}

pub enum ExecutionRequest {
    NativeTurn {
        prompt_handle: PrivateInputHandle,
        attachments: Vec<AttachmentHandle>,
        timeout: Duration,
        tool_policy: ToolPolicyId,
        output_contract: OutputContract,
    },
    ExternalHarness {
        workspace_archive: VerifiedArtifact,
        container_image_digest: String,
        harness_command: Vec<String>,
        timeout: Duration,
    },
}
```

公共结构使用 handle，不把 gated 问题、附件内容、ground truth 或隐藏测试直接放进事件和
报告对象。

### 7.3 TaskOutcome

```rust
pub struct TaskOutcome {
    pub task_id: String,
    pub status: TaskStatus,
    pub prediction: Option<Prediction>,
    pub artifacts: Vec<ArtifactReference>,
    pub usage: Option<UsageMetrics>,
    pub elapsed_ms: u64,
    pub trajectory_ref: Option<PathBuf>,
    pub failure_category: Option<SafeFailureCategory>,
}
```

`Prediction` 是带 type tag 的私有 payload：GAIA 使用最终文本，BFCL 使用结构化函数调用，
WorkBuddy 使用 patch/artifact/test 引用。通用层只持久化 adapter 声明为安全的字段。

### 7.4 BenchmarkAdapter

```rust
pub trait BenchmarkAdapter: Send + Sync {
    fn descriptor(&self) -> &BenchmarkDescriptor;
    fn verify_dataset(&self, dataset_root: &Path) -> Result<VerifiedDataset>;
    fn plan(
        &self,
        dataset: &VerifiedDataset,
        selection: &TaskSelection,
    ) -> Result<BenchmarkPlan>;
    fn prepare_task(
        &self,
        task: &BenchmarkTask,
        run: &RunContext,
    ) -> Result<PreparedTask>;
    fn score(&self, run: &CompletedRun) -> Result<OfficialScoreReport>;
    fn write_submission(
        &self,
        run: &CompletedRun,
        destination: &Path,
    ) -> Result<SubmissionArtifact>;
}
```

adapter contract v1 冻结后，GAIA、BFCL 和 WorkBuddy 可以并行开发。具体 adapter 不能直接
修改 runner；确需扩展 core 时必须单独提出、评审并补 contract tests。

## 8. 完整 Pinvou Agent 的无头桥接

桥接采用接口优先、只读 hook、默认 no-op：

```rust
pub trait HeadlessAgentBackend: Send + Sync {
    async fn prepare(&self, request: PrepareRequest) -> Result<AgentSessionHandle>;
    async fn run(
        &self,
        session: &AgentSessionHandle,
        task: AgentTaskInput,
        observer: Arc<dyn AgentRunObserver>,
    ) -> Result<AgentTaskOutcome>;
    async fn cancel(&self, session: &AgentSessionHandle) -> Result<()>;
    async fn close(&self, session: AgentSessionHandle) -> Result<()>;
}

pub trait AgentRunObserver: Send + Sync {
    fn on_event(&self, event: &SafeAgentEvent);
}
```

普通桌面应用使用 `NoopAgentRunObserver`。benchmark CLI 才注入 session-scoped observer。
observer 只能读取安全生命周期事件，不能修改模型输入输出、工具参数、权限检查、工具结果
或错误传播。observer 不接收 prompt、完整回答、工具输入输出、Token 或 Cookie。

只有在现有 Timeline/TurnResult 确实缺少安全事件时才增加最窄埋点。建议使用非默认
`benchmark-hooks` feature；默认产品构建不启用。hook panic 或 observer 错误不能中断 Agent
主流程。

附件走显式 `PrepareRequest`，不通过 hook 注入。GAIA 附件复制到单 task 私有工作区；Agent
无法访问 dataset/scorer 根目录。WorkBuddy 工作区由官方 Docker/Harbor 挂载。

连接真实产品时，现有应用只允许新增一个 `headless_bridge` 模块、必要的窄导出以及行为等价
测试。禁止重写 EnginePool、复制 Agent、修改 GUI、工具策略、Session 格式或 CodeWhale。

## 9. CLI 编排 GUI 自动测试

`pinvou` CLI 可以拉起 GUI 自动测试，但只能启动隔离的 Test GUI，禁止控制用户正在使用的
Pinvou 窗口或使用操作系统级鼠标坐标。建议命令：

```powershell
pinvou test gui
pinvou test gui --case benchmark-form
pinvou test gui --record-video
pinvou benchmark run gaia --split validation --level 1 --frontend gui-automation
```

官方 benchmark 默认仍使用 `--frontend headless`。`gui-automation` 用于验证 GUI 配置、启动、
状态、恢复和报告展示，或作为显式标记的独立运行通道；GUI 与 headless 成绩不得静默合并。

### 9.1 运行结构

```text
Pinvou CLI
  ├── 创建隔离测试 profile
  ├── 启动 Pinvou Test GUI
  ├── 建立本地认证 IPC
  ├── 驱动语义化 GUI 操作
  ├── 收集截图、视频、事件与结果
  ├── 关闭测试 GUI 进程树
  └── 清理本次测试 profile
```

Test GUI 使用测试专用启动配置，逻辑等价于：

```text
pinvou3 --automation
        --profile <isolated-profile>
        --ipc <private-endpoint>
        --run-id <run-id>
```

IPC 凭据不得直接放在命令行或日志中，必须通过继承句柄、私有权限文件或进程环境注入。
production GUI 默认不编译或不启用自动化控制接口。

### 9.2 隔离目录

每次 GUI 测试只访问：

```text
~/.pinvou3/test-runs/<run-id>/
├── profile/
├── sessions/
├── screenshots/
├── video/
├── events.jsonl
└── result.json
```

禁止读取或修改用户真实设置、Session、知识库、连接器、浏览器 profile 以及已运行的 Pinvou
窗口。若普通 Pinvou 实例占用同一 profile，CLI 必须拒绝启动，而不是抢占或复用。

### 9.3 自动化接口

GUI 自动化优先使用语义化操作：

```text
open_benchmark_page
select_adapter
set_split
start_run
wait_for_status
open_report
```

允许 DOM/组件 selector、测试 IPC、窗口状态、安全事件、截图对比和受控键盘输入。禁止移动
用户真实鼠标、抢占用户键盘焦点、绝对坐标点击、控制已有窗口或自动确认危险权限弹窗。

### 9.4 Manifest 与成绩边界

GUI 运行必须额外记录：

```json
{
  "frontend": "gui-automation",
  "gui_version": "<safe-version>",
  "viewport": "1440x900",
  "dpi_scale": 1.0,
  "locale": "zh-CN"
}
```

人工 GUI 验收只验证布局、文件选择、系统权限和平台差异，不能计入官方 benchmark 分数。
同一 manifest/outcome 在 CLI 和 GUI 的 scorer 结果必须一致。

### 9.5 失效与清理

- IPC 认证失败立即退出；
- GUI 超时后终止本次测试进程树并记录 timeout；
- 清理失败时保留隔离目录并报告安全路径，不递归删除未知目录；
- benchmark hook 未启用时拒绝 GUI automation；
- automation IPC 只使用本地私有通道，不监听局域网；
- GUI 自动化作为 benchmark core 完成后的独立阶段，不阻塞 GAIA Level 1 headless 里程碑。

## 10. Feature Module Contract v1

为了让业务、CLI 命令和 GUI 自动化持续增加而不修改旧功能，Pinvou 采用纵向 feature
module。业务逻辑、CLI adapter、GUI 和 automation driver 归属于同一个 feature，而不是
分别堆入全局中央文件：

```text
platform/
├── feature-registry/
├── cli-api/
├── automation-api/
└── event-api/

features/<feature-id>/
├── domain/
├── cli/
├── gui/
├── automation/
└── tests/
```

新增业务原则上只新增或修改自己的 `features/<feature-id>/`。platform 只提供稳定契约，不
理解具体业务；feature 之间禁止直接依赖彼此的内部实现。

### 10.1 静态 Feature 注册

第一版使用仓库内静态注册，不加载运行时动态库、未知脚本或不受信任 adapter。每个 feature
提供描述与注册函数：

```rust
pub struct FeatureDescriptor {
    pub id: &'static str,
    pub contract_version: ContractVersion,
    pub capabilities: FeatureCapabilities,
}

pub trait FeatureModule: Send + Sync {
    fn descriptor(&self) -> &FeatureDescriptor;
    fn register(&self, registry: &mut FeatureRegistry) -> Result<()>;
}
```

composition root 只汇总模块注册。重复 `feature-id`、重复命令、重复 automation action 或不兼容
contract version 必须启动失败，不能后注册覆盖先注册。

### 10.2 CLI Command Adapter

统一 CLI 只解析顶层命令和调度 feature module：

```rust
pub trait CommandModule: Send + Sync {
    fn name(&self) -> &'static str;
    fn command(&self) -> clap::Command;
    async fn execute(
        &self,
        matches: &ArgMatches,
        services: &ServiceRegistry,
    ) -> Result<ExitStatus>;
}
```

业务逻辑必须位于 domain service。CLI adapter 只将参数转换为 service request，并将结果转换
为稳定退出码和用户输出。GUI 与 CLI 共同调用 service，不互相调用，也不解析彼此的文本。

### 10.3 语义化 GUI Automation Driver

GUI 自动化不依赖 CSS class、DOM 层级、按钮文案或绝对坐标。每个 feature 自己实现稳定的
语义动作：

```rust
pub trait GuiAutomationDriver: Send + Sync {
    fn feature_id(&self) -> &'static str;
    fn contract_version(&self) -> ContractVersion;
    fn supported_actions(&self) -> &'static [ActionDescriptor];
    async fn perform(
        &self,
        action: SemanticAction,
    ) -> Result<ActionOutcome>;
}
```

benchmark v1 动作示例：

```text
benchmark.open
benchmark.select_adapter
benchmark.configure_run
benchmark.start
benchmark.wait
benchmark.open_report
```

知识库等后续业务可以新增自己的动作，而无需修改 benchmark driver。页面由按钮改为向导时，
只要语义契约不变，旧自动化仍应通过。

Test GUI 的 driver 调用正常 domain service，不复制业务逻辑、不跳过权限。production GUI
默认不注册 driver 或 automation IPC。

### 10.4 契约版本

每个 feature 的 CLI、automation action 和安全事件都使用版本化契约：

```json
{
  "feature": "benchmark",
  "contract_version": {"major": 1, "minor": 0},
  "actions": [
    "open",
    "configure_run",
    "start",
    "wait",
    "open_report"
  ]
}
```

- 新增可选 action 或可选字段：提升 minor；
- 新增可选参数必须提供稳定默认值；
- 删除、重命名或改变既有语义：提升 major，并保留明确兼容期；
- CLI/Test GUI 在运行前协商 major；不兼容时明确拒绝；
- golden contract tests 固定旧 action 的 request、outcome、退出码和事件 schema。

### 10.5 Capability 声明

调用方根据能力而不是 feature 名称分支：

```rust
pub struct FeatureCapabilities {
    pub cli: bool,
    pub gui: bool,
    pub automation: bool,
    pub headless: bool,
    pub resumable: bool,
}
```

feature 可以先只提供 CLI，之后兼容地增加 GUI 或 automation。调用方使用
`registry.supports(feature_id, capability)`，禁止在通用平台中增加
`if feature == "benchmark"` 形式的业务判断。

### 10.6 安全事件 Envelope

通用事件只包含路由和版本字段：

```rust
pub struct FeatureEvent {
    pub schema_version: u16,
    pub feature_id: String,
    pub event_type: String,
    pub run_id: Option<String>,
    pub safe_payload: SafePayload,
}
```

每个 feature 必须使用自己的安全 DTO 生成 `SafePayload`，禁止默认序列化领域对象。未知事件
允许旧消费者忽略；格式错误、超限 payload 或敏感字段必须拒绝持久化。事件不得成为修改
业务状态的隐式命令通道。

### 10.7 测试与架构守卫

每个 feature 自己维护：

1. domain contract tests，不启动 GUI；
2. CLI contract tests，覆盖参数、退出码和结构化输出；
3. automation driver contract tests，覆盖语义 action 输入输出；
4. 少量 GUI smoke，验证页面、driver 与 service 接线。

platform 只维护：

- registry 不允许重复 ID、命令或 action；
- contract version 协商和拒绝逻辑；
- production build 不暴露 automation endpoint；
- 一个 driver 失败不改变其他 feature；
- 旧 feature golden contracts 不因新 feature 改变；
- 普通用户 profile 不被 Test GUI 或 feature tests 访问；
- 依赖守卫禁止 feature 反向依赖 platform 和直接引用其他 feature 内部模块。

第一版不实现第三方动态插件、公共二进制 ABI、任意 YAML 代码执行或自动执行仓库中的未知
模块。Feature Module Contract v1 的目标是仓库内安全并行开发，而不是开放不受信任插件。

## 11. Ground truth 与权限隔离

```text
官方数据
├── executor view
│   ├── question
│   └── attachments
└── scorer-only view
    └── ground truth / hidden tests
```

- Agent 进程只能访问 executor view；
- scorer 在任务完成后单独启动；
- scorer 默认无网络；
- GAIA ground truth 不复制到 workspace、trajectory、events、JSONL 或 Markdown；
- WorkBuddy 隐藏测试不得位于 Agent 可读目录；
- BFCL reference calls 不进入模型上下文；
- private test 不允许本地 score，只允许生成官方 submission；
- `submission upload` 必须显式执行并带 `--confirm`，永不自动上传。

WorkBuddy 的命令执行与文件修改只允许在 Docker 沙箱。GAIA/BFCL 的宿主机执行使用
benchmark 专用工具策略，不继承无约束开发者权限。

## 12. Run Manifest、目录与恢复

每次运行开始前生成不可变 `manifest.json`，至少包含：

```json
{
  "schema_version": 1,
  "benchmark": "gaia",
  "adapter_version": "gaia-adapter/v1",
  "dataset_revision": "682dd723ee1e1697e00360edccf2366dc8418dd9",
  "scorer_revision": "9f133d71362e77b3539f1514f31b9c101a545fec",
  "split": "validation",
  "filters": {"level": 1},
  "model": {"provider": "<safe-provider>", "model": "<safe-model>"},
  "tool_policy": "gaia/v1",
  "concurrency": 1,
  "pass": 1
}
```

运行目录：

```text
~/.pinvou3/eval/runs/<run-id>/
├── manifest.json
├── events.jsonl
├── predictions.jsonl
├── score.json
├── report.md
├── artifacts/
└── trajectories/
```

manifest 创建后不可修改。`resume` 时 benchmark、adapter、dataset/scorer revision、split、
filter、模型、工具策略、pass 或并发任一项不匹配都拒绝恢复。

任务状态由追加式事件记录：

```text
planned → running → completed
                  ↘ failed
                  ↘ timeout
```

- `resume` 只重跑未完成任务；
- completed prediction 不重复调用模型；
- outcome 原子写入后才标记 completed；
- 同一 task 只允许一个活动 lease；
- 默认并发为 1；
- 重试次数与原因进入 manifest，不静默重试。

## 13. GAIA 最终答案与首个里程碑

GAIA 指令要求最后输出：

```text
FINAL ANSWER: <answer>
```

只接受最后一条 assistant 消息中的最后一个标记。提取器只去除前后空白，不自行重写答案。
缺失时记录 `missing_final_answer` 并按错误答案计；不调用 Judge 帮助提取。

首个可运行里程碑固定为：

> 使用锁定 revision 的 GAIA 2023 官方 validation Level 1 数据和附件，完整执行 Pinvou
> Agent，按固定官方 scorer revision 输出 pass@1 accuracy，并生成不泄露题目或答案的可审计
> 报告。

完整链路：

```text
revision 校验
→ Parquet Level 1 validation 读取
→ 附件复制到单 task 私有目录
→ 完整 Pinvou Agent 执行
→ FINAL ANSWER 提取
→ 官方 scorer parity
→ overall / Level 1 accuracy
→ JSONL / Markdown
→ resume
→ cleanup
```

报告必须显示 evaluated、correct、incorrect、failed、timeout、pass@1 accuracy、模型、工具
策略、并发、耗时、成本、dataset revision 和 scorer revision，并明确：

> 这是 GAIA validation 本地复现，不是 private test 官方排行榜成绩。

Smoke Health Score 必须分开显示或不显示，不进入 GAIA Accuracy。

## 14. 安全与兼容验收

实现必须证明：

1. HF Token、模型 Token、Cookie 不进入命令行、manifest、日志和报告；
2. GAIA ground truth 不进入 prompt、workspace、trajectory、events、JSONL 或 Markdown；
3. task workspace 不能遍历到 dataset/scorer 根目录；
4. scorer 不能修改 prediction；
5. resume 不重复 completed task；
6. hook 默认 no-op，普通 GUI 聊天不创建 benchmark 文件；
7. `NoopAgentRunObserver` 与未启用 hook 的业务结果一致；
8. hook 失败不能中断 Agent 主流程；
9. 新 smoke 迁移期间保持原 case、结果、退出码和兼容 schema；等价验证后删除旧 executable；
10. CodeWhale gitlink 与源码无修改；
11. WorkBuddy 只在 Docker 内获得命令与文件修改权限；
12. Git 工作树不包含任何 GAIA 数据文件；
13. 路径全部 canonicalize，禁止对根目录、用户主目录或仓库根执行递归清理；
14. adapter 静态注册，不执行未知动态插件；
15. private test 无 ground truth 时 CLI 拒绝本地评分。
16. GUI automation 不能访问用户真实 profile、已有窗口或局域网接口；
17. GUI 与 CLI 对同一 manifest/outcome 使用相同 scorer 并得到相同结果；
18. 人工 GUI 操作不能进入官方 benchmark 成绩。
19. 新 feature 不能改变旧 feature 的 golden CLI/automation/event contracts；
20. registry 拒绝重复 ID、重复 action 和不兼容 major version；
21. production build 不注册 automation driver 或 automation IPC；
22. feature 事件只能使用显式安全 DTO，不能默认序列化领域对象。

## 15. 开发顺序与并行边界

1. 新建独立 `pinvou-cli/` workspace；
2. 实现 Feature Module Contract v1、`benchmark-core`、`agent-backend-api` 和契约守卫；
3. 使用 mock 跑通 NativeTurn、ExternalHarness、manifest、resume、score 和 report；
4. 新增窄 `pinvou3_lib::headless_bridge` 与 `pinvou-product-backend`；
5. 将现有 eval 的可复用能力按 5.1 节迁入 core 与 `adapter-smoke`；
6. 跑通 `pinvou benchmark run smoke`，与旧 executable 做等价和隐私回归；
7. 删除 `eval_smoke.exe`、旧 `eval_cli` 与重复编排，冻结 adapter contract v1 并提交；
8. GAIA、BFCL、WorkBuddy adapter 基于已冻结 core 开始并行开发；
9. GAIA 完成 dataset 校验、Level 1 planning、answer extractor 与 scorer parity；
10. 先运行单条 GAIA validation Level 1，再运行完整 Level 1；
11. 规格、隐私、质量和真实运行审查通过后再扩 Level 2/3；
12. BFCL 和 WorkBuddy 各自遵循官方版本、运行器和 scorer，不修改 GAIA 或通用 runner；
13. 在 headless 基线稳定后实现 GUI automation backend，不阻塞前三个 adapter 的核心开发。

每个 adapter 原则上只能修改自己的 crate 和 CLI registry 的显式注册行。若通用契约不足，
必须先提出 core 变更、评估影响并通过契约审查，禁止在 adapter 中旁路实现第二套 manifest、
resume、报告或权限系统。

第一里程碑的提交门槛是：core/mock 全量通过、真实 smoke 在新命令运行成功、旧新结果契约等价、
旧 executable 与重复代码已删除、普通 GUI/聊天行为回归不变。达到该门槛后才宣布通用层可供
其他 adapter 并行开发。
