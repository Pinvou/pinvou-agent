#!/usr/bin/env python3
"""Test-registry guard: fail on duplicate test registrations or missing pins.

A duplicated `#[test]` attribute registers one function twice while silently
stripping the attribute from its neighbor — both halves happened in PR #455
(round 26 and again in round 28), and raw run totals cannot catch the pair
because the inflation and the deflation cancel. Against
`cargo test --lib -- --list` this guard asserts:

  1. every test path is registered at most once (duplicate = inflated run);
  2. every consent-critical pin in REQUIRED_PINS is present exactly once
     (absent = a stolen attribute silenced the pin). Extend REQUIRED_PINS
     when a new consent/rollback-critical pin lands.

Platform caveat (review #455 round-30 m10): two REQUIRED_PINS are
`#[cfg(unix)]`-gated (they need an unreadable-file fixture), so a local run
only goes green on Linux — exactly where the CI rust-test leg wires this
guard; on macOS/Windows the missing unix pins are expected misses, not
thefts. The registry scope is the pinvou3-app/src-tauri crate; covering the
other test manifests (e.g. pinvou-cli) is a registered follow-up.

Run it wherever the lib test binary is already built (it reuses cargo's
artifacts); CI wires it into the linux rust-test leg right after
`cargo test --lib --no-run`.
"""

import collections
import os
import subprocess
import sys

MANIFEST = os.path.join("pinvou3-app", "src-tauri", "Cargo.toml")

# Consent/rollback-critical regression pins. A missing entry here means the
# pin's `#[test]` attribute was lost to an edit — the exact failure this
# guard exists for (PR #455 rounds 26 and 28).
REQUIRED_PINS = [
    # commands/tests.rs: the install-path consent rollback pair (round-33
    # minor 6, review #455) — exactly the consent/rollback class this guard
    # exists to protect from attribute theft.
    "install_validation_failure_rollback_leaves_no_consent_rows",
    "install_consent_sync_persist_failure_is_honest_and_keeps_pack",
    # scope.rs: exact-cleanup hijack pin (round-26 MAJOR 1) and the
    # state_changed hot-refresh gate pin (round-28 MINOR 3) — the pair whose
    # attributes were swapped by the round-28 insertion.
    "exact_cleanup_never_reowns_absent_dir_id_onto_foreign_claim",
    "enable_packages_state_changed_tracks_persisted_delta",
    # mod.rs: secrets resync keeps the previous registry on unreadable state,
    # and (round-30 MAJOR) a faulting mid-rebuild keyring read discards the
    # partial rebuild wholesale instead of leaving a half-rebuilt registry.
    "secret_values_resync_unreadable_registry_keeps_previous",
    "secret_values_resync_keyring_fault_keeps_previous_registry",
    # plugin_import.rs: rebaseline keeps the restore point through a supply
    # failure rollback.
    "unified_import_keeps_backup_through_supply_failure_rollback",
    # recycle_bin.rs: consent-gate collision pin, secrets-restore pin, and
    # the supply-failure rollback pin.
    "restore_consent_gate_never_reowns_collided_pack_id",
    "restore_secrets_pack_into_uninitialized_scope_persists_consent",
    "restore_mcp_supply_failure_rolls_back_to_recycle_bin",
    # mod.rs (round-34): a legacy scope file that exists but cannot be
    # consumed must fail closed over the DenyAll fallback, never migrate as
    # "nothing was disabled" while the wide signal initializes plain fully-on.
    "corrupt_legacy_file_fails_closed_not_implicitly_all_on",
    "schema_invalid_legacy_file_fails_closed",
]


def list_tests():
    cmd = [
        "cargo",
        "test",
        "--manifest-path",
        MANIFEST,
        "--lib",
        "--features",
        "benchmark-hooks",
        "--locked",
        "--",
        "--list",
        "--format",
        "terse",
    ]
    env = dict(os.environ, CARGO_TERM_COLOR="never")
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit("test-registry-guard: cargo test --list failed")
    paths = []
    for line in result.stdout.splitlines():
        if line.endswith(": test"):
            paths.append(line[: -len(": test")])
    if not paths:
        raise SystemExit("test-registry-guard: parsed zero tests — output format changed?")
    return paths


def main():
    paths = list_tests()
    failures = []

    counts = collections.Counter(paths)
    duplicates = sorted(p for p, n in counts.items() if n > 1)
    if duplicates:
        failures.append(
            "duplicate test registrations (a duplicated #[test] attribute "
            "silences a neighbouring pin):\n"
            + "\n".join(f"  {p} x{counts[p]}" for p in duplicates)
        )

    seen = {p.rsplit("::", 1)[-1] for p in paths}
    missing = sorted(pin for pin in REQUIRED_PINS if pin not in seen)
    if missing:
        failures.append(
            "required pins absent from the registry (stolen #[test] "
            "attribute?):\n" + "\n".join(f"  {p}" for p in missing)
        )

    if failures:
        print("test-registry-guard: FAILED", file=sys.stderr)
        for failure in failures:
            print(f"\n{failure}", file=sys.stderr)
        return 1

    print(f"test-registry-guard: ok — {len(paths)} unique test paths, "
          f"{len(REQUIRED_PINS)} required pins present")
    return 0


if __name__ == "__main__":
    sys.exit(main())
