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
  subcommand that takes its own `--output PATH` (the lanes that share the
  collision: `sessions export`, `plugins export`, `plugins recycle export`,
  `code providers export`, `files ingest`, and the legacy `benchmark
  submission gaia --output` alias) accepts any other value as
  that flag's argument — so `code providers export --agent claude
  --output json`, where the value names a destination file, would instead
  print the provider JSON WITH PLAINTEXT KEYS to stdout (without `--agent`
  the command still exits 2 on the missing required flag, so the hazard
  needs the complete command) — and a parser with no
  `--output` refuses the pair as an unknown flag instead of silently
  collapsing the tokens around it. A file literally named `json` or `human`
  at the end of the line must still be spelled `./json`.
- `pinvou --version` (or `pinvou version`) prints the CLI version. `pinvou
  --help` / `-h` prints the top-level usage on **stdout** and exits `0` (under
  `--output json` it is wrapped as `{"usage": ...}`); it accepts no further
  arguments, so `pinvou --help benchmark` is a usage error. There is no
  per-family `--help`: an invalid invocation prints the usage line for that
  family on stderr and exits 2, which is the same text in the diagnostic role.
- A panic that escapes every layer above exits `101` with one clean
  `pinvou: internal error (panic ...) ...` line on stderr — that is always a
  bug to report, never a diagnosable command failure. The windowless product
  host contains its own bootstrap/event-loop panics into ordinary exit-1
  failures (every host lane: `monitor`, `knowledge`, `voice`, `agent run`,
  benchmarks, the organize lanes), and `RUST_BACKTRACE=1` captures the trace
  for the report.
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
    `--yes` (`require_yes`), invalid session-id charset, non-UTF-8 argv
    (rejected wholesale before dispatch, naming the 0-based argv slot), and a few remaining
    content findings that ARE classified as usage in the current code: empty
    `--content` in `memory`, empty prompt/body in `scheduled`, and
    `voice postprocess --mode edit` with an empty draft
    file (dictation/task silently drop an empty draft), where every other
    input-content error in those same files is exit 1. (`personas` is the
    deliberate exit-1 exception: an empty persona body read from
    `--file`/`--stdin` is content-classified — whether the body is empty
    depends on the resource the command was pointed at, not the argv —
    so scripts must not branch on exit 2 for it.) Plus
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
- JSON output field naming: `snake_case` everywhere, with ONE documented
  exception class — commands that intentionally publish the GUI's wire DTOs
  (`scheduled` task/run objects, `code` provider/workspace objects,
  and the whole `knowledge` family — collections, index/import jobs, scan
  state, remote connections) reuse the webview's camelCase keys
  (`scheduleLabel`, `nextRunAt`, `checkedAt`, `jobId`, `collectionId`,
  `parseStatus`, …) so a script interpolating GUI state and CLI output sees
  one shape. The `code` family's SESSION objects are the border case and
  are snake_case (`created_at`, `agent_id`, `workspace_kind`, … — the
  underlying store DTO carries no serde renames), so a script must not
  assume camelCase across that family. Everything CLI-native is snake_case
  (`rebound_session_ids`, `type_counts`, `feedback_id`, …); no other family
  publishes camelCase keys.
- All commands operate on `$PINVOU3_HOME` (or `~/.pinvou3`), the same root the
  desktop app uses. Do not run CLI mutations against a data directory while the
  desktop app is mid-write on the same files.
- Secrets are never accepted as plaintext argv (shell history/process lists):
  credential flags are `--api-key-env VAR` or `--api-key-stdin` (`code login
  claude` also accepts the code via `--code-env VAR` / `--code-stdin`, and
  keeps a literal `--code C` for callers that already hold it in argv). One
  connector flow also re-passes a short-lived value as argv: the feishu
  connect poll re-invokes the vendor CLI with the device code it was given
  (`--device-code`), mirroring the GUI's flow — single-use and short-lived,
  but visible in a process list while the poll runs. Two
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
  keeps the vendor child's stdin open for the whole login. On a terminal the
  lane prints a reminder that the code must be terminated with Ctrl-D (the
  read consumes stdin to end-of-input, not to the first newline); piped
  callers are unaffected — their write end closes.
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
  the whole headless host run, so other mutating scheduled commands fail fast
  with `scheduled_store_busy` (exit 1) instead of sitting silent behind a
  minutes-long hold; retry after the run finishes. A GUI turn or GUI
  rewind takes none of these locks (the app's guards are process-local) — do not
  rewind while its GUI Code session may be mid-turn. Similarly, `agent run
  --session` does not lock the session against concurrent GUI use. Remaining
  shared stores (plugins `installed.json`, the memory JSONL files, and
  `settings.json` — every `models`/`settings` write goes through the app's
  load→mutate→atomic-rename `update_transaction` under an in-process mutex)
  have no cross-process lock in either surface: two racing writers each
  commit a whole-file snapshot, so the loser's change silently vanishes
  with exit 0. Avoid CLI mutations to these while the desktop app is
  running (and avoid two concurrent CLI writers). `acp-providers.json` is locked
  instead: every registry mutator on both surfaces takes a `.json.lock`
  sibling (an OS file lock, released when the holder dies) around its
  reload→mutate→persist section, so the CLI's writes are pulled into the
  app's next provider operation instead of being rolled back minutes later
  by a stale in-memory table. The residuals there are named for what they
  are: a live-but-stopped lock holder blocks the other surface's mutators
  indefinitely (crashes canNOT — the OS releases the lock), the
  same-instant rename race the other stores have, and a config-write vs
  store-current split between a mutator's `apply` and its `persist` when
  two surfaces switch concurrently (each store call is locked; the two
  together are not one transaction) — the next store mutator re-syncs the
  pair. `sessions rename` is the one to
  treat with real care: it rewrites the whole transcript JSON from a snapshot
  read moments earlier, so renaming a session the GUI is ACTIVELY streaming
  can drop the engine's newest messages — metadata mutations belong to idle
  sessions.
- Engine/model-backed operations (`memory organize`, `scheduled run`,
  `monitor status|snapshot`, `voice postprocess`, `knowledge remote *`) need a
  configured active model where they sample one — the same prerequisites as
  `pinvou agent run`. `memory organize`, `scheduled run` and `voice postprocess`
  boot the full windowless product host, which needs a display (or
  `xvfb-run`); `monitor status|snapshot` and the `knowledge remote *` reads run
  on a bare async host instead — no Tauri context, no session-store boot (so
  no retention sweep can fire from a status command) and no display, which
  makes them usable on headless Linux.
- Input caps beyond the 4 MiB ones called out in the family rows:
  `feedback submit --body-file` and `memory add --file` read at most 64 KiB
  (over-cap is exit 1, like every `read_text_file_capped` failure). `monitor status|snapshot` and `knowledge remote
  connections` answer offline without a model — where "without a model"
  means "nothing answers the default endpoint either": with no model
  configured the monitor still probes `http://127.0.0.1:8000/v1`, so an
  unrelated service listening there reads as the backend (GUI parity, not
  a zero state); `monitor` reports a clean zero state instead of an error.

## Command families

| Family | Commands | Notes |
|---|---|---|
| `pinvou agent run` | `--prompt-file [--workspace] [--timeout-secs] [--session ID] [--mode plan\|agent] [--model ID] [--attach PATH]...` | One product-equivalent agentic turn: unlimited tool-call rounds and a persisted session (GUI parity; `PINVOU3_AGENT_TASK_KEEP_SESSION=0` restores one-shot cleanup). See [agent-task-cli.md](agent-task-cli.md); the session/mode/model/attach flags extend it without changing its defaults or exit contract — and `--attach` applies the same credential-path refusal as `files ingest`/`feedback --attach` (`check_sensitive_path`, canonicalized), because the GUI's picker refuses credential paths before a chat send and this lane must not be the way around it — but `--prompt-file` itself was narrowed — it must now be a REGULAR file (symlinks are followed; FIFOs, character devices, `/dev/stdin` and `<(...)` process substitution are refused, because their read blocks before any cap or deadline can act) of at most 4 MiB, both exit 1. |
| `pinvou benchmark` | `list`, `run smoke`, `run/fetch/verify/score/submission gaia`, `status`, `resume`, `report` | Evaluation harness; see [gaia-benchmark.md](gaia-benchmark.md). |
| `pinvou sessions` | `list [--archived] [--limit N]`, `show [--last N] [--full]`, `rename`, `pin`, `unpin`, `archive`, `restore`, `delete --yes`, `export [--format markdown\|json] [--output PATH]`, `timeline`, `subagents`, `folder` | Same `SessionStore` the GUI uses, including scheduled-run cascades. Any store-opening command (even reads like `list`) runs the shared retention sweep, which counts headless `agent run` sessions (`agentic_` ids) against their own 50-session budget, separate from the 50-session chat budget the GUI's chat sessions occupy — so a CLI store open can evict the oldest non-pinned GUI chat sessions (pins are exempt, on the durable pin file, including pins made in the GUI while the CLI process is live), and likewise evicts the oldest non-pinned headless runs when that bucket is over cap. Avoid `rename`/`pin`/`archive`/`restore`/`delete` while the desktop app is running, streaming or not — but scope the worry to what the store actually caches: the list cache pairs its generation counter with the sessions directory's entry-NAME set (not the directory's mtime, which the store deliberately rejected — two mutations landing inside one timestamp tick leave an mtime identical), so create/delete by the CLI DOES invalidate it — a CLI-deleted session does not resurface in a later GUI listing. What stays stale until the app restarts are the per-process maps: an in-place title rewrite does not move the directory mtime, so a CLI `rename` can resurface the old title on the GUI side until the app's next own store write or restart (the pin/hidden maps genuinely last until restart), and pin/hidden state (boot-time maps) resurfaces an un-pinned pin state the same way. The momentary loss is the ordinary read-modify-write race (last writer wins for a write that lands between the GUI's load and save), not a delayed write-back: `set_pinned` and `set_title` are id-level durable RMWs that preserve a CLI write that landed earlier. `delete` performs the GUI's full cascade: after the transcript directory it also drops the session's record from the `session-agents.json` index (without that second half the app's boot-time sidecar backfill re-created `sessions/<deleted-id>/code-session.json`, resurrecting a ghost directory for a session that no longer exists); the index file is only touched when it already exists, and a delete that committed but could not clear the record fails loudly rather than silently. `rename`'s parser refuses a title containing a mid-title `--output json`/`--output human` pair followed by more title words (exit 2, nothing stored — a pair at the very END of the line is how JSON output is requested, so a pair still visible inside the title cannot be that), and refuses a title that looks like a flag; such titles must be set from the desktop app. ACP/code sessions are listed too — the CLI has no live pool to filter them like the GUI does. Residual `eval_`-prefixed sessions are invisible to `list` (this build filters them like the benchmark lanes) while retention still counts them, so they can hold retention slots without appearing here. A delete that finds the index UNREADABLE starts from the empty table and persists that over the corrupt file (the app's own bless-and-replace stance): every other record in the index is dropped from it — restorable via the app's boot-time sidecar backfill, but not by this command; the corruption is announced on stderr by the store layer. |
| `pinvou models` / `pinvou settings` | `models list/add/edit/remove/use/show [--reveal-key]/test/probe-local [--url U [--model ID]] [--api-key-env V]`; `settings get/set`, `settings search list/set [--clear [--yes]]/test` | `settings` is an alias routed to the same module. `models list`/`show` JSON uses the GUI DTO's field names (`context_window_tokens`, `max_output_tokens`, plus the CLI-writable `alias`/`provider_kind`/`vendor`/`endpoint_mode`/`vision_model_id`), so a script reads back what it wrote without a key map. Settings writes go through the GUI's own prefs transactions (migrations and locale policies included); the prefs lock is in-process, so a CLI write racing a GUI write is last-writer-wins on the whole settings file and can drop the other side's change — edit settings from one side at a time. `settings get` without a key always prints the full settings JSON regardless of `--output human`. Even the read commands of this family (`models list`/`show`/`test`, `settings get`, `search list`/`test`) load settings through the persisting `UserPrefs::load`, so a read can rewrite `settings.json` as a side effect — including the plaintext-key migration that moves an old API key out of the file and into the OS keyring. `models remove` follows the app's ordering contract: the model is removed from settings first, then its keyring secret is deleted best-effort — a failed keyring delete only warns on stderr and the command still succeeds, so a broken keyring can leave an orphaned credential entry. `models edit <id>` mutates a stored model IN PLACE and never touches the id, which is the reason it exists: the previous `remove` + `add` round-trip minted a new id and orphaned every per-session model binding and scheduled-task model pin that named the old one. Its credential lanes mirror the GUI — no flag keeps the existing secret, `--api-key-env`/`--api-key-stdin` replace it inside the save transaction, `--clear-api-key` marks the record missing in the transaction and defers the keyring delete until after the commit — the delete runs only after a successful save, so the clear lane never leaves a configured model without a secret; the orphan risk belongs to the REPLACE lane, where a failed save's best-effort rollback can itself fail and orphan the previous keyring entry — and because it deletes a stored credential it is gated behind `--yes` like the other destructive actions. `add` and `edit` share the same five GUI-form metadata flags (`--alias`, `--provider-kind`, `--vendor`, `--endpoint-mode`, `--vision-model-id`); `--provider-kind` is an allow-list (`official_api`/`custom`/`coding_plan`) and a blank metadata value is a usage error, because the prefs normalizer would discard either. `add`/`edit` also accept the payload knobs the JSON body carries (`--preset`, `--context-window`, `--max-output`, `--reasoning-effort`) plus `--set-active`, which re-points the durable active model — a state change, not a metadata edit, and undiscoverable from the metadata list alone. `probe-local` is deliberately stricter than the GUI's local-server probe: loopback addresses only (the GUI also accepts LAN/private hosts), a credential-read failure fails the probe, and redirects are not followed; `--model ID` is its spelling of the GUI's `model_id` — it names which SAVED model's stored credential the probe should present (mutually exclusive with `--api-key-env`, and only meaningful with `--url`). The two probe commands report a malformed STORED `base_url` over two different channels by design: `models test` renders it as a result payload on stdout (`{"ok":false,"code":"invalid_url",...}`, exit 1, `render_probe_outcome`'s shape) because every `models test` outcome is a probe result, while `models probe-local` reports it as a stderr `CliError` (exit 1, "invalid_url: …") because the URL guard classifies it as a host failure before any probe request can run; both agree on the `invalid_url` code and exit 1 even though the channel differs. When every signature probe is answered 401/403 the result is `kind: unknown_authenticated` with exit 1 — deliberately not one of the GUI's kind strings, because each of those asserts something about the server's identity and this case asserts that nothing was learned (the sequential mirror used to report `generic` here, the silently wrong answer). A probe round that could not COMPLETE (a probe thread panicked) answers `kind: probe_failed` with exit 1 on the same doctrine, instead of reading an incomplete round as `generic`. `settings search test <provider>` now issues a REAL request for all five providers (bing, tavily, bocha, metaso, baidu) using the request shapes the product's own search tool sends; the new `verified` field says what `ok` is a statement about — `live_probe` when a request was actually sent and its status classified, `credential_presence` when nothing was sent because no key is configured or the credential could not be read, `nothing` when the probe could not run at all. Previously four of the five returned `{"ok":true,"code":"configured"}` on mere credential presence, so a revoked key passed a command called `test`. `settings search set --provider P --clear` no longer switches the active provider as a side effect: clearing is a credential operation that deletes the stored key (requiring `--yes`), and only a caller actually selecting a provider moves search onto it. Bing is selectable but takes no credential: `settings search set --provider bing --api-key-env V` is refused with exit 2 ("provider bing does not use an api key"), matching the provider's keyless request shape. `models show --reveal-key` prints `api_key_source` (`credential_store` / `environment` / `none`, rendered ahead of the secret so the verbatim value stays the last line): an `environment`-override model has a working key the CLI deliberately does not echo, which the old single `(not stored)` placeholder reported as if there were no key at all. |
| `pinvou memory` | `overview`, `profile get/set`, `list [--store]`, `add preference/work-context`, `update`, `delete --yes`, `archive`, `pending confirm/ignore/never`, `organize --yes`, `organize-history` | `organize` rewrites the stores under LLM decisions, so it is gated behind `--yes` like the other destructive actions; it needs the model host (the organize pass runs with global prefs and the ACTIVE model — the same fallback the GUI's fresh-bridge path uses — and there is no `--model-id` on this command), boots the session store on the way up (see Known limitations: the retention sweep can evict), and, like the GUI, refreshes `snapshot.md` afterwards (best-effort). `organize` also takes a cross-process fd lock (`$PINVOU3_HOME/locks/memory-organize.lock`) before booting the host, so a second concurrent CLI pass is refused with `memory_organize_busy` instead of letting two CLI passes interleave destructive apply steps from up-to-75-second-old snapshots. Cross-surface exclusion now holds too: the feature layer's `organize_memory_with_llm` — the funnel every surface goes through (GUI button, scheduled executor, this CLI host lane) — takes its own cross-process `.organize.lock` (user memory directory) around the whole pass, so a GUI-triggered organize fails with a busy error while a CLI organize runs and vice versa. The residual disclosed gap is the post-pass `snapshot.md` refresh: it runs after `organize_memory_with_llm` returns and therefore outside `.organize.lock`, so two passes' snapshot refreshes can still interleave even though the store mutations stay inside the lock on every surface. `overview` reads like a read-only summary but also rewrites the shared `snapshot.md`, and it does so with `runtime: None` — a one-shot CLI owns no active session — so the rewritten document loses the runtime section the desktop app wrote, until the app next refreshes it. The command says so on stderr and in its own output (`snapshot_rewritten_without_runtime`) rather than leaving it to be discovered. Adding to a preference/work-context topic replaces the previous item in that bucket; profile-shaped preference text is rejected up front with `memory_add_not_materialized` and touches nothing. The same error refuses a dedupe-reused pending row that is not this add's own candidate: the enqueue's dedupe hands back an existing pending row matched on the kind plus a case-insensitive content key and keeps that row's own topic, so a same-text GUI candidate queued in another topic bucket would otherwise be confirmed from here — approving a candidate the user never reviewed and replacing that bucket's item; topic, kind and text are each compared before the confirm, and nothing is confirmed or written on mismatch. `list --store` JSON is always `{items, cleanup_warnings}` for every store. Content is capped at 120 characters per item for both stores: `add` routes through the shared pending queue, which normalizes candidates to 120 before either store's own limit (160 for work context) can apply. A longer input is reported — `add` warns on stderr and sets `truncated`, `submitted_characters` and `stored_characters` in its output — never silently truncated, and `update` applies the same disclosure for its per-store writer caps (preferences 120, work context 160, timed stores 180). Separately from the cap, the confirm-time sentence cleanup (`clean_candidate_sentence`, the same stage the GUI runs) strips 请记住-style leading prefixes and outer punctuation from work-context text before it is stored, so what lands in the store can be a shorter sentence than what was submitted — that normalization is not part of the cap disclosure and fires no `truncated` flag. Write-time refusals: `add`/`update` can reject content (`memory_add_failed: memory candidate looks sensitive or task-like`, never-list blocks, empty preference) — that write-time gate is why `list`/`overview` render store content VERBATIM with no read-time redaction. `organize` refuses with `memory_organize_disabled` when the memory feature is disabled for the current app language (default installs and any non-zh-Hans setting). The gate is organize-only and that asymmetry is deliberate: `add`/`list`/`update`/`delete`/`pending` read and write the stores without consulting the locale policy — pure, reversible storage the disabled app ignores — so gating them would only break scripting on installs that merely do not want the LLM pass. |
| `pinvou scheduled` | `list`, `show`, `create`, `update`, `pause`, `resume`, `pin`, `unpin`, `delete --yes`, `run`, `runs [--limit N]`, `runs-all [--limit N]`, `mark-viewed`, `chat-prompt` | There is no daemon here: a task fires only when the desktop app's scheduler sweep runs, so a task created while the app is closed starts firing at the next app start. Run HISTORY is bounded only by the desktop's retention loop (`MAX_TERMINAL_RUNS_PER_AUTOMATION`, pruned by the app's sweep): the CLI's `run` appends terminal records and never prunes, so cron-driven headless use grows a task's `runs/` directory without bound — reads degrade linearly and any later GUI run prunes back to the cap. Surfacing the foundation's prune through `pinvoy3_lib` is the upstream-first follow-up. What a running app re-reads and what it holds in memory are different halves. Task DEFINITIONS are re-read from disk on every sweep (`AutomationManager::list_automations` does a fresh `read_dir`), so a CLI `create`/`update`/`delete`/`pause`/`resume` is seen by a live app at its next tick without a restart. The SIDECARS the CLI co-owns — task kind, model binding, pin/UI metadata, run read-state — are no longer read once and rewritten whole from a stale in-memory copy: every app-side mutator of those files now re-reads the file first whenever it changed on disk (a cheap `FileStamp` identity check — length, mtime, and on Unix the file's inode, so a foreign write that preserves the byte length is still noticed), so a running app MERGES a CLI `pin`/`unpin`/`mark-viewed`/kind/model-binding write into its own next write to that sidecar instead of overwriting it. The residual same-instant write race is now fail-loud rather than silent: a persist whose file moved since this handle's last read (the collision window between the reload and the rename) is REFUSED with an error naming the retry, so neither side's write is destroyed — the loser's caller rolls back, the retry's reload merges the winner, and only a genuine double-write to the identical instant still loses one (Windows keeps the disclosed len+mtime aliasing corner, where such a same-length same-tick write is undetectable). Archived-run reads are stamp-gated too: a CLI `delete`'s archived runs show up in a running app's sidebar at its next read instead of at its next unrelated archive mutation. A CLI MUTATION refuses with `scheduled_store_unreadable` (exit 1) instead of writing when its read of that sidecar was unhealthy — the file was malformed (quarantined to `.invalid-<timestamp>` beside the original), is absent right after a quarantine (the desktop app may hold the only healthy copy in memory and would be overwritten by a default-based rewrite on its next persist), or is unreadable at rest — so a corrupt sidecar never silently rewrites the registry from its default and drops the other tasks' entries; repair the file (or remove the `.invalid-*` copies to start fresh) and retry, and read commands keep degrading to the default with a warning. The executor lookups still re-read on a MISS: a task whose kind this handle has never seen, and a task whose model binding it has never seen, both pay one disk read before answering, so a task the CLI created while the app was up runs as its real kind on its pinned model instead of as an unattended full-permission chat — a guarantee that covers the chat kind only: a `memory-organize` task's recorded `--model-id` is displayed but not applied by its run path (GUI parity; that kind executes with the active model on both surfaces). A HIT is still answered from memory, so a CLI edit that changes an entry the app already cached (flipping a kind back to chat) stays invisible until the app's next write to that sidecar reloads it. A CLI `--model-id` change is no longer held hostage to that cache: it rewrites the DEFINITION's model wire name as well (definitions are re-read every tick), so the executor never sees the old wire name with the new pin. The reverse also holds: a CLI `delete` cannot stop a run the app's scheduler already started. `run` executes `memory-organize` tasks headless; chat-kind runs need the desktop runtime. `update --kind/--mode` are rejected (creation-time properties; every run is forced to `yolo` like the GUI), `--model-id X` (on `create` and `update` alike) resolves the model pin and the definition's model wire name as ONE pair — X's own wire name, exactly the GUI's `model: selected.model, modelId: selected.id` — and an unknown id is refused (`model not found: X`, exit 1) before anything is persisted; an active create (and `update`/`resume` that change an active task's schedule or status) resolves the next run slot eagerly through the foundation, so a past `FREQ=ONCE;AT=` is refused (`no future run`); an `update` that touches neither schedule nor status (e.g. a rename) does not re-resolve or re-refuse instead of being silently paused by the first sweep, and `create --paused` still stages a past stamp like the GUI. `run` reconciles stranded CLI queued records (no foundation task id) to a terminal failed record on the next run, and `delete` only refuses GUI-owned active runs — a CLI process killed mid-run cannot wedge a task. Created tasks default `auto_approve` to true (the GUI's new-task default) and read `allow_shell` from the environment/settings like the GUI. Every scheduled command that boots the session store (including reads) runs the same retention sweep as the `sessions` family — counting headless `agent run` sessions (`agentic_` ids) against their own separate 50-session budget — so it can evict the oldest non-pinned GUI chat sessions (and the oldest non-pinned headless runs when that bucket is over cap); scheduled-run sessions themselves are exempt from both budgets. The delete receipt always returns `deletedSessionIds: []` — the GUI populates real deleted session ids from its live pool, which a one-shot CLI cannot enumerate; scripts must not branch on that field. The fail-loud persist guard is the APP side's: the CLI's own registry writes still go read→write without a stamp re-check, so a GUI write landing inside a CLI write's window can still be renamed over (a millisecond window at human frequency), and the guard itself keeps a residual check→rename sliver of the same shape on the app side (a foreign write lost there forces a merge re-read on the next touch instead of freezing the cache). Terminal run RECORDS written by CLI `run`s are not pruned by the CLI — the GUI's 50-terminal-run retention sweep lives in the desktop app — so a purely headless setup accumulates one record per invocation per task until a GUI run prunes it back; watch `runs`/`runs-all` output size in cron-only setups, and bound it with `runs --limit N` / `runs-all --limit N` — the limit is pushed down per task (each task's newest N are read before the global merge, which then keeps the newest N overall). One more disclosed window on the destructive path: a CLI `delete`'s run-directory removal cannot fence the app's scheduler tick (the fd-lock is CLI×CLI only), so a tick whose definition read predates the provisional pause but whose run persist lands inside the delete can lose that run record unarchived, or leave it an orphan of a deleted automation — the window is sub-second and gated on the app's tick; the desktop's own delete fences it with cancel-active-run plus reconcile. |
| `pinvou plugins` | `tools list [--installed-only]/install/uninstall/auth/oauth-*`, `skills list [--installed-only]/install/update/uninstall`, `import <PATH>`, `export [--output PATH]`, `meta`, `recycle ...`, `readiness`, `enable/disable [--scope]`, `project-skills on\|off` | `import` replaces the GUI's native dialog. OAuth login declines headless (`oauth_login_unavailable_in_cli`): the interactive grant happens in the desktop app. `readiness` reads credential presence from the OS keyring (for `ima` on every run regardless of install state; for other bundles when installed), which can prompt for access on macOS, and it NEVER reports a `cli`-kind bundle (the connectors) as ready: such a row always carries `ready:false` with `probe:"unavailable_in_cli"` and, absent any registry-visible fault, `reason:"connection_unknown_in_cli"`. A connector's readiness IS its live connection state, which the desktop answers from a `*_status` probe this crate does not link, so `false` there means "not determined here", not "known broken" — `pinvou connectors status` is the authority. Every other row carries `probe:"registry"`, and an installed package whose assets are gone is demoted to `reason:"assets_missing"`. Three write-path deviations from the GUI, each printed at the point of action: `tools install` skips the desktop's post-install `validate_remote_connection` handshake AND the rollback that handshake guards, so a tool the GUI would have uninstalled stays installed here; `tools uninstall` does not delete stored remote OAuth tokens, so a reinstall of that tool is still authorized; and `enable`/`disable` send no hot-refresh broadcast (the GUI's `refresh_live_sessions_skills` + `refresh_permission_rulesets` need the engine pool), so a running desktop app keeps its live engines on the whitelist they started with until they respawn. `--scope both` is two independent single-scope writes with no two-scope transaction behind them, so a failure on the second scope cannot roll the first one back — the error names which scopes already landed, and the success payload reports them as `scopes_applied`. Each scope's write is the GUI's own load-modify-save (`load_disabled_bundles_for` → `save_disabled_bundles_for`), verified on the exact list handed to the writer, and a failed write fails loudly instead of reporting an unpersisted success (concurrency with a running desktop app: see Known limitations). The install/import DenyAll syncs and the uninstall scope cleanups carry the same fail-loud contract, and the uninstall lanes snapshot every scope-cleanup owner BEFORE any directory disappears, then remove through the exact-owner form — a deleted id's normalized lookup can be re-owned by a foreign pack's claim, and the un-snapshotted removal would erase that pack's consent rows. Export and recycle-export refuse to overwrite an existing destination. `tools install <id> --secret KEY=ENV_VAR_NAME` (repeatable, one per manifest config key) feeds the tool's declared config from environment variables — the argv carries only the variable NAME, never the value, the same contract as `--api-key-env`. Builtin plugins (the embedded catalog, e.g. `session-reader`) can never be disabled, hidden or uninstalled: `plugins disable <builtin>` and `tools uninstall <builtin>` exit 1 with the builtin rule and leave the persisted toggle state untouched (reading the scope may itself persist the GUI-shared plain→DenyAll migration marker, but no scope list or initialization is written). |
| `pinvou connectors` | `status [<CONNECTOR>]`, `ensure-cli <CONNECTOR>`, `enable/disable <CONNECTOR>`, `logout <CONNECTOR> --yes`, `apply-skills <CONNECTOR>`, `connect <CONNECTOR> [--timeout SECS]`, `ima status` / `connect --client-id-env V [--api-key-env V\|--api-key-stdin]` / `logout [--yes]` | Vendor-CLI lifecycle for feishu/wecom/dingtalk/tmeet plus the ima skill connector (secrets via env/stdin only, like everywhere else). Vendor CLIs resolve in the GUI's order (lock-table install path → managed bin dir → npm global prefixes → PATH), so CLI-installed and GUI-installed vendor CLIs are visible to each other; `status` is the live connection authority the `plugins readiness` rows defer to. `connect` announces the login/QR URL live as the vendor prints it, and wecom's `qr.png` is written to a scratch directory the CLI points at (QR *rendering* stays GUI-bound, see the intro). `ensure-cli` installs the lock-table-pinned archive through the GUI `download_verified` candidate chain (opt-in GitHub acceleration-prefix env → the lock table's reviewed `mirrorUrl` → the official URL; each candidate sha256-verified, and the error names the candidate class, never the URL) — the mirrors exist because the official endpoints are unreachable on the networks the lock table was reviewed for, so a single-URL fetch would fail permanently there; tmeet installs through npm with the GUI's own two-attempt chain (default registry, then one npmmirror retry — a first-attempt timeout counts as the failure that triggers the retry, exactly like the GUI, and a double failure aggregates both attempts' causes; both attempts append to the shared `cli-install.log` under marker lines) — note this lane (and the tmeet shim execution) requires a system `node`/`npm` on PATH, where the desktop wraps its bundled runtime; the failure names the remedy, and both install lanes serialize through the cross-process `locks/connector-install.lock`. Two headless deviations, disclosed in the command output rather than silently: materializing the skill directories unpacks the desktop app's embedded bundle, so the SHOW direction reports `skills_unpack: "app-only"` (the HIDE direction on `logout` removes the skill dirs itself and reports failures), and the execpolicy ruleset hot-refresh after `connect`/`apply-skills` needs the GUI's engine pool, so it is reported as not run. Consent writes (`enable`/`disable`, the DenyAll sync after `apply-skills`/`ima connect`) serialize only in-process — see Known limitations — and a failed consent write fails the command with the GUI's consent-failure marker instead of reporting success with zero consent (the install/connection itself has landed; the error names the residual). |
| `pinvou knowledge` | `scan`, `stats`, `type-counts`, `collections ...`, `documents ...`, `index ...`, `search`, `model status/download/cancel`, `mounts/mount/unmount`, `remote connections/probe/collections/search`, `host status` | `collections add-sources`, `index resume` and `index retry` block until their import job reaches a terminal phase (like `scan start`): a one-shot process exits right after printing, so a fire-and-forget import thread was reaped mid-work and its job stranded `running` on disk with no owner. Their exit code and header are phase-honest (`done` exits 0; `interrupted`, `cancelled` and `done_with_errors` exit 1 and name the remedy). The wait has a no-progress liveness bound (300 s by default, overridable with `PINVOU_KB_IMPORT_STALL_MILLIS`); the liveness keys are job-state transitions, which do not move while the source-root walk or the model load runs, so a source tree whose walk alone exceeds the bound trips it on every attempt — a healthy but huge import needs the env override, not a fix; on timeout the invocation INTERRUPTS its stalled job first, so it is left `interrupted` — immediately resumable with `index resume <job-id>`, no desktop-app boot required — and the report says so (if even the interrupt cannot land, the remedy text honestly keeps the app-boot route). No CLI command ever runs the GUI's boot recovery of interrupted jobs — that is the desktop app's own crash handler; a job a killed one-shot process strands keeps its `running` state on disk, and reads keep reporting it until the app's next start relabels it `interrupted`/resumable. While the latest job is still `running`, `add-sources`/`resume`/`retry` refuse (its owner is another process; `index cancel <job-id>` can drop it — immediate and deliberately NOT gated behind `--yes`, unlike `collections delete`/`documents remove`; the drop discards the staged progress (a cancelled job can never `resume` or `retry` — those accept only `interrupted`/`done_with_errors` — so recovery means re-running `add-sources` from scratch, and the cancel output says so). There is no signal handler on the import lanes, so Ctrl-C kills the CLI the hard way: the job is recoverable exactly like a crash (staged progress survives; the app's next boot flips it to `interrupted`). `model download` declines headless (the in-process ONNX verification and progress events are GUI-bound); `model cancel` and `scan cancel` refuse honestly (`knowledge_{model_cancel,scan_cancel}_requires_product_host`) — the flags they could set are process-local to the desktop app, and nothing inside a one-shot process can ever be cancelled through them. `scan start` waits for the scan to finish inside the invocation (a fire-and-forget scan would be killed by process exit before doing any work), canonicalizes `--root` so a relative path or a symlink keys entries the way the index already does, and refuses a missing or non-directory root before starting. The incremental stale sweep only deletes entries that live under the roots the scan actually walked WITHOUT walk errors — any unreadable subdirectory (a chmod-000 subtree, an I/O error mid-walk) vetoes that root's stale sweep for the round, because the unreadable slice is just as undecidable — so scanning one directory never prunes what was indexed from another, and a vetoed round still reports `done` while ghost entries under the error root LINGER until a fully readable round cleans them (the safe direction; the veto is not reported separately). `collections delete` never boots the session store: mounted collections live in the desktop app's process memory and are deliberately not persisted, so there is no CLI-side mount to sweep, and the `SessionStore::boot()` retention side effect is not triggered (a booted sweep here was always empty by construction). `mounts`/`mount`/`unmount` refuse with `knowledge_*_requires_product_host` — mounted collections live in the desktop app's process memory and are not persisted, so a one-shot process can neither observe nor change them — and the refusal is returned BEFORE any store is opened. That ordering is the point: `SessionStore::boot()` is not a read, it enforces retention and irreversibly deletes the oldest non-pinned sessions, and booting it merely to decorate an unavoidable refusal with "session not found" destroyed chat history as a side effect of a command that can never succeed. The session-id charset gate still runs first, at parse time (exit 2). `--before YYYY-MM-DD` is exclusive: it matches files with mtime before the start of that UTC day, so the named day itself is never included (`--after` includes its named day from its start). Remote-knowledge WRITE operations are desktop-only in this build: the GUI registers roughly 35 `remote_kb_*` commands (collection create/delete/restore and permanent delete, document upload/replace/delete/restore/download, share create/stop, join approve/reject/cancel, device management), while this CLI ships only the four read-only `remote` lanes listed above — nothing here writes to a remote knowledge service. The wait carries the same no-progress liveness bound as the import lanes (300 s by default, overridable with `PINVOU_KB_SCAN_STALL_MILLIS`); on timeout it exits 1 with the partial-results caveat rather than hanging on a wedged scan thread, and a scan thread that PANICS lands the scan phase at `interrupted`, which this lane reports as an error instead of waiting forever. |
| `pinvou personas` | `list [--source builtin\|user\|all]`, `show`, `create`, `update`, `delete --yes`, `equip`, `unequip`, `active` | Expert card deck CRUD. `equip` records the staged persona for the session sidecar and it IS delivered on this surface: the next `pinvou agent run --session <id>` turn consumes the equip sidecar once — the staged body is prepended into the submitted prompt at the same injection point the GUI chat send uses, then cleared one-shot. Like the GUI's chat send, the delivery re-checks the card pool: if the staged card was deleted in the meantime, the turn runs WITHOUT the injection, a stderr warning names the deleted persona id, and the sidecar's staged body is cleared one-shot (`persona_id` retained, so `personas active` still reports the orphan until `unequip`). The desktop app keeps its own equip state and does not read this sidecar, so a CLI `equip` does not change how the GUI renders that session. `delete --yes` also sweeps every `persona_equipped.json` sidecar referencing the deleted card and reports the cleared sessions in `cleared_sessions`. Like the GUI's empty-input path, CLI-created cards fix `dept` to "specialized" and default emoji/color; a stdin body over the 4 MiB cap is a content error (exit 1). The sweep is one-sided: the GUI's own persona delete never touches `persona_equipped.json` (the sidecar is a CLI concept), so a card deleted from the desktop app leaves an orphan behind. `active` now fails on such a session instead of answering "none" — the sidecar is there and still holds the deleted card's full injection body at 0600 — and names `pinvou personas unequip <id>` as the remedy, which clears it without consulting the card pool. |
| `pinvou projects` | `list`, `create --name N [--root PATH]...`, `update <id> [--name N] [--root PATH]... [--clear-roots]`, `delete <id> --yes`, `move <session-id> [<project-id>] [--yes]`, `rebind <from> <to> [--yes]` | Session project grouping on the same `ProjectStore` as the GUI (the CLI's list JSON is a superset of the GUI wire DTO: the GUI omits per-root availability and the timestamps from the wire, the CLI renders them, plus the full `assignments` map). Deleting unassigns sessions, never deletes them. `move` without a project id is the GUI picker's ungrouped entry, and it is now gated the way the GUI gates it (`aria-disabled` unless the session resolves to a project): the session must currently RESOLVE to one — tier 1 the store's own assignment, tier 2 auto-grouping by its bound workspace directory — or the command is refused. The gate matters because the entry it writes is not a "clear": it is an EXPLICIT ungroup whose whole purpose is to stop auto-grouping from putting the session back, and nothing on either surface turns it back into "grouped automatically"; only another `projects move <session> <project>` overwrites it. Because that write is irreversible, the ungroup form requires `--yes` before any store access, like `projects delete`; the reassignment form `projects move <session> <project>` deliberately takes no `--yes` — its write is reversible through another move, so it is not in the destructive set with `delete`/ungroup/`rebind`. `move` also skips the GUI's add-workspace-root lane (needs the ACP pool) and rejects scheduled-run sessions like the GUI. `rebind <from> <to>` is the store's directory rebind under its fence: it migrates every project root and session assignment/workspace binding under the old directory onto the new one — session bindings first, project roots committed last after an overlap pre-flight, idempotent so a retried run converges — and it is gated behind `--yes` like the family's other wholesale writes. It deliberately skips the desktop-only runtime half of the GUI's `rebind_workspace_root` (active-turn eviction and post-busy reclaim need the live ACP/engine pools), and the command output discloses that skip. The store boots once per process and rewrites the whole file on every write: changes made while the GUI runs stay invisible to it until it reloads, and its next projects write overwrites them — do projects edits while the app is closed. |
| `pinvou code` | `agents list/status/install`, `login/logout`, `providers ...`, `sessions ...`, `workspace list/search/preview/changes/diff/branches/checkout`, `checkpoints ...`, `run`/`permissions`/`respond`/`providers probe` (decline with `code_*_requires_product_host`; `agents install` declines the same way — the vendor install script runs under GUI supervision) | Code-mode (ACP) configuration and read-mostly workspace ops; read-only workspace ops call the GUI's own `codex_acp::workspace` module; checkpoints reuse the real shadow-git implementation; agent CLIs resolve like the GUI (override env var → official install dir → PATH; Windows `.exe`/`.cmd` aware), except that the GUI also tries the adapter-beside native runtime location for claude first, which is app-bundle-specific and not mirrored here. `workspace checkout --mode commit` disables `commit.gpgsign` for its commit (git runs with stdin `/dev/null`, so a signing pinentry prompt would hang the command until interrupted — the git lanes deliberately run without a deadline): the commit is UNSIGNED even for a `commit.gpgsign=true` user, unlike the GUI's commit lane — a signature-based audit must not treat CLI-made commits as verified. The busy cases surface with STABLE error codes, so scripts can match a code instead of message text: `rewind_busy`, `checkout_busy`, `diff_busy`, `undo_busy` (any `{action}_busy`: another pinvou process is mutating the session/root, exit 1) and `{action}_lock` when the lock file itself cannot be taken. `providers --wire-api` also accepts the app parser's `openai_compatible`/`chat` aliases. `providers add --agent claude` requires a `--model-slot SLOT=MODEL` pair for every slot in the app's published `CLAUDE_MODEL_SLOTS` list: `ProviderManager::save` treats every one as mandatory, because a missing slot makes Claude Code's sub-agent and helper calls fall back to official models (official traffic). The pre-check imports the app's published slot list (`features::codex_acp::CLAUDE_MODEL_SLOTS`, widened from `pub(crate)` for exactly this use), so it fails with a readable message naming the live slot ids instead of the store's untranslated "<slot> is a required field", and cannot drift when the app adds a slot. `code permissions`/`code respond` refuse before any session-store access (the refusal is unconditional, so the store boot's retention sweep never runs for them). `login` announces the authorization URL and device code on stderr the moment the vendor CLI prints them rather than after it exits: the child deliberately stays alive until the user opens the link (kimi's device-code flow cannot complete otherwise), so a link published at exit is published too late; the rest of the transcript is still echoed once, redacted, at the end, and a timeout keeps the captured link. Interactive ACP turns and the pending-permission flow are desktop-process-bound. |
| `pinvou files` | `ingest <PATH> [--output PATH]` | File → markdown extraction (pdf/office/email/archive/text), the GUI attachment pipeline. A relative `<PATH>` is resolved against the cwd first (the GUI's upload policy rejects a relative path outright, which is useless from a shell); the resolved path still has to be an existing regular file under `$HOME` and outside the credential directories. The policy errors and the `warning` chips are GUI i18n copy (Chinese) and are translated to English here, including the `$HOME` refusal, which explains the confinement rather than restating it. `--output PATH` follows the family export contract (same as `sessions export`): an existing destination is refused, the file is created exclusive at 0600 (unix), and a failed write cleans up the partial file it created. |
| `pinvou voice` | `transcribe <audio>`, `postprocess --mode ... (--text S\|--text-file F) [--draft S\|--draft-file F]`, `asr-status`, `asr-install [--yes]` | Transcription of an audio file up to 4 MiB (GUI `recording_too_long` bound); recording itself is GUI-bound. `--draft`/`--draft-file` carry what the GUI sends as `draft_text` (the input-box text), assembled into the same `DRAFT_TEXT` block the edit prompt's rule 10 promises, and `--mode edit` — whose whole job is rewriting that draft — now REFUSES without one (exit 2) instead of sending the model a prompt with nothing to edit (an empty or all-whitespace `--draft-file` counts as none; `--mode dictation`/`--mode task` silently drop an empty draft instead). Every postprocess result carries `omitted_stages` (`deterministic-rule-corrections`, `shrink-and-protected-term-validation`) in JSON and a trailing `Note:` line in human output: those are the GUI's client-side pipeline stages (`applyVoiceDeterministicCorrections` and `validateVoicePostprocessOutput`) that the CLI deliberately does not run, because the GUI writes its result straight into the user's input box unseen while the CLI prints it for a human to read. `asr-install [--yes]` is Linux-only and system-touching (it can install ffmpeg via the OS package manager), so it requires `--yes` like `deps install`; the model download is verified by sha256, and the command now takes a cross-process lock before any probe or download — a second concurrent run is refused with `asr_install_busy` rather than interleaving writes into the same `.part` inode (the GUI's equivalent guard is a process-local flag another process cannot see). `transcribe` reports `ffmpeg_missing` when only ffmpeg is absent, except for `.wav` inputs: the native lane feeds those the raw wav (GUI parity), so only a warning is printed. Postprocess reports `truncated: true` on the Anthropic preset when the response carries `stop_reason: max_tokens` — the GUI's detection, mirrored here (raise the token budget or retry if an answer still looks cut off). The postprocess request bodies carry the GUI's wire exactly: no `temperature` field (a hard-coded 0 400s on gateways that pin sampling server-side) and the mandatory `x-opencode-session` header on OpenCode gateway routes. Two disclosed deviations remain on this lane: the shared-bridge fallback is a hand-rolled clone that cannot run the pool's `finalize_runtime_bridge` step (vLLM served-name correction + operator-route fact adoption), so on a single-model vLLM-class route whose configured name is not the served name the GUI sends the corrected name and the CLI sends the configured one; and the reasoning-control fallback covers the qwen and deepseek model-name arms but not the GUI's URL sniffing (the dialect module is crate-private there). On macOS, `asr-status` reports the host Speech runtime (`ready: true`), but `transcribe` uses the external ASR CLI lane — the JSON adds `cli_transcribe_ready` for what the CLI itself can do. On Windows, the CLI probes the real engine/ffmpeg state instead of trusting the GUI's MSI-bundled-runtime branch, so its `asr-status` can disagree with the GUI's and reports `installable: false` plus `gui_install_only: true`. The note there now names the remediation that is actually reachable from this process: point `PINVOU3_ASR_CMD` at the MSI's bundled `pinvou-asr.exe` (or copy the engine and model into AsrDir). The old "repair or reinstall the desktop app" advice could never flip those flags — the MSI installs the engine and model beside the desktop executable, and the CLI only ever looks under `$PINVOU3_HOME/asr`. Making the CLI discover the MSI copy by itself is an app-side change, not a CLI one: those paths resolve through `pub(crate)` `platform::os::windows` helpers that this crate cannot call. |
| `pinvou deps` | `check`, `install <NAME...> --yes` | External dependency detection/installation (apt/Homebrew/bundled). `install` streams the installer's progress hook to stderr (the same hook the GUI turns into `deps:install_progress` events), so a multi-minute `brew`/`pkexec` run is not indistinguishable from a hang; stdout stays the single result line. Its JSON field is `requested`, not `installed`: the value is the caller's argv, and the adapters do not report back which packages the package manager actually placed on disk (Homebrew installs per package and joins the failures; apt installs the batch in one `pkexec` call), so naming it `installed` would read as a verified outcome it is not — `deps check` is the lane that answers what is installed now. On Windows, an `install` of an already-present dependency (LibreOffice) is a lib-owned no-op (`Ok(())`) that the CLI renders exactly like a real install — the same "Requested: …" line and "installer reported success" note — so the two are indistinguishable from the output alone; verify with `deps check`, which is the lane that answers what is installed now. `deps check` itself also diverges from the GUI's settings dialog on Windows in one named way: the GUI's Windows adapter drops the `voice_asr` row and adds the two model-download rows, while the CLI renders the shared lib list verbatim — `deps.rs` documents why (the shared list is what the CLI can verify without the app's Windows-only download lanes). The installer child itself has no deadline (GUI parity — a `pkexec`/`brew` run takes as long as it takes), and it runs as a foreground child of the CLI: a terminal-wide interrupt (Ctrl-C) reaches it, while a kill addressed to the CLI pid alone leaves it running to completion, reparented. |
| `pinvou feedback` | `submit --type issue\|suggestion --title T --body-file F [--attach PATH...]` | Writes one receipt under `feedback/receipts/` (0600 on unix, carrying the submitted request) and prints the GitHub issues URL (GUI opens the browser). Nothing is left under `feedback/pending/`, which means "failed to upload". The JSON carries an `uploaded` boolean — the field scripts should branch on, since the community edition never uploads and a human-readable `status` cannot be parsed for that — plus an `attachments` list echoing the registered paths. `--attach` registers a path only (nothing is read, nothing is uploaded), but the path lands in that permanent receipt, so the CLI refuses a path crossing a credential component — and the same refusal now covers `--body-file`, whose content is embedded in the same receipt — (`artifact_crosses`-style `check_sensitive_path`) — a one-sided guard: the GUI only limits its own picker (image/video extensions `png/jpg/jpeg/gif/webp/mp4/mov/webm`, at most 5 attachments), and its Rust submit lane applies no path or extension policy, so the CLI's credential-path rule is stricter than the surface it mirrors, not a mirror of one. The CLI also does not restrict extensions or count the way the GUI picker does. |
| `pinvou monitor` | `status`, `snapshot` | One-shot model/GPU/vLLM sample instead of the live dashboard. Both subcommands emit one key set regardless of whether a model is configured. The model lookup loads settings through the persisting `UserPrefs::load` (the same disclosed side effect the models/settings row names): a nominally read-only `monitor status` can normalize `settings.json` or migrate a plaintext key into the OS keyring. `snapshot` reports `self_perf` and `app.session_uptime_secs` as not-applicable (listed in `not_applicable_headless`): they accumulate over the desktop app's lifetime and a one-shot process cannot sample them. |
| `pinvou artifacts` | `list [--session ID]`, `read`, `write` | Cross-session deliverables index + markdown-safe artifact read/write. Reads are capped at 10 MiB (the GUI's edit cap — a larger artifact the GUI displays is CLI-unreadable) and the list scan skips per-record files over 32 MiB, reporting them as `skipped_sessions` in the JSON instead of silently shortening the index; a sessions root that cannot be listed AT ALL (permissions, `sessions` shadowed by a file) fails the command rather than rendering as a clean empty index. `read`/`write` apply the credential-component half of the GUI's path policy: a path crossing a credential component is refused with `artifact_crosses_sensitive_component`. `write` overwrites an existing md deliverable WITHOUT `--yes` on purpose — it is the GUI editor's save semantics (md-only, existing-file-only, 10 MiB cap) — the one family mutation that silently replaces prior content, so scripts wanting a guard should `read` first. |

> Note: `feedback` human output renders `Attachment:` cells through the same control-character collapse as every other human cell; JSON stays verbatim.

> Note: the human `Warning:` row collapses control characters like every other human cell; the JSON `warnings` array keeps the converter's verbatim tail.

## Deliberately absent (GUI-bound)

Detach/tear-off windows, desktop pet, the embedded browser and its shared
control, artifact visual previews/design inspector, chat cards and streaming
rendering, voice recording UI and the global shortcut, drag-drop/clipboard
capture, the live theme/language switching and notification surfaces (their
`settings set` prefs persist, as do the pet and voice-shortcut prefs:
`settings set pet.enabled`, `settings set voice_shortcut_enabled`), QR image rendering
(URLs are printed instead), the WebUI remote-control pairing, the updater
(stubbed in the community build), the local vLLM bootstrap wizard, and the
super-permission pkexec toggle.

## Known limitations

- Connector CLI installs (`connectors ensure-cli`) are serialized CLI×CLI
  through the shared `locks/connector-install.lock`, but a GUI-triggered
  install of the same connector takes only the desktop's in-process mutex —
  GUI×CLI interleaving is NOT serialized. The outcome stays correct (both
  payloads are SHA-256-pinned identical bytes installed by atomic rename;
  realistic worst cases are a duplicated download and a transient
  spawn-ENOENT while the other surface migrates a legacy layout), but the
  residual is real until the GUI takes the same file lock. The voice
  `asr-install` residual is of the same class and disclosed on the command.
- CLI consent/scope writes to `disabled_bundles.json` (`connectors
  enable`/`disable`, `plugins enable`/`disable`/`project-skills`, and the
  DenyAll sync after `plugins` installs/imports and `connectors
  apply-skills`/`ima connect`) serialize only in-process, like the desktop
  app's own writers; a CLI write racing a live desktop-app write to that file
  is last-writer-wins until the cross-process lock (#517) lands. The same
  holds for the `bundles.json` store mirror `connectors status` connects and
  disconnects write through the app's `BundleStore` (`bundle_store_on_connected`
  / `bundle_store_on_disconnected`): its `BUNDLES_FILE_LOCK` is an in-process
  mutex only, so that file has the same last-writer-wins race between
  surfaces – it is disclosed here for the same reason `disabled_bundles.json`
  is.
- The vendor-CLI spawn/drive plumbing (~16 spawn sites across ~10
  orchestration loops: connectors `ensure`/`connect`/drain/tar, voice ASR and
  model download, code `login`/probes) each carry their own drain/cap/kill
  plumbing; consolidating them behind a shared `run_vendor_cli` helper is a
  tracked follow-up, not done in this PR.
- The featureless build compiles again — lib and tests alike: `cargo check
  --workspace --no-default-features --locked` (and `--all-targets`) is clean
  since the family modules used to be `cfg`-guarded and are not anymore (the
  unguard also removed `knowledge.rs`'s unguarded `anyhow` — the reason
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
  `sessions` and `scheduled` families, the `personas` family's
  session-touching lanes, and `projects move`/`rebind` — the store-rewriting
  half of that family; its `list`/`create`/`update`/`delete` open only the
  project store — boots the shared session store and runs its retention (see
  the family rows above). The same
  sweep also runs inside every command that boots the windowless product
  host — `voice postprocess`, `memory organize` — because the host's
  session-store boot is shared. `monitor status|snapshot` and the
  `knowledge remote *` reads do NOT: they run on a bare async host with no
  session-store boot, so no retention sweep can fire from a status command.
  The sweep reports on and evicts from BOTH
  budgets in one pass: unpinned GUI chat sessions above the 50-session chat
  cap and unpinned headless `agent run` sessions (`agentic_` ids) above their
  own 50-session cap. Separating the budgets means headless runs no longer
  CONSUME the chat budget (the point of #504's carve-out), but it does not
  make a headless save blind to the chat bucket: a `pinvou agent run`
  prepare-time save runs the same sweep and can evict the oldest unpinned
  GUI chat sessions when the chat bucket is over cap. The stderr warning
  covers the headless budget only — it carries the evicted headless-run
  count (no session ids); a chat-budget eviction by the same sweep is the
  chat budget's own enforcement and is deliberately not warned from the
  headless side — and it prints even
  when the run itself later fails, because the evicting save precedes the
  fault. The local `knowledge` lanes never boot it: `collections delete` does its job without the
  (provably empty) CLI-side mount sweep, and `knowledge mounts`/`mount`/
  `unmount` refuse before opening the store — precisely so an impossible
  command cannot evict sessions on its way to saying so. The remote/host
  lanes are in the same safe class: `remote probe`, `remote connections`,
  `remote collections`, `remote search`, and `host status` run on the bare
  async host (`run_bare_host` — no Tauri context, no session-store boot),
  so none of them can run the retention sweep.
- Two inherited upstream gaps this CLI mirrors byte-for-byte: the
  credential-component path refusal (`files ingest`, `artifacts read`/`write`,
  `feedback submit --attach/--body-file`, `agent run --attach`) compares path
  components byte-exactly on unix,
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
