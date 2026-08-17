# Pinvou Benchmark Common Platform Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 交付一个独立、可运行、可恢复且安全的 `pinvou` CLI 通用评测平台，并冻结 GAIA、BFCL、WorkBuddy 可以并行实现的 Adapter Contract v1。

**Architecture:** 在仓库根目录新增独立 Rust workspace `pinvou-cli/`，由 `cli`、`benchmark-core`、`agent-backend-api` 三个 crate 构成；本阶段不修改 `pinvou3-app`、`CodeWhale` 或 `eval_smoke`。`benchmark-core` 只处理版本化契约、静态注册、运行清单、事件恢复、双 Runner 调度与安全持久化，不理解任何官方 benchmark 的题目或评分语义。

**Tech Stack:** Rust 1.97.1 / edition 2021 / MSRV 1.89、Cargo workspace、clap 4、serde/serde_json、tokio、async-trait、thiserror、sha2、uuid、chrono、fs2、assert_cmd、predicates、tempfile、Python 3 架构守卫。

---

## 范围与交付边界

本计划只实现批准设计中的通用平台基础层。完成后：

- `pinvou benchmark list|fetch|verify|run|status|resume|score|report|submission` 均有稳定 CLI 语法和退出码；
- Feature Module Contract v1、Benchmark Adapter Contract v1、Headless Agent Contract v1 已由 golden tests 固定；
- Native Turn 与 External Harness 两类执行路径可使用 mock backend 完成端到端运行、恢复和评分；
- 运行数据只能写入显式 base directory 下的 `eval/runs/<run-id>`，manifest 不可覆盖，事件只追加；
- ground truth、凭据和私有输入不会进入通用事件、manifest 或用户输出；
- GAIA、BFCL、WorkBuddy 的开发者只新增自己的 adapter crate 和一行静态注册，不创建第二套 CLI、manifest、恢复或报告系统。

本计划明确不接入真实 Pinvou Agent、不读取 GAIA Parquet、不移植 GAIA scorer、不启动 Docker/Harbor，也不实现 GUI automation。这四项分别使用后续独立计划完成。

## 文件结构

```text
pinvou-cli/
├── .cargo/config.toml                         # 将构建产物放到根目录已忽略的 target/
├── Cargo.toml                                 # 独立 workspace 与统一依赖版本
├── README.md                                  # 用户入口、边界和本地验证
├── docs/
│   ├── adapter-contract-v1.md                 # Adapter 作者契约与并行边界
│   └── security-model.md                      # ground truth、凭据、路径和事件边界
├── scripts/
│   └── check_boundaries.py                    # 禁止依赖 app/CodeWhale/adapter 互相引用
├── crates/
│   ├── agent-backend-api/
│   │   ├── Cargo.toml
│   │   ├── src/{lib,backend,observer,types}.rs
│   │   └── tests/backend_contract.rs
│   ├── benchmark-core/
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── adapter.rs
│   │   │   ├── contracts.rs
│   │   │   ├── event.rs
│   │   │   ├── feature.rs
│   │   │   ├── manifest.rs
│   │   │   ├── registry.rs
│   │   │   ├── runner.rs
│   │   │   ├── security.rs
│   │   │   ├── service.rs
│   │   │   └── store.rs
│   │   └── tests/
│   │       ├── adapter_contract.rs
│   │       ├── feature_contract.rs
│   │       ├── fixtures/{feature-v1.json,manifest-v1.json,outcome-v1.json}
│   │       ├── recovery_contract.rs
│   │       ├── runner_contract.rs
│   │       └── service_contract.rs
│   └── cli/
│       ├── Cargo.toml
│       ├── src/{app,exit,lib,main,output}.rs
│       ├── src/commands/{benchmark,mod}.rs
│       └── tests/cli_contract.rs
└── tests/
    └── forbidden-data-markers.txt
.github/workflows/pinvou-cli.yml               # 仅 pinvou-cli 路径触发的独立门禁
```

### Task 1: 创建独立 Cargo workspace 与正式二进制

**Files:**
- Create: `pinvou-cli/.cargo/config.toml`
- Create: `pinvou-cli/Cargo.toml`
- Create: `pinvou-cli/crates/agent-backend-api/Cargo.toml`
- Create: `pinvou-cli/crates/agent-backend-api/src/lib.rs`
- Create: `pinvou-cli/crates/benchmark-core/Cargo.toml`
- Create: `pinvou-cli/crates/benchmark-core/src/lib.rs`
- Create: `pinvou-cli/crates/cli/Cargo.toml`
- Create: `pinvou-cli/crates/cli/src/lib.rs`
- Create: `pinvou-cli/crates/cli/src/main.rs`

- [ ] **Step 1: 写 workspace 冒烟测试**

先在 `pinvou-cli/crates/cli/src/lib.rs` 写出会失败的测试，要求正式二进制名稳定且 package 版本存在：

```rust
pub const BINARY_NAME: &str = "pinvou";

#[cfg(test)]
mod tests {
    #[test]
    fn formal_binary_is_named_pinvou() {
        assert_eq!(super::BINARY_NAME, "pinvou");
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
```

- [ ] **Step 2: 运行测试并确认 workspace 尚不存在**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli formal_binary_is_named_pinvou`

Expected: FAIL，Cargo 报告 `pinvou-cli/Cargo.toml` 不存在。

- [ ] **Step 3: 创建完整 workspace manifests**

`pinvou-cli/Cargo.toml` 使用以下内容：

```toml
[workspace]
resolver = "2"
members = [
  "crates/agent-backend-api",
  "crates/benchmark-core",
  "crates/cli",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.89"
license = "MIT"
authors = ["pinvou"]

[workspace.dependencies]
anyhow = "1.0"
async-trait = "0.1"
chrono = { version = "0.4", default-features = false, features = ["clock", "serde"] }
clap = { version = "4.5", features = ["derive"] }
fs2 = "0.4"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
sha2 = "0.10"
thiserror = "2.0"
tokio = { version = "1.49", features = ["fs", "macros", "process", "rt-multi-thread", "signal", "sync", "time"] }
uuid = { version = "1.18", features = ["serde", "v4"] }

[workspace.lints.rust]
unsafe_code = "forbid"
unused_must_use = "deny"

[workspace.lints.clippy]
dbg_macro = "deny"
todo = "deny"
unwrap_used = "deny"
```

`.cargo/config.toml`：

```toml
[build]
target-dir = "../../target/pinvou-cli"
```

三个 crate 均继承 workspace package/lints。CLI manifest 必须包含：

```toml
[package]
name = "pinvou-cli"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true

[[bin]]
name = "pinvou"
path = "src/main.rs"

[dependencies]
anyhow.workspace = true
benchmark-core = { path = "../benchmark-core" }
clap.workspace = true
serde_json.workspace = true
tokio.workspace = true

[dev-dependencies]
assert_cmd = "2.0"
predicates = "3.1"
tempfile = "3.20"

[lints]
workspace = true
```

`agent-backend-api/Cargo.toml` 必须完整写成：

```toml
[package]
name = "agent-backend-api"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true

[dependencies]
async-trait.workspace = true
serde.workspace = true
thiserror.workspace = true

[dev-dependencies]
serde_json.workspace = true
tokio.workspace = true

[lints]
workspace = true
```

`benchmark-core/Cargo.toml` 必须完整写成：

```toml
[package]
name = "benchmark-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true

[dependencies]
agent-backend-api = { path = "../agent-backend-api" }
anyhow.workspace = true
async-trait.workspace = true
chrono.workspace = true
fs2.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
thiserror.workspace = true
tokio.workspace = true
uuid.workspace = true

[dev-dependencies]
tempfile = "3.20"

[lints]
workspace = true
```

`main.rs` 先保持最小可运行：

```rust
fn main() {
    println!("pinvou {}", env!("CARGO_PKG_VERSION"));
}
```

- [ ] **Step 4: 验证 workspace 与二进制**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli formal_binary_is_named_pinvou`

Expected: PASS。

Run: `cargo run --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli --bin pinvou`

Expected stdout: `pinvou 0.1.0`。

- [ ] **Step 5: 提交**

```powershell
git add pinvou-cli
git commit -s -m "build(cli): 创建独立 Pinvou CLI 工作区"
```

### Task 2: 固化 Feature Module Contract v1

**Files:**
- Create: `pinvou-cli/crates/benchmark-core/src/feature.rs`
- Create: `pinvou-cli/crates/benchmark-core/tests/feature_contract.rs`
- Create: `pinvou-cli/crates/benchmark-core/tests/fixtures/feature-v1.json`
- Modify: `pinvou-cli/crates/benchmark-core/src/lib.rs`

- [ ] **Step 1: 写重复注册、版本和 capability 的失败测试**

测试必须构造两个 feature，覆盖重复 feature ID、重复 CLI 命令、重复 automation action、不兼容 major 和 minor 向后兼容：

```rust
#[test]
fn registry_rejects_duplicate_and_incompatible_contracts() {
    let mut registry = FeatureRegistry::new(ContractVersion::new(1, 0));
    registry.register(fixture("benchmark", "benchmark", "benchmark.open")).unwrap();

    assert_eq!(registry.register(fixture("benchmark", "other", "other.open")).unwrap_err().code(), "duplicate_feature");
    assert_eq!(registry.register(fixture("knowledge", "benchmark", "knowledge.open")).unwrap_err().code(), "duplicate_command");
    assert_eq!(registry.register(fixture("mail", "mail", "benchmark.open")).unwrap_err().code(), "duplicate_action");
    assert_eq!(registry.register(fixture_with_version("future", 2, 0)).unwrap_err().code(), "incompatible_contract_major");
    assert!(registry.register(fixture_with_version("compatible", 1, 1)).is_ok());
}
```

Golden fixture 精确固定字段：

```json
{
  "id": "benchmark",
  "contract_version": {"major": 1, "minor": 0},
  "capabilities": {"cli": true, "gui": false, "automation": false, "headless": true, "resumable": true},
  "cli_commands": ["benchmark"],
  "automation_actions": []
}
```

- [ ] **Step 2: 运行 RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test feature_contract`

Expected: FAIL，缺少 `FeatureRegistry`、`FeatureDescriptor` 与 `ContractVersion`。

- [ ] **Step 3: 实现窄契约与 fail-closed 注册**

`feature.rs` 定义：

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContractVersion { pub major: u16, pub minor: u16 }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FeatureCapabilities {
    pub cli: bool,
    pub gui: bool,
    pub automation: bool,
    pub headless: bool,
    pub resumable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FeatureDescriptor {
    pub id: String,
    pub contract_version: ContractVersion,
    pub capabilities: FeatureCapabilities,
    pub cli_commands: Vec<String>,
    pub automation_actions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FeatureEvent {
    pub schema_version: u16,
    pub feature_id: String,
    pub event_type: String,
    pub run_id: Option<String>,
    pub safe_payload: SafePayload,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SafePayload(BTreeMap<SafeFieldName, SafeScalar>);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SafeScalar { Bool(bool), Unsigned(u64), Signed(i64), Text(SafeText) }

pub struct FeatureRegistry {
    supported: ContractVersion,
    features: BTreeMap<String, FeatureDescriptor>,
    commands: BTreeSet<String>,
    actions: BTreeSet<String>,
}
```

`register` 必须先在临时集合完成全部检查，只有所有检查通过才写入三个集合，避免部分注册。ID、命令和 action 只允许非空 ASCII 小写字母、数字、`-`、`.`，长度上限 64；major 必须等于 1，feature minor 可以大于 platform minor，因为新增 optional contract 是向后兼容的。`SafeFieldName` 使用显式 allowlist，`SafeText` 最大 512 个 Unicode scalar；Task 10 会用跨模块安全测试验证整个落盘边界。

- [ ] **Step 4: 验证行为与 golden schema**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test feature_contract`

Expected: PASS，且 golden round-trip 字节稳定。

- [ ] **Step 5: 提交**

```powershell
git add pinvou-cli/crates/benchmark-core
git commit -s -m "feat(cli): 固化业务模块契约"
```

### Task 3: 固化 Benchmark Adapter Contract v1 与静态 registry

**Files:**
- Create: `pinvou-cli/crates/benchmark-core/src/contracts.rs`
- Create: `pinvou-cli/crates/benchmark-core/src/adapter.rs`
- Create: `pinvou-cli/crates/benchmark-core/src/registry.rs`
- Create: `pinvou-cli/crates/benchmark-core/tests/adapter_contract.rs`
- Modify: `pinvou-cli/crates/benchmark-core/src/lib.rs`

- [ ] **Step 1: 用 mock adapter 写 contract test**

测试实现一个 `ContractFixtureAdapter`，要求 descriptor 使用固定 revision、两种 execution kind 均可注册、重复 adapter ID 拒绝、`main/latest` revision 拒绝：

```rust
#[test]
fn adapter_registry_accepts_only_immutable_unique_descriptors() {
    let mut registry = BenchmarkRegistry::default();
    registry.register(Arc::new(fixture_adapter("gaia", ExecutionKind::NativeTurn))).unwrap();
    registry.register(Arc::new(fixture_adapter("workbuddy", ExecutionKind::ExternalHarness))).unwrap();
    assert_eq!(registry.ids(), vec![BenchmarkId::new("gaia").unwrap(), BenchmarkId::new("workbuddy").unwrap()]);
    assert_eq!(registry.register(Arc::new(fixture_adapter("gaia", ExecutionKind::NativeTurn))).unwrap_err().code(), "duplicate_adapter");
    assert_eq!(fixture_descriptor("floating", "latest").validate().unwrap_err().code(), "mutable_revision");
}
```

- [ ] **Step 2: 运行 RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test adapter_contract`

Expected: FAIL，缺少 adapter 契约。

- [ ] **Step 3: 实现完整 v1 类型**

`contracts.rs` 必须定义并 serde 固定：

```rust
pub enum ExecutionKind { NativeTurn, ExternalHarness }
pub struct BenchmarkDescriptor { pub id: BenchmarkId, pub adapter_version: String, pub dataset_revision: String, pub scorer_revision: String, pub supported_splits: Vec<Split>, pub execution_kind: ExecutionKind }
pub struct TaskSelection { pub split: Split, pub filters: BTreeMap<String, String>, pub pass: u16 }
pub struct BenchmarkPlan { pub descriptor: BenchmarkDescriptor, pub tasks: Vec<BenchmarkTask> }
pub struct BenchmarkTask { pub task_id: TaskId, pub category: Option<String>, pub level: Option<String>, pub execution: ExecutionRequest, pub reference_handle: Option<ReferenceHandle> }
pub enum ExecutionRequest {
    NativeTurn { prompt_handle: PrivateInputHandle, attachments: Vec<AttachmentHandle>, timeout_ms: u64, tool_policy: ToolPolicyId, output_contract: OutputContract },
    ExternalHarness { workspace_archive: VerifiedArtifact, container_image_digest: String, harness_command: Vec<String>, timeout_ms: u64 },
}
pub struct TaskOutcome { pub task_id: TaskId, pub status: TaskStatus, pub prediction: Option<Prediction>, pub artifacts: Vec<ArtifactReference>, pub usage: Option<UsageMetrics>, pub elapsed_ms: u64, pub trajectory_ref: Option<PrivateArtifactHandle>, pub failure_category: Option<SafeFailureCategory> }
pub struct VerifiedDataset { pub handle: DatasetHandle, pub fingerprint: String, pub has_local_ground_truth: bool }
pub struct PreparedTask { pub task: BenchmarkTask, pub executor_view: ExecutorViewHandle, pub scorer_reference: Option<ReferenceHandle> }
pub struct RunContext { pub run_id: RunId, pub private_workspace: PrivateWorkspaceHandle }
pub struct CompletedRun { pub run_id: RunId, pub manifest: RunManifest, pub outcomes: Vec<TaskOutcome>, pub scorer_view: ScorerViewHandle }
pub struct DatasetFetchRequest { pub revision: String, pub destination: DatasetDestinationHandle }
pub struct DatasetSnapshot { pub handle: DatasetHandle, pub fingerprint: String }
```

所有 `*Handle` 是经过验证的 opaque ID，不是绝对路径；`Prediction` 为 `kind + PrivateArtifactHandle`，禁止在通用领域对象中放任意 `serde_json::Value`。`SafeFailureCategory` 只允许固定枚举 `BackendUnavailable|InvalidTask|PermissionDenied|Timeout|Cancelled|ExecutionFailed|OutputContractMissing`。

`BenchmarkAdapter` 使用同步数据准备/评分契约：

```rust
#[async_trait]
pub trait BenchmarkAdapter: Send + Sync {
    fn descriptor(&self) -> &BenchmarkDescriptor;
    async fn fetch_dataset(&self, request: DatasetFetchRequest) -> Result<DatasetSnapshot, PlatformError>;
    fn verify_dataset(&self, dataset_root: &Path) -> Result<VerifiedDataset, PlatformError>;
    fn plan(&self, dataset: &VerifiedDataset, selection: &TaskSelection) -> Result<BenchmarkPlan, PlatformError>;
    fn prepare_task(&self, task: &BenchmarkTask, run: &RunContext) -> Result<PreparedTask, PlatformError>;
    fn score(&self, run: &CompletedRun) -> Result<OfficialScoreReport, PlatformError>;
    fn write_submission(&self, run: &CompletedRun, destination: &Path) -> Result<SubmissionArtifact, PlatformError>;
}
```

`fetch_dataset` 不接收 token 字符串或任意 headers。具体 gated adapter 只能从自己的 credential provider 读取命名凭据，且它的 CLI adapter 不得暴露 token 参数。无网络下载能力的 adapter 返回固定 `fetch_unsupported`。revision 的合法形式限定为：40/64 位十六进制 commit/checksum，`sha256:<64-hex>`，或 `v<major>.<minor>.<patch>`；`main`、`master`、`latest`、分支名和空值全部拒绝。

`BenchmarkRegistry` 只接受显式 `Arc<dyn BenchmarkAdapter>`；无目录扫描、动态库、脚本或 YAML 执行。

- [ ] **Step 4: 验证 adapter contract**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test adapter_contract`

Expected: PASS，adapter ID 按字典序稳定输出。

- [ ] **Step 5: 提交**

```powershell
git add pinvou-cli/crates/benchmark-core
git commit -s -m "feat(eval): 固化评测 Adapter 契约"
```

### Task 4: 实现安全的 Headless Agent Contract v1

**Files:**
- Create: `pinvou-cli/crates/agent-backend-api/src/types.rs`
- Create: `pinvou-cli/crates/agent-backend-api/src/observer.rs`
- Create: `pinvou-cli/crates/agent-backend-api/src/backend.rs`
- Create: `pinvou-cli/crates/agent-backend-api/tests/backend_contract.rs`
- Modify: `pinvou-cli/crates/agent-backend-api/src/lib.rs`

- [ ] **Step 1: 写 no-op、panic containment 与关闭契约测试**

```rust
#[tokio::test]
async fn observer_failure_cannot_change_backend_outcome() {
    let observer: Arc<dyn AgentRunObserver> = Arc::new(PanickingObserver);
    let event = SafeAgentEvent::new("turn.completed", SafeAgentPayload::TurnCompleted { elapsed_ms: 12 });
    assert_eq!(emit_observer_event(&observer, &event), ObserverDelivery::Panicked);
    assert_eq!(event.event_type(), "turn.completed");
}

#[test]
fn safe_event_has_no_free_form_secret_fields() {
    let json = serde_json::to_string(&SafeAgentEvent::new(
        "tool.completed",
        SafeAgentPayload::ToolCompleted { canonical_tool: "web_search".into(), failed: false, elapsed_ms: 9 },
    )).unwrap();
    assert!(!json.contains("prompt"));
    assert!(!json.contains("input"));
    assert!(!json.contains("output"));
    assert!(!json.contains("token"));
}
```

- [ ] **Step 2: 运行 RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p agent-backend-api --test backend_contract`

Expected: FAIL，缺少 backend/observer 类型。

- [ ] **Step 3: 实现只读接口**

```rust
#[async_trait]
pub trait HeadlessAgentBackend: Send + Sync {
    async fn prepare(&self, request: PrepareRequest) -> Result<AgentSessionHandle, BackendError>;
    async fn run(&self, session: &AgentSessionHandle, task: AgentTaskInput, observer: Arc<dyn AgentRunObserver>) -> Result<AgentTaskOutcome, BackendError>;
    async fn cancel(&self, session: &AgentSessionHandle) -> Result<(), BackendError>;
    async fn close(&self, session: AgentSessionHandle) -> Result<(), BackendError>;
}

pub trait AgentRunObserver: Send + Sync {
    fn on_event(&self, event: &SafeAgentEvent);
}
```

`SafeAgentPayload` 只能是结构化枚举：`SessionPrepared`、`TurnStarted`、`FirstDelta`、`ToolStarted`、`ToolCompleted`、`TurnCompleted`、`TurnFailed`。不得包含 prompt、完整 response、工具参数/结果、路径、provider error、credential。`emit_observer_event` 使用 `catch_unwind(AssertUnwindSafe(...))`，只返回 delivery 状态，绝不传播 observer panic。

- [ ] **Step 4: 验证所有 backend contract tests**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p agent-backend-api`

Expected: PASS。

- [ ] **Step 5: 提交**

```powershell
git add pinvou-cli/crates/agent-backend-api
git commit -s -m "feat(cli): 增加安全无头 Agent 接口"
```

### Task 5: 实现不可变 Run Manifest 与安全路径

**Files:**
- Create: `pinvou-cli/crates/benchmark-core/src/security.rs`
- Create: `pinvou-cli/crates/benchmark-core/src/manifest.rs`
- Create: `pinvou-cli/crates/benchmark-core/tests/fixtures/manifest-v1.json`
- Create: `pinvou-cli/crates/benchmark-core/tests/manifest_contract.rs`
- Modify: `pinvou-cli/crates/benchmark-core/src/lib.rs`

- [ ] **Step 1: 写不可覆盖、无凭据、不可浮动 revision 的测试**

```rust
#[test]
fn manifest_is_immutable_and_secret_free() {
    let temp = tempfile::tempdir().unwrap();
    let paths = PlatformPaths::from_base(temp.path()).unwrap();
    let manifest = fixture_manifest();
    let run = RunDirectory::create(&paths, &manifest).unwrap();
    assert_eq!(RunDirectory::create_with_id(&paths, run.run_id(), &manifest).unwrap_err().code(), "run_exists");

    let bytes = std::fs::read(run.manifest_path()).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.to_ascii_lowercase().contains("token"));
    assert!(!text.to_ascii_lowercase().contains("cookie"));
    assert!(!text.to_ascii_lowercase().contains("authorization"));
}
```

- [ ] **Step 2: 运行 RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test manifest_contract`

Expected: FAIL，缺少 manifest/store path 类型。

- [ ] **Step 3: 实现 schema v1 与 atomic create-new**

`RunManifest` 精确包含：schema_version、run_id、benchmark、adapter/dataset/scorer revision、split、filters、safe model identity、tool policy、frontend、concurrency、pass、retry policy、created_at。`SafeModelIdentity` 只有 provider/model；禁止 endpoint、api_key、headers。

`PlatformPaths::from_base(base, &PathPolicy { home, repository_root })` 必须：创建 base 后 canonicalize；拒绝文件、文件系统根、用户 home 本身和仓库根；派生路径只能通过已验证 `RunId` join。`RunDirectory::create` 先用 `create_dir` 独占创建 run 目录，再以 `create_new(true)` 写 `manifest.json.tmp`、`sync_all`，最后用 hard-link/create-new 语义无覆盖发布 `manifest.json` 并删除自己的 tmp。任何失败只清理本次创建且已确认位于 `runs/` 下的目录。

- [ ] **Step 4: 验证 manifest golden 与路径攻击**

增加 `..`、绝对 run ID、symlink escape、重复 run、`latest` revision、credential-shaped model string 的拒绝测试。

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test manifest_contract`

Expected: PASS，`manifest-v1.json` round-trip 稳定。

- [ ] **Step 5: 提交**

```powershell
git add pinvou-cli/crates/benchmark-core
git commit -s -m "feat(eval): 增加不可变运行清单"
```

### Task 6: 实现追加事件、lease 与确定性恢复

**Files:**
- Create: `pinvou-cli/crates/benchmark-core/src/event.rs`
- Create: `pinvou-cli/crates/benchmark-core/src/store.rs`
- Create: `pinvou-cli/crates/benchmark-core/tests/recovery_contract.rs`
- Modify: `pinvou-cli/crates/benchmark-core/src/lib.rs`

- [ ] **Step 1: 写状态机和崩溃恢复测试**

```rust
#[test]
fn resume_runs_only_tasks_without_durable_terminal_outcome() {
    let store = fixture_store_with_tasks(["done", "running", "planned"]);
    store.append_outcome(&fixture_outcome("done")).unwrap();
    store.append_event(&RunEvent::completed("done")).unwrap();
    store.append_event(&RunEvent::lease_acquired("running", "worker-a", 100)).unwrap();

    let state = store.recover(101).unwrap();
    assert_eq!(state.completed_task_ids(), vec![TaskId::new("done").unwrap()]);
    assert_eq!(state.runnable_task_ids(), vec![TaskId::new("planned").unwrap(), TaskId::new("running").unwrap()]);
}
```

另写测试证明：没有 outcome 的 `completed` 事件属于损坏；有 outcome 但进程在 completed event 前崩溃时，恢复逻辑补写/识别完成且不重复模型调用；活动 lease 阻止第二 worker。

- [ ] **Step 2: 运行 RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test recovery_contract`

Expected: FAIL，缺少 RunEvent、EventStore 和 reducer。

- [ ] **Step 3: 实现 append-only store**

事件 schema：

```rust
pub enum RunEventKind {
    Planned,
    LeaseAcquired { worker_id: String, expires_at_ms: i64 },
    Running,
    Completed,
    Failed { category: SafeFailureCategory },
    TimedOut,
    Cancelled,
}
```

`EventStore::append_event` 与 `append_outcome` 使用同进程 mutex + OS advisory lock，单行 JSON 后 `flush`/`sync_data`。顺序固定为 outcome durable → terminal event durable。reducer 拒绝非法转换和重复 terminal；expired lease 才允许接管。恢复配置必须与 manifest 完整相等，不接受命令行覆盖 revision、split、filter、model、tool policy、pass、concurrency。

- [ ] **Step 4: 验证恢复与并发 lease**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test recovery_contract -- --test-threads=1`

Expected: PASS。

- [ ] **Step 5: 提交**

```powershell
git add pinvou-cli/crates/benchmark-core
git commit -s -m "feat(eval): 增加可恢复事件存储"
```

### Task 7: 实现 NativeTurn 与 ExternalHarness 双 Runner

**Files:**
- Create: `pinvou-cli/crates/benchmark-core/src/runner.rs`
- Create: `pinvou-cli/crates/benchmark-core/tests/runner_contract.rs`
- Modify: `pinvou-cli/crates/benchmark-core/Cargo.toml`
- Modify: `pinvou-cli/crates/benchmark-core/src/lib.rs`

- [ ] **Step 1: 写执行类型隔离测试**

```rust
#[tokio::test]
async fn dispatcher_never_sends_external_task_to_native_backend() {
    let native = Arc::new(RecordingNativeRunner::default());
    let external = Arc::new(RecordingExternalRunner::default());
    let dispatcher = RunnerDispatcher::new(native.clone(), external.clone());

    dispatcher.run(&external_fixture_task(), &fixture_context()).await.unwrap();
    assert_eq!(native.calls(), 0);
    assert_eq!(external.calls(), 1);
}
```

增加 timeout 触发 cancel+close、native observer panic 不改变结果、external harness 没有 sandbox capability 时拒绝、harness command 不经 shell 拼接的测试。

- [ ] **Step 2: 运行 RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test runner_contract`

Expected: FAIL，缺少 runner traits 和 dispatcher。

- [ ] **Step 3: 实现双 Runner 契约**

```rust
#[async_trait]
pub trait NativeTurnRunner: Send + Sync {
    async fn run_native(&self, request: PreparedNativeTurn, observer: Arc<dyn AgentRunObserver>) -> Result<TaskOutcome, PlatformError>;
}

#[async_trait]
pub trait ExternalHarnessRunner: Send + Sync {
    async fn run_external(&self, request: PreparedExternalHarness) -> Result<TaskOutcome, PlatformError>;
}
```

`RunnerDispatcher` 只按 `ExecutionRequest` enum 分发，不按 benchmark ID 分支。External 请求必须携带 `SandboxCapability::ContainerIsolated` 和不可变 image digest；命令为 `Vec<OsString>` 直接传进程 API，禁止 `cmd /c`、`sh -c` 或字符串拼接。Native timeout 后执行 cancel，并始终 close；close 失败映射固定安全类别，不复制 backend error。

- [ ] **Step 4: 验证两类 mock runner**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test runner_contract`

Expected: PASS。

- [ ] **Step 5: 提交**

```powershell
git add pinvou-cli/crates/benchmark-core
git commit -s -m "feat(eval): 增加双执行后端调度"
```

### Task 8: 完成 BenchmarkService 端到端编排

**Files:**
- Create: `pinvou-cli/crates/benchmark-core/src/service.rs`
- Create: `pinvou-cli/crates/benchmark-core/tests/service_contract.rs`
- Modify: `pinvou-cli/crates/benchmark-core/src/lib.rs`

- [ ] **Step 1: 写 mock adapter 端到端测试**

测试必须执行 verify → plan → manifest → run → outcome → score → report，并模拟第二次 resume：

```rust
#[tokio::test]
async fn completed_task_is_not_reexecuted_on_resume() {
    let fixture = ServiceFixture::new_two_tasks();
    let first = fixture.service.run(fixture.request.clone()).await.unwrap();
    assert_eq!(first.summary.completed, 2);
    assert_eq!(fixture.runner.calls(), 2);

    let resumed = fixture.service.resume(first.run_id.clone()).await.unwrap();
    assert_eq!(resumed.summary.completed, 2);
    assert_eq!(fixture.runner.calls(), 2);
}
```

增加 runner failure 继续其他 task、默认 concurrency=1、重试原因进入 manifest、private test `has_local_ground_truth=false` 时 score 拒绝但 submission 允许的测试。

- [ ] **Step 2: 运行 RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test service_contract`

Expected: FAIL，缺少 BenchmarkService。

- [ ] **Step 3: 实现编排服务**

公开服务方法固定为：

```rust
impl BenchmarkService {
    pub fn list(&self) -> Vec<BenchmarkDescriptor>;
    pub async fn fetch(&self, benchmark: BenchmarkId, request: DatasetFetchRequest) -> Result<DatasetSnapshot, PlatformError>;
    pub fn verify(&self, request: VerifyRequest) -> Result<VerifiedDataset, PlatformError>;
    pub async fn run(&self, request: RunRequest) -> Result<RunSummary, PlatformError>;
    pub async fn resume(&self, run_id: RunId) -> Result<RunSummary, PlatformError>;
    pub fn status(&self, run_id: RunId) -> Result<RunSummary, PlatformError>;
    pub fn score(&self, run_id: RunId) -> Result<OfficialScoreReport, PlatformError>;
    pub fn report(&self, run_id: RunId) -> Result<ReportArtifact, PlatformError>;
    pub fn submission(&self, request: SubmissionRequest) -> Result<SubmissionArtifact, PlatformError>;
}
```

`submission` 默认只写本地文件；上传不是这个 API 的副作用。任何未来 upload 方法必须独立且要求显式确认。Service 不读取 adapter 私有 reference，不序列化 prepared task，不把 adapter/backend error 原文写入 store。

- [ ] **Step 4: 验证完整 mock 生命周期**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test service_contract -- --test-threads=1`

Expected: PASS，临时目录没有 `.tmp`、没有未释放 lease。

- [ ] **Step 5: 提交**

```powershell
git add pinvou-cli/crates/benchmark-core
git commit -s -m "feat(eval): 完成评测编排服务"
```

### Task 9: 接入统一 `pinvou benchmark` 命令与稳定退出码

**Files:**
- Create: `pinvou-cli/crates/cli/src/app.rs`
- Create: `pinvou-cli/crates/cli/src/exit.rs`
- Create: `pinvou-cli/crates/cli/src/output.rs`
- Create: `pinvou-cli/crates/cli/src/commands/mod.rs`
- Create: `pinvou-cli/crates/cli/src/commands/benchmark.rs`
- Create: `pinvou-cli/crates/cli/tests/cli_contract.rs`
- Modify: `pinvou-cli/crates/cli/src/lib.rs`
- Modify: `pinvou-cli/crates/cli/src/main.rs`

- [ ] **Step 1: 写 CLI contract tests**

```rust
#[test]
fn benchmark_command_surface_is_stable() {
    Command::cargo_bin("pinvou").unwrap()
        .arg("benchmark").arg("--help")
        .assert().success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("fetch"))
        .stdout(predicate::str::contains("verify"))
        .stdout(predicate::str::contains("run"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("resume"))
        .stdout(predicate::str::contains("score"))
        .stdout(predicate::str::contains("report"))
        .stdout(predicate::str::contains("submission"));
}

#[test]
fn unknown_adapter_is_structured_and_never_prints_credentials() {
    Command::cargo_bin("pinvou").unwrap()
        .args(["benchmark", "verify", "unknown", "--output", "json"])
        .assert().code(4)
        .stdout(predicate::str::contains("adapter_not_found"))
        .stdout(predicate::str::contains("token").not());
}
```

- [ ] **Step 2: 运行 RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli --test cli_contract`

Expected: FAIL，当前二进制没有 benchmark 子命令。

- [ ] **Step 3: 实现完整命令树**

clap 顶层：

```rust
#[derive(Parser)]
#[command(name = "pinvou", version, about = "Pinvou Agent command line interface")]
pub struct Cli {
    #[command(subcommand)] pub command: TopLevelCommand,
}

#[derive(Subcommand)]
pub enum TopLevelCommand {
    Benchmark(BenchmarkArgs),
    Version,
}
```

本阶段 `chat`、`agent`、`config` 不注册占位命令，避免宣称不可用功能；它们由真实后端阶段新增。Benchmark 的九个子命令全部解析到 typed request。输出模式固定 `human|json`，stdout 只放结果，stderr 只放诊断。退出码：0 成功、2 参数错误、3 run/adapter 不存在、4 数据验证失败、5 执行失败、6 score/submission 不可用、7 安全边界拒绝、70 内部错误。不得接受 Token/Cookie/API key 命令行参数。

`build_registry()` 采用显式静态函数，当前返回空 registry。后续 adapter 只增加：

```rust
registry.register(Arc::new(adapter_gaia::GaiaAdapter::new(config.gaia)?))?;
```

- [ ] **Step 4: 验证 CLI contract 与 help snapshot**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli --test cli_contract`

Expected: PASS。

Run: `cargo run --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli -- benchmark list --output json`

Expected stdout: `{"benchmarks":[]}`，exit 0。

- [ ] **Step 5: 提交**

```powershell
git add pinvou-cli/crates/cli
git commit -s -m "feat(cli): 接入统一 benchmark 命令"
```

### Task 10: 冻结事件、outcome、隐私与 Adapter golden contracts

**Files:**
- Create: `pinvou-cli/crates/benchmark-core/tests/fixtures/outcome-v1.json`
- Create: `pinvou-cli/crates/benchmark-core/tests/security_contract.rs`
- Create: `pinvou-cli/tests/forbidden-data-markers.txt`
- Modify: `pinvou-cli/crates/benchmark-core/tests/adapter_contract.rs`
- Modify: `pinvou-cli/crates/benchmark-core/tests/feature_contract.rs`

- [ ] **Step 1: 加入会使现实现暴露边界缺口的测试**

`forbidden-data-markers.txt` 精确包含通用持久化中不允许出现的字段名：

```text
authorization
cookie
api_key
access_token
ground_truth
reference_answer
hidden_test
raw_prompt
tool_input
tool_output
```

测试构造每个字段的 sentinel，分别尝试进入 manifest、FeatureEvent、TaskOutcome 和 PlatformError，最终递归扫描 run directory，断言 sentinel 与字段名均不存在。另断言 unknown event 可以被 reader 忽略，但 malformed/oversize event 拒绝追加。

- [ ] **Step 2: 运行 RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test security_contract`

Expected: 至少一个 FAIL，指出缺少统一 event payload 大小门禁或错误文本未收口。

- [ ] **Step 3: 实现统一安全收口**

收紧 Task 2 已定义的 `SafePayload`：禁止嵌套任意 JSON，完整 event 序列化后最大 16 KiB。所有落盘前统一调用 `validate_for_persistence()`；`PlatformError` 持久化只写固定 code，不写 source/display 文本。

`outcome-v1.json` 只保留 task ID、status、prediction handle、artifact handles、usage 数值、elapsed 和安全 failure category；不含 trajectory 内容或实际路径。

- [ ] **Step 4: 验证全部 contract suites**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml --workspace --all-targets -- --test-threads=1`

Expected: PASS。

- [ ] **Step 5: 提交**

```powershell
git add pinvou-cli
git commit -s -m "test(eval): 冻结通用评测安全契约"
```

### Task 11: 增加依赖边界守卫与 path-scoped CI

**Files:**
- Create: `pinvou-cli/scripts/check_boundaries.py`
- Create: `.github/workflows/pinvou-cli.yml`

- [ ] **Step 1: 先写会检测违规 fixture 的守卫自测**

`check_boundaries.py --self-test` 在临时目录建立三份 manifests，分别模拟 adapter→adapter、core→Tauri、agent-api→CodeWhale 依赖，必须返回失败；合法的 cli→core/api 与 adapter→core 返回成功。

- [ ] **Step 2: 运行 RED**

Run: `python pinvou-cli/scripts/check_boundaries.py --self-test`

Expected: FAIL，因为脚本尚不存在。

- [ ] **Step 3: 实现 metadata + source boundary 检查**

脚本执行 `cargo metadata --manifest-path pinvou-cli/Cargo.toml --no-deps --format-version 1`，强制：

```text
benchmark-core        -> 不得依赖 cli、adapter-*、pinvou3-tauri、codewhale-*
agent-backend-api     -> 不得依赖 benchmark-core、cli、adapter-*、pinvou3-tauri、codewhale-*
adapter-*             -> 只可依赖 benchmark-core、agent-backend-api 和第三方 crate，不得互相依赖
pinvou-cli            -> 可依赖 benchmark-core、agent-backend-api、adapter-*
```

并扫描 `pinvou-cli/` 源码，拒绝 `../pinvou3-app`、`../CodeWhale` path dependency、动态库加载 API 与 shell 拼接执行。只扫描源文件，不读取数据集目录。

CI workflow 仅在 `pinvou-cli/**`、自身 workflow 或根 `rust-toolchain.toml` 变化时触发，步骤固定为 checkout、Rust 1.97.1、`cargo fmt --check`、boundary check、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace --all-targets -- --test-threads=1`。不得下载 GAIA 或要求 credential。

- [ ] **Step 4: 验证守卫和现有架构守卫**

Run: `python pinvou-cli/scripts/check_boundaries.py --self-test`

Expected: PASS。

Run: `python pinvou-cli/scripts/check_boundaries.py`

Expected: PASS。

Run: `python scripts/architecture-guard.py`

Expected: PASS，现有 app 架构 debt 不增加。

- [ ] **Step 5: 提交**

```powershell
git add pinvou-cli/scripts .github/workflows/pinvou-cli.yml
git commit -s -m "ci(eval): 增加独立评测平台门禁"
```

### Task 12: 完成文档、兼容审计与 Adapter 并行开发验收

**Files:**
- Create: `pinvou-cli/README.md`
- Create: `pinvou-cli/docs/adapter-contract-v1.md`
- Create: `pinvou-cli/docs/security-model.md`
- Modify: `pinvou-cli/Cargo.toml`

- [ ] **Step 1: 写文档契约检查**

在 `benchmark-core/tests/adapter_contract.rs` 增加检查，读取 `docs/adapter-contract-v1.md`，断言存在以下精确章节：`Contract v1`、`Static registration`、`Ground truth isolation`、`NativeTurnRunner`、`ExternalHarnessRunner`、`Resume invariants`、`Security review checklist`、`GAIA adapter`、`BFCL adapter`、`WorkBuddy adapter`。

- [ ] **Step 2: 运行 RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test adapter_contract adapter_author_guide_covers_v1_invariants`

Expected: FAIL，文档不存在。

- [ ] **Step 3: 编写可直接执行的 Adapter 作者指南**

`adapter-contract-v1.md` 必须明确：

1. adapter crate 只依赖 core/api，不依赖 CLI 或其他 adapter；
2. dataset/scorer revision 必须固定 SHA/checksum；
3. gated credential 只从 credential store/env 读取，禁止 CLI 参数；
4. executor view 与 scorer-only view 分离；
5. private test 无 ground truth 时 `score` 返回 `local_scoring_unavailable`；
6. NativeTurn 与 ExternalHarness 的选择规则；
7. adapter 不得创建自己的 run store/resume/report 系统；
8. 需要新增 core 能力时先改单独 core proposal 和 contract tests；
9. GAIA 只新增 `adapter-gaia`，BFCL 只新增 `adapter-bfcl`，WorkBuddy 只新增 `adapter-workbuddy`；
10. registry composition root 是唯一允许的共享修改点。

`README.md` 给出构建、九个 benchmark 命令、退出码、数据目录和“当前 registry 为空，adapter 在独立阶段接入”的准确说明。`security-model.md` 给出 threat model、信任边界、路径策略、删除策略、事件 allowlist、ground truth 与 submission 边界。

- [ ] **Step 4: 做最终验证与旧功能零影响审计**

Run: `cargo fmt --manifest-path pinvou-cli/Cargo.toml --all -- --check`

Expected: PASS。

Run: `cargo clippy --manifest-path pinvou-cli/Cargo.toml --workspace --all-targets -- -D warnings`

Expected: PASS。

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml --workspace --all-targets -- --test-threads=1`

Expected: PASS。

Run: `python pinvou-cli/scripts/check_boundaries.py && python scripts/architecture-guard.py`

Expected: 两个命令均 PASS。

Run: `git diff --name-only 7804f252..HEAD`

Expected: 只有 `pinvou-cli/**` 与 `.github/workflows/pinvou-cli.yml`；不得出现 `pinvou3-app/**`、`CodeWhale`、`.gitmodules`、GAIA 数据文件或 `eval_smoke` 变更。

Run: `git status --short`

Expected: 空。

- [ ] **Step 5: 提交文档**

```powershell
git add pinvou-cli
git commit -s -m "docs(eval): 记录 Adapter 并行开发契约"
```

## 完成定义

只有以下条件全部成立，才可以宣布“通用 Adapter 层开发完，可以并行开发 GAIA/BFCL/WorkBuddy”：

- 独立 workspace 全量 fmt/clippy/test 通过；
- CLI 九个 benchmark 子命令的 parse/output/exit-code contract 通过；
- mock NativeTurn 与 ExternalHarness 端到端 run/resume/score/report 通过；
- duplicate registry、major version、mutable revision、path traversal、secret persistence、ground-truth leakage 测试通过；
- completed task 的 resume 不产生第二次 runner 调用；
- private test 在本地 score 时明确拒绝、submission 仍可生成；
- boundary guard 证明 common core 不依赖 app/CodeWhale/具体 adapter，adapter 之间不能互相依赖；
- `pinvou3-app`、CodeWhale、现有 `eval_smoke` 没有代码或行为变化；
- 文档明确 adapter 只能修改自身 crate 和静态 registry composition root；
- 实际未运行或伪造任何 GAIA/BFCL/WorkBuddy 官方成绩。

## 后续独立实施计划

通用层通过上述完成定义后，按以下互不阻塞的计划继续：

1. `pinvou-headless-bridge`：只在 `pinvou3-app` 新增窄桥接模块和只读 observer 埋点；
2. `gaia-official-adapter`：固定官方数据/scorer revision，validation Level 1，pass@1；
3. `bfcl-official-adapter`：结构化 function-call prediction 与官方 scorer；
4. `workbuddy-official-adapter`：Docker/Harbor External Harness；
5. `pinvou-gui-automation`：隔离 Test GUI、认证 IPC 和 feature semantic drivers。

这些计划不得复制本计划的 registry、manifest、resume、runner、event、report 或安全系统。
