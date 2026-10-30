# Prompt optimization evaluation records

Current integration: [CodeWhale#125](https://github.com/Pinvou/CodeWhale/pull/125)
merged as `f1eb10efc`; the application consumes that actual public maintenance
squash. The approved R6 checkpoint is `58431e750b` on main `3cec34e2b`;
`1aac7942c` is the earlier replay, not the current review boundary. Subsequent
follow-up changes are identified by the PR head and their own receipts. Public-head
verification passes. The former candidate dependency and draft instructions in
dated R1–R4 records are superseded; those records retain their original heads.

Illustrated [Chinese report](提升报告-V2.4.5.md), [English report](report-v2.4.5.md),
[PDF](pinvou-improvement-report-v2.4.5.pdf), and [poster](figures/pinvou-poster-v2.4.5.png)
contain the historical pre-review V2.4.5 measurements. PNG/SVG figures are reproducible
with `render_report_figures.py`. No R1–R7 model run or new score is implied.

Read the result-layer labels: frozen raw, frozen calibration, uniformly repaired
command-path grading, and secondary author review are separate. Current-source
fixes do not overwrite frozen receipts or retroactively change headline results.
The corpus contains known/tuned regression families, not fresh holdouts. The
[frozen evaluator copies](frozen-evaluator-v2.4.5/README.md) preserve their measured
bytes and documented defects; current graders live under `scripts/`.

## Evidence index

The full 27-family summary layers are [raw](summary-v2.4.5-raw.json),
[calibrated](summary-v2.4.5-calibrated.json), [reviewed](summary-v2.4.5-reviewed.json),
and [path-corrected](summary-v2.4.5-path-corrected.json). The figure renderer uses
the corresponding non-browser `-regression` summaries; these are separate views.
Both `reviewed` and `path-corrected` branch directly from `calibrated`.
They are parallel interpretations, not consecutive stages: the reviewed layer
does not include the path-correction layer.

The published calibrated JSONL contains **1,284 local / 1,204 GLM** observed
`model_requests` entries. The **1,314 / 1,209** HTTP-attempt totals in
[wire parameters](wire-parameters-v2.4.5.json) are separate proxy-side observations,
including attempts without a corresponding published model-request receipt.
The **30 / 5** differences cannot be independently reconstructed from this
JSONL. Do not use those proxy totals as receipt-derived counts or as evidence of
a measured improvement. The historical files retain their original bytes.

Supporting receipts: [Git preservation](git-preservation-v2.4.5.json),
[loaded Skill bodies](loaded-skill-body-audit-v2.4.5.json),
[third-root command diagnostic](third-root-command-diagnostic-v2.4.5.json),
and [calibration changes](calibration-changes-v2.4.5.json).


## Current review source and validation boundary

The R7 follow-up makes the provenance unit fixture independent of bubblewrap
and restores shared grader bindings after both normal and interrupted tests.
Real namespace execution remains in the explicit Linux acceptance opt-in.
See [R7 verification](review-r7-verification.json) and
[R7 source manifest](source-manifest-review-r7.json).

R6 over `d3e84dcec` fixes the secret-scan Python prerequisite, fingerprints lateral
grader imports and runner adapters, and refuses mixed revisions during resume,
aggregation and adjudication. Deleting a manifest does not make old rows eligible.
Wholly unversioned historical corpora remain readable as legacy evidence; they
cannot be appended to a current live run. See [R6 verification](review-r6-verification.json)
and [R6 source manifest](source-manifest-review-r6.json).

Current numeric controls reject extra alternative values and accept equivalent
whole-number `.00` notation. Negated numbers and other prose interpretations
still require disclosed transcript review; these lexical controls are not a
general semantic oracle. Overlapping duplicate tool-call IDs stay unsettled,
since the trace cannot establish which call owns a result. Missing turn traces
or deleted preservation sentinels fail checks instead of crashing an audit.

The presenter schema adds input cost to every ordinary-mode request when its
server is healthy. There is no measured per-request token figure for the current
schema; tokenizer, serialization and model vary. The earlier R5 review estimate
is historical, not a validated bound. Eager exposure does not guarantee a healthy
backend or a visible native card.

The original R5 checkpoint is `5a78aebfd`. A [Linux loader follow-up](review-r5-ci-verification.json)
adds the read-only `/lib64` view needed by x86_64 ELF interpreters; the earlier
R5 receipts below retain their exact original source/test boundary. Current
ready-PR CI is tracked separately from local ARM64 execution.

[R5 verification](review-r5-verification.json) and [R5 source manifest](source-manifest-review-r5.json)
record grading correctness, regression controls, integrated-state documentation,
exact Plan/browser pins and applicable current checks. New evaluation rows carry
`grader_revision`; its manifest includes every delegated grader source. Existing
suite-specific acceptance and browser policies are identified by that revision,
not mixed into a common historical score.

The immutable [measurement sources](source-manifest-v2.4.5.json) and
[measurement verification](verification-v2.4.5.json) retain their own execution
boundary. The [historical prompt snapshot](prompts-v2.4.5.md) has an explicit
supersession banner; its original body is preserved.

## Earlier review and submission receipts

- R1: [verification](review-r1-verification.json), [sources](source-manifest-review-r1.json).
- R2: [verification](review-r2-verification.json), [sources](source-manifest-review-r2.json).
- R3: [verification](review-r3-verification.json), [sources](source-manifest-review-r3.json).
- R4: [verification](review-r4-verification.json), [sources](source-manifest-review-r4.json).
- Original submission: [checks](pr-submission-verification-v2.4.5.json),
  [UI/Node](pr-submission-ui-v2.4.5.json), and the superseded
  [candidate dependency](pr-public-head-boundary-v2.4.5.json).

Historical reports, rejected experiments and earlier result files remain evidence
only for their stated source boundaries. Prose attribution, DCO trailers and
cryptographic Git signatures are distinct.

## Scheduled-origin follow-up

Status: separate behavior fix pending; this is the actionable follow-up for the
R5 exclusion, not a claim that PR799 repairs it. At the approved R6 checkpoint,
`build_engine_config_for_session_roots` in
[`bridge.rs`](../../pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs)
pushes `runtime:scheduled-request-origin` for non-aux sessions, then replaces
`cfg.instructions` with `session_instructions(session_id)`, discarding that entry.
The entry carries session provenance for idempotent automation operations; it
does not grant permission.

A follow-up should assemble the session instruction stack before adding the
origin entry exactly once. Verify ordinary, scheduled, Code and Plan sessions,
the empty-project-rule case, and repeated config construction. Aux sessions must
retain their minimal instruction/tool surface without this entry. Check the
actual session id and preserve the distinction between provenance and authority.
Keep those behavior changes and their regression tests in a separate PR.

## Evidence storage boundary

The frozen evidence package remains in this PR as reviewed. The approved R6
review reported approximately 15 MB / 128 files. Approval of the behavior does
not establish a general size threshold. A future in-tree, LFS or release-asset
policy remains a maintainer decision; this follow-up records the question
without establishing a policy.

Author: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).
