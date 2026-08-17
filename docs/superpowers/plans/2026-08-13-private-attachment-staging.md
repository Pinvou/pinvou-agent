# Private Attachment Staging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a privacy-safe attachment resolution contract and lifecycle-bound product staging without claiming runtime attachment support.

**Architecture:** `PrivateInputResolver` resolves opaque handles into non-serializable, redacted sources before prepare; `PrepareRequest` transports those sources in memory. `ProductHeadlessBackend` validates and copies sources into per-session RAII temp workspaces, then deterministically refuses attachment runs until the product runtime can consume the workspace.

**Tech Stack:** Rust 2024, async-trait, tempfile, Tokio contract tests

---

### Task 1: Freeze the API contract

**Files:**
- Modify: `pinvou-cli/crates/agent-backend-api/src/lib.rs`
- Test: `pinvou-cli/crates/agent-backend-api/tests/backend_contract.rs`

- [ ] Add failing tests proving `ResolvedAttachmentSource` and `PrepareRequest` Debug output omit a sentinel path/name, and the default resolver returns exactly `attachment_resolution_unsupported` without the handle.
- [ ] Run `cargo +1.97.1-x86_64-pc-windows-msvc test --manifest-path pinvou-cli/Cargo.toml -p agent-backend-api --offline --test backend_contract` and confirm compilation fails because the new API is absent.
- [ ] Add a cloneable `ResolvedAttachmentSource(PathBuf, String)` with custom redacted Debug and no serde; add `PrivateInputResolver::resolve_attachment` with a fixed safe default error; add `PrepareRequest::with_resolved_attachments` and redacted accessors/Debug.
- [ ] Re-run the same command and confirm all contract tests pass.

### Task 2: Stage and clean product attachments

**Files:**
- Modify: `pinvou3-app/src-tauri/Cargo.toml`
- Modify: `pinvou3-app/src-tauri/src/headless_bridge.rs`
- Test: `pinvou3-app/src-tauri/tests/headless_bridge_contract.rs`

- [ ] Add feature-gated contract tests using real temporary files. Assert valid sources are copied, attachment run returns `attachments_runtime_unsupported`, and workspaces disappear after run/cancel/close/prepare failure.
- [ ] Add rejection cases for missing files, directories, symlinks where supported, unsafe suggested names, and files over `25 * 1024 * 1024` bytes; confirm tests fail because staging is absent.
- [ ] Add `tempfile = "3"`; store `HashMap<session_id, TempDir>` behind the existing mutex-backed backend state. Validate `symlink_metadata().file_type().is_file()`, reject symlinks, enforce size and safe single-component ASCII names, then copy into the TempDir.
- [ ] On staging or runtime prepare failure, drop the workspace. On attachment run, remove the workspace before returning fixed `attachments_runtime_unsupported`. On cancel and close, remove it regardless of runtime result. Keep the empty-attachment path unchanged.
- [ ] Run rustfmt on the four owned Rust files. Run only the API offline suite; do not repeatedly compile Tauri. Inspect the feature-gated bridge test statically and run `git diff --check`.

### Task 3: Scope verification and signed commit

**Files:**
- Verify only the API, bridge, feature-gated bridge tests, necessary app manifest, and these approved docs.

- [ ] Since GitNexus is unavailable, use `rg` to enumerate every `prepare/run/cancel/close` caller and confirm default API compatibility.
- [ ] Stage only owned files, inspect `git diff --cached --name-only` and `git diff --cached --check`.
- [ ] Commit with `git -c user.name=Codex -c user.email=codex@openai.com commit -s -m "feat(eval): 增加私有附件暂存契约"`.
