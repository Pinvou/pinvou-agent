# Contributing to Pinvou Agent

[English](CONTRIBUTING.md) | [简体中文](CONTRIBUTING.zh-CN.md)

Thank you for helping improve Pinvou Agent. Bug fixes, documentation, connectors, Skills, workflows, platform support, and focused product improvements are welcome.

## Before you start

1. Search existing issues and pull requests to avoid duplicate work.
2. For a large feature or architecture change, open an issue first and describe the user problem and proposed boundary.
3. Start from the latest `main` and initialize the submodule:

   ```bash
   git fetch origin
   git switch main
   git pull --ff-only
   git submodule sync --recursive
   git submodule update --init --recursive
   ```

## Developer Certificate of Origin

Pinvou Agent uses the [Developer Certificate of Origin 1.1](https://developercertificate.org/). Every commit must include a `Signed-off-by` trailer that matches the commit author:

```bash
git commit -s
```

For an existing commit:

```bash
git commit --amend --signoff
```

By signing off, you certify that you have the right to submit the contribution under this repository's license. See [DCO.md](DCO.md). CI rejects pull requests containing unsigned commits.

## Where changes belong

Pinvou Agent uses [CodeWhale](https://github.com/Pinvou/CodeWhale) as its agent engine. Do not reimplement engine capabilities in the desktop layer.

| Goal | Location |
|---|---|
| Add a domain agent or tool bundle | A `SKILL.md` package |
| Connect an external API | An independent MCP server or connector |
| Change model guidance | Bundle instructions |
| Change desktop UI, Tauri integration, or runtime configuration | `pinvou3-app/` |
| Fix a reusable engine issue | CodeWhale, following `docs/fork-policy.md` |

Changes to the CodeWhale gitlink must update `docs/fork-modifications.md` and the relevant fingerprints in `scripts/fork-guard.sh` in the same pull request.

## Commit messages

Use the repository format:

```text
<type>[optional scope]: <short Chinese description>
```

Allowed types are `feat`, `fix`, `refactor`, `perf`, `docs`, `style`, `test`, `build`, `ci`, `chore`, and `revert`.

Example:

```text
fix(settings): 修复模型配置保存失败
```

The Chinese description rule is retained for the current maintainers; issue and pull-request discussions may be written in English or Chinese.

## Local checks

Run the checks relevant to your change. The common baseline is:

```bash
./scripts/fork-guard.sh --fast

(cd pinvou3-app && npm run lint:ui)
(cd pinvou3-app && npm run build:ui)
(cd pinvou3-app && npm test)

(cd pinvou3-app/src-tauri && cargo test --lib -- --test-threads=1)
```

Tests that require a live model, network service, or large model asset must be ignored by default and provide an explicit opt-in command.

## Pull requests

A pull request should explain:

- what changed;
- why the change is needed;
- affected features, platforms, compatibility, and known risks;
- which tests were run.

Keep changes focused. Update documentation and regression tests together with behavior changes. Resolve merge conflicts against the latest `main` before merge.

The project uses CI-gated pull requests and squash merge by default.

## Security

Do not open a public issue for a vulnerability. Follow [SECURITY.md](SECURITY.md) or email `security@pinvou.com`.
