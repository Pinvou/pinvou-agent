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
    """Pins the release size / debug-info policy (docs/binary-size-optimization.md).

    Prevents later changes from quietly dropping these flags or reverting the
    profiles:
    - Linux release builds must carry lld + --icf=safe + remap-path-prefix;
    - the deb must be repacked as xz -9, with repacking before sha256;
    - the macOS dmg must get the ULMO compression upgrade;
    - the macOS 27+ strip=none workaround (obsolete since rustc 1.98.0) must
      not come back;
    - the release profile must ship zero debug info (strip=symbols + debug=false).
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
            # lld: thin LTO is executed by rustc itself, but BFD link OOMs on
            # binaries this size and BFD has no ICF (see the job env comment).
            self.assertIn("-C link-arg=-fuse-ld=lld", job_env, start)
            # ICF identical code folding (safe tier: only folds functions
            # whose addresses are provably never taken).
            self.assertIn("-C link-arg=-Wl,--icf=safe", job_env, start)
            # Normalizes build-machine absolute paths embedded in the
            # artifacts (panic locations, etc.).
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
        self.assertIn("-format ULMO", macos_job)
        # Format detection: artifacts already in ULMO are not re-converted
        # (also covers the output of the degradation path).
        self.assertIn("hdiutil imageinfo -format", macos_job)

    def test_macos_strip_workaround_stays_removed(self):
        # rustc 1.98.0 contains the Mach-O __LINKEDIT alignment fix
        # (rust#157750/#158410); a strip=none injection would keep symbol
        # tables / debug maps in macOS artifacts and must not come back.
        self.assertNotIn("CARGO_PROFILE_RELEASE_STRIP", self.release_workflow)
        self.assertNotIn("CARGO_PROFILE_RELEASE_FAST_STRIP", self.mac_build_workflow)
        self.assertNotIn("-C strip=none", self.release_workflow)
        self.assertNotIn("-C strip=none", self.mac_build_workflow)
        # Path normalization flag of the macOS release job (same reason as Linux).
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
        # SmallestSize does not exist on .NET Framework (Windows PowerShell
        # 5.1); the script must declare the pwsh requirement so it fails
        # loudly up front instead of dying mid-run on the default shell.
        self.assertIn("#requires -Version 6", self.ota)

    def test_manual_release_scripts_share_the_same_pipeline(self):
        self.assertIn("scripts/repack-deb-xz.sh", self.release_deb)
        self.assertIn("-format ULMO", self.release_macos)
        self.assertIn("hdiutil imageinfo -format", self.release_macos)


if __name__ == "__main__":
    unittest.main()
