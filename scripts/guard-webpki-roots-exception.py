#!/usr/bin/env python3
"""Enforce change control for the pinned webpki-roots wrapper graphs.

Requires Python 3.11+. No Cargo, network, registry cache or submodule is needed.
Run this before cargo-deny; advisory scanning remains independent and unignored.
"""

from pathlib import Path
import sys
try:
    import tomllib
except ModuleNotFoundError:
    sys.exit('webpki-roots guard requires Python 3.11+ (tomllib)')


LOCKFILES = (
    "pinvou-knowledge/Cargo.lock",
    "pinvou3-app/src-tauri/Cargo.lock",
    "pinvou-cli/Cargo.lock",
)
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
# Pinned baseline as recorded in the three lockfiles at the time this guard
# landed (see docs/webpki-roots-exception.md). The 0.26.11 entry is the
# semver-compatibility shim that only re-exports the 1.x root store; 1.0.11
# is the pinned baseline of this repository. Any version, checksum or
# dependency-edge change requires re-pinning through a review of this table,
# not a widening of the accepted set.
REVIEWED = {
    "0.26.11": {
        "checksum": "521bc38abb08001b01866da9f51eb7c5d647a19260e00054a8c7fd5f9e57f7a9",
        "dependencies": ["webpki-roots 1.0.11"],
    },
    "1.0.11": {
        "checksum": "f5f280bbb2b287bc8e7de7e3852945a0ff50d1bbcb5ed42b6c7a568c546df171",
        "dependencies": ["rustls-pki-types"],
    },
}


class GuardError(ValueError):
    pass


def validate_lock(value, label):
    if not isinstance(value, dict) or type(value.get("version")) is not int or value["version"] != 4:
        raise GuardError(f"{label}: unsupported lockfile format")
    packages = value.get("package")
    if not isinstance(packages, list) or not all(isinstance(row, dict) for row in packages):
        raise GuardError(f"{label}: invalid package inventory")
    roots = [row for row in packages if row.get("name") == "webpki-roots"]
    # Exactly two packages: duplicate versions (including alternate sources),
    # older roots and unreviewed future versions require a new graph review.
    if (len(roots) != 2
            or not all(isinstance(row.get("version"), str) for row in roots)
            or sorted(row["version"] for row in roots) != sorted(REVIEWED)):
        raise GuardError(f"{label}: expected exactly the two pinned webpki-roots packages")
    for row in roots:
        expected = REVIEWED[row["version"]]
        if row.get("source") != REGISTRY:
            raise GuardError(f"{label}: webpki-roots source is not the pinned registry")
        if row.get("checksum") != expected["checksum"]:
            raise GuardError(f"{label}: webpki-roots checksum changed; reassess the pinned graph")
        if row.get("dependencies") != expected["dependencies"]:
            raise GuardError(f"{label}: webpki-roots dependency edge changed; reassess the pinned graph")


def validate_repository(root):
    # There is deliberately no CLI selector: every use of the shared policy
    # checks every consumer, including the CLI's independently resolved lock.
    for relative in LOCKFILES:
        try:
            with (root / relative).open("rb") as lock:
                value = tomllib.load(lock)
        except (OSError, ValueError, RecursionError) as error:
            raise GuardError(f"{relative}: cannot read a valid lockfile") from error
        validate_lock(value, relative)


def main():
    if len(sys.argv) != 1:
        print("webpki-roots guard takes no arguments; all three lockfiles are required", file=sys.stderr)
        return 1
    try:
        validate_repository(Path(__file__).resolve().parents[1])
    except GuardError as error:
        print(f"webpki-roots graph rejected: {error}; see docs/webpki-roots-exception.md", file=sys.stderr)
        return 1
    print("webpki-roots graph: all three pinned lock graphs verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
