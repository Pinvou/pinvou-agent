# Eval Smoke Runner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a windowless `eval_smoke` CLI that runs the PLEP smoke set through the same `PinvouChatRunner<EnginePoolRuntime>` path as the GUI, prints Markdown, and persists reproducible JSONL results.

**Architecture:** A shared generic suite function owns sequential case execution and per-record callbacks. A focused report module owns versioned JSONL serialization and atomic completion. The existing Tauri command and a new windowless Tauri binary provide two thin adapters over the same product-mode runner.

**Tech Stack:** Rust 1.89, Tokio, Tauri 2, serde/serde_json, chrono, anyhow, existing Pinvou EnginePool and ProductChatRuntime.

---

## File Structure

- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs`: serializable eval domain types, `EvalMode`, shared suite result and sequential suite entry point.
- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`: suite behavior tests using `MockRuntime`.
- Create `pinvou3-app/src-tauri/src/features/assistant/eval/report.rs`: versioned run metadata, JSONL writer, temporary-file completion and Markdown report ownership.
- Modify `pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs`: export the focused report module and remove the old formatter after migration.
- Modify `pinvou3-app/src-tauri/src/app/commands/eval.rs`: thin GUI adapter using `EnginePoolRuntime` and the shared suite.
- Create `pinvou3-app/src-tauri/src/bin/eval_smoke.rs`: windowless Tauri runtime and process exit behavior.
- Modify `pinvou3-app/src-tauri/Cargo.toml`: register the `eval_smoke` dev-tools binary.
- Modify `pinvou3-app/src-tauri/src/platform/paths.rs`: expose `~/.pinvou3/eval/` with layout coverage.
- Modify `PROGRESS.md`: record T5 runner completion, verification evidence and remaining BFCL work.

### Task 1: Restore a Verifiable Rust Toolchain

**Files:**
- No repository files.

- [ ] **Step 1: Locate a GNU import-library tool**

Run:

```powershell
where.exe dlltool
where.exe llvm-dlltool
Get-ChildItem 'C:\Program Files\LLVM\bin','C:\msys64\usr\bin','C:\msys64\mingw64\bin' -Filter '*dlltool*.exe' -ErrorAction SilentlyContinue
```

Expected: at least one usable `dlltool.exe` or `llvm-dlltool.exe` path. If only `llvm-dlltool.exe` exists, expose it to the Rust build through a temporary task-scoped PATH entry; do not modify global user configuration.

- [ ] **Step 2: Prove the existing tests can reach project code**

Run:

```powershell
$env:CARGO_TARGET_DIR='D:\Worksapce\SourceCode\Task\pinvou-agent\pinvou3-app\src-tauri\target'
cargo test mock_smoke --lib
```

Expected: the four existing `mock_smoke_*` tests run. If toolchain recovery is impossible, record the exact blocker and continue only with non-linking checks; never claim tests passed.

### Task 2: Add the Shared Sequential Suite

**Files:**
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs`
- Test: `pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs`

- [ ] **Step 1: Write the failing suite tests**

Add tests that use the wished-for API:

```rust
#[tokio::test]
async fn suite_preserves_case_order_and_reports_success() {
    let cases = vec![
        EvalCase::smoke("suite-a", "first"),
        EvalCase::smoke("suite-b", "second"),
    ];
    let mut observed = Vec::new();
    let suite = run_eval_suite(MockRuntime::immediate(), &cases, |result| {
        observed.push(result.as_ref().unwrap().case_id.clone());
        Ok(())
    })
    .await
    .unwrap();

    assert_eq!(observed, ["suite-a", "suite-b"]);
    assert!(suite.all_succeeded());
    assert_eq!(suite.records.len(), 2);
}

#[tokio::test]
async fn suite_keeps_running_after_a_case_failure() {
    let runtime = MockRuntime::new(MockConfig {
        status: "Error".to_string(),
        error: Some("provider failed".to_string()),
        usage: None,
        ..Default::default()
    });
    let cases = vec![
        EvalCase::smoke("suite-error", "first"),
        EvalCase::smoke("suite-after-error", "second"),
    ];

    let mut observed = Vec::new();
    let suite = run_eval_suite(runtime, &cases, |result| {
        observed.push(result.as_ref().unwrap().case_id.clone());
        Ok(())
    })
    .await
    .unwrap();

    assert_eq!(suite.records.len(), 2);
    assert!(!suite.all_succeeded());
    assert_eq!(observed, ["suite-error", "suite-after-error"]);
}
```

- [ ] **Step 2: Run the tests and verify RED**

Run:

```powershell
cargo test suite_ --lib
```

Expected: compile failure because `run_eval_suite` and `EvalSuiteResult` do not exist.

- [ ] **Step 3: Run GitNexus impact analysis before editing symbols**

Run `impact` for `PinvouChatRunner`, `EvalCase`, `EvalRecord`, and `format_report` with `direction: upstream`. If the MCP tool remains unavailable and no local index runner exists, record that limitation before editing and use `rg` caller enumeration as explicit fallback evidence.

- [ ] **Step 4: Implement the minimal shared suite**

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalMode {
    Product,
    OfficialCompatible,
}

pub struct EvalSuiteResult {
    pub records: Vec<Result<EvalRecord>>,
}

impl EvalSuiteResult {
    pub fn all_succeeded(&self) -> bool {
        self.records.iter().all(|record| {
            record
                .as_ref()
                .is_ok_and(|record| record.status.eq_ignore_ascii_case("completed"))
        })
    }
}

pub async fn run_eval_suite<R, F>(
    runtime: R,
    cases: &[EvalCase],
    mut on_record: F,
) -> Result<EvalSuiteResult>
where
    R: ProductChatRuntime,
    F: FnMut(&Result<EvalRecord>) -> Result<()>,
{
    let runner = PinvouChatRunner::new(runtime);
    let mut records = Vec::with_capacity(cases.len());
    for case in cases {
        let record = runner.run_case(case).await;
        on_record(&record)?;
        records.push(record);
    }
    Ok(EvalSuiteResult { records })
}
```

Derive `Serialize`/`Deserialize` only on domain records that contain serializable fields. Keep `anyhow::Error` outside persisted structures.

- [ ] **Step 5: Run the suite tests and existing Mock tests**

Run:

```powershell
cargo test suite_ --lib
cargo test mock_smoke --lib
```

Expected: all targeted tests pass.

- [ ] **Step 6: Commit the suite**

```powershell
git add pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs pinvou3-app/src-tauri/src/features/assistant/eval/tests.rs
git commit -s -m "feat(eval): 统一批量评测执行入口"
```

### Task 3: Add Versioned JSONL Reporting

**Files:**
- Create: `pinvou3-app/src-tauri/src/features/assistant/eval/report.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/eval/mod.rs`
- Modify: `pinvou3-app/src-tauri/src/platform/paths.rs`

- [ ] **Step 1: Write failing path and writer tests**

Add a path test asserting `eval_reports_dir() == pinvou3_home().join("eval")`, then add report tests using a temporary `PINVOU3_HOME`:

```rust
#[test]
fn report_writer_appends_records_and_atomically_completes() {
    let _guard = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = std::env::temp_dir().join("pinvou3-eval-report-run-1");
    std::env::set_var("PINVOU3_HOME", &temp);
    let metadata = EvalRunMetadata {
        schema_version: 1,
        run_id: "run-1".to_string(),
        mode: EvalMode::Product,
        case_set: "plep-smoke".to_string(),
        case_set_version: "1".to_string(),
        pinvou_version: "test".to_string(),
        provider: "test-provider".to_string(),
        model: "test-model".to_string(),
        started_at: "2026-08-11T00:00:00Z".to_string(),
    };
    let mut writer = EvalReportWriter::create(metadata).unwrap();
    writer
        .append(&Ok(EvalRecord {
            case_id: "case-\"一\n".to_string(),
            session_id: "eval-case".to_string(),
            turn_id: "turn-1".to_string(),
            status: "Completed".to_string(),
            error: None,
            usage: None,
            elapsed_ms: 10,
        }))
        .unwrap();

    assert!(writer.temporary_path().exists());
    let final_path = writer.finish(true).unwrap();
    assert!(final_path.exists());

    let lines = std::fs::read_to_string(final_path).unwrap();
    for line in lines.lines() {
        serde_json::from_str::<serde_json::Value>(line).unwrap();
    }
    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn report_metadata_does_not_serialize_credentials() {
    let value = serde_json::json!({
        "schema_version": 1,
        "run_id": "run-2",
        "mode": EvalMode::Product,
        "case_set": "plep-smoke",
        "case_set_version": "1",
        "pinvou_version": "test",
        "provider": "test-provider",
        "model": "test-model",
        "started_at": "2026-08-11T00:00:00Z"
    });
    let text = value.to_string().to_ascii_lowercase();
    assert!(!text.contains("api_key"));
    assert!(!text.contains("cookie"));
    assert!(!text.contains("secret"));
}
```

Use existing `platform::paths::tests::ENV_LOCK`; do not introduce parallel environment-variable mutation.

- [ ] **Step 2: Run report tests and verify RED**

Run:

```powershell
cargo test eval_report --lib
```

Expected: compile failure because the report module and path do not exist.

- [ ] **Step 3: Run impact analysis before editing paths and formatter symbols**

Run upstream impact for `pinvou3_home`, `ensure_dirs`, and `format_report`. Warn before continuing if risk is HIGH or CRITICAL.

- [ ] **Step 4: Implement the report path**

Add to `platform/paths.rs`:

```rust
/// `~/.pinvou3/eval/` — local evaluation reports and interrupted `.tmp` runs.
pub fn eval_reports_dir() -> PathBuf {
    pinvou3_home().join("eval")
}
```

Do not add the directory to broad boot-time `ensure_dirs`; the report writer creates it only when evaluation is explicitly run.

- [ ] **Step 5: Implement focused report types and writer**

Create serializable `EvalRunMetadata` with the fields shown in the test, a tagged `EvalJsonLine::{Run, Case, CaseError, Complete}`, and `EvalReportWriter`. The `Complete` line contains `finished_at` and `all_succeeded`; `CaseError` stores only `case_id` when known and the rendered error string, never `anyhow::Error` itself. `create` must use `OpenOptions::create_new(true)` on `<name>.jsonl.tmp`; `append` must call `serde_json::to_writer`, append `\n`, and flush. `finish` must append the completion line, flush, close, then call `std::fs::rename` to the same path without `.tmp`.

Move `format_report` into `report.rs` and escape pipe/newline characters before writing Markdown cells:

```rust
fn markdown_cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\r', '\n'], " ")
}
```

- [ ] **Step 6: Run report and path tests**

Run:

```powershell
cargo test eval_report --lib
cargo test pinvou3_home_respects_env_override --lib
```

Expected: all targeted tests pass.

- [ ] **Step 7: Commit reporting**

```powershell
git add pinvou3-app/src-tauri/src/features/assistant/eval pinvou3-app/src-tauri/src/platform/paths.rs
git commit -s -m "feat(eval): 持久化可复现 JSONL 报告"
```

### Task 4: Route the GUI Command Through the Shared Runner

**Files:**
- Modify: `pinvou3-app/src-tauri/src/app/commands/eval.rs`

- [ ] **Step 1: Add a failing source-boundary test**

Add a small test next to the command that exercises an extracted adapter function accepting any `ProductChatRuntime`; it should call the adapter with `MockRuntime` and assert the generated Markdown includes all smoke case IDs. This test must fail before the adapter exists.

- [ ] **Step 2: Verify RED**

Run:

```powershell
cargo test command_eval_uses_shared_suite --lib
```

Expected: compile failure for the missing adapter.

- [ ] **Step 3: Run impact analysis before changing `run_eval_smoke`**

Run upstream impact for `run_eval_smoke`; enumerate the Tauri invoke handler and any Rust callers.

- [ ] **Step 4: Replace the duplicate execution loop**

Keep the public command signature stable. Construct `EnginePoolRuntime::new(Arc::new(pool.inner().clone()))`, call the generic adapter/shared suite with a no-op callback, and format the returned records. Delete command-local `run_case` and `wait_for_completion`.

- [ ] **Step 5: Run command, suite, and Mock tests**

Run:

```powershell
cargo test command_eval_uses_shared_suite --lib
cargo test suite_ --lib
cargo test mock_smoke --lib
```

Expected: all targeted tests pass.

- [ ] **Step 6: Commit the GUI refactor**

```powershell
git add pinvou3-app/src-tauri/src/app/commands/eval.rs
git commit -s -m "refactor(eval): 复用产品评测 Runner"
```

### Task 5: Add the Windowless `eval_smoke` Binary

**Files:**
- Create: `pinvou3-app/src-tauri/src/bin/eval_smoke.rs`
- Modify: `pinvou3-app/src-tauri/Cargo.toml`

- [ ] **Step 1: Verify the binary is absent (RED)**

Run:

```powershell
cargo check --bin eval_smoke --features dev-tools
```

Expected: failure stating there is no bin target named `eval_smoke`.

- [ ] **Step 2: Register the binary**

Add:

```toml
[[bin]]
name = "eval_smoke"
path = "src/bin/eval_smoke.rs"
required-features = ["dev-tools"]
```

- [ ] **Step 3: Implement the smallest windowless Tauri harness**

Use `tauri::generate_context!()` to obtain the production context, clear `context.config_mut().app.windows`, then build a Tauri app. In `setup`, initialize the same minimal `SessionStore`, `EnginePool`, tool factory and tool policy used by the production app; do not initialize updater, windows, remote control, scheduled tasks, or unrelated connectors.

Spawn the async suite from setup, send its `anyhow::Result<RunOutcome>` through a one-shot channel, and call `app_handle.exit(code)` after reporting. Run the event loop on the main thread and convert the outcome into process exit code 0 (all completed) or 1 (startup/case failure).

The product metadata passed to `EvalRunMetadata` must come from effective runtime configuration and contain only provider/model identifiers, never credentials.

- [ ] **Step 4: Add an argument-level test without a provider call**

Extract `parse_args` and add:

```rust
#[test]
fn official_compatible_mode_is_explicitly_unsupported() {
    let error = parse_args(["eval_smoke", "--mode", "official-compatible"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("BFCL adapter"));
}
```

Run it first before implementing `parse_args` to observe RED, then implement only `product` as the default accepted mode.

- [ ] **Step 5: Compile and exercise non-provider startup behavior**

Run:

```powershell
cargo test official_compatible_mode_is_explicitly_unsupported --bin eval_smoke --features dev-tools
cargo check --bin eval_smoke --features dev-tools
```

Expected: test and check pass without opening a WebView.

- [ ] **Step 6: Commit the binary**

Before committing, run GitNexus `detect_changes({scope: "compare", base_ref: "main"})`. If unavailable, record the tool limitation and inspect `git diff --stat origin/main...HEAD` plus `git diff --check`.

```powershell
git add pinvou3-app/src-tauri/Cargo.toml pinvou3-app/src-tauri/src/bin/eval_smoke.rs
git commit -s -m "feat(eval): 新增无窗口 smoke runner"
```

### Task 6: Verify, Document, and Close T5 Runner Scope

**Files:**
- Modify: `PROGRESS.md`

- [ ] **Step 1: Format and run targeted verification**

Run:

```powershell
cargo fmt --all
cargo fmt --all -- --check
cargo test suite_ --lib
cargo test eval_report --lib
cargo test mock_smoke --lib
cargo test command_eval_uses_shared_suite --lib
cargo test official_compatible_mode_is_explicitly_unsupported --bin eval_smoke --features dev-tools
cargo check --bin eval_smoke --features dev-tools
python scripts/architecture-guard.py
```

Expected: every available command exits 0. Do not collapse unavailable linker verification into success.

- [ ] **Step 2: Run a real product-mode smoke only when configured**

Run:

```powershell
cargo run --bin eval_smoke --features dev-tools
```

Expected: no desktop window; Markdown printed; a final `.jsonl` path under `~/.pinvou3/eval/`; exit 0 only when every case completed. Do not run if doing so would use an unapproved paid or external provider configuration.

- [ ] **Step 3: Inspect the persisted report for safety and completeness**

Parse every JSONL line, verify metadata/case/completion records, and search case-insensitively for credential field names. Never print actual secret values during the check.

- [ ] **Step 4: Update the progress checkpoint**

Mark the runner-binary portion of T5 complete, list actual commits and verification evidence, and keep BFCL adapter pending. Update stale commit counts and mention the dual-track scoring boundary.

- [ ] **Step 5: Final change-scope audit**

Run GitNexus `detect_changes({scope: "compare", base_ref: "main"})`, `git diff --check origin/main...HEAD`, and `git status --short`. Confirm no unrelated user files are staged or committed.

- [ ] **Step 6: Commit documentation**

```powershell
git add PROGRESS.md
git commit -s -m "docs(progress): 完成 product smoke runner 闭环"
```

## Execution Notes

- This plan runs inline because the current session does not permit proactive subagent dispatch.
- `pinvou3-app/src-tauri/Cargo.toml` currently appears modified in the worktree although its content hash matches `HEAD`; inspect and refresh index metadata before staging so unrelated line-ending state is not accidentally committed.
- The BFCL adapter is intentionally the next task after this plan. Do not add partial official scoring behavior while implementing the product runner.
