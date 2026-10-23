# Windows bootstrap host component

This standalone leaf owns the original-source first installation, the single LocalSystem SCM helper, and the ordinary-user launcher. Application wiring is opt-in through the mutually exclusive `upgrade-host-lab` and `upgrade-host-production` features. A build without an embedded bootstrap profile refuses host operations. The build embeds independently supplied public bootstrap Seed/Root bytes; runtime JSON, a manifest, or a caller PID cannot construct an installed/source context. This batch supports Windows x64, NTFS, per-machine NSIS only. Other platforms return `HOST_UNSUPPORTED`.

The standalone helper and launcher require a separate leaf build with an explicit `--target x86_64-pc-windows-msvc` and `-C target-feature=+crt-static` applied consistently to all target dependencies. Their binary entrypoints reject a Windows MSVC build without this target feature. Use an isolated role target directory and supply the resulting two PE images to `PINVOU_HOST_HELPER_BINARY` and `PINVOU_HOST_LAUNCHER_BINARY`; these flags must not be applied to the GUI project build. Normal and fault profiles use the same CRT policy, with separately bound Seed/artifact bytes.

Both role images also carry `/DEPENDENTLOADFLAG:0x800`, restricting static DLL imports to System32 before Rust entry on supported Windows versions. Later dynamic loads retain their independent runtime hardening. Packaging must inspect the actual role PE load policy and reject dynamic VC/UCRT imports, and must verify exact file identities, sizes and hashes. Native code-signing certificates are optional; metadata signatures and authenticated source bindings remain mandatory. The GUI and NSIS images retain their own build configuration. A Windows loader refusal is not the helper's prerequisite exit code 15: the standalone roles must reach their own checks even when GUI VC prerequisites are absent, allowing authenticated same-source maintenance.

The two standalone executables use closed arguments:

- `pinvou-upgrade-helper --profile` describes public compiled inputs only.
- `pinvou-upgrade-helper --install-local` accepts no additional argument. An already elevated local administrator explicitly selects the complete package in that helper's parent directory; the command holds and validates its files, creates the fixed protected source, and starts only that retained installer. It does not request elevation or authorize a metadata upgrade. Existing installations admit only the exact original source for repair. A partial protected source is preserved and refused.
- `pinvou-upgrade-helper --service` enters the real SCM dispatcher only.
- `pinvou-upgrade-helper --installer-stage <stage>` accepts only `prepare-install`, `prepare-local-install`, `prepare-metadata-upgrade`, `verify-files`, `start-service`, `prepare-remove`, `remove-target`, or `launch-desktop`, from the actual independently protected original source installer. `prepare-metadata-upgrade` currently refuses with `InitialTrustUnavailable` until its authenticated producer is implemented; it never falls back to local installation.
- `pinvou-upgrade-launcher` starts the fixed registered app as its ordinary interactive user. Its only additional mode is `--restart-handoff <restart-id>`, authenticated against the actual original association.

The private manifest version 2 binds the complete application inventory through at most eight separately parsed shards (each at most 1 MiB and 4,096 files). Root and every shard are embedded before NSIS archive/CRC construction, and atomically persisted with the original registry intent. Missing, changed, swapped, extra or wrong-language shards refuse. Unpublished inline manifest version 1 is not adopted. The assembled inventory is never serialized as one oversized metadata input.

The original source installer establishes a native intent before target creation. Files are created with protected security descriptors, flushed, reopened and checked by retained identity/hash. SCM creation starts DISABLED, checks the real default security, seals the service, and only then configures automatic startup. Service RUNNING is separate from the authenticated registration Ready transition. The registry stores synchronous transacted CAS history; an uncertain post-commit outcome remains unknown. Recovery preserves the original scope, Root, creation history and epochs. Unknown files, references or conflicting services are preserved and refused.

The same first-intent native registry transaction initializes the independent metadata and preparation owners. It stores their immutable, scope-bound genesis and the exact original Root, then reopens and verifies the committed values. Original Root self-threshold signatures are checked at their signed issuance time solely for anchor integrity. This does not establish trusted current time, current metadata acceptance or online upgrade qualification. Initial ownership has zero allocated epochs and no preparation, freeze, execution or reconciliation occupancy. Retries never recreate missing genesis or reset existing state. Old lab registrations remain distinguishable and readable without claiming these owners were initialized; production requires the complete owner set. Removal retains the original anchors and tombstone.

First-install admission only reads installed prerequisites and legacy markers. The VC++ floor comes from the actual selected MSVC linker toolset. Machine-wide WebView2 Runtime must already exist. Both machine registry views and every loaded ordinary-user hive are checked for known legacy Pinvou installations. Every user profile in the actual machine ProfileList must have a readable loaded hive; offline, unknown or inaccessible profiles return unsupported/unavailable, rather than being assumed clean. No hive is loaded automatically and no legacy uninstaller is executed. This bootstrap does not migrate or qualify user data. Existing same-source repair and removal do not require missing GUI prerequisites.

App restart preserves one authenticated association while existing app cleanup runs. A real matching live launcher must be ready before cleanup and exit commit. Only a definite matching commit acknowledgment within its original monotonic grace permits app exit. Cancellation, timeouts, process death and lost replies cannot create a new grant or trigger fallback. The broker never launches a GUI as System. Optional installer finish dispatch uses the real ordinary desktop Explorer automation route; success means dispatch only, not an observed child or health.

`native-acceptance-hooks` is a separate, default-off guest fault-test feature requiring a distinct lab fault profile. It opens two fixed protected native event pairs at the actual post-CreateService and post-seal/pre-AUTO boundaries. It accepts no runtime fault arguments, paths or environment switches. Instrumented profiles are not shipping acceptance evidence. Independent normal-profile installation/restart/repair/removal scenarios include the supported unsigned-package path. Environment-only gaps are recorded as unverified under the project's non-blocking E2E policy; completing the implementation does not certify those physical scenarios.

Fixed English diagnostics contain no sensitive paths or native error payloads. Exit codes: 0 success, 10 disabled/unsupported/trust unavailable, 11 invalid identity/context/input, 12 busy/referenced, 13 corrupt/repair required, 14 outcome unknown, 15 unavailable/incomplete prerequisites. No service, certificate root, clock or machine registry test changes are permitted on the development host.

This workflow does not establish Artifact validity, production Publisher mapping, ApprovedRegistry, current upgrade qualification, trusted wall time, data freeze/backup readiness, or install/consume permissions. Those owners remain independent.

## Retained download byte staging

Trusted in-process composition can create `DownloadedPackageSource::new(file,
package_envelope, root_claims_pin, supplied_historical_time)` and call
`stage_retained_download(&source, NativeRole::FinalInstaller, &limits, cancelled)`.
The source is untrusted borrowed input, not cache ownership or retired writers.
A cache caller retains its `ObservedCache` and borrows its file and Package
inputs for the synchronous call; the host does not depend on the QA workflow or
its HTTP runtime. `ExecutionHelper` and `Launcher`
are the other closed roles. There is no new IPC/CLI request, cache slot producer,
automatic updater or install handler. The ordinary job remains QA/support data;
the host independently rechecks its signed Package under the compiled original
Root. Exact product/component/target expectations come only from the compiled
Seed, and artifact facts come from that same verified Package. A different
Root-claims pin refuses; no rotation chain is inferred or adopted. Original Root
self-threshold integrity uses signed issuance time only, and the retained Package
time remains a supplied historical semantic input, never trusted current time.

`StagingLimits::new(archive_limits, reserve_bytes)` takes validated `ArchiveLimits`
and a positive disk reserve from trusted composition. It does not impose a fixed
package-size ceiling or a total download deadline. Available disk space
must cover the full package, selected native member and reserve before creation;
it is checked again before extraction and at completion. This is an actual
capacity observation, not a reserved or authenticated preparation budget. A full
or concurrently consumed disk fails without returning a capability. The caller
must supply its cancellation/validation-budget poll; staging and full `recheck`
latch cancellation and check it throughout streaming operations. They do not
start or reset a budget. Native certificate-provider calls are checked before
and after return, rather than being unsafely terminated. Cancellation preserves
partial bytes and returns `HOST_BYTE_STAGING_CANCELED` without a capability.

The fixed private namespace is `%ProgramFiles%/Pinvou/UpdaterDownloadStaging/`
`<compiled-profile-id>/bytes-<generated-id>/` for production, or the separate
`UpdaterDownloadStagingLab` namespace for lab. Every invocation exclusively creates
a new leaf. Existing or partial leaves are never adopted, repaired, overwritten
or automatically removed. It is a sibling of T31 code/source/state inventories;
normal T31 verification and uninstall do not scavenge these objects. Unknown
objects retain the original refusal/retention policy.

The source FullPack is checked before copying. The protected copy is flushed,
closed, reopened by the held parent and checked by object ID and complete hash.
The complete protected FullPack is inspected again, and the exact declared native
role must match one unique safe ZIP member; duplicate byte-identical candidates
refuse. The native copy is likewise flushed, reopened and verified, with the
existing PE-machine and certificate policy. All extraction uses the original
bounded safe-archive parser. No ordinary source hash substitutes for protected
destination readback.

`ProtectedByteCapability` retains the protected FullPack/native files, ancestry,
object identities and signed bindings. It offers size/hash/role facts, bounded
reads (at most 64 KiB per call) and a full cancellable `recheck`; it has no Clone/Serialize, raw path/handle, execution
or installation API. It is **not** a `VerifiedExecutionObject`, a current
qualification, a publisher mapping, a preparation/freeze owner or an installation grant.
Missing current T27 capabilities must still refuse at any future execution
composition. This component does not add resume/source renewal. Unsupported
platforms return `HOST_UNSUPPORTED`.

The ordinary host library tests cover genuine Root/Package signatures, independent
envelope/claims pins, scope and threshold failures, historical Root expiry,
ambiguous native selection, resource policy and cancellation of a real file hash.
The actual protected-copy, ACL/reparse,
disk-exhaustion and replacement scenarios require an isolated Windows environment
with an independently embedded public profile and real signed FullPack input;
they do not require installing a service on the development machine. No such
environmental scenario is implied by library compilation or pure-test success.

## Same-process execution-object ownership

`ProtectedByteCapability::into_execution_object(cancelled)` consumes the staged
byte capability and completely rechecks its protected files and ancestry before
returning a `VerifiedExecutionObject`. The object verifies identity/custody only;
it does not establish a current non-denied key, T27 eligibility, T28 preparation
or freeze, T32 consume, installation permission or an execution grant. Historical
Package authentication under the original compiled Root remains historical.

`ObjectBinding` is derived inside the actual staging adapter from the same
signature-checked Package and compiled Seed. It retains the Root-claims pin,
original Package-envelope hash, compiled product/component/target/profile/Seed,
appVersion, FullPack packageId/size/hash and exact native role/claim/size/hash.
`ObjectGeneration` includes that complete binding and both actual protected file
identities, including creation fields. It is not a PID, generated directory name,
wall-clock token, preparation epoch or persisted adoption permission.

`ObjectReference` can be cloned as read-only identification data. It has no public
constructor or deserialization and never keeps an owner alive. Use
`owner.handoff(&expected_reference, cancelled)` to consume and move the same owner:
an exact reference/generation/binding match and complete physical recheck precede
the returned owner. All original protected handles remain continuously held.
Failure returns no replacement owner and leaves retained staging unadopted.

`owner.before_action(&expected_reference, cancelled)` performs the same checks
and returns a narrow borrow guard holding `&mut` access to that owner. The guard
offers only the reference and bounded byte reads. It does not expose a raw path,
handle, launch/install method or arbitrary execution closure. Future T31/T36
composition must retain the owner/borrow and separately prove its own current
action authority. A cloned old reference never substitutes for that borrow.

There is no Clone/Deserialize implementation on the owner, cross-process
handoff, restart adoption or reference registry. Process exit ends the capability;
the retained directory is not a way to resurrect it. Cancellation is mandatory
and latched around physical rechecks, without creating or resetting a budget.
Pure tests cover identity-creation generations, complete metadata mismatches,
same-reference transitions, cancellation and physical-check refusal; they do
not create fake protected capabilities or claim native E2E. The `handoff`
compile-fail doctest checks that the moved old owner cannot be reused.

The build-time Seed accepts explicit `lab` and `production` kinds with the same trust and content checks. Production uses a stable installation `profileId`, independent of each `buildId`. Its service, pipe, source/code/app/state directories, registry keys and mutexes are separate from lab even for the same profile ID. Existing lab names and serialized policies remain unchanged. Production rejects fault profile IDs and `native-acceptance-hooks`; there is no default production Root or runtime profile selector. The project NSIS wrapper requires `PINVOU_HOST_PROFILE_KIND=production` to match a production Seed. See [packaging](../../../../../../docs/windows-host-lab-packaging.md) for its explicit public inputs and source-deployment prerequisite.

The component's library regression is run independently of the GUI build:

```sh
cargo test --manifest-path pinvou3-app/src-tauri/src/features/updater/host/Cargo.toml --offline --locked --target x86_64-pc-windows-msvc --lib -- --test-threads=1
```

Passing these tests or compiling the standalone roles does not qualify SCM installation, authenticated IPC, multiple users or crash recovery. Those scenarios require the isolated Windows guest; do not install or exercise the service on the development host.
