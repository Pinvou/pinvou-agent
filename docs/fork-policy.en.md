# Pinvou CodeWhale Fork Policy

> Updated: 2026-08-27. Public maintenance baseline: upstream `v0.9.5` r11; PRs #18, #21, #22, #25, #26, #27, #29, and #30 are published within the existing four long-lived topics.
> Canonical Chinese policy: [`docs/fork-policy.md`](fork-policy.md). This English page is a condensed summary; the Chinese version is the complete, authoritative process.

## Baseline

- Upstream: `Hmbown/CodeWhale` `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c`.
- Public maintenance branch: `Pinvou/CodeWhale:pinvou3-clean` at `0d89a31be` (`pinvou-v0.9.5-r11`).
- The pre-upgrade head `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` remains available as tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`.
- `Pinvou/CodeWhale#18`, `#21`, `#22`, `#25`, `#26`, `#27`, `#29`, and `#30` are published in r11. `pinvou3-clean` and immutable tag `pinvou-v0.9.5-r11` are publicly reachable at `0d89a31be016457c180501417dd2c0f34ce844a6`; tags `r1` through `r11` remain immutable.
- Keep exactly four long-lived topics:

  1. Host embedding and routing boundary
  2. Tool compatibility and command-execution safety
  3. Embedded context and Skill sources
  4. Automation and runtime lifecycle

The exact commits and fingerprints are recorded in [`docs/fork-modifications.md`](fork-modifications.md).

## Rules

- Prefer the app bridge, bundle instructions/Skills, MCP/connectors/plugins, then an upstream contribution. Keep a fork patch only when the behavior must be atomic inside CodeWhale's Engine, SubAgent, Task, or Automation lifecycle.
- Product tool policy, UI, workspace selection, and business routing stay in `pinvou3-app`.
- The soft drift limits are 1,500 net added lines and 200 fork-distinct lines per file (net = added minus removed, same accounting as the register). The published r11 baseline is 96 files and `+7840/-980` (net 6,860 added lines), with `+2242/-292` across 48 files over r10. r11 adds strict-direct model casing compatibility, explicit route output ceilings, per-tool Moonshot degradation with a visible diagnostic, host MCP secret resolution and deny-surface sealing, observable steer withdrawal, Windows Shell decoding, and dependency security fixes. Future reduction prioritizes generic per-turn policy, host insertion/edit APIs, session snapshot/recovery APIs, provider compatibility, host MCP policy, and Automation lifecycle fixes for upstreaming.
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
