# Metadata bootstrap owner

This leaf defines the immutable original anchor initialized by the Windows host's
first-install transaction. The native adapter derives the scope binding and the
independent Root pin from its original installation context. The module checks
the complete canonical Root and real self-threshold signatures at its own signed
issuance time, then defines the exact bounded genesis record and Root bytes.

That deterministic check proves anchor integrity only. It reads no system clock,
creates no protected storage and returns no current-chain acceptance or online
qualification. A later trusted metadata check must use its genuine time and
protected high-water owners; supplied issuance time cannot replace them.

Existing genesis is recognized by exact canonical bytes, Root and scope binding.
Partial, changed or different-scope data is refused, never adopted or reset.
Active metadata state belongs to subsequent owner operations, not this bootstrap
interface. The ordinary SQLite QA store is not used as a production backend.

On Windows, a new installation also creates `metadata-state-v1` in the original
registration transaction. Its initial head and history bind the exact anchor,
including the original Root envelope hash and version; the original Root bytes
are stored in the same transaction. A post-commit reopen verifies this initial
state. Missing state is never initialized by a read. Existing installations do
not acquire this namespace automatically and require a separate migration.
On retry of a revision-one first-install intent, the installer rechecks the
committed initial state before resuming or taking over. An old unfinished
revision-one installation is indistinguishable and also refuses if the state is
missing.
This initial state has no production metadata acceptance or public read caller;
it does not grant upgrade rights. Host registration reads also validate the
protected product registration catalog and fail if its required state is absent
or invalid. Ordinary reads verify the current product's protected local history
and members without reading another product's records; initial registration
still checks the shared machine catalog before updating both in one transaction.
The protected per-product enrollment anchor detects a product history changed
without the corresponding anchor; it is not an independent anti-rollback
source if both are restored together. It is not the product Root high-water.
The first installation of a product also creates its protected `root-state-v1`
genesis, Head and first history record in the same registry transaction. They
bind the actual signed original Root's product, version, size and digest to the
first protected product enrollment. Product reads and original-intent recovery
recompute and check those bytes, and refuse missing or changed state. This is a
genesis-only owner: no current Root advancement, shared product Root authority,
metadata acceptance or trusted time is established by this record.
Current time, deny information and Root transitions must be integrated before
any accepting writer is exposed.

The Windows time publisher may advance its protected Root only after it proves
the same profile's complete metadata namespace is exactly the original empty
layout in its held registry transaction. Missing, damaged or unexpected state
refuses that Root transition; no-Root-change time updates are unaffected. This
is an initial-state guard, not active metadata acceptance. Before introducing an
active metadata writer, Root publication must instead authenticate the complete
active set and atomically coordinate both owners under the same transaction.
The protocol's Root high-water is product-wide; this per-profile guard does not
establish a product-wide census. A production writer must establish that wider
ownership and fail closed whenever it cannot prove the complete set.
