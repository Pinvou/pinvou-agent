# V2.4.2 final comparison addendum

Author: Codex (OpenAI). Model: GPT-6 (exact variant/reasoning level unavailable).

Status: prepared during development, before the complete final-matrix calls.
This addendum inherits the matrix, isolation, stronger API/one-sentence
contracts, acceptance rules, verification and reporting requirements from
[the V2.4.1 protocol](retest-v2.4.1-protocol.md). Hash both documents in the final
source freeze. The final candidate is V2.4.2 rather than V2.4.1.

## Development observation and final time wording

V2.4.1's ten candidate local date probes all passed the automatic checks, but one answer
said tomorrow and next Monday together. They conflict under the supplied
Monday2026-10-26 reference. This is a factual failure missed by the numeric-date
checker, not a complete10/10 success. The word also appeared in the system
example; example interference is a hypothesis, not an established sole cause.
Retain the complete development record and do not merge it into the final run.

V2.4.2 removes the concrete time example. It treats preserved time expressions
as strings copied only from the user's source or requested source file, without
interpretation, completion, alternative wording or appended dates. Explicit
changes/translations/calculations remain allowed. An explicitly requested
schedule or query with time constraints may still need calendar calculations;
the preservation rule does not disable those operations.

The next ten local development probes have raw9/10 for each version. Applying
the already declared closed weekday-wording calibration gives baseline9/10
and candidate10/10: the sole candidate raw failure says next week Monday with
an equivalent repeated-week word. Manual review finds all ten candidate
answers retain not-started/planned state and the intended next-Monday deadline,
without an added date, conflicting relative day or draft/correction text. These
are development probes, not final performance or evidence of universal safety.

Complete the separately recorded known workflow probes before formal freeze:
three local repetitions of seven JSON/calendar/API/default-workspace/format
families, and one cloud smoke repetition of five relevant families. They use
the actual configured models and product lifecycle. No new unseen holdout is
claimed, and their60 turns are excluded from the final620-turn matrix.

## Calendar consistency review declared before final calls

The existing `due_date_consistent` adjudication must consider **all asserted
calendar meanings** in the factual-progress answer, including relative day
claims without digits. Evaluate them against the fixture's supplied reference,
not the wall-clock date at review time. An added tomorrow cannot pass merely
because the same answer also contains the requested next Monday.

For that fixed case, reference2026-10-26 is Monday and the requested next Monday
is2026-11-02; tomorrow from the reference is2026-10-27. A clearly contextual
restatement of the supplied reference is not an incorrect target date. Apply
the same interpretation to every model/version/repetition, with a written
annotation and correction ledger. Keep the raw numeric regex result visible.
Such grading corrections are not prompt gains. Other source/instruction
judgments remain in the separately labeled secondary review.

This uses the existing replay annotation field; it does not silently edit raw
receipts, introduce new model calls or forgive strict JSON/prefix/date-number
failures. It also does not make the author review independent or fully blinded.
The original date regex and finite sentence checks remain imperfect; do not
equate a raw pass with complete factual fidelity.

The earlier V2.4.1 core protocol describes the development example historically;
the final V2.4.2 source contains no concrete time-word example. No production
source, case or grader may change during the final run. A necessary correction
requires a separately disclosed new version, not selected favorable reruns.

Signed: Codex (OpenAI) / GPT-6 (exact variant/reasoning level unavailable).
