# Pinvou CodeWhale Fork Policy

> Updated: 2026-10-22. T2 candidate window: the parent gitlink registers `af50096ff` above public maintenance head `083cebfd9`; immutable tag `pinvou-v0.9.12-r3` stays at closure `61cb769be`. The candidate is not merged into maintenance. The public-submodule gate remains blocking; merge and publication are pending the final re-pin described below.
> Canonical Chinese policy: [`docs/fork-policy.md`](fork-policy.md). This English page is a condensed summary; the Chinese version is the complete, authoritative process.

## Baseline

- Upstream: `Hmbown/CodeWhale` `v0.9.12` at `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5`.
- Published maintenance baseline: `Pinvou/CodeWhale:pinvou3-clean` at head `083cebfd9aac5f6b9993ed610383fe2b38c6785f` (leading the immutable tag `pinvou-v0.9.12-r3`, which stays pinned at its closure `61cb769be`), with seventy-six registered commits, seventy-five of them DCO-signed-off (the sole unsigned one: the #107 squash `f27672935`, whose merge commit message dropped the sign-off trailer — an upstream merge-message gap reported to the maintainers — see CodeWhale#107; later batches must recount and record the then-actual signed count); the immutable tag `pinvou-v0.9.12-r1` stays pinned at the r1 closure `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf` (fifteen commits formed through CodeWhale PR #44 and fast-follow PR #46), followed by sixty-one squash-merged PRs: thirteen from the 2026-09-10/11 backlog batch, five from the 2026-09-17 batch (#56/#58/#59/#60/#61), three from the 2026-09-18 batch (#55/#57/#62), one from the 2026-09-20 batch (#66), two from the 2026-09-21 batch (#64/#67), ten from the 2026-09-22/23 batch (#63/#65/#68/#69/#70/#71/#72/#73/#76, plus windows-compile fix #77), one from the 2026-10-02 batch (#80), seven from the 2026-10-08/09 batch (#79/#88/#89/#75/#78/#94/#95), five from the 2026-10-12/13 batch (#90/#91/#93/#97/#98), six from the 2026-10-13/14 batch (#92/#96/#101/#102/#103/#104), two from the 2026-10-14 batch (#54/#107), one from the 2026-10-15 batch (#108), three from the 2026-10-17 batch (#105/#106/#111), and two from the 2026-10-20 batch (#112/#113).
- Current parent candidate: `af50096ff843617f3f68f47bc6fc45bbffe3c937`, adding the two unmerged CodeWhale#115 commits above public maintenance head `083cebfd9aac5f6b9993ed610383fe2b38c6785f`; 78 commits above v0.9.12, 77 DCO-signed, and 63 above r1 (61 squash-merged plus two candidate commits). It is reviewable, not a published maintenance baseline.
- Candidate registration does not exempt the public-submodule gate: `verify-public-submodule.sh` still requires the gitlink to equal the public maintenance head, so its current failure blocks parent merge. After reviewed integration of CodeWhale#115, synchronize the CodeWhale gitlink, `EXPECTED_HEAD` / `EXPECTED_COMMITS` / `MAINTENANCE_BASE` in `scripts/fork-guard.sh`, and the current head/history in both policy files and both modification registers to the actual maintenance commit, recount, and pass the unchanged gate. Keep `EXPECTED_UPSTREAM` at v0.9.12. Parent #747 must also integrate through normal review and satisfy the dependency-security gate. Do not relax either gate or move immutable tags.
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
