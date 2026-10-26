# V1 platform observation contracts

`contract.mjs` is the canonical executable semantic definition, revision
`pinvou-platform-observations-v1`. It exposes `operations`, `checkObservation`
and a callable `unsupported` refusal. Family modules define closed request/fact
shapes and relational invariants once; the conformance program consumes these
definitions directly. There is no generated second state machine or Rust ABI.
Native adapters remain responsible for their actual language/OS capabilities.

This is an **observation audit contract**, not an RPC, workflow owner or execution
API. `checkObservation` returns defensive DATA. A boolean, identifier, hash,
evidence reference or passing report cannot create authority, prove the truth of
an observation, clear a recovery barrier or authorize a native action. Expected
requests must come from the real owner and protected state, never be copied from
the incoming result. Use the existing host's non-Clone
`VerifiedExecutionObject::handoff(self)` and borrowed `before_action` guard for
actual object transfer; serialized references are not those capabilities.

Input is a bounded parsed JSON record (use the existing `parseJson` for external
bytes), with exactly `operation`, `context`, `request`, `result`. Do not pass live
objects/getters. Context binds installation scope, target, OS, actual host
architecture and normal/forced/silent type. macOS Universal permits either host
architecture but does not certify both. Semantic identifiers are bounded opaque
names, not filesystem paths, user names, secrets, commands or KV keys. An adapter
must keep native handles, capabilities and sensitive facts out of reports.
Canonical IDs use T01's bounded ASCII alphabet, including `:`, without trimming
or accepting trailing line terminators. Hashes and product versions likewise
require a complete lexical match. These expressions do not enable multiline
matching.

## Outcomes and ownership

An `observed` result has exactly `kind`, `effect`, `facts`. Its effect is `none`
for a read/check or `committed` for the operation's stated mutation. `observed`
reports that operation's fact, **not overall upgrade success**: an installer
invocation fact says nothing about its exit, installation or health result.
Individual field schemas and invariants are in the corresponding modules.

Failure results have exactly `kind`, `effect`, `reason`, `faultEvidenceId`.
The implementation freezes the closed reasons:

| Kind | Meaning and owner obligation |
|---|---|
| `unsupported` | No implementation for this port/target; no effect |
| `prerequisite_unavailable` | Required qualification, owner, registration or duration absent; no effect |
| `refused` | Definite pre-effect refusal (permissions, interaction, stale owner, incomplete scope, identity, consistency, budget, format or safety); no effect |
| `unavailable` | Dependency unavailable, not a business conflict; no effect |
| `time_unknown` | Anchor/continuity/remaining time unproven; no effect, pause dependent action |
| `confirmed_fault` | Positive actual fault evidence required; no effect, fault evidence reference required |
| `outcome_unknown` | A mutating operation may have committed; `possible` effect, reconcile original operation, never blindly repeat |

All non-fault failures require a null fault reference. An arbitrary string in a
fault reference still does not prove a fault. Definite failures after an earlier
boundary belong to the execution owner's subsequent result/reconciliation, not a
retroactive `refused` result for that invocation. The existing transaction model
and eventual T36 owner retain manual-repair/late-success semantics. Failure
schemas do not grant permission to skip an operation's real prerequisite checks.

Protected transaction owners record their original request/revision before
effects; re-open the same operation after an unknown result. A matching readback
must establish commit, and a definite CAS refusal must precede any write. This
package audits reported facts; it neither implements a store nor proves TxR,
ACLs, anti-rollback or crash persistence from data supplied to it.

## Callable operations

Each operation has a fixed accepted observation and distinguishing counterexample
in `vectors.mjs`. The focused tests add related cases. The program checks the
registry and vector set are exactly equal, preventing silent uncovered additions.

| Operations | Required facts / boundary | Native owner |
|---|---|---|
| `detectInstallation` | Exact requested installation scope; installed version/identity or both absent | Installation adapter |
| `normalizeVersion` | Canonical three-part numeric product version; Windows `major.minor.patch.0` with 16-bit words; other OS file version null | Version adapter |
| `verifyArtifact`, `verifyExecutableIdentity` | Exact package/size/hash/trusted metadata; full read/format or exact role/generation plus retained actual object and immediate recheck | Existing artifact/object owners; no mandatory native certificate |
| `acquirePrivilege`, `verifyExistingHelperPrivilege` | Explicit observed consent only for normal/forced acquisition; existing helper exact identity and rights without any prompt | Native privilege adapter |
| `readProtectedPartition`, `commitProtectedPartition` | Fixed owner partition and exact installation scope; bounded record identity, revision, content digest; precommit guard and committed readback | Relevant protected owner |
| `acquireGlobalLock`, `recoverOwnership` | Exact preparation, scope, owner epoch, exclusive durable ownership | T28 live preparation owner |
| `persistPreparation` | Same original start and identity; CAS revision increments once, authentic readback | T28 |
| `transferOwnership`, `fenceOwner` | Exactly next epoch; old owner invalidated/fenced; no stale cleanup/unfreeze of a newer owner | T28; observations do not implement fencing |
| `coordinateWriters`, `freezeAffectedScopes`, `verifyDataConsistency` | Complete exact affected writer inventory; all saved/stopped/consistent at same freeze epoch; actual admission fence and readback | T28 plus actual writer adapters |
| `TrustedDeadlineProvider` | Exact boot/policy, full UTC interval, proven continuity and no outstanding cleanup barrier | Existing protected time owner |
| `persistStageBudget` | Original window/start/duration/deadline immutable; current duration/Session cutoff only tighten; effective deadline is their earlier bound; spent high-water cannot decrease, even above budget; original revision increments | Existing budget owner; numeric facts do not prove a clock |
| `handoffVerifiedExecutionObject`, `recheckBeforeAction` | Exact object/generation/size/hash/metadata, consuming prior owner and retained native custody through the action | Existing host object owner; not this DATA validator |
| `runDirectInstaller` | Boundary is actual native invocation, not later file change or exit | T32/T36 execution owner |
| `inspectInstalledVersion` | Full active identity equals expected target | Installation adapter |
| `preinstallInactiveSlot`, `verifyStagedSlot`, `cleanInactiveSlot` | Exact slot/revision/digest; isolated no-active-write preinstall; full verification; cleanup cannot touch active slot | Slot owner |
| `activateStagedSlot` | Earliest first active-data write or pointer switch, including write-before-switch | T36 activation owner |
| `launchAndHealthCheck` | Exact active identity/scope; required migrations complete, consistent data, local readiness/storage/components, no fatal loop; original total <=5min, <=2 attempts, each <=120s | T36 health owner |
| Backup operations | See [backup semantics](../backup/README.md) | T30 nonzero backup/key/retention owner |

No download stage or total deadline exists. Approved initial preparation policy
is coordination/freeze120000ms and backup1800000ms, configured by the protected
host's existing build inputs. These are not execution qualifications. Existing
original windows survive retry, session/config change, crash and reopen without
renewal. Cross-boot unknown time pauses and conservatively clears query secrets;
fresh authenticated time must finish original-deadline cleanup before resume.
The validator requires original window equality but cannot prove persistence or
native time provenance. The lab's same-boot stopwatch measures only an experiment.

Health facts describe local readiness only; missing external login/network/model
services do not alone mean local health failure. Process-alive alone, inactive
slot boot and a late exit code cannot supply these facts. The probe must not make
paid requests, upload or mutate business content.

## Run and interpret

```powershell
npm --prefix specs/upgrade/v1 run test:platforms
node tools/upgrade-platform-conformance/conformance.mjs model
```

The pure model entry works on Node22+. The independent program also has bounded
Node26+ actual primitive probes, described in its
[README](../../../../tools/upgrade-platform-conformance/README.md). A model pass,
experimental primitive pass, unsupported production port and unattempted native
workflow remain distinct. No whole T03/T26, installed workflow, platform
qualification, CI, merge or release approval follows from this report.
