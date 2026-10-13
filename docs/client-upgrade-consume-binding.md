# Client upgrade supplied consume binding

T27 batch 19 implements the exact post-verification relationships from frozen
assertConsumeBinding. It accepts a private, borrowed ConsumeRequestShape and
a SchemaCheckedEnvelope. The request constructor reuses the unchanged B6
closed semantic request predicate, now visible only to its parent module.
No new dependency, frozen Schema change or unchecked public value constructor
is introduced.

The request has all 17 original semantic fields. Unlike the digest projection,
this constructor rejects the four transport/self fields consumeRequestDigest,
recoverySecret, requestId and headers, including explicit null values.
All original JSON is resource bounded and duplicate checked before construction.
Required nullable fields remain required; missing and null are distinct.

The three purposes select the exact corresponding authorization role. The
declared hash must equal SHA-256 of the factory's original complete envelope,
including its signature records. Hashing only the signed body is insufficient.
Product, component, purpose, authorization JTI, transaction ID, client facts,
preparation, freeze epoch, backup, staged content and preinstall slot compare as
whole bounded JCS values. Installation scope ID and the entire plans object also
bind exactly. Canonicalization failures propagate; no failed comparison can turn
into an equality between two missing results.

Consume key and recovery handle hash retain their frozen request Schema checks;
they have no corresponding equality check against authorization claims here.
This pure binding subset adds no clock, credential lifetime, current ownership,
qualification, key custody or atomic outcome rule. The caller must separately
perform all of verifyEnvelope and the later authoritative consume operation.
Neither a correct binding nor an encoding-valid zero signature grants permission.
Success produces no consumed authorization, persisted result, execution object
or erasure certificate.

Four test groups cover all three purposes, every bound request leaf, full
nullable records and plans, reordered JSON, complete-envelope signature changes,
signed-body hash refusal, other-role refusal, closed request boundaries and
escaped duplicate keys. Positive controls deliberately retain independent
consume key/recovery handle values and Schema-valid invalid credential timing:
those constraints belong to other operations.

The actual Node oracle pins the exact original export prefix, extracts only
the unchanged statements after verifyEnvelope, and runs the original request
Schema and full envelope parser/encoding separately. It never substitutes fake
cryptography for verifyEnvelope or claims whole-export equivalence. Its
self-derived expected context isolates these relationships only; B16 separately
tests independent expected context. App integration exercises all three purposes,
stale complete-envelope hashes and forbidden transport fields.

Final evidence is recorded in [implementation progress](upgrade-implementation-progress.md).
Complete T27 authenticity, protected persistence and consumption composition remain
unfinished. Downloads retain no total/cumulative duration or retry cap.
