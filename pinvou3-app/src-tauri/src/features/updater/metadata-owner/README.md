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
missing. Ready installations and ordinary host registration reads are unchanged.
This initial state has no production metadata acceptance or public read caller;
it does not change ordinary host registration reads or grant upgrade rights.
Current time, deny information and Root transitions must be integrated before
any accepting writer is exposed.
