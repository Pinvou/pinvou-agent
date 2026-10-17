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
  release job, main-only saves in the pr-check leg, saves gated on setup
  success);
- both jobs must provision a LoongArch-capable libclang (>= 16; jammy's
  archive tops out at a target-less 15) via the pinned apt.llvm.org llvm-18
  suite and LIBCLANG_PATH, with a byte-identical shared run block;
- the artifact/deb naming, the deb Depends parity with tauri.conf.json, and
  the glibc 2.38 floor guard must match the sibling Linux jobs;
- the pr-check leg must cargo-check all three workspaces for the target and
  be wired into required-gate like every other leg.

Same YAML-splitting idiom as test_release_linux_floor_policy /
test_release_size_policy: workflow sections are split by exact job names, so
moving or renaming a job changes the split points and fails loudly here.
"""

import re
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
        self.packager = PACKAGING_SCRIPT.read_text(encoding="utf-8")
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
        # recorded for (never the arm base image, never latest). Judged on
        # comment-stripped job text, like every live-configuration pin.
        self.assertIn("runs-on: ubuntu-22.04\n", self.job_code)
        self.assertNotIn("runs-on: ubuntu-22.04-arm", self.job_code)
        self.assertNotIn("ubuntu-latest", self.job_code.split("\n    steps:", maxsplit=1)[0])

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

    def test_release_job_sets_remap_only_rustflags(self):
        # The pinned cross toolchain ships NO lld (bfd only): the arm64 job's
        # -fuse-ld=lld / --icf=safe must never be copied, and LTO stays owned
        # by Cargo.toml. RUSTFLAGS must be exactly the remap-only spelling:
        # --remap-path-prefix is rustc-side (the macOS job sets it alone),
        # keeps build-machine paths out of the artifact like every other
        # release job, and needs no lld. Comment mentions of the forbidden
        # flags are deliberate (they warn future editors) and are stripped
        # here — only live configuration counts.
        self.assertNotIn("-fuse-ld=lld", self.job_code)
        self.assertNotIn("--icf=safe", self.job_code)
        self.assertNotIn("CARGO_PROFILE_RELEASE_LTO", self.job_code)
        self.assertNotIn("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", self.job_code)
        job_env = self.job_code.split("\n    steps:", maxsplit=1)[0]
        rustflags = [
            line.strip()
            for line in job_env.splitlines()
            if line.strip().startswith("RUSTFLAGS:")
        ]
        self.assertEqual(
            rustflags,
            ['RUSTFLAGS: "--remap-path-prefix=${{ github.workspace }}=/"'],
            "the loong64 job must set exactly the remap-only RUSTFLAGS",
        )
        # The orchestrator owns the actual cargo invocations and must not
        # override the job's flags (its comments deliberately warn; strip
        # them before scanning).
        orchestrator_code = _without_comments(self.orchestrator)
        self.assertNotIn("-fuse-ld=lld", orchestrator_code)
        self.assertNotIn("--icf=safe", orchestrator_code)
        self.assertNotIn("CARGO_PROFILE_RELEASE_LTO", orchestrator_code)
        for line in orchestrator_code.splitlines():
            stripped = line.strip()
            self.assertFalse(
                stripped.startswith("RUSTFLAGS=") or stripped.startswith("export RUSTFLAGS"),
                f"the orchestrator must leave RUSTFLAGS unset, found: {stripped}",
            )

    def test_release_job_gives_the_cross_build_headroom(self):
        # arm64 (native) gets 90 minutes; the cross build reuses the same
        # runner shape with the cross toolchain in front of it — the budget
        # is 90 * ~1.5. Pin it so the headroom cannot silently disappear.
        self.assertIn("timeout-minutes: 135", self.job_code)

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
        self.assertIn(f"name: {artifact_name}\n", self.job_code)
        # Both upload paths (deb + .sha256) carry the unified name.
        self.assertEqual(self.job_code.count(deb_name), 2)

    def test_release_job_runs_the_loongarch64_floor_guard(self):
        step = self._step_block(self.job_code, "check-linux-glibc-floor.sh")
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
        # Judged on comment-stripped text: the pin must live in code.
        toolchain = _without_comments(self.toolchain)
        self.assertIn(TOOLCHAIN_SHA256, toolchain)
        self.assertIn('GLIBC_VERSION="2.38"', toolchain)
        self.assertIn('VARIANT="stable"', toolchain)

    def test_sysroot_setup_script_pins_the_glibc_2_38_baseline(self):
        sysroot = _without_comments(self.sysroot)
        self.assertIn('GLIBC_FLOOR="2.38"', sysroot)
        self.assertIn('DPKG_ARCH="loong64"', sysroot)
        # The deepin beige source is the audited glibc-2.38 baseline.
        self.assertIn("community-packages.deepin.com/beige", sysroot)

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
            # --target must ride along explicitly: bindgen 0.72's
            # clang_macro_fallback PCH never sees the --target it injects
            # for cross builds, so without this the fallback evaluates the
            # sysroot's glibc headers as the host arch and #define-only
            # constants like spa's SPA_ID_INVALID drop out of the bindings
            # (libspa E0425).
            "BINDGEN_EXTRA_CLANG_ARGS=--target=$TRIPLE --sysroot=",
        ):
            self.assertIn(required, sysroot_block)

    def test_cache_reuse_never_skips_generation_checks(self):
        # The cache-generation trust behaviors are the difference between a
        # stale or truncated cache and a broken build — deleting any of them
        # must fail this suite, not a release link stage:
        # (1) the toolchain marker is only trusted when it names the CURRENT
        #     pin (tag/variant/host/asset-hash) AND the generating script's
        #     own content hash, so a restore-keys fallback to an older
        #     generation after a script edit rebuilds instead of reusing;
        # (2) sysroot reuse re-runs the full verification, so a truncated
        #     restore fails at setup time, not at link time;
        # (3) rebuilds start from a fresh tree (dpkg-deb -x never deletes,
        #     so extracting over a previous generation would mix sources);
        # (4) the manifest is written atomically (tmp + rename), so a reader
        #     can never observe a half-written generation marker;
        # (5) the sysroot manifest records the generating script's content
        #     hash and reuse compares it — same restore-keys defense as (1).
        toolchain = _without_comments(self.toolchain)
        sysroot = _without_comments(self.sysroot)
        self.assertIn(
            'expected_id="${CROSS_TOOLS_TAG}/${VARIANT}/${host_arch}/${pinned_sha256}/${self_sha256}"',
            toolchain,
        )
        self.assertIn('self_sha256="$(sha256sum "$self_path"', toolchain)
        self.assertIn('[ "$(cat "$marker")" != "$expected_id" ]', toolchain)
        self.assertIn('self_sha256="$(sha256sum "$self_path"', sysroot)
        # Both the manifest write and the reuse-side grep must carry the
        # script hash: removing either half silently re-enables stale reuse.
        self.assertEqual(
            sysroot.count('script-sha256=$self_sha256'),
            2,
            "the script hash must be recorded AND compared on reuse",
        )
        self.assertEqual(
            sysroot.count('verify_sysroot "$output_dir"'),
            2,
            "sysroot verification must run on BOTH the reuse and the rebuild path",
        )
        self.assertIn('rm -rf -- "$output_dir"', sysroot)
        self.assertIn('> "$manifest.tmp"', sysroot)
        self.assertIn('mv "$manifest.tmp" "$manifest"', sysroot)

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
        # release job read — the repo's rust-cache save-if convention) AND to
        # setup success: both setup scripts mkdir their target dir before
        # downloading, so a failed setup saves a half-empty tree that would
        # otherwise be restored forever under that exact key.
        for step in pr_job.split("\n      - name:"):
            if "actions/cache/save@" in step:
                self.assertIn(
                    "github.ref == 'refs/heads/main'",
                    step,
                    "a loong64 cache save must be main-only",
                )
                self.assertIn(
                    "success()",
                    step,
                    "a loong64 cache save must be gated on setup success",
                )
                self.assertNotIn(
                    "!cancelled()",
                    step,
                    "!cancelled() saves after failed setups and poisons the cache generation",
                )

    # ---- pr-check cross leg ----

    def test_loong64_jobs_provision_a_loongarch_capable_libclang(self):
        # bindgen feeds `--target=loongarch64-...` to libclang on cross
        # builds (bindgen injects the target only for cross compilation, so
        # this pipeline is the repo's first cross-bindgen consumer).
        # LoongArch is a primary LLVM target only from 16 on and the jammy
        # archive ships no libclang >= 16 (14/15 lack the target), so a
        # plain `libclang-dev` makes bindgen panic with "unknown target
        # triple". Both jobs must pin the apt.llvm.org llvm-18 suite and
        # steer clang-sys via LIBCLANG_PATH.
        for label, host in (("pr-check leg", self.pr_job), ("release job", self.job_code)):
            with self.subTest(host=label):
                self.assertIn("apt.llvm.org/jammy/", host, label)
                self.assertIn("llvm-toolchain-jammy-18", host, label)
                self.assertIn("libclang-18-dev", host, label)
                self.assertIn("LIBCLANG_PATH=/usr/lib/llvm-18/lib", host, label)
        # The provisioning run block is a shared contract duplicated across
        # the two workflows: it must stay byte-identical (comments stripped).
        # Scope to the loong64 jobs — sibling jobs carry same-named steps.
        pr_block = self._step_run_block(self.pr_job, "Install host build tools")
        rel_block = self._step_run_block(self.job, "装 Tauri + pinvou3 Linux 系统依赖")
        self.assertEqual(pr_block, rel_block)
        # The orchestrator (release path) exports the same pins for the
        # cargo builds it drives: sysroot-scoped bindgen args — with the
        # explicit cross target, same rationale as the sysroot export
        # block above — and the llvm-18 libclang discovery (overridable on
        # dev hosts).
        orchestrator_code = _without_comments(self.orchestrator)
        self.assertIn(
            'export BINDGEN_EXTRA_CLANG_ARGS="--target=$TRIPLE --sysroot=$sysroot"',
            orchestrator_code,
        )
        self.assertIn("/usr/lib/llvm-18/lib", orchestrator_code)

    def test_changes_filters_route_the_loong64_scripts(self):
        # The gate leg triggers on rust_full and the release contract suite
        # reads these scripts: a script-only edit (sha re-pin, sysroot
        # package list, packaging step) must run the legs that execute them
        # instead of skipping everything with required-gate green on
        # `skipped`. Judged on comment-stripped text: commenting a path out
        # (a plausible filter-debugging move) must fail here, not pass the
        # pin while the regression it guards is live.
        stripped_workflow = _without_comments(self.pr_check_workflow)
        rust_full = stripped_workflow.split(
            "\n            rust_full:", maxsplit=1
        )[1].split("\n            cli_rust:", maxsplit=1)[0]
        release_contract = stripped_workflow.split(
            "\n            release_contract:", maxsplit=1
        )[1].split("\n            pet:", maxsplit=1)[0]
        for script in (
            "scripts/ci-loongarch64-toolchain.sh",
            "scripts/ci-loongarch64-sysroot.sh",
            "scripts/ci-build-linux-loongarch64.sh",
        ):
            self.assertIn(f"'{script}'", rust_full, script)
        for script in (
            "scripts/package-deb-loongarch64.sh",
            "scripts/ci-build-linux-loongarch64.sh",
            "scripts/ci-loongarch64-toolchain.sh",
            "scripts/ci-loongarch64-sysroot.sh",
        ):
            self.assertIn(f"'{script}'", release_contract, script)

    def test_packager_depends_match_the_tauri_config(self):
        # The hand-assembled deb duplicates the bundler's Depends/Recommends
        # (tauri-bundler reads them from bundle.linux.deb of the linux
        # tauri.conf.json); a silent drift would stale the loong64 deb's
        # runtime deps exactly like the drifts this packager exists to
        # avoid. Both sides carry the same source order, so the parity is
        # pinned order-sensitive via assertListEqual (stricter, and a
        # reorder must be a conscious change on both sides).
        config = (
            ROOT / "pinvou3-app/src-tauri/config/platforms/linux/tauri.conf.json"
        ).read_text(encoding="utf-8")

        def json_list(name):
            match = re.search(rf'"{name}":\s*\[(.*?)\]', config, re.S)
            self.assertIsNotNone(match, f"{name} missing from the linux tauri.conf.json")
            return [
                item.strip().strip('"')
                for item in match.group(1).split(",")
                if item.strip()
            ]

        def packager_list(name):
            match = re.search(rf'{name}="([^"]*)"', self.packager)
            self.assertIsNotNone(match, f"{name} missing from the packager script")
            return [
                item.strip()
                for item in match.group(1).split(",")
                if item.strip()
            ]

        self.assertListEqual(packager_list("DEPENDS"), json_list("depends"))
        self.assertListEqual(packager_list("RECOMMENDS"), json_list("recommends"))

    def test_packager_replicates_the_bundler_content_surface(self):
        # The loong64 deb is hand-assembled to tauri-bundler's layout, and
        # the Depends parity test above is the only other packager pin.
        # Everything else that decides what a user's system looks like after
        # `dpkg -i` is pinned here, so a packaging regression cannot ship
        # green: (1) executables are 0755 — the app binary, the ASR engine,
        # the asr shim (exec'd directly via PINVOU3_ASR_CMD; the repo blob
        # is 0755 and tauri-bundler's resource copy preserves that on the
        # amd64/arm64 debs) and the maintainer scripts (dpkg requires it);
        # (2) data files are 0644; (3) the desktop entry and the three
        # hicolor icon destinations (incl. the @2 retina directory the
        # bundler derives from real pixel dimensions) are installed;
        # (4) md5sums cover every data file; (5) Installed-Size sums
        # regular files under the data root, DEBIAN excluded, /1024 (the
        # bundler's total_dir_size excludes directory inodes, so `du -sb`
        # would overstate).
        packager = _without_comments(self.packager)
        for executable in (
            'install -m 0755 "$app_binary"',
            'install -m 0755 "$asr_engine"',
            'install -m 0755 "$APP_SRC_TAURI/resources/platforms/linux/asr/pinvou3-asr-shim.py"',
            'install -m 0755 "$postinst_src"',
            'install -m 0755 "$prerm_src"',
        ):
            self.assertIn(executable, packager, executable)
        self.assertNotIn('install -m 0644 "$APP_SRC_TAURI/resources/platforms/linux/asr/pinvou3-asr-shim.py"', packager)
        for data_file in (
            'install -m 0644 "$APP_SRC_TAURI/resources/platforms/linux/asr/LICENSE-SenseVoice.cpp"',
            'install -m 0644 "$desktop_template"',
        ):
            self.assertIn(data_file, packager, data_file)
        for icon in ('$icon_32', '$icon_128', '$icon_256'):
            self.assertIn(f'install -m 0644 "{icon}"', packager, icon)
        # freedesktop hicolor destinations, including the bundler's @2
        # spelling for the 256px retina icon.
        for destination in (
            "usr/share/applications/$PACKAGE_NAME.desktop",
            "usr/share/icons/hicolor/32x32/apps/",
            "usr/share/icons/hicolor/128x128/apps/",
            "usr/share/icons/hicolor/256x256@2/apps/",
        ):
            self.assertIn(destination, packager, destination)
        # md5sums over every data file, sorted for determinism, ./ prefix
        # stripped (bundler's generate_md5sums shape).
        self.assertIn("find . -type f -not -path './DEBIAN/*' -print0", packager)
        self.assertIn("xargs -0 md5sum", packager)
        # Installed-Size: regular files only (never directory inodes),
        # integer division by 1024 per Debian policy.
        self.assertIn("find . -type f -not -path './DEBIAN/*' -printf '%s\\n'", packager)
        self.assertIn("/ 1024", packager)

    def test_sysroot_pins_the_verified_package_and_pc_module_lists(self):
        # The sysroot's self-verification is only as strong as the lists it
        # checks: trimming VERIFY_PC_MODULES or the DEV_PACKAGES seed set
        # would shrink the verification (and the dependency closure)
        # silently. The package list is also part of the cache-generation
        # identity (the manifest records it), so both lists are pinned in
        # full — any edit must be a conscious, reviewed change.
        def bash_list(script_text, name):
            match = re.search(rf"^{name}=\(\n((?:  .*\n)+)\)", script_text, re.M)
            self.assertIsNotNone(match, f"{name} list missing from the sysroot script")
            return [line.strip() for line in match.group(1).splitlines() if line.strip()]

        sysroot = _without_comments(self.sysroot)
        self.assertListEqual(
            bash_list(sysroot, "DEV_PACKAGES"),
            [
                "libwebkit2gtk-4.1-dev",
                "libssl-dev",
                "libayatana-appindicator3-dev",
                "libpipewire-0.3-dev",
                "libdrm-dev",
                "libgbm-dev",
                "libegl-dev",
                "libdbus-1-dev",
            ],
        )
        self.assertListEqual(
            bash_list(sysroot, "VERIFY_PC_MODULES"),
            [
                "webkit2gtk-4.1",
                "javascriptcoregtk-4.1",
                "libsoup-3.0",
                "gtk+-3.0",
                "gdk-3.0",
                "glib-2.0",
                "gio-2.0",
                "gobject-2.0",
                "ayatana-appindicator3-0.1",
                "openssl",
                "libssl",
                "libcrypto",
                "libpipewire-0.3",
                "libdrm",
                "gbm",
                "egl",
                "dbus-1",
            ],
        )

    def test_pr_check_leg_rust_cache_saves_on_main_only(self):
        # The loong leg's rust-cache block is the sixth of its kind; the
        # five sibling blocks are pinned by the gate/cache-policy suites.
        # Dropping `save-if` would turn every PR into a cache write (quota
        # churn the cache policy forbids); dropping the shared key would
        # cross-pollute the host-arch caches the comment above the block
        # warns about.
        leg = _without_comments(self.pr_check_workflow).split(
            "\n  rust-loongarch64-check:", maxsplit=1
        )[1]
        # Job-level keys sit at exactly two spaces; everything inside the
        # job is indented deeper, so the next `  <key>:` line ends the leg.
        leg = re.split(r"\n  [a-zA-Z]", leg, maxsplit=1)[0]
        self.assertIn("shared-key: rust-loongarch64-check", leg)
        self.assertIn("save-if: ${{ github.ref == 'refs/heads/main' }}", leg)

    def _step_run_block(self, workflow, step_name):
        # The run block of one named step, comments stripped. Both host-tools
        # steps sit inside their job's steps at the same indentation, so the
        # extractor scopes by the exact "- " step boundary; trailing
        # whitespace is normalized (the next-step boundary differs between
        # the two workflows by one trailing newline).
        at = workflow.find(step_name)
        self.assertGreater(at, 0, f"missing step {step_name!r}")
        start = workflow.rfind("\n      - ", 0, at)
        end = workflow.find("\n      - ", at)
        block = workflow[start:] if end == -1 else workflow[start:end]
        return _without_comments(block).split("run: |", maxsplit=1)[1].strip()

    def test_pr_check_leg_checks_all_three_workspaces(self):
        self.assertIn("runs-on: ubuntu-22.04\n", self.pr_job)
        self.assertIn(
            "--manifest-path pinvou3-app/src-tauri/Cargo.toml --target "
            "loongarch64-unknown-linux-gnu --locked",
            self.pr_job,
            "the cross check must cover src-tauri",
        )
        # knowledge additionally compiles its tests for the target: the stub
        # contract (embedding::stub_tests) is the cfg-live path on
        # loongarch64, so the cross leg must type-check it (execution stays
        # on the host feature-off leg).
        self.assertIn(
            "--manifest-path pinvou-knowledge/Cargo.toml --target "
            "loongarch64-unknown-linux-gnu --locked --tests",
            self.pr_job,
            "the cross check must cover pinvou-knowledge including its tests",
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
