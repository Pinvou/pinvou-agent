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

On Windows, `--trusted-approval-wrapper <absolute-pwsh.exe>` is an explicit
operator trust decision for the one PowerShell executable that app-server may
use to wrap scenario C's fixed inner command. The original absolute path is
opened and write/delete-locked first, then its final canonical path, regular-file
type, and file identity are derived from that same handle before app-server
launch; invalid paths fail generically before raw capture. If omitted, only the
protected Program Files auto-candidate is eligible. The selected path is never
copied into sanitized evidence or summary artifacts.

Before opening the raw capture, the runner invokes the selected executable with
`--version` and requires the exact pinned output `codex-cli 0.139.0`. A mismatch,
malformed output, nonzero exit, or timeout fails before app-server evidence is
collected. Version preflight and app-server execution share one run-global
deadline and the same contained-process cleanup: a timeout or malformed result
terminates the complete spawned tree and bounds pipe-reader shutdown. On Windows
children are created suspended, assigned to a Job Object, and only then resumed;
on Unix they are placed in a dedicated process group before exec.

On Windows the default executable lookup searches PATH specifically for
`codex.cmd`; it does not fall through to an extensionless POSIX shim or a later
`codex.exe`. The selected script is canonicalized, required to be a regular
`.cmd` file, and rejected if its command path contains cmd.exe metacharacters or
line breaks. It is invoked through the absolute trusted System32 `cmd.exe` with
fixed `/d /s /c` arguments and fail-closed quoting. Explicit regular `.cmd`
scripts use the same path, while explicit `.exe` test overrides remain direct.

Scenarios A, B, and D request exactly one fixed, side-effect-free command built
from a deterministic, non-sensitive restricted ASCII corpus and constants. A
emits 36 one-KiB chunks at one-second cadence, B emits 56 one-KiB chunks at
50 ms cadence, and D emits 64 256-byte chunks at 250 ms cadence so interruption
can occur after a real backlog. Commands use the canonical OS-protected Windows
PowerShell path derived from `GetSystemDirectoryW` on Windows, or absolute
`/bin/sh` on Unix, and never consult PATH or interpolate caller paths. Windows
scripts are UTF-16LE/Base64 `-EncodedCommand` payloads, so the literal prompt
contains no nested JSON-style quote escapes. Their prompts forbid other tools
and prose before completion. Gates still use only real, correlated R1 timestamps
and content, including official agent message and command-execution output
deltas.

Scenario C uses `on-request` approval and a read-only sandbox, then requests one
fixed platform-specific command that writes `.codex-s2-approval-marker` relative
to the isolated working directory. The command uses absolute `/bin/sh` or the
absolute `cmd.exe` returned from `GetSystemDirectoryW`, and the runner accepts only an
`item/commandExecution/requestApproval` whose command exactly matches it, whose
thread/turn IDs match scenario C, and whose canonical working directory remains
exactly equal to the scenario workspace. On Windows the runner also accepts the
one observed app-server PowerShell wrapper form. Without an explicit option,
automatic trust is limited to canonical `pwsh.exe` below the WinAPI
`FOLDERID_ProgramFiles` PowerShell root. With
`--trusted-approval-wrapper`, the operator instead trusts exactly that one
canonical executable, even outside Program Files. The runner retains a read
handle that excludes write/delete sharing for the whole run, records its volume
and file ID, and revalidates path identity immediately before approval. In both
modes, escaping, argument shape, and the sole `commandActions` entry must exactly
preserve the allowlisted inner command. Other shells, child directories, and all
wrapper variations are rejected. No caller-controlled path is interpolated into
the command. Immediately before accepting, the runner atomically rejects any
preexisting marker object. After completion it reads the exact marker bytes from
one no-follow, identity-stable handle under the scenario deadline; the marker is
left inside the runner-owned temporary workspace rather than removed by path.
Exactly one such approval is required.
Every other server request, command, or target is rejected and invalidates the
run. Scenario D sends `turn/interrupt` only after a real backlog of at least
eight nonempty deltas and 2 KiB, and reports request-write-to-response and
request-write-to-interrupted-terminal latency.

Production gates require scenario A to span at least 30 seconds with at least
eight nonempty deltas and 2 KiB, and scenario B to contain at least 32 nonempty
deltas and 32 KiB. Sliding one-second peak rates, event sizes, and 50 ms merge
windows relative to B's first event are computed from scenario B alone. These
thresholds cannot be lowered through the CLI. Each scenario uses one deadline
covering thread start, turn start, and streaming; inbound traffic is bounded,
and timeout/error cleanup terminates the spawned process tree and bounds reader
shutdown.
Auth, quota, malformed protocol, timeout, missing approval,
missing interrupt response, and terminal-state mismatches all produce sanitized
INVALID artifacts and a nonzero exit. Account identifiers are never copied into
evidence, reports, summaries, or stdout; they can exist only in the restricted
raw capture.

For deterministic no-quota tests, `--executable <path>` replaces only the Codex
executable while preserving the `app-server --stdio` arguments.
