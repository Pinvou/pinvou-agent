# Durable metadata acceptance QA workflow

This independent operator tool runs real V1 signature/reference/window/graph and
high-water checks, then saves the original metadata bytes and high waters in one
ordinary SQLite transaction. A new process opens the same state and refuses
rollback or equal-version different envelope bytes. It is an isolated QA
workflow, not the production protected metadata owner. T27 remains in progress.

`production` explicitly refuses with
`PROTECTED_METADATA_ACCEPTANCE_UNSUPPORTED`. No result is connected to the App
updater, download jobs, consume, installation or an execution-object constructor.
Production control HTTP remains fixed to its existing origin and is unchanged.

## Run

Build the standalone leaf using its committed lock:

```sh
cargo build --locked --manifest-path pinvou3-app/src-tauri/src/features/updater/protocol/acceptance/Cargo.toml
```

The resulting `pinvou-upgrade-metadata-qa` accepts these exact commands:

```text
init NEW_DIRECTORY INDEPENDENT_ROOT_FILE EXTERNAL_ROOT_ENVELOPE_SHA256 SUPPLIED_QA_TIME_MS
accept DIRECTORY EXTERNAL_ROOT_ENVELOPE_SHA256 BUNDLE_FILE
inspect DIRECTORY EXTERNAL_ROOT_ENVELOPE_SHA256
original-after DIRECTORY EXTERNAL_ROOT_ENVELOPE_SHA256 CANONICAL_CONTEXT_CURSOR EXPECTED_GENERATION
original DIRECTORY EXTERNAL_ROOT_ENVELOPE_SHA256 ACCEPTED_ENVELOPE_SHA256 EXPECTED_GENERATION
production
```

`init` requires a new directory and an independently acquired original complete
Root envelope plus its external SHA256 pin. It verifies the actual self threshold
at the explicit QA time. Never derive that pin from the incoming bundle or offer
an automatic reset after a missing/corrupt state. An existing directory is
refused, including after a failed initialization; no cleanup/repair is implied.

Bundle JSON is a private QA descriptor, not a frozen V1 API. Its exact fields are
`product`, `component`, `suppliedNowMs`, `roots`, `timestamp`, `snapshot`, `members`,
`deniedKeyIds`, and `deniedByRole`. Every path is a single input basename relative
to the bundle's parent directory. It is never an output/database filename.
`roots` is the ordered new contiguous successor batch (empty for unchanged Root,
otherwise at most 4096). `members` must be the exact complete Snapshot inventory,
with no omitted/duplicate/extra object. Deny inputs and time are explicitly
supplied QA facts, not an attested production deny source or trusted clock.
Member order and input basenames are not metadata identity. Repeating the same
already-current originals, including the same ordered Root successor bytes,
rechecks the newly supplied time and denies without advancing generation.

Each actual input object retains the original one-MiB/decoded64KiB/depth32/4096
item profile. The original whole envelope bytes, including signatures, determine
identity; they are never reconstructed from the signed body. No whole-store,
active-component-count, lifetime-history,32MiB cache or artifact download cap is
added. The CLI does not perform HTTP/file downloads or alter their timing policy.

`inspect` returns a minimal current QA state summary. `original-after` returns one
high-water original envelope frame after a canonical context key (use the empty
string initially), or null. Each call requires the generation returned by
`inspect`; a changed generation refuses rather than silently continuing a mixed
export. `original` reads an exact accepted historical hash, including Package
which has no version high water. These private output frames may encode one
original one-MiB object as a string: their derived output cap is twice that object
limit plus4096 wrapper bytes, not a relaxation of any protocol input limit.
Original historical bytes are not a fresh online qualification.

## Actual transaction and recovery

The store uses SQLite `BEGIN IMMEDIATE`, foreign keys, a fixed checked schema,
`synchronous=FULL` and the DELETE rollback journal. It verifies the entire existing
history before accepting more input. The actual locked generation is compared
again when publishing. Original objects, all applicable role/context version and
hash records, Root batch evidence and the component's active-set association are
one commit. Intermediate writes never produce a success or usable partial set.

Root history retains genuine committed batch boundaries. Each historical batch
is replayed at its retained historical supplied time solely for integrity
diagnostics. Expired arbitrary4096-step chunk tails are never treated as fresh
Root transitions. A new batch starts from the actual accepted Root, requires a
fresh final Root, and scans all database-owned active components/members for key
compatibility. The caller cannot substitute an empty active set. Historical
component/keyset cursors read a bounded object at a time; no lifetime-history or
all-component Vec is constructed. The graph relationship source shares the same
codec kernel as the existing in-memory API.
Temporary coverage/high-water B-trees are file-backed. Fixed schema definitions
and every stored field have type/byte-length gates before Rust materialization;
these checks do not turn SQLite's file parser into a hostile-input sandbox.

Lower versions and equal-version different original envelopes refuse. Root is
per product, Timestamp/Snapshot per component, Target per complete target scope,
and Release uses its releaseId/revision identity. Removing a current member never
deletes that member's retained high water. Package remains immutable-hash bound
without inventing a version high water. A byte-identical already-current set is
an idempotent repeat after current verification; it does not advance generation.

A definite precommit refusal rolls the whole transaction back. A commit/IO or
postcommit response uncertainty returns a fixed uncertain error and no guessed
new accepted result. Reopen determines the actual complete old/new state. A
process killed after a successful commit can leave the entire new state even
though it never printed its response; “all failures imply zero writes” would be
incorrect. No fail-open initialization or corruption repair is provided.

## Boundaries

The database, directory and clock are ordinary QA resources. They do not prove
system-protected custody, hostile-user namespace/ABA safety, antirollback against
replacement of the **whole valid database**, OS boot continuity, trustworthy
time, physical power-loss durability, publisher identity, current Registry or
Artifact qualification, or installation permission. A checksum or adjacent HMAC
key cannot provide those missing facts. SQLite's path-based VFS is not the NT
relative-handle custody adapter. T26/T28/T31 must supply actual production owner
and time capabilities before a production adapter can exist.

Tests generate ephemeral QA Ed25519 keys in memory using the frozen independent
reference. Native files, SQLite commits, separate child processes, locking,
restart and process kill are real. Fixture metadata does not certify a native
publisher or production Deployment. Default tests do not require network,
administrator permission, service installation, a certificate store or OpenBao.

From this leaf directory, run the complete collection/process tests with
optimized artifacts. CI uses the same release profile for these tests because
large genuine histories repeat complete cryptographic validation; formatting
and clippy checks keep their ordinary development profile. All cases, assertions
and `fault-tests` behavior remain enabled.

```sh
cargo test --locked --release --all-targets -- --test-threads=1
cargo test --locked --release --all-targets --features fault-tests -- --test-threads=1
```

`fault-tests` is an explicit isolated-QA test-only build option. It exposes fixed
SQL stage and per-acceptance persistent-DML notifications and waits for test stdin;
no guard/signature/source is
replaced. Parents kill and join the real process before reopening SQLite, or send
`lose-ack` only after actual COMMIT. These tests demonstrate process-level
transaction recovery; they do not claim a machine power interruption or physical
network failure.
