# Preparation ownership bootstrap

This leaf defines immutable scope-bound genesis for the first-install native
transaction. Both allocated epochs start at zero; preparation, freeze, execution
and reconciliation occupancy are absent. Its values describe the original empty
state, not a live ownership lease, writer permission or freeze capability.

The protected host adapter atomically creates the original intent and both
owners, then reopens and checks all committed bytes. Existing genesis must match
exactly. Advanced epochs, occupancies, partial data or another scope are refused;
initialization never clears or rewrites them. Future acquisition, handoff,
recovery and freezing require their own real owner operations and storage.

## Live preparation reducer

`live` defines a bounded canonical ScopeHead and individually stored canonical
OperationRecord values. LiveOwnership is a nonserialized reducer view of one
selected operation. There is no lifetime history array or attempt cap. The head
retains per-installation monotonic owner/freeze epochs and the exact current
operation/revision reference. Each operation retains its original Session,
request lineage, inventory, protected PrepFreeze budget origin and active/closed
outcome. A closed operation remains independently addressable after later ones.

LiveOwnership::load consumes the adapter's ExistingOperation and BudgetPresence
read results. NoPrior is valid only when both the operation and original T26
budget are genuinely absent in the same protected transaction. Partial, orphaned,
mislabelled or mismatched pairs are CorruptReadSet, never first acquisition or
healing. Native absence must include the real T26 window/index integrity checks;
a caller's assertion or a missing ordinary file is insufficient. Reuse T26's
actual operation/window identity rather than minting a parallel budget key.

Transitions expose a narrow write_intent containing next_head and one
operation_write. The native adapter must atomically persist both with the actual
original budget update and admission state under ONE borrowed TxR, using exact
previous-byte CAS, protected original Session/current binding/inventory and other
owner checks, then exact committed readback. Existing Store::budget_window opens
its own TxR and cannot simply precede a separate owner publication. Immutable
InitialOwnership genesis remains unchanged and is provenance only. Missing live
records never trigger initialization or budget recreation.

Context, ACK and receipt structures are DATA, not native grants or authenticated
facts. The private adapter constructs them from protected original Session and
held native objects; it must not accept caller-populated receipt hashes or saved
booleans. Writer ACKs include their actual owner/freeze epochs and exact inventory
revision/hash. Frozen requires complete exact saved/drained ACK coverage plus a
separate native fence observation. Preinstall allocates no freeze epoch and does
not coordinate active business writers. There is no download acquisition API.

Unknown or exhausted budgets preserve occupancy and any held fence. A fresh known
clock alone does not resume. Explicit resume keeps the physical fence, transfers
owner and applicable freeze epochs, and requires ACKs naming those new epochs.
Delayed ACKs from the prior coordination cycle cannot satisfy the new cycle.
Recovery fences old executors before transferring epochs, clears old ACKs,
preserves the original budget and enters Holding. Safe cancellation publishes
Releasing before any physical fence release. Completion must name that exact
published safety snapshot; its separate release receipt supplies fresh
post-release convergence/readback. The native admission gate remains closed
throughout. Crossed/unknown boundaries or other execution/reconciliation/unknown
occupancy never release the owner.

Per-record limits are 128 participants, 256 KiB and canonical safe-integer epochs
and revisions. Capacity refusal never deletes a prior operation or original budget.
There is no automatic history garbage collection and no fixed lifetime operation
count. Actual host/client integration and physical fencing remain required before
the complete preparation workflow can be delivered.
