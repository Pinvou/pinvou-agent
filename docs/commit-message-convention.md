# Commit Message Convention

## Purpose and scope

This convention keeps the Git history readable, consistent, traceable, and suitable for automated release tooling. It applies to every new human-authored commit on every branch.

The project uses a focused form of [Conventional Commits](https://www.conventionalcommits.org/). Existing commit history does not need to be rewritten when this convention changes.

## Format

Use this subject format:

```text
<type>: <English description>
<type>(<scope>)!: <English description>
```

For changes that need more context, add an English body and optional trailers:

```text
<type>: <English description>
<type>(<scope>)!: <English description>

Explain why the change is needed, how it works, and any relevant impact.

Optional-Trailer: value
Signed-off-by: Name <email@example.com>
```

The fields have these meanings:

- `type` is required and must be one of the allowed values below.
- `scope` is optional and identifies the affected module, page, or capability, such as `session`, `search`, or `installer`.
- `!` is optional and marks a breaking change.
- `description` is required. Use concise English, no more than 50 characters, without ending punctuation. Automated checks validate the format, not the language.
- The GitHub-generated terminal suffix ` (#<PR number>)` on a squash commit is platform metadata and does not count toward the 50-character description limit.
- The optional body and trailers must be written in English, apart from standard identifiers, paths, URLs, code, and proper names.

Every human-authored commit must also include the DCO `Signed-off-by` trailer described in [`DCO.md`](../DCO.md). Create it with `git commit -s` or add `--signoff` when amending or rebasing.

## Allowed types

| Type | Use |
|---|---|
| `feat` | Add user-visible functionality, an API, interaction, or configuration option |
| `fix` | Correct a bug, failure, data issue, or compatibility problem |
| `refactor` | Restructure code without adding a feature or fixing a bug |
| `perf` | Improve performance, resource use, caching, or rendering |
| `docs` | Change documentation or comments only |
| `style` | Change formatting without changing behavior |
| `test` | Add, modify, or remove tests |
| `build` | Change build configuration, packaging, dependencies, or environment setup |
| `ci` | Change continuous integration or deployment automation |
| `chore` | Make focused maintenance changes not covered by another type |
| `revert` | Revert an earlier commit |

## Breaking changes

Add `!` after the type or scope when a change breaks compatibility, removes a public interface, or requires migration:

```text
feat(api)!: remove the legacy login parameters
fix(storage)!: change the persisted session schema
```

Explain the migration and compatibility impact in the commit body and pull request.

## Examples

Features and fixes:

```text
feat(search): add offline result filtering
feat(settings): allow custom model identifiers
fix(session): prevent duplicate sync attempts
fix(installer): preserve runtime resources on upgrade
```

Maintenance changes:

```text
refactor(chat): isolate message retry state
perf(list): avoid redundant result rendering
docs: explain the release verification flow
test(connector): cover missing credential errors
chore(deps): update the browser test runtime
```

Reverts:

```text
revert: restore the previous session timeout
```

## Invalid examples

Do not use vague descriptions:

```text
chore: update
fix: fix bug
chore: modify code
test: test
```

Do not use an unsupported type, omit the required separator, end the description with punctuation, exceed the length limit, or write the description in a language other than English:

```text
feature: add search filters
fix(session) prevent duplicate sync attempts
fix: prevent duplicate sync attempts.
```

## Best practices

1. Keep each commit focused on one coherent change.
2. Choose the type that describes the result, not the files that happened to change.
3. State the concrete outcome in the subject; avoid process notes such as `WIP` or `address review`.
4. Use the body to explain motivation, tradeoffs, compatibility, and non-obvious implementation decisions.
5. Reference relevant issues or pull requests in the body or trailers when useful.
6. Never include credentials, tokens, customer data, private data, or internal-only addresses in a commit message.
