# Release Artifact Size and Debug Info Policy

This document is the single reference for the binary linking + packaging size
optimization: which compression each artifact uses, which flag is pinned
where, and why certain options are deliberately rejected. The policy
assertions are pinned by `scripts/tests/test_release_size_policy.py`, which
the `fast-gate` job of `pr-check.yml` runs unconditionally on every PR
(unittest discover over `scripts/tests`); changing a flag requires updating
that test in the same PR.

## Overview of the levers (since 2026-09)

| Stage | Mechanism | Pinned at |
|---|---|---|
| Rust whole-graph optimization | `opt-level=3` + `lto="thin"` + `codegen-units=1` | `pinvou3-app/src-tauri/Cargo.toml [profile.release]` |
| Thin LTO executor | **rustc itself** (the final unit gets `-C lto=thin`; dependencies get `-C linker-plugin-lto`, whose objects are pure LLVM bitcode that rustc reads). Genuinely effective on all three platforms (Apple ld / MSVC link.exe / lld), **independent of the linker identity**. The Linux lld injection exists for link memory (proven OOM) and ICF, not for LTO | Cargo.toml comments; verified with cargo 1.98 verbose output |
| Dead-code elimination | rustc passes `--gc-sections` by default on GNU/ELF; `-dead_strip` by default on macOS | rustc built-in, nothing to configure |
| Identical code folding (ICF — semantically uniform, expressed per linker) | Linux: lld `--icf=safe` (RUSTFLAGS injection); Windows: rustc automatically passes `/OPT:REF,ICF` at opt-level>0 (same class of linker-side ICF); macOS: Apple ld/ld-prime have **no ICF capability** and switching to lld-MachO was explicitly rejected (see below) | `release-packages.yml` env of both Linux jobs |
| Debug info | `debug=false` + `strip="symbols"`: shipped artifacts carry zero DWARF/line tables/symbol tables. MSVC PDBs land in the build directory and never enter installers; macOS returns to uniform stripping on all OS versions (see below). Line tables live only in `release-fast` (CI smoke profile, never shipped), which sets `strip="none"` explicitly — the inherited `strip="symbols"` would otherwise strip the line tables at link time | `[profile.release]` / `[profile.release-fast]` |
| Build-machine paths | `--remap-path-prefix=<workspace>=/` (RUSTFLAGS of the Linux + macOS release jobs) | `release-packages.yml` |
| deb compression | data.tar repacked from tauri's hardcoded gzip-6 to xz -9 (`scripts/repack-deb-xz.sh`; `--root-owner-group` keeps root ownership; control.tar comes out as xz too) | `release-packages.yml` both Linux jobs + `scripts/release-deb.sh` |
| DMG compression | tauri emits UDZO without zlib-level (hdiutil default level 1); converted to **ULMO** (LZMA) after the build (mounting needs macOS 10.15+; our floor is 11.0) | `release-packages.yml` macOS job + `scripts/release-macos.sh` |
| NSIS compression | already solid LZMA (`bundle.windows.nsis.compression` in `config/platforms/windows/tauri.conf.json`; the tauri v2 default is the best tier) | no change needed |
| Frontend assets | tauri `compression` (default feature) embeds frontendDist with Brotli q9 | tauri built-in, no stronger tier |
| Windows OTA zip | `CompressionLevel::SmallestSize` (was Optimal); requires pwsh ≥ 6 — the value does not exist on .NET Framework / Windows PowerShell 5.1, so the script declares `#requires -Version 6` | `scripts/build-windows-ota.ps1` |
| knowledge-server ELF | the standalone ELF inside the deb now matches the main app: thin LTO + strip | `pinvou-knowledge/Cargo.toml [profile.release]` |

## macOS strip and the dyld alignment fix

The macOS 27 dyld added an 8-byte LINKEDIT string-pool alignment check, and
the strip output of rustc 1.96/1.97 was misaligned (proc-macro dylib dlopen
rejected → E0463), so CI injected `strip=none` on macOS 27+ runners. The bug
was fixed in LLVM and backported into **rustc 1.98.0**
(rust-lang/rust#157750, #158410; toolchain pinned in
`pinvou3-app/src-tauri/rust-toolchain.toml`). The workaround steps in both
workflows are removed and macOS artifacts return to the same
zero-symbol-table state as Linux/Windows.

## When debug symbols are needed (crash symbolization)

Release artifacts deliberately carry zero debug info; standalone debug
artifacts for symbolization (none of them enter any release artifact):

- macOS: temporarily add `split-debuginfo = "packed"` to `[profile.release]`
  (`debug` must not be false); rustc produces a `.dSYM`;
- Linux: `split-debuginfo = "packed"` produces a `.dwp` (stable);
- Windows: the PDB is always written to `target/` (rustc passes `/DEBUG`
  unconditionally on MSVC); pick it up directly.

## Deliberately rejected options (and why)

| Option | Reason |
|---|---|
| `panic = "abort"` (saves 5-10%) | The CodeWhale foundation uses `catch_unwind` to isolate a single tool panic from dragging down the session — a design constraint |
| Switching macOS/Windows to lld "to unify the ICF flag" | The folding policy is already semantically uniform across the three linkers (see overview); the literal flags differ only as each linker's expression: lld=`--icf=safe`, link.exe=`/OPT:REF,ICF` (rustc automatic). Switching linkers for literal uniformity is not worth it: macOS lld-MachO means handling the Tauri framework/signing chain (rejected in the header comment of `.cargo/config.toml`); Windows lld-link destabilizes the WebView2/COM link path. Also, lld is not a thin-LTO prerequisite (rustc performs it, see overview) |
| `trim-paths` (cargo profile) | Still nightly-only on rustc/cargo 1.98 (cargo#12137 not stabilized); stable `--remap-path-prefix` already achieves the same artifact-side path cleanup |
| `build.removeUnusedCommands` (tauri ≥2.4 size item) | Depends on ACL enumeration; this app's capabilities only reference core/dialog permissions and the custom commands rely on the default allow, so enabling it would prune all custom commands |
| `include_dir` compression | include_dir 0.7.4 has no compression feature; compressing the ~10MB embedded skill resources would be custom-code work for a separate project |
| MSI / AppImage / rpm compression tiers | MSI, AppImage and rpm are currently not in the release matrix (deb/dmg/nsis only); rpm has the `bundle.linux.rpm.compression` (zstd) knob — when rpm ships, set zstd directly |
| fat LTO (instead of thin) | Link memory exceeded the 16GB ubuntu runner and got SIGTERM (proven by historical runs); thin is the settled trade-off |

## Known boundaries

- A Developer-ID-signed DMG (private release pipeline) must be re-signed if
  the container is converted **after** signing; the community path in this
  repo (ad-hoc, `signingIdentity="-"`) does not sign the dmg and is
  unaffected.
- The `RUSTFLAGS` env takes priority over the rustflags of
  `.cargo/config.toml`: the Linux/macOS release jobs inject all flags through
  the env, and `.cargo/config.toml` deliberately hardcodes nothing
  (cross-platform linker availability, see its header comment).
- The deb repack uses `dpkg-deb`, available on Linux only; both callers (the
  CI jobs and release-deb.sh) are Linux environments.
