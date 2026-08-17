# Run-scoped Private Prediction Store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist official benchmark predictions behind a run-scoped scorer capability so resume can score or export submissions without placing raw answers in public artifacts.

**Architecture:** `benchmark-core` owns a bounded secret payload, platform protection, and a run-scoped blob store. Durable adapters commit protected private output before public outcome/terminal records; `ScorerView` is the only read path. Windows protects every blob with current-user DPAPI, Unix enforces 0700/0600, and Smoke remains ephemeral.

**Tech Stack:** Rust 1.97.1 / edition 2024, `windows-sys` DPAPI, `sha2`, `rand`, `zeroize`, `fs2`, tempfile-based contract tests

---

## File map

- `benchmark-core/src/private_prediction.rs`: payload, store, envelope, quotas, GC and scorer view.
- `benchmark-core/src/private_protection/mod.rs`: binding and platform-neutral protection API.
- `benchmark-core/src/private_protection/windows.rs`: DPAPI.
- `benchmark-core/src/private_protection/unix.rs`: Unix permissions.
- `benchmark-core/src/contracts.rs`: retention, payload and CompletedRun capability.
- `benchmark-core/src/{adapter,runner,service,store}.rs`: policy and transactional wiring.
- `benchmark-core/tests/private_prediction_contract.rs`: focused store/security/recovery tests.
- Existing core security/run tests and Smoke contract: integration gates.

### Task 1: Freeze payload, retention and scorer contracts

**Files:**
- Modify: `pinvou-cli/crates/benchmark-core/src/contracts.rs`
- Modify: `pinvou-cli/crates/benchmark-core/src/adapter.rs`
- Modify: `pinvou-cli/crates/benchmark-core/src/lib.rs`
- Create: `pinvou-cli/crates/benchmark-core/src/private_prediction.rs`
- Modify: `pinvou-cli/Cargo.toml`
- Modify: `pinvou-cli/crates/benchmark-core/Cargo.toml`
- Create: `pinvou-cli/crates/benchmark-core/tests/private_prediction_contract.rs`

- [ ] Write failing tests for redacted payload Debug, 1 MiB rejection, Ephemeral default, and capability-free `CompletedRun::new` returning `private_prediction_unavailable`.
- [ ] Run `cargo +1.97.1-x86_64-pc-windows-msvc test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --test private_prediction_contract payload_contract --offline`; expect missing APIs.
- [ ] Add workspace/crate `zeroize`, then add `PrivateOutputRetention::{Ephemeral, DurableUntilPurge}`, bounded non-serde/non-Clone `PrivatePredictionPayload` backed by `Zeroizing<Vec<u8>>`, and `PrivatePredictionContentType::{Utf8Text, CanonicalJson}`.
- [ ] Add default `BenchmarkAdapter::private_output_retention() -> Ephemeral`; preserve existing adapters without changes.
- [ ] Add optional internal scorer capability to `CompletedRun` and public `resolve_private(&TaskOutcome)`; keep ordinary constructor capability-free and Debug redacted.
- [ ] Re-run the focused test and `adapter_contract`; expect PASS.
- [ ] Commit signed as `feat(eval): 冻结私有预测评分契约`.

### Task 2: Implement platform protection primitives

**Files:**
- Create: `pinvou-cli/crates/benchmark-core/src/private_protection/mod.rs`
- Create: `pinvou-cli/crates/benchmark-core/src/private_protection/windows.rs`
- Create: `pinvou-cli/crates/benchmark-core/src/private_protection/unix.rs`
- Modify: `pinvou-cli/crates/benchmark-core/src/lib.rs`
- Modify: `pinvou-cli/Cargo.toml`
- Modify: `pinvou-cli/crates/benchmark-core/Cargo.toml`
- Test: `pinvou-cli/crates/benchmark-core/tests/private_prediction_contract.rs`

- [ ] Add cfg-gated RED tests: Windows DPAPI round-trip/wrong binding/corrupt ciphertext/cross-run swap; Unix 0700/0600 plus unsafe permission/symlink/non-file rejection. Assert fixed errors omit payload/path/handle.
- [ ] Run the focused `platform_` tests; expect missing implementation.
- [ ] Add `sha2`, `rand`, `fs2`; on Windows add `windows-sys` with only Foundation, Security.Cryptography and System.Memory features. `zeroize` already landed in Task 1.
- [ ] Implement DPAPI with current-user scope and `CRYPTPROTECT_UI_FORBIDDEN`; entropy is SHA-256 of length-prefixed domain/run/task/type/handle. Always `LocalFree`, zeroize scratch, and never return OS error text or fall back to plaintext.
- [ ] Implement Unix create-time 0700/0600 and strict read-time permission/file-type checks. SHA-256 detects corruption but is documented as non-authenticating.
- [ ] Run all platform tests; expect PASS on the host and cfg-clean source for the other platform.
- [ ] Commit signed as `feat(eval): 增加私有预测平台保护层`.

### Task 3: Build store, quota, atomic publication and GC

**Files:**
- Modify: `pinvou-cli/crates/benchmark-core/src/private_prediction.rs`
- Modify: `pinvou-cli/crates/benchmark-core/src/store.rs`
- Test: `pinvou-cli/crates/benchmark-core/tests/private_prediction_contract.rs`

- [ ] Add RED tests for 256-bit random handles, no raw/backend handle in public JSONL, same-run resolve, cross-run refusal, all three quotas, no-clobber publication, orphan/temp GC, unknown-layout refusal and exact purge containment.
- [ ] Run focused `store_` tests; expect missing filesystem store.
- [ ] Implement a bounded binary envelope with magic/schema/content type/binding digest/plain length/protected payload. Check lengths before allocation and never serde secret bytes.
- [ ] Generate handles from 32 OS-random bytes. Write create-new private temp, flush/sync, no-clobber publish and sync Unix parent before returning.
- [ ] Implement `ScorerView::resolve(outcome)` requiring run/task/type/handle binding; do not expose paths or handle-only resolution.
- [ ] Implement 1 MiB/item, 10,000 items/run and 100 MiB/run checks before write.
- [ ] Implement direct-child-only GC/purge. Delete only recognized names; reject unknown files, dirs, symlinks/reparse points; never recursive-delete a run or broad path.
- [ ] Run all store tests; expect PASS, then commit signed as `feat(eval): 实现运行级私有预测存储`.

### Task 4: Wire durable-before-public execution and process locking

**Files:**
- Modify: `pinvou-cli/crates/benchmark-core/src/runner.rs`
- Modify: `pinvou-cli/crates/benchmark-core/src/service.rs`
- Modify: `pinvou-cli/crates/benchmark-core/src/store.rs`
- Test: `pinvou-cli/crates/benchmark-core/tests/run_core_contract.rs`
- Test: `pinvou-cli/crates/benchmark-core/tests/private_prediction_contract.rs`

- [ ] Add RED tests proving private failure writes no Completed outcome/terminal, private precedes public outcome, outcome precedes terminal, backend handle never persists, outcome-before-terminal crash recovers, missing blob refuses score, and two reopened stores share an OS lock.
- [ ] Run focused `private_prediction_` run tests; expect ordering failures.
- [ ] Stop constructing persisted `Prediction` from backend handle in runner. Resolve output before close and pass only redacted private material to service.
- [ ] Make plain `run` Ephemeral and adapter runs use `private_output_retention`. Ephemeral emits no durable prediction; Durable calls put, replaces with core handle, drops secret, appends outcome, then terminal.
- [ ] Use one `.run.lock` with `fs2` for event/outcome/private/GC/purge mutations; retain mutex only as an in-process fast path.
- [ ] Run focused and full benchmark-core offline tests; expect PASS.
- [ ] Commit signed as `feat(eval): 原子提交私有预测与公开结果`.

### Task 5: Recover scorer view and freeze official/Smoke behavior

**Files:**
- Modify: `pinvou-cli/crates/benchmark-core/src/store.rs`
- Modify: `pinvou-cli/crates/benchmark-core/src/service.rs`
- Modify: `pinvou-cli/crates/benchmark-core/tests/adapter_contract.rs`
- Modify: `pinvou-cli/crates/adapter-smoke/tests/smoke_contract.rs`
- Test: `pinvou-cli/crates/benchmark-core/tests/private_prediction_contract.rs`

- [ ] Add a GAIA-like durable fixture: run, drop every object, reopen, construct recovered CompletedRun, resolve UTF-8 candidate and score. Add private-test no-reference=`local_scoring_unavailable` while submission still resolves candidate.
- [ ] Add a Smoke test asserting no private blob/durable prediction is created.
- [ ] Run focused `recovered_` tests; expect missing scorer capability.
- [ ] Add `RunStore::completed_run` or equivalent core service API that reads public outcomes and attaches a run-scoped ScorerView. Score/submission must consume this core-created run, not caller-injected paths.
- [ ] Keep Smoke Ephemeral and its health analysis limited to status/tool/usage/latency; Judge NotConfigured must not retain answers.
- [ ] Run core adapter contracts and adapter-smoke offline tests; expect GAIA-like reopen success and unchanged Smoke goldens.
- [ ] Commit signed as `feat(eval): 恢复官方评分私有视图`.

### Task 6: Security closure and documentation

**Files:**
- Modify: `pinvou-cli/crates/benchmark-core/tests/security_contract.rs`
- Modify: `pinvou-cli/docs/adapter-contract-v1.md`
- Modify: `pinvou-cli/docs/security-model.md`
- Modify: `pinvou-cli/README.md`

- [ ] Add recursive artifact-scan tests: public files contain neither raw answer nor backend handle; Windows blob lacks plaintext; Debug/serde/errors omit answer/path/handle. Cover unsafe layout, permissions, quota, corrupt DPAPI, GC and purge.
- [ ] Run focused `private_prediction_` security tests. Fix only gaps inside the approved design until PASS.
- [ ] Document retention, ScorerView, GAIA validation/private-test, BFCL canonical JSON, explicit submission export, purge, platform boundary, quotas and fixed errors.
- [ ] Run rustfmt only on changed Rust files, then offline benchmark-core and adapter-smoke tests. Do not compile Tauri.
- [ ] Since GitNexus is unavailable, use `rg` to enumerate callers of `CompletedRun`, `record_outcome`, `score_adapter`, `write_adapter_submission` and `private_output_retention`; run `git diff --check` and scope inspection.
- [ ] Commit signed as `docs(eval): 记录私有预测存储安全边界`.

## Parallel waves

- Wave 1: Task 1 only; it freezes shared contracts.
- Wave 2: Task 2 and Task 3 pure envelope/quota logic can run in parallel. Task 2 exclusively owns `private_protection/**`; Task 3 owns `private_prediction.rs` and `store.rs`. Task 3 may use a test protection fixture until Task 2 lands, then the root owner performs the small `lib.rs` composition.
- Wave 3: Task 4, after both Wave 2 branches land.
- Wave 4: Task 5 and Task 6 documentation draft can run in parallel; Task 6 security tests wait for Task 5.
- Wave 5: Task 6 verification and final audit.

Critical path: `1 → 3 → 4 → 5 → 6`. Windows DPAPI work can proceed as `1 → 2` independently.
