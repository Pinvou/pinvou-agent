# Pinvou Agent Project Conventions

## Core constraints

### 0. Local private memory

- If `.codex-memory.md` exists at the repository root, read it before starting work. It is local private memory and must not be committed.

### 1. Development and pull request conventions

- Before the first development work for a new task and before creating its pull request, safely sync the latest `origin/main` and align every submodule with the parent repository gitlink. Do not repeatedly sync only because `main` advances during continued development or review of the same task. Before merge, the Merge Queue runs applicable gates against the combined tree on the latest `main`; manually rebase only when a real conflict or queue integration failure requires changing the branch.
- When resolving conflicts, preserve compatible functionality and user changes from both sides. Do not choose between behaviorally different alternatives without explaining the options and their impact to the user.
- Use `<type>(<scope>): <English description>` for commit messages (`scope` is optional), and include a valid `Signed-off-by` trailer. Follow `CONTRIBUTING.md` and the commit message convention for the exact format and DCO requirements.
- Pull request titles and descriptions must use English and reflect the actual target-branch diff, including the background, changes, verification, and known risks.
- Repository collaboration uses English, including branches, issues, pull requests, commits, code comments, developer documentation, and diagnostics. Existing history and localized resources are exempt; UI copy follows the i18n rule below.
- Before the final commit and pull request creation, self-review requirement completeness, root cause and related cases, implementation boundaries, impact on existing functionality, exceptional states, and verification sufficiency. Passing tests does not replace this review.
- Fix and verify any in-scope gap found during self-review. Disclose and await a decision for out-of-scope changes, mutually exclusive approaches, unverified scenarios, or regression risks.

### 2. CodeWhale and fork boundaries

CodeWhale provides the foundation for model calls, streaming output, tool loops, Sessions, Skills, Commands, MCP, Hooks, and Compaction. Pinvou Agent does not reimplement these capabilities.

Place extensions according to these boundaries:

| Change type | Location |
|---|---|
| Domain Agent or tool composition | `SKILL.md` |
| External API or standalone capability | MCP server / connector |
| Model behavior guidance | Bundle `instructions.md` |
| UI, Tauri integration, or Engine configuration | `pinvou3-app/` |
| Reusable foundation issue | CodeWhale, upstream first |

- Keep Pinvou-specific semantics in the fork only when they must participate in the foundation lifecycle and cannot be implemented in the app, a Skill, or MCP. Prefer contributing reusable fixes upstream.
- When adding or modifying fork-distinct behavior, update `docs/fork-modifications.md`, the relevant fingerprints, and behavior tests in the same pull request, then run `./scripts/fork-guard.sh --fast`.
- When only the CodeWhale gitlink changes without a behavior change, update the register and fingerprints as required by the guard. New behavior tests are not mandatory when existing tests can be shown to cover the behavior.
- Treat `docs/fork-policy.md` and `docs/fork-modifications.md` as the single sources of truth for the fork baseline, size, topics, and synchronization process.

### 3. Cross-platform architecture boundaries

Pinvou Agent is organized by business capability first and platform adaptation second:

| Change type | Location |
|---|---|
| Frontend business logic | `pinvou3-app/src/features/<name>/` |
| Tauri / Web host adaptation | `pinvou3-app/src/platform/{tauri,web}/` |
| Rust business logic and platform differences | `pinvou3-app/src-tauri/src/features/<name>/`; feature-specific adapters under its `platform/` |
| Cross-feature OS primitives | `pinvou3-app/src-tauri/src/platform/`; interfaces and OS implementations under `platform/os/` |
| Shared resources and platform configuration | `pinvou3-app/src-tauri/resources/common/`, `pinvou3-app/src-tauri/resources/platforms/`, `pinvou3-app/src-tauri/config/platforms/` |

- Keep business logic in `features/`. Only low-level capabilities reused across features belong in the global `platform/`. Dependencies must flow `app → features → platform/core`, never in reverse.
- React code must not inspect the user agent or access Tauri globals directly. Consume semantic capabilities through `get_platform_capabilities` and `can(capability)`.
- Express OS differences with `cfg(target_os)` and explicit interfaces. Return `unsupported` explicitly for unavailable capabilities; do not silently reuse another platform's implementation.
- If conditional compilation or platform implementation details must remain outside an adapter layer, use an explicit exception supported by the architecture guard, document the concrete reason at the top of the file, and add a test covering the scenario. Exceptions must not be used merely to avoid moving a module.
- Use project npm commands for builds; do not run `npx tauri build/bundle` directly. After changes, run `python3 scripts/architecture-guard.py` and tests for the affected area.

#### Modular development conventions

- Define module boundaries by responsibility, state ownership, reason for change, dependency direction, and independent testability, not by file length or total repository size.
- Place new or modified functionality in a clearly scoped `feature`, `platform`, or `core` module. Do not continue adding unrelated responsibilities to an existing module. Coordinate across modules through narrow, stable interfaces.
- When responsibilities are mixed, state is coupled, dependencies point backward, or independent testing is difficult, split the code incrementally along behavior boundaries while preserving existing behavior and public API compatibility in every batch.
- Do not mechanically split files, compress code, or sacrifice naming, comments, error handling, or readability to satisfy a metric. File size is a review signal, not a compliance gate.

### 4. Community edition development conventions

- Before starting, search existing issues, pull requests, and foundation capabilities to avoid duplicate work. Confirm the approach and acceptance criteria before large features or breaking changes.
- Community features must work completely without private services, internal addresses, or enterprise-only data. Integrate enterprise capabilities through generic extension interfaces.
- New features must form a complete, usable workflow and handle key errors, insufficient permissions, and unsupported platforms. Placeholder code or fake data is not a completed implementation.
- Preserve compatibility with existing configuration, data, and public interfaces. Confirm unavoidable breaking changes first and provide a migration path and regression tests.
- Application UI copy must reuse `pinvou3-app/src/shared/i18n.js` and provide Simplified Chinese, English, and Japanese. Do not introduce single-language copy in components or depend on fallback from another language.
- Use secure defaults and clearly inform users when changes involve network access, uploads, external commands, or new dependencies. Update tests and documentation with behavior changes.
- Never put accounts, passwords, keys, tokens, cookies, customer or private data, or internal addresses in code, commits, pull requests, examples, or logs. Report suspected disclosures privately according to `SECURITY.md`.

## Project facts

- `pinvou3-app/`: Tauri 2 + React/Vite desktop application and Engine wrapper.
- `CodeWhale/`: Pinvou/CodeWhale submodule; changes follow the CodeWhale and fork boundaries above.
- Runtime data is stored under `~/.pinvou3/` (`sessions`, `settings.json`, `bundle`, `knowledge`, and `connectors`).
- Bundle extension sources live in `pinvou3-app/src-tauri/resources/common/bundle/`; they are compiled into the application and extracted to `~/.pinvou3/bundle/`.
- Start development with `./pinvou3-app/run-dev.sh`.
- The root `VERSION` file is the single source of truth for the version. After changing it, run `node scripts/sync-version.mjs`; CI verifies consistency with `--check`.

## Policy entry points

- `CONTRIBUTING.md`: single source of truth for contribution, DCO, checks, and pull request workflow. `CONTRIBUTING.zh-CN.md` is a Chinese reference; when content conflicts, `CONTRIBUTING.md` takes precedence.
- `SECURITY.md`: private reporting process for vulnerabilities and sensitive information.
- `docs/fork-policy.md` / `docs/fork-modifications.md`: CodeWhale fork policy and current inventory.
- `pinvou3-app/src/ARCHITECTURE.md`, `pinvou3-app/src-tauri/src/README.md`, and `pinvou3-app/src-tauri/config/README.md`: frontend, Rust, and platform configuration boundaries.
- `docs/architecture-guard.md`: architecture guard rules.
- `docs/commit-message-convention.md`: commit message convention.
