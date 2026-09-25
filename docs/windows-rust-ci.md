# Windows Rust CI

## Coverage contract

`windows-rust-test` is the required native-Windows **Rust** job (the required
gate also has `windows-codex-runtime-test`, which is out of scope here). It is
a two-leg matrix on `windows-latest` (`fail-fast: false`, `max-parallel: 2`);
the required gate aggregates the matrix result, so both legs must pass:

- `all-targets-check` runs the metadata-only checks (steps 4-5 below).
- `regression` links the `pinvou3_lib` test executable and runs everything
  that needs it (steps 6-10 below).

Routing is job level and identical for both legs. The job runs on every push
to `main` (cumulative Windows coverage plus cache warm-up) and on ready,
non-draft pull requests matching the `rust_full` or `cli_rust` paths filters,
or `rust_code` plus the explicit `ci:full-rust` label; Merge Queue, drafts,
closed, and non-Rust pull requests are skipped. The matrix changes
scheduling, not the set or failure semantics of the checks.

`rust_full` fails closed: all of `pinvou3-app/src-tauri/**/*.rs` plus shared
manifests (`Cargo.toml`, `Cargo.lock`, `deny.toml`, `build.rs`,
`pinvou3-app/src-tauri/.cargo/**`, `pinvou3-app/src-tauri/rust-toolchain.toml`,
the `CodeWhale` submodule, `.gitmodules`) trigger it, except the documented
low-risk leaf features `feedback`, `personas`, and `pet` (their registration
or app command surface still matches). `cli_rust` covers `pinvou-cli` Rust
paths and keeps `feedback` and `personas` gated because the CLI path-depends
on the app crate; only `pet` is exempt from both filters.

## What the job runs

The job shell is `bash`; three steps opt into `pwsh`. Steps 1-3 and the
cache restore run on both legs; the leg of every later step is noted. In
order:

1. Checkout (`submodules: false`), then
   `git submodule update --init --recursive -- CodeWhale`.
2. `dtolnay/rust-toolchain@stable`, then `rustup default` aligned to the
   `rust-toolchain.toml` pin (same toolchain as local development).
3. Compile `rustc-stack-wrapper.exe` (`rustc -O`) and export it as
   `RUSTC_WRAPPER`: compile-time-only `RUST_MIN_STACK=16MiB`; the `.exe` form
   avoids the cmd.exe 8191-character command-line limit.
   `AWS_LC_SYS_PREBUILT_NASM=1` substitutes for the NASM the runner lacks.
4. (`all-targets-check`) `cargo check --manifest-path
   pinvou3-app/src-tauri/Cargo.toml --all-targets --features dev-tools`.
5. (`all-targets-check`) `cargo check --manifest-path pinvou-cli/Cargo.toml --workspace
   --all-targets --locked`: the CLI's Windows-only branches (exe/cmd
   candidates, `cmd /D /S /C` shims, taskkill tree kill, `CREATE_NO_WINDOW`)
   compile-check only on a Windows runner.
6. (`regression`) Link check: `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml
   --lib --no-run --message-format=json`, capturing the `pinvou3_lib` test
   executable as `PINVOU3_TEST_EXE`.
7. (`regression`) Embed the Common-Controls v6 manifest (resource `#1`) with the Windows SDK
   `mt.exe`: `muda` statically imports `TaskDialogIndirect`, which exists only
   in the Common-Controls v6 side-by-side assembly, and Windows ignores a
   side-by-side `<exe>.manifest` once `link.exe` embedded a default one.
8. (`regression`) Run `python scripts/ci-windows-imports-diagnose.py` on `PINVOU3_TEST_EXE`
   — a non-blocking PE import-table diagnostic (`continue-on-error`), after
   step 7 so the embedded manifest exempts SxS DLLs such as `comctl32`.
9. (`regression`) Run the CodeWhale PowerShell regression filters
   (`forkguard_powershell` and `forkguard_windows_shell_text`) from the
   dependency crate itself. The parent application jobs do not execute a
   dependency crate's lib tests. The step unsets `SHELL` so the Windows
   fallback to `pwsh.exe` is deterministic, and each filter must match at
   least one test so a rename cannot silently pass.
10. (`regression`) Regression loop: run the patched application binary directly — re-invoking
   `cargo test` could relink and drop the embedded manifest — once per filter
   with `--test-threads=1`; each filter must match at least one test
   (`running [1-9][0-9]* tests?`) so a renamed test fails loudly:

   - `platform::filesystem::tests::windows_`
   - `platform::paths::tests::managed_python_`
   - `artifact_read_recovers_an_occupied_1177_layout_after_release`
   - `artifact_writes_never_expose_partial_utf8_to_concurrent_readers`
   - `artifact_public_read_`
   - `features::voice::platform::windows::tests::closed_temp_wav_can_be_reopened_when_asr_denies_write_sharing`
   - `features::memory::tests::topic_migration_`
   - `connector_introspection_guard_matches_complete_names_only`
   - `features::projects::tests::`
   - `run_with_timeout_reaps_and_stays_bounded`
   - `reap_killed_child_`
   - `features::browser::platform::windows::tests::`
   - `features::monitor::platform::windows_cpu::tests::`
   - `features::monitor::platform::windows_memory::tests::`
   - `features::monitor::platform::windows_gpu::tests::`
   - `features::remote_control::platform::windows::tests::`
   - `features::computer_use::platform::windows::tests::`

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

`Swatinem/rust-cache@v2` stores `pinvou3-app/src-tauri` and `CodeWhale` in one
entry under shared key `windows-rust-test`. CodeWhale is a path dependency,
not an application-workspace member, so its required lib tests need their own
target directory; including that directory in the existing entry is the
minimum cache shape for this coverage, not a second cache. The first `main`
run after this change may compile CodeWhale cold and is expected to add about
0.8–2GB compressed. Restore-key fallback then reuses the entry across lockfile
changes. Both legs restore the same entry on their isolated runners, but
`save-if` only allows the `regression` leg on `refs/heads/main` to write it
(that leg owns the linked application and CodeWhale test artifacts), so the
split adds neither a second writer nor a new namespace, pull requests never
rewrite the entry, and usage stays within the repository-wide 10GB budget.
The saved entry holds build (not check) artifacts, so the `all-targets-check`
leg compiles its metadata-only dependency graph without a warm cache; that is
the expected cost of the split and is visible in the timing lines below.
`pinvou-cli` has no separate cache. Node/npm setup is absent —
under the debug profile tauri's `generate_context!` uses `devUrl`, `dist/`
is never packaged, and `build.rs` only depends on `tauri-build`/`cc` —
saving 3-5 minutes per run.

## Duration, timeout, and failure diagnosis

Each run emits only aggregate, non-sensitive diagnostics:

- `WINDOWS_RUST_CACHE` records the matrix phase, target presence, the
  fingerprint-crate count and the direct dependency-artifact count, without
  recursively scanning the large target tree.
- `WINDOWS_RUST_TIMING` records the phase, a fixed step or filter identifier,
  the elapsed seconds and the exit status for CodeWhale initialization, the
  two checks, the test link, the CodeWhale PowerShell regressions, each
  regression filter and the whole filter suite.

No environment values, credentials, or cache keys are printed. Compare at
least three high-risk PR or `main` runs using these lines; if the legs do not
overlap, or the check leg becomes the slower path, revert the matrix
scheduling while keeping the metrics.

Passing runs historically took 85-87 minutes; since 2026-09 several died at
the previous 90-minute cap during the link step, and run 34802015051 was
cancelled mid-build after PR #478 added the full `pinvou-cli` workspace
compile. `timeout-minutes` is now 180, with headroom for two cold workspaces;
the cap applies per leg, since either leg can still compile cold on a cache
miss.

On failure, read the import-diagnostic output (step 8) and the failing filter
name (steps 9–10); cache restore misses stay visible rollback signals. Do not
recover time by removing a regression filter, moving a step to the other leg
without its prerequisites, skipping the manifest or import contract, changing
failures to warnings, or adding another independent large target cache beyond
the single two-workspace entry documented above.

Source of truth: the `windows-rust-test` job in
`.github/workflows/pr-check.yml`; on any mismatch the workflow wins.
