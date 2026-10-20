"""Pins the LoongArch64 (dpkg arch: loong64) release/CI contract.

GitHub hosts no loong64 runner, so the release build runs cross on x86_64
ubuntu-22.04 and the pr-check cross leg type-checks the target per PR:
- the release job must invoke the orchestrator (scripts/ci-build-linux-
  loongarch64.sh) on an x86_64 runner, must keep RUSTFLAGS remap-only (the
  pinned cross toolchain ships bfd only — cargo links route through the
  generated rust-lld wrapper instead) and must not override LTO;
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

import json
import os
import re
import shlex
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
ELF_POLICY_SCRIPT = ROOT / "scripts" / "check-linux-elf-policy.sh"
ASR_BUILD_SCRIPT = ROOT / "scripts" / "asr" / "build-sensevoice-runtime.sh"

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
        # RUSTFLAGS stay remap-only: --remap-path-prefix is rustc-side (the
        # macOS job sets it alone) and keeps build-machine paths out of the
        # artifact like every other release job. Linker selection is NOT a
        # RUSTFLAGS concern here: cargo routes through the toolchain's
        # generated <triple>-gcc-rust-lld wrapper (see the toolchain test
        # below — rustc's LLVM emits R_LARCH_CALL36 that the pinned
        # bfd 2.41 cannot resolve), so the arm64 job's HOST-side
        # -fuse-ld=lld / --icf=safe must still never be copied, and LTO
        # stays owned by Cargo.toml. Comment mentions of the forbidden
        # flags are deliberate (they warn future editors) and are stripped
        # here — only live configuration counts.
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
        self.assertNotIn("-fuse-ld=lld", self.job_code)
        self.assertNotIn("--icf=safe", self.job_code)
        self.assertNotIn("CARGO_PROFILE_RELEASE_LTO", self.job_code)
        self.assertNotIn("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", self.job_code)
        # The orchestrator owns the actual cargo invocations and must not
        # override the job's flags (its comments deliberately warn; strip
        # them before scanning).
        orchestrator_code = _without_comments(self.orchestrator)
        self.assertNotIn("-fuse-ld=lld", orchestrator_code)
        self.assertNotIn("--icf=safe", orchestrator_code)
        self.assertNotIn("CARGO_PROFILE_RELEASE_LTO", orchestrator_code)
        self.assertNotIn("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", orchestrator_code)
        for line in orchestrator_code.splitlines():
            stripped = line.strip()
            self.assertFalse(
                stripped.startswith("RUSTFLAGS=") or stripped.startswith("export RUSTFLAGS"),
                f"the orchestrator must leave RUSTFLAGS unset, found: {stripped}",
            )

    def test_verify_sysroot_asserts_the_startup_objects(self):
        # Startup objects are named by no linker script and no -l flag: the
        # gcc driver selects them by its own rules (Scrt1.o/crti.o/crtn.o
        # for -pie, crt1.o for -no-pie) out of directories it derives from
        # its own configuration. A missing one surfaces only at real link
        # time ("ld: cannot find Scrt1.o" — the exact failure the 0.10.13
        # release link died on) which the check-only pr-check legs never
        # reach. verify_sysroot must assert the full set so a prune/extract
        # regression fails at setup time with the object named.
        sysroot = _without_comments(self.sysroot)
        self.assertIn(
            "for crt_obj in Scrt1.o crt1.o crti.o crtn.o; do",
            sysroot,
            "verify_sysroot must assert the startup objects the cross driver "
            "selects by its own rules",
        )
        self.assertIn(
            "sysroot lacks the startup object $crt_obj",
            sysroot,
            "the startup-object check must fail with its own message",
        )

    def test_cmake_link_surfaces_carry_the_multiarch_search_path(self):
        # The deepin tree is Debian-multiarch: startfiles and lib* live under
        # usr/lib/loongarch64-linux-gnu/, which neither the cross gcc's
        # default search nor <sysroot>/usr/lib covers. Both cmake link
        # surfaces must hand that dir to the driver: the generated toolchain
        # file seeds -B (startfiles) and -L (-l resolution) via the
        # *_FLAGS_INIT / *_LINKER_FLAGS_INIT seeds (CI round 2), and the ASR
        # build's explicit -DCMAKE_EXE_LINKER_FLAGS must RE-ADD the -L — an
        # explicit -D cache entry silently replaces the INIT seed (CI round
        # 10: "cannot find -ldl/-lpthread" after configure went green).
        sysroot = _without_comments(self.sysroot)
        self.assertEqual(
            sysroot.count("-B$root/usr/lib/$MULTIARCH_DIR"),
            2,
            "both cmake language-flag variants must carry the same -B "
            "startfile dir",
        )
        for kind in ("EXE", "SHARED", "MODULE"):
            self.assertIn(
                f"set(CMAKE_{kind}_LINKER_FLAGS_INIT \\\"-L$root/usr/lib/$MULTIARCH_DIR\\\")",
                sysroot,
                f"the {kind} linker-flag seed must carry the multiarch -L",
            )
        asr = _without_comments(ASR_BUILD_SCRIPT.read_text(encoding="utf-8"))
        self.assertIn(
            '-DCMAKE_EXE_LINKER_FLAGS="-L$(dirname "$CMAKE_TOOLCHAIN_FILE")'
            '/usr/lib/loongarch64-linux-gnu -static-libstdc++ -static-libgcc"',
            asr,
            "the ASR cross build must re-add the multiarch -L its explicit "
            "EXE linker flags otherwise drop",
        )
        # A cmake failure must dump the try-compile probe logs (tail, fail
        # closed): cmake 3.31 buries try-compile stderr in
        # CMakeConfigureLog.yaml, and without the dump a cross-build failure
        # is undiagnosable from CI logs alone (CI round 8).
        self.assertIn("CMakeConfigureLog.yaml", asr)
        self.assertIn("tail -n 120", asr)

    def test_release_job_gives_the_cross_build_headroom(self):
        # arm64 (native) gets 90 minutes; the cross build reuses the same
        # runner shape with the cross toolchain in front of it — the budget
        # is 90 * ~1.5. Pin it so the headroom cannot silently disappear,
        # and pin the count: GitHub's last-key-wins YAML parsing means a
        # second live timeout-minutes would silently void the pinned value.
        self.assertIn("timeout-minutes: 135", self.job_code)
        self.assertEqual(
            self.job_code.count("timeout-minutes:"),
            1,
            "exactly one timeout budget may exist in the job",
        )

    def test_packager_header_names_the_bundled_cli_version(self):
        # The hand packager derives its parity claims from the bundler inside
        # @tauri-apps/cli; the dependency is caret-ranged, so a routine CLI
        # bump could silently rot the parity (Installed-Size semantics, tar
        # header modes, control field order, symlink handling) and the
        # bundler can never run for this triple to expose it. Pin the
        # header's named version to the app's actual dependency at FULL
        # major.minor.patch precision: a caret range admits patch bumps
        # that still rebuild the bundler binary, so the re-derivation duty
        # triggers on any version change, not just minor ones.
        named = re.search(r"@tauri-apps/cli (\d+\.\d+\.\d+)", self.packager)
        self.assertIsNotNone(
            named, "the packager header must name its @tauri-apps/cli baseline"
        )
        package_json = json.loads(
            (ROOT / "pinvou3-app" / "package.json").read_text(encoding="utf-8")
        )
        spec = package_json.get("dependencies", {}).get(
            "@tauri-apps/cli"
        ) or package_json.get("devDependencies", {}).get("@tauri-apps/cli")
        self.assertIsNotNone(spec, "package.json must depend on @tauri-apps/cli")
        bundled = re.search(r"[\^~]?(\d+\.\d+\.\d+)", spec)
        self.assertIsNotNone(bundled, f"unexpected @tauri-apps/cli spec: {spec}")
        self.assertEqual(
            named.group(1),
            bundled.group(1),
            "the packager header's tauri-cli baseline drifted from the "
            "bundled @tauri-apps/cli — re-derive the parity claims "
            "(Installed-Size, tar header modes, control order, symlink "
            "handling) against the new bundler in the same PR",
        )

    # ---- bundling contract ----

    def test_bundling_is_skipped_via_the_bundles_none_path(self):
        # tauri-bundler panics on the loongarch64 triple (binary_arch() has
        # no LoongArch variant), so the orchestrator's tauri invocation must
        # go through build.js's `--bundles none` (translated to the CLI's
        # --no-bundle) and the deb must be hand-assembled. Judged on
        # comment-stripped text, like every live-configuration pin: the
        # orchestrator's header banners name the packager three times, so a
        # raw-text match stays green even with the live invocation deleted.
        orchestrator_code = _without_comments(self.orchestrator)
        self.assertIn(
            'node scripts/tauri/build.js build --target "$TRIPLE" --bundles none',
            orchestrator_code,
        )
        self.assertIn("package-deb-loongarch64.sh", orchestrator_code)
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

    def test_orchestrator_bridge_node_guard_cannot_die_of_sigpipe(self):
        # `find | grep -q` under pipefail is a race: a second match written
        # after grep exits kills find (SIGPIPE, 141) and the `!` turns that
        # into a spurious "no node binary" failure — the exact artifact the
        # guard exists to protect would be reported missing. The traversal
        # must stop at the first match.
        self.assertIn(
            'find "$bridge_dir" -type f -name node -print -quit | grep -q .',
            _without_comments(self.orchestrator),
        )

    def test_orchestrator_smoke_links_before_the_first_real_build(self):
        # The pr-check legs are check-only and never link, so the release
        # job's first link used to be the full knowledge-server compile —
        # the worst place to discover a broken cross link environment. A
        # hello-world link must sit before it and reuse the exact exported
        # linker variable (the wrapper, not a copied literal) so the smoke
        # cannot drift from what the cargo builds link with. Cargo links
        # carry no rust flags of their own any more (the remap rides the
        # workflow's RUSTFLAGS; the linker wrapper embeds the -B/-fuse-ld
        # handoff), so the smoke passes exactly what a cargo link passes.
        code = _without_comments(self.orchestrator)
        self.assertIn("-C linker=\"$CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_LINKER\"", code)
        self.assertIn('assert_loongarch_elf "$smoke_dir/hello"', code)
        self.assertIn(
            "cross link smoke failed",
            code,
            "the smoke must fail loudly with its own message",
        )
        self.assertLess(
            code.index("hello.rs"),
            code.index("--manifest-path"),
            "the link smoke must run before the first real cargo build",
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
                    # The block must stay pure, underscore-spelled env
                    # assignments: the pr-check leg's GITHUB_ENV grep keeps
                    # only `^[A-Za-z_][A-Za-z0-9_]*=` lines, and a hyphenated
                    # name is not a valid bash identifier — one pasted
                    # `export` of it dies at runtime (bash -n accepts it).
                    # A dash-spelled line printed here is dead weight today
                    # and a crash waiting for a copy-paste, so it fails here.
                    self.assertRegex(line, r"^[A-Za-z_][A-Za-z0-9_]*=")
        def assert_block_exports(block, block_name, required):
            # Line-anchored on purpose: the block is heredoc DATA, where a
            # leading `#` is printed literally and the pr-check leg's
            # `^[A-Za-z_]` grep drops the line — so a commented-out required
            # variable must fail HERE (a bare substring match would stay
            # green while GITHUB_ENV loses the variable).
            self.assertTrue(
                any(line.startswith(required) for line in block.splitlines()),
                f"{block_name} export block is missing or commented out: {required!r}",
            )

        toolchain_block = self.toolchain.split(
            "---- ci-loongarch64-toolchain: export in the CI step ----", maxsplit=1
        )[1].split("---- notes ----", maxsplit=1)[0]
        for required in (
            "PINVOU_LOONG64_TOOLCHAIN_PREFIX=",
            "CC_loongarch64_unknown_linux_gnu=",
            "CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_LINKER=",
        ):
            assert_block_exports(toolchain_block, "toolchain", required)
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
            assert_block_exports(sysroot_block, "sysroot", required)

    def test_pipeline_scripts_only_export_valid_bash_identifiers(self):
        # Execution probe: `bash -n` accepts `export CC_loongarch64-...=x`
        # because the hyphen is only rejected when the line RUNS — under
        # `set -euo pipefail` the release build would die with `not a valid
        # identifier` before the first compile step. Judge comment-stripped
        # text so a disabled line cannot satisfy (or hide behind) the pin,
        # and tokenize with shlex so quoted values containing spaces stay
        # one assignment token.
        offenders = []
        for path in (ORCHESTRATOR, TOOLCHAIN_SCRIPT, SYSROOT_SCRIPT, PACKAGING_SCRIPT):
            for number, line in enumerate(
                _without_comments(path.read_text(encoding="utf-8")).splitlines(), start=1
            ):
                match = re.match(r"\s*export\s+(.+?)\s*$", line)
                if not match:
                    continue
                for token in shlex.split(match.group(1)):
                    if token.startswith("-"):
                        continue  # export flags (-n, -p), not names
                    name = token.split("=", 1)[0]
                    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
                        offenders.append(f"{path.name}:{number}: export {token}")
        self.assertEqual(
            offenders,
            [],
            "hyphenated (or otherwise non-identifier) export names are runtime "
            "errors under set -euo pipefail; use the underscore spelling that "
            "cc-rs actually reads:\n" + "\n".join(offenders),
        )

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
        # The auto fallback must re-establish the fresh-tree invariant before
        # the apt set extracts: docker_method can fail AFTER partially
        # extracting into the output dir (dpkg-deb -x never deletes), and the
        # fallback branch is the only path that reaches apt_method without
        # passing the leading rm -rf again.
        fallback = sysroot.split("auto: falling back to the direct apt method", maxsplit=1)[1]
        fallback = fallback.split(";;", maxsplit=1)[0]
        self.assertIn('rm -rf -- "$output_dir"', fallback)
        self.assertIn('mkdir -p "$output_dir"', fallback)
        self.assertIn('> "$manifest.tmp"', sysroot)
        self.assertIn('mv "$manifest.tmp" "$manifest"', sysroot)

    def test_toolchain_cargo_linker_routes_through_rust_lld(self):
        # rustc's LLVM (1.99+) emits R_LARCH_CALL36 (psABI v2.30) for some
        # direct calls; the pinned toolchain's bundled bfd (2.41, chosen for
        # the glibc 2.38 floor) rejects it with "unsupported relocation type
        # 0x6e" at the cargo link step (pinvou3#1993 CI round 5). The
        # toolchain therefore materializes a <triple>-gcc-rust-lld wrapper —
        # the sibling gcc with -B<shim dir> -fuse-ld=lld, where the shim dir
        # holds an `ld.lld` that resolves the rust-bundled gcc-ld/ld.lld
        # from the ACTIVE rustc sysroot at link time — and points
        # CARGO_TARGET_*_LINKER at it, so cargo links with the same LLVM
        # that emitted the relocs while native cmake/cc builds keep bfd and
        # the glibc 2.38 floor stays unchanged. (GCC's -fuse-ld only takes
        # NAMES resolved as `ld.<name>` via -B dirs — CI round 7 proved an
        # absolute path dies with "unrecognized command-line option".) Pins
        # the wiring, then executes the generated shim and wrapper against
        # fake rustc/gcc so the exec chain is exercised, not just spelled.
        self.assertIn('mkdir -p "$toolchain_bin/lld-shim"', self.toolchain)
        self.assertIn('wrapper="$toolchain_bin/$TRIPLE-gcc-rust-lld"', self.toolchain)
        self.assertIn(
            "CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_LINKER="
            "$toolchain_bin/$TRIPLE-gcc-rust-lld",
            self.toolchain,
        )
        # The shim and wrapper must be regenerated and syntax-checked on
        # every run.
        self.assertIn('sh -n "$toolchain_bin/lld-shim/ld.lld"', self.toolchain)
        self.assertIn('sh -n "$wrapper"', self.toolchain)

        # The orchestrator is the release path's single env source of truth
        # and re-exports CARGO_TARGET_*_LINKER: it must point at the wrapper
        # (an earlier upstream round silently re-exported the bare gcc and
        # undid the whole wrapper — CI round 6) and fail closed when a stale
        # prefix lacks it, at setup time rather than at the final link.
        orchestrator = _without_comments(self.orchestrator)
        self.assertIn(
            'cargo_linker="$TOOLCHAIN_BIN/$TRIPLE-gcc-rust-lld"',
            orchestrator,
        )
        self.assertIn(
            'export CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_LINKER="$cargo_linker"',
            orchestrator,
        )
        self.assertIn(
            "re-run scripts/ci-loongarch64-toolchain.sh to materialize the "
            "rust-lld linker wrapper",
            orchestrator,
            "a stale prefix without the wrapper must fail with the actionable "
            "message",
        )

        lines = self.toolchain.splitlines()
        start = next(
            i for i, l in enumerate(lines) if l.startswith('mkdir -p "$toolchain_bin/lld-shim"')
        )
        end = next(
            i
            for i in range(start, len(lines))
            if lines[i].startswith('chmod +x "$wrapper"')
        )
        generation = "\n".join(lines[start : end + 1])

        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            bin_dir = tmp / "bin"
            bin_dir.mkdir()
            sysroot = tmp / "sysroot"
            gcc_ld = sysroot / "lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld"
            gcc_ld.mkdir(parents=True)
            fake_rustc = bin_dir / "rustc"
            fake_rustc.write_text("#!/bin/sh\necho %s\n" % shlex.quote(str(sysroot)))
            fake_rustc.chmod(0o755)
            fake_gcc = bin_dir / "loongarch64-unknown-linux-gnu-gcc"
            fake_gcc.write_text(
                '#!/bin/sh\nprintf "FAKE-GCC"; printf " <%s>" "$@"; printf "\\n"\n'
            )
            fake_gcc.chmod(0o755)
            # A REAL executable stand-in for the rust-bundled ld.lld: the
            # shim must exec it with cargo's link arguments verbatim.
            fake_ld = gcc_ld / "ld.lld"
            fake_ld.write_text(
                '#!/bin/sh\nprintf "FAKE-LLD"; printf " <%s>" "$@"; printf "\\n"\n'
            )
            fake_ld.chmod(0o755)

            proc = subprocess.run(
                [
                    "bash",
                    "-c",
                    "toolchain_bin=%s; TRIPLE=loongarch64-unknown-linux-gnu; source /dev/stdin"
                    % shlex.quote(str(bin_dir)),
                ],
                input=generation,
                capture_output=True,
                text=True,
                timeout=60,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            wrapper = bin_dir / "loongarch64-unknown-linux-gnu-gcc-rust-lld"
            shim = bin_dir / "lld-shim" / "ld.lld"
            self.assertTrue(wrapper.exists(), "setup must materialize the wrapper")
            self.assertTrue(shim.exists(), "setup must materialize the ld.lld shim")

            # Happy path through the FULL chain: wrapper → gcc (fake) gets
            # the -B shim dir + -fuse-ld=lld ahead of cargo's own arguments.
            linked = subprocess.run(
                [str(wrapper), "-o", "app", "a.o", "b.o"],
                env={"PATH": f"{bin_dir}:{os.environ.get('PATH', '')}"},
                capture_output=True,
                text=True,
                timeout=60,
            )
            self.assertEqual(linked.returncode, 0, linked.stderr)
            self.assertIn(f"<-B{bin_dir / 'lld-shim'}>", linked.stdout)
            self.assertIn("<-fuse-ld=lld>", linked.stdout)
            self.assertIn("<-o> <app>", linked.stdout)

            # The shim itself resolves and execs the rust-bundled ld.lld
            # with the linker arguments gcc passes it (argv0 is the shim
            # path, so lld's flavor detection sees `ld.lld`).
            shimmed = subprocess.run(
                [str(shim), "-o", "app", "a.o", "b.o"],
                env={"PATH": f"{bin_dir}:{os.environ.get('PATH', '')}"},
                capture_output=True,
                text=True,
                timeout=60,
            )
            self.assertEqual(shimmed.returncode, 0, shimmed.stderr)
            self.assertIn("FAKE-LLD <-o> <app> <a.o> <b.o>", shimmed.stdout)

            # No rustc on PATH: fail loudly with the actionable message, not
            # with a silent bfd fallback.
            broken = subprocess.run(
                [str(shim), "-o", "app", "a.o"],
                env={"PATH": str(bin_dir)},
                capture_output=True,
                text=True,
                timeout=60,
            )
            self.assertEqual(broken.returncode, 127, broken.stderr)
            self.assertIn("no gcc-ld/ld.lld", broken.stderr)

    def test_sysroot_loader_view_is_a_resolving_relative_symlink(self):
        # The /lib64 loader view is created by prepending "../" to the
        # ROOT-relative loader path: a symlink target resolves against the
        # LINK's directory ($root/lib64), not the sysroot root, so a bare
        # root-relative target dangles and every later resolution of
        # ld-linux-loongarch-lp64d.so.1 through the /lib64 view dies
        # (pinvou3#1993 CI round 3: "sysroot lacks the /lib64/... view").
        # The corrected form and the fail-closed post-check are pinned, then
        # the whole prune_sysroot runs on a synthetic tree so the view
        # contract is exercised, not just spelled.
        sysroot = _without_comments(self.sysroot)
        self.assertIn('ln -s "../${loader_src#"$root"/}"', sysroot)
        self.assertIn("view did not resolve after materialization", sysroot)

        import os
        import shutil
        import tempfile

        lines = self.sysroot.splitlines()

        def fn_text(name):
            start = next(
                i for i, l in enumerate(lines) if l.startswith(f"{name}() {{")
            )
            end = next(i for i in range(start, len(lines)) if lines[i] == "}")
            return "\n".join(lines[start : end + 1])

        prune_start = next(
            i for i, l in enumerate(lines) if l.startswith("prune_sysroot() {")
        )
        prune_end = next(i for i in range(prune_start, len(lines)) if lines[i] == "}")

        with tempfile.TemporaryDirectory() as tmp:
            fragment = Path(tmp) / "prune-fragment.sh"
            fragment.write_text(
                "MULTIARCH_DIR=loongarch64-linux-gnu\n"
                + fn_text("fail")
                + "\n"
                + fn_text("log")
                + "\n"
                + "\n".join(lines[prune_start : prune_end + 1])
                + "\n",
                encoding="utf-8",
            )
            root = Path(tmp) / "root"
            ma = root / "usr/lib/loongarch64-linux-gnu"
            # Loader deliberately in the lib64/multiarch variant location —
            # the worst case for a "../"-target computation.
            loader = root / "lib64/loongarch64-linux-gnu/ld-linux-loongarch-lp64d.so.1"
            loader.parent.mkdir(parents=True)
            ma.mkdir(parents=True)
            loader.write_text("ELF")
            (ma / "libc.so.6").write_text("ELF")
            # glibc ≥2.34 runtime stubs: libc6 ships these; libc6-dev ships
            # only EMPTY .a stubs (pruned) and no lib*.so.
            (ma / "libpthread.so.0").write_text("ELF")
            (ma / "libdl.so.2").write_text("ELF")
            # Debian dev symlink with an ABSOLUTE target into the (usr-merged)
            # runtime root: inside the sysroot it escapes to the build host
            # unless the prune rebases it onto the sysroot's /lib view.
            (ma / "libdbus-1.so.3.32.4").write_text("ELF")
            (ma / "libdbus-1.so").symlink_to("/lib/loongarch64-linux-gnu/libdbus-1.so.3.32.4")
            (ma / "libc_nonshared.a").write_text("ar")
            (ma / "libc.so").write_text(
                "GROUP ( /lib/loongarch64-linux-gnu/libc.so.6"
                " /usr/lib/loongarch64-linux-gnu/libc_nonshared.a"
                "  AS_NEEDED ( /lib64/ld-linux-loongarch-lp64d.so.1 ) )\n"
            )
            (ma / "libunreferenced.a").write_text("ar")
            (ma / "libunreferenced.la").write_text("libtool")
            # Debian layout: arch-specific glibc headers under the multiarch
            # include subtree, generic headers directly in usr/include.
            multi_inc = root / "usr/include/loongarch64-linux-gnu/bits"
            multi_inc.mkdir(parents=True)
            (multi_inc / "wordsize.h").write_text("#define __WORDSIZE 64\n")
            (root / "usr/include/stdio.h").write_text("/* generic */\n")
            (root / "etc").mkdir()
            (root / "usr/bin").mkdir()

            env = dict(os.environ)
            system_sed = shutil.which("sed")
            is_gnu_sed = False
            if system_sed:
                probe = subprocess.run(
                    [system_sed, "--version"], capture_output=True, text=True
                )
                is_gnu_sed = probe.returncode == 0 and "GNU sed" in probe.stdout
            if not is_gnu_sed and system_sed:
                # Dev-macOS only: BSD sed cannot run the script's GNU
                # `sed -i -e` rewrite (it eats -e as the backup suffix), so
                # shim it to the equivalent `-i ''` form. CI (Ubuntu) has
                # GNU sed natively and takes the unshimmed path.
                shim = Path(tmp) / "bin"
                shim.mkdir()
                shim_sed = shim / "sed"
                shim_sed.write_text(
                    "#!/bin/sh\n"
                    'if [ "$1" = "-i" ]; then shift; set -- -i \'\' "$@"; fi\n'
                    f'exec "{system_sed}" "$@"\n'
                )
                shim_sed.chmod(0o755)
                env["PATH"] = f"{shim}:{env.get('PATH', '')}"

            proc = subprocess.run(
                [
                    "bash",
                    "-c",
                    "source %s; prune_sysroot %s"
                    % (shlex.quote(str(fragment)), shlex.quote(str(root))),
                ],
                env=env,
                capture_output=True,
                text=True,
                timeout=120,
            )
            self.assertEqual(proc.returncode, 0, f"prune_sysroot failed: {proc.stderr}")
            # Every view the rewritten ld scripts resolve through bare names
            # via the -L search path must exist inside the sysroot.
            self.assertTrue((root / "lib").is_symlink() and (root / "lib").exists())
            self.assertTrue(
                (root / "lib64/ld-linux-loongarch-lp64d.so.1").exists(),
                "the /lib64 loader view must resolve through the ../ target",
            )
            self.assertTrue((root / "lib/loongarch64-linux-gnu/libc.so.6").exists())
            rewritten = (ma / "libc.so").read_text()
            # Bare filenames: resolution rides the -L search path (present in
            # both cargo and cmake link contexts), NOT the =-sysroot form —
            # that resolves against whichever --sysroot the driver applies,
            # and cargo links get gcc's bundled sysroot, not this one.
            self.assertIn("GROUP ( libc.so.6 libc_nonshared.a", rewritten)
            self.assertIn("ld-linux-loongarch-lp64d.so.1", rewritten)
            self.assertNotIn("=/", rewritten)
            self.assertNotIn(" /lib/", rewritten)
            self.assertNotIn(" /usr/lib/", rewritten)
            # Referenced archive member kept; unreferenced bulk dropped.
            self.assertTrue((ma / "libc_nonshared.a").exists())
            self.assertFalse((ma / "libunreferenced.a").exists())
            self.assertFalse((ma / "libunreferenced.la").exists())
            self.assertFalse((root / "etc").exists())
            self.assertFalse((root / "usr/bin").exists())
            # The multiarch header merge must land arch bits in usr/include
            # without disturbing the generic headers (union, no clobber).
            self.assertTrue(
                (root / "usr/include/bits/wordsize.h").exists(),
                "multiarch header merge did not materialize bits/wordsize.h",
            )
            self.assertTrue((root / "usr/include/stdio.h").exists())
            self.assertTrue(
                (root / "usr/include/loongarch64-linux-gnu/bits/wordsize.h").exists()
            )
            dl_script = (ma / "libdl.so").read_text()
            # Bare runtime names: cargo links carry no --sysroot, so the
            # stub must resolve through the -L search path.
            self.assertIn("GROUP ( libpthread.so.0 )", (ma / "libpthread.so").read_text())
            self.assertIn("GROUP ( libdl.so.2 )", dl_script)
            # The absolute dev symlink must be rebased onto the sysroot's
            # /lib view and still resolve through it.
            self.assertTrue(
                (ma / "libdbus-1.so").exists(), "rebased libdbus-1.so does not resolve"
            )
            self.assertEqual(
                str((ma / "libdbus-1.so").readlink()),
                "../../../lib/loongarch64-linux-gnu/libdbus-1.so.3.32.4",
            )

    def test_elf_policy_uses_bfd_independent_readelf(self):
        # check-linux-elf-policy.sh dumps dynamic symbols with readelf, not
        # objdump: objdump's BFD only knows the targets its build enabled and
        # the x86_64 jammy runner's binutils (and its binutils-multiarch)
        # has no LoongArch backend, so a foreign-arch ELF dies with "file
        # format not recognized" and kills the release job. readelf parses
        # the ELF structures directly, so one spelling serves every arch.
        # The ASR build script required objdump only for the policy call and
        # must drop it from its host-command lists too.
        policy = _without_comments(ELF_POLICY_SCRIPT.read_text(encoding="utf-8"))
        self.assertNotIn("objdump", policy)
        self.assertIn("readelf -W --dyn-syms", policy)
        asr = _without_comments(ASR_BUILD_SCRIPT.read_text(encoding="utf-8"))
        self.assertNotIn("objdump", asr)

    # ---- cache keys (release restore-only, pr-check main-only saves) ----

    def test_release_job_restores_loong64_caches_read_only(self):
        # The FULL key spelling must match the pr-check save side exactly
        # (same script hashed): a prefix-only pin let the release key drift
        # (a different hashFiles target, an extra suffix) while staying green
        # here — the release job would then restore a cache the writer never
        # created and pay a full cold rebuild every release.
        for key, script in (
            (TOOLCHAIN_CACHE_KEY, "scripts/ci-loongarch64-toolchain.sh"),
            (SYSROOT_CACHE_KEY, "scripts/ci-loongarch64-sysroot.sh"),
        ):
            full_key = "key: " + key + "${{ hashFiles('" + script + "') }}"
            self.assertEqual(self.job_code.count(full_key), 1, key)
            self.assertIn(f"restore-keys: |\n            {key}", self.job_code, key)
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
            # Judge comment-stripped, line-anchored text: a commented-out
            # assignment must not satisfy the parity check (the raw-text
            # regex here was mutation-verified hollow — commenting the line
            # out stayed green).
            matches = re.findall(
                rf'(?m)^{name}="([^"]*)"$', _without_comments(self.packager)
            )
            self.assertEqual(
                len(matches),
                1,
                f"{name} must be assigned exactly once in live code",
            )
            return [
                item.strip()
                for item in matches[0].split(",")
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
        # (4) md5sums cover every data file; (5) Installed-Size mirrors the
        # bundler's total_dir_size — every data-root entry with DEBIAN
        # pruned, directory inodes included (verified against the pinned
        # tauri-cli-v2.11.4 sources), //1024; (6) the data tree is
        # normalized to the bundler's deterministic 0755/0644 (+exec set)
        # under umask 022 before packing; (7) resource-tree symlinks are
        # materialized first (the bundler's copy path dereferences).
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
        # stripped (bundler's generate_md5sums shape) — the sort and the
        # strip are pinned too: the comment here used to claim them while
        # the assertions only covered the find/xargs plumbing.
        self.assertIn("find . -type f -not -path './DEBIAN/*' -print0", packager)
        self.assertIn("LC_ALL=C sort -z", packager)
        self.assertIn("xargs -0 md5sum", packager)
        self.assertIn(r"sed 's#^\([0-9a-f]\{32\}  \)\./#\1#'", packager)
        # Installed-Size mirrors the bundler's total_dir_size byte for byte:
        # it sums EVERY WalkDir entry of the data root — directory inodes
        # included (verified against the pinned tauri-cli-v2.11.4 sources,
        # debian.rs total_dir_size: WalkDir with no file filter) — with
        # DEBIAN pruned, //1024 per Debian policy. The prune shape must stay
        # distinct from the md5sums find above: md5sums is files-only by
        # dpkg's format.
        self.assertIn(
            "find . \\( -path './DEBIAN' -o -path './DEBIAN/*' \\) -prune -o -printf '%s\\n'",
            packager,
        )
        self.assertIn("/ 1024", packager)
        # Archive-entry mode normalization (bundler parity): tar-rs emits
        # deterministic 0755/0644 headers regardless of the build umask;
        # without this pass the cp -a/mkdir staging under the runners' umask
        # 0002 would ship 0775 dirs and group-writable files. All three
        # statements must prune DEBIAN/ (its entries are chmod'd explicitly)
        # and run before dpkg-deb assembles the archive. Order is load-
        # bearing: the owner-exec restore must run while the original exec
        # bit is still set, so non-exec files normalize last.
        self.assertIn("umask 022", packager)
        normalization = (
            'find "$pkg_dir" -path "$control_dir" -prune -o -type d -exec chmod 0755 {} +',
            'find "$pkg_dir" -path "$control_dir" -prune -o -type f -perm -u=x -exec chmod 0755 {} +',
            'find "$pkg_dir" -path "$control_dir" -prune -o -type f ! -perm -u=x -exec chmod 0644 {} +',
        )
        for statement in normalization:
            self.assertIn(statement, packager, statement)
        self.assertLess(
            packager.index(normalization[0]),
            packager.index("dpkg-deb --root-owner-group"),
            "the mode normalization must run before the deb is built",
        )
        # (7) Resource-tree symlinks are materialized before the parity
        # passes: the bundler's copy path dereferences (fs::copy per
        # resource file), so the amd64/arm64 debs carry the npm .bin entries
        # as regular files — in md5sums, at full target size in
        # Installed-Size — and the loong64 deb must match.
        self.assertIn('readlink -f -- "$link"', packager)
        self.assertIn("-type l -print0", packager)
        self.assertIn('cp -- "$target" "$link"', packager)
        self.assertLess(
            packager.index("-type l -print0"),
            packager.index(normalization[0]),
            "symlink materialization must run before mode normalization",
        )
        # The staged-ELF sweep must dereference when probing content: find
        # -xtype f visits a symlink-to-file as the LINK path, and a plain
        # file -b lstats it into "symbolic link to ..." — silently skipping
        # exactly the wrong-arch ELF behind an npm .bin link the sweep
        # exists to catch.
        self.assertIn('file -bL "$elf"', packager)
        # Control emission (mutation-verified hollow before this pin): the
        # DEPENDS/RECOMMENDS/Installed-Size computations above are inert
        # unless they are actually written into DEBIAN/control — deleting an
        # emission line used to leave the suite green while shipping a deb
        # whose control file lost the field.
        for emission in (
            "printf 'Package: %s\\n' \"$PACKAGE_NAME\"",
            "printf 'Version: %s\\n' \"$version\"",
            "printf 'Architecture: %s\\n' \"$DEB_ARCH\"",
            "printf 'Installed-Size: %s\\n' \"$installed_size_kb\"",
            "printf 'Maintainer: pinvou\\n'",
            "printf 'Priority: optional\\n'",
            "printf 'Depends: %s\\n' \"$DEPENDS\"",
            "printf 'Recommends: %s\\n' \"$RECOMMENDS\"",
            '} > "$control_dir/control"',
        ):
            self.assertIn(emission, packager, emission)
        self.assertLess(
            packager.index('} > "$control_dir/control"'),
            packager.index("dpkg-deb --root-owner-group"),
            "the control file must be written before the deb is built",
        )

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
        # cache-on-failure matches macos-cli-check/rust-test/windows-rust-test
        # (round-48 rationale): without it a failed main push strands the
        # loong64 cache cold and every later PR pays the full cross setup.
        self.assertIn("cache-on-failure: true", leg)

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
