# Windows Rust CI

## Coverage contract

`windows-rust-test` is the required native-Windows **Rust** job (the required
gate also has `windows-codex-runtime-test`, which is out of scope here): one
run on
`windows-latest`, one serial pass. It runs on every push to `main` (cumulative
Windows coverage plus cache warm-up) and on ready, non-draft pull requests
matching the `rust_full` or `cli_rust` paths filters, or `rust_code` plus the
explicit `ci:full-rust` label; Merge Queue, drafts, closed, and non-Rust pull
requests are skipped.

`rust_full` fails closed: all of `pinvou3-app/src-tauri/**/*.rs` plus shared
manifests (`Cargo.toml`, `Cargo.lock`, `deny.toml`, `build.rs`,
`pinvou3-app/src-tauri/.cargo/**`, `pinvou3-app/src-tauri/rust-toolchain.toml`,
the `CodeWhale` submodule, `.gitmodules`) trigger it, except the documented
low-risk leaf features `feedback`, `personas`, and `pet` (their registration
or app command surface still matches). `cli_rust` covers `pinvou-cli` Rust
paths and keeps `feedback` and `personas` gated because the CLI path-depends
on the app crate; only `pet` is exempt from both filters.

## What the job runs

The job shell is `bash`; three steps opt into `pwsh`. In order:

1. Checkout (`submodules: false`), then
   `git submodule update --init --recursive -- CodeWhale`.
2. `dtolnay/rust-toolchain@stable`, then `rustup default` aligned to the
   `rust-toolchain.toml` pin (same toolchain as local development).
3. Compile `rustc-stack-wrapper.exe` (`rustc -O`) and export it as
   `RUSTC_WRAPPER`: compile-time-only `RUST_MIN_STACK=16MiB`; the `.exe` form
   avoids the cmd.exe 8191-character command-line limit.
   `AWS_LC_SYS_PREBUILT_NASM=1` substitutes for the NASM the runner lacks.
4. `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml --all-targets
   --features dev-tools`.
5. `cargo check --manifest-path pinvou-cli/Cargo.toml --workspace
   --all-targets --locked`: the CLI's Windows-only branches (exe/cmd
   candidates, `cmd /D /S /C` shims, taskkill tree kill, `CREATE_NO_WINDOW`)
   compile-check only on a Windows runner.
6. Link check: `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml
   --lib --no-run --message-format=json`, capturing the `pinvou3_lib` test
   executable as `PINVOU3_TEST_EXE`.
7. Embed the Common-Controls v6 manifest (resource `#1`) with the Windows SDK
   `mt.exe`: `muda` statically imports `TaskDialogIndirect`, which exists only
   in the Common-Controls v6 side-by-side assembly, and Windows ignores a
   side-by-side `<exe>.manifest` once `link.exe` embedded a default one.
8. Run `python scripts/ci-windows-imports-diagnose.py` on `PINVOU3_TEST_EXE`
   — a non-blocking PE import-table diagnostic (`continue-on-error`), after
   step 7 so the embedded manifest exempts SxS DLLs such as `comctl32`.
9. Run the CodeWhale PowerShell regression filters
   (`forkguard_powershell` and `forkguard_windows_shell_text`) from the
   dependency crate itself. The parent application jobs do not execute a
   dependency crate's lib tests. The step unsets `SHELL` so the Windows
   fallback to `pwsh.exe` is deterministic, and each filter must match at
   least one test so a rename cannot silently pass.
10. Regression loop: run the patched application binary directly — re-invoking
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

## Cache

`Swatinem/rust-cache@v2` stores `pinvou3-app/src-tauri` and `CodeWhale` in one
entry under shared key `windows-rust-test`. CodeWhale is a path dependency,
not an application-workspace member, so its required lib tests need their own
target directory; including that directory in the existing entry is the
minimum cache shape for this coverage, not a second cache. The first `main`
run after this change may compile CodeWhale cold and is expected to add about
0.8–2GB compressed. Restore-key fallback then reuses the entry across lockfile
changes, while `save-if` remains restricted to `refs/heads/main`, so pull
requests never rewrite it and usage stays within the repository-wide 10GB
budget. `pinvou-cli` has no separate cache. Node/npm setup is absent —
under the debug profile tauri's `generate_context!` uses `devUrl`, `dist/`
is never packaged, and `build.rs` only depends on `tauri-build`/`cc` —
saving 3-5 minutes per run.

## Duration, timeout, and failure diagnosis

Passing runs historically took 85-87 minutes; since 2026-09 several died at
the previous 90-minute cap during the link step, and run 34802015051 was
cancelled mid-build after PR #478 added the full `pinvou-cli` workspace
compile. `timeout-minutes` is now 180, with headroom for two cold workspaces.

On failure, read the import-diagnostic output (step 8) and the failing filter
name (steps 9–10); cache restore misses stay visible rollback signals. Do not
recover time by removing a regression filter, skipping the manifest or import
contract, changing failures to warnings, or adding another independent large
target cache beyond the single two-workspace entry documented above.

Source of truth: the `windows-rust-test` job in
`.github/workflows/pr-check.yml`; on any mismatch the workflow wins.
