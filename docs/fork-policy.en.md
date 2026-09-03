# Pinvou CodeWhale Fork Policy

> Updated: 2026-09-01. Public maintenance baseline: upstream `v0.9.5` r13, published through CodeWhale PR #32; r11 PRs #18, #21, #22, #25, #26, #27, #29, and #30 plus r12 PRs #33 and #35 are published within the existing four long-lived topics; the phase-2 PR #37 (execpolicy expressiveness and subagent wiring) is rebased onto r13 and filed pending publication — once merged into `pinvou3-clean`, tag `pinvou-v0.9.5-r14` is cut at the merged head, and the parent gitlink lands via this parent PR.
> Canonical Chinese policy: [`docs/fork-policy.md`](fork-policy.md). This English page is a condensed summary; the Chinese version is the complete, authoritative process.

## Baseline

- Upstream: `Hmbown/CodeWhale` `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c`.
- Public maintenance branch: `Pinvou/CodeWhale:pinvou3-clean` — published r13 head `f853f8f1` (`pinvou-v0.9.5-r13`); phase-2 candidate head `aaae5133b` (PR #37, 9 commits, rebased onto r13), which becomes r14 on merge.
- The pre-upgrade head `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` remains available as tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`.
- `Pinvou/CodeWhale#18`, `#21`, `#22`, `#25`, `#26`, `#27`, `#29`, and `#30` are published in r11, `#33` plus `#35` in r12, and `#32` in r13. `pinvou3-clean` and immutable tag `pinvou-v0.9.5-r13` are publicly reachable at `f853f8f1566c57e6be40d5439a222a932aa79ef5`; tags `r1` through `r13` remain immutable. PR #37 is pending at `aaae5133b`; `pinvou-v0.9.5-r14` is cut after its merge, and the parent gitlink plus `scripts/verify-public-submodule.sh` are already registered against r14 (a follow-up commit re-pins them if the merge rewrites SHAs).
- Keep exactly four long-lived topics:

  1. Host embedding and routing boundary
  2. Tool compatibility and command-execution safety
  3. Embedded context and Skill sources
  4. Automation and runtime lifecycle

The exact commits and fingerprints are recorded in [`docs/fork-modifications.md`](fork-modifications.md).

## Rules

- Prefer the app bridge, bundle instructions/Skills, MCP/connectors/plugins, then an upstream contribution. Keep a fork patch only when the behavior must be atomic inside CodeWhale's Engine, SubAgent, Task, or Automation lifecycle.
- Product tool policy, UI, workspace selection, and business routing stay in `pinvou3-app`.
- The soft drift limits are 1,500 net added lines and 200 fork-distinct lines per file (net = added minus removed, same accounting as the register). The published r13 baseline is 110 files and `+10895/-1195` (net 9,700 added lines), with `+1941/-188` across 17 files over r11 (provider-native search adapters with exact fail-closed endpoint gating, and the keyless Bing chain tail after API-backed providers, both in T2) and `+1088/-1` across 6 files in r13 (GAIA benchmark isolation). The prior r11 baseline was 96 files and `+7840/-980` (net 6,860 added lines). The phase-2 candidate (#37) adds `+934/-32` across 5 files over r13, totaling 111 files and `+11829/-1227` (net 10,602). Retention reasons and the reduction order are recorded in the register's soft-limit assessment; future reduction prioritizes generic per-turn policy, host insertion/edit APIs, session snapshot/recovery APIs, provider compatibility, host MCP policy, and Automation lifecycle fixes for upstreaming.
- Fixups are squashed into their owning topic; no long-lived catch-up commit chains are maintained, and generic host configuration, routing, tools, Automation, and OAuth must remain within their owning boundary.
- A fork-distinct change must update the modification register and guard fingerprints, include a result-oriented `forkguard_*` test where applicable, and pass `./scripts/fork-guard.sh --fast`.
- For a large upstream refactor, clean re-fork from the release tag and re-express each surviving topic. Do not preserve merge-conflict batches as long-lived history.
- Push the maintenance branch and create an immutable tag only after explicit authorization. The published tag, maintenance branch, and parent gitlink must resolve to the same commit.

## Required verification

```bash
./scripts/fork-guard.sh --fast
cargo check --manifest-path CodeWhale/Cargo.toml -p codewhale-tui --lib --locked
cargo test --manifest-path CodeWhale/Cargo.toml -p codewhale-tui --lib --locked \
  forkguard_ -- --test-threads=1
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml --locked
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib --locked \
  -- --test-threads=1
python3 scripts/architecture-guard.py
```

Automated gates do not replace real-model, GUI, MCP/OAuth, and scheduled-task acceptance.
