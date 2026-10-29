# Prompt optimization evaluation records

Current integration: [CodeWhale#125](https://github.com/Pinvou/CodeWhale/pull/125)
merged as `f1eb10efc`; the application consumes that actual public maintenance
squash. The reviewed replay is `1aac7942c` on main `3cec34e2b`. Public-head
verification passes. The former candidate dependency and draft instructions in
dated R1–R4 records are superseded; those records retain their original heads.

Illustrated [Chinese report](提升报告-V2.4.5.md), [English report](report-v2.4.5.md),
[PDF](pinvou-improvement-report-v2.4.5.pdf), and [poster](figures/pinvou-poster-v2.4.5.png)
contain the historical pre-review V2.4.5 measurements. PNG/SVG figures are reproducible
with `render_report_figures.py`. No R1–R5 model run or new score is implied.

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

Supporting receipts: [Git preservation](git-preservation-v2.4.5.json),
[loaded Skill bodies](loaded-skill-body-audit-v2.4.5.json),
[third-root command diagnostic](third-root-command-diagnostic-v2.4.5.json),
and [calibration changes](calibration-changes-v2.4.5.json).


## Current review source and validation boundary

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

Author: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).
