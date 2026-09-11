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
  `json` prints a single-line JSON object mirroring the same fields. The flag
  is recognized anywhere; a subcommand that takes its own `--output PATH`
  (e.g. `sessions export`) accepts any other value as that flag's argument,
  so a file literally named `json` or `human` must be spelled `./json`.
- `pinvou --version` prints the CLI version.
- Exit codes: `0` success, `1` host/runtime failure, `2` usage error. Errors
  print a human-readable stderr message; many messages carry a stable
  snake_case code prefix (e.g. `product_backend_not_enabled`,
  `scheduled_task_not_found`), but the prefix is a convention rather than a
  structurally enforced contract — scripts that need to branch should match on
  exit codes, not message text.
- All commands operate on `$PINVOU3_HOME` (or `~/.pinvou3`), the same root the
  desktop app uses. Do not run CLI mutations against a data directory while the
  desktop app is mid-write on the same files.
- Secrets are never accepted as plaintext argv (shell history/process lists):
  credential flags are `--api-key-env VAR` or `--api-key-stdin` (`code login
  claude` also accepts the code via `--code-env VAR` / `--code-stdin`, and
  keeps a literal `--code C` for callers that already hold it in argv). Two
  commands expose stored secrets, both explicitly: `models show <id>
  --reveal-key` (mirrors the GUI reveal action) prints the key, and `code
  providers export` writes/prints the provider JSON with plaintext API keys
  (warns on stderr; a file destination is written 0600).
- Destructive actions require an explicit `--yes` (family `delete`/`remove`
  commands, `purge`, `deps install`, `connectors logout`, checkpoint
  `rewind`; the usage error names the flag). Concurrent CLI mutations
  are serialized through two cross-process locks: a per-session lock (checkpoint
  `rewind`/`undo`/`diff`, `workspace checkout`) and a per-execution-root lock
  (`rewind`/`undo`, `checkout`) so two different sessions bound to the same
  project directory cannot interleave working-tree restores. A GUI turn or GUI
  rewind takes neither lock (the app's guards are process-local) — do not
  rewind while its GUI Code session may be mid-turn. Similarly, `agent run
  --session` does not lock the session against concurrent GUI use. Other shared
  stores (plugins `installed.json`, scheduled sidecars, memory JSONL files)
  have no cross-process lock in either surface: last writer wins, so avoid CLI
  mutations while the desktop app is running.
- Engine/model-backed operations (`memory organize`, `scheduled run`,
  `monitor status|snapshot`, `voice postprocess`, `knowledge remote *`) boot the
  windowless product host, which needs a display (or `xvfb-run`) and a
  configured active model — the same prerequisites as `pinvou agent run`.

## Command families

| Family | Commands | Notes |
|---|---|---|
| `pinvou agent run` | `--prompt-file [--workspace] [--timeout-secs] [--session ID] [--mode plan\|agent] [--model ID] [--attach PATH]...` | One product-equivalent agentic turn: unlimited tool-call rounds and a persisted session (GUI parity; `PINVOU3_AGENT_TASK_KEEP_SESSION=0` restores one-shot cleanup). See [agent-task-cli.md](agent-task-cli.md); the session/mode/model/attach flags extend it without changing its defaults or exit contract. |
| `pinvou benchmark` | `list`, `run smoke`, `run/fetch/verify/score/submission gaia`, `status`, `resume`, `report` | Evaluation harness; see [gaia-benchmark.md](gaia-benchmark.md). |
| `pinvou sessions` | `list [--archived]`, `show`, `rename`, `pin`, `unpin`, `archive`, `restore`, `delete --yes`, `export [--format markdown\|json] [--output PATH]`, `timeline`, `subagents`, `folder` | Same `SessionStore` the GUI uses, including scheduled-run cascades. Any store-opening command (even reads like `list`) runs the shared 50-sessions-per-kind retention, so CLI runs can evict the oldest GUI chat sessions. ACP/code sessions are listed too — the CLI has no live pool to filter them like the GUI does. |
| `pinvou models` / `pinvou settings` | `models list/add/remove/use/show [--reveal-key]/test/probe-local`; `settings get/set`, `settings search list/set/test` | `settings` is an alias routed to the same module. Settings writes go through the GUI's own prefs transactions (migrations and locale policies included). |
| `pinvou memory` | `overview`, `profile get/set`, `list`, `add preference/work-context`, `update`, `delete --yes`, `archive`, `pending confirm/ignore/never`, `organize`, `organize-history` | `organize` needs the model host. |
| `pinvou knowledge` | `scan`, `stats`, `type-counts`, `collections ...`, `documents ...`, `index ...`, `search`, `model status/download/cancel`, `mounts/mount/unmount`, `remote connections/probe/collections/search`, `host status` | One-shot imports progress only while the process lives; an import interrupted at exit is marked resumable and continues only after an explicit `index resume <job-id>` (desktop app completes large imports). `model download` declines headless (the in-process ONNX verification and progress events are GUI-bound); `model cancel` and `scan cancel` execute but can only signal cancels inside the CLI's own process (an app-side scan needs the desktop app). `mounts/mount/unmount` refuse with `knowledge_*_requires_product_host`: mounted collections live in the desktop app's process memory and are not persisted, so a one-shot process can neither observe nor change them. `--before` filters on UTC midnight boundaries. |
| `pinvou scheduled` | `list`, `show`, `create`, `update`, `pause`, `resume`, `pin`, `unpin`, `delete --yes`, `run`, `runs`, `runs-all`, `mark-viewed`, `chat-prompt` | `run` executes `memory-organize` tasks headless; chat-kind runs need the desktop runtime. `update --kind/--mode` are rejected (creation-time properties; every run is forced to `yolo` like the GUI). `run` reconciles stranded CLI queued records (no foundation task id) to a terminal failed record on the next run, and `delete` only refuses GUI-owned active runs — a CLI process killed mid-run cannot wedge a task. Created tasks default to the GUI's `allow_shell`/`auto_approve` settings. |
| `pinvou plugins` | `tools list/install/uninstall/auth/oauth-*`, `skills list/install/update/uninstall`, `import <PATH>`, `export`, `meta`, `recycle ...`, `readiness`, `enable/disable [--scope]`, `project-skills on\|off` | `import` replaces the GUI's native dialog. OAuth login declines headless (`oauth_login_unavailable_in_cli`): the interactive grant happens in the desktop app. `readiness` reads credential presence from the OS keyring, which can prompt for access on macOS. |
| `pinvou connectors` | `status`, `ensure-cli`, `enable`, `disable`, `logout --yes`, `apply-skills`, `connect [--timeout]`, `ima status/connect/logout --yes` | For feishu/wecom/dingtalk/tmeet. Vendor CLIs resolve through the managed assets install (what `ensure-cli` and the GUI install) before PATH. `connect` prints the login URL to stderr as soon as the vendor CLI emits it (and again in the final/error summary — a timeout keeps the captured link); wecom prints the one-scan `qr.png` path the same way (the stdout URL alone is a landing page). `--timeout` bounds every blocking vendor-CLI phase. |
| `pinvou personas` | `list`, `show`, `create`, `update`, `delete --yes`, `equip`, `unequip`, `active` | Expert card deck CRUD. `equip` records the staged persona for the session sidecar; prompt injection happens in the GUI, so the CLI itself does not deliver it. |
| `pinvou code` | `agents list/status`, `login/logout`, `providers ...`, `sessions ...`, `workspace list/search/preview/changes/diff/branches/checkout`, `checkpoints ...` | Code-mode (ACP) configuration and read-mostly workspace ops; checkpoints reuse the real shadow-git implementation; agent CLIs resolve like the GUI (override env var → official install dir → PATH; Windows `.exe`/`.cmd` aware). Interactive ACP turns and the pending-permission flow are desktop-process-bound. |
| `pinvou files` | `ingest <PATH> [--output PATH]` | File → markdown extraction (pdf/office/email/archive/text), the GUI attachment pipeline. |
| `pinvou voice` | `transcribe <audio>`, `postprocess --mode ...`, `asr-status`, `asr-install` | Transcription of an audio file up to 4 MiB (GUI `recording_too_long` bound); recording itself is GUI-bound. `asr-install` verifies the model download by sha256. On macOS, `asr-status` reports the host Speech runtime (`ready: true`), but `transcribe` uses the external ASR CLI lane — the JSON adds `cli_transcribe_ready` for what the CLI itself can do. |
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
  (`parse`/`execute`, human+json rendering). Contract tests live in
  `crates/cli/tests/<family>_contract.rs` where present
  (code/connectors/knowledge/memory/models/personas/plugins/scheduled/sessions);
  the remaining families are covered by `misc_contract.rs`, the dispatch
  contract, and inline unit tests.
- `pinvou-cli/crates/cli/src/support.rs` — shared helpers (sandbox home,
  secret resolution, output rendering, `--yes` gating).
- Product capabilities are reused from the app crate (`pinvou3-tauri`, depended
  on as `pinvou3_lib` with `features = ["benchmark-hooks", "local-embed"]`):
  `features::sessions`, `features::knowledge`, `features::marketplace`,
  `features::personas`, `features::codex_acp`, `features::code_checkpoints`,
  `features::memory`, `features::monitor`, `features::dependencies`,
  `features::feedback`, `features::files`, `platform::prefs`,
  `platform::credential_store` (plus `platform::connector_lock` for vendor-CLI
  resolution and integrity verification). `features::scheduled`,
  `features::connectors`, and `features::voice` are `pub(crate)` to the app
  crate and therefore mirrored at the store/protocol level with their
  deviations disclosed in the module headers.
- Engine/model-backed paths boot through `features::assistant::product_runtime`
  (`run_windowless_host` / `run_agentic_task_headless`), the same windowless
  host the benchmark family uses.
