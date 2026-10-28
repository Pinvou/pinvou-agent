# Prompt optimization evaluation records

Illustrated Chinese report: [V2.4.5 improvement report](提升报告-V2.4.5.md). The four charts are available as PNG/SVG under `figures/` and are reproducible with `render_report_figures.py` from the published summaries. A portable PDF edition is [available here](pinvou-improvement-report-v2.4.5.pdf).

Latest complete comparison: [V2.4.5 report](report-v2.4.5.md), [source freeze](source-manifest-v2.4.5.json), and [verification](verification-v2.4.5.json).

Read the result-layer labels: frozen raw, frozen calibration, uniformly repaired command-path grading, and secondary author review are separate. Current measured evaluator copies and post-run repair controls preserve the known grading defect instead of hiding it. See the report for per-family losses and GLM tail-latency regression.

Historical `report.md`, `community-report.md`, earlier result files and rejected-version protocols remain evidence for their own execution/source boundaries. Early 24-task smoke figures are not the current controlled headline; all current families are known regression coverage.

Author: Codex (OpenAI). Model: GPT-6 (exact variant/reasoning level unavailable).

## PR submission boundary (2026-10-27)

Foundation review: [CodeWhale#125](https://github.com/Pinvou/CodeWhale/pull/125), candidate `50a42c38edf81e45019403d33ac05ef87da77555`. Public maintenance remains `6e1ee88ee`; the application PR stays draft until the foundation merges and the gitlink is repinned. The public-head verifier is unchanged and currently rejects the review gitlink as expected.

The application branch integrates main `0dd56c965` without changing the measured prompt bodies. Model results retain their frozen historical binary/source/lock boundary. Fresh [submission checks](pr-submission-verification-v2.4.5.json), [UI/Node checks](pr-submission-ui-v2.4.5.json), and [public-head dependency record](pr-public-head-boundary-v2.4.5.json) are separate from the earlier full Rust receipts. The [single-poster summary](figures/pinvou-poster-v2.4.5.png) uses the same result-layer caveats.

Signed: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).

## Review revision and attribution

The V2.4.5 model results are historical measurements of the frozen sources, not
new measurements of later review fixes. The review revision restores bounded
documented-name probes for deferred tools, fixes browser/authority/Plan wording,
and tightens publication evidence. Its deterministic checks are recorded
separately. No new 540-task model run is implied.

Current foundation review head: `0af53af6c0648b544e4b79194acc9e4f408ef7a4`.
Public maintenance `6e1ee88ee` names `pinvou3-clean`, not CodeWhale `main`.
The parent follows the reviewer's clean replay onto main `ab6ac99e6`;
all three previous patch identities and their merged tree were verified before R3.
The [R2 verification](review-r2-verification.json) and [R2 source manifest](source-manifest-review-r2.json)
record discovery/probe/receipt agreement. The [R3 verification](review-r3-verification.json)
and [R3 source manifest](source-manifest-review-r3.json) record the register, evidence,
and guard repairs independently of the frozen measurements.
See the [R1 verification](review-r1-verification.json) and separate
[R1 source manifest](source-manifest-review-r1.json). The original submission's
seven freshly executed namespace/proxy checks are in
[pr-submission-verification-v2.4.5.json](pr-submission-verification-v2.4.5.json);
they are distinct from the retained notes in `verification-v2.4.5.json`.

"Signed" in these prose reports means author/model attribution, not a digital
signature. File integrity is recorded by the SHA-256 manifest chain. Cryptographic
Git signatures, if present, are a separate property; a DCO trailer is also distinct.

Author: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).

## Evidence index

The full 27-family summary layers are [raw](summary-v2.4.5-raw.json),
[calibrated](summary-v2.4.5-calibrated.json), [reviewed](summary-v2.4.5-reviewed.json),
and [path-corrected](summary-v2.4.5-path-corrected.json). The figure renderer uses
the corresponding non-browser `-regression` summaries; these are separate views.

Supporting receipts: [Git preservation](git-preservation-v2.4.5.json),
[loaded Skill bodies](loaded-skill-body-audit-v2.4.5.json),
[third-root command diagnostic](third-root-command-diagnostic-v2.4.5.json),
and [calibration changes](calibration-changes-v2.4.5.json).

## Approval-review follow-up (R4)

The foundation review approved `643e5e1b3` and suggested nonblocking guidance/pin
improvements. The follow-up restores match/inspection semantics, aligns discovery
availability qualifiers and proves mixed peer/custom-body migration. See the
[R4 verification](review-r4-verification.json) and [R4 source manifest](source-manifest-review-r4.json).
The earlier approval and all frozen V2.4.5/R1–R3 records retain their own heads;
this is no new model measurement. The app remains draft until foundation integration
and the real maintenance-squash repin/public verifier.
