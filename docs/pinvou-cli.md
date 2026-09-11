# Pinvou CLI (`pinvou`)

The `pinvou` binary is the headless counterpart of the Pinvou Agent desktop app
(`pinvou3-app`). It exposes the product's business capabilities as scriptable
subcommands so the same data under `~/.pinvou3/` (relocatable via
`PINVOU3_HOME`) can be driven from a terminal, CI, or another program. It does
not replace the GUI: anything that is inherently visual (windows, QR codes,
voice capture, the embedded browser, artifact previews) stays in the desktop
app and is deliberately absent here.

Build:

```bash
cargo build --manifest-path pinvou-cli/Cargo.toml --bin pinvou
```

## Conventions

- `--output human|json` (global flag): `human` prints short labeled lines;
  `json` prints a single-line JSON object mirroring the same fields.
- Exit codes: `0` success, `1` host/runtime failure (stable snake_case code in
  the stderr message), `2` usage error.
- All commands operate on `$PINVOU3_HOME` (or `~/.pinvou3`), the same root the
  desktop app uses. Do not run CLI mutations against a data directory while the
  desktop app is mid-write on the same files.
- Secrets are never accepted as plaintext argv (shell history/process lists):
  credential flags are `--api-key-env VAR` or `--api-key-stdin`. The only
  command that prints a stored secret is `models show <id> --reveal-key`,
  mirroring the GUI's explicit reveal action.
- Destructive actions (`delete`, `purge`, `deps install`, checkpoint `rewind`)
  require an explicit `--yes`.
- Engine/model-backed operations (`memory organize`, `scheduled run`,
  `monitor status|snapshot`, `voice postprocess`, `knowledge remote *`) boot the
  windowless product host, which needs a display (or `xvfb-run`) and a
  configured active model — the same prerequisites as `pinvou agent run`.

## Command families

| Family | Commands | Notes |
|---|---|---|
| `pinvou agent run` | `--prompt-file [--workspace] [--timeout-secs] [--session ID] [--mode plan\|agent] [--model ID] [--attach PATH]...` | One product-equivalent agentic turn: unlimited tool-call rounds and a persisted session (GUI parity; `PINVOU3_AGENT_TASK_KEEP_SESSION=0` restores one-shot cleanup). See [agent-task-cli.md](agent-task-cli.md); the session/mode/model/attach flags extend it without changing its defaults or exit contract. |
| `pinvou benchmark` | `list`, `run smoke`, `run/fetch/verify/score/submission gaia`, `status`, `resume`, `report` | Evaluation harness; see [gaia-benchmark.md](gaia-benchmark.md). |
| `pinvou sessions` | `list [--archived]`, `show`, `rename`, `pin`, `unpin`, `archive`, `restore`, `delete --yes`, `export [--format markdown\|json]`, `timeline`, `subagents`, `folder` | Same `SessionStore` the GUI uses, including scheduled-run cascades. |
| `pinvou models` / `pinvou settings` | `models list/add/remove/use/show [--reveal-key]/test/probe-local`; `settings get/set`, `settings search list/set/test` | `settings` is an alias routed to the same module. Settings writes go through the GUI's own prefs transactions (migrations and locale policies included). |
| `pinvou memory` | `overview`, `profile get/set`, `list`, `add preference/work-context`, `update`, `delete --yes`, `archive`, `pending confirm/ignore/never`, `organize`, `organize-history` | `organize` needs the model host. |
| `pinvou knowledge` | `scan`, `stats`, `type-counts`, `collections ...`, `documents ...`, `index ...`, `search`, `model status/cancel`, `mounts/mount/unmount`, `remote connections/probe/collections/search`, `host status` | One-shot imports progress only while the process lives; interrupted imports resume on the next invocation (desktop app completes large imports). `model download` stays desktop-only (in-process ONNX verification + progress events). |
| `pinvou scheduled` | `list`, `show`, `create`, `update`, `pause`, `resume`, `pin`, `unpin`, `delete --yes`, `run`, `runs`, `runs-all`, `mark-viewed`, `chat-prompt` | `run` executes `memory-organize` tasks headless; chat-kind runs need the desktop runtime. |
| `pinvou plugins` | `tools list/install/uninstall/auth/oauth-*`, `skills list/install/update/uninstall`, `import <PATH>`, `export`, `meta`, `recycle ...`, `readiness`, `enable/disable [--scope]`, `project-skills on\|off` | `import` replaces the GUI's native dialog. OAuth login prints the URL instead of a QR window. |
| `pinvou connectors` | `status`, `ensure-cli`, `enable`, `disable`, `logout`, `apply-skills`, `connect [--timeout]`, `ima status/connect/logout` | For feishu/wecom/dingtalk/tmeet. `connect` prints the login URL instead of rendering a QR image. |
| `pinvou personas` | `list`, `show`, `create`, `update`, `delete --yes`, `equip`, `unequip`, `active` | Expert card deck CRUD + per-session equip. |
| `pinvou code` | `agents list/status`, `login/logout`, `providers ...`, `sessions ...`, `workspace list/search/preview/changes/diff/branches/checkout`, `checkpoints ...` | Code-mode (ACP) configuration and read-mostly workspace ops; checkpoints reuse the real shadow-git implementation. Interactive ACP turns and the pending-permission flow are desktop-process-bound. |
| `pinvou files` | `ingest <PATH> [--output PATH]` | File → markdown extraction (pdf/office/email/archive/text), the GUI attachment pipeline. |
| `pinvou voice` | `transcribe <audio>`, `postprocess --mode ...`, `asr-status`, `asr-install` | Transcription of an audio file; recording itself is GUI-bound. |
| `pinvou deps` | `check`, `install <NAME...> --yes` | External dependency detection/installation (apt/Homebrew/bundled). |
| `pinvou feedback` | `submit --type issue\|suggestion --title T --body-file F [--attach PATH...]` | Writes the feedback bundle locally and prints the GitHub issues URL (GUI opens the browser). |
| `pinvou monitor` | `status`, `snapshot` | One-shot model/GPU/vLLM sample instead of the live dashboard. |
| `pinvou artifacts` | `list [--session ID]`, `read`, `write` | Cross-session deliverables index + markdown-safe artifact read/write. |

## Deliberately absent (GUI-bound)

Detach/tear-off windows, desktop pet, the embedded browser and its shared
control, artifact visual previews/design inspector, chat cards and streaming
rendering, voice recording UI and the global shortcut, drag-drop/clipboard
capture, theme/language switching, desktop notifications, QR image rendering
(URLs are printed instead), the WebUI remote-control pairing, the updater
(stubbed in the community build), the local vLLM bootstrap wizard, and the
super-permission pkexec toggle.

## Implementation map

- `pinvou-cli/crates/cli/src/<family>.rs` — one module per family
  (`parse`/`execute`, human+json rendering, contract tests in
  `crates/cli/tests/<family>_contract.rs`).
- `pinvou-cli/crates/cli/src/support.rs` — shared helpers (sandbox home,
  secret resolution, output rendering, `--yes` gating).
- Product capabilities are reused from the app crate (`pinvou3-tauri`, depended
  on as `pinvoy3_lib` with `features = ["benchmark-hooks", "local-embed"]`):
  `features::sessions`, `features::knowledge`, `features::scheduled`,
  `features::marketplace`, `features::connectors`, `features::personas`,
  `features::codex_acp`, `features::code_checkpoints`, `features::memory`,
  `features::monitor`, `features::dependencies`, `features::feedback`,
  `features::files`, `platform::prefs`, `platform::credential_store`.
- Engine/model-backed paths boot through `features::assistant::product_runtime`
  (`run_windowless_host` / `run_agentic_task_headless`), the same windowless
  host the benchmark family uses.
