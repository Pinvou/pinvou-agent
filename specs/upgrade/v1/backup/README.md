# V1 backup observation semantics

These callable operations are part of the
[platform observation contract](../platforms/README.md). T30 owns the eventual
real backup/key/protected-record producer and original retention transitions.
This package implements no backup producer, restore, cleanup worker or key store.

Every request/result binds `backupId`, `preparationId`, `freezeEpoch`, complete
affected `scopeSha256`, exact ordered `manifestSha256`, protected `recordId`,
opaque `keyReferenceId` and **original** cleanup deadline. Neither content nor
keys nor storage/user destination paths belong in observations. The expected
binding must come from the actual protected owner, not be copied from the result.

| Operation | Precondition and observed result |
|---|---|
| `registerBackup` | Privilege, unique preparation owner, complete writer freeze and consistency already established. Protected record/key reference committed and read back with zero backup bytes written. |
| `createBackup` | Exact registration already exists **before the first byte**, including partial container bytes. Whole affected scope is included and resulting bytes durable. Partial/error results never imply verified success. |
| `verifyBackup` | Recognized version/profile, complete authenticated ordered manifest, all bytes/readback and AEAD final verified, including end/trailing-data checks. No success before authentication final. |
| `exportBackup` | Explicit destination confirmation, independent verified copy and audit; exact original protected binding/deadline retained. Export does not extend platform retention and no automatic upload occurs. |
| `deleteBackup` | Original record and deadline determine cleanup; actual container and protected key absent, complete cleanup readback. Missing one or unknown deletion cannot clear the recovery barrier. |

Each has fixed positive/negative contract-model observations. Registration order,
durability, no-leak guarantees and original provenance require **actual owner
evidence**; checking booleans cannot prove them. All current experimental adapter
production backup calls return `unsupported`, even when its real AES-GCM
primitive probes pass. `notRequired` is a business decision owned outside these
ports and must not fabricate an empty/verified backup or invoke backup creation.

The eventual production format must explicitly version its bounded parser,
framing, authenticated scope/manifest/order/length/lineage, accepted algorithms,
per-key limits and key lifecycle; reject unknown versions, extra or truncated
bytes, object replacement and incomplete/wrong manifest. This semantic contract
does **not** freeze a production byte encoding or substitute a string algorithm
name for that work. The existing `PINVOU-LAB-BACKUP-V0` stream is experimental:
its 60GiB/510000-entry limits are lab limits, its in-memory key is ephemeral and
its source name list is synthetic. It is not a production backup profile or
evidence of protected custody, crash key recovery or full affected-user coverage.

Retention policy (including consumed/pending binding), original-deadline cleanup
and no automatic restore remain in the normative requirements and owner models.
Do not invent a new retention policy, refresh an expired deadline on retry or
mark a nonzero backup obligation satisfied from a zero-custody observation.
