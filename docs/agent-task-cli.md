# Headless agentic single-task CLI (`pinvou agent run`)

`pinvou agent run` executes **one product-equivalent agentic turn** inside a windowed Tauri host: the same send path as GUI chat (Yolo mode, product tool allowlist, real Bash / File (write/edit) / Git / Web execution), with the session execution root bound to a caller-provided task directory. It targets external benchmark harnesses (Terminal-Bench/Harbor) that install Pinvou Agent into a task container to perform real terminal work. It does not go through the eval backend (`HeadlessAgentBackend`) and never touches any eval tool policy — the read-only isolation semantics of GAIA-style evaluation are unaffected.

Relationship with `pinvou benchmark ...`: the benchmark subcommand family runs batch evaluation over fixed datasets, with a read-only tool policy and a privacy output pipeline; `agent run` performs full agentic execution of a single task instruction and returns its output (assistant text, tool events, usage) directly to the caller. Both share the same windowless host bootstrap.

## Build and run

```bash
cargo build --manifest-path pinvou-cli/Cargo.toml --bin pinvou
PINVOU3_HOME=/path/to/sandbox pinvou agent run \
    --prompt-file task.txt \
    --workspace /path/to/task-dir \
    --timeout-secs 600 \
    --output json
```

- `--prompt-file` (required): task instruction file; its content enters the product send path verbatim, without any eval envelope.
- `--workspace` (optional): task working directory. When provided, the directory must already exist; it is canonicalized to an absolute path and becomes the engine cwd and the shell execution directory (same mechanism as the project binding of native code sessions). The `ExecutionRootResolver` applies to this run regardless of the session: a caller-provided `session_id` also runs in the given directory, but only its run scope — its persisted binding is untouched. For a fresh session the workspace is additionally persisted as the session's durable workspace binding, so reopening the session in the desktop app keeps working in that directory. When omitted, a session-private directory is used (the same isolated scratch as eval sessions).
- `--timeout-secs` (default 600, capped at 604800 = 7 days): task timeout. The deadline covers the whole task, including session prepare/submit — a hang in either phase still produces a `timeout` report instead of hanging forever. On timeout the turn is cancelled first; if it has not settled within the 30s settle window, the report is emitted immediately and the process never waits unboundedly — in the default keep mode. Under the falsy `KEEP_SESSION` values below, the one-shot delete still waits for the engine's turn gate before deleting, so a turn that ignores cancel can withhold the report up to the engine's per-turn wall clock; batch harnesses should prefer the default keep mode and rely on retention instead. The cap is enforced at parse time — an unbounded `u64` would overflow the internal `Instant + Duration` and panic; the library clamps to the same cap (`MAX_TIMEOUT_SECS`) for direct API callers.
- `--output human|json`: `human` prints a session/status line plus the assistant text; `json` prints the full report as a single-line JSON.
- `PINVOU3_AGENT_TASK_KEEP_SESSION` (default: keep): the run's session persists into the shared session store (`$PINVOU3_HOME/sessions/<session_id>`, GUI-visible, subject to a 50-session retention cap of its own; pinned sessions are exempt from retention). Headless runs are counted and evicted against a **separate budget from the desktop app's chat sessions**, so no number of `agent run` invocations can evict a conversation you started in the GUI. The transcript/artifacts stay available for later continuation through the request's `session_id` (a library surface; the one-shot CLI does not expose a resume flag). When the prepare-time save evicts sessions at the headless cap, a stderr warning reports how many previous headless runs were evicted (chat-budget evictions by the same sweep are not counted in it — see the `session_id` budget note below). One gap: evictions committed by a boot-time sweep fire before the run's warning observer arms, so a home that is already over the cap when the process starts sees them only as vanished sessions — the run's own stderr warning does not cover them. `PINVOU3_AGENT_TASK_KEEP_SESSION=0|false|no|off` restores the legacy one-shot cleanup (the run's own session is removed after the report; a save-time eviction at the cap still happens and is still warned about). The falsy comparison is ASCII case-insensitive but does not trim whitespace — a quoted `="0 "` counts as keep — so avoid inline comments or stray spaces in harness scripts. Under the falsy values a FRESH run's attachment marked `remove_after_ingest` is refused up front: the run would delete the caller's source after ingest and then delete its own session — and with it the staged copy — leaving neither (a caller-provided session is never deleted, so the refusal does not apply to it). This is a flip from the legacy truthy parse (where `1|true|yes|on` meant keep and any other value — including the old unset default — meant delete): the enumerated set is now the delete side, so a harness exporting a value outside both lists (e.g. `=cleanup`) gets keep where the legacy parse deleted. A fresh run whose turn never started cleans up its own session — no zero-message stubs — unless its record cannot be read at all, which keeps it (deleting on unknown state is the unsafe direction). "Never started" is decided on the durable record: if the engine admitted the user message before the fault surfaced, the transcript stays inspectable (deleted anyway under the falsy values, matching the legacy one-shot contract); the stub cleanup also re-checks emptiness under the engine's turn gate, so a transcript admitted between the classification and the delete is kept as a started transcript.

### Library parity fields (not exposed as CLI flags)

`AgenticTaskRequest` additionally carries optional fields the one-shot CLI does not expose (it passes them as unset — a CLI run behaves exactly as documented above). They exist for the stacked CLI surfaces and library callers:

- `session_id`: continue an existing chat session instead of creating a fresh one. The session must exist in the store and be an ordinary chat session (scheduled-run sessions and native code sessions are refused with `agent_session_not_chat`, unknown ids with `agent_session_not_found`, unloadable records with `agent_session_unreadable`). A caller-provided session is never auto-deleted by the run, regardless of `KEEP_SESSION`. Budget note: only sessions whose id carries the durable `agentic_` prefix count against the separate headless retention budget — a caller-provided id without that prefix counts against the desktop chat budget instead, so batch runs that keep reusing plain chat ids can evict GUI conversations at the chat cap.
- `mode`: `agent` (default, full Yolo turn) or `plan` (the read-only Plan turn the GUI produces; persisted through the GUI's per-session lane so the session reopens in Plan — a persistence failure fails the run before submit).
- `model_id`: pin the session's model for this run, validated against the configured model list (`agent_model_not_found` for unknown ids); fresh sessions bind through the eval selection route, existing sessions through the GUI per-session model switch.
- `attachments`: files staged into the session's `attachments/` directory, ingested through the GUI attachment pipeline, and rendered with the same attachment text (the aggregate byte cap can surface as either of two codes depending on where it is caught: the pre-copy re-stat of the staging directory reports the breach as `agent_attachment_too_large: attachments total … at staging time`, while the post-copy landed-size re-check — which closes the tiny-at-stat, large-at-stream gap — reports `agent_attachment_total_too_large: staged attachments exceed …`; other validation errors are `agent_attachment_not_found` / `agent_attachment_too_many`; the staging-and-ingest pipeline can additionally fail with `agent_attachment_invalid_name` / `agent_attachment_stage_failed` / `agent_attachment_ingest_failed`); `remove_after_ingest` removes the caller-side source after the turn is admitted (refused for fresh sessions under falsy `KEEP_SESSION` — error string `agent_attachment_ingest_would_lose_the_file`, see above; the refusal scopes its promise to the run: a caller-provided session is never auto-deleted *by the run*, but it remains subject to the chat-budget retention sweep like any chat session, so a `remove_after_ingest` caller whose source was already removed should pin the session to keep the staged copy alive).

See the `AgenticTaskRequest` struct docs in `agentic_task.rs` for the exact error strings and limit constants (`MAX_ATTACHMENTS`, per-file and aggregate byte caps).

Prerequisites: the `settings.json` of the sandbox `PINVOU3_HOME` needs an active model (any OpenAI-compatible endpoint works, `preset = "openai_compatible"`); `PINVOU3_ALLOW_SHELL=1` pins shell authorization without relying on prefs. There is no per-turn tool-call cap; runaway protection stays with the engine's per-turn bounds — a default 200-step budget and a 1-hour per-turn wall clock (foundation defaults), plus this command's `--timeout-secs` watchdog that cancels the turn and still emits a report. If your scripts still export `PINVOU3_MAX_TOOL_CALLS`, delete the export: the knob is no longer read, and a one-line stderr warning reminds you once per process when it is present.

## Shared-home cross-process consistency (known limitation)

Headless runs and the desktop app share one `PINVOU3_HOME` session store, and this layer serializes the session sidecar files (`_pinned_sessions.json`, `_hidden_sessions.json`, `_session_models.json`, `_session_mode_states.json`, multi-agent flags) with **per-process io mutexes only**: each mutation is a whole-file load→mutate→atomic-rename, and there is no cross-process lock (flock) on these files yet. A pin can therefore still be lost in a two-process race even though both writers reported success:

1. Process A (say the GUI) loads the pin sidecar, adds pin P, and is preempted before its rename.
2. Process B (a headless `agent run`, including its prepare-time save and retention sweep) completes its own sidecar RMW in between.
3. Process A resumes and renames its stale whole-file snapshot — B's already-confirmed write is silently reverted even though B's caller already saw `Ok(())`.
4. The next retention sweep consults the durable file, no longer sees P, and can evict the session the user believes is pinned keep-forever. The transcript loss is unrecoverable.

The same whole-file RMW class covers record create/delete races in the store itself (`features/sessions/sidecars.rs` documents the residual risk where the in-code contract lives). Mitigations until a cross-process lock lands (#623):

- Do not pin/unpin (or toggle hidden/model/mode) from the GUI while a headless `agent run` is in flight against the same `PINVOU3_HOME`, and vice versa — the race window is one sidecar RMW, short but real.
- After running batches alongside GUI session management, re-verify the pins you care about (re-pin from the GUI): the durable sidecar file, not the earlier success toast, is what the next sweep reads.
- Unattended harnesses that don't need GUI interplay can point `PINVOU3_HOME` at a dedicated home, so the sweep's eviction surface stays limited to headless-created sessions and no GUI pin is ever in the race.

## Output contract

JSON report fields: `session_id`, `status` (`Completed`/`Failed`/`timeout`/`error` or another engine status), `timed_out`, `completed_after_deadline` (timeout race marker: a turn that finished naturally after the deadline but before the cancel took effect keeps the engine's real `status` instead of being rewritten to `timeout`, letting graders distinguish "finished, but past the line" from "cancelled"; absent in older reports, defaults to false), `assistant_text` (last turn's assistant text), `tool_events` (tool names and success flags only, never arguments/results), `usage` (input/output/cache hit/cache miss/cache write/reasoning tokens and context window), `error` (host-side root causes — populated on `status=error` reports and on `timeout` reports whose session setup did not finish in time, whose cancel did not settle, or whose final turn-result read failed after the deadline fired — as well as the engine's own failure message for failed turns). Tool events deliberately carry no payloads, so reports can safely be persisted under `/logs` for harness usage aggregation. While the turn runs, a liveness heartbeat is written to stderr every 10 seconds; stdout stays reserved for the final report.

Exit codes: whenever a report is produced (including `timeout`/`error` statuses) the process exits 0 — in-turn failures are settled by the harness grader from the report, while a non-zero exit would make timed-out tasks count as exceptions instead of zero-reward runs and skew the mean. Non-zero exit codes are reserved for host-level failures (unreadable `--prompt-file`, missing or non-directory `--workspace`, unusable backend, ...); argument errors (missing `--prompt-file`, out-of-range `--timeout-secs`, ...) exit 2.

## Running inside a container (Terminal-Bench shape)

The binary is a dynamically linked Tauri program; task containers need the GTK3/WebKit2GTK 4.1 runtimes and xvfb (the windowed event loop still requires an X server without a display):

```dockerfile
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y \
    libwebkit2gtk-4.1-0 libgtk-3-0 libjavascriptcoregtk-4.1-0 \
    libpipewire-0.3-0 libgbm1 libegl1 \
    libayatana-appindicator3-1 xvfb xauth ca-certificates procps curl
```

Run as `xvfb-run -a pinvou agent run ...`. The model endpoint must be reachable from inside the container (docker bridge gateway, e.g. `http://172.17.0.1:13000/v1`). Mind glibc forward compatibility: a binary compiled on an older distro baseline (e.g. Debian bookworm) runs on newer baselines, not the other way around.

## Implementation location

- `pinvou3-app/src-tauri/src/features/assistant/product_runtime/agentic_task.rs`: host bootstrap, execution root binding, turn driving, and the timeout watchdog.
- `pinvou-cli/crates/pinvou-product-backend`: the public `run_agentic_task` launcher.
- `pinvou-cli/crates/cli/src/lib.rs`: `agent run` argument parsing and output rendering.
