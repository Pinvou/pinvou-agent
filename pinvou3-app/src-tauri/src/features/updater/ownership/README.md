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
