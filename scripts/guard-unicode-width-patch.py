#!/usr/bin/env python3
"""Guard the CJK-aware unicode-width patch in the app-facing workspaces.

The CodeWhale root workspace patches crates.io unicode-width 0.2.2 with a
CJK-aware copy (upstream codewhale-hq/Codewhale issue #4479, fixed by
upstream PR #5001, landed in the submodule as 001032646) so the circled
U+2460-24FF / U+2776-2793 / U+3248-324F ranges measure as 2 columns.
pinvou-cli and pinvou3-app/src-tauri resolve the same graph and must keep
declaring the same patch. This guard fails loudly when:

- a workspace [patch.crates-io] table disappears or points elsewhere,
- the patched copy loses its crate identity or the circled-digit
  reclassification that makes it CJK-aware,
- either Cargo.lock resolves unicode-width from the registry instead of
  the patched path.

Pure stdlib file checks: no Cargo, no network, no Python version floor.
The CodeWhale submodule working tree must be initialized (both guarded
workspaces already require it to build at all).
"""

from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
PATCH_DIR_REL = "CodeWhale/patches/unicode-width-0.2.2"
# workspace -> the exact patch path its Cargo.toml must declare.
WORKSPACES = {
    "pinvou-cli": "../" + PATCH_DIR_REL,
    "pinvou3-app/src-tauri": "../../" + PATCH_DIR_REL,
}
# Markers of the three reclassified ranges in the patched src/lib.rs;
# upstream 0.2.2 lib.rs mentions none of them.
REQUIRED_RANGE_MARKERS = ("2460", "2776", "3248")


class GuardError(ValueError):
    pass


def check_patch_dir(patch_root: Path) -> None:
    manifest = (patch_root / "Cargo.toml").read_text(encoding="utf-8")
    if 'name = "unicode-width"' not in manifest or 'version = "0.2.2"' not in manifest:
        raise GuardError(
            f"{PATCH_DIR_REL}/Cargo.toml: expected the unicode-width 0.2.2 manifest"
        )
    lib_rs = (patch_root / "src" / "lib.rs").read_text(encoding="utf-8")
    missing = [m for m in REQUIRED_RANGE_MARKERS if m not in lib_rs]
    if missing:
        raise GuardError(
            f"{PATCH_DIR_REL}/src/lib.rs: lost the circled-digit reclassification "
            f"(missing markers {missing}); the copy has gone vanilla"
        )


def check_patch_table(workspace: str, expected_path: str) -> None:
    manifest = (ROOT / workspace / "Cargo.toml").read_text(encoding="utf-8")
    expected_line = f'unicode-width = {{ path = "{expected_path}" }}'
    if expected_line not in manifest:
        raise GuardError(
            f"{workspace}/Cargo.toml: missing or drifted patch entry, "
            f"expected line: {expected_line}"
        )


def check_lock(workspace: str) -> None:
    lines = (ROOT / workspace / "Cargo.lock").read_text(encoding="utf-8").splitlines()
    blocks: list[list[str]] = []
    current: list[str] | None = None
    for line in lines:
        if line == "[[package]]":
            if current is not None:
                blocks.append(current)
            current = []
        elif current is not None:
            current.append(line)
    if current is not None:
        blocks.append(current)
    entries = [
        block
        for block in blocks
        if any(line.strip() == 'name = "unicode-width"' for line in block)
    ]
    if len(entries) != 1:
        raise GuardError(
            f"{workspace}/Cargo.lock: expected exactly one unicode-width "
            f"package entry, found {len(entries)}"
        )
    if any('source = "registry' in line for line in entries[0]):
        raise GuardError(
            f"{workspace}/Cargo.lock: unicode-width resolves from the "
            "registry instead of the patched path"
        )


def main() -> int:
    patch_root = ROOT / PATCH_DIR_REL
    if not patch_root.is_dir():
        raise GuardError(
            f"{PATCH_DIR_REL} not found — initialize the CodeWhale submodule first"
        )
    check_patch_dir(patch_root)
    for workspace, expected_path in WORKSPACES.items():
        check_patch_table(workspace, expected_path)
        check_lock(workspace)
    print("unicode-width patch guard: OK (tables, patched copy and locks consistent)")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except GuardError as exc:
        print(f"unicode-width patch guard: FAIL\n{exc}", file=sys.stderr)
        sys.exit(1)
