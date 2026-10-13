# Client upgrade supplied opening binding

T27 batch11 checks the supplied signed-body Target, JSON Release-envelope hash
string and nonnull opening against frozen assertOpening acceptance rules. All
three inputs use original complete bounded/duplicate-aware Documents. Null is
always rejected, including when a supplied empty commitment matches. Nonnull
opening reuses the existing closed seven-field validation and fixed22-byte
domain/NUL/fixed-array JCS commitment.

Three strict string comparisons bind Target.scope.targetKey to opening.targetKey,
the supplied hash to opening.releaseEnvelopeSha256 and the supplied Target
commitment to the newly computed commitment. Validated opening/hash output makes
matching counterparts canonical lowercase64hex without type coercion, trim or
case conversion. Fields are fetched safely; missing/primitive/scope/hash types
fail without panic. Target is the signed body, never automatically an envelope.
Other Target fields/scope extras are ignored by this function while still passing
the entire raw parser's resource/duplicate checks.

Success is supplied-value consistency only, not authenticated membership,
complete Target Schema, signatures, expected product/component, current selection,
freshness, epochs, qualification or execution authority. Minimal or non-Target
supplied bodies may be shape-positive, exactly as the frozen math helper allows.
The owner performs the complete trust/context/currentness checks. After recomputing
a commitment, changing deployment/rollout revisions/IDs or salt can remain math-
positive without inventing independent selection or entropy rules. Fixed safe
error kinds need not reproduce the original short-circuit error-code ordering.

Four groups cover three independent bindings, every opening leaf with old and
recomputed commitments, null with a correct empty commitment, wrong types/fields/
case/line endings, signed-body versus envelope, equivalent JSON order/escapes,
and all three raw input resource/escaped-duplicate/invalidUTF8 boundaries. An
actual original Node parseJson/assertOpening oracle checks acceptance; existing
rollout commitment oracle retains exact preimage/hash equivalence. An actual
App facade accepts the supplied match and rejects null or wrong Release hash.

No new dependency, lock, Schema, CI, product version or gitlink is needed. Final
evidence appears in [implementation progress](upgrade-implementation-progress.md).
Full T27 trust/selection/high-watermark integration remains unfinished. Downloads
retain no total/cumulative duration or retry cap.
