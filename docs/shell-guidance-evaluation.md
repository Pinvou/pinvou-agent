# Shell guidance evaluation

This change keeps the `Bash` tool name, schema fields, permissions, execution
dispatcher, and background lifecycle compatible. Only model-facing guidance is
changed. Application instructions no longer prescribe Unix-only defaults.

## Integration review status

Independent source review accepted the scoped repairs. This does not establish
release readiness: [parent PR #443](https://github.com/Pinvou/pinvou-agent/pull/443)
must remain draft while its public-submodule gate is blocked.
`scripts/verify-public-submodule.sh` requires the official immutable
`pinvou-v0.9.5-r13` tag, whose commit is
`f853f8f1566c57e6be40d5439a222a932aa79ef5`; the candidate parent gitlink is
`0d409a97802179f1df9bcdbef185c1bfb5dc23e2`. These commits differ, so the candidate
does not satisfy that gate even though its focused source tests passed.

An authorized `Pinvou/CodeWhale` maintainer must integrate
[fork PR #42](https://github.com/Pinvou/CodeWhale/pull/42) and publish a new
immutable release tag containing the fix. The authenticated `zhuowp` identity
lacks push permission to the official repository. Once that release exists,
align the parent gitlink, verifier, fork guard, and release documentation with
the actual tag and SHA, then rerun the integration checks before marking the
parent ready. The current r13 tag must stay immutable; accepting a contributor
ref or guessing a future release tag would not resolve the dependency.

[Upstream PR #5900](https://github.com/Hmbown/CodeWhale/pull/5900) adapts the fix
to the newer model-visible lowercase `bash` surface, preserving its foreground,
timeout, and approval contracts. Its source repairs were approved in the
independent review; compilation checks are pending. The local model results
below measure the older fork surface, not that newer upstream integration.
Upstream acceptance also does not replace publication of the Pinvou release.

## Final scope

- CodeWhale: derive model-visible tool and command guidance from the existing
  execution dispatcher, with named guidance constants for each supported shell
  family and custom executable paths. Preserve the zsh expansion warning.
- Application: remove Unix-specific command defaults and instruct the model to
  follow the actual shell declared by the tool.
- Adopt the curl-reminder-removed variant: its focused HTTP ablation achieved
  19/20 correct commands, identical to the retained variant, with zero shell
  mismatch errors in either arm. The earlier four-task results below describe
  the historical candidate before that removal, not a new full evaluation of
  the final text.
- Keep tool names, permissions, shell detection, execution, output processing,
  and background-task behavior unchanged. Do not automatically translate model
  commands or add retries. Guidance reduces errors but cannot guarantee valid
  commands or correct API usage.

The upstream PowerShell invocation fix (#4593, merge `8b600db2`) is already an
ancestor of this fork baseline. This change complements that execution fix.
When updating to upstream `a58ef2d5` or later, merge its updated background-wait
description into `guidance::description()` rather than overwriting it: the newer
description documents blocking `wait` and nonblocking `wait=false`. Do not apply
those newer lifecycle claims to this older baseline without its implementation.

## Reproduce the focused model simulation

Live tests are opt-in and require an authorized model endpoint. The script sends
repository-owned instructions and synthetic tasks only. It never executes model
commands or writes credentials/provider addresses to its output.

1. Export the current tool fixture using the compiled CodeWhale test:

   ```powershell
   $env:SHELL_GUIDANCE_FIXTURE = 'D:\shell-eval\fixture.json'
   cargo test --manifest-path CodeWhale/Cargo.toml -p codewhale-tui --lib --locked export_shell_guidance_eval_fixture -- --ignored
   ```

2. Configure `SHELL_EVAL_BASE_URL`, `SHELL_EVAL_API_KEY`, and `SHELL_EVAL_MODEL`
   through the process environment. Do not put credentials in command history,
   tracked files, or evaluation artifacts.
3. Run `python scripts/eval-shell-guidance.py --fixture <fixture.json> --output
   <private-directory> --app-base <before-app-commit> --engine-base
   <before-engine-commit> --repeats 5`. Output must be outside the repository.
   `--arms after` supports verification of an updated candidate without
   regenerating an unchanged baseline.
   For a controlled tool-guidance ablation, pass `--before-fixture <other.json>`;
   both arms then use the same current application instructions. Add
   `--tasks http_preview json_fields` to reproduce the HTTP-only task selection.
   The fixtures must declare the same shell; inspect their diff to ensure that
   only the intended guidance differs. Arm names identify fixtures, not whether
   a reminder is present: `before` uses `--before-fixture`, `after` uses `--fixture`.
4. Review the generated commands before execution. Use a disposable workspace
   and a loopback HTTP fixture, preserve the detected shell's invocation rules,
   and check results as well as exit codes. No automatic command repair is part
   of this experiment.

The tasks cover response truncation, JSON field extraction, CSV summation, and
reading a filename containing spaces and Chinese characters. The baseline
description is read verbatim from Git. Before/after arms use the same environment,
model route, synthetic tasks, and tool schema except for the changed descriptions.
Both arms include the corresponding repository application instructions.

## Interpretation

This is a forced single-tool simulation, not a full desktop/Engine session.
It omits other tools, user memory, arbitrary third-party skills, session history,
and provider-specific Engine request assembly. It tests initial command generation
and execution, not autonomous recovery or task completion over multiple turns.
Small repeated samples are regression evidence, not an estimate of production
failure rates or a guarantee that a model cannot misuse PowerShell APIs.

The cross-platform unit matrix checks guidance for Windows PowerShell, pwsh,
cmd, sh, Bash, custom zsh, custom pwsh, and another custom shell. Actual execution
on one OS does not establish execution coverage on the others.

## Local run, 2026-09-05

- Application baseline: `5094357f6`; CodeWhale baseline: `f853f8f1566c57e6be40d5439a222a932aa79ef5`.
- Runtime: Windows PowerShell 5.1, native `curl.exe`, no Unix `head` assumption.
- Models: `deepseek-v4-pro` and `deepseek-v4-flash`; thinking disabled; maximum
  output 1,800 tokens; four tasks, five repetitions per model and arm.
- Initial generation: 80 calls. The first candidate exposed an additional
  Windows PowerShell `Invoke-WebRequest` compatibility gap. Guidance was updated
  to recommend `-UseBasicParsing`, then 40 candidate calls were regenerated.
- Every baseline HTTP-preview generation (10/10 across both models) used
  `curl ... | head -c 2000`. Final candidate generations used PowerShell text
  handling and compatible HTTP clients instead.

### Command execution results

| Model | Baseline correct commands | Final candidate correct commands | Final shell mismatch errors |
|---|---:|---:|---:|
| deepseek-v4-pro | 3/20 (15%) | 20/20 (100%) | 0/20 |
| deepseek-v4-flash | 2/20 (10%) | 19/20 (95%) | 0/20 |

Each denominator contains five commands for each of the four tasks. Baseline
successes were CSV calculations through the available Python executable.
Both models failed all five baseline HTTP-preview commands with the Unix-style
`curl ... | head` pattern. All ten final HTTP-preview commands returned exactly
the required prefix. The final candidate was evaluated separately after the
`-UseBasicParsing` clarification; initial candidate observations are not mixed
into the final candidate denominator.

The remaining Flash failure used `New-Object System.Net.WebClient` and assigned
its nonexistent `Timeout` property. It printed the correct weather values and
exited with code zero, but also produced `PropertyAssignmentException` and did
not establish the requested timeout. It is counted as a failure, not hidden by
the exit code. This is an API-usage error, not Bash/PowerShell syntax mixing.

The first execution harness imposed an artificial 35-second timeout. Thirteen
commands timed out during concurrent compilation/process startup. Those exact
commands were re-executed using the tool's normal 120-second default (or their
explicit tool timeout); no model regeneration or command editing was performed.
All infrastructure timeouts were resolved. These figures measure correctness of
the first generated command, not process-start latency or first-attempt timing.

The host used fresh `powershell.exe -NoLogo -NoProfile -NonInteractive` processes,
UTF-8 output setup, and the dispatcher's `-Command` versus BOM-encoded `.ps1`
selection rules. Execution was outside the Engine in a disposable workspace;
approval, sandbox, hooks, and tool-loop behavior were not exercised.

### Verification status

The subsequent Unix-guidance review expands Bash, POSIX sh, zsh, cmd, and fish
instructions and recognizes custom Bash/sh executable paths. The PowerShell
branch and shared tool description are unchanged, so the Windows model results
above still describe the same model-visible guidance. These measurements do
not establish model accuracy on Unix shells; that requires a separate evaluation.

- Architecture guard: passed.
- `scripts/fork-guard.sh --fast`: passed, including the new guidance fingerprints.
- BrowserCore and preview-instruction regression: 11/11 passed.
- Isolated production guidance module tests after the Unix review: 2/2 passed,
  covering the interpreter matrix and custom Bash/sh/zsh paths.
- Final CodeWhale library test target rebuilt successfully; all 23 selected
  guidance-related tests passed, including the interpreter matrix, custom Unix
  paths, catalog consistency, and fixture export. This is a targeted library
  run, not the full workspace or process-integration suite.
- Exported catalog cross-check: tool name, shell, description, and property
  descriptions match the evaluated fixture. The catalog sanitizer removes
  `required` entries inside `anyOf`, while the standalone evaluation retains
  those source-schema entries. The fixture is therefore not an exact catalog
  replica; the measured comparison isolates guidance with the same source
  schema in both arms, rather than verifying the complete Engine request.
- Final compiled fixture cross-check: the description and property guidance
  match all 20 reminder-removed ablation requests. The only schema difference
  remains the documented `anyOf` sanitization.
- Evaluation script: Python compilation and offline mocked-request checks
  passed for paired fixtures, task selection, and after-only evaluation without
  a Git baseline lookup. Application preview regression: 11/11 passed.

### Curl reminder ablation (2026-09-05)

This follow-up tests the incremental value of one sentence in tool guidance:
`Use curl.exe for native curl flags on Windows; curl may alias Invoke-WebRequest.`
The `before` arm removes this sentence from both the tool description and the
command parameter description; the `after` arm retains it. All other request
fields are identical within each task/repetition pair, verified by structural
JSON comparison. Application instructions remain unchanged, including their
existing Windows `curl.exe` hint for browser preview verification. Both arms
also retain the PowerShell cmdlet alternatives and the warning about `head`.

Two real models each generated five repetitions of two tasks (raw response
prefix and JSON field extraction) in both arms: 40 new API calls total.
Requests were interleaved by repetition/task/arm, with three generation workers
per model and no explicit temperature or seed. All commands were reviewed and
hashed before execution against synthetic loopback data in fresh Windows
PowerShell 5.1 processes. No generated commands were repaired or regenerated.

| Model | Reminder removed | Reminder retained | Shell errors, removed / retained |
|---|---:|---:|---:|
| deepseek-v4-pro | 10/10 | 10/10 | 0/10 / 0/10 |
| deepseek-v4-flash | 9/10 | 9/10 | 0/10 / 0/10 |

Both Flash failures parsed JSON and reserialized it before truncation, changing
the requested original response text. Neither failure involved curl aliases or
shell syntax. One Pro command in the retained arm used `curl.exe`; all other
commands used PowerShell web cmdlets. No bare curl invocations occurred.

This sample found no incremental correctness benefit from the tool-level curl
sentence. It does not prove equivalence, establish behavior with only a shell
label, or justify removing every curl hint: application instructions still
contain one. Following review of the results, production tool guidance now
adopts the reminder-removed arm. Only this sentence was removed; all other
PowerShell guidance and application instructions remain as evaluated.

Private local artifacts: `D:/13_Pinvou/shell-guidance-eval/curl-ablation/` contains
the generation scripts, fixtures, paired requests, model-generated commands,
review hashes, and `execution-final.json`. Credentials and provider endpoints
are excluded from these artifacts. The same standalone schema/Engine limits
documented above apply.
