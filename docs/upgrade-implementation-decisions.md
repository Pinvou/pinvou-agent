# Upgrade implementation decisions and certification proposal

Date: 2026-10-09. Status: health, strict time policy and backend stack approved;
platform/data scope, measurement plan and physical evidence remain pending.

This supplements the [V1.0 task decision register](pinvou-upgrade-platform-development-tasks.zh-CN.md#10-实施前需要确认的决策).
It does not change R/D/S/C/F/M/K, approve new support boundaries, or complete T03.
The inventory below is based on repository source, not inspection of user profiles,
credentials, browser contents or production data. Paths are symbolic local examples.

## Confirmed decisions

The product owner requested a platform/data inventory and proposals; preparation
budgets must follow a test proposal, actual measurements and approval of concrete
values. The suggested local health criteria are approved. Strict trustworthy time
option A is approved: ordinary offline operation and shutdown are not fault exceptions.
The backend stack is approved: Go, Gin, pgx and PostgreSQL 17; React, TypeScript
and Vite for administration; PostgreSQL durable jobs/transactional outbox, OIDC
with self-hosted Keycloak available for community deployment, and separate signing
and file-storage ports. The product owner selected the separate `UpdateServer`
repository. Its private hosting address stays in local Git configuration, not in
committed artifacts. No server project is created in this client repository.
Previously approved Ed25519 thresholds, input limits and absence of a cumulative
download deadline remain unchanged.

T05 management decisions are also approved (2026-10-09): explicitly initialize
the intended immutable OIDC identity through a separately authenticated
provisioning command and a protected server permission table. There is no default
administrator, first-login enrollment or request/JWT self-declared role. Grants
are explicit per product/component/channel/target, with null as non-applicable.
Use a dedicated provider ACR that proves password plus OTP; the service verifies
the signed authentication level and authentication time, requiring MFA within
the latest five minutes by default. Expiry requires step-up again. The actual
realm flow must prove it cannot issue this ACR with password alone; signed test
IdP fixtures do not certify a deployed Keycloak OTP configuration.

## Platform certification candidates

Approve this as the initial test coverage list, not as a claim that these systems
already have certified upgrade capabilities. Record exact edition/build/patch,
hardware, filesystem, privilege model, installer/helper/launcher identities and
time-provider evidence for every certified combination.

| Frozen targetKey | Proposed OS coverage | Host and package | Current evidence |
|---|---|---|---|
| `windows-x86_64-nsis-perMachine` | Windows 10 22H2; Windows 11 24H2 and 25H2; exact editions/builds recorded at test time | x64, NSIS, machine installation | R §13.2 specifies Windows 10/11; Windows overlay specifies NSIS perMachine |
| `linux-x86_64-deb-perMachine` | Ubuntu 22.04 LTS and 24.04 LTS with standard updates | x86_64, DEB, machine installation | R §13.3; README requires glibc 2.35+ and WebKitGTK 2.40+ |
| `linux-arm64-deb-perMachine` | Ubuntu 22.04 LTS and 24.04 LTS with standard updates | aarch64, DEB, machine installation | Same dependency floor; native arm64 execution must be tested |
| `macos-universal-dmg-perMachine` | macOS 11, 12, 13, 14, 15 and 26 on hardware supported by that OS | Intel and Apple Silicon separately, Universal DMG, machine installation | Existing configuration sets minimumSystemVersion 11.0; R §13.4 requires both hosts |

Windows edition choices, LTSC variants and other Windows 11 versions still require
explicit inclusion; they are not silently approved or removed from existing app
support. Other Linux distributions, Ubuntu 26.04, Windows arm64, MSI and per-user
packages are not additions to this first upgrade certification proposal. A system
absent from the certification list is not evidence of unsupported application use.
The macOS list preserves the existing app minimum, but does not imply every version
can run every newly required helper/runtime. Verify bundled dependencies and signing
identities on each row before claiming compatibility; disclose incompatible cases.

Every row must cover normal/forced direct installation and silent inactive staging
followed by restart activation, required/no-backup cases, multiple users, disconnected
sessions, background services, host crashes and original-budget recovery. Linux must
also exercise dpkg contention/partial configuration and relevant display/GPU variants;
macOS must check each Universal slice, Team ID, notarization and privilege deployment;
Windows must check Authenticode and machine ACLs. Existing ad-hoc macOS development
signing is not production identity or certification evidence.

Initial physical environments: lowest approved OS per architecture, each other
listed OS major, and each distinct certified time provider. Virtual machines can
verify logic and races but cannot substitute for physical clock/power-loss evidence.
All four targetKeys remain candidates; lack of proven time capability blocks their
certification rather than silently reducing modes or dropping a target.

Source anchors: [README](../README.md), [Windows overlay](../pinvou3-app/src-tauri/config/platforms/windows/tauri.conf.json),
[Linux overlay](../pinvou3-app/src-tauri/config/platforms/linux/tauri.conf.json),
[macOS overlay](../pinvou3-app/src-tauri/config/platforms/macos/tauri.conf.json).

## Data inventory and proposed protection policy

Define the final inventory per installationScopeId and exact migration plan.
Enumerate all affected users (including logged-out/disconnected accounts), service
identities, configured roots and writers. `PINVOU3_HOME` can relocate the app root;
one interactive user's default home is not the machine inventory. Canonicalize and
validate roots, aliases, symlinks/reparse points and filesystem/object identities.
Do not recursively follow an arbitrary link into another user's entire home.

The default proposal is to preserve every persistent app-owned object. Include all
affected non-rebuildable data in required backup; grant exclusions only through an
explicit migration/certification inventory. Unknown entries are not cache by default.
Unchanged external resources are preserved, not copied wholesale or modified by the
upgrade. If a plan affects them, expand the protected inventory before approval.

| Data group / symbolic location | Observed owning modules and writers | Proposed treatment |
|---|---|---|
| `settings.json`, `acp-providers.json`, `session-agents.json`, `selected_pet.json`, `pet_window.json`, disabled bundle/connector/skill settings and connector marker files | Preferences, ACP stores, marketplace, pet and connector state | Preserve configuration and compatibility; include affected files in encrypted backup |
| `sessions/` including JSON records, sidecars, artifacts, workspaces, checkpoint shadow repositories, review/scene/timing/steering records | SessionStore, assistant engines, ACP, checkpoint and file tools; independent CLI/headless agent/eval hosts boot the same SessionStore | Non-rebuildable; cover associated files as one consistent scope, not just session JSON; stopping the GUI alone is insufficient |
| `eval/runs/` including manifest, event/prediction history and protected `private/predictions` payloads | pinvou-cli benchmark RunStore and private prediction store; independently running eval host and child tools | Preserve affected run/resume records and private payloads in encrypted consistent backup; stop/fence independent writers and validate recovery before restart; do not restore execution-lock bytes as live ownership |
| `user/`, `notes.md`, `memory.md`, `workspace/` | User skills/personas/instructions, memory organizer, assistant/tools | Preserve user-authored content; generated memory artifacts require evidence before exclusion |
| `projects/`, `scheduled/`, `scheduled-runs/`, `automations/model-bindings.json`, `tasks/` | Project store, scheduling/task engines and their child processes | Preserve definitions, run history, read state and task workspaces; stop/fence autonomous writers |
| `knowledge/index.db` and actual SQLite journal/WAL state; `knowledge/remote-connections.json` | KnowledgeService, scanners/indexers, remote knowledge configuration | Database contains non-rebuildable data; use consistent SQLite snapshot/checkpoint discipline, not raw independent copies of a live DB/WAL |
| `bundle/`, `bundles/`, `marketplace/` including import journal/recycle bin; `assets/`, `connectors/`, `runtimes/` | Bundle extraction, marketplace import/install, dependency/CLI installers | Preserve user changes, manifests, locks and recovery journals; exclude only verified immutable reconstructible payloads with a proven reconstruction path |
| `asr/`, `knowledge/models/` and configured model-directory overrides | Model managers/downloaders; ASR/embedding processes | Candidate payload exclusions only if unchanged and independently reconstructible; custom files/configuration are not excluded automatically |
| `browser/restore`, `browser/webview2-profile`, and the actual application WebView profile outside the app root | Browser host, WebView child processes, frontend localStorage/IndexedDB | Preserve persistent page/login state and UI preferences; quiesce profile writers and retain OS identity/ACL constraints; never export cookies into telemetry |
| `feedback/`, `draft-attachments/`, `uploads/`, `web-session-downloads/`, `computer-use/`, `logs/` | Feedback/outbox, file tools, remote control, audit and diagnostics | Pending user content and protected audit are not cache; classify logs by existing retention rules, not a blanket exclusion |
| `web-access.json`, `web-access-pending-revocations.json`, `web-access-rpc-ledger.json`, `web-relay.json` | Remote-control manager, background networking and RPC ledger | Preserve secrets under encryption and replay/revocation history; never revive a revoked capability through an upgrade backup |
| OS credential-store references and entries used by model/MCP/ACP/remote-knowledge/shared-host features | `platform/credential_store.rs`, CodeWhale secrets backend | Preserve identities, access controls and references; do not dump the entire keyring. If migration changes a secret, prove an allowed protected backup/migration method or reject that migration |
| Effective `CODEWHALE_HOME/secrets/secrets.json` or `~/.codewhale/secrets/secrets.json`; applicable legacy `~/.deepseek/secrets/secrets.json` import source | SystemCredentialStore normally falls back to the foundation file store when OS keyring probing fails; app and independent foundation/CLI users can share this root | `PINVOU3_HOME` does not relocate it. Preserve untouched; if affected, declare exact files/entries and all writers, protect a consistent encrypted snapshot without reviving deleted credentials or copying entire external directories. Legacy import conditions follow the existing backend |
| External provider-managed portions of `~/.claude/settings.json`, `~/.codex/config.toml`, and the effective Kimi config root (`KIMI_CODE_HOME` or `~/.kimi-code`) | ACP provider writers and independently running external CLIs | Normally leave untouched. If affected, explicitly inventory exact files/fields and all writers; preserve unrelated provider/user state; no blanket copy of external auth directories |
| Connector-specific auth/state outside the app root, including `~/.config/wecom`; other CLI roots resolved by their adapters | Connector CLI processes and their authentication flows | Preserve untouched; verify actual roots per version/OS. A migration must declare its precise scope and must not log credential values |
| Linux `/var/lib/pinvou-knowledge`, service configuration, `/var/lib/pinvou-knowledge-model` and actual shared-host data roots | System knowledge server, root helper and systemd; independent network clients | Include affected service data/identity/configuration under its own permission and consistent-snapshot contract. Models may be excluded only under the same reconstructibility rule |
| External workspaces, imported original documents and remote knowledge servers | User tools, external applications and remote services | Not blanket upgrade-backup roots. A plan that modifies local external files must name them and coordinate their writers; remote data is not backed up by this desktop task |
| Future upgrade protected state, backups, staging, helper/launcher versions, accepted metadata and recovery records | T26–T39/T45 owners | Separate from business backup so no recursive backup. Preserve original deadlines, ownership/fencing, terminal and deny facts; never restore stale rights or prohibited recovery-secret copies |

Ephemeral PID/port files, process locks, browser automation tokens and temporary
host/MCP mappings are candidates for recreation after safe owner recovery; restoring
their bytes must never restore old ownership. Identify these separately from durable
operation journals and pending work. Rebuildable WebKitCache is a candidate exclusion;
localStorage, IndexedDB and persistent login profiles are not the same cache.
Independent agent/eval hosts, CLI benchmark runners and all their child tools join
the affected-writer inventory even when the GUI is absent. Their inherited roots,
session/run ownership and restart paths must obey the same save/stop/fence proof.

No new data is uploaded by this inventory or by the approved health checks. Backup
contents and secret-bearing manifests stay protected locally, including manual export
constraints from R/S. The final scope manifest includes source/target plan identities,
roots, owners, data classifications, exclusions, snapshot consistency method and
writer/freeze evidence. It must be approved before an irreversible migration.

Source anchors: [paths](../pinvou3-app/src-tauri/src/platform/paths.rs),
[sessions](../pinvou3-app/src-tauri/src/features/sessions/store.rs),
[knowledge](../pinvou3-app/src-tauri/src/features/knowledge/store.rs),
[credentials](../pinvou3-app/src-tauri/src/platform/credential_store.rs),
[credential fallback](../CodeWhale/crates/secrets/src/lib.rs),
[CLI data root](../pinvou-cli/crates/cli/src/lib.rs),
[eval run store](../pinvou-cli/crates/benchmark-core/src/store.rs),
[private predictions](../pinvou-cli/crates/benchmark-core/src/private_prediction.rs),
[headless shared store](../pinvou3-app/src-tauri/src/features/assistant/product_runtime/headless_bridge.rs),
[provider roots](../pinvou3-app/src-tauri/src/features/codex_acp/providers/mod.rs),
[UI cache boundaries](../pinvou3-app/src-tauri/src/platform/ui_cache.rs),
[shared-host helper](../pinvou3-app/src-tauri/resources/platforms/linux/knowledge-host/pinvou-knowledge-host-helper).

## Preparation-budget measurement proposal

No certified millisecond value is proposed yet. Approve the measurement plan first;
run the T03 experimental adapters on representative physical systems, then submit a
per-target table of measured distributions, worst observed time, safety margin,
supported workload envelope, user wait cap, chosen budget and failure behavior.
Contract fixtures, file-copy throughput and this development machine alone cannot
measure actual save/freeze, encrypted backup or crash-safe resume capability.

The bootstrap sequence avoids depending on later production tasks for T03's inputs:

1. Within T03, publish an explicitly experimental adapter interface revision v0
   and an independently runnable consistency/measurement program. This names an
   unpublished experimental interface, not a new wire protocolVersion. Build
   isolated experimental adapters that use real OS storage, process/locking,
   encryption, durability and time-provider primitives with synthetic writers/data.
   They do not wait for T28/T30/T33–T35 production workflows or write app business state.
2. Measure actual experimental behavior and failure handling on the declared
   hardware/filesystems/time provider. Record exact prototype identity, scope and
   limitations. Submit concrete initial contract budgets and capability boundaries
   for owner approval; synthetic-writer results do not prove the real app's writers
   already implement cooperation or that a production package is certified.
3. Freeze the T03 v1 interfaces, bounded preparation policy and consistency tests
   only when those initial values are approved and the strict time path has real
   evidence. An unproven time provider remains unproven, not an assumed capability.
4. T28/T30/T33–T35 and related tasks implement the production workflows/adapters
   against v1, then repeat measurements with the real app, users and services.
   Exceeding the approved budget fails certification; revised values require
   explicit approval and contract compatibility review, never an automatic increase.
5. T46 validates exact production binaries/package and complete workload/mode
   evidence. Experimental results cannot substitute for that certification or T43.

Use disposable test profiles/volumes and dedicated lab devices. Explicitly isolate
both PINVOU3_HOME and CODEWHALE_HOME before any host boot. Explicit CODEWHALE_HOME
also prevents the backend's ambient legacy-secret import; verify that condition.
Credential experiments use a dedicated synthetic namespace/test store and must not
probe or modify real OS keyring entries. Moving PINVOU3_HOME alone is insufficient.
Tests that change system date, power state or service ownership run only on the
dedicated lab system, not the developer's normal workstation.

Proposed synthetic profiles (not approved product data-size limits): small 1 GiB /
10,000 files, medium 10 GiB / 100,000 files, large 50 GiB / 500,000 files; include
both many-small-file and few-large-file variants, representative SQLite/WAL,
session/checkpoint state, user-owned bundles and service data. Use synthetic secrets
and content only. Larger/unsupported workloads must receive a safe explanation and
may not be silently truncated to fit a budget.

Measure at least 30 successful repetitions per baseline profile/platform/storage
class with cold and warm caches, recording sample count, median, p95 and maximum.
Repeat slow-path cases separately; do not treat a percentile as a hard worst-case
guarantee. Product approval must bound the supported workload and accepted wait.
SSD and slower supported storage, free-space pressure, CPU/I/O contention, encrypted
volumes and antivirus effects belong in the matrix where applicable. Network or
removable data roots require an explicit tested capability; they are not assumed safe.

| Phase | Measurement start/end | Required adverse cases |
|---|---|---|
| Coordination and freeze | First actual coordination under preparation ownership through all affected writers saved/stopped and frozen-scope evidence persisted | Two active users, disconnected user, logged-out user data, background schedule/indexer/service, new writer arrival, unsavable state, inaccessible required root |
| Backup preparation and verification | First actual backup preparation through encrypted consistent snapshot and verified manifest, with original budget anchors persisted | Partial write, low space, DB/WAL activity, corrupted object, lost key access, helper crash, reboot, stale owner revival, interrupted export |
| Preinstall/direct install/activation | Separate first-start anchor through each mode's defined completion or safe/manual-repair result | Object replacement, links, installer lock/partial failure, pointer switch races and earliest execution boundary |
| Local health | First new-version startup attempt through all mandatory local health evidence | Unresponsive UI, startup loop, data/identity mismatch, missing required component, offline external services, one allowed retry and reboot without budget reset |

OS privilege/user-consent prompt waiting occurs before freezing; measure it
separately and do not disguise it as execution progress. Short-lived server
authorizations are separate and follow each purpose's required local preparation;
silent does not automatically request OS elevation. Coordination/backup preparation remains
bounded by the earlier of the certified budget and the current session's remaining
time. Actual retries, user changes, crashes and reboot never reset first-start or
consumed time. Complete verification remains at most 30 minutes; preinstall/direct
install/activation each at most two hours; these existing maxima are not selected
target budgets. Health keeps 120 seconds per check, one permitted new-version
restart retry, and five minutes total including starts/waits. Download has no total
or cumulative time budget.

Failure cases must prove no discarded user state, no stale-owner unfreeze and no
second executor. Before the execution boundary, follow confirmed cancellation or
unknown-consume retirement/reconciliation rules; after it, converge forward safely
or enter manual repair. A timeout never proves consumption was refused. forced
gates remain governed by R, not by the benchmark runner.

## Approved local health criteria

Require the actual active version, native identity and launcher entry to match the
consumed target; mandatory migration and all affected-data compatibility must pass.
The new active process must remain alive and answer its local readiness/UI probe,
open current-user configuration/data safely and initialize mandatory local components
without startup loops, corruption or fatal errors. Check other affected users' data
compatibility without requiring them to log in. Probes must not modify business
content, issue paid model calls, or upload it. Unavailable optional dependencies or
external services are not mandatory local components merely because they exist.
Explicit identity/data errors fail immediately; retry cannot hide them. Network or
account unavailability alone is not local health failure. A failed original
transaction remains failed even if a separate higher forward repair succeeds.

## Strict trustworthy time research and certification gate

Option A preserves R §17.1: signed anchors plus OS wall time are insufficient by
themselves. Offline shutdown duration must be provable, with uncertainty handled
without early deletion or extended normal retention. A component that never had
this capability cannot use `retention_time_fault` as its initial implementation.

| Primitive | Documented property | Gap for strict certification |
|---|---|---|
| Windows interrupt time | Since last system start; ordinary date adjustments do not change it | No independent proof of elapsed ordinary power-off duration |
| Linux CLOCK_BOOTTIME | Monotonic since boot and includes suspension | A new boot/power-off interval needs an independent trusted source |
| macOS mach_continuous_time | Includes sleep in its continuously increasing counter | Does not itself establish trustworthy elapsed powered-off time across boots |
| TPM2 readclock | TPM powered-on time and reset/restart information | Powered-off elapsed time is not a battery-backed trusted UTC clock |

Research candidate hardware/platform mechanisms with an independently protected,
power-off-capable time source and verifiable continuity, integrity, uncertainty and
failure detection. Do not claim a generic RTC, TPM, online time server or VM clock
provides that mechanism. No concrete portable mechanism is currently certified.

The test matrix must cover date jumps forward/back, sleep/hibernate, normal reboot,
shutdown across before/equal/after boundaries, clock-source damage, persisted state
tampering and recovery without resetting original deadlines. Observe elapsed time
with an independent laboratory reference; the test device remains offline and must
clean expired objects before permitting its first network connection. Short-boundary
primitive tests and model tests do not replace required long-duration/accuracy and
failure evidence. Cover backup/key/staging rules and immediate recovery-secret
cleanup on confirmed time fault independently; no secret is retained merely to
permit an extended query window.

T03 owns the experimental interfaces/adapters and measurement program described
above as well as its final semantic contract and consistency tests. Final
contract/capability freeze requires approved initial target budgets and an explicit
proven time path; later production measurements and certification remain mandatory.
T26/T33–T35/T46 cannot claim implementation/certification through fixtures. Under
the chosen policy, unsupported hardware blocks certification; any support-policy
change must return to the owner as a separate decision.

References: [Windows interrupt time](https://learn.microsoft.com/en-us/windows/win32/sysinfo/interrupt-time),
[Linux clock_gettime](https://man7.org/linux/man-pages/man2/clock_gettime.2.html),
[Apple mach_time.h](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/mach_time.h),
[TPM2 readclock](https://github.com/tpm2-software/tpm2-tools/blob/master/man/tpm2_readclock.1.md).

## Approved backend technology and repository boundary

The product owner approved Go + Gin + pgx + PostgreSQL 17 for the control service,
and React + TypeScript + Vite for the management application. The product owner
provided the separate `UpdateServer` repository on 2026-10-09; it was cloned and
confirmed empty before engineering. Its private address is retained only in local
Git configuration. Pin maintained Go/framework/driver versions and lock
dependencies when engineering starts; this decision installs no dependencies here.
Go supports straightforward HTTP/worker development and deployment; React aligns
with the desktop frontend. C's consistency and R/S security requirements remain
mandatory regardless of implementation language.

Use a modular service with domain ownership following T05–T23/T44/T47 and narrow
ports; workers can run separately while sharing the same transactional owner
records. No microservice split that distributes one required atomic write set.
Clients load the same published V1 contract version and fixed vectors; backend
ports/checks are implemented in Go, not by running desktop Node. Go canonicalization,
signatures and digest bindings must match T01/T02 byte and behavior vectors;
ordinary encoding/json output alone is not RFC 8785 canonicalization evidence.
Publish a versioned contract artifact with revision/digest and verify no drift in
both repositories. Do not create a second independently edited normative spec.

| Concern | Approved stack choices / required constraints / pending infrastructure |
|---|---|
| Atomic business state | PostgreSQL transactions, domain serialization/row locking, revision/hash CAS and unique constraints; explicitly lock inventory/fanout domains or use SERIALIZABLE with bounded safe retry; no application-only check-then-write |
| Durable operation/timeout/event work | Transactional operation/outbox/result/timeout records and PostgreSQL worker queues using ownership epochs; retries are at-least-once with idempotent exact bindings, not claimed exactly-once delivery |
| Production acknowledged-state RPO=0 | Primary plus synchronous durable standby in independent failure domains; synchronous_commit=on with nonempty synchronous_standby_names, optionally remote_apply when applied visibility is required. Fence the old primary and promote only a standby proven to contain all acknowledged commits; do not silently downgrade to asynchronous replication when quorum is lost |
| Authority reads | Guard and consume-status reads use the current authoritative primary; replicas/caches do not grant execution rights or substitute for final CAS |
| Community/local deployment | Runnable isolated service + database + admin + local object storage and optional self-hosted identity provider with synthetic keys/data. Single-node development does not claim production HA/RPO=0 or production signing protection |
| File storage | S3-compatible immutable byte-addressed storage port; community local-filesystem adapter with durable atomic writes. Production supplier is pending proof of immutability, retention/archive and durability. Validate complete bytes before publishing references |
| Publication versus file storage | Durable verified immutable blobs first; one PostgreSQL transaction publishes complete authoritative bytes/references, both heads, generations, openings and audit/outbox. No distributed DB/object-store transaction is assumed; orphan pre-publication blobs are safely collectible, published material is protected from deletion |
| Identity and MFA | OIDC provider port, recommended self-hosted Keycloak for community use; server enforces current scope/RBAC and evidenced MFA. Two distinct approved identities and exact change digest/revision remain a separate dual-approval requirement |
| Signing | Dedicated signing port/service with scoped Ed25519 roles; Root 2-of-3 independent protected keys and separately approved rotation. Production KMS/HSM supplier must prove Ed25519 support, independent custody and long-term verification retention; development software keys do not qualify |
| Admin application | Domain workflows include draft, review, MFA/dual approval, publication, diagnosis and recovery; read/write permission checks are on the server. Do not expose kernel dispatch or broad client-provided projections as trusted owner facts |
| Monitoring/deployment | Structured secret-safe logs and OpenTelemetry; health/readiness and metrics. Containerized local development, explicit production HA/fencing and restore runbooks; deployment provider/scale/cost remain separate choices |

RPO=0 above refers to the approved failure model and acknowledged authoritative
state. Losing every synchronous durable copy is outside that protection. Backups
and async cross-region replication alone do not prove RPO=0. Artifact/archive
durability has its own required evidence and cannot inherit a database guarantee.
Unknown commit responses recover via the original operation/authorization identity;
they never imply zero writes or authorize a new consumption.

Verify against the frozen T01/T02 package plus real PostgreSQL concurrency/fault
tests: consume/cancel/expire races, commit ACK loss, failover, scope inventory
phantoms, denied-key replay, two-head publication, hidden opening isolation,
lease takeover, timeout/outbox recovery, expired status/tombstones and quality
watermark races. Validate actual immutable storage and current identity/MFA/signing
providers separately. Cross-repository E2E completion remains T43's responsibility.

PostgreSQL evidence: [synchronous replication](https://www.postgresql.org/docs/17/warm-standby.html#SYNCHRONOUS-REPLICATION),
[synchronous_commit](https://www.postgresql.org/docs/17/runtime-config-wal.html#GUC-SYNCHRONOUS-COMMIT).

## Decisions to request after review

1. Approve or amend the exact OS coverage list; it is a test commitment, not existing certification.
2. Approve or amend the data-protection policy and inventory, including conditional external/service scopes and exclusions.
3. Approve or amend the synthetic workloads, storage classes and measurement process; actual budgets return for approval after measurement.

Strict time option A, local health criteria and the backend stack are already
approved, and the separate `UpdateServer` repository has been supplied; these are
not asked again. Hardware/time mechanism research remains technical work; unresolved
feasibility must be disclosed rather than treated as approval to relax the policy.
