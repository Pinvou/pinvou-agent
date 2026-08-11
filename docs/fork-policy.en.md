# Pinvou CodeWhale Fork Policy

> Updated: 2026-08-11. Public maintenance baseline: upstream `v0.9.5` plus five Pinvou themes.
> Canonical Chinese policy: [`docs/fork-policy.md`](fork-policy.md).

## Baseline

- Upstream: `Hmbown/CodeWhale` `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c`.
- Public maintenance branch: `Pinvou/CodeWhale:pinvou3-clean` at `d1010aa3bbaf76780e29df4434fd1e03a95b2ca6`.
- The pre-upgrade head `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` remains available as tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`.
- The branch and immutable tag `pinvou-v0.9.5-r4` are publicly reachable and resolve to the same commit as the parent gitlink. `r1`, `r2`, and `r3` remain immutable historical tags.
- Keep exactly five long-lived topics:

  1. Host embedding and routing boundary
  2. Tool compatibility and command-execution safety
  3. Embedded context and Skill sources
  4. Automation and runtime lifecycle
  5. Three Departments and Six Ministries orchestration and completion gate

The exact commits and fingerprints are recorded in [`docs/fork-modifications.md`](fork-modifications.md).

## Rules

- Prefer the app bridge, bundle instructions/Skills, MCP/connectors/plugins, then an upstream contribution. Keep a fork patch only when the behavior must be atomic inside CodeWhale's Engine, SubAgent, Task, or Automation lifecycle.
- Product tool policy, UI, workspace selection, and business routing stay in `pinvou3-app`.
- The soft drift limits are 1,500 total changed lines and 200 fork-distinct lines per file. Exceeding a limit requires an explicit retention and reduction assessment.
- Fixups are squashed into their owning topic. Topic 5 contains only Three Departments and Six Ministries behavior; generic host configuration, routing, tools, Automation, and OAuth must stay outside it.
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

Automated gates do not replace real-model, GUI, MCP/OAuth, scheduled-task, and workflow acceptance.
