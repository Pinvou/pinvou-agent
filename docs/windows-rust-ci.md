# Windows Rust CI

## Coverage contract

`windows-rust-test` is the required native-Windows **Rust** job (the required
gate also has `windows-codex-runtime-test`, `macos-codex-runtime-test`,
`windows-rustup-repair-test`, and `relay-test`, see the related-jobs section).
It is a two-leg matrix on `windows-latest` (`fail-fast: false`,
`max-parallel: 2`); the required gate aggregates the matrix result, so both
legs must pass:

- `all-targets-check` runs the metadata-only checks (steps 8-10 below).
- `regression` links the `pinvou3_lib` test executable and runs everything
  that needs it (steps 11-16 below).

Routing is job level and identical for both legs. The job runs on every push
to `main` (cumulative Windows coverage plus cache warm-up) and on ready,
non-draft pull requests matching the `rust_full` or `cli_rust` paths filters,
or `rust_code` plus the explicit `ci:full-rust` label; Merge Queue does not
repeat these legs (Linux validates the combined tree), and drafts, closed,
and non-Rust pull requests are skipped. The matrix changes scheduling, not
the set or failure semantics of the checks.

`rust_full` fails closed: all of `pinvou3-app/src-tauri/**/*.rs` plus shared
manifests (`**/Cargo.toml`, `**/Cargo.lock`, `**/deny.toml`, `**/build.rs`,
`pinvou3-app/src-tauri/.cargo/**`, `pinvou3-app/src-tauri/rust-toolchain.toml`,
the `CodeWhale` submodule, `.gitmodules`) and the loongarch64 cross-pipeline
build scripts the rust legs execute (`scripts/ci-loongarch64-toolchain.sh`,
`scripts/ci-loongarch64-sysroot.sh`, `scripts/ci-build-linux-loongarch64.sh`)
trigger it, except the documented low-risk leaf features `feedback`,
`personas`, and `pet` (their registration or app command surface still
matches). `cli_rust` covers `pinvou-cli` Rust paths plus the compile inputs
the CLI embeds (`pinvou-cli/.cargo/**`, `pinvou-cli/**/*.manifest`,
`pinvou3-app/src-tauri/tauri.conf.json`, the `CodeWhale` submodule, the
connector lock tables `pinvou3-app/src-tauri/resources/platforms/**`, the
bundle resources `pinvou3-app/src-tauri/resources/common/bundle/**` and
`pinvou3-app/resources/mcp-servers/**`, and the embedded docs
`docs/plugin-package-spec.md` / `docs/gaia-benchmark.md`), the pipewire
setup script its legs run (`scripts/ci-libpipewire-build.sh`), and keeps all
three leaf features (`feedback`, `personas`, `pet`) gated through the CLI
suite because the CLI path-depends on the app crate — they are exempt only
from the full app suite, never from pre-merge Windows coverage.

## What the job runs

The job shell is `bash`; five steps opt into `pwsh`. Steps 1-7 (the cache
restore is step 6) run on both legs; the leg of every later step is noted. In
order:

1. Checkout (`submodules: false`), then
   `git submodule update --init --recursive -- CodeWhale`.
2. `dtolnay/rust-toolchain@stable`, then `rustup default` aligned to whatever
   the `rust-toolchain.toml` selects (same toolchain as local development).
3. (`both legs`, `pwsh`) `dev profile 内存边界覆盖(codewhale-tui O0,仅本 job)`:
   append `[profile.dev.package.codewhale-tui] opt-level = 0` to
   `CARGO_HOME/config.toml` before anything compiles — a package-level
   profile override can only come from config, and the repo `Cargo.toml`
   stays untouched (see "Link memory on the hosted runner" for why both
   legs need this crate back at O0).
4. Compile `rustc-stack-wrapper.exe` (`rustc -O`) and export it as
   `RUSTC_WRAPPER`: compile-time-only `RUST_MIN_STACK=16MiB`; the `.exe` form
   avoids the cmd.exe 8191-character command-line limit.
   `AWS_LC_SYS_PREBUILT_NASM=1` substitutes for the NASM the runner lacks.
5. (`both legs`) `Probe and export the Windows lld link flags`: compile a
   hello-world with `-C linker-features=+lld` (falling back to the explicit
   `-C linker=rust-lld -C linker-flavor=lld-link` pair), always with
   `-C link-arg=/threads:1`, and export the working flags as `RUSTFLAGS` via
   `GITHUB_ENV`, so an unsupported flag fails in seconds instead of hours
   into the cold build. See the "Link memory on the hosted runner" section
   for why lld is required.
6. Cargo cache restore — see the "Cache" section.
7. (`both legs`, `pwsh`) `Windows Rust cache baseline diagnostics`: print the
   `WINDOWS_RUST_CACHE` marker (target presence plus fingerprint and
   direct dependency-artifact counts, no recursive scan) so cold-cache
   regressions are visible per leg without leaking cache contents.
8. (`all-targets-check`) `cargo check --manifest-path
   pinvou3-app/src-tauri/Cargo.toml --all-targets --features dev-tools
   --locked`.
9. (`all-targets-check`) `cargo check --manifest-path pinvou-cli/Cargo.toml
   --workspace --all-targets --locked`: the CLI's Windows-only branches
   (exe/cmd candidates, `cmd /D /S /C` shims, taskkill tree kill,
   `CREATE_NO_WINDOW`) compile-check only on a Windows runner.
10. (`all-targets-check`) `cargo check --manifest-path pinvou-cli/Cargo.toml
    -p adapter-gaia --features test-support --all-targets --locked`: the
    `dataset_contract` integration target is gated behind
    `required-features = ["test-support"]` and is silently skipped by the
    workspace-wide run of step 9, so this leg must at least compile it
    (`--all-targets` is required: a bare `cargo check -p` builds lib+bins
    only, and `required-features` can only filter targets out of the selected
    set, never add the test target in).
11. (`regression`, `pwsh`) Link check: `cargo test --manifest-path
    pinvou3-app/src-tauri/Cargo.toml --lib --no-run --locked --features
    benchmark-hooks --message-format=json`, capturing the `pinvou3_lib` test
    executable as `PINVOU3_TEST_EXE`. The `benchmark-hooks` feature compiles
    the headless-bridge contract module that step 15's
    `windows_attachment_runtime_stays_security_gated` filter pins; without it
    that filter would match zero tests. The yaml records the leg's wall clock
    rising accordingly (~21m → ~56m) as the price of the filters matching a
    real test binary.
12. (`regression`, `pwsh`) Embed the Common-Controls v6 manifest (resource `#1`) with the Windows SDK
    `mt.exe`: `muda` statically imports `TaskDialogIndirect`, which exists only
    in the Common-Controls v6 side-by-side assembly, and Windows ignores a
    side-by-side `<exe>.manifest` once `link.exe` embedded a default one.
13. (`regression`, `pwsh`) Run `python scripts/ci-windows-imports-diagnose.py` on `PINVOU3_TEST_EXE`
    — a non-blocking PE import-table diagnostic (`continue-on-error`), after
    step 12 so the embedded manifest exempts SxS DLLs such as `comctl32`.
14. (`regression`) Run the CodeWhale Windows PowerShell regression filters
    (`forkguard_powershell`, `forkguard_windows_shell_text`,
    `command_spec_env_overlays_preserve_existing_values`,
    `forkguard_windows_shell_env_overlays_keep_python_stdio_utf8`,
    `windows_shell_env_explicit_override_is_case_insensitive`) from the
    dependency crate itself. The parent application jobs do not execute a
    dependency crate's lib tests. The step pins `RUSTUP_TOOLCHAIN` to the
    aligned toolchain — the env var overrides CodeWhale's own
    `rust-toolchain.toml`, so the CodeWhale sources here compile with the
    same compiler as every other leg. The step unsets `SHELL` so the Windows
    fallback to `pwsh.exe` is deterministic, and each filter must match at
    least one test so a rename cannot silently pass.
15. (`regression`) `Windows 原子替换状态机回归`: run the patched application
    binary directly — re-invoking `cargo test` could relink and drop the
    embedded manifest — once per filter with `--test-threads=1`; each filter
    must match at least one test (`running [1-9][0-9]* tests?`) so a renamed
    test fails loudly:

    - `platform::filesystem::tests::windows_`
    - `platform::paths::tests::managed_python_`
    - `cross_process_lock`
    - `artifact_read_recovers_an_occupied_1177_layout_after_release`
    - `artifact_writes_never_expose_partial_utf8_to_concurrent_readers`
    - `artifact_public_read_`
    - `features::voice::platform::windows::tests::closed_temp_wav_can_be_reopened_when_asr_denies_write_sharing`
    - `features::memory::tests::topic_migration_`
    - `connector_introspection_guard_matches_complete_names_only`
    - `multiagent_depth_guard_enforces_two_level_inputs`
    - `features::marketplace::tests::reinstall_upload_with_unlocked_pip_dependencies_contract`
    - `features::marketplace::scope::tests::`
    - `before_landing`
    - `before_anything_lands`
    - `preserves_consent_state`
    - `command_bodies_register_deny_first`
    - `vacuous_uninstall_keeps_fresh_consent_entries`
    - `standalone_tencent_docs_preset_deny_entry_excludes_the_materialized_dir`
    - `restore_of_package_missing_entry_registers_no_deny_entry`
    - `standalone_preset_install_never_folds_package_ids_on_save_or_remove`
    - `owner_claimed`
    - `import_rejects_case_variant_connector_ids`
    - `strips_under_import_lock`
    - `standalone_components_deny_first`
    - `market_id_collisions`
    - `keeps_deny_entries`
    - `keeps_deny_entry_when`
    - `stale_store_record_survives`
    - `leaves_no_deny_entry`
    - `entries_in_lock`
    - `install_unknown_ids_write_no_consent_state`
    - `connector_uninstall_withdraws_materialized_dirs`
    - `physically_uninstalls_preset_companions`
    - `install_waits_for_the_per_id`
    - `restore_waits_for_the_cross_process`
    - `manifest_load_failure`
    - `freeze_persist_failures_mirror`
    - `keeps_mark_on_failure`
    - `cleanup_defers_consent_strip`
    - `pre_land_gate_runs_under_the_landing_mark`
    - `uninstall_waits_for_the_cross_process`
    - `reconcile_spares_a_live_import`
    - `quarantine_of_empty_content`
    - `consent_failure_message`
    - `cli_bundle_of_skill_folds_case_variant`
    - `parks_while_a_peer_holds_the_landing_lease`
    - `cleanup_keeps_consent_rows_when_uninstall_rolls_back`
    - `uninstall_refuses_hostile_ids`
    - `install_gates_refuse_companion_claimed`
    - `restores_the_reimport_backup`
    - `restored_backup_quarantines`
    - `holds_the_landing_lease_across`
    - `holds_the_landing_lease_through`
    - `crash_stranded_reimport_backup`
    - `marker_directory_aborts`
    - `claimed_companion_name`
    - `features::projects::tests::`
    - `features::sessions::`
    - `run_with_timeout_reaps_and_stays_bounded`
    - `reap_killed_child_`
    - `features::browser::platform::windows::tests::`
    - `features::monitor::platform::windows_cpu::tests::`
    - `features::monitor::platform::windows_memory::tests::`
    - `features::monitor::platform::windows_gpu::tests::`
    - `features::remote_control::platform::windows::tests::`
    - `features::computer_use::platform::windows::tests::`
    - `failed_probe_never_deletes_an_existing_store_windows`
    - `features::voice_shortcut::platform::tests::`
    - `app::commands::dependencies::platform::windows::tests::`
    - `features::codex_acp::platform::windows::tests::`
    - `platform::os::windows::`
    - `windows_attachment_runtime_stays_security_gated`

16. (`regression`) `pinvou-cli Windows-gated unit tests`: the `pinvou-cli`
    workspace's own cfg(windows) unit tests run on no other leg — the Linux
    CLI leg cannot compile them and the all-targets-check leg only
    type-checks them — so this step runs each pinned filter directly with
    per-filter zero-match and `test result: ok` guards:
    `windows_batch_tests` (the BatBadBut std-batch hardening lock),
    `rebind_nesting_rejection_folds_case_where_the_os_folds` and
    `rebind_noop_fold_still_collapses_windows_alias_spellings`
    (`pinvou-cli`), `dataset_windows` and `fetch_windows_acl`
    (`adapter-gaia`), and `private_prediction::tests::windows_` plus
    `windows_private_acl::tests::` (`benchmark-core`).

## Related jobs that share this gate

The `windows-rust-test` job is not the only native-Windows or routing entry
in `pr-check.yml`; the doc covers the others so the routing stays
understandable in one place:

- `windows-codex-runtime-test` (`windows_codex`/`acp_runtime` filters): a
  15-minute Windows job natively running the browser-wrapper
  lifecycle/lazy regressions, the runtime manifest lifecycle contract, the
  runtime packaging contract, and the ACP bridge runtime smoke.
- `macos-codex-runtime-test` (`acp_runtime` filter): a 20-minute macOS job
  that prepares the universal ACP Bridge and runs the native Codex/Claude
  runtime packaging smoke.
- `windows-rustup-repair-test` (`windows_rustup_repair` filter): installs
  the pinned toolchain in an isolated directory, breaks it, and verifies
  the download-source fallback and reinstall path
  (`test:windows-rustup-repair`).
- `relay-test` (`relay` filter): a 10-minute Linux job running the
  remote-control relay suite (`npm --prefix remote-control-relay test`).

## Why the legs are independent

The default feature graph includes `local-embed`; `dev-tools` adds no
dependency but enables the `dump_system_prompt` binary and gives the package a
distinct feature fingerprint. More importantly, `cargo check` produces
metadata-only artifacts, while `cargo test --lib --no-run` must generate and
link executable code for the default-feature test harness, so running the
check first never turns the link into an incremental step. The serial job put
both compile graphs on one critical path; the split makes the wall time the
slower leg (`max(setup + checks, setup + link + regressions)`) instead of the
sum. CodeWhale initialization stays on both legs because the application
path-depends on its crates.

## Cache

`Swatinem/rust-cache@v2` stores `pinvou3-app/src-tauri`, `CodeWhale`, and
`pinvou-cli` in one entry under shared key `windows-rust-test`. CodeWhale is a
path dependency, not an application-workspace member, so its required lib tests
need their own target directory; including that directory in the existing entry
is the minimum cache shape for this coverage, not a second cache. The first
`main` run after adding CodeWhale to the entry may compile it cold and is
expected to add about 0.8–2GB compressed. Restore-key fallback then reuses the
entry across lockfile changes. Both legs restore the same entry on their
isolated runners, but
`save-if` only allows the `regression` leg on `refs/heads/main` to write it
(that leg owns the linked application and CodeWhale test artifacts; the step
also sets `cache-on-failure: true`, so a failed `main` regression run still
saves the compiled artifacts — otherwise the namespace stays cold until a
green run lands), so the
split adds neither a second writer nor a new namespace, pull requests never
rewrite the entry, and usage stays within the repository-wide 10GB budget.
The saved entry holds the regression leg's build artifacts, so much of the
dependency graph stays warm for the `all-targets-check` leg; the application
crate's own `--all-targets --features dev-tools` metadata is the part compiled
cold on every run. That residual cost is visible in the timing lines below.
The `pinvou-cli` workspace joined the entry on 2026-10-15 (both legs compile
it — the check leg's manifest checks and the regression leg's filtered CLI
test runs — so its target directory needs caching here; before that every
run cold-compiled the CLI tree on the slowest leg). Node/npm setup is absent —
under the debug profile tauri's `generate_context!` uses `devUrl`, `dist/`
is never packaged, and `build.rs` only depends on `tauri-build`/`cc` —
saving 3-5 minutes per run.

## Link memory on the hosted runner

The `regression` leg's cold test-binary link can OOM the 16 GB hosted runner.
The failure mode is not a crash but silent pagefile thrash: the 2026-10
stable-1.99 rollover reproduced it twice (runs 36919362322 and 36940184038),
with the whole dependency graph compiling in ~25 minutes and the final link
producing zero output for the remaining hours until the cap killed the leg.
The job therefore mirrors the recipe the linux `rust-test` leg measured for
the same OOM: `CARGO_PROFILE_DEV_LTO=thin` (cross-crate dead-code elimination
shrinks the test binary; cargo compiles dependency crates with
`-C linker-plugin-lto`, and the final crate's rustc consumes that bitcode
and runs the ThinLTO backends in-process — the linker only ever receives
post-LTO objects), `CARGO_PROFILE_DEV_DEBUG=0` (no DWARF; panic file:line
comes from `Location` rodata), and `RUSTFLAGS` selecting the bundled lld
with `/threads:1` so lld-link's internal threading and the final link stay
serial (the linux leg uses the gcc-driver equivalents `-fuse-ld=lld`,
`--thinlto-jobs=1`, `--threads=1`; `--thinlto-jobs` is a no-op on both legs
because no bitcode reaches the linker — it stays only pending a separately
evaluated removal). Growing the hosted runner's pagefile is not an option: a
pagefile change requires a reboot a CI job cannot perform,
so peak link memory is cut below RAM instead. The linker-selection flags are
probed per toolchain by step 5, since the stable surface for picking lld has
moved between releases. The step 3 `codewhale-tui` dev-profile override is
the same memory discipline applied at compile time rather than link time.

Peak memory is also cut before the link: both `rust-test` legs pin
`codewhale-tui` back to dev-default opt-level 0 by appending a
`[profile.dev.package.codewhale-tui]` section to `CARGO_HOME/config.toml`
(a package-level profile override can only come from config — cargo has no
environment-variable equivalent — and the repo's `Cargo.toml` is left
untouched, so local builds and release artifacts are unaffected). The
v0.9.12 base restored dependency-level O2 and the 16 GB hosted runners
went out-of-memory: on the windows leg rustc/LLVM died compiling that
crate itself, while on the linux leg the crate's O2-sized LLVM IR pushed
the final-crate thin-LTO compile (the `pinvou3-tauri` lib test) over the
memory watchdog; on the windows leg, converging codegen-units instead
stretched that crate's compile time far beyond its earlier timings
(measured), so the O0 override is that leg's mitigation.
The linux leg complements it with `CARGO_PROFILE_DEV_CODEGEN_UNITS=16`
(the dev default of 256 codegen units expands the large test harness into
too many ThinLTO inputs and amplifies the link peak).

## Duration, timeout, and failure diagnosis

Each run emits only aggregate, non-sensitive diagnostics:

- `WINDOWS_RUST_CACHE` records the matrix phase, target presence, the
  fingerprint-crate count and the direct dependency-artifact count, without
  recursively scanning the large target tree.
- `WINDOWS_RUST_TIMING` records the phase, a fixed step or filter identifier,
  the elapsed seconds and the exit status for CodeWhale initialization, the
  two cargo check steps, the test link, the CodeWhale PowerShell regressions, each
  regression filter, the whole filter suite and the CLI Windows unit
  tests.

No environment values, credentials, or cache keys are printed. Compare at
least three high-risk PR or `main` runs using these lines; if the legs do not
overlap, or the check leg becomes the slower path, revert the matrix
scheduling while keeping the metrics.

Passing runs historically took 85-87 minutes; since 2026-09 several died at
the previous 90-minute cap during the link step, and run 34802015051 was
cancelled mid-build after PR #478 added the full `pinvou-cli` workspace
compile, so the cap moved to 180. The 2026-10 stable rollover to 1.99.0 then
invalidated the rust-cache key and the cold test-binary link OOM'd (see the
previous section), killed at the 180-minute cap (run 36919362322) and then,
after the cap moved to 240, at that cap too (run 36940184038) — the raise
alone could not fix an out-of-memory link. `timeout-minutes` is therefore 240
together with the link-memory recipe above, with headroom for one fully cold
toolchain rollover per channel bump; the cap
applies per leg, since either leg can still compile cold on a cache miss.

On failure, read the import-diagnostic output (step 13) and the failing filter
name (steps 14-16); cache restore misses stay visible rollback signals. Do not
recover time by removing a regression filter, moving a step to the other leg
without its prerequisites, skipping the manifest or import contract, changing
failures to warnings, or adding another independent large target cache beyond
the single three-workspace entry documented above.

Source of truth: the `windows-rust-test` job in
`.github/workflows/pr-check.yml`; on any mismatch the workflow wins.
