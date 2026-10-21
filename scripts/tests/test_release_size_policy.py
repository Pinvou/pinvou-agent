import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release-packages.yml"
PR_CHECK_WORKFLOW = ROOT / ".github/workflows/pr-check.yml"
APP_CARGO_TOML = ROOT / "pinvou3-app/src-tauri/Cargo.toml"
KNOWLEDGE_CARGO_TOML = ROOT / "pinvou-knowledge/Cargo.toml"
REPACK_SCRIPT = ROOT / "scripts/repack-deb-xz.sh"
ULMO_SCRIPT = ROOT / "scripts/convert-dmg-ulmo.sh"
OTA_SCRIPT = ROOT / "scripts/build-windows-ota.ps1"
RELEASE_DEB_SCRIPT = ROOT / "scripts/release-deb.sh"
RELEASE_MACOS_SCRIPT = ROOT / "scripts/release-macos.sh"


def cargo_profile(text: str, profile_header: str, next_header: str) -> str:
    """Extract the text between two TOML table headers (empty if absent)."""
    if profile_header not in text:
        return ""
    return text.split(profile_header, maxsplit=1)[1].split(next_header, maxsplit=1)[0]


class ReleaseSizePolicyTests(unittest.TestCase):
    """Pins the release size / debug-info policy (docs/binary-size-optimization.md).

    Prevents later changes from quietly dropping these flags or reverting the
    profiles:
    - the amd64/arm64 release builds must carry lld + --icf=safe + remap-path-
      prefix; the loongarch64 build must NOT pick up lld flags (its pinned
      cross toolchain ships bfd only — copying them would fail every link)
      and must not override LTO;
    - the deb must be repacked as xz -9, with repacking before sha256
      (amd64/arm64 in the workflow, loongarch64 inside its orchestrator
      script);
    - the macOS dmg must get the ULMO compression upgrade;
    - the macOS 27+ strip=none workaround (obsolete since rustc 1.98.0) must
      not come back;
    - the release profile must ship zero debug info (strip=symbols + debug=false).
    """

    def setUp(self):
        self.release_workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        self.pr_check_workflow = PR_CHECK_WORKFLOW.read_text(encoding="utf-8")
        self.app_cargo = APP_CARGO_TOML.read_text(encoding="utf-8")
        self.knowledge_cargo = KNOWLEDGE_CARGO_TOML.read_text(encoding="utf-8")
        self.repack = REPACK_SCRIPT.read_text(encoding="utf-8")
        self.ulmo = ULMO_SCRIPT.read_text(encoding="utf-8")
        self.ota = OTA_SCRIPT.read_text(encoding="utf-8")
        self.release_deb = RELEASE_DEB_SCRIPT.read_text(encoding="utf-8")
        self.release_macos = RELEASE_MACOS_SCRIPT.read_text(encoding="utf-8")

    def job(self, start_marker: str, end_marker: str) -> str:
        return self.release_workflow.split(start_marker, maxsplit=1)[1].split(
            end_marker, maxsplit=1
        )[0]

    def test_linux_jobs_keep_icf_and_remap_rustflags(self):
        for start, end in (
            ("\n  build-linux-x64:", "\n  build-linux-arm64:"),
            ("\n  build-linux-arm64:", "\n  build-linux-loongarch64:"),
        ):
            job_env = self.job(start, end).split("\n    steps:", maxsplit=1)[0]
            # lld: thin LTO is executed by rustc itself, but BFD link OOMs on
            # binaries this size and BFD has no ICF (see the job env comment).
            self.assertIn("-C link-arg=-fuse-ld=lld", job_env, start)
            # ICF identical code folding (safe tier: only folds functions
            # whose addresses are provably never taken).
            self.assertIn("-C link-arg=-Wl,--icf=safe", job_env, start)
            # Normalizes build-machine absolute paths embedded in the
            # artifacts (panic locations, etc.). Top-level rustc flag, not a
            # -C codegen option — rustc rejects `-C remap-path-prefix`.
            self.assertIn(
                "--remap-path-prefix=${{ github.workspace }}=/", job_env, start
            )
            # The -C spelling would fail every rustc invocation of the job
            # (checked on the RUSTFLAGS lines only; comments may discuss it).
            for line in job_env.splitlines():
                if line.strip().startswith("RUSTFLAGS:"):
                    self.assertNotIn("-C remap-path-prefix", line, start)

    def test_windows_probe_exports_remap_path_prefix(self):
        # PR #745: the Windows lld probe exports --remap-path-prefix beside
        # the probed linker flags so the nsis artifact normalizes
        # build-machine absolute paths like every other release job. Both
        # separator variants ship because the embedded form follows
        # whatever cargo passed to rustc. Dropping the flags line reverts
        # to path-leaking artifacts with no build error, hence the pin.
        windows = self.job("\n  build-windows-x64:", "\n  build-macos-universal:")
        self.assertIn('ws="${GITHUB_WORKSPACE}"', windows)
        self.assertIn('ws_fwd="${ws//\\\\//}"', windows)
        self.assertIn(
            'flags="$flags --remap-path-prefix=$ws=/ --remap-path-prefix=$ws_fwd=/"',
            windows,
        )
        self.assertNotIn("-C remap-path-prefix", windows)

    def test_loongarch64_job_must_not_copy_lld_rustflags(self):
        # The pinned loong64/cross-tools asset ships NO lld (bfd ld only —
        # see scripts/ci-loongarch64-toolchain.sh). Copying the arm64 job's
        # -fuse-ld=lld (or the lld-only --icf=safe) would fail every link;
        # RUSTFLAGS carries only the rustc-side --remap-path-prefix and LTO
        # stays in Cargo.toml (thin LTO runs inside rustc and is
        # linker-independent).
        job = self.job("\n  build-linux-loongarch64:", "\n  build-windows-x64:")
        # Only live configuration counts: the job's comments deliberately name
        # the forbidden flags to warn future editors (same rule as the
        # cache-policy suite's comment-stripped pins).
        job_code = "\n".join(
            line for line in job.splitlines() if not line.lstrip().startswith("#")
        )
        self.assertNotIn("-fuse-ld=lld", job_code)
        self.assertNotIn("--icf=safe", job_code)
        self.assertNotIn("CARGO_PROFILE_RELEASE_LTO", job_code)
        self.assertNotIn("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", job_code)
        # RUSTFLAGS must stay remap-only in this job — in EVERY spelling a
        # cargo/rustc invocation can see: a `RUSTFLAGS:` job/step env key AND
        # a shell-style `export RUSTFLAGS=` / `RUSTFLAGS=` line inside any
        # run block. --remap-path-prefix is rustc-side and keeps
        # build-machine paths out of the artifact; linker selection is NOT a
        # RUSTFLAGS concern (cargo routes through the generated rust-lld
        # wrapper — pinned by test_release_loongarch64_policy), so any
        # -C link-arg here would either fail (the pinned toolchain ships
        # bfd only) or bypass the audited wrapper. The three spellings are
        # collected together so a run-block export or a target-scoped
        # CARGO_TARGET_*_RUSTFLAGS env key cannot slip past the job-env-only
        # equality check.
        # Containment, not line-anchored prefixes: an INLINE prefix
        # (`run: RUSTFLAGS=... bash ...`) starts with neither spelling but
        # is fully visible to cargo — every line mentioning RUSTFLAGS must
        # BE the pinned definition.
        rustflags = [
            line.strip()
            for line in job_code.splitlines()
            if "RUSTFLAGS" in line
        ]
        self.assertEqual(
            rustflags,
            ['RUSTFLAGS: "--remap-path-prefix=${{ github.workspace }}=/"'],
            "the loong64 job must keep RUSTFLAGS remap-only (job/step env, "
            "run-block exports, inline prefixes, and CARGO_TARGET_*_RUSTFLAGS "
            "keys alike)",
        )

    def test_linux_jobs_repack_deb_as_xz_before_sha256(self):
        for start, end in (
            ("\n  build-linux-x64:", "\n  build-linux-arm64:"),
            ("\n  build-linux-arm64:", "\n  build-linux-loongarch64:"),
        ):
            job_text = self.job(start, end)
            repack_call = "scripts/repack-deb-xz.sh"
            self.assertIn(repack_call, job_text, start)
            # Repacking must happen before sha256, otherwise the checksum
            # covers the old package.
            self.assertLess(
                job_text.index(repack_call),
                job_text.index("sha256sum"),
                start,
            )
            # The glibc floor guard still runs on the final repacked artifact
            # (dpkg-deb -x reads xz natively).
            self.assertIn("check-linux-glibc-floor.sh", job_text, start)

    def test_loongarch64_repack_and_floor_guard_live_in_the_orchestrator(self):
        # The loong64 job delegates the whole pipeline to
        # scripts/ci-build-linux-loongarch64.sh; the repack (gzip-6 → xz -9,
        # same as the amd64/arm64 jobs) and the glibc floor guard must run
        # there BEFORE the artifact sha256 is computed, and the workflow job
        # must re-run the floor guard on the renamed artifact before upload
        # (same 重命名→守护→上传 sequence as build-linux-arm64).
        orchestrator = (ROOT / "scripts" / "ci-build-linux-loongarch64.sh").read_text(
            encoding="utf-8"
        )
        code_only = "\n".join(
            line for line in orchestrator.splitlines() if not line.lstrip().startswith("#")
        )
        # The orchestrator consumes the shared helpers through $SCRIPT_DIR,
        # so the pins match on the script file names.
        self.assertIn("repack-deb-xz.sh", code_only)
        self.assertIn("check-linux-glibc-floor.sh", code_only)
        self.assertIn("loongarch64", code_only)
        # Inside the orchestrator the order is repack → floor guard → sha256,
        # so the checksum covers the final repacked, guarded bytes.
        self.assertLess(
            code_only.index("repack-deb-xz.sh"),
            code_only.index("sha256sum"),
            "the orchestrator must repack before hashing the deb",
        )
        self.assertLess(
            code_only.index("check-linux-glibc-floor.sh"),
            code_only.index("sha256sum"),
            "the orchestrator's floor guard must run before the sha256 footer",
        )
        # The workflow job mirrors build-linux-arm64's 重命名→守护→上传
        # sequence: the rename step hashes the final bytes, the guard step
        # (before upload) judges exactly those bytes.
        job = self.job("\n  build-linux-loongarch64:", "\n  build-windows-x64:")
        self.assertIn("scripts/ci-build-linux-loongarch64.sh", job)
        self.assertIn("check-linux-glibc-floor.sh", job)
        self.assertLess(
            job.index("check-linux-glibc-floor.sh"),
            job.index("uses: actions/upload-artifact@"),
            "the job's floor guard must run before upload",
        )

    def test_repack_script_uses_max_compression(self):
        # Only executable lines count (counterexamples inside comments don't).
        code_only = "\n".join(
            line
            for line in self.repack.splitlines()
            if not line.lstrip().startswith("#")
        )
        # --root-owner-group: non-root build machines still produce root:root.
        self.assertIn("--root-owner-group", code_only)
        # data.tar xz -9 (control.tar follows via dpkg uniform compression,
        # the default since dpkg 1.19).
        self.assertIn("-Zxz", code_only)
        self.assertIn("-z9", code_only)

    def test_macos_job_upgrades_dmg_to_ulmo(self):
        macos_job = self.job(
            "\n  build-macos-universal:", "\n      - name: 上传 dmg artifact"
        )
        # The conversion lives in the shared helper (same file for the
        # workflow and the manual release script, so the paths cannot drift).
        self.assertIn("scripts/convert-dmg-ulmo.sh", macos_job)

    def test_ulmo_helper_converts_and_degrades(self):
        # Only executable lines count (counterexamples inside comments don't).
        code_only = "\n".join(
            line
            for line in self.ulmo.splitlines()
            if not line.lstrip().startswith("#")
        )
        # Format detection: artifacts already in ULMO are not re-converted
        # (also covers the output of the degradation path).
        self.assertIn("hdiutil imageinfo -format", code_only)
        self.assertIn("-format ULMO", code_only)
        # Degradation: a failed conversion keeps the original dmg instead of
        # failing the release packaging.
        self.assertIn("conversion failed, keeping the original", code_only)

    def test_macos_strip_workaround_stays_removed(self):
        # rustc 1.98.0 contains the Mach-O __LINKEDIT alignment fix
        # (rust#157750/#158410); a strip=none injection would keep symbol
        # tables / debug maps in macOS artifacts and must not come back.
        self.assertNotIn("CARGO_PROFILE_RELEASE_STRIP", self.release_workflow)
        self.assertNotIn("CARGO_PROFILE_RELEASE_FAST_STRIP", self.pr_check_workflow)
        self.assertNotIn("-C strip=none", self.release_workflow)
        self.assertNotIn("-C strip=none", self.pr_check_workflow)
        # Path normalization flag of the macOS release job (same reason as
        # Linux; top-level rustc flag, not a -C codegen option).
        macos_env = self.job(
            "\n  build-macos-universal:", "\n    steps:"
        ).split("\n    steps:", maxsplit=1)[0]
        self.assertIn(
            'RUSTFLAGS: "--remap-path-prefix=${{ github.workspace }}=/"',
            macos_env,
        )
        # The -C spelling would fail every rustc invocation of the job
        # (checked on the RUSTFLAGS lines only; comments may discuss it).
        for line in macos_env.splitlines():
            if line.strip().startswith("RUSTFLAGS:"):
                self.assertNotIn("-C remap-path-prefix", line)

    def test_app_release_profile_ships_no_debug_info(self):
        release = cargo_profile(
            self.app_cargo, "[profile.release]", "[profile.release-fast]"
        )
        # strip=symbols: link-time --strip-all on ELF (no-op on MSVC, where
        # the PDB never enters installers).
        self.assertIn('strip = "symbols"', release)
        # debug=false: artifacts carry zero debug info; line tables are kept
        # only in the release-fast smoke profile.
        self.assertIn("debug = false", release)
        release_fast = cargo_profile(
            self.app_cargo, "[profile.release-fast]", "[profile.dev]"
        )
        self.assertIn('debug = "line-tables-only"', release_fast)
        # strip must be relaxed back to none: the inherited strip=symbols
        # strips debug info as a superset, so the line tables would never
        # actually reach the smoke binaries.
        self.assertIn('strip = "none"', release_fast)
        # The panic policy is pinned by the CodeWhale catch_unwind design.
        self.assertIn('panic = "unwind"', release)

    def test_knowledge_server_release_profile_is_size_optimized(self):
        # pinvou-knowledge-server is one of the three ELFs in the deb, built
        # standalone (knowledge-host.js); it must match the main app's
        # thin LTO + strip policy.
        profile = self.knowledge_cargo.split("[profile.release]", maxsplit=1)[1]
        self.assertIn('lto = "thin"', profile)
        self.assertIn("codegen-units = 1", profile)
        self.assertIn('strip = "symbols"', profile)

    def test_windows_ota_uses_smallest_size_compression(self):
        self.assertNotIn("CompressionLevel]::Optimal", self.ota)
        self.assertIn("CompressionLevel]::SmallestSize", self.ota)
        # SmallestSize was introduced in .NET 6, so it is absent on Windows
        # PowerShell 5.1 (.NET Framework) and on PowerShell 6.x/7.0/7.1
        # (.NET Core / .NET 5); the first PowerShell built on .NET 6 is 7.2.
        # The script must declare that requirement so it fails loudly up
        # front instead of dying mid-run at the first CreateEntry.
        self.assertIn("#requires -Version 7.2", self.ota)

    def test_manual_release_scripts_share_the_same_pipeline(self):
        self.assertIn("scripts/repack-deb-xz.sh", self.release_deb)
        self.assertIn("scripts/convert-dmg-ulmo.sh", self.release_macos)


if __name__ == "__main__":
    unittest.main()
