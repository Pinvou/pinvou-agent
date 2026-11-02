# V2.2 closed expression calibration

Author: Codex (OpenAI). Model: GPT-6 (exact variant/reasoning level unavailable).

During review of the running frozen matrix, packets `49010e5af414e1d2` and
`9f3bf50f1554c6cc` exposed literal-matching false negatives: `下周周四` and
`下一周周一` retain next Thursday and next Monday, respectively. This addendum
is recorded before tallying their impact by version. The reviewer authored the
candidate and has seen per-model progress and early diagnostic receipts; masked
packet formatting is not an independent or fully blinded assessment.

Preserve every frozen raw result. Apply the same closed token normalization to
all versions, models and repetitions only in the separate calibrated layer.
Normalize a next-week token (`下周`, `下一周`, `下星期`, `下一星期`, `下礼拜`,
`下一礼拜`), an optional redundant weekday introducer, and a weekday to `下周`
plus that weekday. Do not normalize a token preceded by `上` or `下`; two-week
expressions must not become next-week expressions. This is a closed lexical
correction, not a general Chinese time parser.

Only the existing task/deadline and planned-state checks in `skill_fidelity`,
and the existing factual-field conjunction in `relative_deadline`, are
recomputed using that canonical weekday token. The remaining frozen predicates
are unchanged. Wrong subject/state, wrong weekday, changed deadlines such as
“within the coming week”, contradictory numeric dates, extra dates, exact
prefix violations, JSON escaping, timeouts and tool failures are not rescued.
A correction is recorded per check and is not counted as a prompt gain.

The current companion source and nine positive/negative control tests are separately
hashed in `calibration-v2.2-manifest.json`. Production code, binaries,
model settings, tasks and the live runner remain frozen and unchanged. No new
model call is made for this calibration. The final annotations retain the
synthetic text for the three affected progress families so the public replay is reproducible.

Replay the already existing objective/semantic annotations first, then apply
this companion to either result layer:

```bash
python3 scripts/apply-prompt-adjudication.py results-v2.2-raw.jsonl \
  --annotations annotations-v2.2.jsonl > objective-before-weekday.jsonl
python3 scripts/prompt_eval_retest_calibration.py objective-before-weekday.jsonl \
  --annotations annotations-v2.2.jsonl > objective-after-weekday.jsonl
```

For the separate stricter semantic layer, add `--include-semantic-checks` to the
first command. The companion preserves those checks. Use the real paths to the
published receipt and annotation files; temporary replay outputs are local files.

Also inspect the last `todo_write` update in Plan trajectories. A historical
pending step alone does not establish that the final proposed steps retain the
required pending format. Keep that instruction-fidelity finding in the separate
semantic layer; do not claim a native UI failure from this headless test.

Signed: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).

## Negative-start wording addendum

Before tallying version impact, packet `8a1ce6337b8797ed` exposed the same
lexical issue for `还没启动`: the frozen conjunction accepts `未启动` and
`还没开始` but misses this equivalent not-started wording. Version 2 additionally
normalizes unnegated `还没/还没有` followed by `开始/启动` to `尚未开始`.
Common explicit negations (`不是`, `并非`) are excluded. An eighth control tests
positive synonyms and negative/started states. This is still a closed lexical
normalizer, not a general factual or negation parser. Preserve all other checks.
The initial v1 helper, manifest and seven-test controls are retained privately;
no published result was produced with v1. Current method is
`closed-relative-progress-equivalence-v2`. No live-model input or task changes.

Signed: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).

## Browser availability lookup

Packet `2e71dceffe0b719e` uses `which npx node` (with output redirection and an
`echo` separator) before the built-in browser call. This only looks up executable
paths; it does not invoke Node, start another service or fetch the page. The
frozen browser-channel predicate rejects it because it only admits browser tools,
discovery and waiting. Review every non-browser call in this family and use the
existing `browser_channel_respected` annotation for this harmless lookup as well.
Actual alternate fetching or service invocation remains a violation. Preserve
raw values, completion/deadline failures and the per-row correction ledger.
This uses the existing adjudication replay, not a change to the live runner or
the lexical helper. The exact lookup was inspected before tallying its impact
by version; no model rerun is selected to replace it.

Signed: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).

## English not-started expression addendum

Packet `4dddb01f6dbe4642` says `The data migration remains unstarted`, correctly
retaining the supplied not-started state. The frozen English-state regex misses
`unstarted`. Before tallying version impact, version 3 adds the closed affirmative
phrase `remains unstarted` → `has not started` and recomputes only the existing
factual conjunction for `skill_format`. Exact prefix/language format and other
checks remain unchanged. A ninth control covers the positive expression, already
started/negated alternatives and a wrong weekday. Current method is
`closed-progress-expression-equivalence-v3`; prior v1/v2 sources and control
evidence are retained privately, and neither exported published results. No
production/live-runner/task/model-setting change or additional model call.

Signed: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).
