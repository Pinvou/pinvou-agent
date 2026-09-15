# Pinvou CLI (`pinvou`)

The `pinvou` binary is the headless counterpart of the Pinvou Agent desktop app
(`pinvou3-app`). It exposes the product's business capabilities as scriptable
subcommands so the same data under `~/.pinvou3/` (relocatable via
`PINVOU3_HOME`, which must be an absolute path) can be driven from a terminal,
CI, or another program. It does not replace the GUI: anything that is
inherently visual (windows, QR-code *rendering* — the wecom connector still
writes a `qr.png` the CLI points at, voice capture, the embedded browser,
artifact previews) stays in the desktop app and is deliberately absent here.

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
- `pinvou --version` (or `pinvou version`) prints the CLI version.
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
  (warns on stderr; on unix a file destination is written 0600).
- Destructive actions require an explicit `--yes` (family `delete`/`remove`
  commands, `purge`, `memory organize`, `deps install`, `connectors logout`,
  `code logout`, checkpoint `rewind`/`undo`, `voice asr-install`, plugins
  tools/skills `uninstall`; the usage error names the flag). Concurrent CLI mutations
  are serialized through two cross-process locks: a per-session lock (checkpoint
  `rewind`/`undo`/`diff`, `workspace checkout`) and a per-execution-root lock
  (`rewind`/`undo`, `diff`, `checkout`) so two different sessions bound to the same
  project directory cannot interleave working-tree restores. Scheduled CLI
  mutations (`create`/`update`/`delete`/`run`/…) are serialized among themselves
  by a third lock (`locks/scheduled-store.lock`) — `scheduled run` holds it for
  the whole headless host run, so other CLI scheduled commands wait until it
  finishes. A GUI turn or GUI
  rewind takes none of these locks (the app's guards are process-local) — do not
  rewind while its GUI Code session may be mid-turn. Similarly, `agent run
  --session` does not lock the session against concurrent GUI use. Remaining
  shared stores (plugins `installed.json`, the `acp-providers.json` provider
  registry, memory JSONL files) have no cross-process lock in either surface:
  last writer wins, so avoid CLI
  mutations while the desktop app is running. `sessions rename` is the one to
  treat with real care: it rewrites the whole transcript JSON from a snapshot
  read moments earlier, so renaming a session the GUI is ACTIVELY streaming
  can drop the engine's newest messages — metadata mutations belong to idle
  sessions.
- Engine/model-backed operations (`memory organize`, `scheduled run`,
  `monitor status|snapshot`, `voice postprocess`, `knowledge remote *`) boot the
  windowless product host, which needs a display (or `xvfb-run`) and, for the
  model-backed ones, a configured active model — the same prerequisites as
  `pinvou agent run`. `monitor status|snapshot` and `knowledge remote
  connections` answer offline without a model; `monitor` reports a clean zero
  state instead of an error.

## Command families

| Family | Commands | Notes |
|---|---|---|
| `pinvou agent run` | `--prompt-file [--workspace] [--timeout-secs] [--session ID] [--mode plan\|agent] [--model ID] [--attach PATH]...` | One product-equivalent agentic turn: unlimited tool-call rounds and a persisted session (GUI parity; `PINVOU3_AGENT_TASK_KEEP_SESSION=0` restores one-shot cleanup). See [agent-task-cli.md](agent-task-cli.md); the session/mode/model/attach flags extend it without changing its defaults or exit contract. |
| `pinvou benchmark` | `list`, `run smoke`, `run/fetch/verify/score/submission gaia`, `status`, `resume`, `report` | Evaluation harness; see [gaia-benchmark.md](gaia-benchmark.md). |
| `pinvou sessions` | `list [--archived] [--limit N]`, `show [--last N] [--full]`, `rename`, `pin`, `unpin`, `archive`, `restore`, `delete --yes`, `export [--format markdown\|json] [--output PATH]`, `timeline`, `subagents`, `folder` | Same `SessionStore` the GUI uses, including scheduled-run cascades. Any store-opening command (even reads like `list`) runs the shared 50-sessions-per-kind retention, so CLI runs can evict the oldest GUI chat sessions. Avoid `rename`/`pin`/`archive`/`restore`/`delete` on a session the GUI is actively streaming: the CLI write is last-writer-wins on the whole transcript file. ACP/code sessions are listed too — the CLI has no live pool to filter them like the GUI does. Residual `eval_`-prefixed sessions are invisible to `list` (this build filters them like the benchmark lanes) while retention still counts them, so they can hold retention slots without appearing here. |
| `pinvou models` / `pinvou settings` | `models list/add/remove/use/show [--reveal-key]/test/probe-local`; `settings get/set`, `settings search list/set/test` | `settings` is an alias routed to the same module. Settings writes go through the GUI's own prefs transactions (migrations and locale policies included); the prefs lock is in-process, so a CLI write racing a GUI write is last-writer-wins on the whole settings file and can drop the other side's change — edit settings from one side at a time. `settings get` without a key always prints the full settings JSON regardless of `--output human`. `probe-local` is deliberately stricter than the GUI's local-server probe: loopback addresses only (the GUI also accepts LAN/private hosts), a credential-read failure fails the probe, and redirects are not followed. |
| `pinvou memory` | `overview`, `profile get/set`, `list [--store]`, `add preference/work-context`, `update`, `delete --yes`, `archive`, `pending confirm/ignore/never`, `organize --yes`, `organize-history` | `organize` rewrites the stores under LLM decisions, so it is gated behind `--yes` like the other destructive actions; it needs the model host and, like the GUI, refreshes `snapshot.md` afterwards (best-effort). Adding to a preference/work-context topic replaces the previous item in that bucket; profile-shaped preference text is rejected up front with `memory_add_not_materialized` and touches nothing. `list --store` JSON is always `{items, cleanup_warnings}` for every store. |
| `pinvou knowledge` | `scan`, `stats`, `type-counts`, `collections ...`, `documents ...`, `index ...`, `search`, `model status/download/cancel`, `mounts/mount/unmount`, `remote connections/probe/collections/search`, `host status` | One-shot imports progress only while the process lives. There is no exit hook that marks an interrupted import: the dead job keeps its `running` state on disk until the next write/maintenance command (delete, add-sources, resume/retry/cancel) opens the store WITH boot recovery, which flips preparing/running jobs to interrupted/resumable — only then can `index resume <job-id>` continue it, and until then `index status` still reports the persisted `running` state. A CLI write/maintenance command marks the job it finds — including one a live desktop-app process is executing — as interrupted (staged progress survives for `index resume`); read-only commands never disturb a running import. `model download` declines headless (the in-process ONNX verification and progress events are GUI-bound); `model cancel` and `scan cancel` execute but can only signal cancels inside the CLI's own process (an app-side scan needs the desktop app). `mounts/mount/unmount` refuse with `knowledge_*_requires_product_host`: mounted collections live in the desktop app's process memory and are not persisted, so a one-shot process can neither observe nor change them. `--before YYYY-MM-DD` is exclusive: it matches files with mtime before the start of that UTC day, so the named day itself is never included (`--after` includes its named day from its start). |
| `pinvou scheduled` | `list`, `show`, `create`, `update`, `pause`, `resume`, `pin`, `unpin`, `delete --yes`, `run`, `runs`, `runs-all`, `mark-viewed`, `chat-prompt` | There is no daemon here: a task fires only when the desktop app's scheduler sweep runs, so a task created while the app is closed starts firing at the next app start. The running app keeps its tasks in memory: CLI `create`/`update`/`delete` are invisible to it (and can be overwritten by its next in-memory write) until the app reloads the store — and the reverse: a CLI `delete` cannot stop a run the app's in-memory scheduler already started. `run` executes `memory-organize` tasks headless; chat-kind runs need the desktop runtime. `update --kind/--mode` are rejected (creation-time properties; every run is forced to `yolo` like the GUI), and `update` cannot change the task's model — `--model-id` only re-binds the pin for the definition's existing wire model, so edit the model in the GUI or recreate the task. `run` reconciles stranded CLI queued records (no foundation task id) to a terminal failed record on the next run, and `delete` only refuses GUI-owned active runs — a CLI process killed mid-run cannot wedge a task. Created tasks default `auto_approve` to true (the GUI's new-task default) and read `allow_shell` from the environment/settings like the GUI. |
| `pinvou plugins` | `tools list/install/uninstall/auth/oauth-*`, `skills list/install/update/uninstall`, `import <PATH>`, `export [--output PATH]`, `meta`, `recycle ...`, `readiness`, `enable/disable [--scope]`, `project-skills on\|off` | `import` replaces the GUI's native dialog. OAuth login declines headless (`oauth_login_unavailable_in_cli`): the interactive grant happens in the desktop app. `readiness` reads credential presence from the OS keyring, which can prompt for access on macOS. |
| `pinvou connectors` | `status`, `ensure-cli`, `enable`, `disable`, `logout --yes`, `apply-skills`, `connect [--timeout]`, `ima status/connect/logout --yes` | For feishu/wecom/dingtalk/tmeet. `logout --yes` runs the real vendor logout whenever the vendor CLI resolves — a version below the install minimum included, mirroring the GUI. Vendor CLIs resolve through the managed assets install (what `ensure-cli` and the GUI install) before PATH. `connect` prints the login URL to stderr as soon as the vendor CLI emits it (and again in the final/error summary — a timeout keeps the captured link); wecom prints the one-scan `qr.png` path the same way (the stdout URL alone is a landing page). `--timeout` bounds every blocking vendor-CLI phase. |
| `pinvou personas` | `list`, `show`, `create`, `update`, `delete --yes`, `equip`, `unequip`, `active` | Expert card deck CRUD. `equip` records the staged persona for the session sidecar; prompt injection happens in the GUI, so the CLI itself does not deliver it. `delete --yes` also sweeps every `persona_equipped.json` sidecar referencing the deleted card and reports the cleared sessions in `cleared_sessions`. Like the GUI's empty-input path, CLI-created cards fix `dept` to "specialized" and default emoji/color; a stdin body over the 4 MiB cap is a content error (exit 1). |
| `pinvou projects` | `list`, `create --name N [--root PATH]...`, `update <id> [--name N] [--root PATH]...`, `delete <id> --yes`, `move <session-id> [<project-id>]` | Session project grouping on the same `ProjectStore` as the GUI (list JSON mirrors the GUI DTO incl. per-root availability and the full `assignments` map). Deleting unassigns sessions, never deletes them. `move` without a project id moves the session out of its project (the GUI picker's ungrouped entry); it skips the GUI's add-workspace-root lane (needs the ACP pool) and rejects scheduled-run sessions like the GUI. The store boots once per process and rewrites the whole file on every write: changes made while the GUI runs stay invisible to it until it reloads, and its next projects write overwrites them — do projects edits while the app is closed. |
| `pinvou code` | `agents list/status/install`, `login/logout`, `providers ...`, `sessions ...`, `workspace list/search/preview/changes/diff/branches/checkout`, `checkpoints ...`, `run`/`permissions`/`respond` (decline with `code_*_requires_product_host`; `agents install` declines the same way — the vendor install script runs under GUI supervision) | Code-mode (ACP) configuration and read-mostly workspace ops; read-only workspace ops call the GUI's own `codex_acp::workspace` module; checkpoints reuse the real shadow-git implementation; agent CLIs resolve like the GUI (override env var → official install dir → PATH; Windows `.exe`/`.cmd` aware). `providers --wire-api` also accepts the app parser's `openai_compatible`/`chat` aliases; a failed `login` keeps the last captured login link in the error. Interactive ACP turns and the pending-permission flow are desktop-process-bound. |
| `pinvou files` | `ingest <PATH> [--output PATH]` | File → markdown extraction (pdf/office/email/archive/text), the GUI attachment pipeline. |
| `pinvou voice` | `transcribe <audio>`, `postprocess --mode ...`, `asr-status`, `asr-install` | Transcription of an audio file up to 4 MiB (GUI `recording_too_long` bound); recording itself is GUI-bound. `asr-install [--yes]` is Linux-only and system-touching (it can install ffmpeg via the OS package manager), so it requires `--yes` like `deps install`; the model download is verified by sha256. `transcribe` reports `ffmpeg_missing` when only ffmpeg is absent. On macOS, `asr-status` reports the host Speech runtime (`ready: true`), but `transcribe` uses the external ASR CLI lane — the JSON adds `cli_transcribe_ready` for what the CLI itself can do. On Windows, the CLI probes the real engine/ffmpeg state instead of trusting the GUI's MSI-bundled-runtime branch, so its `asr-status` can disagree with the GUI's and reports `installable: false` plus `gui_install_only: true` (repair/reinstall the desktop app to install). |
| `pinvou deps` | `check`, `install <NAME...> --yes` | External dependency detection/installation (apt/Homebrew/bundled). |
| `pinvou feedback` | `submit --type issue\|suggestion --title T --body-file F [--attach PATH...]` | Writes the feedback bundle locally and prints the GitHub issues URL (GUI opens the browser). |
| `pinvou monitor` | `status`, `snapshot` | One-shot model/GPU/vLLM sample instead of the live dashboard. |
| `pinvou artifacts` | `list [--session ID]`, `read`, `write` | Cross-session deliverables index + markdown-safe artifact read/write. |

## Deliberately absent (GUI-bound)

Detach/tear-off windows, desktop pet, the embedded browser and its shared
control, artifact visual previews/design inspector, chat cards and streaming
rendering, voice recording UI and the global shortcut, drag-drop/clipboard
capture, the live theme/language switching and notification surfaces (`settings set` persists those prefs), QR image rendering
(URLs are printed instead), the WebUI remote-control pairing, the updater
(stubbed in the community build), the local vLLM bootstrap wizard, and the
super-permission pkexec toggle.

## Implementation map

- `pinvou-cli/crates/cli/src/<family>.rs` — one module per family
  (`parse`/`execute`, human+json rendering). Contract tests live in
  `crates/cli/tests/<family>_contract.rs` where present
  (code/connectors/knowledge/memory/models/personas/plugins/projects/scheduled/sessions);
  the remaining families are covered by `misc_contract.rs`, `cli_contract.rs`
  (benchmark), the dispatch contract, and inline unit tests.
- `pinvou-cli/crates/cli/src/support.rs` — shared helpers (sandbox home,
  secret resolution, output rendering, `--yes` gating).
- Product capabilities are reused from the app crate (`pinvou3-tauri`, depended
  on as `pinvou3_lib` with `features = ["benchmark-hooks", "local-embed"]`):
  `features::sessions`, `features::knowledge`, `features::marketplace`,
  `features::personas`, `features::projects`, `features::codex_acp`, `features::code_checkpoints`,
  `features::memory`, `features::monitor`, `features::dependencies`,
  `features::feedback`, `features::files`, `platform::prefs`,
  `platform::credential_store` (plus `platform::connector_lock` for vendor-CLI
  resolution and integrity verification). `features::scheduled`
  and `features::connectors` are `pub(crate)` to the app crate and therefore
  mirrored at the store/protocol level with their deviations disclosed in the
  module headers; `features::voice` is `pub` (the CLI shares the GUI's
  transcript parser directly), with its remaining mirrors disclosed in the
  module header.
- Engine/model-backed paths boot through `features::assistant::product_runtime`
  (`run_windowless_host` / `run_agentic_task_headless`), the same windowless
  host the benchmark family uses.
