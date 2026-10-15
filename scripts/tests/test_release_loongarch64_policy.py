"""Pins the LoongArch64 (dpkg arch: loong64) release/CI contract.

GitHub hosts no loong64 runner, so the release build runs cross on x86_64
ubuntu-22.04 and the pr-check cross leg type-checks the target per PR:
- the release job must invoke the orchestrator (scripts/ci-build-linux-
  loongarch64.sh) on an x86_64 runner, must NOT copy the arm64 lld RUSTFLAGS
  (the pinned cross toolchain ships bfd only) and must not override LTO;
- bundling must be skipped via the build.js `--bundles none` path (tauri-
  bundler has no LoongArch variant and panics); the deb is hand-assembled;
- the toolchain/sysroot must come from the sha256-pinned setup scripts, with
  cache keys in the release-linux-loong64-jammy shape (restore-only in the
  release job, main-only saves in the pr-check leg);
- the artifact/deb naming and the glibc 2.38 floor guard must match the
  sibling Linux jobs;
- the pr-check leg must cargo-check all three workspaces for the target and
  be wired into required-gate like every other leg.

Same YAML-splitting idiom as test_release_linux_floor_policy /
test_release_size_policy: workflow sections are split by exact job names, so
moving or renaming a job changes the split points and fails loudly here.
"""

import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release-packages.yml"
PR_CHECK_WORKFLOW = ROOT / ".github/workflows/pr-check.yml"
ORCHESTRATOR = ROOT / "scripts" / "ci-build-linux-loongarch64.sh"
TOOLCHAIN_SCRIPT = ROOT / "scripts" / "ci-loongarch64-toolchain.sh"
SYSROOT_SCRIPT = ROOT / "scripts" / "ci-loongarch64-sysroot.sh"
PACKAGING_SCRIPT = ROOT / "scripts" / "package-deb-loongarch64.sh"
BUILD_JS = ROOT / "pinvou3-app" / "scripts" / "tauri" / "build.js"

# scripts/ci-loongarch64-toolchain.sh: the x86_64-host asset pin (tag
# 20260912, variant stable, gcc 14.4, glibc 2.38). The workflow must consume
# the pinned script, never an unpinned ad-hoc download.
TOOLCHAIN_SHA256 = "cf68c8c59249fa6707d88b0380f8eb2e3598749958deac4d2149e149b0ccc51b"

TOOLCHAIN_CACHE_KEY = "release-linux-loong64-jammy-toolchain-"
SYSROOT_CACHE_KEY = "release-linux-loong64-jammy-sysroot-"


def _without_comments(text):
    """A commented-out step or flag must not satisfy a live-configuration pin."""
    return "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith("#")
    )


class ReleaseLoongArch64PolicyTests(unittest.TestCase):
    def setUp(self):
        self.release_workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        self.pr_check_workflow = PR_CHECK_WORKFLOW.read_text(encoding="utf-8")
        self.orchestrator = ORCHESTRATOR.read_text(encoding="utf-8")
        self.toolchain = TOOLCHAIN_SCRIPT.read_text(encoding="utf-8")
        self.sysroot = SYSROOT_SCRIPT.read_text(encoding="utf-8")
        self.build_js = BUILD_JS.read_text(encoding="utf-8")
        self.job = self.release_workflow.split(
            "\n  build-linux-loongarch64:", maxsplit=1
        )[1].split("\n  build-windows-x64:", maxsplit=1)[0]
        self.job_code = _without_comments(self.job)
        self.pr_job = _without_comments(self.pr_check_workflow).split(
            "\n  rust-loongarch64-check:", maxsplit=1
        )[1].split("\n  required-gate:", maxsplit=1)[0]

    # ---- release job: runner + orchestrator ----

    def test_release_job_cross_builds_on_x86_64(self):
        # GitHub has no loong64 hosted runner: the cross build must be pinned
        # to the x86_64 jammy image the toolchain asset's sha256 pin was
        # recorded for (never the arm base image, never latest).
        self.assertIn("runs-on: ubuntu-22.04\n", self.job)
        self.assertNotIn("runs-on: ubuntu-22.04-arm", self.job)
        self.assertNotIn("ubuntu-latest", self.job.split("\n    steps:", maxsplit=1)[0])

    def test_release_job_invokes_the_ci_orchestrator(self):
        # Every stage (knowledge cross build, ASR cross build, tauri app
        # build, hand-assembled deb, repack, floor guard) is owned by the
        # orchestrator; the job must not grow a private copy of the pipeline.
        self.assertIn("bash scripts/ci-build-linux-loongarch64.sh", self.job_code)
        self.assertIn("scripts/ci-loongarch64-toolchain.sh", self.job_code)
        self.assertIn("scripts/ci-loongarch64-sysroot.sh", self.job_code)
        # The job consumes the setup scripts' env contract instead of running
        # build.js (or any bundling) itself.
        self.assertNotIn("node scripts/tauri/build.js", self.job_code)

    def test_release_job_does_not_use_lld_or_override_lto(self):
        # The pinned cross toolchain ships NO lld (bfd only): the arm64 job's
        # -fuse-ld=lld / --icf=safe must never be copied, RUSTFLAGS stays
        # unset, and LTO stays owned by Cargo.toml. Comment mentions of the
        # forbidden flags are deliberate (they warn future editors) and are
        # stripped here — only live configuration counts.
        self.assertNotIn("-fuse-ld=lld", self.job_code)
        self.assertNotIn("--icf=safe", self.job_code)
        self.assertNotIn("CARGO_PROFILE_RELEASE_LTO", self.job_code)
        self.assertNotIn("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", self.job_code)
        job_env = self.job_code.split("\n    steps:", maxsplit=1)[0]
        for line in job_env.splitlines():
            self.assertFalse(
                line.strip().startswith("RUSTFLAGS:"),
                f"the loong64 job must leave RUSTFLAGS unset, found: {line.strip()}",
            )

    def test_release_job_gives_the_cross_build_headroom(self):
        # arm64 (native) gets 90 minutes; the cross build reuses the same
        # runner shape with the cross toolchain in front of it — the budget
        # is 90 * ~1.5. Pin it so the headroom cannot silently disappear.
        self.assertIn("timeout-minutes: 135", self.job)

    # ---- bundling contract ----

    def test_bundling_is_skipped_via_the_bundles_none_path(self):
        # tauri-bundler panics on the loongarch64 triple (binary_arch() has
        # no LoongArch variant), so the orchestrator's tauri invocation must
        # go through build.js's `--bundles none` (translated to the CLI's
        # --no-bundle) and the deb must be hand-assembled.
        self.assertIn(
            'node scripts/tauri/build.js build --target "$TRIPLE" --bundles none',
            self.orchestrator,
        )
        self.assertIn("package-deb-loongarch64.sh", self.orchestrator)
        # The build.js side of the contract: the value is wrapper-owned and
        # translated into the flag the pinned CLI actually accepts.
        self.assertIn("function extractBundlesNone", self.build_js)
        self.assertIn('"--no-bundle"', self.build_js)

    # ---- artifact naming + guards ----

    def test_artifact_and_deb_use_the_unified_loong64_name(self):
        version_ref = "${{ needs.check-version-bump.outputs.version }}"
        artifact_name = f"pinvou-agent-{version_ref}-linux-loong64"
        deb_name = f"pinvou-agent_{version_ref}-linux-loong64.deb"
        self.assertIn(f"name: {artifact_name}\n", self.job)
        # Both upload paths (deb + .sha256) carry the unified name.
        self.assertEqual(self.job_code.count(deb_name), 2)

    def test_release_job_runs_the_loongarch64_floor_guard(self):
        step = self._step_block(self.job, "check-linux-glibc-floor.sh")
        self.assertRegex(step, r'check-linux-glibc-floor\.sh "[^"]+\.deb" loongarch64\s*$')

    def _step_block(self, job, needle):
        # Same step extractor as test_release_linux_floor_policy: the block
        # spans from the step's "- " line to the next step, so an end-anchored
        # regex judges the guard's own argument list.
        at = job.find(needle)
        self.assertGreater(at, 0, f"missing step containing {needle!r}")
        start = job.rfind("\n      - ", 0, at)
        self.assertGreater(start, 0, f"{needle!r} must live inside a step")
        end = job.find("\n      - ", at)
        return job[start:] if end == -1 else job[start:end]

    def test_orchestrator_runs_repack_and_floor_guard_internally(self):
        code_only = _without_comments(self.orchestrator)
        self.assertIn("repack-deb-xz.sh", code_only)
        self.assertIn("check-linux-glibc-floor.sh", code_only)
        self.assertLess(
            code_only.index("repack-deb-xz.sh"),
            code_only.index("sha256sum"),
        )

    # ---- pinned toolchain / sysroot ----

    def test_toolchain_setup_script_pins_the_asset_hash(self):
        # The release job consumes the sha256-pinned script; the pin must
        # stay in the script (a workflow-side env override could weaken it).
        self.assertIn(TOOLCHAIN_SHA256, self.toolchain)
        self.assertIn('GLIBC_VERSION="2.38"', self.toolchain)
        self.assertIn('VARIANT="stable"', self.toolchain)

    def test_sysroot_setup_script_pins_the_glibc_2_38_baseline(self):
        self.assertIn('GLIBC_FLOOR="2.38"', self.sysroot)
        self.assertIn('DPKG_ARCH="loong64"', self.sysroot)
        # The deepin beige source is the audited glibc-2.38 baseline.
        self.assertIn("community-packages.deepin.com/beige", self.sysroot)

    def test_setup_scripts_print_the_export_block_the_pr_leg_consumes(self):
        # The pr-check leg extracts the "export in the CI step" block into
        # GITHUB_ENV (identifier-safe lines only). If a required variable
        # vanished from a block — or was only printed in the dash-spelled
        # verbatim-triple form that the filter drops — the cross check would
        # silently compile against the wrong environment. Pin the exact
        # underscore-spelled variables each consumer needs.
        for source, marker in (
            (self.toolchain, "---- ci-loongarch64-toolchain: export in the CI step ----"),
            (self.sysroot, "---- ci-loongarch64-sysroot: export in the CI step ----"),
        ):
            self.assertIn(marker, source)
            block = source.split(marker, maxsplit=1)[1].split("---- notes ----", maxsplit=1)[0]
            for line in block.splitlines():
                line = line.strip()
                if line and not line.startswith("#"):
                    # The block must stay pure env assignments (the pr-check
                    # leg greps it into GITHUB_ENV); dash-spelled verbatim-
                    # triple forms are allowed alongside the underscore ones.
                    self.assertRegex(line, r"^[A-Za-z_][A-Za-z0-9_-]*=")
        toolchain_block = self.toolchain.split(
            "---- ci-loongarch64-toolchain: export in the CI step ----", maxsplit=1
        )[1].split("---- notes ----", maxsplit=1)[0]
        for required in (
            "PINVOU_LOONG64_TOOLCHAIN_PREFIX=",
            "CC_loongarch64_unknown_linux_gnu=",
            "CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_LINKER=",
        ):
            self.assertIn(required, toolchain_block)
        sysroot_block = self.sysroot.split(
            "---- ci-loongarch64-sysroot: export in the CI step ----", maxsplit=1
        )[1].split("---- notes ----", maxsplit=1)[0]
        for required in (
            "PINVOU_LOONG64_SYSROOT=",
            "PKG_CONFIG_LIBDIR=",
            "PKG_CONFIG_SYSROOT_DIR=",
            "CMAKE_TOOLCHAIN_FILE=",
        ):
            self.assertIn(required, sysroot_block)

    # ---- cache keys (release restore-only, pr-check main-only saves) ----

    def test_release_job_restores_loong64_caches_read_only(self):
        self.assertIn(TOOLCHAIN_CACHE_KEY, self.job_code)
        self.assertIn(SYSROOT_CACHE_KEY, self.job_code)
        # Release runs never write caches (same quota policy as the
        # save-if: false rust-caches of the x64/arm64 jobs).
        self.assertNotIn("actions/cache/save@", self.job_code)

    def test_pr_check_leg_saves_the_same_caches_on_main_only(self):
        pr_jobs = _without_comments(self.pr_check_workflow)
        pr_job = pr_jobs.split("\n  rust-loongarch64-check:", maxsplit=1)[1].split(
            "\n  required-gate:", maxsplit=1
        )[0]
        for key, script in (
            (TOOLCHAIN_CACHE_KEY, "scripts/ci-loongarch64-toolchain.sh"),
            (SYSROOT_CACHE_KEY, "scripts/ci-loongarch64-sysroot.sh"),
        ):
            # The exact restore and save keys must stay identical (a drift
            # makes the release job restore a cache this job never wrote).
            full_key = "key: " + key + "${{ hashFiles('" + script + "') }}"
            self.assertEqual(pr_job.count(full_key), 2, key)
            # The prefix fallback keeps partial restores alive between
            # script changes.
            self.assertIn(f"restore-keys: |\n            {key}", pr_job, key)
        # ... and every save step is gated to main (main writes, PRs and the
        # release job read — the repo's rust-cache save-if convention).
        for step in pr_job.split("\n      - name:"):
            if "actions/cache/save@" in step:
                self.assertIn(
                    "github.ref == 'refs/heads/main'",
                    step,
                    "a loong64 cache save must be main-only",
                )

    # ---- pr-check cross leg ----

    def test_pr_check_leg_checks_all_three_workspaces(self):
        self.assertIn("runs-on: ubuntu-22.04\n", self.pr_job)
        for workspace in ("pinvou3-app/src-tauri", "pinvou-knowledge"):
            self.assertIn(
                f"--manifest-path {workspace}/Cargo.toml --target "
                "loongarch64-unknown-linux-gnu --locked",
                self.pr_job,
                f"the cross check must cover {workspace}",
            )
        # pinvou-cli is a virtual workspace: the check goes through
        # --workspace like the macos-cli-check compile leg.
        self.assertIn(
            "--manifest-path pinvou-cli/Cargo.toml --workspace --target "
            "loongarch64-unknown-linux-gnu --locked",
            self.pr_job,
        )

    def test_pr_check_leg_is_wired_into_required_gate(self):
        gate = _without_comments(self.pr_check_workflow).split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn(
            "- rust-loongarch64-check",
            gate,
            "the leg must stay in required-gate's needs list",
        )
        self.assertIn(
            "RUST_LOONGARCH64_CHECK_RESULT: ${{ needs.rust-loongarch64-check.result }}",
            gate,
            "the leg's result must be backfilled into required-gate's summary env",
        )
        self.assertIn(
            '"rust-loongarch64-check:$RUST_LOONGARCH64_CHECK_RESULT"',
            gate,
            "the leg must enter the failure loop (success|skipped accepted)",
        )

    # ---- supply-chain: the consumed scripts stay valid bash ----

    def test_loongarch64_ci_scripts_are_valid_bash(self):
        for script in (ORCHESTRATOR, TOOLCHAIN_SCRIPT, SYSROOT_SCRIPT, PACKAGING_SCRIPT):
            with self.subTest(script=script.name):
                subprocess.run(["bash", "-n", str(script)], check=True)


if __name__ == "__main__":
    unittest.main()
