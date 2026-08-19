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
permissions (and tightens an existing output file before writing).

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
