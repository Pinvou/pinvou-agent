# GAIA NativeTurn Tool Policy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enforce registered GAIA public-web and offline tool profiles from benchmark manifest through the real product runtime without changing ordinary GUI behavior.

**Architecture:** Carry an opaque validated policy ID through the backend API and bind it to each prepared session. The app maps it to a hardened Agent turn; exact catalog filtering, dispatch validation, a `ToolCallBefore` hook, and workspace containment form independent gates. The design is feasible without CodeWhale changes because current `Op::SendMessage` already carries allowlist, approval, provenance and hook controls, while app config constructs `ToolContext`; inability to enforce app-side dispatch/hook or to clear trusted roots is a release blocker, never grounds for weakening the policy.

**Tech Stack:** Rust 2024, async-trait, Tokio, CodeWhale EnginePool/Op gates, Tauri product-runtime contract tests

---

## File map

- `pinvou-cli/crates/agent-backend-api/src/lib.rs`: safe policy-ID prepare contract.
- `pinvou-cli/crates/benchmark-core/src/runner.rs`: forward `NativeTurn.tool_policy`.
- `pinvou3-app/src-tauri/src/features/assistant/product_runtime/eval_tool_policy.rs`: registered profiles and pure projection.
- `pinvou3-app/src-tauri/src/features/assistant/product_runtime/mod.rs`: optional eval policy; GUI default remains `None`.
- `pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs`: app-only hardened turn builder and deny hook.
- `pinvou3-app/src-tauri/src/headless_bridge.rs`: validate/bind policy to session and submit it.
- Existing contract/unit test files beside those modules: API, forwarding, catalog, dispatch, path, network and lifecycle tests.
- No file under `CodeWhale/` is modified by this plan.

### Task 1: Freeze the policy-ID API contract

**Files:**
- Modify: `pinvou-cli/crates/agent-backend-api/src/lib.rs`
- Test: `pinvou-cli/crates/agent-backend-api/tests/backend_contract.rs`

- [ ] **Step 1: Write the failing contract test**

```rust
let policy = AgentToolPolicyId::new("pinvou-gaia-offline/v1").unwrap();
let request = PrepareRequest::new("case-1", vec![]).with_tool_policy(policy.clone());
assert_eq!(request.tool_policy(), Some(&policy));
assert!(AgentToolPolicyId::new("token=secret").is_err());
assert!(PrepareRequest::new("legacy", vec![]).tool_policy().is_none());
```

Also assert Debug contains no credential sentinel.

- [ ] **Step 2: Observe RED**

```powershell
cargo +1.97.1-x86_64-pc-windows-msvc test --manifest-path pinvou-cli/Cargo.toml -p agent-backend-api --offline --test backend_contract
```

Expected: compilation fails because the safe type/accessors do not exist.

- [ ] **Step 3: Add the narrow type and optional prepare field**

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolPolicyId(String);

impl AgentToolPolicyId {
    pub fn new(value: impl Into<String>) -> Result<Self, UnsafeAgentToolPolicyId>;
    pub fn as_str(&self) -> &str;
}
```

Use conservative safe-identity validation. Add `tool_policy: Option<AgentToolPolicyId>` plus
`with_tool_policy`/`tool_policy` to `PrepareRequest`; preserve its legacy constructor.

- [ ] **Step 4: Observe GREEN and commit**

Run the same API test, then:

```powershell
git add pinvou-cli/crates/agent-backend-api/src/lib.rs pinvou-cli/crates/agent-backend-api/tests/backend_contract.rs
git commit -s -m "feat(eval): 增加评测工具策略契约"
```

### Task 2: Forward NativeTurn policy

**Files:**
- Modify: `pinvou-cli/crates/benchmark-core/src/runner.rs`
- Test: `pinvou-cli/crates/benchmark-core/tests/run_core_contract.rs`

- [ ] **Step 1: Add a failing mock-backend test**

Record `PrepareRequest::tool_policy`; assert a task with `pinvou-gaia-offline/v1` prepares with that exact
ID. Invalid conversion must return fixed `unsupported_tool_policy` with zero prepare/run calls.

- [ ] **Step 2: Observe RED**

```powershell
cargo +1.97.1-x86_64-pc-windows-msvc test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --offline --test run_core_contract
```

Expected: assertion fails because runner currently discards the policy.

- [ ] **Step 3: Validate and forward at prepare**

```rust
let policy = AgentToolPolicyId::new(tool_policy.as_str())
    .map_err(|_| BenchmarkError::coded("unsupported_tool_policy"))?;
let request = PrepareRequest::new(task.task_id(), attachments)
    .with_resolved_attachments(resolved_attachments)
    .with_tool_policy(policy);
```

Do not add policy to observer, private output or persisted outcome; immutable manifest already records it.

- [ ] **Step 4: Observe GREEN and commit**

Run focused and full offline core tests, then commit the two owned files with
`feat(eval): 传递原生任务工具策略` and DCO sign-off.

### Task 3: Register and validate exact app profiles

**Files:**
- Create: `pinvou3-app/src-tauri/src/features/assistant/product_runtime/eval_tool_policy.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/product_runtime/mod.rs`
- Test: unit tests in the new module

- [ ] **Step 1: Write failing registry tests**

```rust
assert!(resolve_eval_policy("pinvou-gaia-public-web/v1")?.allows("fetch_url"));
assert!(!resolve_eval_policy("pinvou-gaia-offline/v1")?.allows("fetch_url"));
assert_eq!(resolve_eval_policy("unknown/v1").unwrap_err().code(), "unsupported_tool_policy");
```

Assert no duplicates and every candidate name exists exactly once in a real catalog fixture. Candidates are
listed in `docs/gaia-native-turn-tool-policy.md`; absent names are removed, not replaced by broader tools.

- [ ] **Step 2: Observe RED using the smallest app test target**

If a Tauri build produces no diagnostic for 60 seconds, stop rather than repeatedly compiling; retain the
static review and let the integration owner run the feature build once.

- [ ] **Step 3: Implement immutable registered profiles**

```rust
pub enum EvalToolPolicy { GaiaPublicWebV1, GaiaOfflineV1 }
pub enum EvalNetworkClass { PublicWeb, Offline }
pub struct EvalTurnPolicy {
    pub allowed_tools: &'static [&'static str],
    pub network: EvalNetworkClass,
}
```

Add `eval_policy: Option<EvalToolPolicy>` to internal `TurnInput`. GUI constructors set `None`; only headless
sets `Some`.

- [ ] **Step 4: Snapshot the real catalog and commit**

Assert projected schema names equal the verified profile snapshot. Commit app profile files with
`feat(eval): 注册GAIA安全工具策略` and DCO sign-off.

### Task 4: Build the hardened app-only turn path

**Files:**
- Modify: `pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs`
- Modify: `pinvou3-app/src-tauri/src/features/assistant/product_runtime/mod.rs`
- Test: bridge/product-runtime unit tests

- [ ] **Step 1: Freeze GUI behavior first**

Snapshot ordinary `eval_policy=None` Yolo and Plan fields: mode, shell, trust, approval, allowed/dynamic tools,
provenance and hook. These snapshots must not change.

- [ ] **Step 2: Write failing hardened projection tests**

For both profiles assert Agent mode, shell/trust/auto-approve false, approval Never, exact allowlist,
no dynamic tools, ImportedTranscript provenance, empty trusted roots and follow-symlinks false. Verify Never
still executes an allowlisted no-approval read tool and fails closed for approval-required tools.

- [ ] **Step 3: Add a separate eval builder**

Add app-level `build_eval_send_message_op`/`send_eval_user_message`; do not alter the result of ordinary
`build_send_message_op`. Reuse routing/compaction/model plumbing, then overwrite authority structurally.

- [ ] **Step 4: Install the mandatory second dispatch gate**

Install a `ToolCallBefore` hook that denies any name outside the exact profile. Test a forged hidden call reaches
the hook and is rejected. This is a hard release gate: if app hooks cannot enforce a second execution-time deny,
stop implementation and report blocked; do not ship catalog-only filtering and do not weaken the spec.

- [ ] **Step 5: Force the eval filesystem context**

At app config/session construction, set trusted external roots empty and follow-symlinks false. If this cannot
be isolated from GUI with current app seams, stop as blocked and prepare a separate fork proposal; do not silently
inherit user trust settings.

- [ ] **Step 6: Observe GREEN and commit**

Run focused snapshots and forged dispatch tests. Commit with `feat(eval): 隔离原生评测工具权限` and
DCO sign-off.

### Task 5: Bind policy to the headless session

**Files:**
- Modify: `pinvou3-app/src-tauri/src/headless_bridge.rs`
- Test: `pinvou3-app/src-tauri/tests/headless_bridge_contract.rs`

- [ ] **Step 1: Write failing lifecycle tests**

Using a mock runtime, assert prepare validates/stores policy, run forwards the same enum, missing/unknown policy
fails before model use, cancel/close/prepare failure clears it, and one task cannot replace another session's
binding.

- [ ] **Step 2: Add session policy state**

Store only the resolved enum keyed by opaque session ID beside existing attachment state. Never store prompt,
source path, query or tool arguments.

- [ ] **Step 3: Replace headless Yolo submission**

Submit Agent mode plus `eval_policy=Some(bound_policy)`. Keep every GUI `TurnInput` at `None`.

- [ ] **Step 4: Verify fixed errors and cleanup**

Unknown, missing and mismatched policy branches return only fixed codes. Timeout/cancel/close clears policy even
when runtime cleanup errors; attachment/private-output cleanup remains intact.

- [ ] **Step 5: Observe GREEN and commit**

Run the focused bridge contract once and commit with `feat(eval): 绑定产品会话工具策略` plus DCO sign-off.

### Task 6: Add adversarial security coverage

**Files:**
- Test: closest app bridge/product-runtime security test modules
- Test: `pinvou3-app/src-tauri/tests/headless_bridge_contract.rs`

- [ ] **Step 1: Test path containment**

Relative staged read succeeds. Absolute outside, `..`, persisted trusted root, workspace symlink, Windows
junction/reparse, and non-existent descendant below an escaping link all fail.

- [ ] **Step 2: Test forged tool calls**

An adversarial mock model directly emits shell, code, write, subagent, MCP, dynamic and tool-search calls.
Each is rejected at execution time and produces no process/file/tool side effect.

- [ ] **Step 3: Test both network profiles**

Public-web permits a normal public target but rejects localhost, RFC1918, link-local, metadata, `file://` and
redirect-to-private. Offline exposes no network schema and rejects forged network calls. Bound call count,
redirects, bytes and the shared task deadline.

- [ ] **Step 4: Test prompt injection and confidentiality selection**

Malicious attachment/web text asks to read home, run shell, write, spawn and exfiltrate. Assert structural gates
block forbidden capabilities. Confidential input plus public-web must fail before model invocation; public GAIA
provider egress is explicitly accepted and reported.

- [ ] **Step 5: Test Office/image boundaries**

Office/PDF ingest remains host-side with size/deadline limits and no Office/shell tool. Images are workspace
contained; offline rejects remote vision, public-web records only canonical tool name/status/elapsed.

- [ ] **Step 6: Commit tests**

Commit focused security tests with `test(eval): 覆盖GAIA权限对抗场景` and DCO sign-off.

### Task 7: Final verification and fork-boundary audit

**Files:**
- Verify: all Task 1-6 files
- Modify docs only when catalog verification changes exact names: `docs/gaia-native-turn-tool-policy.md`

- [ ] **Step 1: Run lightweight suites**

```powershell
cargo +1.97.1-x86_64-pc-windows-msvc test --manifest-path pinvou-cli/Cargo.toml -p agent-backend-api --offline
cargo +1.97.1-x86_64-pc-windows-msvc test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --offline
```

Run the smallest app policy/headless target once; do not repeatedly compile full Tauri.

- [ ] **Step 2: Run formatting and guards**

```powershell
rustup run 1.97.1-x86_64-pc-windows-msvc cargo fmt --manifest-path pinvou-cli/Cargo.toml --all -- --check
python scripts/architecture-guard.py
git diff --check
```

- [ ] **Step 3: Audit privacy and GUI compatibility**

Search changed serialization, Debug and logs for prompt, answer, paths, arguments, queries and URLs. Confirm
ordinary GUI snapshots and every `eval_policy=None` constructor remain unchanged.

- [ ] **Step 4: Prove no fork change**

`git status --short CodeWhale` must show no new submodule/gitlink change attributable to this work. If a hard
gate cannot be met app-side, stop; create a separate fork proposal with docs/fingerprint/behavior tests and
`./scripts/fork-guard.sh --fast` rather than weakening the gate.

- [ ] **Step 5: Run change-impact check**

Run GitNexus `detect_changes(scope="compare", base_ref="main")`. If unavailable, record HIGH-risk fallback,
enumerate every policy/TurnInput caller with `rg`, and inspect `git diff --stat` plus `git diff --check`.

- [ ] **Step 6: Commit verified documentation corrections only**

If real catalog names changed the spec, commit only that correction with
`docs(eval): 校准GAIA工具策略清单` and DCO sign-off.
