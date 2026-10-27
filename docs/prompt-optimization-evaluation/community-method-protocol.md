# Pinvou community-method evaluation protocol v2 (isolated) — 2026-10-26

Author: Codex (OpenAI). Model: GPT-6 (exact variant and reasoning level unavailable).

This is a custom Pinvou evaluation informed by public agent evaluation methods.
It is not a run of the official SWE-bench, Terminal-Bench, or tau-bench datasets,
and its scores must not be labeled as public benchmark scores.

## Methods and sources

| Primary source | Method adopted here |
|---|---|
| [SWE-bench evaluation guide](https://www.swebench.com/SWE-bench/guides/evaluation/) | Validate the actual generated code using independent functional tests and preserve previously working behavior. |
| [Terminal-Bench](https://www.tbench.ai/news/announcement) and [Harbor evaluation guide](https://github.com/harbor-framework/docs/blob/main/guides/running-evaluations.mdx) | Freeze task environments, record complete trajectories, verify filesystem outcomes, and retain failures. |
| [tau-bench repository](https://github.com/sierra-research/tau-bench) | Repeat tasks and report reliability across trials, separately from at-least-one-success metrics. |
| [Anthropic: Demystifying evals for AI agents](https://www.anthropic.com/research/demystifying-evals-for-ai-agents) | Separate outcome, workflow, and safety checks; calibrate graders against positive and negative controls; review transcripts beyond automatic scores. |

## Frozen comparison

- Parent source baseline: `983c65e79`; foundation baseline: `ea9fdc90f`.
- Compare the previously frozen original executable with the final V2.1 executable.
  This isolates the implemented candidate on the same historical product base.
  It does not claim that current `origin/main` has already been integrated.
- Use the configured Ornith-1.5-35B-A3B-NVFP4 and glm-5.3-flash routes. Preserve
  configured sampling and reasoning behavior. Only synthetic fixtures reach them.
- Five independent task environments per scenario, model, and version.
- Sixteen task families, 320 complete-task trials, and 380 user turns expected.
  Three families have two turns; all other families have one.
- Alternate baseline/candidate order by scenario and repetition. Trials of one
  model run serially. The two model services may run concurrently after local
  repository regression checks finish; no intentional model concurrency is added
  within either service.
- Fresh private home and project per trial; memory off; shipped Skills plus the
  named synthetic Skill only; no user sessions, projects, or connectors copied.
- Opaque trial directory names avoid exposing the version label in workspace paths.
- Forty-step limit; 180-second product deadline, except the explicit unavailable
  browser-host task's 75-second deadline. A process-level guard adds 60 seconds.
- Freeze executable, runner, fixture/grader and accounting-helper SHA-256 hashes
  before formal runs. Calibration probes are separate and excluded.
- No prompt changes during formal testing. Resumption requires identical hashes.

## Cases

The original ten families remain: consultation, artifact creation/revision,
code repair, secondary-root artifact, secondary-root code, Plan/execute,
Skill continuation, full CSV attachment, untrusted data, and unavailable browser.

The six holdouts add:

1. Explicitly synchronize accepted status and archive next steps in an existing
   report, preserving the supplied facts and sentinel.
2. Produce a strict JSON response from real file contents, including negative values.
3. Repair recursive configuration merging: visible tests plus 104 hidden
   independently generated functional/input-aliasing cases, with original files
   preserved. The hidden checks are not placed in the model's project directory.
4. Report a genuinely absent input honestly without inventing results or data.
5. Use a named Skill while retaining an explicitly planned, unstarted status and date.
6. Explain a code defect without modifying files or executing commands.

## Grading and reporting

- Report completion, outcome, application workflow, and safety separately. Overall
  success requires every applicable check and completion within the deadline.
- Application delivery and Plan submission have observable tool contracts; generic
  coding outcomes do not require one arbitrary tool sequence.
- Equivalent acceptance wording is allowed. The old report-revision instruction
  says to preserve other content, so contradictory old next steps are retained
  as semantic-review warnings rather than silently imposing a new requirement.
  The new holdout explicitly requires synchronizing both status and next steps.
- The status grader has positive controls for equivalent wording and negative
  controls for accepted-and-pending contradictions. The code oracle rejects the
  original shallow merge and a recursive implementation that shares input objects,
  and accepts a fully independent reference result.
- Review generated reports and Skill outputs for unsupported facts and semantic
  contradictions. This review is model-assisted evidence review by this assistant,
  not an independent human panel or a calibrated external judge benchmark.
- Report per-attempt and complete-task success, each case's 0–5 successes, and
  the fraction of cases successful on all five trials (observed pass^5). Do not
  conflate this with pass@5, which asks whether any attempt succeeded.
- Report paired task deltas and uncertainty with scenarios as clusters; five
  trials on sixteen synthetic families do not establish broad model rankings.
- Include failures/timeouts in total workload. Report median/p90 task latency,
  main-session tool attempts/errors, and whole-turn usage when provided. Missing
  usage and unfinished tool calls remain visible. No output-content error heuristic
  overrides a typed tool failure flag.

## Environment and limits

The real product engine/tool loop runs in the evaluation host; model responses
and tool execution are real. Each trial now runs in its own bubblewrap mount,
PID, network, user, IPC, and UTS namespaces. Only its primary/secondary projects,
fresh application home, explicitly declared attachments, fixed agent executable,
and bootstrap runtime are mounted. Controller metadata, graders, neighboring
trials, source checkouts, the real user home, D-Bus, and real credentials are not
visible. System tools and libraries are mounted read-only. Namespace acceptance
checks must pass before every worker starts. The generated-code verifier runs in
a separate read-only project namespace with no network or agent state.

A private Xvfb display permits GTK initialization. It does not add a healthy
browser host or validate native GUI rendering. Only a scoped model proxy socket
is exposed to the trial. The trusted proxy resolves the configured credential in
memory, replaces a temporary capability with the real authorization header, and
forwards unchanged request bodies and real streamed responses. Routing metadata,
model, sampling, context and reasoning settings remain configured. Real keys and
endpoints are never written to model-visible trial settings. Synthetic unit
fixtures verify transport behavior separately and do not count as model trials.

The first attempted formal batch lacked OS isolation. A model read controller
metadata and neighboring trial files, so that entire batch was stopped and is
excluded, including apparently successful rows. Earlier 128-turn exploratory
results also lacked this isolation and remain historical evidence only. Failed
calibration probes and both generations of raw evidence are retained privately.
The clean v2 comparison starts in fresh output directories after calibration.

Server hardware, load, chat templates, and caches are not independently controlled
or flushed. The credential-isolating transport, private display, dev executable,
and process startup contribute to wall time. These are controlled-suite timings,
not native release-app latency or provider billing measurements. Partial usage
from completed main-session requests is labeled separately from whole-turn usage;
missing usage is never interpreted as zero cost.

The browser family intentionally exercises an unavailable windowless host. It
cannot prove successful browsing or visual card rendering. Desktop startup is
verified separately, and repository UI/protocol checks provide additional coverage;
any untested healthy native GUI path must remain explicitly unverified.

Repository verification runs the architecture/fork guards, Python graders, Rust
formatting, UI/Node lint and build, the full Node suite, full application library
tests, full foundation TUI library tests with benchmark controls, CLI workspace
library tests, and knowledge library tests. Exact commands/results are recorded
separately; a failed check is not turned into a passing check by this protocol.

The first full foundation run used the inherited network environment. The final
recheck uses a reject-only loopback HTTP proxy for external unit-test probes with
synthetic credentials; localhost fixture services bypass it. This avoids repeated
public model-list probe waits. No tests are skipped for this change. The proxy is
not applied to either live-model evaluation process, which retains its configured
real routes.

PR submission remains paused until this testing request is handled. Any later
integration with newer mainline commits requires fresh integration verification;
the historical real-model measurements remain versioned evidence.
