# Windows offline package build adapter

This is a post-build project entry point. Build native inputs with the existing
project command (`npm run build:nsis`), then invoke:

```text
npm run upgrade:pack -- --config ABSOLUTE_PRIVATE_CONFIG --out ABSOLUTE_NEW_DIRECTORY
```

The native build and this packaging invocation are separate completion points.
An installer returned by a product build is an input, never evidence that a
Package, Artifact or installation was approved. Existing or separately built
small native inputs can test this tooling without waiting for final T31/T46
hosts. No native publisher certificate is mandatory.

The adapter composes the service's existing `upgrade-pack.exe`:

1. Copy stable exact inputs into a new private output root.
2. `upgrade-pack prepare` emits FullPack, unsigned claims and signing input.
3. Invoke the explicitly pinned standalone external signer with exactly
   `--input SIGNING_INPUT --claims UNSIGNED_CLAIMS --output NEW_SIGNED_MANIFEST`.
4. `upgrade-pack assemble`, with the independently pinned offline Root and time,
   verifies the external Manifest's real signature and exact freshly rebuilt body.
5. `upgrade-pack verify` checks the deterministic upload independently.
6. Recheck all original input identities/bytes, snapshots and generated outputs;
   exclusively publish `completion.json` as the final operation.

The signer must be a standalone trusted `.exe`, accept those fixed positional
flag pairs, write its new canonical envelope exclusively, return nonzero on any
failure and not detach background work. There is no shell or arbitrary-argv
configuration. No private key, certificate password, account or provider token
is accepted by this adapter. Provider authentication belongs to the externally
controlled signing tool. Child environment excludes inherited provider variables,
`NODE_OPTIONS` and `NODE_PATH`; working TEMP/TMP are under this new private root.

Cancellation attempts to stop the known direct child and awaits its actual
`close`. It never publishes completion on cancellation, and never asserts that
arbitrary descendants have been closed. CI qualification requiring complete
native Job custody must use the established outer OwnedJob collector. No new
total elapsed product or download deadline is imposed here.

## Closed local configuration

Supply exactly these top-level keys:

| Key | Value |
| --- | --- |
| `version` | `1` |
| `packTool`, `manifestSigner` | Exact `{path,size,sha256}` references to standalone `.exe` files |
| `claims`, `sbom`, `provenance`, `root` | Exact `{path,size,sha256}` references to existing immutable files |
| `at` | Explicit offline verification Unix milliseconds, a nonnegative safe integer |
| `inputs` | Exact role rows described below |
| `limits` | Existing Windows subset of `packaging.Limits`, detailed below |

Every path is absolute, size is a positive safe integer and SHA-256 is exactly
64 lowercase hexadecimal characters. Root bytes and digest must come from the
operator's independently trusted build configuration, not the supplied upload.
The unsigned V1 claims must select `windows-x86_64-nsis-perMachine`, match the
project's actual `0.10.13` version and retain production `incrementalPackages:[]`.
The underlying CLI owns complete claims Schema validation and required helper
inventory. It allocates no package identifier: supply one through the real build
owner. This tool never invents a global package ID or publisher identity.

Each input row has exactly `{role,helperId,name,file}`. `file` is an exact file
reference. Roles are `finalInstaller`, `helper`, `launcher`, `helperArtifact`;
only the latter has a nonempty helper ID, and other rows use explicit `null`.
Roles and archive member names are unique. Every declared helper must appear.
The existing CLI performs all archive-name, native-role and machine/version
checks. Adapter success cannot bypass a refusal by that executable.

`InputBytes`, `TotalInputBytes` and `OutputBytes` are mandatory positive explicit
limits. The metadata/config/Root/Manifest bound stays 1 MiB. All native files
are streamed, with at most a 1 MiB scratch buffer per copy; the adapter does not
allocate a native file-sized JavaScript buffer. Retained original input bytes
have an aggregate configured ceiling. The two explicitly pinned tool image
sizes bound their separate copies. The original Go tool still has its own
documented parser/snapshot/generated-output memory costs; this adapter does not
claim that its scratch-buffer bound limits that child's process working set.
The closed limits object contains `InputBytes`, `TotalInputBytes`, `OutputBytes`,
`Archive`, `JSONL`, `Native`. Archive and JSONL require every existing field from
service `docs/package-tooling.md`. Native contains exactly `PE` (`Bytes`,
`OptionalHeaderBytes`, `Sections`, `Certificates`), `Resources` (`SourceBytes`,
`MemoryBytes`) and `ResourceTree` (`Entries`, `Depth`, `NameUnits`), all positive
safe integers. Unused Linux/macOS capacity structures are omitted, not copied
as arbitrary objects. The Go CLI still validates its own complete constraints.

## SBOM and signed provenance are external build materials

Provide the actual complete CycloneDX 1.6 SBOM produced for the build, with the
installer as its primary exact SHA-256 component. Provide the original single
signed in-toto/SLSA1 DSSE envelope as JSONL. Do not supply a made-up minimal SBOM,
`{"isolated-format-fixture":true}`, or a fabricated signer/source declaration
as production evidence. No SBOM generator, scanner or provider is downloaded or
run implicitly. Their exact original file identities are retained in completion.

The adapter checks signed-envelope structure, installer subject and declared
source-revision consistency, and the existing CLI verifies full SBOM Schema and
exact Manifest/evidence byte binding. **It does not authenticate DSSE signatures,
pipeline identity or approved current provenance policy.** The receipt explicitly
sets `dsseAuthenticated:false` and `currentQualificationChecked:false`.
The existing server supply-chain verifier remains responsible for those checks;
no new client proof download or alternate signing protocol is introduced.
Offline Package Manifest verification is independently performed by the actual
existing CLI, and never grants online publication/download/install rights.

## Output and error boundary

Use an already trusted private parent that stays stable for the whole command.
The Node adapter rejects symbolic links/junctions reported by Node and multi-link
input leaves, hashes through stable file descriptors, and rechecks original
same-interface identity and bytes after every child. It is local build tooling,
not Windows native deny-write/delete custody against a hostile administrator.
Mode `0700` alone does not establish a Windows protected DACL; the operator owns
the private parent and its effective permissions. No installer is executed.

Output is never overwritten. `prepared/` and `assembled/` retain original CLI
reports and material. Failure may leave incomplete files; do not delete retained
evidence or treat their presence as success. `completion.json` is linked only
after its private pending file was fully written, synced and successfully closed.
Both hard links are intentionally retained, with no fallible cleanup or output
after that commit point. An existing completion cannot be replaced. Exit zero
and that complete receipt are required; generic diagnostics expose no provider
output or source paths. Root/Manifest signature failure, signer nonzero, signal,
input drift, cancellation and output failure do not fall back or retry.

## Acceptance still pending

The source controls use small synthetic files and injected pack/signer results
to test orchestration and file/error boundaries. They are not genuine NSIS,
Manifest signature or product qualification evidence. Separate real acceptance
must build and invoke the actual CLI, an isolated external test signer and a
real native NSIS specimen, with exact bytes independently pinned. The Windows
stub/target profile repair must be integrated first: old T04 incorrectly assumes
every NSIS installer is PE64; this adapter deliberately does not bypass it.
macOS DMG version observation, the other three complete native input workflows,
SBOM producer integration and isolated nonempty-incremental fallback remain
separate T04 work. Full T31/T46/T43 product qualification remains separate.
