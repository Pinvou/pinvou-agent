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
