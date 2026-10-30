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
    # skill_gate.rs (connect-grant fix wave, round-1 review): the
    # exact-footprint restore pin — the compensation must rebuild ONLY the
    # scopes the grant actually lifted, never downgrading another scope's
    # stored verdict to a liftable default-off row.
    "revoke_restores_exactly_the_scopes_the_grant_lifted",
    # Round-3 review wave: the remaining consent-critical pins of the
    # connect-grant feature. Each guards a fail-closed invariant a dropped
    # attribute would silently disable — the boot verdict guard and its
    # legacy cohort, the recovery hold, the in-lock ledger recheck, the
    # standing-consent negative, the materialization-failure revoke on both
    # command bodies, and the three round-3 revoke-contract pins
    # (covered-uninitialized footprint, grant-consumed predicate,
    # marker-faithful restoration).
    "connect_refresh_never_overrides_a_stored_verdict",
    "legacy_migrated_disable_survives_the_boot_refresh",
    "recovery_hold_keeps_the_boot_backfill_fail_closed",
    "revoke_rearms_a_recovery_hold_its_grant_cleared",
    "gesture_grant_rechecks_the_ledger_under_the_lock",
    "toggle_off_in_one_scope_removes_the_global_grant",
    "boot_refresh_never_reverts_a_verdict_after_a_failed_connect",
    "failed_materialization_leaves_a_standing_consent_alone",
    "failed_materialization_revokes_a_fresh_connect_grant",
    "revoke_restores_a_scope_that_materializes_mid_connect",
    "revoke_leaves_a_consumed_grant_alone",
    "revoke_restores_markers_only_where_the_lift_found_one",
    # Round-6 review P2: the compensating revoke owns its grant by the
    # operation token — a stale failure after a teardown + reconnect must
    # leave the replacement grant alone.
    "stale_revoke_leaves_a_replacement_grant_alone",
    # Round-7 review P2: an overlapping completion commits the relied-upon
    # standing grant (its gate captures the standing token), so the older
    # in-flight operation's compensating revoke stops matching; the commit
    # declines over a replaced owner so a newer operation's own failure
    # compensation still strips in full.
    "committed_relied_grant_survives_the_in_flight_revoke",
    "stale_commit_leaves_a_replacement_owner_alone",
    # Round-7 review P2, command-body level: the real apply_skills_command
    # with a controlled materializer commits the relied-upon standing grant
    # on success, so the in-flight operation's stale revoke preserves it.
    "connect_body_commits_the_relied_standing_grant",
    # Round-8 review P2: the commit is a completion requirement. A relied-upon
    # grant that is GONE at commit time (the owner operation's compensating
    # revoke won the race / a teardown swept it / a verdict consumed it) is
    # reported as lost and writes nothing; the real connect and refresh bodies
    # fail with the consent-sync copy instead of returning visible:true over
    # consent that computes OFF; a commit whose persist fails fails the
    # completion too, leaving the ownership claim standing.
    "revoked_relied_grant_reports_lost_and_commits_nothing",
    "connect_body_fails_when_the_relied_grant_vanishes_before_its_commit",
    "refresh_body_fails_when_the_relied_grant_vanishes_before_its_commit",
    "connect_body_fails_when_the_relied_grant_commit_cannot_persist",
    # Round-9 review P2 pair: the known-bundle shortcut captures the standing
    # grant's reliance token (the third gate exit — the one that dropped it),
    # and the commit-persist failure carries the commit-stage marker (still
    # enabled), never the fail-closed consent-sync copy.
    "known_bundle_shortcut_captures_the_standing_reliance_token",
    # Round-12 review P2/P3, the completion claim set: an operation whose
    # gate captured an EMPTY token (a fresh grant a real teardown consumed,
    # or committed tokenless consent) still validates the final state at
    # completion — declining over a newer owner's armed pending grant
    # (ReplacementPending), standing on permanent tokenless consent
    # (Superseded) — while a fresh grant's completion FINALIZES its own
    # token, and a completed owner supersedes a late relied completion
    # (both the completed and the genuinely-pending halves pinned).
    "teardown_regrant_between_gate_and_tail_declines_an_empty_capture_completion",
    "completion_over_committed_tokenless_consent_validates_the_final_state",
    "fresh_completion_finalizes_its_own_grant",
    "completed_owner_supersedes_a_late_relied_completion",
    "gateless_refresh_tail_validates_the_standing_consent_it_observed",
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


def ignored_source_pins():
    """Round-35 minor 10 (review #455): libtest's stable `--list` output cannot
    distinguish an #[ignore]d test from a live one, so scan the crate sources:
    any `fn <pin>` whose preceding attribute lines contain `#[ignore` is a
    silenced required pin."""
    src_root = os.path.join(os.path.dirname(MANIFEST), "src")
    hits = {}
    for root, _dirs, files in os.walk(src_root):
        for name in files:
            if not name.endswith(".rs"):
                continue
            path = os.path.join(root, name)
            try:
                lines = open(path, encoding="utf-8", errors="replace").read().splitlines()
            except OSError:
                continue
            for i, line in enumerate(lines):
                stripped = line.lstrip()
                body = stripped[6:] if stripped.startswith("async ") else stripped
                if body.startswith("fn ") and "{" in body:
                    fn_name = body[3:].split("(")[0].strip()
                    window = "\n".join(lines[max(0, i - 4):i])
                    if "#[ignore" in window:
                        hits.setdefault(fn_name, path)
    return hits


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

    # Pin matching (round-35 minor 10, review #455): per full-path match
    # count — exactly once — plus a source-level #[ignore] scan (stable
    # libtest --list cannot flag ignored tests). A name-collision test
    # elsewhere no longer satisfies a pin whose real test was deleted, and an
    # ignored pin is reported instead of passing silently.
    full_path_matches = collections.Counter(paths)
    ignored_src = ignored_source_pins()
    for pin in REQUIRED_PINS:
        matches = [p for p in paths if p.rsplit("::", 1)[-1] == pin]
        if not matches:
            failures.append(f"required pin absent from the registry (stolen #[test] attribute?): {pin}")
        elif len(matches) > 1:
            failures.append(f"required pin registered more than once: {matches}")
        elif pin in ignored_src:
            failures.append(f"required pin is #[ignore]d at {ignored_src[pin]}")

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
