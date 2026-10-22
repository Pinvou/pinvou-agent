# Pinvou CodeWhale Fork Policy

> Updated: 2026-10-22. Inside the transition window after the r3 closure: the immutable tag `pinvou-v0.9.12-r3` stays pinned at its closure commit `61cb769be` while the registered head advanced to `0bc868ff0` with the 2026-10-08/09 batch, again to `3118c36b8a` with the 2026-10-12/13 batch (upstream absorptions #90/#91/#93/#97/#98), and to `480bebf61` with the 2026-10-13/14 batch (#92/#96/#101/#102/#103/#104), and to `803e672bb` with the 2026-10-14 batch (#54/#107), and again to `13ee4dfd2` with the 2026-10-15 batch (#108), and to `c08c8c022` with the 2026-10-17 batch (#105/#106/#111), and to `083cebfd9` with the 2026-10-20 batch (#112/#113) — the published-state parent gitlink tracks the maintenance branch and leads the tag; this tree’s independent review candidate and blocking gate are described below; `verify-public-submodule.sh` asserts the transition pinning via `TRANSITION_BASELINE`.
> Canonical Chinese policy: [`docs/fork-policy.md`](fork-policy.md). This English page is a condensed summary; the Chinese version is the complete, authoritative process.

## T2 review candidate: Recover engine-visible tool output

- Adopt the recoverable output-budget slice of upstream #6619 (`3e39820f530496dec909a57541f25f3f6f63ac9f`) plus the foundation review fixes. Share the route budget between the engine, Web wrapper, Chat Completions backstop and TUI mirror; preserve the engine's complete input before serial/parallel fan-out with immutable artifacts, truthful save-failure hints and existing retrieval/ownership boundaries. Fork child-route/handle summaries stay bounded.
- Review closure: `cf82289a2` restores bounded failure-summary wiring and updates both sides of the wire-budget/dedup contract and the receipt/body recovery bound. The new candidate tracks actual field/row/stream omissions rather than formatting changes, so complete structured results request no recovery artifact. Adaptive previews record original line totals; repeated unsaved previews are not advertised as full-content back-references. Small-budget footers stay single and bounded; corrupt preview lengths fail closed.
- Additional exact-head review: keep distinct nonempty sibling stdout/stderr summaries inline for `tasks gate_run`; real field cuts still retain recovery, including execution with no active task/log artifact. Short interrupted child snapshots also retain recovery for omitted `checkpoint`/`needs_input` fields, whether on the row or its wrapper. Regressions use the actual TasksTool result and the real interrupted-checkpoint/needs-input DTO builder; complete rows without these omissions stay unchanged.
- Scope decision for R4: only output that reaches the engine is recoverable. Existing producer caps remain: `git.rs` and `git_history.rs` 40,000 characters; `test_runner.rs` 40,000 per stream; `verifier.rs` 16,000 per gate stdout/stderr stream. Tails discarded inside those tools never reach the engine and cannot be recovered by this slice. Removing those caps is explicitly deferred, rather than silently included in the upstream backport.
- Separate search decision: the native-search answer-cap chain remains at its existing 2,048/4,096-token limits; provider length-stops are not newly marked here. Its cap/finish-reason changes are deferred to a separately reviewed foundation slice. The Responses provider gets the engine view for normal new execution, but has no added legacy-history wire backstop; the Chat Completions legacy backstop remains route-independent. No claim of a universal provider-wire backstop is made.
- Remaining nonblocking boundaries: Web envelope artifacts are not re-advertised as recovery refs on later route-budget shrink; legacy previews without original line metadata retain their conservative visible-line fallback. The existing /status raw-over-cap copy and session-relative artifact-size observation remain follow-ups. No new packaged-host or live-provider evidence is claimed.
- Candidate gitlink: `293d7376a82b59189c01503f71f1c83880a33a4c` (81 commits above v0.9.12), based on public maintenance `083cebfd9aac5f6b9993ed610383fe2b38c6785f`. This independent candidate does not contain the separate failure-flag, JSONPath or Python-env candidates. Public maintenance and immutable tags remain unchanged.
- Merge dependency: first land CodeWhale#118, then replace the candidate gitlink, `EXPECTED_HEAD` and both register heads with the actual public maintenance SHA; verify `EXPECTED_UPSTREAM`, recount `EXPECTED_COMMITS`, reconcile any other landed fixes and synchronize both policy/register pairs. Pass unchanged `verify-public-submodule.sh` and review the final integrated tree. Candidate checks do not waive that gate; this draft is not merge or release eligible.
- Verification is recorded against exact commits in the carrying PR. Fingerprints pin the header-padded Web regression, complete/cut structured-result behavior, adaptive line-count regression, corrupt preview metadata and repeated unsaved-wire previews. Changelog edits are deliberately absent: CodeWhale `CONTRIBUTING.md` requires entries on main at merge time, not on PR branches. Customer data, credentials and runtime/session logs are excluded.

## 0. Current maintenance baseline and validation boundary

- Public maintenance and parent gitlink: `ea9fdc90f1c90335554833195eeeb847c83d82dd` (2026-10-23). CodeWhale#115 squash-merged as `7d26369934`, then #116 as `e9a7f48da1`. This follow-up changes parent references only; it does not advance maintenance. #117 merged remotely on 2026-10-23 as `ea9fdc90f`.
- Measured history: 79 commits above upstream v0.9.12 `dcd4c200f`, 64 above r1 `1fafee7e2`; 77 carry DCO sign-offs. The unsigned squash commits are #107 `f27672935` and the new #116 `e9a7f48da1`. The previous single-exception claim no longer applies. Published maintenance history is preserved.
- Immutable `pinvou-v0.9.12-r3` remains at `61cb769be5b33abc64f64da4272f5b39a8b6c1fd`; r1 remains at `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf`. The public verifier still requires maintenance-head equality and the frozen r3 tag; no predicate is relaxed.
- Measured drift: 504 files, +86011/-37667, net +48344; 231 distinct `forkguard_*` names (floor remains 64). Earlier native Windows receipts retain their recorded commit boundaries; they do not prove execution on this integrated tree. Current parent CI and final integrated-tree review remain separate requirements.

- The public pre-upgrade rollback point is immutable tag `pinvou-v0.9.5-r13` at `f853f8f1566c57e6be40d5439a222a932aa79ef5`; local `backup/pre-v0.9.12-sync` at the same SHA is only a convenience ref.
- r1 is the protected consumable baseline. At each rN closure the parent gitlink, maintenance branch, and immutable tag resolve to the same commit.
- Transition exemption (from 2026-09-11): between two rN closures the parent gitlink may advance along `pinvou3-clean` ahead of the immutable tag. During the transition `scripts/verify-public-submodule.sh` asserts gitlink equals the public maintenance-branch head and the immutable tag stays pinned at its closure commit; the next rN closure cuts a fresh immutable tag at the merged head and restores three-way equality.
- Keep four long-lived topics, three appended reduction topics, and three merged-into-maintenance topics:

  1. Host embedding and routing boundary
  2. Tool compatibility and command-execution safety
  3. Embedded context and Skill sources
  4. Automation and runtime lifecycle
  5. Session archive export (T5, appended)
  6. Swarm rate-limit governance (T6, appended)
  7. Compaction checkpoint role compatibility (T7, merged into the maintenance branch)
  8. Roster presentation of host profiles (T8, appended)
  9. Custom-wire runtime route (T9, merged into the maintenance branch)
  10. Tachyon reasoning-effort dialect (T10, merged into the maintenance branch)

The exact commits and fingerprints are recorded in [`docs/fork-modifications.md`](fork-modifications.md).


## Rules

- Prefer the app bridge, bundle instructions/Skills, MCP/connectors/plugins, then an upstream contribution. Keep a fork patch only when the behavior must be atomic inside CodeWhale's Engine, SubAgent, Task, or Automation lifecycle.
- Product tool policy, UI, workspace selection, and business routing stay in `pinvou3-app`.
- The soft drift limits remain 1,500 net added lines and 200 fork-distinct lines per file. The v0.9.12 r1 baseline is 94 files and `+5022/-944` (net 4,078), down from v0.9.5 r13 at 110 files and `+10895/-1195`. Newly touched files include equivalent Rust/rustdoc release-lint adjustments, review-requested result-level lifecycle and evaluation regressions, the API-search fallback reachability/error-guidance fix, removal of an obsolete upstream-comparison test helper, an exact runtime-contract budget ratchet for the official v0.9.12 plus Pinvou r1 model-visible schemas, a bounded macOS cold-build timeout, and overdue one-shot delivery; they add no fork behavior topic. The remaining excess is justified by Engine/Task-atomic steer, final-dispatch security, host prompt/profile/Skills ownership, Automation lifecycle behavior, and their safety tests. Upstreaming priority is generic steer and per-turn security first, Automation lifecycle second, parent migration to narrow re-export APIs followed by retirement of the 18-module compatibility facade, then replacement of prompt/profile/Skills ownership with stable host APIs.
- Fixups are squashed into their owning topic; no long-lived catch-up commit chains are maintained, and generic host configuration, routing, tools, Automation, and OAuth must remain within their owning boundary.
- A fork-distinct change must update the modification register and guard fingerprints, include a result-oriented `forkguard_*` test where applicable, and pass `./scripts/fork-guard.sh --fast`.
- For a large upstream refactor, clean re-fork from the release tag and re-express each surviving topic. Do not preserve merge-conflict batches as long-lived history.
- Candidate review branches may be pushed for a PR. Update the protected maintenance branch and create an immutable tag only after explicit authorization. The published tag, maintenance branch, and parent gitlink resolve to the same commit and must remain aligned.

## Required verification

```bash
./scripts/fork-guard.sh --fast
cargo check --manifest-path CodeWhale/Cargo.toml -p codewhale-tui --lib --locked
cargo test --manifest-path CodeWhale/Cargo.toml -p codewhale-tui --lib --locked \
  forkguard_ -- --test-threads=1
cargo test --manifest-path CodeWhale/Cargo.toml -p codewhale-tui --lib --locked \
  --features benchmark-eval-controls forkguard_benchmark_ -- --test-threads=1
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml --locked
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml --all-targets \
  --features benchmark-hooks --locked
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib --locked \
  -- --test-threads=1
python3 scripts/architecture-guard.py
```

Automated gates do not replace real-model, GUI, MCP/OAuth, and scheduled-task acceptance.
