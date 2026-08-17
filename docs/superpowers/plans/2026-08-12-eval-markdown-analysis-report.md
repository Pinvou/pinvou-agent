# Eval Markdown Analysis Report Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为每次 Product smoke 自动生成带确定性问题诊断、独立 Judge 评分和优先级建议的同名 Markdown 报告，同时保持原始评测内容不进入 JSONL/Markdown。

**Architecture:** 在现有 runner 中保留仅驻内存的分析材料，先由纯规则模块产出可复现 findings，再由绑定独立保存模型的 Judge adapter 产出结构化评分，最后合并为稳定报告模型并原子写入 `.md`。JSONL 仍是增量事实源；Judge 失败降级为规则报告，Markdown 交付失败则命令返回非零。

**Tech Stack:** Rust 2021、Tauri 2、serde/serde_json、anyhow、tokio、现有 EnginePool/ProductChatRuntime、Markdown 文件输出。

---

## File structure

- Create `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/mod.rs`: 分析领域类型、finding 合并、优先级排序和报告级摘要。
- Create `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/rules.rs`: 无网络的确定性规则和阈值。
- Create `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/judge.rs`: Judge schema、prompt、模型响应解析和可降级 adapter。
- Create `pinvou3-app/src-tauri/src/features/assistant/eval/markdown_report.rs`: Markdown 渲染、脱敏、同 basename 原子写入。
- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs`: 工具期望、仅内存分析材料、suite 分析入口和新模块导出。
- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/cases.rs`: 为 smoke case 声明工具使用期望。
- Modify `pinvou3-app/src-tauri/src/features/assistant/product_runtime/mod.rs`: 支持可选 session 模型、从临时 session 提取最终回答和工具摘要。
- Modify `pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs`: 按指定 saved model 创建 Judge 临时 session，并提供只读 transcript 快照。
- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/mock.rs`: 为新 runtime 结果和 Judge 测试提供确定性材料。
- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/report.rs`: complete 行增加可选分析状态/Markdown 路径，保留旧字段。
- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`: runner、规则、Judge、Markdown、隐私和降级覆盖。
- Modify `pinvou3-app/src-tauri/src/eval_cli.rs`: 编排规则、Judge、JSONL 与 Markdown 终结流程。
- Modify `pinvou3-app/src-tauri/src/bin/eval_smoke.rs`: 解析 `--judge-model-id` 并打印两个报告路径。
- Modify `pinvou3-app/src-tauri/src/app/commands/eval.rs`: GUI 路径复用规则报告，未指定 Judge 时明确降级。
- Modify `PROGRESS.md`: 记录运行方式、报告结构和验证结果。

### Task 1: Add ephemeral analysis material to runner results

**Files:**
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/cases.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/product_runtime/mod.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/mock.rs`
- Test: `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`

- [ ] **Step 1: Run impact analysis before changing runtime and record symbols**

Run GitNexus upstream impact for `EvalRecord`, `TurnResult`, `SessionSpec`, `EnginePoolRuntime::wait_for_completion`, and `EnginePool::prepare_eval_session`. Record direct callers and affected flows. If GitNexus remains unavailable in this worktree, run:

```powershell
rg -n "EvalRecord|TurnResult|SessionSpec|wait_for_completion|prepare_eval_session" pinvou3-app/src-tauri/src -g '*.rs'
```

Expected: changes are limited to eval runner, mock runtime, GUI eval command, CLI eval composition root, and EnginePool's eval-only preparation method. Warn before editing if an impact result is HIGH or CRITICAL.

- [ ] **Step 2: Write failing serialization and extraction tests**

Add tests that construct a completed record with secret-looking analysis text and prove serialization omits it:

```rust
#[test]
fn eval_record_serialization_omits_analysis_material() {
    let mut record = completed_record("privacy");
    record.analysis = EvalAnalysisMaterial {
        user_message: "secret prompt".into(),
        assistant_text: "secret answer".into(),
        tool_events: vec![EvalToolEvent {
            name: "web_search".into(),
            failed: false,
        }],
    };
    let json = serde_json::to_string(&record).expect("serialize record");
    assert!(!json.contains("secret prompt"));
    assert!(!json.contains("secret answer"));
    assert!(!json.contains("tool_events"));
}
```

Add a focused helper test using `deepseek_tui::models::Message` blocks that proves the last assistant text and tool result error flags are extracted without copying tool inputs or outputs.

- [ ] **Step 3: Run tests to verify they fail**

Run:

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib eval_record_serialization_omits_analysis_material -- --nocapture
```

Expected: compile failure because `EvalAnalysisMaterial`, `EvalToolEvent`, and `EvalRecord::analysis` do not exist. On the known Windows test-loader environment, use `--no-run` after observing the compile failure; do not claim execution success if the executable returns `0xc0000139`.

- [ ] **Step 4: Add the minimal in-memory types and runtime fields**

Define in `eval/mod.rs`:

```rust
#[derive(Debug, Clone, Default)]
pub struct EvalAnalysisMaterial {
    pub user_message: String,
    pub assistant_text: String,
    pub tool_events: Vec<EvalToolEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalToolEvent {
    pub name: String,
    pub failed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExpectation {
    Forbidden,
    Optional,
    Required,
}
```

Add `tool_expectation: ToolExpectation` to `EvalCase`. `EvalCase::smoke` defaults it to `Optional`; set the named PLEP `weather` case to `Required`, `date` to `Optional`, and `hi`, `math`, `poem` to `Forbidden`. Add this skipped field to `EvalRecord`:

```rust
#[serde(skip)]
pub analysis: EvalAnalysisMaterial,
```

Define `RuntimeToolEvent { name: String, failed: bool }` in `product_runtime/mod.rs`, then extend `TurnResult` with `assistant_text: String` and `tool_events: Vec<RuntimeToolEvent>`. Keep `product_runtime` independent of eval; implement `From<RuntimeToolEvent> for EvalToolEvent` in `eval/mod.rs`. Add an EnginePool eval-only read method returning a cloned `Vec<Message>` from `SessionStore::load(session_id)`. In `EnginePoolRuntime::wait_for_completion`, extract:

- the text blocks from the last `role == "assistant"` message;
- each `ToolUse` name;
- `ToolResult.is_error == Some(true)` matched by tool ID;
- no tool input or result content.

Populate `EvalRecord.analysis` in `to_record`; error and timeout records use the case prompt plus empty response/tool lists.

- [ ] **Step 5: Update the mock and existing constructors**

Add to `MockConfig`:

```rust
pub assistant_text: String,
pub tool_events: Vec<EvalToolEvent>,
```

Default to `"mock answer"` and an empty vector. Update every `EvalRecord`, `EvalCase`, `SessionSpec`, and `TurnResult` literal in the crate so it supplies the new fields.

- [ ] **Step 6: Run focused compile and tests**

Run:

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib eval_record_serialization_omits_analysis_material --no-run
rustup run stable-x86_64-pc-windows-msvc cargo check --bin eval_smoke --features dev-tools
```

Expected: both commands exit 0; JSON serialization still contains all existing `EvalRecord` fields and none of the analysis material.

- [ ] **Step 7: Scope-check and commit**

Run `detect_changes({scope: "compare", base_ref: "main"})`; if unavailable, inspect `git diff --` for the six files above. Then commit:

```powershell
git add pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs pinvou3-app/src-tauri/src/features/assistant/eval/cases.rs pinvou3-app/src-tauri/src/features/assistant/product_runtime/mod.rs pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs pinvou3-app/src-tauri/src/features/assistant/eval/mock.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs
git commit -s -m "feat(eval): 采集仅驻内存的分析材料"
```

### Task 2: Implement deterministic rule analysis

**Files:**
- Create: `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/mod.rs`
- Create: `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/rules.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs`
- Test: `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`

- [ ] **Step 1: Write failing rule tests with exact thresholds**

Cover these cases:

```rust
#[test]
fn rules_prioritize_failed_cases_as_p0() {
    let cases = vec![EvalCase::smoke("failed", "hi")];
    let mut record = completed_record("failed");
    record.status = "Error".into();
    record.error = Some("provider failed".into());
    let analysis = analyze_rules(&cases, &[Ok(record)]);
    let finding = analysis.findings.iter().find(|item| item.id == "case_failed").unwrap();
    assert_eq!(finding.severity, FindingSeverity::P0);
    assert!(finding.evidence.contains("provider failed"));
}

#[test]
fn rules_flag_slow_high_token_case_as_p1() {
    let cases = vec![EvalCase::smoke("expensive", "hi")];
    let mut record = completed_record("expensive");
    record.elapsed_ms = 30_000;
    record.usage = Some(TurnUsage { input_tokens: 40_000, ..TurnUsage::default() });
    let analysis = analyze_rules(&cases, &[Ok(record)]);
    assert!(analysis.findings.iter().any(|item| {
        item.id == "slow_high_token" && item.severity == FindingSeverity::P1
    }));
}

#[test]
fn rules_flag_forbidden_tool_use() {
    let mut case = EvalCase::smoke("simple", "1+1");
    case.tool_expectation = ToolExpectation::Forbidden;
    let mut record = completed_record("simple");
    record.analysis.tool_events.push(EvalToolEvent {
        name: "exec_shell".into(),
        failed: false,
    });
    let analysis = analyze_rules(&[case], &[Ok(record)]);
    assert!(analysis.findings.iter().any(|item| item.id == "unexpected_tool_use"));
}

#[test]
fn rules_require_absolute_and_relative_latency_for_outlier() {
    let cases = ["fast-a", "fast-b", "slow", "not-slow"]
        .map(|id| EvalCase::smoke(id, "hi"));
    let records = [("fast-a", 4_000), ("fast-b", 4_000), ("slow", 12_000), ("not-slow", 8_000)]
        .map(|(id, elapsed)| {
            let mut record = completed_record(id);
            record.elapsed_ms = elapsed;
            Ok(record)
        });
    let analysis = analyze_rules(&cases, &records);
    assert!(analysis.findings.iter().any(|item| {
        item.id == "latency_outlier" && item.case_id.as_deref() == Some("slow")
    }));
    assert!(!analysis.findings.iter().any(|item| {
        item.id == "latency_outlier" && item.case_id.as_deref() == Some("not-slow")
    }));
}
```

Also assert a five-case healthy batch emits only the sample-size limitation and no fabricated finding.

- [ ] **Step 2: Run tests to verify they fail**

Run:

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib rules_ -- --nocapture
```

Expected: compile failure because the analysis module does not exist.

- [ ] **Step 3: Define stable analysis domain types**

In `analysis/mod.rs`, define serde-capable public(crate) types with snake_case serialization:

```rust
pub enum FindingSource { Rule, Judge }
pub enum FindingSeverity { P0, P1, P2 }

pub struct EvalFinding {
    pub id: String,
    pub source: FindingSource,
    pub severity: FindingSeverity,
    pub case_id: Option<String>,
    pub category: String,
    pub title: String,
    pub evidence: String,
    pub impact: String,
    pub recommendation: String,
    pub confidence: Option<f32>,
}

pub struct RuleAnalysis {
    pub findings: Vec<EvalFinding>,
    pub limitations: Vec<String>,
}

pub enum JudgeStatus {
    Completed,
    NotConfigured,
    SkippedSameModel { reason: String },
    Failed { reason: String },
}

pub struct JudgeDimensionScore {
    pub dimension: String,
    pub score: u8,
    pub confidence: f32,
    pub evidence: String,
}

pub struct JudgeReport {
    pub status: JudgeStatus,
    pub dimensions: Vec<JudgeDimensionScore>,
    pub findings: Vec<EvalFinding>,
}
```

Implement deterministic ordering by severity, case ID, finding ID. Add a merge function that deduplicates exact `(case_id, category, normalized title)` matches while preserving conflicting findings with different evidence.

- [ ] **Step 4: Implement exact first-pass rules**

In `analysis/rules.rs`, implement:

- non-Completed, timeout, runner/provider error -> P0;
- failed tool event -> P0;
- forbidden tool use -> P1;
- required tool with no tool event -> P1;
- elapsed `>= 30_000` and input tokens `>= 40_000` -> P1 efficiency;
- input tokens `>= 40_000` and cache-hit ratio `< 0.25` -> P1 cache;
- same tool name called at least three times -> P2 repetition;
- elapsed `>= 10_000` and greater than twice the successful-case median -> P2 latency outlier;
- fewer than 10 cases -> limitation: single smoke run is insufficient for trend conclusions.

Every emitted finding must have non-empty evidence, impact, and recommendation.

- [ ] **Step 5: Run rule tests and compile**

Run:

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib rules_ --no-run
rustup run stable-x86_64-pc-windows-msvc cargo check --bin eval_smoke --features dev-tools
```

Expected: exit 0. If the loader is healthy, run without `--no-run` and expect all `rules_` tests to pass.

- [ ] **Step 6: Scope-check and commit**

Run change detection, then:

```powershell
git add pinvou3-app/src-tauri/src/features/assistant/eval/analysis pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs
git commit -s -m "feat(eval): 增加确定性问题分析规则"
```

### Task 3: Build the private Markdown report model and atomic writer

**Files:**
- Create: `pinvou3-app/src-tauri/src/features/assistant/eval/markdown_report.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs`
- Test: `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`

- [ ] **Step 1: Write failing renderer, atomicity, and privacy tests**

Create tests that assert:

- all eight required headings are present;
- rule findings render with `[规则事实]`, Judge findings with `[AI 推断]`;
- findings appear in P0/P1/P2 order;
- `secret prompt`, `secret answer`, `Authorization: Bearer abc`, and a 32-character API key do not appear;
- final Markdown shares the JSONL stem;
- `.md.tmp` is absent after success.

Use an isolated `PINVOU3_HOME` and an explicit JSONL path, not the user's real eval directory.

- [ ] **Step 2: Run tests to verify they fail**

Run:

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib markdown_report_ -- --nocapture
```

Expected: compile failure because `markdown_report` does not exist.

- [ ] **Step 3: Implement the report input and renderer**

Define:

```rust
pub struct EvalMarkdownReport<'a> {
    pub metadata: &'a EvalRunMetadata,
    pub records: &'a [Result<EvalRecord>],
    pub findings: &'a [EvalFinding],
    pub judge: &'a JudgeReport,
    pub limitations: &'a [String],
}

pub struct MarkdownReportOutcome {
    pub path: PathBuf,
    pub markdown: String,
}
```

Render fixed sections in this order: 运行结论、关键指标、逐用例诊断、工具与性能观察、确定性规则发现、独立 Judge 评分、P0/P1/P2 改进建议、评测限制与可比性说明. Use only `EvalRecord` serialized facts and sanitized findings; never render `record.analysis`.

- [ ] **Step 4: Implement atomic same-basename writing**

Expose:

```rust
pub fn write_markdown_report(
    jsonl_path: &Path,
    report: &EvalMarkdownReport<'_>,
) -> Result<MarkdownReportOutcome>
```

Replace `.jsonl` with `.md`, create `<name>.md.tmp` with `create_new`, write, flush, `sync_all`, then rename. On error, return context containing both temporary and final paths. Apply a final sensitive-pattern guard to the rendered Markdown and fail closed if an API-key/auth pattern is detected.

- [ ] **Step 5: Run tests and formatter**

Run:

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib markdown_report_ --no-run
rustup run 1.97.1-x86_64-pc-windows-gnu rustfmt --edition 2021 --config skip_children=true pinvou3-app/src-tauri/src/features/assistant/eval/markdown_report.rs pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs
git diff --check
```

Expected: compile succeeds, formatting and diff checks succeed.

- [ ] **Step 6: Scope-check and commit**

```powershell
git add pinvou3-app/src-tauri/src/features/assistant/eval/markdown_report.rs pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs
git commit -s -m "feat(eval): 生成原子 Markdown 分析报告"
```

### Task 4: Add independent saved-model selection for Judge sessions

**Files:**
- Modify: `pinvou3-app/src-tauri/src/features/assistant/product_runtime/mod.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/mock.rs`
- Test: `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`

- [ ] **Step 1: Run symbol impact and write failing model-isolation tests**

Impact-check `SessionSpec`, `EnginePool::prepare_eval_session`, and `default_model_for_new_session`. Then add pure tests for a helper accepting tested and Judge identities:

```rust
assert!(validate_judge_identity(
    &ModelIdentity::new("provider-a", "model-a"),
    &ModelIdentity::new("provider-b", "model-b")
).is_ok());
assert!(validate_judge_identity(
    &ModelIdentity::new("provider-a", "model-a"),
    &ModelIdentity::new("provider-a", "model-a")
).is_err());
```

Add a missing saved-model ID test that expects a contextual error containing the ID but no credential fields.

- [ ] **Step 2: Run tests to verify they fail**

Run:

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib validate_judge_identity -- --nocapture
```

Expected: compile failure because model identity support does not exist.

- [ ] **Step 3: Add optional saved model to session preparation**

Change `SessionSpec` to:

```rust
pub struct SessionSpec {
    pub session_id: String,
    pub model_id: Option<String>,
}
```

Change `EnginePool::prepare_eval_session` to accept `model_id: Option<&str>`. For `Some(id)`, load `UserPrefs`, locate the exact saved model, and pass its wire model and ID into `create_empty_with_id`; for `None`, retain the current default-model behavior byte-for-byte.

Add the non-secret `ModelIdentity` type to `analysis/judge.rs`, containing only provider and model. Resolve tested and Judge identities from `UserPrefs::active_model()` and the selected `SavedModel` using existing preset/provider routing semantics, and reject equality after trimming and ASCII case normalization. Do not return `SavedModel` from report-facing APIs.

- [ ] **Step 4: Update runtime, mock, and runner callers**

Normal eval cases pass `model_id: None`; Judge will pass `Some(judge_model_id)`. Derive `Clone` for `EnginePoolRuntime` so suite execution and the later Judge turn can share its internal `Arc<EnginePool>`. Update the mock to record the requested model ID so tests can prove the Judge path does not silently use the active model.

- [ ] **Step 5: Run compile and focused tests**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib judge_identity --no-run
rustup run stable-x86_64-pc-windows-msvc cargo check --bin eval_smoke --features dev-tools
```

Expected: both commands exit 0, default eval behavior is unchanged when model ID is `None`.

- [ ] **Step 6: Scope-check and commit**

```powershell
git add pinvou3-app/src-tauri/src/features/assistant/product_runtime/mod.rs pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs pinvou3-app/src-tauri/src/features/assistant/eval/mock.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs
git commit -s -m "feat(eval): 支持独立 Judge 模型会话"
```

### Task 5: Implement the structured Judge adapter and safe degradation

**Files:**
- Create: `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/judge.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/analysis/mod.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs`
- Test: `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`

- [ ] **Step 1: Write failing Judge parser and degradation tests**

Cover valid six-dimension JSON, malformed JSON, missing dimension, score outside `0..=100`, confidence outside `0.0..=1.0`, timeout, same-model skip, and successful degradation. Assert degradation yields `JudgeStatus::Failed` or `SkippedSameModel` plus an empty Judge finding list, not an eval error.

- [ ] **Step 2: Run tests to verify they fail**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib judge_ -- --nocapture
```

Expected: compile failure because Judge types and adapter do not exist.

- [ ] **Step 3: Complete validation for the shared Judge schema**

Use the `JudgeStatus`, `JudgeDimensionScore`, and `JudgeReport` types introduced in Task 2. Add the wire-only response schema inside `analysis/judge.rs`; it must deserialize dimensions and proposed findings without exposing that wire shape to the Markdown renderer.

```rust
#[derive(Deserialize)]
struct JudgeWireResponse {
    dimensions: Vec<JudgeDimensionScore>,
    findings: Vec<JudgeWireFinding>,
}
```

Require exactly these dimensions: `task_completion`, `correctness`, `tool_choice`, `efficiency`, `safety_boundaries`, `overall_quality`. Limit each evidence and recommendation string to 500 Unicode scalar values before persistence.

- [ ] **Step 4: Implement a runtime-agnostic Judge client seam**

Define an async `JudgeClient` trait whose only production operation accepts a prompt and returns response text. Implement `ProductRuntimeJudge<R: ProductChatRuntime + Clone>` by creating a unique temporary Judge session with the requested saved model ID, submitting one no-tools turn, awaiting completion with a 90-second timeout, returning `assistant_text`, and always closing the session. Derive `Clone` for `MockConfig` and `MockRuntime` so this exact production adapter is testable without a provider.

Build the prompt from case ID, user prompt, assistant response, tool names/error flags, status, usage, and milestones. Explicitly demand JSON only, the six dimensions, evidence grounded in supplied material, no public-ranking claim, and concise prioritized findings. Do not include credentials, session filesystem paths, full tool inputs, or full tool outputs.

- [ ] **Step 5: Implement parse, validate, and degrade orchestration**

Expose an async function returning `JudgeReport` rather than propagating provider/parse errors. Convert all expected Judge failures into a sanitized `Failed` status. Only process Judge findings after schema validation; label them `FindingSource::Judge`.

- [ ] **Step 6: Run focused tests and compile**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --lib judge_ --no-run
rustup run stable-x86_64-pc-windows-msvc cargo check --bin eval_smoke --features dev-tools
```

Expected: exit 0; mock Judge tests prove timeout and invalid JSON do not fail the eval suite.

- [ ] **Step 7: Scope-check and commit**

```powershell
git add pinvou3-app/src-tauri/src/features/assistant/eval/analysis pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs
git commit -s -m "feat(eval): 接入结构化独立 Judge 分析"
```

### Task 6: Wire CLI, JSONL completion metadata, and GUI rule reports

**Files:**
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/report.rs`
- Modify: `pinvou3-app/src-tauri/src/eval_cli.rs`
- Modify: `pinvou3-app/src-tauri/src/bin/eval_smoke.rs`
- Modify: `pinvou3-app/src-tauri/src/app/commands/eval.rs`
- Modify: `pinvou3-app/src-tauri/src/lib.rs`
- Test: `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`
- Test: `pinvou3-app/src-tauri/src/bin/eval_smoke.rs`

- [ ] **Step 1: Impact-check public entry points and write failing CLI tests**

Impact-check `run_product_eval_smoke`, `EvalSmokeOutcome`, `EvalReportWriter::finish`, and `run_eval_smoke`. Add parser tests for:

```rust
assert_eq!(
    parse_args(["eval_smoke", "--mode", "product", "--judge-model-id", "judge-a"])
        .unwrap()
        .judge_model_id.as_deref(),
    Some("judge-a")
);
assert!(parse_args(["eval_smoke", "--judge-model-id"]).is_err());
assert!(parse_args(["eval_smoke", "--judge-model-id", " "]).is_err());
```

Add an orchestration test with a failing mock Judge that still creates Markdown and preserves `all_succeeded=true`.

- [ ] **Step 2: Run tests to verify they fail**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --bin eval_smoke parse_args --no-run --features dev-tools
```

Expected: compile failure because `CliArgs` has no Judge field.

- [ ] **Step 3: Add options and outcome types**

Define in the library composition root:

```rust
pub struct EvalSmokeOptions {
    pub judge_model_id: Option<String>,
}

pub struct EvalSmokeOutcome {
    pub all_succeeded: bool,
    pub jsonl_report_path: PathBuf,
    pub markdown_report_path: PathBuf,
    pub markdown: String,
    pub judge_status: JudgeStatus,
}
```

Change `run_product_eval_smoke(options)` and update the binary caller. The GUI command keeps its existing no-argument Tauri payload and explicitly builds:

```rust
JudgeReport {
    status: JudgeStatus::NotConfigured,
    dimensions: Vec::new(),
    findings: Vec::new(),
}
```

- [ ] **Step 4: Order finalization so Markdown failure is visible**

After the suite:

1. run rules;
2. run or skip Judge;
3. merge findings;
4. finish JSONL to obtain its final path;
5. write Markdown beside it;
6. return both paths.

If step 5 fails, return `Err`, causing CLI exit 1 while leaving the finalized JSONL available. Keep Judge failure inside `JudgeStatus`, not as `Err`.

Apply the same persistence order in `app/commands/eval.rs`: construct `EvalReportWriter` and metadata from the live pool, append each case through the existing callback, finish JSONL, run rules with `JudgeStatus::NotConfigured`, write the same-basename Markdown, and return its full Markdown text to the Tauri caller. This makes GUI-triggered Product eval produce both files without adding a Judge argument to the existing invoke contract.

Extend the JSONL `complete` line only with optional backward-compatible fields:

```rust
analysis_status: Option<String>,
markdown_report: Option<String>,
```

Because Markdown is written after JSONL finalization, set `analysis_status` to the computed Judge/rule status and omit `markdown_report` from JSONL in this version; the CLI outcome is authoritative for the path. This avoids rewriting an atomically finalized JSONL file.

- [ ] **Step 5: Print human-readable paths and help text**

Update help to:

```text
Usage: eval_smoke [--mode product] [--judge-model-id <saved-model-id>]
```

Print:

```text
JSONL report: <absolute path>
Markdown report: <absolute path>
Judge status: <completed|not_configured|skipped_same_model|failed>
```

- [ ] **Step 6: Run CLI, report, and app compile checks**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test --bin eval_smoke --features dev-tools --no-run
rustup run stable-x86_64-pc-windows-msvc cargo test --lib eval_ --no-run
rustup run stable-x86_64-pc-windows-msvc cargo check --bin eval_smoke --features dev-tools
python scripts/architecture-guard.py
```

Run the architecture guard from the repository root. Expected: all compilation checks exit 0 and the guard reports no increased debt.

- [ ] **Step 7: Scope-check and commit**

Run `detect_changes` or the documented diff fallback, then:

```powershell
git add pinvou3-app/src-tauri/src/features/assistant/eval/report.rs pinvou3-app/src-tauri/src/eval_cli.rs pinvou3-app/src-tauri/src/bin/eval_smoke.rs pinvou3-app/src-tauri/src/app/commands/eval.rs pinvou3-app/src-tauri/src/lib.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs
git commit -s -m "feat(eval): 串联 Markdown 报告与 Judge 参数"
```

### Task 7: Document and verify the complete workflow

**Files:**
- Modify: `PROGRESS.md`
- Modify: `docs/superpowers/specs/2026-08-12-eval-markdown-analysis-report-design.md` only if implementation reveals a necessary clarified contract; do not change approved behavior silently.

- [ ] **Step 1: Update user-facing run documentation**

Document these exact commands from `pinvou3-app/src-tauri`:

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo run --bin eval_smoke --features dev-tools -- --mode product
rustup run stable-x86_64-pc-windows-msvc cargo run --bin eval_smoke --features dev-tools -- --mode product --judge-model-id <saved-model-id>
```

Explain rule-only degradation, independent model requirement, report paths, privacy boundary, exit codes, and the fact that Product mode cannot be compared directly with BFCL rankings.

- [ ] **Step 2: Run static verification**

```powershell
git diff --check
python scripts/architecture-guard.py
rustup run stable-x86_64-pc-windows-msvc cargo check --bin eval_smoke --features dev-tools
rustup run stable-x86_64-pc-windows-msvc cargo test --lib eval_ --no-run
```

Expected: all commands exit 0 except actual test execution may still be blocked by the documented Windows `0xc0000139`; `--no-run` must succeed.

- [ ] **Step 3: Run a real rule-only Product smoke**

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo run --bin eval_smoke --features dev-tools -- --mode product
```

Expected: five cases execute, JSONL and Markdown paths print, Markdown reports Judge as not configured, exit code matches case success, and no eval session or `.tmp` remains.

- [ ] **Step 4: Run a real independent-Judge Product smoke**

Choose an existing saved model ID whose normalized provider/model identity differs from the active tested model, then run:

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo run --bin eval_smoke --features dev-tools -- --mode product --judge-model-id <saved-model-id>
```

Expected: `Judge status: completed`; Markdown contains six dimensions, at least the sample-size limitation, and only evidence-backed findings. If no independent model is configured locally, record that external prerequisite and verify the same-model and missing-model degradation paths instead.

- [ ] **Step 5: Audit output privacy and cleanup**

For both generated paths, verify:

```powershell
Select-String -Path <jsonl>,<markdown> -Pattern 'Authorization:|Bearer\s+[A-Za-z0-9._-]{8,}|api[_-]?key\s*[:=]|cookie\s*[:=]' -CaseSensitive:$false
Get-ChildItem "$env:USERPROFILE\.pinvou3\eval" -Filter '*.tmp'
Get-ChildItem "$env:USERPROFILE\.pinvou3\sessions" -Directory -Filter 'eval_*'
```

Expected: no credential-pattern match, no `.tmp`, and no sessions from the completed run. Token metric field names such as `input_tokens` are allowed and must not be mistaken for credentials.

- [ ] **Step 6: Final change detection and commit docs**

Run GitNexus `detect_changes({scope: "compare", base_ref: "main"})`. If unavailable, record that fact and audit `git diff --name-only main...HEAD` plus the final working-tree diff. Then:

```powershell
git add PROGRESS.md
git commit -s -m "docs(eval): 记录 Markdown 分析报告用法"
```

- [ ] **Step 7: Final self-review**

Confirm every approved requirement has evidence:

- same-basename JSONL/Markdown;
- deterministic rules;
- independent Judge and same-model rejection;
- Judge degradation does not change eval success;
- Markdown failure returns nonzero;
- original prompts/responses absent from persistent reports;
- evidence-backed P0/P1/P2 suggestions;
- no public-ranking claim;
- no temporary artifacts or sessions.
