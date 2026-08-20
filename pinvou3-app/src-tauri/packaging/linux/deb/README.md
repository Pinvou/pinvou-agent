# Linux Supervisor deployment acceptance

The generic deb remains inert with respect to the MegaBook app resource profile. It installs the
fixed profile and launcher as package assets, starts only the user Supervisor socket for online
sessions, and does not enable the app unit or restart ASR.

## MegaBook profile helper

`/usr/lib/pinvou3/supervisor/pinvou-megabook-profile` accepts exactly one of `activate`,
`deactivate`, or `status`. It has no path, unit, property, PID, or command input and never touches
ASR. Both activation and deactivation require `pinvou3-app.service` to be stopped: its state must
be `Inactive` or `Failed`, with `MainPID=0`.

Activation uses a two-phase v2 ownership transaction. It first publishes an `installing` marker,
then publishes each hash-pinned target separately, reloads the user manager, verifies the fixed base
unit identity, restart policy, `DropInPaths`, 4/8/2-GiB policy and a trusted Supervisor `Status +
Reconciled` receipt, and only then publishes `applied`. Each file publication is atomic, but the
group is not one filesystem-wide atomic step. Publications use no-clobber hard links, file and
parent-directory fsync, and post-publication inode/mode/hash/link-count checks. Staging lives only in
fixed, private `0700` namespaces. A strict six-character staging name with the expected uid, regular
file type, mode and one link is an unpublished helper-owned reservation, so the same command may
remove an empty or partially written interruption residue. A two-link residue is removed only after
its fixed public target is proven to be the same inode with the final expected hash and metadata.
Unknown names, metadata or link relationships are preserved and reported with their recovery path.

The complete v1 cleanup ABI is frozen, not only its marker. It comprises the package sources
`/usr/share/pinvou3/supervisor/profiles/{megabook-canary.conf,pinvou3-megabook-canary.desktop}`,
their per-user targets under `.config/systemd/user` and `.local/share/applications`, their exact
bytes and hashes, plus the exact path, bytes and hash of
`.local/state/pinvou3/megabook-profile-v1.registered`. New activation never rewrites or silently
claims v1 state: an exact legacy marker remains supported by `status` and `deactivate`, while all
new transactions use distinct v2 marker names and bytes. Any future profile, desktop or marker
content change must use new v2 asset, installed-target and marker paths, while retaining the v1
bytes and cleanup allowlist.

Deactivation retains the marker while removing the two targets, reloads the user manager, rechecks
that the app is inactive, and retires the marker last. It never directly unlinks a prevalidated
public path: each fixed target is atomically renamed without overwrite to a same-directory hidden
quarantine, revalidated there, and only then unlinked. An editor's concurrent atomic save is
preserved; mismatched quarantine content remains at the named recovery path and fails closed.
A concurrent start or interruption therefore leaves a registered, recoverable state.
Always run `deactivate` (normally through `prepare-purge`) before removing the package: after purge,
the hash-pinned package sources and helper are intentionally unavailable.

## Acceptance harness

Run `pinvou3-app/scripts/megabook-supervisor-e2e.sh` from the matching source checkout. The harness
does not invoke `sudo`; the only privileged boundaries are the real install and purge:

```sh
./pinvou3-app/scripts/megabook-supervisor-e2e.sh baseline "$HOME/pinvou3_VERSION_amd64.deb"
sudo apt-get install --no-install-recommends "$HOME/pinvou3_VERSION_amd64.deb"
# Choose one acceptance mode. verify-memory-max includes the verify-safe path.
./pinvou3-app/scripts/megabook-supervisor-e2e.sh verify-safe
# OR, instead of verify-safe, use the disruptive calibrated 32-GiB path:
./pinvou3-app/scripts/megabook-supervisor-e2e.sh verify-memory-max
./pinvou3-app/scripts/megabook-supervisor-e2e.sh prepare-purge
sudo apt-get purge pinvou3
./pinvou3-app/scripts/megabook-supervisor-e2e.sh verify-purged
```

Do not run both verification modes against one pre-install baseline: each mode deliberately
restarts ASR only after proving that package installation did not, so the full memory mode must be
selected directly when High/OOM evidence is required.

The v3 pre-install baseline binds the selected deb's SHA-256, the control archive's complete
maintainer-member set and install-behavior fields, the generated dpkg path list, the control
`md5sums` bytes, and a canonical manifest of the 12 executable, unit, descriptor, profile and
desktop files used by the acceptance path. Before it executes any installed helper or Supervisor
binary, verification recomputes those attestations from the unchanged deb; requires the installed
dpkg control members, generated `.list`, and optional `.conffiles` to have the exact root-owned
mode, size and bytes; rejects unexpected `pinvou3.*` database members; requires
`dpkg --verify pinvou3` to return success with no findings; and compares every critical installed
path as a root-owned, regular, non-symlink, single-link file with the same mode, size and SHA-256.
This proves the selected critical payload, every md5sums-listed payload file, generated package
path list, maintainer controls and tracked installation fields are behaviorally identical to the
unchanged baseline deb. It deliberately does not claim that dpkg state can reconstruct or prove
which byte-for-byte compressed archive crossed the privileged install boundary; version and
architecture equality alone remain insufficient evidence.

Both verification modes require the app to begin explicitly `inactive` with `MainPID=0`, record the
socket and Supervisor service as either active or inactive, and restore and recheck those initial
states on exit. A failed or transitional initial app/service state is rejected rather than silently
normalized. `verify-safe` additionally proves that every memory-test drop-in, loader, go/once marker
and evidence file is absent before its first Launch. Only `verify-memory-max`, after the calibrated
host gates below pass, is allowed to stage those assets.

`verify-memory-max` refuses hosts outside its fixed cgroup-v2, approximately-32-GiB, no-swap, and
`MemAvailable` gates. It uses only hash-pinned runtime fixtures. An `ExecStartPost` child is born
inside the app cgroup, writes ready evidence, and waits for a fixed go marker; the harness releases
it only after checking the 4/8/2-GiB systemd and cgroup policy, `memory.oom.group=1`, the app,
loader and a real WebKit process in one cgroup, a trusted below-high Runtime baseline, and the
Supervisor in a separate stable cgroup. The harness uses fixed private staging namespaces and the
same one-link/two-link recovery rule as the helper, so a validated interruption residue can be
retired and its parent directory fsynced before the next run. Unexpected residue is never deleted.

The High and Max phases use separate app `InvocationID`s and are cleaned between phases. Evidence
checks bind the current ledger prefix and append window, the Resource → Claim → ASR directive → ACK
→ reconcile chain, Supervisor Pending and terminal tombstone, journald's old-invocation
`snapshot-app` receipt, OOM counter deltas, one bounded restart, and Supervisor survival. Trap
rollback stops the fixed app unit, removes only transaction-owned or exact hash/content-matching
test assets, reloads and resets the initially-inactive app unit, deactivates the profile, and
restores and verifies the captured active/inactive socket, Supervisor and ASR states. It never
attaches or kills an arbitrary PID.

These commands are an acceptance procedure, not evidence that MegaBook execution has already
happened. Record the real command results separately when the two sudo boundaries are authorized.
