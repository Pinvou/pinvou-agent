# Pinvou CodeWhale Fork Policy

> Updated: 2026-10-20. Inside the transition window after the r3 closure: the immutable tag `pinvou-v0.9.12-r3` stays pinned at its closure commit `61cb769be` while the registered head advanced to `0bc868ff0` with the 2026-10-08/09 batch and again to `480bebf61` with the 2026-10-13/14 batch (#92/#96/#101/#102/#103/#104), and to `803e672bb` with the 2026-10-14 batch (#54/#107), and again to `13ee4dfd2` with the 2026-10-15 batch (#108), and to `c08c8c022` with the 2026-10-17 batch (#105/#106/#111), and again to the T10 candidate head `3f1ecf8dd` with the 2026-10-19 T10 candidate batch — the T10 candidate window pins the parent gitlink directly at the candidate commit (a new pinning mode with no historical precedent: earlier batches all re-pinned the public head only after the squash merge), so the "gitlink = public maintenance-branch head" leg of `verify-public-submodule.sh` remains blocking during the disclosed candidate window and doubles as merge-order enforcement; the `TRANSITION_BASELINE` leg is unaffected. After CodeWhale#113 squash-merges, the parent re-pins the gitlink, `EXPECTED_HEAD`/`EXPECTED_COMMITS`, and the ledgers (zh and en) to the squash SHA.
> Canonical Chinese policy: [`docs/fork-policy.md`](fork-policy.md). This English page is a condensed summary; the Chinese version is the complete, authoritative process.

## Baseline

- Upstream: `Hmbown/CodeWhale` `v0.9.12` at `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5`.
- Current fork baseline: `Pinvou/CodeWhale:pinvou3-clean` at head `c08c8c022a31aa4bd4a5a146887690de99495786` (leading the immutable tag `pinvou-v0.9.12-r3`, which stays pinned at its closure `61cb769be`), with seventy-four merged registered commits (seventy-three DCO-signed-off), plus 4 T10 candidate commits at head `3f1ecf8dd` (Tachyon reasoning-effort dialect; see fork-modifications T10 — pinned pre-squash, re-pinned after CodeWhale#113 merges) (the sole unsigned one: the #107 squash `f27672935`, whose merge commit message dropped the sign-off trailer — an upstream merge-message gap reported to the maintainers — see CodeWhale#107; later batches must recount and record the then-actual signed count); the immutable tag `pinvou-v0.9.12-r1` stays pinned at the r1 closure `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf` (fifteen commits formed through CodeWhale PR #44 and fast-follow PR #46), followed by fifty-nine squash-merged PRs: thirteen from the 2026-09-10/11 backlog batch, five from the 2026-09-17 batch (#56/#58/#59/#60/#61), three from the 2026-09-18 batch (#55/#57/#62), one from the 2026-09-20 batch (#66), two from the 2026-09-21 batch (#64/#67), nine from the 2026-09-22/23 batch (#63/#65/#68/#69/#70/#71/#72/#73/#76, plus windows-compile fix #77), one from the 2026-10-02 batch (#80), seven from the 2026-10-08/09 batch (#79/#88/#89/#75/#78/#94/#95), five from the 2026-10-12/13 batch (#90/#91/#93/#97/#98), six from the 2026-10-13/14 batch (#92/#96/#101/#102/#103/#104), two from the 2026-10-14 batch (#54/#107), one from the 2026-10-15 batch (#108), and three from the 2026-10-17 batch (#105/#106/#111).
- The public pre-upgrade rollback point is immutable tag `pinvou-v0.9.5-r13` at `f853f8f1566c57e6be40d5439a222a932aa79ef5`; local `backup/pre-v0.9.12-sync` at the same SHA is only a convenience ref.
- r1 is the protected consumable baseline. At each rN closure the parent gitlink, maintenance branch, and immutable tag resolve to the same commit.
- Transition exemption (from 2026-09-11): between two rN closures the parent gitlink may advance along `pinvou3-clean` ahead of the immutable tag. During the transition `scripts/verify-public-submodule.sh` asserts gitlink equals the public maintenance-branch head and the immutable tag stays pinned at its closure commit; the next rN closure cuts a fresh immutable tag at the merged head and restores three-way equality.
- Keep four long-lived topics, three appended reduction topics, two merged-into-maintenance topics, and one candidate-window topic:

  1. Host embedding and routing boundary
  2. Tool compatibility and command-execution safety
  3. Embedded context and Skill sources
  4. Automation and runtime lifecycle
  5. Session archive export (T5, appended)
  6. Swarm rate-limit governance (T6, appended)
  7. Compaction checkpoint role compatibility (T7, merged into the maintenance branch)
  8. Roster presentation of host profiles (T8, appended)
  9. Custom wire runtime routes (T9, merged into the maintenance branch)
  10. Tachyon (驭光) engine reasoning-effort dialect (T10, candidate window)

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
