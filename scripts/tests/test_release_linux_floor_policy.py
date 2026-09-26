import platform
import subprocess
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release-packages.yml"
GUARD_SCRIPT = ROOT / "scripts" / "check-linux-glibc-floor.sh"
ELF_POLICY_SCRIPT = ROOT / "scripts" / "check-linux-elf-policy.sh"
ASR_BUILD_SCRIPT = ROOT / "scripts" / "asr" / "build-sensevoice-runtime.sh"
ASR_SETUP_SCRIPT = ROOT / "scripts" / "asr" / "setup-sensevoice.sh"
LINUX_OVERLAY = ROOT / "pinvou3-app/src-tauri/config/platforms/linux/tauri.conf.json"

# Linux 发布基线是 Ubuntu 22.04 (glibc 2.35) x86_64/arm64。发布二进制链接
# 构建机 glibc,runner 升级会静默抬高基线,因此 runner 标签与 glibc 守护
# 都是有意决策,本测试防止无意的基线漂移。


class ReleaseLinuxFloorPolicyTests(unittest.TestCase):
    def setUp(self):
        self.workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        self.x64_job = self.workflow.split("\n  build-linux-x64:", maxsplit=1)[1].split(
            "\n  build-linux-arm64:", maxsplit=1
        )[0]
        self.arm64_job = self.workflow.split("\n  build-linux-arm64:", maxsplit=1)[1].split(
            "\n  build-windows-x64:", maxsplit=1
        )[0]

    def test_linux_builds_pin_ubuntu_22_04_runners(self):
        # 带换行断言,避免 x64 段误匹配 ubuntu-22.04-arm 这类更长标签。
        self.assertIn("runs-on: ubuntu-22.04\n", self.x64_job)
        self.assertIn("runs-on: ubuntu-22.04-arm\n", self.arm64_job)
        self.assertNotIn("ubuntu-latest", self.x64_job.split("\n    steps:")[0])
        self.assertNotIn("ubuntu-24.04", self.arm64_job.split("\n    steps:")[0])

    def test_linux_builds_run_glibc_floor_guard_before_upload(self):
        # 守护必须覆盖两个 Linux job,且位于产物上传之前(失败即阻断发布)。
        for job in (self.x64_job, self.arm64_job):
            guard = job.find("check-linux-glibc-floor.sh")
            upload = job.find("uses: actions/upload-artifact@")
            self.assertGreater(guard, 0, "missing glibc floor guard step")
            self.assertGreater(upload, guard, "glibc floor guard must run before upload")
        self.assertEqual(self.workflow.count("check-linux-glibc-floor.sh"), 2)
        # Each guard must receive its own deb architecture, so an ELF built
        # for the other architecture can never slip into the package.
        for job, arch in ((self.x64_job, "amd64"), (self.arm64_job, "arm64")):
            step = self._step_block(job, "check-linux-glibc-floor.sh")
            self.assertRegex(step, rf'\.deb" {arch}\s*$')

    def test_linux_builds_rotate_rust_cache_key_per_runner_baseline(self):
        # 换 runner 基座必须换 rust-cache key:24.04 glibc 环境编译的缓存产物
        # 复用进 22.04 构建会把高版本符号引用混进发布二进制,-jammy 后缀把
        # 两个基座的缓存隔离。runner 标签与 cache key 必须绑定变更。
        self.assertIn("shared-key: release-linux-x64-jammy", self.x64_job)
        self.assertIn("shared-key: release-linux-arm64-jammy", self.arm64_job)

    def _step_block(self, job, needle):
        # 提取含 needle 的步骤块(步骤以 6 空格 + '- ' 开始),供单步骤断言。
        at = job.find(needle)
        self.assertGreater(at, 0, f"missing step containing {needle!r}")
        start = job.rfind("\n      - ", 0, at)
        self.assertGreater(start, 0, f"{needle!r} must live inside a step")
        end = job.find("\n      - ", at)
        return job[start:] if end == -1 else job[start:end]

    def test_glibc_floor_guard_step_cannot_be_neutralized(self):
        # continue-on-error 或步骤级 if: 会让超基线产物绿着通过守护,把启动
        # 崩溃留给 22.04 用户;守护步骤必须无条件执行且直接调用脚本。
        for job in (self.x64_job, self.arm64_job):
            step = self._step_block(job, "check-linux-glibc-floor.sh")
            self.assertIn("run: ./scripts/check-linux-glibc-floor.sh", step)
            self.assertNotIn("continue-on-error", step)
            self.assertNotIn("\n        if:", step)

    def test_linux_deb_unified_name_consistent_across_rename_guard_upload(self):
        # deb 统一名横跨 重命名→守护→上传 手工同步(重命名/守护用 ${VERSION},
        # 上传用 needs 输出表达式);失配只能在 90 分钟构建后的发布 run 末尾
        # 暴露(guard usage exit 2 或 upload 找不到文件),契约层直接钉住。
        for job, arch in ((self.x64_job, "x64"), (self.arm64_job, "arm64")):
            local_name = "pinvou-agent_${VERSION}-linux-" + arch + ".deb"
            upload_name = (
                "pinvou-agent_${{ needs.check-version-bump.outputs.version }}"
                "-linux-" + arch + ".deb"
            )
            self.assertEqual(
                job.count(local_name), 2, f"{arch}: unified deb name must appear in rename and guard"
            )
            self.assertEqual(
                job.count(upload_name), 2, f"{arch}: unified deb name must appear in both upload paths"
            )

    def test_guard_script_defaults_to_2204_glibc_floor(self):
        # The wrapper takes the floor as the optional third argument and
        # delegates to check-linux-elf-policy.sh, which owns the jammy
        # (gcc 12) GLIBCXX_/CXXABI_ floors; the workflow passes no floor,
        # so the defaults must match the Ubuntu 22.04 baseline.
        source = GUARD_SCRIPT.read_text(encoding="utf-8")
        self.assertIn('glibc_floor="${3:-2.35}"', source)
        self.assertIn("check-linux-elf-policy.sh", source)
        policy = ELF_POLICY_SCRIPT.read_text(encoding="utf-8")
        self.assertIn('glibc_floor="${3:-2.35}"', policy)
        self.assertIn('"3.4.30"', policy)  # GLIBCXX_ (jammy gcc 12)
        self.assertIn('"1.3.13"', policy)  # CXXABI_ (jammy gcc 12)

    def test_guard_script_is_valid_bash(self):
        for script in (GUARD_SCRIPT, ELF_POLICY_SCRIPT, ASR_BUILD_SCRIPT, ASR_SETUP_SCRIPT):
            with self.subTest(script=script.name):
                subprocess.run(["bash", "-n", str(script)], check=True)

    def test_sensevoice_runtime_is_rebuilt_from_pinned_source(self):
        source = ASR_BUILD_SCRIPT.read_text(encoding="utf-8")
        self.assertIn("sensevoice-source.env", source)
        self.assertIn("-DBUILD_SHARED_LIBS=OFF", source)
        self.assertIn("-DGGML_NATIVE=OFF", source)
        self.assertIn("check-linux-elf-policy.sh", source)
        # The shallow checkout must be verified to be exactly the pinned
        # commit (the same tripwire as setup-sensevoice.sh), and the
        # mismatch diagnostic must name the commit actually checked out.
        self.assertIn('rev-parse HEAD)" = "$SENSEVOICE_SOURCE_COMMIT"', source)
        self.assertIn("expected $SENSEVOICE_SOURCE_COMMIT, got $(git -C", source)

    def test_sensevoice_setup_script_pins_downloads_and_checksums(self):
        # Pin the supply-chain hardening of the user setup helper: retried
        # and sha256-verified downloads with per-quant/per-arch checksums,
        # the pinned-commit tripwire, loud checksum failures (`--status`
        # alone exits silently) carrying expected/actual values on stderr,
        # a static engine build (only the binary leaves the deleted WORK
        # dir), and same-directory temp file + rename installs for the
        # engine, model and shim with EXIT-trap cleanup.
        source = ASR_SETUP_SCRIPT.read_text(encoding="utf-8")
        self.assertIn("q4_k) MODEL_SHA256=", source)
        self.assertIn("q8_0) MODEL_SHA256=", source)
        self.assertIn("aarch64) CMAKE_SHA256=", source)
        self.assertIn("x86_64) CMAKE_SHA256=", source)
        self.assertEqual(source.count("--retry-all-errors"), 2)
        self.assertEqual(source.count("sha256sum --check --status \\\n"), 2)
        self.assertEqual(source.count("sha256 check failed"), 2)
        self.assertEqual(source.count(", actual $(sha256sum"), 2)
        self.assertIn("pinned commit check failed", source)
        self.assertIn("-DBUILD_SHARED_LIBS=OFF", source)
        # Every error message goes to stderr so piped/CI captures keep it.
        for line in source.splitlines():
            if 'echo "❌' in line:
                with self.subTest(line=line.strip()):
                    self.assertIn(">&2", line)
        self.assertEqual(source.count(".tmp.$$"), 3)
        self.assertEqual(source.count("mv -f"), 3)
        for temp in ("engine_tmp", "model_tmp", "shim_tmp"):
            self.assertIn(f'[ -z "${{{temp}:-}}" ] || rm -f "${temp}"', source)

    @unittest.skipUnless(sys.platform.startswith("linux"), "needs a Linux ELF host")
    def test_elf_policy_accepts_host_arch_and_rejects_mismatch(self):
        machine = platform.machine().lower()
        if machine in ("x86_64", "amd64"):
            expected, wrong = "amd64", "arm64"
        elif machine in ("aarch64", "arm64"):
            expected, wrong = "arm64", "amd64"
        else:
            self.skipTest(f"unsupported test host architecture: {machine}")

        host_elf = str(Path("/bin/true").resolve())
        subprocess.run([str(ELF_POLICY_SCRIPT), host_elf, expected], check=True)
        mismatch = subprocess.run(
            [str(ELF_POLICY_SCRIPT), host_elf, wrong],
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(mismatch.returncode, 0)
        self.assertIn("architecture", mismatch.stderr)
        self.assertIn("!= expected", mismatch.stderr)

    def test_deb_declares_webkit_floor(self):
        # tauri-runtime-wry 启用 webkit2gtk v2_40,运行时需要 WebKitGTK ≥ 2.40;
        # jammy 原始 pocket 只有 2.36.0,显式 depends 让 apt 给出明确错误而非启动崩溃。
        overlay = LINUX_OVERLAY.read_text(encoding="utf-8")
        self.assertIn("libwebkit2gtk-4.1-0 (>= 2.40)", overlay)


if __name__ == "__main__":
    unittest.main()
