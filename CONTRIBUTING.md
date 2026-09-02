# Contributing to Pinvou Agent

[English](CONTRIBUTING.md) | [简体中文](CONTRIBUTING.zh-CN.md)

Thank you for helping improve Pinvou Agent. Bug fixes, documentation, connectors, Skills, workflows, platform support, and focused product improvements are welcome.

This document covers the contribution workflow. Project-wide implementation and quality boundaries are defined in [AGENTS.md](AGENTS.md).

## Before you start

1. Search existing issues and pull requests to avoid duplicate work.
2. Discuss large features, architecture changes, and breaking changes in an issue first.
3. Follow [README.md](README.md) for development setup.
4. Start from the latest official `main`.

Maintainers may branch from `origin/main`. External contributors should configure the official repository once and branch from `upstream/main`:

```bash
git remote add upstream https://github.com/Pinvou/pinvou-agent.git
git fetch upstream
git switch -c feat/short-description upstream/main
git submodule update --init --recursive
```

Sync the latest official `main` before opening a pull request. During review, do not
repeatedly rebase only because `main` advances. When a pull request is ready, the
merge queue validates its combined tree against the latest `main`; manually rebase
again only to resolve a real conflict or when the queue reports an integration
failure that requires a branch change.

When resolving conflicts, preserve compatible functionality and user changes from
both sides. Do not choose between behaviorally different alternatives without
explaining the options and their impact to the user.

## DCO

Every human-authored commit must include a valid `Signed-off-by`:

```bash
git commit -s
```

Use `--signoff` when amending or rebasing existing commits. See [DCO.md](DCO.md). CI rejects unsigned human commits; trusted Dependabot and GitHub Actions bot commits and merge commits (more than one parent) are exempt.

## Where changes belong

Pinvou Agent uses [CodeWhale](https://github.com/Pinvou/CodeWhale) as its agent engine. Do not reimplement engine capabilities in the desktop layer. The extension-boundary table is defined once in [AGENTS.md](AGENTS.md) §2; CodeWhale changes must follow it and [`docs/fork-policy.md`](docs/fork-policy.md), including the required same-PR documentation, fingerprints, and tests.

## Commit messages

Use:

```text
<type>: <English description>
<type>(<scope>)!: <English description>
```

`scope` and `!` are optional. Allowed types are `feat`, `fix`, `refactor`, `perf`, `docs`, `style`, `test`, `build`, `ci`, `chore`, and `revert`. Write concise English descriptions of at most 50 characters without ending punctuation. CI validates the format, not the language.

Use English for branches, issues, pull requests, commits, code comments, developer documentation, and diagnostics. Existing history and localized resources are exempt; UI copy follows [AGENTS.md](AGENTS.md).

See [`docs/commit-message-convention.md`](docs/commit-message-convention.md) for the full commit rules.

## Local checks

Run checks that match the affected area. A common baseline is:

```bash
./scripts/fork-guard.sh --fast
python3 scripts/architecture-guard.py
npm --prefix pinvou3-app run lint:ui
npm --prefix pinvou3-app run build:ui
npm --prefix pinvou3-app test
cargo fmt --manifest-path pinvou3-app/src-tauri/Cargo.toml -- --check
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib -- --test-threads=1
```

Optionally, enable the local commit-msg hook to catch commit-message format issues before pushing:

```bash
git config core.hooksPath .githooks
```

Run additional frontend, Relay, Rust, CodeWhale, or platform checks when relevant. The workflows in [`.github/workflows/`](.github/workflows/) are the source of truth for automated gates. Disclose checks that could not be run locally.

Tests requiring a live model, network service, credential, or large model asset must be ignored by default and provide an explicit opt-in command.

## CI and merge queue

Pull requests use a staged, path-aware gate. Draft pull requests run fast feedback
(lint, build, deterministic logic tests, and Rust formatting where applicable).
Ready pull requests add browser smokes selected from the actual diff, platform
runtime contracts, and other affected checks. Ready Rust pull requests run fast
formatting, lint, dependency-policy, and compile feedback by default. All unknown
Rust paths fail closed to full regression; only documented isolated leaf-feature
boundaries use the lightweight route. Dependency, CodeWhale gitlink, app-command,
platform, permission, credential, session, remote-control, and other high-blast-
radius paths automatically add full Linux and Windows Rust regression before
queueing. Maintainers can apply `ci:full-rust` to force the same full regression
for another ready Rust pull request; the label has no effect on a draft.
The current lightweight Rust boundaries are internal-only changes under `feedback`,
`personas`, and `pet`. Changing their registration, app commands, shared platform
surface, Cargo metadata, or any unclassified Rust path still runs full regression.
Release-chain changes run lightweight contract tests only; full deb, dmg, and nsis
packages are built only after a
`VERSION` change reaches `main`, or through an explicit `workflow_dispatch`.

The merge queue runs the applicable product gates against the actual combined tree
of the queued pull request and the latest `main`. Rust changes run formatting,
Clippy, compile, and dependency-policy checks there. High-blast-radius Rust changes
also run the full Linux behavior regression on the combined tree; Windows coverage
already ran on the ready PR and is not repeated. Frontend changes run browser smokes
selected from the merge group's actual base/head diff; shared, unknown, or test-
infrastructure paths fall back to the complete smoke set. Every retained `main` push
runs cumulative Linux compile verification (full test execution stays on the
merge-queue leg, which already validates the same combined tree) and native Windows
checks, and continues to warm the shared caches. A red main regression is a stop
signal for further queueing until it is fixed or reverted. During review, maintainers should inspect only required checks:

```bash
gh pr checks <number> --required
```

Do not wait for non-required post-merge platform or release builds when `main` is
green. Queue independent ready pull requests without routine rebases; resolve actual
conflicts, and let the queue validate freshness. Maintainers may queue at most two
low-risk, independent pull requests in one merge group. Dependency-lock, CI, release,
permission, session, CodeWhale gitlink, and other high-risk changes enter alone.

## Pull requests

Review the actual diff against the target branch and complete the quality self-check required by [AGENTS.md](AGENTS.md) before submission.

Pull request titles and descriptions must use English; titles must follow the commit subject convention for squash merges.

A pull request should explain:

- what changed and why;
- affected features, platforms, compatibility, and known risks;
- tests actually run;
- unverified scenarios or environment limitations.

Keep changes focused. Update documentation and regression tests with behavior changes. Resolve conflicts against the latest official `main` before merge. The project uses CI-gated pull requests and squash merge by default.

By participating, you agree to follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Support expectations are documented in [SUPPORT.md](SUPPORT.md).

## Security

Never commit credentials, tokens, passwords, customer or private data, or internal-only addresses. Report unpatched vulnerabilities privately through [SECURITY.md](SECURITY.md) or `security@pinvou.com`.
