# Upgrade implementation progress

Task source: [V1.0 development tasks](pinvou-upgrade-platform-development-tasks.zh-CN.md).
Tasks are completed individually only after in-scope independent code review,
repairs, applicable tests and commit/push. Contract completion is not production
client, service, physical platform certification or rollout readiness.

| Task | Status | Delivery / outstanding dependency |
|---|---|---|
| T01 | Implemented; independently reviewed and verified | Executable metadata/credential Schemas, byte/trust profile, reference verifier, independently verified fixed vectors and local CLI E2E |
| T02–T47 (including final T43) | Not completed | Follow the task graph; no runtime or certification completion is claimed |

Confirmed implementation decisions (2026-10-09):

- Ed25519; initial Root 2-of-3, other roles initially one valid signature; scoped configurable roles and separate high-risk dual approval.
- Metadata 1 MiB, credentials 64 KiB, depth 32, decoded string 64 KiB, per-container 4,096-member bounds.
- Service/admin engineering is in a **separate repository**. Repository location and backend technology will be provided later. No server project is created here.

T01 adds Ajv 8.20.0 only to the isolated contract package; dependency audit reported
no vulnerabilities at installation. Desktop behavior and runtime dependencies are
unchanged. Future service tasks cannot be marked complete through client fixtures.

The first implementation batch safely merged the then-current origin/main and
aligned CodeWhale with the parent gitlink. Continue task/review work on that base
without repeatedly syncing merely because main advances.

T01 validation: 41 tests passed on Node 22 and the current local Node release;
the architecture guard passed. Both independent reviewers rejected the first
implementation, the primary role repaired every finding, and both passed the
second review. Python independently verified all 20 fixed envelopes, signing
inputs, hashes and Ed25519 signatures. No production service, desktop update
workflow, installer or physical platform certification is claimed.

A parallel review run exposed a 15-second CLI subprocess test timeout; isolated
and serial reruns passed. The test-only subprocess allowance was raised to 60
seconds and rechecked. It does not set a network or download time limit. Remaining
tasks retain their dependencies, including the separate backend repository and
technology decisions that will be supplied later.
