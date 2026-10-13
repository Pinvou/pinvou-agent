# Client upgrade independent expected context

T27 batch16 compares one complete SchemaCheckedEnvelope with a separate required
supplied expected-context Document. It never derives expectations from the
candidate, switches to candidate.role to repair a wrong expected role, or supplies
a missing Root component as null. The expected value must be an object with own
role/product/component/scope fields; extra top-level fields are ignored only after
complete Document bounds, UTF8 and decoded duplicate-key checks.

Full frozen candidate Schema restricts role/product/component to string/null.
Exact Value comparison therefore reproduces original strict equality without
coercion, normalization, trim, defaults or scope fallback. Complete scope uses
bounded JCS equality with borrowed subtrees, retaining every installationScopeId,
channelRevision, targetKey, channel and role-specific scope field, rather than
comparing a subset. Reordered objects and equivalent finite binary64 numeric
spellings remain equal; extra or missing scope fields and explicit null mismatches
refuse. No large untrusted Value clone occurs.

Successful binding does not authenticate the source of expected facts or the
candidate, prove a signature/threshold/Root trust/time/currentness, persist high
water or grant execution. The caller must supply expected facts from its protected
request/installation context; accepting UI or network self-reports as trusted facts
is outside this helper. Expected raw input is bounded with the metadata Document
profile as a context document, not a credential envelope. Temporary/caller values
have no certified secure-erasure guarantee. Fixed facade errors distinguish
encoding/Schema/expected-resource/binding stages without payload diagnostics.

Four groups cover all20 original independent expectedContext fixtures, every
required field and scalar/null/missing variant, every complete scope leaf including
numeric channelRevision, ignored extras, scope/field reorder, numeric/escape
equivalence, decoded64 KiB strings and exact/+1 1 MiB expected wire boundaries,
UTF8/depth/escaped duplicates and1716 actual Node comparisons. The oracle executes
original extracted parseEnvelope+decodeBase64Url functions with original imports.
A separate bounded parseJson of expectedRaw precedes those functions, making
expected-document resource constraints explicit. Every expected value is independently
supplied or mutated from the original expectedContext fixture, never reconstructed
from the candidate. Accepted typed candidates satisfy complete frozen Schema.
Acceptance comparison is claimed; exact stage/error-category equivalence is not.
The App facade binds both profiles and rejects wrong product, extra scope field
and missing component.

No dependency, lock, Schema, CI, version or gitlink changes are needed. This
isolated new binding helper does not alter the earlier Schema validator or oracle.
Verification includes the unchanged20-role Schema golden/minimal-negative controls;
the unchanged31556-case Schema mutation test is not redundantly rerun in this
batch, having passed root and both independent final-source runs in B15. CI retains
the complete suite. Final scoped validation appears in
[implementation progress](upgrade-implementation-progress.md).
Full T27 authenticity/persistence remains unfinished. Downloads retain no
total/cumulative duration or retry cap.
