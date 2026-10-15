# Supplied original event acknowledgement correlation

The upgrade codec's `Document::assert_supplied_event_acknowledgements` checks a
supplied original request against its supplied HTTP 200 event receipt. The
application's `check_supplied_event_acknowledgements_bytes` provides the same
capability for raw bounded request/response bytes. This is a T20 client queue
prerequisite; it does not implement a protected durable queue or grant deletion
of local pending events.

The entry accepts only the six original event operations: telemetry, preinstall,
install, activate, reconciliation and recovery review. It reuses the complete
original request and operation response validators. Both documents retain the
1 MiB metadata profile, duplicate JSON member rejection and original nested
64 KiB credential bound. Original credential validation here checks supplied
semantics at its declared time; signatures/current Root/current deny/time and
actual transmission remain separate authorities.

The supplied receipt must have exactly one acknowledgement per original event,
in the same order, with the same event ID in each position. All three original
dispositions remain valid: `committed`, `replayed` and `late-diagnostic`.
Identical immutable events may appear repeatedly, with their original positions
and acknowledgement count retained. Different events with the same immutable
credential JTI/event ID key are refused by comparing complete canonical event
bytes. Matching only the facts digest would miss sequence, occurrence time or
other event-field conflicts. The comparison's map belongs to this one original
batch and introduces no global event-ID uniqueness rule.

The response's server-generated `requestId` has no corresponding field in the
original request. It is validated by the response contract and is not matched
to an invented input identity or used as authentication. The supplied HTTP
status must be exactly 200. A valid error response, lost response, incomplete
receipt, an event ID in the wrong acknowledgement position or an unrelated event
cannot produce a binding success. Two wire-identical duplicate events cannot be
distinguished by swapping their valid acknowledgement dispositions; this adds
no disposition ordering rule. An unsuccessful batch can have a remotely committed
prefix; absence
of a complete matching receipt provides no permission to clear that prefix.

Existing request, response, receipt and HTTP contract-only entries retain their
behavior and error codes. In particular, a valid original HTTP error contract
still passes the old structural HTTP checker. The new dedicated entry refuses
it as an acknowledgement. New errors contain fixed codes and never include
credential or event bodies.

The scoped tests generate genuinely signed original credential templates in
memory and preflight their complete original API request/response contracts.
They cover six operations, identical duplicates and mixed dispositions,
cardinality/order/foreign-ID faults, full-event immutable conflicts with old
contract positives, canonical key-order equivalence, status separation,
unchanged inputs, duplicate/malformed raw JSON, exact metadata bounds, nested
credential bounds and the actual application facade. Mutated supplied-contract
controls do not claim current cryptographic authenticity. This comparison is
not an authenticated service-to-physical-client end-to-end qualification.

Full T20 still requires protected durable original event storage, retry/recovery
ownership and authenticated receipt handling in the actual client workflow.
Current trust and physical host protection retain their T27/T28/T31 boundaries.
No wire contract, Schema, dependency, configured origin/root or UI changes.
No total/cumulative artifact download duration or retry cap is introduced.
