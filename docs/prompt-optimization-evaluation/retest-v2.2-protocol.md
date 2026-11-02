# V2.2 repair and retest protocol — 2026-10-26

Author: Codex (OpenAI). Model: GPT-6 (exact variant and reasoning level unavailable).

The user selected repair, retest, then PR submission after the isolated V2.1
comparison exposed local regressions. Preserve the V2.1 reports and receipts.
This protocol applies the previously researched community methods; it does not
claim an official public benchmark score.

## Integration and intended repair

The working branch was safely advanced to parent `0041ba29d` and CodeWhale
`6e1ee88ee`. All pre-integration work was backed up and retained. Three parent
stash-application conflicts were resolved by preserving compatible mainline and
candidate changes. Mainline interpreter, shell transport and independent-probe
guidance remains present, including its regression checks. No protected branch
or release tag was changed.

Development probes showed the first targeted wording still missed secondary-root
rules and delivery. Inspection found the host injected only the execution root's
rule chain. V2.2 therefore reuses that loader for selected additional roots,
deduplicates shared ancestor files and labels directory scope. Existing home
boundaries, normalization failures, symlink refusal and foundation truncation
remain in force. Deeper path rules still require reading; no recursive directory
scan is added. These sources join the same engine-creation prefix as before,
not a volatile per-turn prefix. The artifact tool's own description is aligned
with file delivery rather than suggesting it is only for visual works.

V2.2 targets four observed mechanisms: omitted delivery after short file edits;
missed root/path-local project rules; invented status/calendar details during
rewriting; and output wrappers that violate an exact format or selected Skill.
Short actionable guidance is added at the relevant layer. The app also uses the
existing `tools_always_load` configuration to expose its artifact tool eagerly;
the existing code-mode deny policy still wins. This is an app configuration and
guidance change, not another engine/tool loop or a model-name-specific fork.
Measure its added input cost rather than assuming it is free.

## Development probes and freeze

Development probes may exercise the previously known regression cases. Keep all
such results separate from the formal run. The six fresh transfer cases below
are not run against a live model during development. Their grader controls use
known synthetic positive and negative outcomes, not model responses.

Before the formal run, freeze the candidate executable, source hashes, extension
module, task fixtures, objective graders and protocol. Record the untouched
integrated-main executable as the baseline. The same evaluator example is added
to both builds, with no simulated tools or model results. Historical V2.1 on the
older base is diagnostic evidence, not a numerically interchangeable baseline.

## Formal matrix

Run the prior sixteen families, the known unbound create/revise regression and
six fresh families, five repetitions, both configured models and both frozen
versions: **460 complete-task trials and 540 user turns**. The original three
two-turn families remain unchanged; the unbound family also has two turns, and
the fresh families each have one. The unbound path is promoted from the prior
single-trial smoke to repeated regression coverage before formal execution.
Alternate AB/BA order by case and repetition,
serialize tasks within one model, and retain failures and deadlines. No selected
successful reruns replace failed trials.

Fresh transfer cases:

1. Fix shipping arithmetic under a secondary root and a nested directory's
   annotation/docstring rules, preserving the primary and unrelated files.
2. Revise an existing secondary-root document exactly and explicitly deliver it.
3. Use a new Skill while retaining not-started/planned state and a relative
   deadline across a year boundary, without adding numeric calendar dates.
4. Return strict JSON with quoted and multilingual labels and a negative value.
5. Follow a new English Skill prefix, one-line format and planned factual state,
   using an English user request consistent with the existing locale recipe.
6. Submit a useful secondary-root Plan while keeping both roots read-only and
   avoiding command/test execution.

The opt-in retest entry reuses the unchanged seven-module isolated v2 runner and
adds two separately hashed extension modules. It saves nested-root snapshots
outside the model namespace before execution. Existing namespaces, fresh homes,
memory-off policy, configured profiles, pass-through proxy, step/deadline limits
and read-only independent verifiers remain in effect. No sampling/reasoning
setting is changed to improve the score.

## Grading and acceptance

Apply the known objective corrections consistently: equivalent acceptance
wording; explicit date consistency against the supplied reference; harmless
browser waiting; and complete primary-project preservation. Keep the original
untrusted-number lexical check in raw receipts and uniformly adjudicate asserted
37 versus rejected quoted 999 for all versions. A grader correction is not a
prompt gain. New graders have positive/negative controls for bad arithmetic,
missed nested rules, undelivered files, altered primary files, fenced JSON,
added dates, and Plan mutations/commands.

Report complete-task results and all-five reliability per family. The prior
sixteen families plus unbound creation/revision are known regression coverage;
only the six fresh families
are transfer holdouts, even if the reused runner's legacy partition field calls
the old extra cases holdouts. Publish an explicitly relabeled summary.
Review factual fidelity and relevant trajectories separately from objective
success. Any assistant content review remains non-independent and secondary.
Repeat the prior tuple/set/bytearray alias diagnostic on all generated merge
implementations, separately from the frozen JSON-shaped primary oracle. This
broader-domain diagnostic does not replace the primary score.

Repair acceptance requires the previously regressed local mechanisms to improve
on their recorded V2.1 behavior without creating a new blocking workflow or scope
regression in the new controlled comparison. Check both models. Do not interpret
small differences as proof of universal performance. If a substantive regression
persists, fix it and issue another version rather than silently shipping it or
calling the current task complete. A draft describing an external blocker does
not stand in for an implemented and verified repair.

Run the affected repository checks on the integrated candidate, including the
app/foundation/CLI/knowledge library suites, Python/Node, architecture/fork guards,
formatting, lint and UI/Web builds. Recheck the previously failing Web budget
with current main's actual code/dependencies. Real-model measurement starts after
heavy compilation to reduce interference. Healthy native browser/card visuals,
memory-on, scheduling, long-context, Qwen and a pure-English prompt variant remain
outside this matrix unless separately tested and labeled.

Before submission, self-review the final diff, preserve credentials/private
trajectories locally, record signed append-only decisions in the original
analysis document, and include improvements and remaining limits in the English
PR description. Any foundation PR is submitted before its consuming parent PR;
attach every created PR to this task. Never update the protected maintenance
branch, merge, tag or publish a release as part of PR creation.

Signed: Codex (OpenAI) / GPT-6 (exact variant and reasoning level unavailable).

## Harness startup correction before the complete matrix

The first launch aborted on the known report-revision case because the extension
omitted the unchanged runner's `acceptance_consistent` helper. Both processes
stopped after three receipted turns and one further response each. Preserve all
these files as an excluded startup attempt. Re-export the existing helper without
changing its behavior and add an interface control covering every `cases` attribute
used by the unchanged runner. Restart the entire matrix with refreshed evaluator
hashes; do not retain only its successful trials. Production sources, binaries,
fixtures and model settings are unchanged. No fresh transfer case was reached.

Signed: Codex (OpenAI) / GPT-6 (exact variant and reasoning level unavailable).
