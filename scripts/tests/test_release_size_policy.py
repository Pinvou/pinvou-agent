import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release-packages.yml"
MAC_BUILD_WORKFLOW = ROOT / ".github/workflows/mac-build.yml"
APP_CARGO_TOML = ROOT / "pinvou3-app/src-tauri/Cargo.toml"
KNOWLEDGE_CARGO_TOML = ROOT / "pinvou-knowledge/Cargo.toml"
REPACK_SCRIPT = ROOT / "scripts/repack-deb-xz.sh"
OTA_SCRIPT = ROOT / "scripts/build-windows-ota.ps1"
RELEASE_DEB_SCRIPT = ROOT / "scripts/release-deb.sh"
RELEASE_MACOS_SCRIPT = ROOT / "scripts/release-macos.sh"


def cargo_profile(text: str, profile_header: str, next_header: str) -> str:
    """Extract the text between two TOML table headers (empty if absent)."""
    if profile_header not in text:
        return ""
    return text.split(profile_header, maxsplit=1)[1].split(next_header, maxsplit=1)[0]


class ReleaseSizePolicyTests(unittest.TestCase):
    """发布产物体积/调试信息策略钉扎(docs/binary-size-optimization.md)。

    防止后续改动悄悄丢掉这些 flag 或回退 profile:
    - Linux 发布构建必须带 lld + --icf=safe + remap-path-prefix;
    - deb 必须重打包为 xz -9,且先重打包再算 sha256;
    - macOS dmg 必须做 ULMO 压缩升级;
    - macOS 27+ strip=none 规避(1.98.0 已含对齐修复)不得复活;
    - release profile 必须零调试信息(strip=symbols + debug=false)。
    """

    def setUp(self):
        self.release_workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        self.mac_build_workflow = MAC_BUILD_WORKFLOW.read_text(encoding="utf-8")
        self.app_cargo = APP_CARGO_TOML.read_text(encoding="utf-8")
        self.knowledge_cargo = KNOWLEDGE_CARGO_TOML.read_text(encoding="utf-8")
        self.repack = REPACK_SCRIPT.read_text(encoding="utf-8")
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
            ("\n  build-linux-arm64:", "\n  build-windows-x64:"),
        ):
            job_env = self.job(start, end).split("\n    steps:", maxsplit=1)[0]
            # lld:thin LTO 的 link 阶段需要支持 LLVM bitcode 的链接器。
            self.assertIn("-C link-arg=-fuse-ld=lld", job_env, start)
            # ICF 相同代码折叠(safe 档:只折叠地址确定未被取用的函数)。
            self.assertIn("-C link-arg=-Wl,--icf=safe", job_env, start)
            # 产物内嵌构建机绝对路径(panic location 等)归一为 /。
            self.assertIn(
                "-C remap-path-prefix=${{ github.workspace }}=/", job_env, start
            )

    def test_linux_jobs_repack_deb_as_xz_before_sha256(self):
        for start, end in (
            ("\n  build-linux-x64:", "\n  build-linux-arm64:"),
            ("\n  build-linux-arm64:", "\n  build-windows-x64:"),
        ):
            job_text = self.job(start, end)
            repack_call = "scripts/repack-deb-xz.sh"
            self.assertIn(repack_call, job_text, start)
            # 重打包必须发生在 sha256 计算之前,否则校验和对应的是旧包。
            self.assertLess(
                job_text.index(repack_call),
                job_text.index("sha256sum"),
                start,
            )
            # glibc 下限守护仍运行在重打包后的最终产物上(dpkg-deb -x 原生支持 xz)。
            self.assertIn("check-linux-glibc-floor.sh", job_text, start)

    def test_repack_script_uses_max_compression(self):
        # 只检查可执行行(注释里的反例字样不算数)。
        code_only = "\n".join(
            line
            for line in self.repack.splitlines()
            if not line.lstrip().startswith("#")
        )
        # --root-owner-group:非 root 构建机产出仍归 root:root。
        self.assertIn("--root-owner-group", code_only)
        # data.tar xz -9(control.tar 随 dpkg ≥1.19 默认 uniform compression 同为 xz)。
        self.assertIn("-Zxz", code_only)
        self.assertIn("-z9", code_only)

    def test_macos_job_upgrades_dmg_to_ulmo(self):
        macos_job = self.job(
            "\n  build-macos-universal:", "\n      - name: 上传 dmg artifact"
        )
        self.assertIn("-format ULMO", macos_job)
        # 格式探测:已是 ULMO 的产物不重复转换(降级路径产出的 dmg 亦覆盖)。
        self.assertIn("hdiutil imageinfo -format", macos_job)

    def test_macos_strip_workaround_stays_removed(self):
        # 1.98.0 已含 Mach-O __LINKEDIT 对齐修复(rust#157750/#158410);
        # strip=none 注入会让 macOS 产物保留符号表/调试映射,不得复活。
        self.assertNotIn("CARGO_PROFILE_RELEASE_STRIP", self.release_workflow)
        self.assertNotIn("CARGO_PROFILE_RELEASE_FAST_STRIP", self.mac_build_workflow)
        self.assertNotIn("-C strip=none", self.release_workflow)
        self.assertNotIn("-C strip=none", self.mac_build_workflow)
        # macOS 发布 job 的路径归一 flag(与 Linux 同理由)。
        macos_env = self.job(
            "\n  build-macos-universal:", "\n    steps:"
        ).split("\n    steps:", maxsplit=1)[0]
        self.assertIn(
            'RUSTFLAGS: "-C remap-path-prefix=${{ github.workspace }}=/"',
            macos_env,
        )

    def test_app_release_profile_ships_no_debug_info(self):
        release = cargo_profile(
            self.app_cargo, "[profile.release]", "[profile.release-fast]"
        )
        # strip=symbols:ELF 链接期 --strip-all(MSVC 上为 no-op,PDB 从不进安装包)。
        self.assertIn('strip = "symbols"', release)
        # debug=false:产物零调试信息;行号表只保留在 release-fast 冒烟 profile。
        self.assertIn("debug = false", release)
        release_fast = self.app_cargo.split("[profile.release-fast]", maxsplit=1)[1]
        self.assertIn('debug = "line-tables-only"', release_fast)
        # panic 策略由 CodeWhale catch_unwind 设计钉死,顺带守护。
        self.assertIn('panic = "unwind"', release)

    def test_knowledge_server_release_profile_is_size_optimized(self):
        # pinvou-knowledge-server 是 deb 内三个 ELF 之一,独立构建
        # (knowledge-host.js),需与主应用对齐 thin LTO + strip。
        profile = self.knowledge_cargo.split("[profile.release]", maxsplit=1)[1]
        self.assertIn('lto = "thin"', profile)
        self.assertIn("codegen-units = 1", profile)
        self.assertIn('strip = "symbols"', profile)

    def test_windows_ota_uses_smallest_size_compression(self):
        self.assertNotIn("CompressionLevel]::Optimal", self.ota)
        self.assertIn("CompressionLevel]::SmallestSize", self.ota)

    def test_manual_release_scripts_share_the_same_pipeline(self):
        self.assertIn("scripts/repack-deb-xz.sh", self.release_deb)
        self.assertIn("-format ULMO", self.release_macos)
        self.assertIn("hdiutil imageinfo -format", self.release_macos)


if __name__ == "__main__":
    unittest.main()
