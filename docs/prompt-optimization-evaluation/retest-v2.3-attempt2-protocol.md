# V2.3 complete rerun after namespace mounting repair

Author: Codex (OpenAI). Model: GPT-6 (exact variant/reasoning level unavailable).

This addendum supersedes the execution attempt in the
[original V2.3 protocol](retest-v2.3-protocol.md), retaining its production
candidate, base revisions, model settings, cases, repetitions, budgets, grading,
acceptance rules and whole-matrix comparison. The production binary and every
production source hash remain unchanged.

## Excluded first attempt

Stop the whole first V2.3 attempt after 93 local and 40 cloud receipted turns.
Review of the third-root outputs exposed a harness defect: the namespace builder
mounted only directories literally named primary and secondary. The third
selected directory existed on the controller but was absent inside both agents.
This is not evidence that either product version cannot access a third root.
Retain all receipts and the interrupted tasks privately; exclude the entire
attempt from the final comparison, including unaffected and successful rows.
Do not resume or replace individual trials.

This is another execution correction discovered after observing partial output,
not a successful preregistered run. The four originally reserved V2.3 transfer
families were observed during this invalid attempt. Report them as
`reserved_transfer_rerun`, not as unseen fresh holdouts. No production prompt or
model setting is changed using those observations. Do not combine the first
attempt or the rejected V2.2 run with this comparison.

## Harness repair and preflight

Mount primary and every explicitly selected fixture workspace from the request,
with no hardcoded two-project limit. Preserve argument boundaries for spaces and
Unicode. Deduplicate nested selections whose ancestor is already mounted. Reject
missing directories, symlinks, traversal, the controller parent, runtime-home
roots and paths outside the trial. Fail before model calls if a selected fixture
root cannot be exposed. Do not mount undeclared siblings or the fixture parent.

The real namespace self-test now reads/writes third and fourth selected roots
(including a space and a Unicode name), and verifies that an undeclared sibling,
controller metadata, neighboring data and real user state remain hidden. Unit
controls cover nested selections, duplicate roots and invalid inputs. A separate
provider-free preflight prepares all 27 case fixtures and verifies every selected
root from inside its actual namespace before any model request.

Freeze the corrected harness, preflight/test receipts and this addendum before
restarting both versions and models from fresh homes for all 540 task trials and
620 user turns. The original seven-module runner claim now has one explicit
exception: `prompt_eval_sandbox.py` receives this mounting repair. Neither the
trial scheduler nor the deterministic/semantic graders are changed.

## Reporting

Publish the first-attempt exclusion reason/counts, original and corrected source
hashes and complete final counts. Separate frozen raw contracts, uniformly
calibrated contracts and the assistant's secondary semantic review. In
particular, a final no-files snapshot cannot detect a file created then deleted;
the secondary trajectory review must retain such a violation when observed.
Healthy native browser/card visuals remain outside the headless evaluation.

Signed: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).
