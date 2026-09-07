# Pinvou CodeWhale Fork Policy

> Updated: 2026-09-07. Local candidate baseline: upstream `v0.9.12` r1. No remote maintenance branch or immutable r1 tag has been published yet.
> Canonical Chinese policy: [`docs/fork-policy.md`](fork-policy.md). This English page is a condensed summary; the Chinese version is the complete, authoritative process.

## Baseline

- Upstream: `Hmbown/CodeWhale` `v0.9.12` at `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5`.
- Local maintenance candidate: `codex/pinvou-v0.9.12-r1` at `09c3b85e366379dbbb69539e4cec8c2dbadd91f4`, five signed commits above upstream.
- The pre-upgrade r13 head `f853f8f1566c57e6be40d5439a222a932aa79ef5` remains at `backup/pre-v0.9.12-sync`; the existing `pinvou-v0.9.5-r13` tag remains immutable.
- r1 is not publicly reachable yet. Only after explicit authorization should `Pinvou/CodeWhale:pinvou3-clean` and immutable tag `pinvou-v0.9.12-r1` be published at the candidate head.
- Keep exactly four long-lived topics:

  1. Host embedding and routing boundary
  2. Tool compatibility and command-execution safety
  3. Embedded context and Skill sources
  4. Automation and runtime lifecycle

The exact commits and fingerprints are recorded in [`docs/fork-modifications.md`](fork-modifications.md).

## Rules

- Prefer the app bridge, bundle instructions/Skills, MCP/connectors/plugins, then an upstream contribution. Keep a fork patch only when the behavior must be atomic inside CodeWhale's Engine, SubAgent, Task, or Automation lifecycle.
- Product tool policy, UI, workspace selection, and business routing stay in `pinvou3-app`.
- The soft drift limits remain 1,500 net added lines and 200 fork-distinct lines per file. The v0.9.12 r1 candidate is 49 files and `+2993/-457` (net 2,536), down sharply from v0.9.5 r13 at 110 files and `+10895/-1195`. The remaining excess is justified by Engine/Task-atomic steer, final-dispatch security, host prompt/profile/Skills ownership, and Automation lifecycle behavior. Upstreaming priority is generic steer and per-turn security first, Automation lifecycle second, then replacement of prompt/profile/Skills ownership with stable host APIs.
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
