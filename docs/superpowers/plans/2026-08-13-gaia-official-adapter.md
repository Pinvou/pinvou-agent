# GAIA Official Adapter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver `pinvou benchmark` support for the pinned GAIA 2023 Level 1 validation dataset, real attachments, durable private predictions, official-scorer-compatible local accuracy, and safe submission export.

**Architecture:** Add an isolated `adapter-gaia` crate for dataset, private-input, scorer, fetch, and submission concerns. Keep generic execution/resume/private storage in `benchmark-core`; extend only the score DTO needed to distinguish complete official-compatible runs from partial runs. Register GAIA in the existing CLI and reuse `pinvou-product-backend` without adding a second executable or a GAIA-specific runtime path.

**Tech Stack:** Rust 2024, `benchmark-core`, `agent-backend-api`, Apache `parquet` record reader, `hf-hub` synchronous dataset API, `sha2`, `serde`/`serde_json`, existing Tauri product backend.

---

## File structure

- `pinvou-cli/crates/adapter-gaia/src/lib.rs`: public adapter facade and fixed revisions.
- `pinvou-cli/crates/adapter-gaia/src/dataset.rs`: pinned Parquet schema, snapshot verification, safe attachment paths.
- `pinvou-cli/crates/adapter-gaia/src/private_inputs.rs`: opaque prompt/attachment handles and resolver.
- `pinvou-cli/crates/adapter-gaia/src/scorer.rs`: exact port of pinned official scorer semantics.
- `pinvou-cli/crates/adapter-gaia/src/fetch.rs`: token-env and local-snapshot acquisition.
- `pinvou-cli/crates/adapter-gaia/src/submission.rs`: private prediction export with atomic no-clobber publication.
- `pinvou-cli/crates/adapter-gaia/tests/gaia_contract.rs`: adapter, privacy, scoring, resume-facing contracts.
- `pinvou-cli/crates/benchmark-core/src/contracts.rs`: official score compatibility/completeness metadata.
- `pinvou-cli/crates/benchmark-core/tests/adapter_contract.rs`: generic DTO backward-compatibility tests.
- `pinvou-cli/crates/cli/src/lib.rs`: GAIA parser, dispatch, manifest, run/score/report/submission composition.
- `pinvou-cli/crates/cli/tests/cli_contract.rs`: human/JSON command and honest-label contracts.
- `pinvou-cli/Cargo.toml`, crate manifests, and `Cargo.lock`: workspace/dependencies.
- `docs/gaia-benchmark.md`: user workflow, access, score labels, and limitations.

### Task 1: Extend official score metadata without changing Smoke

**Files:**
- Modify: `pinvou-cli/crates/benchmark-core/src/contracts.rs`
- Modify: `pinvou-cli/crates/benchmark-core/tests/adapter_contract.rs`

- [ ] **Step 1: Write the failing DTO test**

Add a test that constructs a complete compatible report and a partial report:

```rust
let complete = OfficialScoreReport::compatible(53, 31, "validation", "1");
assert_eq!(complete.evaluated(), 53);
assert_eq!(complete.correct(), 31);
assert!(complete.is_complete());
assert!(complete.is_official_dataset_compatible());
assert_eq!(complete.split(), "validation");
assert_eq!(complete.level(), "1");

let partial = OfficialScoreReport::partial(4, 2, "validation", "1");
assert!(!partial.is_complete());
assert!(!partial.is_official_dataset_compatible());
```

- [ ] **Step 2: Run the focused RED test**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test adapter_contract official_score_ --offline`

Expected: compile failure because `compatible`, `partial`, and metadata accessors do not exist.

- [ ] **Step 3: Implement the score DTO**

Replace the two-field implementation with a constructor-validated structure:

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct OfficialScoreReport {
    evaluated: u64,
    correct: u64,
    complete: bool,
    official_dataset_compatible: bool,
    split: String,
    level: String,
}

impl OfficialScoreReport {
    pub fn compatible(evaluated: u64, correct: u64, split: &str, level: &str) -> Self {
        debug_assert!(correct <= evaluated);
        Self { evaluated, correct, complete: true,
            official_dataset_compatible: true, split: split.into(), level: level.into() }
    }
    pub fn partial(evaluated: u64, correct: u64, split: &str, level: &str) -> Self {
        debug_assert!(correct <= evaluated);
        Self { evaluated, correct, complete: false,
            official_dataset_compatible: false, split: split.into(), level: level.into() }
    }
    pub fn new(evaluated: u64, correct: u64) -> Self {
        Self::partial(evaluated, correct, "unspecified", "unspecified")
    }
    pub fn is_complete(&self) -> bool { self.complete }
    pub fn is_official_dataset_compatible(&self) -> bool { self.official_dataset_compatible }
    pub fn split(&self) -> &str { &self.split }
    pub fn level(&self) -> &str { &self.level }
}
```

Keep existing `evaluated`, `correct`, and `accuracy` behavior source-compatible.

- [ ] **Step 4: Run GREEN tests**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test adapter_contract official_score_ --offline`

Expected: all `official_score_` tests pass.

- [ ] **Step 5: Commit**

```text
git add pinvou-cli/crates/benchmark-core/src/contracts.rs pinvou-cli/crates/benchmark-core/tests/adapter_contract.rs
git commit -s -m "feat(eval): 扩展官方评分状态契约"
```

### Task 2: Build the pinned GAIA dataset verifier

**Files:**
- Create: `pinvou-cli/crates/adapter-gaia/Cargo.toml`
- Create: `pinvou-cli/crates/adapter-gaia/src/lib.rs`
- Create: `pinvou-cli/crates/adapter-gaia/src/dataset.rs`
- Create: `pinvou-cli/crates/adapter-gaia/tests/gaia_contract.rs`
- Modify: `pinvou-cli/Cargo.toml`

- [ ] **Step 1: Add a synthetic Parquet RED contract**

Create tests for exact constants, a valid two-row Level 1 fixture, duplicate task IDs, missing columns, missing attachment, absolute/parent attachment paths, and revision mismatch. The valid test must assert only safe metadata:

```rust
assert_eq!(GAIA_DATASET_REVISION, "682dd723ee1e1697e00360edccf2366dc8418dd9");
let verified = GaiaDataset::verify(&snapshot_root)?;
assert_eq!(verified.rows().len(), 2);
assert_eq!(verified.rows()[0].level(), 1);
assert!(verified.rows()[0].attachment().unwrap().starts_with(&snapshot_root));
assert!(!format!("{verified:?}").contains("secret question"));
```

Use a generated Parquet fixture; do not copy a GAIA question or answer into the repository.

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia --test gaia_contract dataset_ --no-run`

Expected: package/API missing.

- [ ] **Step 3: Add the crate and constants**

Use pinned dependencies in `adapter-gaia/Cargo.toml`:

```toml
[dependencies]
agent-backend-api = { path = "../agent-backend-api" }
benchmark-core = { path = "../benchmark-core" }
parquet = { version = "59.1.0", default-features = false, features = ["snap", "flate2", "zstd"] }
serde.workspace = true
serde_json = "1"
sha2 = "0.10.9"
thiserror.workspace = true
```

Expose immutable constants in `lib.rs`:

```rust
pub const GAIA_DATASET_REVISION: &str = "682dd723ee1e1697e00360edccf2366dc8418dd9";
pub const GAIA_SCORER_REVISION: &str = "1349a17979f0aca0ee9c46cd7ec26eb2fb41102e";
pub const GAIA_ADAPTER_VERSION: &str = "pinvou-gaia-adapter/v1";
pub const GAIA_SPLIT: &str = "validation";
pub const GAIA_LEVEL: u8 = 1;
```

- [ ] **Step 4: Implement fail-closed row loading**

Use `SerializedFileReader<File>::get_row_iter(None)` and convert by exact column name, not column position. Store question/reference in non-Serde types with redacted `Debug`:

```rust
pub struct GaiaRow {
    task_id: String,
    question: SecretText,
    reference: SecretText,
    attachment: Option<PathBuf>,
    level: u8,
}

impl fmt::Debug for GaiaRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GaiaRow([redacted])")
    }
}
```

Require exact schema names, Level=1, non-empty safe task ID, unique IDs, non-empty validation reference, and `file_path` containment. Open attachment metadata after canonical containment; reject symlink/reparse, directory, missing file, and files over the existing attachment limit. Return only fixed codes such as `gaia_schema_mismatch`, `gaia_attachment_missing`, and `gaia_revision_mismatch`.

- [ ] **Step 5: Run GREEN and privacy scan**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia --test gaia_contract dataset_ --offline`

Expected: dataset tests pass.

Run: `rg -n "secret question|secret answer" pinvou-cli/target -g '*.json' -g '*.jsonl' -g '*.md'`

Expected: no matches.

- [ ] **Step 6: Commit**

```text
git add pinvou-cli/Cargo.toml pinvou-cli/Cargo.lock pinvou-cli/crates/adapter-gaia
git commit -s -m "feat(eval): 校验固定版 GAIA 数据集"
```

### Task 3: Map GAIA rows to private NativeTurn tasks

**Files:**
- Create: `pinvou-cli/crates/adapter-gaia/src/private_inputs.rs`
- Modify: `pinvou-cli/crates/adapter-gaia/src/lib.rs`
- Modify: `pinvou-cli/crates/adapter-gaia/tests/gaia_contract.rs`

- [ ] **Step 1: Write RED task/resolver tests**

Assert descriptor revisions, durable retention, UTF-8 output, exact policy, 10-minute timeout, opaque handles, attachment resolution, selection, and unknown-handle failure:

```rust
let task = adapter.plan(&verified, &TaskSelection::all())?.tasks()[0].clone();
let ExecutionRequest::NativeTurn { tool_policy, output_contract, timeout, .. } = task.execution() else { panic!() };
assert_eq!(tool_policy.as_str(), "pinvou-gaia-public-web/v1");
assert_eq!(output_contract.as_str(), "gaia-final/v1");
assert_eq!(*timeout, Duration::from_secs(600));
assert_eq!(adapter.private_output_retention(), PredictionRetention::DurableUntilPurge);
```

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia --test gaia_contract adapter_ --offline`

Expected: missing `GaiaAdapter` and `GaiaPrivateInputs`.

- [ ] **Step 3: Implement resolver and adapter**

Handles contain only a namespaced task ID:

```rust
let prompt_handle = PrivateInputHandle::new(format!("gaia:{task_id}:prompt"));
let attachment_handle = AttachmentHandle::new(format!("gaia:{task_id}:attachment"));
```

`GaiaPrivateInputs` owns an `Arc<GaiaDataset>` and resolves prompt into `SecretText`; `resolve_attachment` returns a redacted `ResolvedAttachmentSource` for the verified canonical file. `GaiaAdapter::plan` filters only exact known task IDs and rejects an empty/unknown requested selection. `prepare_task` clones the already verified task without touching reference content.

- [ ] **Step 4: Run GREEN**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia --test gaia_contract adapter_ --offline`

Expected: adapter tests pass.

- [ ] **Step 5: Commit**

```text
git add pinvou-cli/crates/adapter-gaia
git commit -s -m "feat(eval): 映射 GAIA 私有任务输入"
```

### Task 4: Port the pinned official scorer exactly

**Files:**
- Create: `pinvou-cli/crates/adapter-gaia/src/scorer.rs`
- Modify: `pinvou-cli/crates/adapter-gaia/src/lib.rs`
- Modify: `pinvou-cli/crates/adapter-gaia/tests/gaia_contract.rs`

- [ ] **Step 1: Write scorer golden RED tests**

Cover number, currency/percent/comma removal, numeric and string lists, list-length mismatch, whitespace, ASCII punctuation, case, `None`, and the official punctuation distinction for list elements:

```rust
assert!(question_scorer("$1,200", "1200"));
assert!(question_scorer("A ; 2%", "a;2"));
assert!(!question_scorer("a,b", "a,b,c"));
assert!(question_scorer("Sea gull!", "seagull"));
assert!(question_scorer("None", "None"));
assert!(!question_scorer("a-b", "ab;ignored"));
```

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia scorer_ --offline`

Expected: missing scorer API.

- [ ] **Step 3: Implement the literal scorer port**

Implement three branches in the same order as official revision `1349a179...`:

```rust
pub fn question_scorer(candidate: Option<&str>, ground_truth: &str) -> bool {
    let candidate = candidate.unwrap_or("None");
    if let Ok(expected) = ground_truth.parse::<f64>() {
        return normalize_number(candidate) == Some(expected);
    }
    if ground_truth.contains(',') || ground_truth.contains(';') {
        return score_list(candidate, ground_truth);
    }
    normalize_string(candidate, true) == normalize_string(ground_truth, true)
}
```

Use ASCII punctuation to mirror Python `string.punctuation`; use Unicode whitespace removal to mirror `re.sub(r"\s", ...)`. Do not add tolerance, Unicode punctuation stripping, answer extraction, or LLM judging.

- [ ] **Step 4: Add CompletedRun scoring**

For every planned official task, require a completed durable prediction and resolve it through `CompletedRun::resolve_private_prediction`. Complete coverage returns `OfficialScoreReport::compatible`; any failure/missing task returns `OfficialScoreReport::partial` and must not claim a comparable accuracy in CLI output.

- [ ] **Step 5: Run GREEN**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia scorer_ --offline`

Expected: all scorer tests pass.

- [ ] **Step 6: Commit**

```text
git add pinvou-cli/crates/adapter-gaia
git commit -s -m "feat(eval): 移植 GAIA 官方评分语义"
```

### Task 5: Add safe fetch/import and offline verify

**Files:**
- Create: `pinvou-cli/crates/adapter-gaia/src/fetch.rs`
- Modify: `pinvou-cli/crates/adapter-gaia/Cargo.toml`
- Modify: `pinvou-cli/crates/adapter-gaia/src/lib.rs`
- Modify: `pinvou-cli/crates/adapter-gaia/tests/gaia_contract.rs`

- [ ] **Step 1: Write fetch/import RED tests**

Use a fake `SnapshotDownloader` seam. Assert token comes only from the named environment variable, revision is exact, import rejects a source inside the Git worktree, failed verify leaves no ready marker, and errors contain no token/source path.

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia fetch_ --offline`

Expected: missing fetch API.

- [ ] **Step 3: Implement acquisition**

Add `hf-hub = { version = "0.4.3", default-features = false, features = ["ureq", "rustls-tls"] }`. Define:

```rust
pub enum GaiaSource { TokenEnvironment(String), ExistingSnapshot(PathBuf) }
pub struct GaiaAcquisition { pub snapshot_root: PathBuf, pub revision: &'static str }
pub trait SnapshotDownloader: Send + Sync {
    fn fetch(&self, token: &SecretText, revision: &str, destination: &Path)
        -> Result<(), GaiaFetchError>;
}
```

The production downloader requests only the pinned dataset revision. Publish a `.pinvou-gaia-ready-v1` marker only after `GaiaDataset::verify` passes. Error display returns fixed codes: `gaia_access_denied`, `gaia_download_failed`, `gaia_import_failed`, `gaia_verify_failed`.

- [ ] **Step 4: Run GREEN**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia fetch_ --offline`

Expected: fake downloader/import tests pass without network.

- [ ] **Step 5: Commit**

```text
git add pinvou-cli/Cargo.lock pinvou-cli/crates/adapter-gaia
git commit -s -m "feat(eval): 获取并验证 GAIA 官方快照"
```

### Task 6: Export official-format submissions safely

**Files:**
- Create: `pinvou-cli/crates/adapter-gaia/src/submission.rs`
- Modify: `pinvou-cli/crates/adapter-gaia/src/lib.rs`
- Modify: `pinvou-cli/crates/adapter-gaia/tests/gaia_contract.rs`

- [ ] **Step 1: Write submission RED tests**

Construct a reopened durable `CompletedRun`; assert deterministic task order and exact public keys, reject missing prediction/partial coverage, reject overwrite and symlink destination, and scan output for prompt/reference/tool/session sentinels.

```rust
let lines = read_jsonl(&artifact.path());
assert_eq!(lines[0]["task_id"], "safe-task-1");
assert_eq!(lines[0]["model_answer"], "candidate");
assert!(lines[0].get("ground_truth").is_none());
```

Freeze the pinned official `app.py` contract to exactly two input keys per JSONL line: `task_id` and `model_answer`. Do not emit the scorer's derived `id`, `score`, or `level` fields and do not infer additional fields.

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia submission_ --offline`

Expected: missing submission writer.

- [ ] **Step 3: Implement atomic private export**

Resolve each prediction only through the run-bound scorer view. Open a sibling temporary file with `create_new`, use private permissions, write one compact JSON object per line, sync, then publish without overwrite. Reject destination parent symlink/reparse and non-regular existing target. Never include reference, question, attachment, backend handle, error, tool observation, or session ID.

- [ ] **Step 4: Run GREEN**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia submission_ --offline`

Expected: submission tests pass.

- [ ] **Step 5: Commit**

```text
git add pinvou-cli/crates/adapter-gaia
git commit -s -m "feat(eval): 导出 GAIA 官方提交文件"
```

### Task 7: Register and compose GAIA in the unified CLI

**Files:**
- Modify: `pinvou-cli/crates/cli/Cargo.toml`
- Modify: `pinvou-cli/crates/cli/src/lib.rs`
- Modify: `pinvou-cli/crates/cli/tests/cli_contract.rs`
- Modify: `pinvou-cli/Cargo.lock`

- [ ] **Step 1: Write parser/list RED tests**

Add exact parsing contracts for fetch/verify/run/score/submission and availability:

```rust
assert_eq!(parse(&["benchmark", "run", "gaia", "--split", "validation", "--level", "1"]),
    BenchmarkCommand::RunGaia { split: "validation".into(), level: 1 });
assert_eq!(gaia_spec.availability(), BenchmarkAvailability::Available);
assert_eq!(gaia_spec.score_kind(), "official_compatible_local");
```

Reject Level 2/3, test execution, missing output, raw `--token`, mutable revision, and task filtering in official-compatible mode.

- [ ] **Step 2: Run parser RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli --no-default-features --test cli_contract gaia_ --offline`

Expected: GAIA remains planned/not_available.

- [ ] **Step 3: Add GAIA commands and dependency**

Add `adapter-gaia = { path = "../adapter-gaia" }`. Introduce typed variants:

```rust
FetchGaia { token_env: Option<String>, source: Option<PathBuf> },
VerifyGaia { source: PathBuf },
RunGaia { split: String, level: u8 },
ScoreGaia { run_id: String },
SubmissionGaia { run_id: String, output: PathBuf },
```

The static registry marks only GAIA validation Level 1 available. BFCL/WorkBuddy remain planned.

- [ ] **Step 4: Compose run/resume/score**

Run flow: locate verified snapshot marker, construct `GaiaDataset`, `GaiaAdapter`, and `GaiaPrivateInputs`; capture product suite identity; create a manifest with split validation, pass 1, concurrency 1, exact revisions, and `pinvou-gaia-public-web/v1`; call `BenchmarkService::run_adapter` inside `run_with_product_backend`.

Score/report flow must reopen `RunStore::completed_run`, call `score_adapter`, and render revisions/completeness. If `is_complete` is false, human/JSON output must use `unofficial_partial` and omit a comparable accuracy field.

- [ ] **Step 5: Add feature-off behavior tests**

With `--no-default-features`, fetch/import/verify/score/submission remain usable; only run/resume require product backend and fail with `product_backend_not_enabled`. Ensure no anyhow chain is printed.

- [ ] **Step 6: Run GREEN CLI tests**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli --no-default-features --test cli_contract gaia_ --offline`

Expected: all GAIA CLI contracts pass.

- [ ] **Step 7: Commit**

```text
git add pinvou-cli/Cargo.lock pinvou-cli/crates/cli
git commit -s -m "feat(cli): 接入 GAIA 官方评测命令"
```

### Task 8: Document and verify the real gated Level 1 workflow

**Files:**
- Create: `docs/gaia-benchmark.md`
- Modify: `PROGRESS.md`
- Modify: `pinvou-cli/crates/adapter-gaia/tests/gaia_contract.rs`

- [ ] **Step 1: Write documentation contract RED test**

Read `docs/gaia-benchmark.md` and assert it contains exact sections: `Access and gating`, `Pinned revisions`, `Fetch or import`, `Validation Level 1`, `Official scorer compatibility`, `Submission`, `Privacy`, `Not a leaderboard score`, and `Known platform limits`.

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia docs_ --offline`

Expected: document missing.

- [ ] **Step 3: Write user documentation**

Document both fetch commands, `HF_TOKEN` safety, exact revisions, all CLI commands, report interpretation, validation contamination warning, test private-answer boundary, Windows attachment platform gate if still present, and explicit no-auto-upload behavior.

- [ ] **Step 4: Run static/package verification**

Run:

```text
cargo fmt --manifest-path pinvou-cli/Cargo.toml --all -- --check
cargo test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core -p adapter-gaia -p pinvou-cli --no-default-features --offline
python scripts/architecture-guard.py
git diff --check
```

Expected: all pass. If disk pressure reappears, stop before destructive cleanup and use package-specific `cargo clean -p` only.

- [ ] **Step 5: Run local official snapshot verification**

Run:

```text
pinvou benchmark fetch gaia --token-env HF_TOKEN
pinvou benchmark verify gaia --source <private-snapshot-directory>
```

Expected: pinned revision verified; output contains no question, answer, token, or attachment absolute path.

- [ ] **Step 6: Run the real agent Level 1 validation**

Run:

```text
pinvou benchmark run gaia --split validation --level 1 --output json
pinvou benchmark score gaia --run <run-id> --output json
pinvou benchmark report <run-id>
pinvou benchmark submission gaia --run <run-id> --output <private-output.jsonl>
```

Expected: every official Level 1 validation task reaches a durable terminal outcome; complete runs return official-compatible local accuracy, incomplete runs return partial/unofficial without comparable accuracy.

- [ ] **Step 7: Cross-check the pinned Python scorer**

In a private temporary directory, resolve the same candidates and validation references, run pinned `scorer.py` revision `1349a179...`, and compare the per-task booleans with Rust output. Do not write references or candidates into the repository.

Expected: exact per-task equality.

- [ ] **Step 8: Privacy and cleanup audit**

Scan manifest/events/predictions/report and Git changes for question/reference/token/attachment sentinels; confirm no `.tmp`, eval session, or attachment staging residue remains after completion/cancel.

- [ ] **Step 9: Commit**

```text
git add docs/gaia-benchmark.md PROGRESS.md pinvou-cli/crates/adapter-gaia/tests/gaia_contract.rs
git commit -s -m "docs(eval): 记录 GAIA 官方评测流程"
```

### Task 9: Final two-stage review and migration truth check

**Files:**
- Review only: all commits from Task 1 through Task 8

- [ ] **Step 1: Spec compliance review**

Verify every section of `docs/superpowers/specs/2026-08-13-gaia-official-adapter-design.md`, with special attention to complete-vs-partial labeling, private test answers, pinned revisions, attachment containment, and durable reopened scoring.

- [ ] **Step 2: Quality/security review**

Review parser ambiguity, Parquet resource limits, symlink/reparse TOCTOU, token lifetime, scorer numeric edge cases, JSONL overwrite behavior, cancellation cleanup, private prediction access, and human/JSON secret leakage.

- [ ] **Step 3: Fix findings with TDD and re-review**

Each blocking finding gets a failing focused test, a minimal fix, focused GREEN evidence, a signed commit, then the same reviewer verifies closure.

- [ ] **Step 4: Final honest status**

Declare GAIA Level 1 validation available only if the real pinned snapshot and complete product run succeeded. Otherwise declare the exact remaining gate (`gaia_access_denied`, platform attachment unsupported, incomplete run, or build environment failure); do not mark the adapter available prematurely.
