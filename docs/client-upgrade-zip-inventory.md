# Client upgrade read-only ZIP inventory

T29 batch 5 supplies a private ZIP32 inspector and App facade. It scans an already
open regular file protected by its owner and returns a complete observed inventory
only after all structure, entry bytes and original package bytes pass. It never
extracts a file, reopens a path or grants execution rights. Constructors establish
only field shape, not authenticated Package metadata or current qualification.

## Compatible format boundary

The V1 Manifest binds a complete packageId/size/hash and native byte identities;
it has no complete list of internal member filenames. This module does not invent
one or require the outer upload's Manifest/SBOM/provenance inside FullPack. Safe
additional data members remain bound by the whole container hash. Locating native
claims by actual direct-member identities, OTA meaning, selecting FullPack and
native authenticity remain later owner integration.

The initial structural subset follows the separate server inspector: ZIP32 Store
and Deflate, supported creators, ASCII V1 relative paths, UTF8 flags with ASCII
names, supported timestamp extra, and 12/16-byte descriptors. ZIP64, split/prefixed
archives, unsupported encodings/extra interpretations and special/link/reparse
types fail explicitly. There is no all-provider or platform certification claim.
Internal ZIPs are treated as opaque data files and never recursively unpacked;
this inventory does not inspect their nested structure.

## Physical structure and content

The raw scanner finds a unique EOCD whose comment ends exactly at source EOF.
It bounds and checks physical central records before building entry objects, keeps
every original name and rejects duplicate/case aliases and file/directory prefix
conflicts. Portable path rules refuse escape, backslashes, Windows device names
and trailing dots. Implicit parents may later acquire an explicit consistent
directory record. Central order can differ from physical local order.

Sorted local records must continuously cover byte zero through central start;
prefixes, gaps, overlap and hidden local content fail. Version, flags, method,
timestamp, name, CRC, sizes and descriptors match their central records. Directories
must be empty Store entries with no descriptor. No general ZIP SDK normalization
or duplicate folding supplies this evidence.

Each regular member streams through fixed 64 KiB buffers. Actual expanded size,
real entry EOF/Deflate stream end, CRC and SHA-256 are checked. Low-level Deflate
steps accept bounded input/output; raw reads and codec steps recheck cancellation.
The decoder's exact consumed input count must equal the declared compressed span,
so valid decoded bytes followed by hidden compressed tail are refused. Raw package
bytes are checked with B3 before parsing and again after all members. These scans
can detect visible changes but cannot replace exclusive ownership or detect every
transient race. No partial inventory escapes after a later entry failure.

## Resource and owner boundary

The owner explicitly supplies positive single-file/aggregate expanded capacities,
entry count, physical metadata, path-state accounting and compression ratio; no
production defaults are guessed. The initial implementation bounds count at 4,096
and physical metadata/path-state capacities separately at 8 MiB. These are parser
resource profile ceilings for later qualification, not wire Schema limits or a
32 MiB artifact ceiling. EOF comments, central/local headers and descriptor bytes
count toward physical metadata. Path prefix strings and a conservative per-node
allowance count before registry insertion, avoiding unbounded deep-path expansion.
Declaration and actual streaming size checks both apply; ratio never replaces
real size/CRC/hash. Test capacities certify no production target or resource budget.

The owner runs this synchronous function on a blocking worker, maintains the same
original T26 validation budget through every scan and uses cancellation for
unknown/fault safe stop. A currently executing OS operation or bounded codec step
cannot be interrupted by the predicate. No download total/cumulative duration or
retry cap is added. Fixed errors retain no raw names, paths or underlying IO/codec
sources. Physical staging protection, safe extraction and native/installation
rights remain later work; full T29 and deployment remain unfinished.

## Verification

Python independently constructs raw ZIP records using struct/zlib and computes
container/member hashes with hashlib. Its standard zipfile SDK verifies every
positive inventory without importing Rust code; negative fixture outer hashes
are also recomputed, so they actually reach the parser after B3. A streamed fixture
constructs a real 33 MiB Store archive from bounded blocks instead of storing its
entire bytes in JSON. The mandatory CI oracle checks golden bytes and positive
interoperability without requiring byte-identical compression across zlib versions.
Python/JSON-only fixture changes route to the standalone Rust gate.

Focused groups cover 48 independent wire vectors, explicit quotas, original hash/
IO failure and all observed cancellation poll boundaries. Cases include positive
Store/Deflate/descriptors/flags/timestamps/directories/central reorder, 33 MiB Store
and Deflate, paths/aliases/types/ZIP64/split, local mismatch, gaps, CRC/length failure,
compressed tails and a later corrupt entry. Final checks and independent review
are recorded in [implementation progress](upgrade-implementation-progress.md).

Final validation passed all three groups with 48 vectors (7.28s), Python's complete
48-container/positive-inventory oracle, standalone fmt/Clippy, 49 CI policies,
architecture, version, diff/YAML and complete locked App/CLI graphs. Each parent
lock adds only twelve lines for the local package and App edge; no external package
version changes. The 30-package isolated lock matches parent versions/sources/
checksums. Actual Windows App integration passed all twelve updater tests (0.02s
after a 2m10s incremental build), including real Store/Deflate success and exact
CompressedTail refusal. No App-wide or physical platform pass is claimed.

Independent review found a material initial verification gap: the standalone
crate's default flate2 miniz backend differed from the App's existing zlib-rs.
The primary role explicitly aligned the standalone crate with existing zlib-rs
0.6.7, reran all vectors and added those App Deflate controls. Baseline Windows,
Linux x86_64/arm64 and macOS arm64 metadata graphs already selected zlib-rs, so
this correction changes no tested App graph's existing decoder choice. These are
dependency graph checks, not native OS certification. Both roles independently
passed the repaired backend's three groups (8.10s and 12.40s respectively).
Earlier miniz runs are not counted as actual App decoder verification.
Both independent roles gave final B5 scoped PASS after reviewing the backend
repair, actual App evidence and final documentation; no mandatory finding remains.
