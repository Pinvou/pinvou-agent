#!/usr/bin/env bash
# End-to-end LoongArch64 (new-world, dpkg arch: loong64) release build:
# cross toolchain -> glibc-2.38 sysroot -> knowledge server -> SenseVoice ASR
# engine -> tauri app -> hand-assembled loong64 deb -> repack -> guards.
# This is the script the loong64 release job calls; every step fails loudly.
#
# Pipeline (each stage is its own script, consumed through its documented
# interface):
#   1. scripts/ci-loongarch64-toolchain.sh   pinned loong64/cross-tools
#      (gcc 14.4 / glibc 2.38) download + verify; reusable via
#      PINVOU_LOONG64_TOOLCHAIN_PREFIX.
#   2. scripts/ci-loongarch64-sysroot.sh     deepin beige (glibc 2.38) dev
#      sysroot + generated CMake toolchain file; reusable via
#      PINVOU_LOONG64_SYSROOT.
#   3. cargo build of pinvou-knowledge-server for the exact target triple —
#      the prebuilt-mode source of
#      pinvou3-app/scripts/tauri/knowledge-host.js
#      (pinvou-knowledge/target/<triple>/release/pinvou-knowledge-server).
#   4. scripts/asr/build-sensevoice-runtime.sh --arch loongarch64, driven by
#      the generated CMAKE_TOOLCHAIN_FILE + STRIP — output lands at
#      src-tauri/target/linux-asr-runtime/loongarch64/sense-voice-main,
#      where the loongarch64 overlay
#      (config/platforms/linux/loongarch64/tauri.conf.json) and
#      scripts/tauri/linux-asr-runtime.js prebuilt mode expect it.
#   5. node scripts/tauri/build.js build --target loongarch64-unknown-linux-gnu
#      with PINVOU3_LINUX_ASR_PREBUILT=1 + PINVOU3_KNOWLEDGE_HOST_PREBUILT=1
#      (the cross-build gate in build.js requires exactly one resolution per
#      sidecar; prebuilt keeps the overlay resource declarations, so the
#      artifacts are packaged). Bundling is skipped via `--bundles none`
#      (build.js translates it to the tauri CLI's --no-bundle; see step 5):
#      tauri-bundler cannot bundle loongarch64 (its arch mapping errors on
#      the triple) — the deb is assembled by hand in step 7.
#   6. Stage the package-deb-loongarch64.sh contract from the locations the
#      tauri overlays declare as resources for a cross build:
#        usr/bin/pinvou3-tauri       <- src-tauri/target/<triple>/release/
#                                       (cargo target dir is keyed by the
#                                       whole triple)
#        runtime/codex-bridge/       <- src-tauri/resources/platforms/linux/
#                                       codex-bridge/ (build.js step 5
#                                       prepares the loong64 node bridge)
#        runtime/knowledge-host/     <- src-tauri/resources/platforms/linux/
#                                       knowledge-host/ (helper script +
#                                       the prebuilt loong64 server that
#                                       build.js copied in)
#        runtime/asr/sense-voice-main <- src-tauri/target/linux-asr-runtime/
#                                       loongarch64/sense-voice-main
#      (the asr shim + license are installed by the packaging script itself,
#      matching the platform overlay declarations).
#   7. scripts/package-deb-loongarch64.sh -> pinvou3_<VERSION>_loong64.deb in
#      the bundler-conventional output dir (target/release/bundle/deb).
#   8. scripts/repack-deb-xz.sh (gzip-6 -> xz -9, same as the amd64/arm64
#      jobs) and scripts/check-linux-glibc-floor.sh <deb> loongarch64
#      (default floor 2.38, deepin 23/25 + UOS V25). The elf policy itself
#      lives in check-linux-elf-policy.sh and is invoked by the floor guard
#      on the extracted tree — the .deb file is not an ELF, so the policy
#      script must not be pointed at the deb directly.
#
# RUSTFLAGS are release-job policy and are inherited unchanged from the
# workflow env (remap-only there: the pinned cross toolchain ships NO lld —
# only bfd ld — so the arm64 job's -fuse-ld=lld must never be copied; the
# rustc-side --remap-path-prefix is kept for artifact-path parity).
#
# Env knobs: PINVOU_LOONG64_TOOLCHAIN_PREFIX / PINVOU_LOONG64_SYSROOT (reuse
# prepared trees), PINVOU_LOONG64_SYSROOT_METHOD and the mirror overrides
# consumed by the two setup scripts, PINVOU_LOONG64_SKIP_KNOWLEDGE_BUILD /
# PINVOU_LOONG64_SKIP_ASR_BUILD (only for debugging the tail of the
# pipeline; the package then fails loudly on the missing sidecar unless the
# artifact is already present from a previous run).
#
# Host requirements: node/npm, cargo+rustup, curl, git, cmake, perl,
# pkg-config, dpkg-deb, readelf/file, python3 — the jammy runner image plus
# the release job's apt list covers all of them.
#
# Local static check: bash -n scripts/ci-build-linux-loongarch64.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_DIR="$REPO_ROOT/pinvou3-app"
SRC_TAURI="$APP_DIR/src-tauri"

TRIPLE="loongarch64-unknown-linux-gnu"
TOOLCHAIN_ROOT_DEFAULT="$SRC_TAURI/target/loongarch64-cross-tools/loongarch64-unknown-linux-gnu"
SYSROOT_DEFAULT="$SRC_TAURI/target/loongarch64-sysroot"
STAGING_ROOT="$SRC_TAURI/target/loong64-staging"
APP_BINARY="$SRC_TAURI/target/$TRIPLE/release/pinvou3-tauri"
KNOWLEDGE_SERVER="$REPO_ROOT/pinvou-knowledge/target/$TRIPLE/release/pinvou-knowledge-server"
ASR_ENGINE="$SRC_TAURI/target/linux-asr-runtime/loongarch64/sense-voice-main"
DEB_OUTPUT_DIR="$SRC_TAURI/target/release/bundle/deb"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

log() {
  echo "[loong64-build] $*"
}

for command_name in node npm cargo rustup curl git cmake perl pkg-config \
  dpkg-deb readelf file python3 bash; do
  command -v "$command_name" >/dev/null || fail "missing required host command: $command_name"
done

[ -f "$REPO_ROOT/VERSION" ] || fail "repo VERSION file missing"
[ -f "$REPO_ROOT/pinvou-knowledge/Cargo.toml" ] \
  || fail "pinvou-knowledge is not checked out; run 'git submodule update --init --recursive' first"

# ---- 1. cross toolchain ------------------------------------------------------
toolchain_root="${PINVOU_LOONG64_TOOLCHAIN_PREFIX:-}"
if [ -n "$toolchain_root" ] && [ ! -x "$toolchain_root/bin/$TRIPLE-gcc" ]; then
  fail "PINVOU_LOONG64_TOOLCHAIN_PREFIX has no bin/$TRIPLE-gcc: $toolchain_root"
fi
if [ -z "$toolchain_root" ]; then
  bash "$SCRIPT_DIR/ci-loongarch64-toolchain.sh"
  toolchain_root="$TOOLCHAIN_ROOT_DEFAULT"
fi
TOOLCHAIN_BIN="$toolchain_root/bin"

# ---- 2. deepin glibc-2.38 sysroot + CMake toolchain file ---------------------
sysroot="${PINVOU_LOONG64_SYSROOT:-}"
if [ -n "$sysroot" ]; then
  [ -f "$sysroot/pinvou-loongarch64-toolchain.cmake" ] \
    || fail "PINVOU_LOONG64_SYSROOT lacks pinvou-loongarch64-toolchain.cmake: $sysroot"
else
  bash "$SCRIPT_DIR/ci-loongarch64-sysroot.sh" --toolchain-prefix "$toolchain_root"
  sysroot="$SYSROOT_DEFAULT"
fi
CMAKE_TOOLCHAIN_FILE="$sysroot/pinvou-loongarch64-toolchain.cmake"
PKG_CONFIG_LIBDIR="$sysroot/usr/lib/loongarch64-linux-gnu/pkgconfig:$sysroot/usr/share/pkgconfig:$sysroot/usr/lib/pkgconfig"
PKG_CONFIG_SYSROOT_DIR="$sysroot"

# ---- cross environment (single source of truth for steps 3-6) ----------------
# Underscore spelling ONLY: cc-rs reads `<var>_<target_with_underscores>`
# (dashes replaced, cc lib.rs "<var>_<target_with_underscores>") and bash
# rejects hyphens in identifier positions — `export CC_loongarch64-...=`
# is a runtime `not a valid identifier` error (bash -n accepts it), which
# would kill the release build before step 3. The verbatim-triple form is
# read by nothing. cargo only knows the underscore form for the linker. gcc
# resolves its bundled glibc sysroot from its own path (relocatable), so no
# SYSROOT/--sysroot flag is needed for plain rust/cargo compilation.
export PINVOU_LOONG64_TOOLCHAIN_PREFIX="$toolchain_root"
export CC_loongarch64_unknown_linux_gnu="$TOOLCHAIN_BIN/$TRIPLE-gcc"
export CXX_loongarch64_unknown_linux_gnu="$TOOLCHAIN_BIN/$TRIPLE-g++"
export AR_loongarch64_unknown_linux_gnu="$TOOLCHAIN_BIN/$TRIPLE-gcc-ar"
export RANLIB_loongarch64_unknown_linux_gnu="$TOOLCHAIN_BIN/$TRIPLE-gcc-ranlib"
export STRIP="$TOOLCHAIN_BIN/$TRIPLE-strip"
# RUSTFLAGS are workflow-owned (job-level, remap-only) and must stay that
# way: a target-scoped CARGO_TARGET_*_RUSTFLAGS silently SHADOWS the
# job-level RUSTFLAGS for the target units, and cargo-only target-scoped
# flags are invisible to the rustc link smoke below — the 0.10.13 release
# link lost its startfiles to exactly such a target-scoped --sysroot
# link-arg. Fail closed at setup time if one is present.
[ -z "${CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS:-}" ] \
  || fail "CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS is set — remove it: RUSTFLAGS are workflow-owned (remap-only) and a target-scoped value silently shadows them for the target units"
# The encoded spelling is cargo's one other rustflags env: alongside the
# job-level RUSTFLAGS it makes cargo abort outright instead of building, so
# reject it here with the policy message rather than letting a generic
# cargo error surface mid-build.
[ -z "${CARGO_ENCODED_RUSTFLAGS:-}" ] \
  || fail "CARGO_ENCODED_RUSTFLAGS is set — remove it: RUSTFLAGS are workflow-owned (remap-only), and cargo refuses to build with the encoded and plain spellings set together"
# Cargo links through the rust-lld wrapper (rustc's LLVM emits R_LARCH_CALL36
# that the bundled bfd 2.41 cannot resolve; the wrapper keeps native cmake/cc
# builds on bfd and the glibc 2.38 floor unchanged). The wrapper is
# materialized by ci-loongarch64-toolchain.sh — a stale or hand-made prefix
# without it must fail here, not at link time.
cargo_linker="$TOOLCHAIN_BIN/$TRIPLE-gcc-rust-lld"
[ -x "$cargo_linker" ] \
  || fail "missing $cargo_linker — re-run scripts/ci-loongarch64-toolchain.sh to materialize the rust-lld linker wrapper"
export CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_LINKER="$cargo_linker"
export PKG_CONFIG_LIBDIR PKG_CONFIG_SYSROOT_DIR CMAKE_TOOLCHAIN_FILE
# bindgen (libspa/pipewire -sys build scripts) must bind against the target
# sysroot's headers, not the host's /usr/include (exported by the sysroot
# script on the pr-check path; re-exported here because this script owns
# the release path's cargo invocations). The explicit --target is load
# bearing: bindgen 0.72 compiles its clang_macro_fallback PCH from the
# clang args known before it injects --target for cross builds, so without
# it the fallback parses the sysroot's glibc headers as the host arch —
# the multiarch bits fail to resolve and typedef-carrying macro bodies
# (spa's SPA_ID_INVALID) evaluate to nothing and drop out of the bindings.
export BINDGEN_EXTRA_CLANG_ARGS="--target=$TRIPLE --sysroot=$sysroot"
# bindgen dlopens libclang. LoongArch became a primary LLVM target in 16 and
# the jammy archive ships no libclang >= 16 (its 14/15 lack the target), so
# the release job installs libclang-18-dev from the pinned apt.llvm.org
# llvm-toolchain-jammy-18 suite; point clang-sys's discovery at it. Dev
# hosts with a different LLVM prefix can override LIBCLANG_PATH; a host
# without the llvm-18 prefix falls back to clang-sys's own discovery.
if [ -d /usr/lib/llvm-18/lib ]; then
  export LIBCLANG_PATH="${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}"
fi

# ---- rust target --------------------------------------------------------------
# rustup resolves the toolchain per CWD and its ancestors only: the tauri
# stage below runs inside src-tauri/, whose rust-toolchain.toml floats on
# "stable", while every REPO_ROOT-cwd invocation (the target add here, the
# link smoke, the knowledge build via --manifest-path) resolves the rustup
# DEFAULT — a runner-image property nothing in this job controls, and the
# release job has no "align the toolchain" step (pr-check does). Align the
# default to the toolchain the rust-toolchain.toml pins first, so all
# stages share one rustc and the std lands on the toolchain that actually
# builds.
pinned_toolchain="$(cd "$REPO_ROOT/src-tauri" && rustup show active-toolchain | awk '{print $1}')"
(cd "$REPO_ROOT" && rustup default "$pinned_toolchain") \
  || fail "cannot align the rustup default to the rust-toolchain.toml toolchain ($pinned_toolchain)"
(cd "$REPO_ROOT" && rustup target add "$TRIPLE") \
  || fail "cannot install the rust $TRIPLE std for the active toolchain"

assert_loongarch_elf() {
  local elf="$1" machine
  [ -f "$elf" ] || fail "expected loong64 artifact missing: $elf"
  # `|| true`: a corrupt/non-ELF artifact makes readelf fail, and under
  # pipefail that would abort BEFORE the fail() below — a silent exit
  # instead of the precise message.
  machine="$(LC_ALL=C readelf -h "$elf" 2>/dev/null | sed -n 's/^[[:space:]]*Machine:[[:space:]]*//p' || true)"
  [ "$machine" = "LoongArch" ] \
    || fail "$elf is not a LoongArch64 ELF (readelf Machine: '${machine:-<none>}')"
}

# ---- link smoke (seconds; the release path's canary for the link env) ---------
# The pr-check legs are check-only and never link, so without this probe the
# release job's first link was a full knowledge-server compile — the worst
# place to discover a broken cross link environment. The tiny link exercises
# the whole cargo-link chain end to end: the wrapper, the -B/-fuse-ld=lld
# handoff into the lld shim, rust-lld resolved from the active rustc sysroot,
# startfiles and the libc.so linker script from the toolchain's bundled
# glibc 2.38 (the same tree every cargo link resolves -lc from — no
# --sysroot is passed; the deepin tree serves the cmake/bindgen/pkg-config
# side) and libgcc from the toolchain. Runs from the repo root on the
# aligned default toolchain (rust target step above) — the same rustc the
# knowledge build uses; only the job's rustc-side --remap-path-prefix
# RUSTFLAGS differs from a cargo link, and remap is not a link input.
smoke_dir="$(mktemp -d)"
printf 'fn main() {}\n' > "$smoke_dir/hello.rs"
(cd "$REPO_ROOT" && rustc --target "$TRIPLE" \
  -C linker="$CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_LINKER" \
  -o "$smoke_dir/hello" "$smoke_dir/hello.rs") \
  || fail "cross link smoke failed: the loong64 link environment is broken (see the error above)"
assert_loongarch_elf "$smoke_dir/hello"
rm -rf -- "$smoke_dir"
log "link smoke passed"

# ---- 3. knowledge server (exact target triple) --------------------------------
if [ "${PINVOU_LOONG64_SKIP_KNOWLEDGE_BUILD:-0}" = "1" ]; then
  log "step 3: skipped by PINVOU_LOONG64_SKIP_KNOWLEDGE_BUILD (reusing an earlier build)"
else
  log "step 3: building pinvou-knowledge-server for $TRIPLE"
  (cd "$REPO_ROOT" && cargo build \
    --manifest-path "$REPO_ROOT/pinvou-knowledge/Cargo.toml" \
    --target "$TRIPLE" --release --locked)
fi
assert_loongarch_elf "$KNOWLEDGE_SERVER"

# ---- 4. SenseVoice ASR engine (cross cmake build) ------------------------------
if [ "${PINVOU_LOONG64_SKIP_ASR_BUILD:-0}" = "1" ]; then
  log "step 4: skipped by PINVOU_LOONG64_SKIP_ASR_BUILD (reusing an earlier build)"
else
  log "step 4: cross-building the SenseVoice engine"
  bash "$SCRIPT_DIR/asr/build-sensevoice-runtime.sh" \
    --output "$ASR_ENGINE" --arch loongarch64
fi
assert_loongarch_elf "$ASR_ENGINE"

# ---- 5. tauri app cross build (no bundling: see the header) --------------------
# build.js owns the flag translation: `--bundles none` is absorbed before the
# tauri CLI sees it (the pinned @tauri-apps/cli 2.11.4 rejects `--bundles none`
# at arg-parse time — verified: "error: invalid value 'none'") and becomes the
# CLI's official `--no-bundle`, "Skip the bundling step".
log "step 5: tauri app cross build (no bundling; the deb is hand-assembled below)"
(
  cd "$APP_DIR" \
    && PINVOU3_LINUX_ASR_PREBUILT=1 \
       PINVOU3_KNOWLEDGE_HOST_PREBUILT=1 \
       node scripts/tauri/build.js build --target "$TRIPLE" --bundles none
) || fail "tauri app cross build failed"
assert_loongarch_elf "$APP_BINARY"

# ---- 6. staging (package-deb-loongarch64.sh contract) ---------------------------
log "step 6: staging the deb inputs"
rm -rf -- "$STAGING_ROOT"
mkdir -p "$STAGING_ROOT/usr/bin" "$STAGING_ROOT/runtime/asr"
install -m 0755 "$APP_BINARY" "$STAGING_ROOT/usr/bin/pinvou3-tauri"

bridge_dir="$SRC_TAURI/resources/platforms/linux/codex-bridge"
[ -d "$bridge_dir" ] || fail "codex bridge runtime not prepared: $bridge_dir"
# -print -quit stops at the first match: a plain `find | grep -q` can die of
# SIGPIPE (a later match written after grep exits) and, under pipefail, the
# guard would then reject the very artifact it exists to protect.
if ! find "$bridge_dir" -type f -name node -print -quit | grep -q .; then
  fail "codex bridge runtime has no node binary (was it prepared for loong64?): $bridge_dir"
fi
cp -a "$bridge_dir" "$STAGING_ROOT/runtime/codex-bridge"

knowledge_dir="$SRC_TAURI/resources/platforms/linux/knowledge-host"
[ -f "$knowledge_dir/pinvou-knowledge-server" ] \
  || fail "prebuilt knowledge server was not staged by the tauri build: $knowledge_dir/pinvou-knowledge-server"
cp -a "$knowledge_dir" "$STAGING_ROOT/runtime/knowledge-host"

install -m 0755 "$ASR_ENGINE" "$STAGING_ROOT/runtime/asr/sense-voice-main"

# ---- 7. hand-assembled loong64 deb ---------------------------------------------
log "step 7: assembling the deb"
bash "$SCRIPT_DIR/package-deb-loongarch64.sh" --staged "$STAGING_ROOT" --output "$DEB_OUTPUT_DIR"

version="$(tr -d '[:space:]' < "$REPO_ROOT/VERSION")"
deb="$DEB_OUTPUT_DIR/pinvou3_${version}_loong64.deb"
[ -f "$deb" ] || fail "packaging script did not produce $deb"

# ---- 8. repack + guards ---------------------------------------------------------
log "step 8: repack (xz -9) + glibc floor guard"
bash "$SCRIPT_DIR/repack-deb-xz.sh" "$deb"
# The floor guard extracts the deb and runs check-linux-elf-policy.sh on the
# tree (the .deb itself is not an ELF); its third argument defaults to the
# loongarch64 floor 2.38 (deepin 23/25 + UOS V25).
bash "$SCRIPT_DIR/check-linux-glibc-floor.sh" "$deb" loongarch64

deb_sha="$(sha256sum "$deb" | awk '{print $1}')"
cat <<EOF
---- ci-build-linux-loongarch64: artifacts ----
deb: $deb
sha256: $deb_sha
staged root: $STAGING_ROOT
toolchain: $toolchain_root
sysroot: $sysroot
# CI step exports (recompute at consumption time; see also the env blocks
# printed by the two setup scripts):
#   PINVOU_LOONG64_TOOLCHAIN_PREFIX,
#   CC/CXX/AR/RANLIB_loongarch64_unknown_linux_gnu, STRIP,
#   CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_LINKER,
#   PKG_CONFIG_LIBDIR, PKG_CONFIG_SYSROOT_DIR, CMAKE_TOOLCHAIN_FILE,
#   BINDGEN_EXTRA_CLANG_ARGS, LIBCLANG_PATH (llvm-18 prefix when present)
#   (all set inside this script for its children; a workflow that runs the
#   setup scripts directly exports the same values from their output).
#   RUSTFLAGS stay the workflow's decision (remap-only; a target-scoped
#   CARGO_TARGET_*_RUSTFLAGS is rejected above); the pinned toolchain has
#   no lld, so the arm64 -fuse-ld=lld flag must not be copied.
EOF
