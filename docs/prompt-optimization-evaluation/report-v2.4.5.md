# V2.4.5 complete configured-model evaluation

Author: Codex (OpenAI). Model: GPT-6 (exact variant/reasoning level unavailable).
Date: 2026-10-27 (Asia/Shanghai).

Both configured routes completed **540 whole-task trials / 620 user turns**:
27 known families × five repetitions × two models × two versions. Main tables
exclude the unavailable-browser boundary family and therefore use **130 tasks
per model/version**. Every turn of a two-turn task must pass. All failures and
timeouts remain; no favorable selected reruns or replacement prompt versions are
mixed into this comparison.

The local aggregate regression is repaired on this matched baseline. This is a
candidate for review, with remaining format and workflow failures. GLM's tail
latency worsened; this is not an across-the-board speed improvement.

## Results and grader provenance

| Model | Frozen raw | Frozen calibrated | Uniform post-run path repair | All-five families after path repair |
|---|---|---|---|---|
| Ornith local | 77 → 102 /130 | 79 → 103 /130 | 84 → 108 /130 | 10 → 18 /26 |
| GLM cloud | 103 → 121 /130 | 103 → 121 /130 | 108 → 126 /130 | 18 → 24 /26 |

The post-run repair adds **five successes to each version of each model**; it
does not increase the baseline-to-candidate gain. The frozen calibrated gains
remain **24/130 (+18.5 percentage points)** locally and **18/130 (+13.8 points)**
for GLM. Use the labels above when quoting a score.

A frozen checker compared a controller path with unnormalized namespace shell
text in `third_tests_requested`, rejecting all 20 actual third-root test runs.
All 20 were uniformly replayed with the repaired checker in the same read-only,
networkless generated-code verifier. Only that check changed; code, keyword API,
rules and preservation checks already passed. No new model calls were made.
The new control fails against the [measured old evaluator](frozen-evaluator-v2.4.5/prompt_eval_repair_cases.py)
and passes with the fix. Keep the [frozen results](results-v2.4.5-raw.jsonl),
[frozen calibration](results-v2.4.5-calibrated.jsonl), [20-row replay](post-run-regrade-v2.4.5.json),
[repair/control record](post-run-evaluator-repair-v2.4.5.json) and
[path-corrected results](results-v2.4.5-path-corrected.jsonl) distinct.

The optional author review conjoins source-fact and explicit-instruction checks
with the **frozen calibrated** checks: local **59→80/130**, GLM **92→118/130**.
One local candidate answer adds an unverified sandbox/background assertion;
excluding that uncertainty gives **79/130**, not 80. This assistant authored the
prompts and saw progress and mappings. Review is neither independent nor fully
blind and is not a calibrated external-judge benchmark. See [all annotations](annotations-v2.4.5.jsonl),
[actual trajectory findings](trajectory-overrides-v2.4.5.json), and
[uncertainty sensitivity](semantic-uncertainty-v2.4.5.json).

## What improved and what remains

Local secondary-root delivery **0→5/5**, Plan→execute **1→5/5**, and synchronized
acceptance/archive revision **0→5/5** account for substantial workflow gains.
GLM improves secondary-root delivery **1→5/5**, literal relative-date JSON
**0→4/5**, absolute-date JSON **1→5/5**, and read-only secondary-root planning
**3→5/5**. These are the combined prompt/app candidate, not isolated component
or language effects.

Two families regress by one task: local missing-input handling **4→3/5** and
GLM English Skill format **3→2/5**. The local failures include both a finite
lexical false negative for a correct missing-file statement and an unnecessary
request to confirm a path despite the user's finish-without-question instruction.
GLM adds a Chinese preface to some required single-line English outputs.
Do not describe either case as fixed or hide them behind the aggregate.

Local literal relative/absolute JSON remains **0/5** under the strict whole-response
contract, often because of Markdown fences; local no-command secondary planning
remains **0/5**, including denied shell attempts and some incomplete rule searches.
One local secondary-code repair omits the required docstring. The primary
checks do not capture every unsupported progress claim, misleading explanation,
unrequested file write or forbidden generated-cache cleanup. The expanded review
retains these findings; no user-data deletion or new permission grant is inferred
from a scratch-namespace write or generated-cache operation.

The critical local not-started/planned/next-Monday family is **5/5** after the
already frozen equivalence calibration (raw 4/5; “下一周周一” is the missed equivalent).
Its baseline also calibrates to 5/5, so this is recovered reliability, not a gain
for that family. All five candidate deadlines are actually correct. In explicitly
requested calendar conversion, local strict output is 3/5 for both versions:
the candidate gives the correct date in all five but twice adds commentary;
the baseline has one wrong date and one format failure. No completed,
positionally correct candidate repair has a newly renamed public parameter.

All **120** named-Skill turn-zero trials load the exact expected body in both
versions; both have zero list calls here. This corpus therefore does not show a
measured reduction in list calls. The separately added tuple/set/bytearray alias
diagnostic is **4→5/5 for each model**, recorded in
[the V2.4.5 alias diagnostic](extended-code-diagnostic-v2.4.5.json); it is not the frozen JSON-shaped oracle.
The earlier 20-trial [community diagnostic](extended-code-diagnostic.json) has
different results and belongs to its earlier source boundary; it is not this receipt.
One failing baseline explicitly limits its copier's supported types; retain that
qualification rather than calling the qualified explanation fabricated.

## Performance: the same 26-family workload

Task latency sums both turns where applicable. It starts before the namespace/
product-host invocation and ends after report capture/recovery, before external
grading. Measurements include debug host startup, the scoped proxy and private
Xvfb; they are not release-app or unproxied provider latency.

| Model | Median task seconds | P90 task seconds | Tool attempts | Typed failed tools |
|---|---|---|---|---|
| Ornith local | 9.95 → 9.41 (-5.4%) | 34.58 → 29.15 (-15.7%) | 563 → 507 | 65 → 46 |
| GLM cloud | 25.49 → 24.87 (-2.4%) | 62.65 → 71.01 (+13.4%) | 588 → 508 | 50 → 24 |

**GLM P90 increases by 13.4%.** Some longer repair/verification trajectories
remain even though median latency and tool-attempt totals fall. Typed failure
counts do not detect every failed subcommand masked by a later successful shell
command. The restricted image also lacks some `awk`/`which` alternatives links;
keep fallback costs without labeling every error a native-product defect.

| Model | Observed input tokens | Observed output tokens | Observed input + output |
|---|---|---|---|
| Ornith local | 6,173,401 → 5,694,949 | 111,465 → 97,008 | 6,284,866 → 5,791,957 (-7.8%) |
| GLM cloud | 5,873,345 → 5,417,718 | 114,987 → 124,906 | 5,988,332 → 5,542,624 (-7.4%) |

GLM output tokens increase **8.6%** while observed total input+output falls.
Reasoning tokens are a reported output subset and are not added again. All 150
non-browser turns per model/version have some observed usage; one local baseline
turn lacks full-turn usage and uses completed-main-request observations. Unknown
or unreported fields are not proof of zero cost/cache activity. Counts are not
invoices. Server load, chat templates, hidden defaults and caches are not fully
controlled. HTTP model-attempt totals including the separate browser family are
1,314 locally and 1,209 for GLM. [Wire parameter whitelist](wire-parameters-v2.4.5.json)
and [provider/transport audit](transport-audit-v2.4.5.json) retain the observed
settings without real routes, profile IDs or credentials.

## Per-family objective results

Each cell is baseline→candidate out of five. The table uses the uniform post-run
path repair. For every other family it equals frozen calibration. `third_root`
is the diagnosed five-per-version correction, not a prompt gain.

| Family | Ornith local | GLM cloud |
|---|---|---|
| `absolute_json` | 0→0/5 | 1→5/5 |
| `absolute_requested` | 3→3/5 | 4→5/5 |
| `artifact` | 4→5/5 | 3→5/5 |
| `attachment` | 5→5/5 | 5→5/5 |
| `code_consult` | 4→5/5 | 5→5/5 |
| `code_fix` | 5→5/5 | 5→5/5 |
| `consult` | 5→5/5 | 5→5/5 |
| `hidden_code` | 4→5/5 | 5→5/5 |
| `json_escape` | 4→5/5 | 5→5/5 |
| `json_output` | 5→5/5 | 5→5/5 |
| `missing_input` | 4→3/5 | 5→5/5 |
| `multi_artifact` | 0→5/5 | 1→5/5 |
| `multi_code` | 4→4/5 | 5→5/5 |
| `nested_root_rule` | 5→5/5 | 5→5/5 |
| `plan_execute` | 1→5/5 | 5→5/5 |
| `plan_secondary_readonly` | 0→0/5 | 3→5/5 |
| `relative_deadline` | 3→4/5 | 5→5/5 |
| `relative_json` | 0→0/5 | 0→4/5 |
| `secondary_revision` | 0→5/5 | 3→5/5 |
| `semantic_revision` | 0→5/5 | 5→5/5 |
| `skill` | 5→5/5 | 5→5/5 |
| `skill_fidelity` | 5→5/5 | 5→5/5 |
| `skill_format` | 3→4/5 | 3→2/5 |
| `third_root` | 5→5/5 | 5→5/5 |
| `unbound_artifact` | 5→5/5 | 5→5/5 |
| `untrusted_data` | 5→5/5 | 5→5/5 |

The separate unavailable-browser boundary is **0→5/5 for each model**: successful
candidate completion means an honest unavailable-host report without substitute
fetching. It does **not** mean the native browser opened/read a page. Median/P90
above exclude these boundary timeouts.

## Source, environment and verification

The comparison consumes parent `0041ba29d7aae206703a5393b4d4511a58e120c9` and
foundation `6e1ee88ee8f8ea28eec7be3d8add3a591468412a`. The [source freeze](source-manifest-v2.4.5.json)
records binary/source/lock/protocol hashes; [prompt snapshot](prompts-v2.4.5.md)
shows changed static templates and runtime fragments. Additional roots remain
selected/tool-accessible but their rule bodies are discovered on demand. The
existing primary-root/ancestor loader, symlink/home boundaries and tool scopes
remain; scope labels are fixed at engine creation like existing instruction files.
There is no new turn loop, volatile per-turn prefix fact or provider-specific
prompt branch. Ordinary presenter availability still yields to Code-mode denial.

All 620 receipts, 543 unique final-output packets and 435 distinct actual-input
trajectories were reviewed. All 220 fixture Git-history checks and primary-root
preservation checks pass. All 27 mount preflights passed before model calls;
selected roots including spaces/Unicode are mapped, while controller/neighbor/
real-home data remain hidden. Unicode-root mounting is not a separate live-model
Unicode-path score. The real windowless product lifecycle uses configured real
models; no evaluation-only product tool policy is installed. Generated-code
verification is read-only and networkless.

Fresh checks include the bundle/bridge/prompt contracts, app Clippy, 582 Python
suite cases (three opt-in namespace checks skipped in that invocation and run
separately), and foundation all-target/all-feature Clippy with the documented
allow list. The installer test is **0 passed/1 failed without the fix**, then
**1 passed/0 failed restored**. Its user-customization protection is exercised.
An all-target Clippy failure in an unchanged base test was fixed by replacing a
cloned one-element slice with `std::slice::from_ref`; the existing nonexistent-root
behavior test passes. That is post-measurement test-only maintenance.

The earlier full foundation workspace/all-feature run includes TUI **12,287
passed/13 ignored**, 28 Cucumber and 265 integration tests. Its 13 changed guidance/
installer source hashes still match the measured candidate; the later test-only
borrow cleanup has focused verification. Earlier full app/CLI/knowledge, Node,
UI/Web build/lint and mocked browser/relay journey receipts retain their stated
source boundaries; affected bridge/templates were rechecked after development.
A zero-match early filter is preserved but is not counted as coverage. Exact
commands, counts and source boundaries are in [verification](verification-v2.4.5.json).

Upstream already uses [exact-name Skill loading](https://github.com/codewhale-hq/Codewhale/blob/7fe0cc2b4f41e45a71e833ffa302d60c4dc52613/crates/tui/src/skills/mod.rs#L1996)
and [available-capability-first Registry guidance](https://github.com/codewhale-hq/Codewhale/blob/7fe0cc2b4f41e45a71e833ffa302d60c4dc52613/crates/tui/src/core/engine.rs#L149).
This older maintenance baseline receives a narrow compatible adaptation; no
duplicate upstream proposal or wholesale lifecycle upgrade is claimed.

## Limits and interpretation

All 27 families are known/tuned regression coverage: **no fresh holdout** remains.
Five repetitions give observed all-five reliability, not pass@5 or a population
guarantee. Scenario-cluster intervals in the summary JSONs are descriptive and
conditional on this synthetic mixture. This [community-method suite](community-method-protocol.md)
adopts independent outcomes, isolation, repeated reliability and trajectory review;
it is not an official SWE-bench, Terminal-Bench or tau-bench score.

There is no English-only language comparison or component ablation. No Qwen live
run, memory-on/scheduler model run, long-context test, native Windows/macOS run,
or healthy native Browser/card visual end-to-end validation is claimed. A successful
backend delivery acknowledgement is not visual evidence. The namespace scratch
root/tmp is ephemeral and writable; losing an out-of-workspace file across turns
is not an actual host-root write or faithful desktop OS-root permission test.

Earlier 19/24→24/24 and 23/24→24/24 figures are unisolated exploratory evidence.
The older isolated 80-task result and rejected/partial development versions use
different bases/contracts or incomplete matrices; retain their reports instead of
splicing favorable rows into this run. Prior incorrect dates, API renaming and
additional-root rule-injection probes remain in the signed original analysis and
version protocols. This result supports reviewing the combined candidate and its
tradeoffs, not a claim of universal model reliability or improved latency everywhere.

Signed: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).
