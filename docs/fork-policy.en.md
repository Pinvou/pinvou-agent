# Pinvou CodeWhale Fork Policy

> Updated: 2026-10-31. The public maintenance head f1eb10efc is unmoved (contains #114/#125); this tree's parent gitlink and `EXPECTED_HEAD` lead the public maintenance head and point at the review candidate 41eab46f2 (CodeWhale#131), to be re-pinned once #131 lands.
> Canonical Chinese policy: [`docs/fork-policy.md`](fork-policy.md). This English page is a condensed summary; the Chinese version is the complete, authoritative process.

Integrated upstream backport and historical provenance: [modification register](fork-modifications.en.md#web-jsonpath-upstream-backport-integrated-historical-candidate-evidence-756). The public maintenance gate and section 6 authorization requirement continue to apply.

## 0. Current maintenance baseline and validation boundary

- The public maintenance head remains `f1eb10efcfe2468fde42d1724d7f5f19d54b904c` (closed 2026-10-28, containing #114 as `fe90f72f9` and #125 as `f1eb10efc`) and is unmoved; this PR sits in a candidate window — the parent gitlink and the `EXPECTED_HEAD` in `scripts/fork-guard.sh` lead the public maintenance head and point at the review candidate `41eab46f2fe2f6cb27f45bc9931afd4060f7c87b` (CodeWhale #131, based on `f1eb10efc`). The public maintenance branch has not advanced.
- Measured history: the public maintenance branch holds 86 commits above upstream v0.9.12 `dcd4c200f`, 84 with a DCO sign-off (the unsigned squash commits remain #107 `f27672935` and #116 `e9a7f48da1`); the candidate `41eab46f2` holds 88 commits, 86 with a sign-off — the two new commits are #131's review history (the registered single commit `998ffe27e` plus the follow-up hardening commit `41eab46f2`), and the unsigned exceptions are unchanged. Published maintenance history is preserved.
- Immutable `pinvou-v0.9.12-r3` remains at `61cb769be5b33abc64f64da4272f5b39a8b6c1fd`; r1 remains at `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf`. The public verifier still requires maintenance-head equality and the frozen r3 tag; no predicate is relaxed. Inside the candidate window `scripts/verify-public-submodule.sh` stays red by design (it asserts gitlink equals the `pinvou3-clean` remote maintenance head, still `f1eb10efc`); this tree's receipt is a green `fork-guard.sh --fast` (Layers 0+1), and the registration batch after #131 lands re-pins `EXPECTED_HEAD` and the gitlink to the squash, restoring the gate to green.
- Measured drift (`dcd4c200f..41eab46f2`): 525 files, +93293/-39351, net +53942; 268 distinct `forkguard_*` names (floor remains 64). Earlier native Windows receipts retain their recorded commit boundaries; they do not prove execution on this integrated tree. Current parent CI and final integrated-tree review remain separate requirements.

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
