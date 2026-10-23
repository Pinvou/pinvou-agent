#!/usr/bin/env bash
# Assemble the LoongArch64 (new-world, dpkg Architecture: loong64) .deb by
# replicating the tauri-bundler deb layout by hand.
#
# Why a hand-assembled deb: tauri-bundler cannot bundle loongarch64 — its
# Arch mapping (binary_arch()) only knows amd64/arm64/riscv64/... and errors
# out on the loongarch64-unknown-linux-gnu triple. This script rebuilds the
# exact same package the bundler would produce, derived from the bundler
# version the repo actually bundles with: @tauri-apps/cli 2.12.4 contains
# tauri-bundler 2.10.4 (verified against the Cargo.lock of the upstream tag
# tauri-cli-v2.12.4, commit 993038c49185307dfc895ab7657d77ca0d5ed7c7;
# source: https://static.crates.io/crates/tauri-bundler/tauri-bundler-2.10.4.crate,
# files src/bundle/linux/debian.rs and src/bundle/linux/freedesktop/mod.rs;
# re-derived for the 2.11.4 -> 2.12.4 CLI bump: the 2.9.4 -> 2.10.4 delta in
# those files is cosmetic — rustfmt import ordering and `unwrap_or` ->
# `unwrap_or_else` — so the replicated parity below is unchanged).
#
# Upstream route (removal trigger): the bundler gap is a small upstream fix —
# an `Arch::LoongArch64` variant, the `binary_arch()` arm, and the debian
# `=> "loong64"` mapping; `Arch::Riscv64` entered upstream exactly this way.
# File/contribute that fix to tauri-apps/tauri and delete this script once the
# repo's `@tauri-apps/cli` floor bundles a bundler containing it. Until then
# this script is coupled to the pinned bundler internals below — a CLI bump
# must re-derive the parity claims (Installed-Size semantics, tar header
# modes, control field order, symlink handling); the policy suite pins the
# header version against pinvou3-app/package.json so the coupling cannot
# silently rot.
#
# Replicated layout (defaults resolved from the repo's tauri configs):
#   data:    /usr/bin/pinvou3-tauri                 main binary (mainBinaryName)
#            /usr/share/applications/pinvou3.desktop  desktopTemplate, copied verbatim
#              (the repo template is fully static — no handlebars placeholders)
#            /usr/share/icons/hicolor/32x32/apps/pinvou3-tauri.png
#            /usr/share/icons/hicolor/128x128/apps/pinvou3-tauri.png
#            /usr/share/icons/hicolor/256x256@2/apps/pinvou3-tauri.png
#              (freedesktop icons: only the bundle.icon PNGs are shipped —
#              icns/ico are skipped; a PNG whose file stem ends in "@2x" is
#              placed under <W>x<H>@2 using its real pixel dimensions)
#            /usr/lib/pinvou3/runtime/...           resource root is
#              /usr/lib/<productName>/ (debian.rs copy_resource_files)
#   control: Package: pinvou3                      (AsKebabCase(productName))
#            Version, Architecture: loong64,
#            Installed-Size: <data bytes / 1024>,  (debian.rs total_dir_size)
#            Maintainer: pinvou                    (no authors/publisher in the
#              configs → identifier "com.pinvou.pinvou3" 2nd component; the
#              Cargo.toml authors list ["pinvou"] yields the same string)
#            Priority: optional                    (bundler default, priority unset)
#            Depends / Recommends:                 (config/platforms/linux/tauri.conf.json)
#            Description: 品悟智能助手桌面应用后端 / " (none)" long-description
#              fallback (bundle longDescription unset → Cargo description +
#              bundler's "(none)" continuation line)
#            md5sums (one "<md5>  <path>" line per data file), postinst + prerm
#              (0755) from packaging/linux/deb/scripts/.
#            No Section/Homepage/Provides/Conflicts/Replaces (all unset in the
#            configs), no triggers (tauri-bundler has no trigger support).
#
# Staging contract — what the CI must place where (paths relative to the
# --staged root; see also scripts/repack-deb-xz.sh which repacks the result):
#   usr/bin/pinvou3-tauri        required. loong64 ELF app binary (0755). The UI
#                                dist is NOT staged and NOT packaged: tauri
#                                embeds frontendDist into the binary at compile
#                                time (generate_context!), exactly like the
#                                amd64/arm64 debs.
#   runtime/codex-bridge/        required. Target-arch bridge runtime (node +
#                                acp adapters; node built/staged for
#                                loongarch64) → /usr/lib/pinvou3/runtime/codex-bridge/
#   runtime/knowledge-host/      required. pinvou-knowledge-host-helper (shell)
#                                + loong64 pinvou-knowledge-server
#                                → /usr/lib/pinvou3/runtime/knowledge-host/
#   runtime/asr/sense-voice-main default ASR engine source (loong64 ELF, built
#                                by scripts/asr/build-sensevoice-runtime.sh
#                                --arch loongarch64). Installed alongside the
#                                repo-shipped pinvou3-asr-shim.py +
#                                LICENSE-SenseVoice.cpp
#                                → /usr/lib/pinvou3/runtime/asr/.
#                                Override the source path with --asr-engine;
#                                build the deb without ASR via --no-asr (the
#                                ggml cross build may fail; shipping without
#                                ASR must be an explicit decision, so one of
#                                the ASR arguments is effectively required).
# Everything under runtime/ is normalized to the bundler's deterministic
# tar-header modes before packing (see the normalization pass below); the
# app binary and the ASR engine are forced to 0755 on install.
#
# Output: pinvou3_<VERSION>_loong64.deb (the bundler's product_name_version_
# arch.deb naming), built with dpkg-deb --root-owner-group -Zxz so the
# downstream scripts/repack-deb-xz.sh pass and scripts/release-deb.sh's
# pinvou3_<V>_<arch>.deb glob work unchanged. Fails loudly (non-zero) on
# missing inputs, any staged ELF whose readelf Machine is not "LoongArch"
# (binutils prints "LoongArch", not "LoongArch64", for EM_LOONGARCH — same on
# jammy binutils 2.38 and newer), or a built package whose
# Package/Architecture fields do not match pinvou3/loong64.
#
# Linux-only by nature (needs dpkg-deb/readelf); static check: bash -n
# scripts/package-deb-loongarch64.sh.
set -euo pipefail
# Deterministic archive content starts with a deterministic working
# environment: runners run umask 0002, which would leak group-write bits
# into everything this script creates before the normalization pass below.
umask 022

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_SRC_TAURI="$REPO_ROOT/pinvou3-app/src-tauri"

PACKAGE_NAME="pinvou3"
MAIN_BINARY_NAME="pinvou3-tauri"        # tauri.conf.json mainBinaryName
DEB_ARCH="loong64"
EXPECTED_ELF_MACHINE="LoongArch"        # readelf Machine: for EM_LOONGARCH
RESOURCE_ROOT="usr/lib/$PACKAGE_NAME"   # debian.rs copy_resource_files

# Must stay in sync with config/platforms/linux/tauri.conf.json
# (bundle.linux.deb.depends / recommends).
DEPENDS="libwebkit2gtk-4.1-0 (>= 2.40), webkit2gtk-driver, libpipewire-0.3-0, libgbm1, libegl1"
RECOMMENDS="poppler-utils, tesseract-ocr, tesseract-ocr-chi-sim, pandoc, p7zip-full, python3, libreoffice, libemail-outlook-message-perl"

help() {
  cat <<'EOF'
usage: package-deb-loongarch64.sh --staged <dir> [--output <dir>] [--version <v>]
                                  [--asr-engine <path> | --no-asr]

Assemble the loong64 .deb (tauri-bundler layout) from a staged root.

  --staged <dir>       required. Staged root (see the header of this script
                       for the exact layout the CI must provide).
  --output <dir>       output directory for pinvou3_<VERSION>_loong64.deb
                       (default: pinvou3-app/src-tauri/target/release/bundle/deb)
  --version <v>        package version (default: the repo root VERSION file)
  --asr-engine <path>  ASR engine binary to package instead of
                       <staged>/runtime/asr/sense-voice-main
  --no-asr             build the deb without the ASR runtime (explicit opt-out)
  --help               show this help
EOF
}

usage() {
  help >&2
  exit 2
}

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

staged_dir=""
output_dir=""
version=""
asr_engine=""
no_asr=false

while [ "$#" -gt 0 ]; do
  case "$1" in
    --staged)     [ "$#" -ge 2 ] || usage; staged_dir="$2"; shift 2 ;;
    --output)     [ "$#" -ge 2 ] || usage; output_dir="$2"; shift 2 ;;
    --version)    [ "$#" -ge 2 ] || usage; version="$2"; shift 2 ;;
    --asr-engine) [ "$#" -ge 2 ] || usage; asr_engine="$2"; shift 2 ;;
    --no-asr)     no_asr=true; shift ;;
    --help)       help; exit 0 ;;
    *) usage ;;
  esac
done
if [ "$no_asr" = true ] && [ -n "$asr_engine" ]; then
  fail "--asr-engine and --no-asr are mutually exclusive"
fi

[ -n "$staged_dir" ] || usage
[ -d "$staged_dir" ] || fail "staged root is not a directory: $staged_dir"
staged_dir="$(cd "$staged_dir" && pwd)"

if [ -z "$version" ]; then
  [ -f "$REPO_ROOT/VERSION" ] || fail "repo VERSION file missing and no --version given"
  version="$(tr -d '[:space:]' < "$REPO_ROOT/VERSION")"
fi
[ -n "$version" ] || fail "empty package version"

if [ -z "$output_dir" ]; then
  output_dir="$APP_SRC_TAURI/target/release/bundle/deb"
fi
mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd)"
output_deb="$output_dir/${PACKAGE_NAME}_${version}_${DEB_ARCH}.deb"

for command_name in dpkg-deb readelf file find md5sum install; do
  command -v "$command_name" >/dev/null || fail "missing required command: $command_name"
done

app_binary="$staged_dir/usr/bin/$MAIN_BINARY_NAME"
[ -f "$app_binary" ] || fail "missing staged app binary: $app_binary"
[ -d "$staged_dir/runtime/codex-bridge" ] || fail "missing staged runtime/codex-bridge/ under $staged_dir"
[ -d "$staged_dir/runtime/knowledge-host" ] || fail "missing staged runtime/knowledge-host/ under $staged_dir"

if [ "$no_asr" = false ] && [ -z "$asr_engine" ]; then
  asr_engine="$staged_dir/runtime/asr/sense-voice-main"
fi
if [ -n "$asr_engine" ]; then
  [ -f "$asr_engine" ] || fail "ASR engine not found: $asr_engine (use --no-asr to build without ASR)"
fi

# readelf Machine: must be LoongArch for every ELF that goes into the deb.
# The glibc-floor guard re-checks the whole deb after repack; this earlier
# check only covers the staged inputs, so a wrong-arch artifact fails before
# dpkg-deb instead of with a deb-level diff.
assert_loongarch_elf() {
  local elf="$1" machine
  machine="$(LC_ALL=C readelf -h "$elf" 2>/dev/null | sed -n 's/^[[:space:]]*Machine:[[:space:]]*//p')" \
    || fail "cannot read ELF header: $elf"
  [ "$machine" = "$EXPECTED_ELF_MACHINE" ] \
    || fail "$elf is not a LoongArch64 ELF (readelf Machine: '${machine:-<none>}', expected '$EXPECTED_ELF_MACHINE' for $DEB_ARCH)"
}

assert_loongarch_elf "$app_binary"
if [ -n "$asr_engine" ]; then
  assert_loongarch_elf "$asr_engine"
fi
# Best-effort sweep of the staged runtime tree (shell scripts and data files
# are skipped by the ELF filter): catches a mis-staged node or knowledge
# server with a precise filename before packaging. -xtype follows symlinks,
# so an ELF staged behind a link (npm .bin style) is visited here as the
# LINK path — and file -L must dereference it for the content probe: a plain
# file -b lstats and reports "symbolic link to ...", silently skipping
# exactly the ELF this sweep exists to catch. Links to directories are
# skipped here and abort loudly later in the materialization pass, and
# dangling links die at the same later pass.
while IFS= read -r -d '' elf; do
  file_out="$(file -bL "$elf")"
  case "$file_out" in
    ELF*) assert_loongarch_elf "$elf" ;;
  esac
done < <(find "$staged_dir/runtime" -xtype f -print0)

work_dir="$(mktemp -d)"
trap 'rm -rf -- "$work_dir"' EXIT
chmod 0755 "$work_dir"
# dpkg-deb --build layout: everything under pkg/ except pkg/DEBIAN becomes
# data.tar; pkg/DEBIAN becomes control.tar.
pkg_dir="$work_dir/pkg"
control_dir="$pkg_dir/DEBIAN"
mkdir -p "$pkg_dir/usr/bin" "$control_dir"

# ---- data: main binary ------------------------------------------------------
install -m 0755 "$app_binary" "$pkg_dir/usr/bin/$MAIN_BINARY_NAME"

# ---- data: resources under /usr/lib/pinvou3/ --------------------------------
resource_dir="$pkg_dir/$RESOURCE_ROOT"
mkdir -p "$resource_dir/runtime/codex-bridge" "$resource_dir/runtime/knowledge-host"
cp -a "$staged_dir/runtime/codex-bridge/." "$resource_dir/runtime/codex-bridge/"
cp -a "$staged_dir/runtime/knowledge-host/." "$resource_dir/runtime/knowledge-host/"
if [ -n "$asr_engine" ]; then
  mkdir -p "$resource_dir/runtime/asr"
  install -m 0755 "$asr_engine" "$resource_dir/runtime/asr/sense-voice-main"
  # The shim carries a shebang and is exec'd directly via PINVOU3_ASR_CMD
  # (voice.rs Command::new); the repo blob is 0755 and tauri-bundler's
  # resource copy preserves that on the amd64/arm64 debs — stay in parity.
  install -m 0755 "$APP_SRC_TAURI/resources/platforms/linux/asr/pinvou3-asr-shim.py" \
    "$resource_dir/runtime/asr/pinvou3-asr-shim.py"
  install -m 0644 "$APP_SRC_TAURI/resources/platforms/linux/asr/LICENSE-SenseVoice.cpp" \
    "$resource_dir/runtime/asr/LICENSE-SenseVoice.cpp"
fi

# ---- data: desktop entry + hicolor icons ------------------------------------
desktop_template="$APP_SRC_TAURI/packaging/linux/deb/pinvou3.desktop"
[ -f "$desktop_template" ] || fail "desktop template missing: $desktop_template"
mkdir -p "$pkg_dir/usr/share/applications"
install -m 0644 "$desktop_template" "$pkg_dir/usr/share/applications/$PACKAGE_NAME.desktop"

# freedesktop/mod.rs copy_icon_files: PNG icons only, destination
# usr/share/icons/hicolor/<W>x<H>[@2]/apps/<mainBinaryName>.png where @2 is
# used when the file stem ends in "@2x" and W/H are the real PNG dimensions.
icon_32="$APP_SRC_TAURI/icons/32x32.png"         #  32 x  32 → 32x32
icon_128="$APP_SRC_TAURI/icons/128x128.png"      # 128 x 128 → 128x128
icon_256="$APP_SRC_TAURI/icons/128x128@2x.png"   # 256 x 256 → 256x256@2 (retina stem)
for icon in "$icon_32" "$icon_128" "$icon_256"; do
  [ -f "$icon" ] || fail "icon missing: $icon"
done
mkdir -p "$pkg_dir/usr/share/icons/hicolor/32x32/apps"
install -m 0644 "$icon_32" "$pkg_dir/usr/share/icons/hicolor/32x32/apps/$MAIN_BINARY_NAME.png"
mkdir -p "$pkg_dir/usr/share/icons/hicolor/128x128/apps"
install -m 0644 "$icon_128" "$pkg_dir/usr/share/icons/hicolor/128x128/apps/$MAIN_BINARY_NAME.png"
mkdir -p "$pkg_dir/usr/share/icons/hicolor/256x256@2/apps"
install -m 0644 "$icon_256" "$pkg_dir/usr/share/icons/hicolor/256x256@2/apps/$MAIN_BINARY_NAME.png"

# ---- symlink materialization (bundler parity) ---------------------------------
# tauri-bundler never ships resource-tree symlinks: copy_resources goes through
# fs::copy (which follows symlinks) per file, and create_tar_from_dir stats and
# opens through links, so the amd64/arm64 debs carry the npm `node_modules/.bin`
# entries as regular files — present in md5sums, counted at full target size in
# Installed-Size. `cp -a` staging would ship them as symlinks (absent from the
# `find -type f` md5sums, lstat-sized in Installed-Size), so materialize every
# symlink into a copy of its target content before the parity passes below.
# The staged npm trees contain symlink-to-file only (verified for the pinned
# @agentclientprotocol packages); anything else fails loudly here exactly like
# the bundler's fs::copy would.
while IFS= read -r -d '' link; do
  target="$(readlink -f -- "$link")" || fail "cannot resolve staged symlink: $link"
  rm -- "$link"
  cp -- "$target" "$link" || fail "cannot materialize staged symlink: $link"
done < <(find "$pkg_dir" -path "$control_dir" -prune -o -type l -print0)

# ---- data-tree mode normalization (bundler parity) ---------------------------
# tauri-bundler's tar entries carry deterministic headers regardless of the
# build umask (tar-rs HeaderMode::Deterministic: uid/gid 0; 0755 for
# directories and owner-executable files, 0644 for everything else; symlinks
# untouched). dpkg-deb --root-owner-group normalizes ownership only, so the
# on-disk staging modes would otherwise ship (cp -a trees and mkdir dirs
# under umask 0002: 0775 dirs, group-writable files) where the amd64/arm64
# debs carry 0755/0644. Mirror the header rule explicitly; DEBIAN/ is
# excluded — its entries are chmod'd explicitly below. Order matters: the
# owner-exec restore must see the ORIGINAL exec bit, so non-exec files are
# normalized last.
find "$pkg_dir" -path "$control_dir" -prune -o -type d -exec chmod 0755 {} +
find "$pkg_dir" -path "$control_dir" -prune -o -type f -perm -u=x -exec chmod 0755 {} +
find "$pkg_dir" -path "$control_dir" -prune -o -type f ! -perm -u=x -exec chmod 0644 {} +

# ---- control: fields replicated from debian.rs generate_control_file ---------
# Installed-Size mirrors the bundler's total_dir_size(data_dir) byte for
# byte: it sums entry.metadata().len() over EVERY WalkDir entry of the data
# root — directory inodes included (4096 apiece on ext4). The data tree is
# symlink-free by the materialization pass above (matching the bundler, whose
# data dir only ever contains dereferenced regular files). Prune DEBIAN/ the
# way the bundler's data root never contains it, sum every remaining entry
# including ".", and divide by 1024 per Debian policy. (`du -sb` would dedupe
# hardlinks and rounds nothing — close but not the same sum; find -printf '%s'
# is the plain st_size WalkDir adds up. The sum stays OUTSIDE $(( )): a command
# substitution with escaped find parens does not survive arithmetic-context
# parsing.)
installed_size_bytes="$(
  cd "$pkg_dir" && find . \( -path './DEBIAN' -o -path './DEBIAN/*' \) -prune -o -printf '%s\n' \
    | awk '{ s += $1 } END { print s + 0 }'
)"
installed_size_kb=$((installed_size_bytes / 1024))
{
  printf 'Package: %s\n' "$PACKAGE_NAME"
  printf 'Version: %s\n' "$version"
  printf 'Architecture: %s\n' "$DEB_ARCH"
  printf 'Installed-Size: %s\n' "$installed_size_kb"
  printf 'Maintainer: pinvou\n'
  printf 'Priority: optional\n'
  printf 'Depends: %s\n' "$DEPENDS"
  printf 'Recommends: %s\n' "$RECOMMENDS"
  printf 'Description: 品悟智能助手桌面应用后端\n'
  # bundler fallback for an unset long description
  printf ' (none)\n'
} > "$control_dir/control"
chmod 0644 "$control_dir/control"

# ---- control: md5sums over every data file (bundler generate_md5sums) --------
(
  cd "$pkg_dir" \
    && find . -type f -not -path './DEBIAN/*' -print0 | LC_ALL=C sort -z \
    | xargs -0 md5sum | sed 's#^\([0-9a-f]\{32\}  \)\./#\1#'
) > "$control_dir/md5sums"
chmod 0644 "$control_dir/md5sums"

# ---- control: maintainer scripts ---------------------------------------------
postinst_src="$APP_SRC_TAURI/packaging/linux/deb/scripts/postinst.sh"
prerm_src="$APP_SRC_TAURI/packaging/linux/deb/scripts/prerm.sh"
[ -f "$postinst_src" ] || fail "postinst script missing: $postinst_src"
[ -f "$prerm_src" ] || fail "prerm script missing: $prerm_src"
install -m 0755 "$postinst_src" "$control_dir/postinst"
install -m 0755 "$prerm_src" "$control_dir/prerm"

# ---- build --------------------------------------------------------------------
dpkg-deb --root-owner-group -Zxz --build "$pkg_dir" "$output_deb"

# ---- self-check: field values as the downstream pipeline expects ---------------
built_package="$(dpkg-deb -f "$output_deb" Package)"
built_arch="$(dpkg-deb -f "$output_deb" Architecture)"
[ "$built_package" = "$PACKAGE_NAME" ] || fail "built deb Package field is '$built_package', expected '$PACKAGE_NAME': $output_deb"
[ "$built_arch" = "$DEB_ARCH" ] || fail "built deb Architecture field is '$built_arch', expected '$DEB_ARCH': $output_deb"

asr_state="omitted"
if [ -n "$asr_engine" ]; then
  asr_state="included"
fi
echo "package-deb-loongarch64: built $output_deb (Installed-Size: ${installed_size_kb} KB, ASR: $asr_state)"
