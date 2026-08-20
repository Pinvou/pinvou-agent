# Codex app-server capture harness

This standalone developer tool records the line-delimited JSON transport used by
`codex app-server --stdio`. It is intentionally outside the `pinvou-cli`
workspace and does not run model turns in its tests.

## Capture modes

Build or run it with its own manifest:

```text
cargo run --manifest-path pinvou-cli/tools/codex-app-server-capture/Cargo.toml -- \
  proxy --output capture.jsonl

cargo run --manifest-path pinvou-cli/tools/codex-app-server-capture/Cargo.toml -- \
  replay --input client.jsonl --output capture.jsonl
```

Both modes resolve `codex` locally and launch `codex app-server --stdio`.
`--executable <path>` replaces only the executable, which permits deterministic
fake app-server tests while preserving the real arguments.

`proxy` is a transparent adaptive driver: client JSON objects arrive on its
stdin and unmodified server JSON objects leave on its stdout. This permits a
later runner to react to approval requests and issue interrupts. Diagnostics
never enter stdout. `replay` sends the JSON objects in a file and does not mirror
server output.

Each capture line is one JSON object:

```json
{"monotonic_ns":1000000,"channel":"client_to_server","line":"{\"id\":1,\"method\":\"initialize\"}"}
```

`channel` is `client_to_server`, `server_to_client`, or `stderr`.
`monotonic_ns` means nanoseconds since an unspecified same-boot monotonic epoch:
Windows QueryPerformanceCounter (QPC) or Linux `CLOCK_MONOTONIC`. It is useful
only for ordering and durations within a capture and has no wall-clock meaning.

Capture files contain raw, unredacted protocol traffic and may include prompts,
tool arguments, account metadata, or other sensitive values. Store and share
them accordingly. On Unix the harness creates them with owner-only `0600`
permissions (and tightens an existing output file before writing). On Windows
it immediately replaces inherited permissions with a protected DACL granting
full access only to the file owner, before any capture bytes are written. ACL
hardening failure aborts capture and removes the empty file.

The sanitized, zero-cost handshake fixture is
`tests/fixtures/zero-cost-handshake.jsonl`. It documents line-delimited frames,
an initialize response without `jsonrpc`, notification/response interleaving,
unknown notification noise, and separate stderr capture.

## Fail-closed S2 validation

`validate-s2 --input evidence.json [--output report.json]` consumes serialized
`S2Evidence` from the library. A report is valid only when all gates pass:

- F1: A/B/C completed, D interrupted, and auth/quota/protocol error counts are zero.
- F2: A/B contain sufficient R1 content and a first delta, C observed approval,
  and D observed both the interrupt response and interrupted terminal state.
- F3: real-content peak events/s and MB/s, an ordered non-empty event-size
  distribution, and valid 50 ms merge inputs are present.

Any missing or failed prerequisite makes the report `valid: false`, removes
`pass_percentiles`, sets `baseline_update_allowed: false`, and makes the CLI
exit unsuccessfully after writing the report.

## Executable S2 runner

`run-s2` drives the four real scenarios against the locally installed `codex`
app-server and then applies the same fail-closed validator:

```text
cargo run --manifest-path pinvou-cli/tools/codex-app-server-capture/Cargo.toml -- \
  run-s2 --output-dir /restricted/path/s2-run
```

The output directory receives `capture.jsonl` (raw and sensitive),
`evidence.json`, `validation-report.json`, `summary.txt`, and an isolated
`workspace/`. If `--output-dir` is omitted, a unique directory under the OS
temporary directory is used and printed after a successful run. Useful bounded
options are `--scenario-timeout-ms` (default 120000) and
`--global-timeout-ms` (default 600000). `--model` is optional and otherwise
leaves model/provider selection to the installed Codex configuration.

Scenario C creates one inert probe script inside the isolated workspace and
accepts only an `item/commandExecution/requestApproval` whose command exactly
matches that script, whose thread/turn IDs match scenario C, and whose working
directory remains inside the workspace. Exactly one such approval is required.
Every other server request, command, or target is rejected and invalidates the
run. Scenario D sends `turn/interrupt` only after a real backlog of at least
eight nonempty deltas and 2 KiB, and reports request-write-to-response and
request-write-to-interrupted-terminal latency.

Production gates require scenario A to span at least 30 seconds with at least
eight nonempty deltas and 2 KiB, and scenario B to contain at least 32 nonempty
deltas and 32 KiB. Peak rates, event sizes, and 50 ms merge inputs are computed
from scenario B alone. These thresholds cannot be lowered through the CLI.
Auth, quota, malformed protocol, timeout, missing approval,
missing interrupt response, and terminal-state mismatches all produce sanitized
INVALID artifacts and a nonzero exit. Account identifiers are never copied into
evidence, reports, summaries, or stdout; they can exist only in the restricted
raw capture.

For deterministic no-quota tests, `--executable <path>` replaces only the Codex
executable while preserving the `app-server --stdio` arguments.
