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
  `json` prints a single-line JSON object. It carries the same facts, but a few
  commands deliberately differ in shape: `benchmark report` prints the whole
  markdown report in `human` and `{run_id, report_path}` in `json`, `benchmark
  list` adds machine-only fields, and `settings get` without a key is always the
  JSON prefs document. The flag
  is claimed only at the two ends of the line, where a global flag can
  legally sit: a leading run at the very front of argv (before the family
  token — `pinvou --output json sessions list`) and the trailing pair at the
  very end of the line (where every existing `... --output json` invocation
  puts it). Everything between them stays ordinary family input: a
  subcommand that takes its own `--output PATH` (the four export lanes that
  share the collision: `sessions export`, `plugins export`, `plugins
  recycle export`, and `code providers export`) accepts any other value as
  that flag's argument — so `code providers export --output json`, which
  names a destination file, would instead print the provider JSON WITH
  PLAINTEXT KEYS to stdout — and a parser with no
  `--output` refuses the pair as an unknown flag instead of silently
  collapsing the tokens around it. A file literally named `json` or `human`
  at the end of the line must still be spelled `./json`.
- `pinvou --version` (or `pinvou version`) prints the CLI version. `pinvou
  --help` / `-h` prints the top-level usage on **stdout** and exits `0` (under
  `--output json` it is wrapped as `{"usage": ...}`); it accepts no further
  arguments, so `pinvou --help benchmark` is a usage error. There is no
  per-family `--help`: an invalid invocation prints the usage line for that
  family on stderr and exits 2, which is the same text in the diagnostic role.
- Exit codes: `0` success, `1` host/runtime failure, `2` usage error. Exit
  2 covers malformed invocations — unknown flags, missing values,
  mutually-exclusive flag combinations — AND a set of content findings the
  shared helpers classify the same way, while the rest of content-dependent
  failures are exit 1. It is NOT a uniform "argv-decidable only" rule; the
  current code draws the line per family, and scripts that need to branch
  should check per command rather than assume one rule:
  - exit 1 (host/content class) in the shared helpers (`support.rs`):
    unreadable/missing/over-cap/non-UTF-8 files (`read_text_file_capped`),
    unset or empty secret env vars, empty or over-cap `--api-key-stdin`
    reads, and other resource-content failures surfaced by family helpers
    (spawn failures, failed writes, store opens).
  - exit 2 (usage class) in the shared helpers: missing
    `--yes` (`require_yes`), invalid session-id charset, and a few remaining
    content findings that ARE classified as usage in the current code: empty
    prompt/body content after a successful read in `personas`, `memory`,
    `scheduled`, and `voice postprocess --mode edit` with an empty draft
    file (dictation/task silently drop an empty draft), where every other
    input-content error in those same files is exit 1, plus
    state-dependent model-store
    findings in the `models` family: removing the last remaining model
    (`models.rs` `remove`), `probe-local` against an active model whose
    STORED base URL is non-loopback, and `add`/`edit` invoked with empty
    `--name`/`--model`/`--base-url` or blank metadata values. Treat "exit 2
  consistently" as a per-family fact to re-verify, not a contract; the
  snake_case code prefixes below are the durable identifiers. A closed
  pipe (`pinvou ... | head`) suppresses the write panic but keeps the run's
  own verdict, so the above survives piping. The one deliberate exception
  is `agent run`, which exits `0` whenever it produced a report — a
  `timeout` or `error` status lives in the report fields (see
  `docs/agent-task-cli.md`). Errors print a human-readable stderr message;
  many messages carry a stable snake_case code prefix (e.g.
  `product_backend_not_enabled`, `scheduled_task_not_found`), but the
  prefix is a convention rather than a structurally enforced contract —
  match on the documented per-command exit code, not message text.
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
  (warns on stderr; a file destination is created 0600 on unix through an
  exclusive create). Like every other export in the CLI, an existing
  destination is refused (exit 1), never truncated or overwritten; the
  refusal is not gated behind `--yes` — there is nothing to consent to,
  because the file is left byte-for-byte untouched. `--code-stdin` reads the
  authorization code only after the CLI has announced the login link on
  stderr (two-phase, matching the GUI's `submit_agent_login_code`), and it
  keeps the vendor child's stdin open for the whole login.
- Destructive actions require an explicit `--yes` (family `delete`/`remove`
  commands, `purge`, `memory organize`, `deps install`, `connectors logout`,
  `code logout`, `code workspace checkout`, checkpoint `rewind`/`undo`,
  `voice asr-install`, plugins
  tools/skills `uninstall`, credential deletions in the `models` family
  (`models edit --clear-api-key`, `settings search set --clear`) and the
  irreversible `projects move <session>` ungroup; the usage error names the
  flag). `code providers update --delete-key` (deleting a stored provider
  key) requires `--yes` like the other destructive actions in that set. Concurrent CLI mutations
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
| `pinvou agent run` | `--prompt-file [--workspace] [--timeout-secs] [--session ID] [--mode plan\|agent] [--model ID] [--attach PATH]...` | One product-equivalent agentic turn: unlimited tool-call rounds and a persisted session (GUI parity; `PINVOU3_AGENT_TASK_KEEP_SESSION=0` restores one-shot cleanup). See [agent-task-cli.md](agent-task-cli.md); the session/mode/model/attach flags extend it without changing its defaults or exit contract, but `--prompt-file` itself was narrowed — it must now be a REGULAR file (symlinks are followed; FIFOs, character devices, `/dev/stdin` and `<(...)` process substitution are refused, because their read blocks before any cap or deadline can act) of at most 4 MiB, both exit 1. |
| `pinvou benchmark` | `list`, `run smoke`, `run/fetch/verify/score/submission gaia`, `status`, `resume`, `report` | Evaluation harness; see [gaia-benchmark.md](gaia-benchmark.md). |
| `pinvou sessions` | `list [--archived] [--limit N]`, `show [--last N] [--full]`, `rename`, `pin`, `unpin`, `archive`, `restore`, `delete --yes`, `export [--format markdown\|json] [--output PATH]`, `timeline`, `subagents`, `folder` | Same `SessionStore` the GUI uses, including scheduled-run cascades. Any store-opening command (even reads like `list`) runs the shared retention sweep, which counts headless `agent run` sessions (`agentic_` ids) against their own 50-session budget, separate from the 50-session chat budget the GUI's chat sessions occupy — so a CLI store open can evict the oldest non-pinned GUI chat sessions (pins are exempt, on the durable pin file, including pins made in the GUI while the CLI process is live), and likewise evicts the oldest non-pinned headless runs when that bucket is over cap. Avoid `rename`/`pin`/`archive`/`restore`/`delete` while the desktop app is running, streaming or not: the GUI caches its session list and boots its title/pinned/hidden maps once per process, so a CLI write here is last-writer-wins not just in the moment but until the app next restarts — a GUI list refresh hours later can still resurface the old title, an un-pinned pin state, or a session the CLI deleted (and a GUI-side rename or pin made in between can be silently dropped by its own cached write-back, reverting hours after the fact). `delete` performs the GUI's full cascade: after the transcript directory it also drops the session's record from the `session-agents.json` index (without that second half the app's boot-time sidecar backfill re-created `sessions/<deleted-id>/code-session.json`, resurrecting a ghost directory for a session that no longer exists); the index file is only touched when it already exists, and a delete that committed but could not clear the record fails loudly rather than silently. `rename`'s parser refuses a title containing a mid-title `--output json`/`--output human` pair followed by more title words (exit 2, nothing stored — a pair at the very END of the line is how JSON output is requested, so a pair still visible inside the title cannot be that), and refuses a title that looks like a flag; such titles must be set from the desktop app. ACP/code sessions are listed too — the CLI has no live pool to filter them like the GUI does. Residual `eval_`-prefixed sessions are invisible to `list` (this build filters them like the benchmark lanes) while retention still counts them, so they can hold retention slots without appearing here. |
| `pinvou models` / `pinvou settings` | `models list/add/edit/remove/use/show [--reveal-key]/test/probe-local [--url U [--model ID]] [--api-key-env V]`; `settings get/set`, `settings search list/set [--clear [--yes]]/test` | `settings` is an alias routed to the same module. Settings writes go through the GUI's own prefs transactions (migrations and locale policies included); the prefs lock is in-process, so a CLI write racing a GUI write is last-writer-wins on the whole settings file and can drop the other side's change — edit settings from one side at a time. `settings get` without a key always prints the full settings JSON regardless of `--output human`. Even the read commands of this family (`models list`/`show`/`test`, `settings get`, `search list`/`test`) load settings through the persisting `UserPrefs::load`, so a read can rewrite `settings.json` as a side effect — including the plaintext-key migration that moves an old API key out of the file and into the OS keyring. `models remove` follows the app's ordering contract: the model is removed from settings first, then its keyring secret is deleted best-effort — a failed keyring delete only warns on stderr and the command still succeeds, so a broken keyring can leave an orphaned credential entry. `models edit <id>` mutates a stored model IN PLACE and never touches the id, which is the reason it exists: the previous `remove` + `add` round-trip minted a new id and orphaned every per-session model binding and scheduled-task model pin that named the old one. Its credential lanes mirror the GUI — no flag keeps the existing secret, `--api-key-env`/`--api-key-stdin` replace it inside the save transaction, `--clear-api-key` marks the record missing in the transaction and defers the keyring delete until after the commit (so a failed save can orphan a keyring entry but never leaves a configured model without a secret) — and because it deletes a stored credential it is gated behind `--yes` like the other destructive actions. `add` and `edit` share the same five GUI-form metadata flags (`--alias`, `--provider-kind`, `--vendor`, `--endpoint-mode`, `--vision-model-id`); `--provider-kind` is an allow-list (`official_api`/`custom`/`coding_plan`) and a blank metadata value is a usage error, because the prefs normalizer would discard either. `probe-local` is deliberately stricter than the GUI's local-server probe: loopback addresses only (the GUI also accepts LAN/private hosts), a credential-read failure fails the probe, and redirects are not followed; `--model ID` is its spelling of the GUI's `model_id` — it names which SAVED model's stored credential the probe should present (mutually exclusive with `--api-key-env`, and only meaningful with `--url`). The two probe commands report a malformed STORED `base_url` over two different channels by design: `models test` renders it as a result payload on stdout (`{"ok":false,"code":"invalid_url",...}`, exit 1, `render_probe_outcome`'s shape) because every `models test` outcome is a probe result, while `models probe-local` reports it as a stderr `CliError` (exit 1, "invalid_url: …") because the URL guard classifies it as a host failure before any probe request can run; both agree on the `invalid_url` code and exit 1 even though the channel differs. When every signature probe is answered 401/403 the result is `kind: unknown_authenticated` with exit 1 — deliberately not one of the GUI's kind strings, because each of those asserts something about the server's identity and this case asserts that nothing was learned (the sequential mirror used to report `generic` here, the silently wrong answer). `settings search test <provider>` now issues a REAL request for all five providers (bing, tavily, bocha, metaso, baidu) using the request shapes the product's own search tool sends; the new `verified` field says what `ok` is a statement about — `live_probe` when a request was actually sent and its status classified, `credential_presence` when nothing was sent because no key is configured or the credential could not be read, `nothing` when the probe could not run at all. Previously four of the five returned `{"ok":true,"code":"configured"}` on mere credential presence, so a revoked key passed a command called `test`. `settings search set --provider P --clear` no longer switches the active provider as a side effect: clearing is a credential operation that deletes the stored key (requiring `--yes`), and only a caller actually selecting a provider moves search onto it. `models show --reveal-key` prints `api_key_source` (`credential_store` / `environment` / `none`, rendered ahead of the secret so the verbatim value stays the last line): an `environment`-override model has a working key the CLI deliberately does not echo, which the old single `(not stored)` placeholder reported as if there were no key at all. |
| `pinvou memory` | `overview`, `profile get/set`, `list [--store]`, `add preference/work-context`, `update`, `delete --yes`, `archive`, `pending confirm/ignore/never`, `organize --yes`, `organize-history` | `organize` rewrites the stores under LLM decisions, so it is gated behind `--yes` like the other destructive actions; it needs the model host and, like the GUI, refreshes `snapshot.md` afterwards (best-effort). `organize` also takes a cross-process fd lock (`$PINVOU3_HOME/locks/memory-organize.lock`) before booting the host, so a second concurrent CLI pass is refused with `memory_organize_busy` instead of letting two CLI passes interleave destructive apply steps from up-to-75-second-old snapshots. Cross-surface exclusion now holds too: the feature layer's `organize_memory_with_llm` — the funnel every surface goes through (GUI button, scheduled executor, this CLI host lane) — takes its own cross-process `.organize.lock` (user memory directory) around the whole pass, so a GUI-triggered organize fails with a busy error while a CLI organize runs and vice versa. The residual disclosed gap is the post-pass `snapshot.md` refresh: it runs after `organize_memory_with_llm` returns and therefore outside `.organize.lock`, so two passes' snapshot refreshes can still interleave even though the store mutations stay inside the lock on every surface. `overview` reads like a read-only summary but also rewrites the shared `snapshot.md`, and it does so with `runtime: None` — a one-shot CLI owns no active session — so the rewritten document loses the runtime section the desktop app wrote, until the app next refreshes it. The command says so on stderr and in its own output (`snapshot_rewritten_without_runtime`) rather than leaving it to be discovered. Adding to a preference/work-context topic replaces the previous item in that bucket; profile-shaped preference text is rejected up front with `memory_add_not_materialized` and touches nothing. The same error refuses a dedupe-reused pending row that is not this add's own candidate: the enqueue's dedupe hands back an existing pending row matched on the kind plus a case-insensitive content key and keeps that row's own topic, so a same-text GUI candidate queued in another topic bucket would otherwise be confirmed from here — approving a candidate the user never reviewed and replacing that bucket's item; topic, kind and text are each compared before the confirm, and nothing is confirmed or written on mismatch. `list --store` JSON is always `{items, cleanup_warnings}` for every store. Content is capped at 120 characters per item for both stores: `add` routes through the shared pending queue, which normalizes candidates to 120 before either store's own limit (160 for work context) can apply. A longer input is reported — `add` warns on stderr and sets `truncated`, `submitted_characters` and `stored_characters` in its output — never silently truncated, and `update` applies the same disclosure for its per-store writer caps (preferences 120, work context 160, timed stores 180). Separately from the cap, the confirm-time sentence cleanup (`clean_candidate_sentence`, the same stage the GUI runs) strips 请记住-style leading prefixes and outer punctuation from work-context text before it is stored, so what lands in the store can be a shorter sentence than what was submitted — that normalization is not part of the cap disclosure and fires no `truncated` flag. |
| `pinvou scheduled` | `list`, `show`, `create`, `update`, `pause`, `resume`, `pin`, `unpin`, `delete --yes`, `run`, `runs`, `runs-all`, `mark-viewed`, `chat-prompt` | There is no daemon here: a task fires only when the desktop app's scheduler sweep runs, so a task created while the app is closed starts firing at the next app start. What a running app re-reads and what it holds in memory are different halves. Task DEFINITIONS are re-read from disk on every sweep (`AutomationManager::list_automations` does a fresh `read_dir`), so a CLI `create`/`update`/`delete`/`pause`/`resume` is seen by a live app at its next tick without a restart. The SIDECARS the CLI co-owns — task kind, model binding, pin/UI metadata, run read-state — are no longer read once and rewritten whole from a stale in-memory copy: every app-side mutator of those files now re-reads the file first whenever it changed on disk (a cheap `FileStamp` identity check — length, mtime, and on Unix the file's inode, so a foreign write that preserves the byte length is still noticed), so a running app MERGES a CLI `pin`/`unpin`/`mark-viewed`/kind/model-binding write into its own next write to that sidecar instead of overwriting it. The residual is a same-instant write race: if both surfaces persist to the same sidecar in the same moment, one of the two writes is lost (last writer wins). The executor lookups still re-read on a MISS: a task whose kind this handle has never seen, and a task whose model binding it has never seen, both pay one disk read before answering, so a task the CLI created while the app was up runs as its real kind on its pinned model instead of as an unattended full-permission chat. A HIT is still answered from memory, so a CLI edit that changes an entry the app already cached (flipping a kind back to chat) stays invisible until the app's next write to that sidecar reloads it. A CLI `--model-id` change is no longer held hostage to that cache: it rewrites the DEFINITION's model wire name as well (definitions are re-read every tick), so the executor never sees the old wire name with the new pin. The reverse also holds: a CLI `delete` cannot stop a run the app's scheduler already started. `run` executes `memory-organize` tasks headless; chat-kind runs need the desktop runtime. `update --kind/--mode` are rejected (creation-time properties; every run is forced to `yolo` like the GUI), `--model-id X` (on `create` and `update` alike) resolves the model pin and the definition's model wire name as ONE pair — X's own wire name, exactly the GUI's `model: selected.model, modelId: selected.id` — and an unknown id is refused (`model not found: X`, exit 1) before anything is persisted; an active create (and `update`/`resume` of an active task) resolves the next run slot eagerly through the foundation, so a past `FREQ=ONCE;AT=` is refused (`no future run`) instead of being silently paused by the first sweep, and `create --paused` still stages a past stamp like the GUI. `run` reconciles stranded CLI queued records (no foundation task id) to a terminal failed record on the next run, and `delete` only refuses GUI-owned active runs — a CLI process killed mid-run cannot wedge a task. Created tasks default `auto_approve` to true (the GUI's new-task default) and read `allow_shell` from the environment/settings like the GUI. Every scheduled command that boots the session store (including reads) runs the same retention sweep as the `sessions` family — counting headless `agent run` sessions (`agentic_` ids) against their own separate 50-session budget — so it can evict the oldest non-pinned GUI chat sessions (and the oldest non-pinned headless runs when that bucket is over cap); scheduled-run sessions themselves are exempt from both budgets. |
| `pinvou plugins` | `tools list [--installed-only]/install/uninstall/auth/oauth-*`, `skills list [--installed-only]/install/update/uninstall`, `import <PATH>`, `export [--output PATH]`, `meta`, `recycle ...`, `readiness`, `enable/disable [--scope]`, `project-skills on\|off` | `import` replaces the GUI's native dialog. OAuth login declines headless (`oauth_login_unavailable_in_cli`): the interactive grant happens in the desktop app. `readiness` reads credential presence from the OS keyring (for `ima` on every run regardless of install state; for other bundles when installed), which can prompt for access on macOS, and it NEVER reports a `cli`-kind bundle (the connectors) as ready: such a row always carries `ready:false` with `probe:"unavailable_in_cli"` and, absent any registry-visible fault, `reason:"connection_unknown_in_cli"`. A connector's readiness IS its live connection state, which the desktop answers from a `*_status` probe this crate does not link, so `false` there means "not determined here", not "known broken" — `pinvou connectors status` is the authority. Every other row carries `probe:"registry"`, and an installed package whose assets are gone is demoted to `reason:"assets_missing"`. Three write-path deviations from the GUI, each printed at the point of action: `tools install` skips the desktop's post-install `validate_remote_connection` handshake AND the rollback that handshake guards, so a tool the GUI would have uninstalled stays installed here; `tools uninstall` does not delete stored remote OAuth tokens, so a reinstall of that tool is still authorized; and `enable`/`disable` send no hot-refresh broadcast (the GUI's `refresh_live_sessions_skills` + `refresh_permission_rulesets` need the engine pool), so a running desktop app keeps its live engines on the whitelist they started with until they respawn. `--scope both` is two independent single-scope writes with no two-scope transaction behind them, so a failure on the second scope cannot roll the first one back — the error names which scopes already landed, and the success payload reports them as `scopes_applied`. Each scope's write is the GUI's own load-modify-save (`load_disabled_bundles_for` → `save_disabled_bundles_for`), verified on the exact list handed to the writer, and a failed write fails loudly instead of reporting an unpersisted success (concurrency with a running desktop app: see Known limitations). Export and recycle-export refuse to overwrite an existing destination. |
| `pinvou connectors` | `status [<CONNECTOR>]`, `ensure-cli <CONNECTOR>`, `enable/disable <CONNECTOR>`, `logout <CONNECTOR> --yes`, `apply-skills <CONNECTOR>`, `connect <CONNECTOR> [--timeout SECS]`, `ima status` / `connect --client-id-env V [--api-key-env V\|--api-key-stdin]` / `logout [--yes]` | Vendor-CLI lifecycle for feishu/wecom/dingtalk/tmeet plus the ima skill connector (secrets via env/stdin only, like everywhere else). Vendor CLIs resolve in the GUI's order (lock-table install path → managed bin dir → npm global prefixes → PATH), so CLI-installed and GUI-installed vendor CLIs are visible to each other; `status` is the live connection authority the `plugins readiness` rows defer to. `connect` announces the login/QR URL live as the vendor prints it, and wecom's `qr.png` is written to a scratch directory the CLI points at (QR *rendering* stays GUI-bound, see the intro). `ensure-cli` installs the lock-table-pinned archive (tmeet through npm like the GUI does), and both install lanes serialize through the cross-process `locks/connector-install.lock`. Two headless deviations, disclosed in the command output rather than silently: materializing the skill directories unpacks the desktop app's embedded bundle, so the SHOW direction reports `skills_unpack: "app-only"` (the HIDE direction on `logout` removes the skill dirs itself and reports failures), and the execpolicy ruleset hot-refresh after `connect`/`apply-skills` needs the GUI's engine pool, so it is reported as not run. Consent writes (`enable`/`disable`, the DenyAll sync after `apply-skills`/`ima connect`) serialize only in-process — see Known limitations. |
| `pinvou knowledge` | `scan`, `stats`, `type-counts`, `collections ...`, `documents ...`, `index ...`, `search`, `model status/download/cancel`, `mounts/mount/unmount`, `remote connections/probe/collections/search`, `host status` | `collections add-sources`, `index resume` and `index retry` block until their import job reaches a terminal phase (like `scan start`): a one-shot process exits right after printing, so a fire-and-forget import thread was reaped mid-work and its job stranded `running` on disk with no owner. Their exit code and header are phase-honest (`done` exits 0; `interrupted`, `cancelled` and `done_with_errors` exit 1 and name the remedy). The wait has a no-progress liveness bound (300 s by default, overridable with `PINVOU_KB_IMPORT_STALL_MILLIS`); on timeout the invocation INTERRUPTS its stalled job first, so it is left `interrupted` — immediately resumable with `index resume <job-id>`, no desktop-app boot required — and the report says so (if even the interrupt cannot land, the remedy text honestly keeps the app-boot route). No CLI command ever runs the GUI's boot recovery of interrupted jobs — that is the desktop app's own crash handler; a job a killed one-shot process strands keeps its `running` state on disk, and reads keep reporting it until the app's next start relabels it `interrupted`/resumable. While the latest job is still `running`, `add-sources`/`resume`/`retry` refuse (its owner is another process; `index cancel <job-id>` can drop it — immediate and deliberately NOT gated behind `--yes`, unlike `collections delete`/`documents remove`, since the drop is recoverable through `index resume`/`retry`). There is no signal handler on the import lanes, so Ctrl-C kills the CLI the hard way: the job is recoverable exactly like a crash (staged progress survives; the app's next boot flips it to `interrupted`). `model download` declines headless (the in-process ONNX verification and progress events are GUI-bound); `model cancel` and `scan cancel` refuse honestly (`knowledge_{model_cancel,scan_cancel}_requires_product_host`) — the flags they could set are process-local to the desktop app, and nothing inside a one-shot process can ever be cancelled through them. `scan start` waits for the scan to finish inside the invocation (a fire-and-forget scan would be killed by process exit before doing any work), canonicalizes `--root` so a relative path or a symlink keys entries the way the index already does, and refuses a missing or non-directory root before starting. The incremental stale sweep only deletes entries that live under the roots the scan actually walked, so scanning one directory never prunes what was indexed from another. `collections delete` never boots the session store: mounted collections live in the desktop app's process memory and are deliberately not persisted, so there is no CLI-side mount to sweep, and the `SessionStore::boot()` retention side effect is not triggered (a booted sweep here was always empty by construction). `mounts`/`mount`/`unmount` refuse with `knowledge_*_requires_product_host` — mounted collections live in the desktop app's process memory and are not persisted, so a one-shot process can neither observe nor change them — and the refusal is returned BEFORE any store is opened. That ordering is the point: `SessionStore::boot()` is not a read, it enforces retention and irreversibly deletes the oldest non-pinned sessions, and booting it merely to decorate an unavoidable refusal with "session not found" destroyed chat history as a side effect of a command that can never succeed. The session-id charset gate still runs first, at parse time (exit 2). `--before YYYY-MM-DD` is exclusive: it matches files with mtime before the start of that UTC day, so the named day itself is never included (`--after` includes its named day from its start). Remote-knowledge WRITE operations are desktop-only in this build: the GUI registers roughly 35 `remote_kb_*` commands (collection create/delete/restore and permanent delete, document upload/replace/delete/restore/download, share create/stop, join approve/reject/cancel, device management), while this CLI ships only the four read-only `remote` lanes listed above — nothing here writes to a remote knowledge service. |
| `pinvou personas` | `list [--source builtin\|user\|all]`, `show`, `create`, `update`, `delete --yes`, `equip`, `unequip`, `active` | Expert card deck CRUD. `equip` records the staged persona for the session sidecar and it IS delivered on this surface: the next `pinvou agent run --session <id>` turn consumes the equip sidecar once — the staged body is prepended into the submitted prompt at the same injection point the GUI chat send uses, then cleared one-shot. Like the GUI's chat send, the delivery re-checks the card pool: if the staged card was deleted in the meantime, the turn runs WITHOUT the injection, a stderr warning names the deleted persona id, and the sidecar's staged body is cleared one-shot (`persona_id` retained, so `personas active` still reports the orphan until `unequip`). The desktop app keeps its own equip state and does not read this sidecar, so a CLI `equip` does not change how the GUI renders that session. `delete --yes` also sweeps every `persona_equipped.json` sidecar referencing the deleted card and reports the cleared sessions in `cleared_sessions`. Like the GUI's empty-input path, CLI-created cards fix `dept` to "specialized" and default emoji/color; a stdin body over the 4 MiB cap is a content error (exit 1). The sweep is one-sided: the GUI's own persona delete never touches `persona_equipped.json` (the sidecar is a CLI concept), so a card deleted from the desktop app leaves an orphan behind. `active` now fails on such a session instead of answering "none" — the sidecar is there and still holds the deleted card's full injection body at 0600 — and names `pinvou personas unequip <id>` as the remedy, which clears it without consulting the card pool. |
| `pinvou projects` | `list`, `create --name N [--root PATH]...`, `update <id> [--name N] [--root PATH]...`, `delete <id> --yes`, `move <session-id> [<project-id>] [--yes]`, `rebind <from> <to> [--yes]` | Session project grouping on the same `ProjectStore` as the GUI (the CLI's list JSON is a superset of the GUI wire DTO: the GUI omits per-root availability and the timestamps from the wire, the CLI renders them, plus the full `assignments` map). Deleting unassigns sessions, never deletes them. `move` without a project id is the GUI picker's ungrouped entry, and it is now gated the way the GUI gates it (`aria-disabled` unless the session resolves to a project): the session must currently RESOLVE to one — tier 1 the store's own assignment, tier 2 auto-grouping by its bound workspace directory — or the command is refused. The gate matters because the entry it writes is not a "clear": it is an EXPLICIT ungroup whose whole purpose is to stop auto-grouping from putting the session back, and nothing on either surface turns it back into "grouped automatically"; only another `projects move <session> <project>` overwrites it. Because that write is irreversible, the ungroup form requires `--yes` before any store access, like `projects delete`; the reassignment form `projects move <session> <project>` deliberately takes no `--yes` — its write is reversible through another move, so it is not in the destructive set with `delete`/ungroup/`rebind`. `move` also skips the GUI's add-workspace-root lane (needs the ACP pool) and rejects scheduled-run sessions like the GUI. `rebind <from> <to>` is the store's directory rebind under its fence: it migrates every project root and session assignment/workspace binding under the old directory onto the new one — session bindings first, project roots committed last after an overlap pre-flight, idempotent so a retried run converges — and it is gated behind `--yes` like the family's other wholesale writes. It deliberately skips the desktop-only runtime half of the GUI's `rebind_workspace_root` (active-turn eviction and post-busy reclaim need the live ACP/engine pools), and the command output discloses that skip. The store boots once per process and rewrites the whole file on every write: changes made while the GUI runs stay invisible to it until it reloads, and its next projects write overwrites them — do projects edits while the app is closed. |
| `pinvou code` | `agents list/status/install`, `login/logout`, `providers ...`, `sessions ...`, `workspace list/search/preview/changes/diff/branches/checkout`, `checkpoints ...`, `run`/`permissions`/`respond` (decline with `code_*_requires_product_host`; `agents install` declines the same way — the vendor install script runs under GUI supervision) | Code-mode (ACP) configuration and read-mostly workspace ops; read-only workspace ops call the GUI's own `codex_acp::workspace` module; checkpoints reuse the real shadow-git implementation; agent CLIs resolve like the GUI (override env var → official install dir → PATH; Windows `.exe`/`.cmd` aware), except that the GUI also tries the adapter-beside native runtime location for claude first, which is app-bundle-specific and not mirrored here. The busy cases surface with STABLE error codes, so scripts can match a code instead of message text: `rewind_busy`, `checkout_busy`, `diff_busy`, `undo_busy` (any `{action}_busy`: another pinvou process is mutating the session/root, exit 1) and `{action}_lock` when the lock file itself cannot be taken. `providers --wire-api` also accepts the app parser's `openai_compatible`/`chat` aliases. `providers add --agent claude` requires a `--model-slot SLOT=MODEL` pair for every slot in the app's published `CLAUDE_MODEL_SLOTS` list: `ProviderManager::save` treats every one as mandatory, because a missing slot makes Claude Code's sub-agent and helper calls fall back to official models (official traffic). The pre-check imports the app's published slot list (`features::codex_acp::CLAUDE_MODEL_SLOTS`, widened from `pub(crate)` for exactly this use), so it fails with a readable message naming the live slot ids instead of the store's untranslated "<slot> is a required field", and cannot drift when the app adds a slot. `code permissions`/`code respond` refuse before any session-store access (the refusal is unconditional, so the store boot's retention sweep never runs for them). `login` announces the authorization URL and device code on stderr the moment the vendor CLI prints them rather than after it exits: the child deliberately stays alive until the user opens the link (kimi's device-code flow cannot complete otherwise), so a link published at exit is published too late; the rest of the transcript is still echoed once, redacted, at the end, and a timeout keeps the captured link. Interactive ACP turns and the pending-permission flow are desktop-process-bound. |
| `pinvou files` | `ingest <PATH> [--output PATH]` | File → markdown extraction (pdf/office/email/archive/text), the GUI attachment pipeline. A relative `<PATH>` is resolved against the cwd first (the GUI's upload policy rejects a relative path outright, which is useless from a shell); the resolved path still has to be an existing regular file under `$HOME` and outside the credential directories. The policy errors and the `warning` chips are GUI i18n copy (Chinese) and are translated to English here, including the `$HOME` refusal, which explains the confinement rather than restating it. |
| `pinvou voice` | `transcribe <audio>`, `postprocess --mode ... (--text S\|--text-file F) [--draft S\|--draft-file F]`, `asr-status`, `asr-install [--yes]` | Transcription of an audio file up to 4 MiB (GUI `recording_too_long` bound); recording itself is GUI-bound. `--draft`/`--draft-file` carry what the GUI sends as `draft_text` (the input-box text), assembled into the same `DRAFT_TEXT` block the edit prompt's rule 10 promises, and `--mode edit` — whose whole job is rewriting that draft — now REFUSES without one (exit 2) instead of sending the model a prompt with nothing to edit (an empty or all-whitespace `--draft-file` counts as none; `--mode dictation`/`--mode task` silently drop an empty draft instead). Every postprocess result carries `omitted_stages` (`deterministic-rule-corrections`, `shrink-and-protected-term-validation`) in JSON and a trailing `Note:` line in human output: those are the GUI's client-side pipeline stages (`applyVoiceDeterministicCorrections` and `validateVoicePostprocessOutput`) that the CLI deliberately does not run, because the GUI writes its result straight into the user's input box unseen while the CLI prints it for a human to read. `asr-install [--yes]` is Linux-only and system-touching (it can install ffmpeg via the OS package manager), so it requires `--yes` like `deps install`; the model download is verified by sha256, and the command now takes a cross-process lock before any probe or download — a second concurrent run is refused with `asr_install_busy` rather than interleaving writes into the same `.part` inode (the GUI's equivalent guard is a process-local flag another process cannot see). `transcribe` reports `ffmpeg_missing` when only ffmpeg is absent, except for `.wav` inputs: the native lane feeds those the raw wav (GUI parity), so only a warning is printed. Postprocess reports `truncated: true` on the Anthropic preset when the response carries `stop_reason: max_tokens` — the GUI's detection, mirrored here (raise the token budget or retry if an answer still looks cut off). On macOS, `asr-status` reports the host Speech runtime (`ready: true`), but `transcribe` uses the external ASR CLI lane — the JSON adds `cli_transcribe_ready` for what the CLI itself can do. On Windows, the CLI probes the real engine/ffmpeg state instead of trusting the GUI's MSI-bundled-runtime branch, so its `asr-status` can disagree with the GUI's and reports `installable: false` plus `gui_install_only: true`. The note there now names the remediation that is actually reachable from this process: point `PINVOU3_ASR_CMD` at the MSI's bundled `pinvou-asr.exe` (or copy the engine and model into AsrDir). The old "repair or reinstall the desktop app" advice could never flip those flags — the MSI installs the engine and model beside the desktop executable, and the CLI only ever looks under `$PINVOU3_HOME/asr`. Making the CLI discover the MSI copy by itself is an app-side change, not a CLI one: those paths resolve through `pub(crate)` `platform::os::windows` helpers that this crate cannot call. |
| `pinvou deps` | `check`, `install <NAME...> --yes` | External dependency detection/installation (apt/Homebrew/bundled). `install` streams the installer's progress hook to stderr (the same hook the GUI turns into `deps:install_progress` events), so a multi-minute `brew`/`pkexec` run is not indistinguishable from a hang; stdout stays the single result line. Its JSON field is `requested`, not `installed`: the value is the caller's argv, and the adapters do not report back which packages the package manager actually placed on disk (Homebrew installs per package and joins the failures; apt installs the batch in one `pkexec` call), so naming it `installed` would read as a verified outcome it is not — `deps check` is the lane that answers what is installed now. On Windows, an `install` of an already-present dependency (LibreOffice) is a lib-owned no-op (`Ok(())`) that the CLI renders exactly like a real install — the same "Requested: …" line and "installer reported success" note — so the two are indistinguishable from the output alone; verify with `deps check`, which is the lane that answers what is installed now. |
| `pinvou feedback` | `submit --type issue\|suggestion --title T --body-file F [--attach PATH...]` | Writes one receipt under `feedback/receipts/` (0600 on unix, carrying the submitted request) and prints the GitHub issues URL (GUI opens the browser). Nothing is left under `feedback/pending/`, which means "failed to upload". The JSON carries an `uploaded` boolean — the field scripts should branch on, since the community edition never uploads and a human-readable `status` cannot be parsed for that — plus an `attachments` list echoing the registered paths. `--attach` registers a path only (nothing is read, nothing is uploaded), but the path lands in that permanent receipt, so the CLI refuses a path crossing a credential component (`artifact_crosses`-style `check_sensitive_path`) — a one-sided guard: the GUI only limits its own picker (image/video extensions `png/jpg/jpeg/gif/webp/mp4/mov/webm`, at most 5 attachments), and its Rust submit lane applies no path or extension policy, so the CLI's credential-path rule is stricter than the surface it mirrors, not a mirror of one. The CLI also does not restrict extensions or count the way the GUI picker does. |
| `pinvou monitor` | `status`, `snapshot` | One-shot model/GPU/vLLM sample instead of the live dashboard. Both subcommands emit one key set regardless of whether a model is configured. `snapshot` reports `self_perf` and `app.session_uptime_secs` as not-applicable (listed in `not_applicable_headless`): they accumulate over the desktop app's lifetime and a one-shot process cannot sample them. |
| `pinvou artifacts` | `list [--session ID]`, `read`, `write` | Cross-session deliverables index + markdown-safe artifact read/write. Reads are capped at 10 MiB (the GUI's edit cap — a larger artifact the GUI displays is CLI-unreadable) and the list scan skips per-record files over 32 MiB, reporting them as `skipped_sessions` in the JSON instead of silently shortening the index. `read`/`write` apply the credential-component half of the GUI's path policy: a path crossing a credential component is refused with `artifact_crosses_sensitive_component`. |

## Deliberately absent (GUI-bound)

Detach/tear-off windows, desktop pet, the embedded browser and its shared
control, artifact visual previews/design inspector, chat cards and streaming
rendering, voice recording UI and the global shortcut, drag-drop/clipboard
capture, the live theme/language switching and notification surfaces (`settings set` persists those prefs), QR image rendering
(URLs are printed instead), the WebUI remote-control pairing, the updater
(stubbed in the community build), the local vLLM bootstrap wizard, and the
super-permission pkexec toggle.

## Known limitations

- CLI consent/scope writes to `disabled_bundles.json` (`connectors
  enable`/`disable`, `plugins enable`/`disable`/`project-skills`, and the
  DenyAll sync after `plugins` installs/imports and `connectors
  apply-skills`/`ima connect`) serialize only in-process, like the desktop
  app's own writers; a CLI write racing a live desktop-app write to that file
  is last-writer-wins until the cross-process lock (#517) lands.
- The vendor-CLI spawn/drive plumbing (~16 spawn sites across ~10
  orchestration loops: connectors `ensure`/`connect`/drain/tar, voice ASR and
  model download, code `login`/probes) each carry their own drain/cap/kill
  plumbing; consolidating them behind a shared `run_vendor_cli` helper is a
  tracked follow-up, not done in this PR.
- The featureless build compiles again — lib and tests alike: `cargo check
  --workspace --no-default-features --locked` (and `--all-targets`) is clean
  since the family modules were `cfg`-guarded (commit 9a857d592;
  `knowledge.rs` no longer names `anyhow` unguarded — the reason
  `scheduled.rs`, `voice.rs`, and `monitor.rs` never name it at all), and the
  `#[cfg(not(feature = "product-backend"))]` fallback arms
  (`product_backend_not_enabled`) in `agent_task.rs` and `lib.rs` are
  reachable again — and a `cli-lint` step builds `--no-default-features` so
  the configuration can no longer rot silently.
- Voice postprocess does not run the GUI's client-side deterministic
  corrections or output validator (both named in every result's
  `omitted_stages`), and does not mirror the
  GUI's ASR engine-integrity verification. On Windows the CLI also cannot
  discover the engine and model the MSI installs beside `pinvou.exe`: those
  resolve through `pub(crate)` `platform::os::windows` helpers, so closing
  that gap needs an app-side visibility change. `PINVOU3_ASR_CMD` is the
  reachable workaround today. The ASR fallback contract also deviates
  headless by necessity: after a native engine failure the external-CLI
  fallback resolves ANY candidate command (configured override, managed
  dir, or `pinvou-asr` on PATH), while the GUI falls back only on an
  explicitly configured override and otherwise reports `asr_engine_error`.
- `knowledge index status` without a job id derives its answer from an
  infallible lib call, so once the store is open there is no failure it can
  report — an empty index reads as an idle status. The store open itself is
  fallible (`knowledge index store unavailable at ...`, exit 1), and `index
  status <job-id>` is fallible too: an unknown id is mapped onto the family's
  stable `knowledge_index_job_not_found` code rather than leaking the
  underlying driver's "Query returned no rows".
- Memory content caps (120 characters per item through `add`, fixed per-store slot
  counts) are feature-store facts shared with the GUI; `memory add` warns on
  stderr when normalization truncates a longer input.
- `deps install` on Windows renders an already-present dependency's
  lib-owned no-op (`Ok(())`) exactly like a real install ("Requested:" plus
  "installer reported success"), so the output cannot tell a no-op from a
  fresh install — the lib result shape has no no-op distinction; verify
  with `deps check`.
- Session eviction side effects: any store-opening command across the
  `sessions`, `scheduled`, `personas`, `projects` families boots the shared
  session store and runs its retention (see the family rows above). The sweep reports on and evicts from BOTH
  budgets in one pass: unpinned GUI chat sessions above the 50-session chat
  cap and unpinned headless `agent run` sessions (`agentic_` ids) above their
  own 50-session cap. Separating the budgets means headless runs no longer
  CONSUME the chat budget (the point of #504's carve-out), but it does not
  make a headless save blind to the chat bucket: a `pinvou agent run`
  prepare-time save runs the same sweep and can evict the oldest unpinned
  GUI chat sessions when the chat bucket is over cap; the stderr warning is
  bucket-aware — it carries the evicted count and names every evicted
  session id under its bucket (headless `agentic_` run sessions vs desktop
  chat sessions) rather than asserting a single kind — and it prints even
  when the run itself later fails, because the evicting save precedes the
  fault. The local `knowledge` lanes never boot it: `collections delete` does its job without the
  (provably empty) CLI-side mount sweep, and `knowledge mounts`/`mount`/
  `unmount` refuse before opening the store — precisely so an impossible
  command cannot evict sessions on its way to saying so. The remote/host
  lanes are NOT in that class: `remote probe`, `remote collections`,
  `remote search`, and `host status` ride the windowless product host
  (`run_windowless_host` boots the session store on its way up), so they
  run the retention sweep like every other store-opening command;
  `remote connections` does too whenever connections are configured — with
  none configured it answers offline without booting.
- Two inherited upstream gaps this CLI mirrors byte-for-byte: the
  credential-component path refusal (`files ingest`, `artifacts read`/`write`,
  `feedback submit --attach`) compares path components byte-exactly on unix,
  so on a case-insensitive filesystem (the macOS default, APFS) a component
  named `.SSH` or `ID_RSA` bypasses the check — the GUI's
  `check_sensitive_components` has the identical hole, so the fix must land
  upstream first; and the dingtalk vendor-output redactor matches
  `access_token` with an underscore only, so a camelCase `accessToken` value
  can survive into the redacted failure tail — a byte-identical mirror of the
  GUI's `safe_auth_log_line`, same upstream-first note.
- macOS-only runtime branches of the CLI (ASR status lanes, vendored locks)
  are compile-checked by CI (`macos-cli-check`) but executed only on a
  developer machine.

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
