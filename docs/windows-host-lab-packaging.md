# Windows host packaging

This opt-in build profile packages the T31 helper and stable launcher through
the existing `npm --prefix pinvou3-app run build:nsis` entry. It supports only
an explicitly selected **lab** or **production** profile. Default builds keep
the original Tauri NSIS hooks, MSI target, arguments and launch behavior.
Production source authorization, native installation and platform qualification
remain separate requirements; packaging public inputs does not create authority.

Supply these explicit build inputs through protected process configuration:

- `PINVOU_HOST_PROFILE_KIND`: set exactly `production` for production packaging;
  absent retains the existing lab selection. An explicit `lab` is also accepted.
  The selection must equal the canonical Seed kind. A selector without the two
  public input files refuses. With all three absent, host packaging is disabled.
- `PINVOU_HOST_BOOTSTRAP_SEED`: absolute path to the public canonical Seed.
- `PINVOU_HOST_INITIAL_ROOT`: absolute path to its independently pinned complete
  canonical V1 Root envelope. Both absent with no selector means disabled;
  a partial pair refuses.
- `PINVOU_HOST_HELPER_BINARY` and `PINVOU_HOST_LAUNCHER_BINARY`: absolute paths
  to the actual host author's compiled images using those same public inputs.
- `PINVOU_HOST_SIGNER_SCRIPT`: an absolute external PowerShell build script,
  optional for Seed V2 and required only by the legacy selected-signer policy.
  It accepts only `-Role helper|launcher|app|installer -Image <build-file>` and
  must complete that file's final signature. Credentials remain with that
  explicit signing operation; there is no default certificate or enrollment.
  Signer stdout/stderr is discarded rather than retained in packaging logs.
- `PINVOU_HOST_MAKENSIS`: the approved NSIS 3.11 compiler executable already
  used by the pinned Tauri toolchain. This workflow installs no compiler.

Install the existing locked build dependencies for the app and
`specs/upgrade/v1`; the latter supplies the frozen canonical codec and Root
verifier. No protocol grammar or dependency version is copied or changed.
The original Root's self-signatures are checked at its signed issuance time.
This proves byte/cryptographic integrity of the independent input, not its
provenance, current validity or acceptance by a protected host.

The wrapper enforces Windows x64, an explicit NSIS-only build and the
matching `upgrade-host-lab` or `upgrade-host-production` app feature. They are
mutually exclusive. Production rejects fault profile IDs and native acceptance
hooks. It invokes its existing Tauri preparation and
build with `--no-bundle`, fixing the MSVC target. The host leaf is built by its
own owner; packaging does not run a second Cargo build for those two images.
Enabled builds require the actual composed version to equal the Seed and the
main binary to remain `pinvou3-tauri` (the repository's Cargo default when no
override is supplied). Resource and presentation overlays remain supported.
Caller identity changes refuse before compilation; final generated configuration
and the packaging entry repeat the same check, so an old fixed-name output
cannot substitute for a renamed build. Default-off overrides remain unchanged.
The host packaging directory is `windows-host-packaging` under the same Cargo
target root resolved during preparation. An absolute `CARGO_TARGET_DIR` selects
that root; relative values retain their existing resolution from `src-tauri`,
and an absent value keeps `src-tauri/target`. The prepared plan retains this
absolute root through final packaging, even if the later environment changes
or an independently retained application input is supplied from another path.
Every application resource comes from the actual composed Tauri resource
manifest. Unknown sidecar/association/deep-link mappings refuse this initial
profile. The complete inventory includes empty files and uses UTF-8 byte order,
with at most 32,768 regular files. Private InstallationManifest version 2 binds
one to eight ordered inventory shards; the root and each complete canonical
shard remain at most one MiB, and each shard contains at most 4096 files.
Greedy partitioning uses the final signed file sizes and hashes, including
zero-length resources. No oversized combined inventory is serialized or fed
back to the public V1 parser. It never omits resources to fit. Unpublished
private manifest version 1 is refused rather than adopted or migrated.
Native `CompareStringOrdinal` rejects Windows aliases and
file/directory collisions, including non-ASCII names; native attributes and
full streaming file hashes reject reparses, hard links and unstable sources.

Builder-only native controls have separate finite limits: at most 16 blocks,
16 MiB aggregate, 4096 entries / one MiB per closed block, 32,768 total entries,
and 3072 UTF8 bytes per absolute source path. Actual 10,877-name measurements
needed four blocks for the original paths and four for the copied paths; their
maximum original source path was 231 UTF8 bytes. These local engineering bounds
do not guarantee that every worst-case 3072-byte path set fits. A long build
directory or excess control capacity causes an explicit
`HOST_BUILD_NATIVE_SOURCE_PATH_UNSUPPORTED` or
`HOST_BUILD_NATIVE_INVENTORY_CAPACITY` refusal; shorten the build directory or
review capacity instead of excluding resources. The native validator checks
aliases and ancestor collisions across the complete typed block aggregate.
Each block is parsed independently with its existing limits. These controls
are trusted builder-created local files and cannot supply runtime authority.

Signing and construction occur in this order:

1. Copy the actual app/resources and two role images into a unique build-owned
   output directory. The external build signer finalizes helper, launcher and
   app when a signer is configured. Legacy Seed V1 requires the selected signer
   certificate; Seed V2 explicitly allows no certificate. A configured signer
   failure never falls back. Certificate policy does not grant source authority.
   `helper --profile` must
   expose the exact compiled public profile/Seed hash; this is diagnostic only.
2. Hash every final policy-checked image and every complete app resource. Generate the
   closed canonical InstallationManifest and exact referenced inventory shards.
   A ninth required shard refuses before resource embedding, archive creation
   or outer-installer signing; earlier role signatures and partial artifacts
   remain retained. No sizing placeholder enters final metadata. The manifest
   contains no installer self-hash.
3. Compile the independent plugin-free template with inherited NSIS configuration
   disabled. Its `!packhdr` transform uses
   Windows `UpdateResourceW` on the **bare stub**, embedding RT_RCDATA 10,
   numeric ID 34101 and used shard IDs 34102..34109, all neutral language 0,
   in the same update phase. Unused reserved shard IDs must be absent; extra
   languages, wrong bindings, missing or changed shard bytes refuse. NSIS then appends its payload and
   calculates its original mandatory CRC. No final installer mutation is used
   to insert the manifest after that calculation.
4. Read the actual unique resource and independently verify the NSIS CRC. Apply
   configured outer-installer signing last, then repeat resource/CRC/file checks. A successful
   signature alone cannot establish intact NSIS payload/CRC. Keep private
   build logs and the exact final artifact evidence; partial outputs are never
   accepted as completed installation.

The native first-intent transaction persists the exact root and all referenced
shards atomically with its protected head/history. Root remains `Manifest`;
fixed `ManifestInventory-0` through `ManifestInventory-7` values hold used
shards and unused slots remain absent. Later service, restart and same-source
maintenance paths must reload and verify the complete protected set. A root-only
intermediate commit, missing/changed/extra page, sidecar fallback or silent
history rewrite is not an accepted installation.

The final source set is `installer.exe`, the same two policy-checked host images and
the complete `app/` subtree. An independent authorized distribution/lab staging
operation must verify and protect that exact source in its fixed Program Files
namespace **before starting the NSIS process**. Building it does not construct
a `SourceInstallerContext`. Source staging is distinct from target installation
and must remain retained for same-build repair, uninstall and its tombstone.

The enabled template invokes only the fixed native helper stages. Its
first executable action invokes the source helper, which commits/authenticates
the original installation intent before copying the complete source app/host
assets through relative native opens, protected security descriptors at creation
and file flushing. NSIS itself never copies target app files: inherited ACLs
from an ordinary `File` instruction cannot replace this native creation policy.
It never runs a legacy uninstaller, installs a
prerequisite, kills a GUI app or loads a temporary native plugin. Missing
already-installed prerequisites are a native early refusal. Every nonzero
required stage stops later effects, including commit-unknown; there is no
automatic new identity or arbitrary path argument. Native exact app copies are followed
by native verification and real service readiness. Both shortcuts select the
stable launcher. Add/Remove Programs points to the retained source
installer's closed `/HOST_UNINSTALL` mode. Native removal seals the tombstone
before those presentation entries are removed. Optional `/R`/interactive launch
calls only the source helper's ordinary-desktop operation, with no elevated GUI
fallback; installation success does not assert that an app start was observed.

Supported options are `/S` or `/P`, optional `/R`, `/HOST_REPAIR` or
`/HOST_UNINSTALL`, and `/L=1033|2052|1041`. Conflicts, duplicates, truncation,
`/D`, `/ARGS`, `/UPDATE`, `/NCRC` and arbitrary argument forwarding refuse.
The default-off installer retains its original argument compatibility.

Run the affected packaging tests with Node's test runner. The actual compiler,
resource, CRC and Windows ordinal checks opt in with `PINVOU_HOST_TEST_NSIS`
pointing to that compiler. Those tests retain their build-only artifacts and
never execute an installer or create a service/certificate. Full build
and native install/repair/removal acceptance require the separately authorized
isolated Windows lab and trusted verification environment. No certificate is
added to the normal workstation to make a test pass.

These capacities apply to build metadata. Product downloads retain no hard
total/cumulative duration or retry-count cap; 32 MiB bounds only the existing
internal atomic business metadata batch, not artifact bytes.


## Native role loader gates

Build the standalone Windows MSVC helper and launcher with `-C target-feature=+crt-static` and `/DEPENDENTLOADFLAG:0x800`. These settings apply to those two role binaries; the GUI application's existing C++/WebView runtime configuration remains separate. The binaries' own compile guards reject a non-static CRT build.

Packaging independently checks the actual helper/launcher PE bytes before their first profile-query execution, before signing and after signing. The read-only bounded metadata parser requires AMD64 PE32+, an exact `LoadConfig.DependentLoadFlags` value of `0x800`, and no ordinary or delay imports from dynamic VC/UCRT runtime families. Malformed/truncated/ambiguous metadata refuses; a missing import terminator is not an empty dependency set. Headers/sections/import work are bounded without loading the image, reading the whole executable, adding a dependency or imposing a new overall artifact/download size or duration limit.

Diagnostics are `HOST_BUILD_ROLE_IMAGE_INVALID`, `HOST_BUILD_ROLE_LOADER_UNSAFE`, and `HOST_BUILD_ROLE_RUNTIME_IMPORT`. The app and outer installer keep their existing checks. All original Authenticode Valid/exact-leaf, full payload/hash, manifest/page and NSIS CRC gates remain mandatory. Static PE observations alone do not qualify a guest install or product recovery.


## Cargo application output snapshot

Cargo can expose its final application executable through multiple hard links. Only this explicit build-output input is copied through one held source descriptor into an exclusively created build-owned snapshot. Full hashes before copying, during copying and after copying must agree; the original file name, native file identity, timestamps, link count and all ancestor directory identities must remain stable. The writer is flushed and closed before the snapshot is reopened through the unchanged strict single-link file validator. Failed or partial snapshots remain diagnostic artifacts and never become installation input.

Resources and the two independently supplied role inputs keep their original single-link requirement. Native inventory, signing and the final installation manifest consume the fresh single-link app snapshot; retained SOURCE, signed images and installed files retain all existing hard-link, reparse, full-hash and native trust checks. This adapter does not add an artifact byte limit, download deadline or retry-count limit.


## Production installation identity

Production uses a stable installation `profileId`, independent of `buildId`.
The fixed Program Files leaves under `Pinvou` are `UpdaterBootstrapSource`
(profile/build retained source), `Updater` (profile host code), and `App`
(profile application). Lab keeps its original `*Lab` leaves. Production service
and uninstall registration both use `PinvouUpgrade-<profileId>`; lab retains
`PinvouUpgradeLab-<profileId>`. These are generated from the selected Seed, never
accepted as arbitrary command-line paths. Runtime registry, pipe and mutex
isolation belongs to the native host profile policy.

Build the helper and launcher with the same explicit public Seed/Root pair,
then invoke the existing `npm --prefix pinvou3-app run build:nsis` with the
documented build environment and `PINVOU_HOST_PROFILE_KIND=production`.
There is no default Root or public-input fallback. The separate authorized
source owner must still protect the complete retained source before starting
the installer; old lab staging scripts are not a production distribution path.
Without its real authorization provider, metadata upgrade remains unavailable.
