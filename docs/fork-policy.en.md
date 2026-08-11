# Pinvou CodeWhale Fork Policy

> Updated: 2026-08-23. Public maintenance baseline: upstream `v0.9.5` r8 plus one upstream-#5461 backport commit; PR #13 reduced the fork to four long-lived topics.
> Canonical Chinese policy: [`docs/fork-policy.md`](fork-policy.md).

## Baseline

- Upstream: `Hmbown/CodeWhale` `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c`.
- Public maintenance branch: `Pinvou/CodeWhale:pinvou3-clean` with published baseline head `d127aed11` (`pinvou-v0.9.5-r8`); the parent gitlink currently points at `2645c6c63`, i.e. r8 plus one upstream-#5461 backport commit (CodeWhale PR #21, see `docs/fork-modifications.md`).
- The pre-upgrade head `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` remains available as tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`.
- `Pinvou/CodeWhale#15` was merged as `d127aed113529dc93754d044b9f352e9746f6b83` and published as `pinvou-v0.9.5-r8`; tags `r1` through `r8` remain immutable. The #5461 backport ships as `pinvou-v0.9.5-r9` once CodeWhale PR #21 merges; `verify-public-submodule.sh` then validates against r9.
- Keep exactly four long-lived topics:

  1. Host embedding and routing boundary
  2. Tool compatibility and command-execution safety
  3. Embedded context and Skill sources
  4. Automation and runtime lifecycle

The exact commits and fingerprints are recorded in [`docs/fork-modifications.md`](fork-modifications.md).

## Rules

- Prefer the app bridge, bundle instructions/Skills, MCP/connectors/plugins, then an upstream contribution. Keep a fork patch only when the behavior must be atomic inside CodeWhale's Engine, SubAgent, Task, or Automation lifecycle.
- Product tool policy, UI, workspace selection, and business routing stay in `pinvou3-app`.
- The soft drift limits are 1,500 total changed lines and 200 fork-distinct lines per file. The r8 baseline plus the #5461 backport is 49 files and `+3303/-476` (net +2827; `#15` itself is maintainer-published baseline at +1449); exceeding a limit requires an explicit retention and reduction assessment.
- Fixups are squashed into their owning topic; generic host configuration, routing, tools, Automation, and OAuth must remain within their owning boundary.
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
