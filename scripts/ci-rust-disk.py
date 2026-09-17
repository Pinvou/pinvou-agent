#!/usr/bin/env python3
"""Reserve disk for Rust jobs on disposable GitHub Linux runners.

Unused preinstalled SDKs may only be removed from two strict allowlist tiers:

- Base tier (default): /usr/local/lib/android, /usr/share/dotnet,
  /usr/local/.ghcup, plus every /usr/local/julia* directory, resolved to
  concrete paths at runtime via a pathlib glob (a non-directory match is
  refused instead of skipped — the cleanup fails closed).
- Aggressive tier (--aggressive): additionally /opt/hostedtoolcache,
  /opt/google/chrome and /opt/microsoft/msedge. /opt/hostedtoolcache is also
  where setup-* actions install their tools, so the aggressive tier is only
  valid before any setup-* action has run in the job; the calling workflow
  must guarantee that ordering (see AGGRESSIVE_SDK_PATHS below).

Never clean the checkout, Cargo cache, toolchain, or build outputs: test
harnesses and release builds still need their root libraries plus dependency
artifacts. Every resolved allowlist path goes through the same symlink,
mount-point and workspace-overlap validation before anything is deleted, and
`--min-free-gib` (default 12) refuses undersized runners after cleanup.
"""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys


BASE_SDK_PATHS = (
    Path("/usr/local/lib/android"),
    Path("/usr/share/dotnet"),
    Path("/usr/local/.ghcup"),
)
# Base-tier glob entries: scanned at runtime and every match is validated
# exactly like the fixed allowlist paths above.
GLOB_SDK_DIRS = ((Path("/usr/local"), "julia*"),)
# Aggressive tier: only removed with --aggressive. /opt/hostedtoolcache is the
# install target of setup-* actions, so this tier may only run while no
# setup-* action of the job has executed yet; the calling workflow owns that
# ordering, this script never widens the allowlist implicitly.
AGGRESSIVE_SDK_PATHS = (
    Path("/opt/hostedtoolcache"),
    Path("/opt/google/chrome"),
    Path("/opt/microsoft/msedge"),
)
DEFAULT_MIN_FREE_GIB = 12
MIN_FREE_BYTES = DEFAULT_MIN_FREE_GIB * 1024**3
# du of a huge SDK tree could in principle stall; the measurement is purely
# informational, so it is hard-capped like every other external call.
DU_TIMEOUT_SECS = 120


def resolve_sdk_paths(aggressive: bool) -> tuple:
    """Resolve the allowlist for the requested tier, glob matches sorted."""
    paths = list(BASE_SDK_PATHS)
    for parent, pattern in GLOB_SDK_DIRS:
        paths.extend(sorted(parent.glob(pattern)))
    if aggressive:
        paths.extend(AGGRESSIVE_SDK_PATHS)
    return tuple(paths)


def _du_size(path: Path) -> str:
    """Human-readable `du -sh` size; a failing du must never block cleanup."""
    try:
        result = subprocess.run(
            ["du", "-sh", str(path)],
            capture_output=True,
            text=True,
            check=False,
            timeout=DU_TIMEOUT_SECS,
        )
    except (OSError, subprocess.TimeoutExpired):
        return "unknown size"
    if result.returncode == 0 and result.stdout.split():
        return result.stdout.split()[0]
    return "unknown size"


def prepare_disk(workspace: Path, *, aggressive: bool = False, min_free_gib: int = DEFAULT_MIN_FREE_GIB) -> None:
    if sys.platform != "linux" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise RuntimeError("Disk preparation is restricted to GitHub-hosted Linux runners")
    if min_free_gib < 1:
        # A zero/negative threshold would silently disable the ENOSPC gate.
        raise RuntimeError("min_free_gib must be >= 1")
    workspace = workspace.resolve(strict=True)
    if not workspace.is_dir() or not (workspace / ".github/workflows/pr-check.yml").is_file():
        raise RuntimeError("GITHUB_WORKSPACE must be the checked-out repository")

    # Validate the entire allowlist (fixed and glob-resolved paths alike)
    # before deleting anything, including parent symlinks and overlapping
    # workspace paths. Missing SDKs are harmless.
    targets = []
    for sdk in resolve_sdk_paths(aggressive):
        if sdk.resolve() != sdk or sdk.is_symlink():
            raise RuntimeError(f"Refusing redirected SDK path: {sdk}")
        if workspace == sdk or workspace in sdk.parents or sdk in workspace.parents:
            raise RuntimeError(f"Refusing SDK path overlapping the workspace: {sdk}")
        if sdk.exists():
            if not sdk.is_dir() or sdk.is_mount():
                raise RuntimeError(f"Refusing unexpected SDK path: {sdk}")
            targets.append(sdk)

    min_free_bytes = min_free_gib * 1024**3
    before = shutil.disk_usage(workspace).free
    print(f"Rust CI disk available before preparation: {before / 1024**3:.1f} GiB", flush=True)
    for sdk in targets:
        print(f"Removing unused hosted-runner SDK: {sdk} (du -sh: {_du_size(sdk)})", flush=True)
        item_free_before = shutil.disk_usage(workspace).free
        shutil.rmtree(sdk)
        freed = shutil.disk_usage(workspace).free - item_free_before
        print(f"Freed {freed / 1024**3:.2f} GiB by removing {sdk}", flush=True)
    after = shutil.disk_usage(workspace).free
    print(f"Rust CI disk available after preparation: {after / 1024**3:.1f} GiB", flush=True)
    if after < min_free_bytes:
        raise RuntimeError(
            f"Rust jobs require at least {min_free_gib} GiB free right after "
            "disk preparation; refusing an undersized runner (the build would ENOSPC)"
        )


def _min_free_gib_arg(value: str) -> int:
    """argparse type: a non-positive threshold would silently disable the
    ENOSPC fast-fail gate, so reject it at the CLI boundary."""
    try:
        parsed = int(value)
    except ValueError:
        raise argparse.ArgumentTypeError(f"invalid int value: {value!r}") from None
    if parsed < 1:
        raise argparse.ArgumentTypeError("--min-free-gib must be >= 1 GiB")
    return parsed


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description="Remove unused preinstalled SDKs on GitHub-hosted Linux runners to free disk for Rust jobs."
    )
    parser.add_argument(
        "--aggressive",
        action="store_true",
        help="also remove /opt/hostedtoolcache, /opt/google/chrome and /opt/microsoft/msedge; "
        "only valid before any setup-* action has run in the job",
    )
    parser.add_argument(
        "--min-free-gib",
        type=_min_free_gib_arg,
        default=DEFAULT_MIN_FREE_GIB,
        help=f"required free disk space in GiB after preparation (default: {DEFAULT_MIN_FREE_GIB})",
    )
    args = parser.parse_args(argv)
    try:
        workspace = os.environ.get("GITHUB_WORKSPACE", "").strip()
        if not workspace:
            raise RuntimeError("GITHUB_WORKSPACE is required")
        prepare_disk(Path(workspace), aggressive=args.aggressive, min_free_gib=args.min_free_gib)
    except (OSError, RuntimeError) as error:
        print(f"Rust CI disk preparation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
