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
- `--workspace` (optional): task working directory. When provided, the directory must already exist; it is canonicalized to an absolute path and becomes the engine cwd and the shell execution directory (same mechanism as the project binding of native code sessions; the `ExecutionRootResolver` only applies to the session generated for this run). When omitted, a session-private directory is used (the same isolated scratch as eval sessions).
- `--timeout-secs` (default 600, capped at 604800 = 7 days): task timeout. The deadline covers the whole task, including session prepare/submit — a hang in either phase still produces a `timeout` report instead of hanging forever. On timeout the turn is cancelled first; if it has not settled within the 30s settle window, the report is emitted immediately and the process never waits unboundedly. The cap is enforced at parse time — an unbounded `u64` would overflow the internal `Instant + Duration` and panic; the library clamps to the same cap (`MAX_TIMEOUT_SECS`) for direct API callers.
- `--output human|json`: `human` prints a session/status line plus the assistant text; `json` prints the full report as a single-line JSON.
- `PINVOU3_AGENT_TASK_KEEP_SESSION=1`: skip the post-turn session cleanup so the session artifacts (full timeline/transcript) stay under `$PINVOU3_HOME/sessions/<session_id>` for harness-side debugging. By default the temporary session is deleted once the report has been produced.

Prerequisites: the `settings.json` of the sandbox `PINVOU3_HOME` needs an active model (any OpenAI-compatible endpoint works, `preset = "openai_compatible"`); `PINVOU3_ALLOW_SHELL=1` pins shell authorization without relying on prefs. benchmark-hooks builds pin the per-turn tool-call cap to 8 (the GAIA runaway guard), so long-horizon agentic tasks must raise it explicitly, e.g. `PINVOU3_MAX_TOOL_CALLS=512`. The knob is process-wide — it also applies to GAIA runs in the same process, so scope the environment to one run kind per container. Invalid values are reported on stderr and fall back to the default cap of 8.

## Output contract

JSON report fields: `session_id`, `status` (`Completed`/`Failed`/`timeout`/`error` or another engine status), `timed_out`, `completed_after_deadline` (timeout race marker: a turn that finished naturally after the deadline but before the cancel took effect keeps the engine's real `status` instead of being rewritten to `timeout`, letting graders distinguish "finished, but past the line" from "cancelled"; absent in older reports, defaults to false), `assistant_text` (last turn's assistant text), `tool_events` (tool names and success flags only, never arguments/results), `usage` (input/output/cache hit/cache miss/cache write/reasoning tokens and context window), `error` (host-side root causes such as a failed turn-result read — non-empty when `status=error` — as well as the engine's own failure message for failed turns). Tool events deliberately carry no payloads, so reports can safely be persisted under `/logs` for harness usage aggregation. While the turn runs, a liveness heartbeat is written to stderr every 10 seconds; stdout stays reserved for the final report.

Exit codes: whenever a report is produced (including `timeout`/`error` statuses) the process exits 0 — in-turn failures are settled by the harness grader from the report, while a non-zero exit would make timed-out tasks count as exceptions instead of zero-reward runs and skew the mean. Non-zero exit codes are reserved for host-level failures (unreadable `--prompt-file`, missing or non-directory `--workspace`, unusable backend, ...); argument errors (missing `--prompt-file`, out-of-range `--timeout-secs`, ...) exit 2.

## Running inside a container (Terminal-Bench shape)

The binary is a dynamically linked Tauri program; task containers need the GTK3/WebKit2GTK 4.1 runtimes and xvfb (the windowed event loop still requires an X server without a display):

```dockerfile
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y \
    libwebkit2gtk-4.1-0 libgtk-3-0 libjavascriptcoregtk-4.1-0 \
    libayatana-appindicator3-1 xvfb xauth ca-certificates procps curl
```

Run as `xvfb-run -a pinvou agent run ...`. The model endpoint must be reachable from inside the container (docker bridge gateway, e.g. `http://172.17.0.1:13000/v1`). Mind glibc forward compatibility: a binary compiled on an older distro baseline (e.g. Debian bookworm) runs on newer baselines, not the other way around.

## Implementation location

- `pinvou3-app/src-tauri/src/features/assistant/product_runtime/agentic_task.rs`: host bootstrap, execution root binding, turn driving, and the timeout watchdog.
- `pinvou-cli/crates/pinvou-product-backend`: the public `run_agentic_task` launcher.
- `pinvou-cli/crates/cli/src/lib.rs`: `agent run` argument parsing and output rendering.
