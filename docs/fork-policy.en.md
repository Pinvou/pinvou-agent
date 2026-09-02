# Pinvou CodeWhale Fork Policy

> Updated: 2026-08-28. Public maintenance baseline: upstream `v0.9.5` r12; r11 PRs #18, #21, #22, #25, #26, #27, #29, and #30 plus r12 PRs #33 and #35 are published within the existing four long-lived topics, and the parent gitlink lands via parent PR #375.
> Canonical Chinese policy: [`docs/fork-policy.md`](fork-policy.md). This English page is a condensed summary; the Chinese version is the complete, authoritative process.

## Baseline

- Upstream: `Hmbown/CodeWhale` `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c`.
- Public maintenance branch: `Pinvou/CodeWhale:pinvou3-clean` at `9c5f4f19` (`pinvou-v0.9.5-r12`).
- The pre-upgrade head `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` remains available as tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`.
- `Pinvou/CodeWhale#18`, `#21`, `#22`, `#25`, `#26`, `#27`, `#29`, and `#30` are published in r11, and `#33` plus `#35` are published in r12. `pinvou3-clean` and immutable tag `pinvou-v0.9.5-r12` are publicly reachable at `9c5f4f19`; tags `r1` through `r12` remain immutable.
- Keep exactly four long-lived topics:

  1. Host embedding and routing boundary
  2. Tool compatibility and command-execution safety
  3. Embedded context and Skill sources
  4. Automation and runtime lifecycle

The exact commits and fingerprints are recorded in [`docs/fork-modifications.md`](fork-modifications.md).

## Rules

- Prefer the app bridge, bundle instructions/Skills, MCP/connectors/plugins, then an upstream contribution. Keep a fork patch only when the behavior must be atomic inside CodeWhale's Engine, SubAgent, Task, or Automation lifecycle.
- Product tool policy, UI, workspace selection, and business routing stay in `pinvou3-app`.
- The soft drift limits are 1,500 net added lines and 200 fork-distinct lines per file (net = added minus removed, same accounting as the register). The published r12 baseline is 110 files and `+9781/-1168` (net 8,613 added lines), with `+1941/-188` across 17 files over r11 (provider-native search adapters with exact fail-closed endpoint gating, and the keyless Bing chain tail after API-backed providers, both in T2). The prior r11 baseline was 96 files and `+7840/-980` (net 6,860 added lines). Retention reasons and the reduction order are recorded in the register's soft-limit assessment; future reduction prioritizes generic per-turn policy, host insertion/edit APIs, session snapshot/recovery APIs, provider compatibility, host MCP policy, and Automation lifecycle fixes for upstreaming.
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
