import re
import unittest
from fnmatch import fnmatchcase
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
PR_WORKFLOW = ROOT / ".github/workflows/pr-check.yml"
RELEASE_WORKFLOW = ROOT / ".github/workflows/release-packages.yml"
MAC_WORKFLOW = ROOT / ".github/workflows/mac-build.yml"
REQUIRED_WORKFLOWS = (
    ROOT / ".github/workflows/dco.yml",
    ROOT / ".github/workflows/secret-scan.yml",
    ROOT / ".github/workflows/dependency-review.yml",
    PR_WORKFLOW,
)
PUBLIC_SUBMODULE_VERIFIER = ROOT / "scripts/verify-public-submodule.sh"


def _extract_quoted_paths(block):
    """提取 YAML 块中 `- 'path'` 形式的路径条目(保持文本序)。"""
    paths = []
    for line in block.splitlines():
        stripped = line.strip()
        if stripped.startswith("- '") and stripped.endswith("'"):
            paths.append(stripped[3:-1])
    return paths


def _without_yaml_comments(block):
    return "\n".join(
        line for line in block.splitlines() if not line.lstrip().startswith("#")
    )


def _is_covered_by_trigger(entry, trigger_paths):
    """entry 被 trigger path 覆盖:完全相同,或 trigger 是其上层 `/**` 目录 glob。"""
    for trigger in trigger_paths:
        if entry == trigger:
            return True
        if trigger.endswith("/**") and entry.startswith(trigger[:-2]):
            return True
    return False


def _matches_paths_filter(path, patterns):
    """Model paths-filter v4 some-with-excludes routing for policy examples."""
    included = any(
        fnmatchcase(path, pattern)
        for pattern in patterns
        if not pattern.startswith("!")
    )
    excluded = any(
        fnmatchcase(path, pattern[1:])
        for pattern in patterns
        if pattern.startswith("!")
    )
    return included and not excluded


class CiGatePolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.pr_workflow = PR_WORKFLOW.read_text(encoding="utf-8")
        cls.release_workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")

    def test_full_release_only_runs_for_version_or_manual_trigger(self):
        trigger = self.release_workflow.split("\non:", maxsplit=1)[1].split(
            "\npermissions:", maxsplit=1
        )[0]
        self.assertNotIn("pull_request:", trigger)
        self.assertIn("push:", trigger)
        self.assertIn("paths:\n      - 'VERSION'", trigger)
        self.assertIn("workflow_dispatch:", trigger)
        self.assertIn("cancel-in-progress: false", self.release_workflow)

    def test_release_workflow_does_not_reference_retired_web_template(self):
        for retired_reference in (
            "test:web-template-packaging",
            "prepare:web-template",
            "resources/common/web-template",
            "网页模板发布前冒烟",
        ):
            self.assertNotIn(
                retired_reference,
                self.release_workflow,
                f"发布流程仍引用已退役网页模板: {retired_reference}",
            )

    def test_pull_request_has_lightweight_release_contract_gate(self):
        self.assertIn("release_contract:", self.pr_workflow)
        self.assertIn("  release-contract-test:", self.pr_workflow)
        self.assertIn(
            "needs.changes.outputs.release_contract == 'true'",
            self.pr_workflow,
        )
        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("- release-contract-test", required_gate)
        self.assertIn(
            '"release-contract-test:$RELEASE_CONTRACT_RESULT"',
            required_gate,
        )

    def test_pr_submodule_verifier_strictly_matches_the_published_tag(self):
        verifier = PUBLIC_SUBMODULE_VERIFIER.read_text(encoding="utf-8")
        verifier_gate = self.pr_workflow.split(
            "- name: 公开底座 gitlink 可达性", maxsplit=1
        )[1].split("- name: 初始化公共底座 submodule", maxsplit=1)[0]
        self.assertIn("./scripts/verify-public-submodule.sh", verifier_gate)
        self.assertNotIn("--allow-registered-candidate", verifier_gate)
        self.assertNotIn("LOCAL_SECURITY_HEAD", verifier)
        self.assertIn('[[ "$tag_target" != "$gitlink" ]]', verifier)
        self.assertIn('PINVOU_CODEWHALE_TAG="pinvou-v0.9.5-r14"', verifier)
        self.assertIn("unknown argument", verifier)

    def test_pr_modes_and_stacked_pr_triggers_are_explicit(self):
        trigger = self.pr_workflow.split("\non:", maxsplit=1)[1].split(
            "\npermissions:", maxsplit=1
        )[0]
        pull_request_trigger = trigger.split("\n  pull_request:", maxsplit=1)[
            1
        ].split("\n  merge_group:", maxsplit=1)[0]
        active_pull_request_trigger = "\n".join(
            line
            for line in pull_request_trigger.splitlines()
            if not line.lstrip().startswith("#")
        )
        self.assertNotIn("branches:", active_pull_request_trigger)
        self.assertIn("ready_for_review", pull_request_trigger)
        self.assertIn("converted_to_draft", pull_request_trigger)

        frontend = self.pr_workflow.split(
            "\n  frontend-test:", maxsplit=1
        )[1].split("\n  relay-test:", maxsplit=1)[0]
        self.assertIn("github.event.pull_request.draft == false", frontend)
        self.assertIn("Ready PR 定向浏览器 smoke", frontend)
        self.assertIn("Merge Queue diff-selected browser smoke", frontend)
        self.assertIn("github.event.merge_group.base_sha", frontend)
        self.assertIn("github.event.merge_group.head_sha", frontend)
        self.assertEqual(frontend.count("select-frontend-smokes.mjs"), 2)
        self.assertNotIn("npm run test:browser-smoke", frontend)
        self.assertEqual(frontend.count("npm run test:markdown"), 0)

    def test_static_analysis_gate_configs_route_to_frontend_test(self):
        # The static-analysis gates (oxlint/Biome/knip/jsconfig/audit-compat)
        # only run inside frontend-test, so their config files must be in the
        # frontend path filter; otherwise a config-only PR skips every gate
        # that consumes the file it changed.
        changes = _without_yaml_comments(
            self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
                "\n  fast-gate:", maxsplit=1
            )[0]
        )
        frontend_paths = changes.split(
            "            frontend:", maxsplit=1
        )[1].split("            relay:", maxsplit=1)[0]
        for path in (
            "pinvou3-app/.oxlintrc.json",
            "pinvou3-app/biome.jsonc",
            "pinvou3-app/knip.json",
            "pinvou3-app/jsconfig.json",
            "pinvou3-app/eslint.config.mjs",
            "pinvou3-app/scripts/audit-compat.mjs",
        ):
            self.assertIn(
                f"- '{path}'",
                frontend_paths,
                f"静态门禁配置 {path} 不在 frontend filter 中,config-only PR 会静默跳过 frontend-test",
            )

    def test_merge_queue_uses_real_path_filtering_and_product_gates(self):
        changes = self.pr_workflow.split(
            "\n  changes:", maxsplit=1
        )[1].split("\n  fast-gate:", maxsplit=1)[0]
        self.assertIn("uses: dorny/paths-filter@v4", changes)
        self.assertIn(
            "github.event_name == 'merge_group'",
            changes,
        )
        for output in (
            "rust_code",
            "rust_dependencies",
            "rust_full",
            "knowledge_rust",
            "knowledge_dependencies",
            "release_contract",
            "pet",
            "frontend",
            "relay",
            "acp_runtime",
            "windows_codex",
        ):
            self.assertIn(
                f"{output}: ${{{{ steps.filter.outputs.{output} }}}}",
                changes,
            )
        self.assertIn(
            "- 'pinvou3-app/run-dev.sh'",
            changes,
            "开发启动入口变化必须触发 ACP Runtime 契约检查",
        )

        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertNotIn("完整门禁已在 PR 入队前验证", required_gate)
        self.assertIn("Merge Queue 基础检查失败", required_gate)

    def test_standalone_knowledge_crate_has_its_own_required_gate(self):
        changes = _without_yaml_comments(
            self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
                "\n  fast-gate:", maxsplit=1
            )[0]
        )
        self.assertIn("knowledge_rust:", changes)
        self.assertIn("knowledge_dependencies:", changes)
        knowledge_paths = changes.split(
            "            knowledge_rust:", maxsplit=1
        )[1].split("            knowledge_dependencies:", maxsplit=1)[0]
        self.assertIn("- 'pinvou-knowledge/**/*.rs'", knowledge_paths)
        self.assertIn("- 'pinvou-knowledge/deploy/**'", knowledge_paths)

        knowledge = _without_yaml_comments(
            self.pr_workflow.split("\n  knowledge-rust:", maxsplit=1)[1].split(
                "\n  rust-lint:", maxsplit=1
            )[0]
        )
        self.assertIn("needs.changes.outputs.knowledge_rust == 'true'", knowledge)
        self.assertIn(
            "cargo fmt --manifest-path pinvou-knowledge/Cargo.toml -- --check",
            knowledge,
        )
        self.assertIn(
            "cargo clippy --manifest-path pinvou-knowledge/Cargo.toml --all-targets --all-features --no-deps",
            knowledge,
        )
        self.assertIn(
            "cargo test --manifest-path pinvou-knowledge/Cargo.toml --all-features",
            knowledge,
        )
        self.assertIn("bash -n pinvou-knowledge/deploy/install.sh", knowledge)
        self.assertIn(
            "needs.changes.outputs.knowledge_dependencies == 'true'",
            knowledge,
        )
        self.assertIn("--manifest-path pinvou-knowledge/Cargo.toml", knowledge)

        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("- knowledge-rust", required_gate)
        self.assertIn('"knowledge-rust:$KNOWLEDGE_RUST_RESULT"', required_gate)

    def test_benchmark_jobs_stay_out_of_product_pr_workflow(self):
        self.assertNotIn("\n  benchmark-contract:", self.pr_workflow)
        self.assertNotIn("\n  benchmark-test:", self.pr_workflow)
        changes = self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
            "\n  fast-gate:", maxsplit=1
        )[0]
        self.assertNotIn("benchmark:", changes)
        self.assertNotIn("benchmark_cli:", changes)
        self.assertNotIn("benchmark_headless:", changes)
        self.assertNotIn("benchmark_codewhale:", changes)

    def test_full_rust_filter_fails_closed_with_stable_module_boundaries(self):
        changes = _without_yaml_comments(
            self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
                "\n  fast-gate:", maxsplit=1
            )[0]
        )
        rust_full = changes.split("            rust_full:", maxsplit=1)[1].split(
            "            knowledge_rust:", maxsplit=1
        )[0]
        rust_full_paths = _extract_quoted_paths(rust_full)
        self.assertIn(
            "predicate-quantifier: some-with-excludes",
            changes,
        )
        self.assertIn("pinvou3-app/src-tauri/**/*.rs", rust_full_paths)

        low_risk_boundaries = (
            "!pinvou3-app/src-tauri/src/features/feedback/**",
            "!pinvou3-app/src-tauri/src/features/personas/**",
            "!pinvou3-app/src-tauri/src/features/pet/**",
        )
        for boundary in low_risk_boundaries:
            self.assertIn(boundary, rust_full_paths)

        high_risk_examples = (
            "pinvou3-app/src-tauri/src/app/commands/chat.rs",
            "pinvou3-app/src-tauri/src/app/commands/interaction.rs",
            "pinvou3-app/src-tauri/src/app/commands/settings.rs",
            "pinvou3-app/src-tauri/src/features/knowledge/mod.rs",
            "pinvou3-app/src-tauri/src/features/review/mod.rs",
            "pinvou3-app/src-tauri/src/features/runtime_bundle/platform/mod.rs",
            "pinvou3-app/src-tauri/src/features/voice/voice_asr.rs",
            "pinvou3-app/src-tauri/src/features/updater/mod.rs",
            "pinvou3-app/src-tauri/src/features/future_feature/mod.rs",
            "pinvou3-app/src-tauri/tests/headless_bridge_contract.rs",
        )
        for path in high_risk_examples:
            self.assertTrue(
                _matches_paths_filter(path, rust_full_paths),
                f"unclassified/high-risk Rust path must run full tests: {path}",
            )

        low_risk_examples = (
            "pinvou3-app/src-tauri/src/features/feedback/mod.rs",
            "pinvou3-app/src-tauri/src/features/personas/mod.rs",
            "pinvou3-app/src-tauri/src/features/pet/platform/detach.rs",
        )
        for path in low_risk_examples:
            self.assertFalse(
                _matches_paths_filter(path, rust_full_paths),
                f"documented low-risk leaf should use the fast route: {path}",
            )

        for workflow_path in (
            ".github/workflows/pr-check.yml",
            ".github/workflows/mac-build.yml",
        ):
            self.assertNotIn(
                workflow_path,
                rust_full_paths,
                "workflow policy changes must not link the full application tests",
            )

        literal_feature_files = [
            path
            for path in rust_full_paths
            if "/src/features/" in path
            and path.endswith(".rs")
            and "*" not in path
        ]
        self.assertEqual(
            literal_feature_files,
            [],
            "rust_full must not enumerate internal feature files",
        )

    def test_rust_modes_run_combined_full_regression_only_for_high_risk(self):
        self.assertIn("merge_group:", self.pr_workflow)
        self.assertIn("ci:full-rust", self.pr_workflow)
        rust_lint = self.pr_workflow.split(
            "\n  rust-lint:", maxsplit=1
        )[1].split("\n  rust-test:", maxsplit=1)[0]
        self.assertIn("RUN_HEAVY_RUST_CHECKS", rust_lint)
        self.assertIn("github.event.pull_request.draft == false", rust_lint)
        self.assertIn("needs.changes.outputs.rust_dependencies == 'true'", rust_lint)

        rust_test = self.pr_workflow.split("\n  rust-test:", maxsplit=1)[1].split(
            "\n  windows-rust-test:", maxsplit=1
        )[0]
        self.assertRegex(
            rust_test,
            r"github\.event_name == 'merge_group'\s*&&\s*"
            r"needs\.changes\.outputs\.rust_full == 'true'",
        )
        self.assertIn(
            "needs.changes.outputs.rust_full == 'true'",
            rust_test,
        )
        self.assertIn(
            "needs.changes.outputs.rust_code == 'true'",
            rust_test,
        )
        self.assertIn("github.event.pull_request.draft == false", rust_test)
        self.assertIn(
            "contains(github.event.pull_request.labels.*.name, 'ci:full-rust')",
            rust_test,
        )
        # Main is a cumulative compile verification and must not depend on
        # adjacent diff paths.
        self.assertIn(
            "github.event_name == 'push' ||",
            rust_test,
        )
        self.assertIn(
            "cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib -- --test-threads=1",
            rust_test,
        )
        # push(main) 只编译暖 cache,不执行测试:MQ 已对同一组合树跑过全量测试;
        # push 恢复暖 cache 后跑全量测试曾连续触发 hosted runner 失联(见 workflow 注释)。
        self.assertIn(
            "- name: cargo test --lib（含 strict_mode 回归；真 bge-m3/vLLM 测试已 #[ignore]）\n"
            "        if: ${{ github.event_name != 'push' }}",
            rust_test,
        )
        self.assertIn(
            "- name: cargo test --lib --no-run (push main 仅编译暖 cache)\n"
            "        if: ${{ github.event_name == 'push' }}",
            rust_test,
        )
        # 16GB runner 失联防护:编译与执行拆成独立 step(失联后日志全丢,按 step
        # 状态定位阶段),内存看门狗把 runner 失联转化为带日志的 step 失败,CI 关
        # DWARF 缩小测试二进制降低链接内存峰值。三条腿(push/MQ 编译/MQ 执行)
        # 都必须挂看门狗。
        self.assertIn(
            "- name: cargo test --lib --no-run（编译链接测试二进制）\n"
            "        if: ${{ github.event_name != 'push' }}",
            rust_test,
        )
        self.assertEqual(rust_test.count("bash scripts/ci-memguard.sh &"), 3)
        self.assertIn('CARGO_PROFILE_DEV_DEBUG: "0"', rust_test)
        self.assertIn("timeout-minutes: 120", rust_test)
        self.assertIn(
            'RUSTFLAGS: "-C link-arg=-fuse-ld=lld '
            '-C link-arg=-Wl,--thinlto-jobs=1 '
            '-C link-arg=-Wl,--threads=1"',
            rust_test,
        )

    def test_windows_rust_test_cumulative_main_push_is_path_independent(self):
        # Main's Windows regression must remain independent of adjacent diff paths.
        windows_rust_test = self.pr_workflow.split(
            "\n  windows-rust-test:", maxsplit=1
        )[1].split("\n  windows-codex-runtime-test:", maxsplit=1)[0]
        self.assertIn(
            "github.event_name == 'push' ||", windows_rust_test
        )
        self.assertIn(
            "needs.changes.outputs.rust_full == 'true'",
            windows_rust_test,
        )
        self.assertNotIn("github.event_name == 'merge_group'", windows_rust_test)
        self.assertIn(
            "contains(github.event.pull_request.labels.*.name, 'ci:full-rust')",
            windows_rust_test,
        )
        self.assertIn(
            "github.event.pull_request.draft == false", windows_rust_test
        )

        windows_rust_test = _without_yaml_comments(
            self.pr_workflow.split("\n  windows-rust-test:", maxsplit=1)[1].split(
                "\n  windows-codex-runtime-test:", maxsplit=1
            )[0]
        )
        self.assertIn(
            "defaults:\n      run:\n        shell: bash",
            windows_rust_test,
        )
        self.assertIn(
            "- name: Windows 原子替换状态机回归\n"
            "        shell: bash\n"
            "        run: |",
            windows_rust_test,
        )
        self.assertIn(
            "- name: Windows 测试 exe 嵌入 Common-Controls v6 清单\n"
            "        shell: pwsh\n"
            "        run: |",
            windows_rust_test,
        )
        self.assertIn(
            '"-outputresource:$($testExe.FullName);#1"',
            windows_rust_test,
        )
        self.assertIn(
            '"PINVOU3_TEST_EXE=$testExe" | Out-File',
            windows_rust_test,
        )
        self.assertIn(
            'test_exe="$(cygpath -u "$PINVOU3_TEST_EXE")"',
            windows_rust_test,
        )
        regression = windows_rust_test.split(
            "- name: Windows 原子替换状态机回归", maxsplit=1
        )[1]
        self.assertIn('"$test_exe" "$filter" --test-threads=1', regression)
        self.assertNotIn("cargo test", regression)

        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("- windows-rust-test", required_gate)
        self.assertIn("WINDOWS_RUST_RESULT", required_gate)
        self.assertIn('"windows-rust-test:$WINDOWS_RUST_RESULT"', required_gate)

    def test_windows_browser_wrapper_lifecycle_runs_in_required_native_job(self):
        changes = self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
            "\n  fast-gate:", maxsplit=1
        )[0]
        windows_codex_filter = changes.split("windows_codex:", maxsplit=1)[1]
        self.assertIn(
            "resources/common/bundle/mcp-servers/browser-*",
            windows_codex_filter,
        )
        self.assertIn(
            "browser_wrapper_windows_lifecycle.test.mjs",
            windows_codex_filter,
        )
        self.assertIn("browser_wrapper_lazy.test.mjs", windows_codex_filter)

        windows_job = self.pr_workflow.split(
            "\n  windows-codex-runtime-test:", maxsplit=1
        )[1].split("\n  macos-codex-runtime-test:", maxsplit=1)[0]
        self.assertIn("needs.changes.outputs.windows_codex == 'true'", windows_job)
        self.assertIn("runs-on: windows-latest", windows_job)
        self.assertIn("Windows browser wrapper lifecycle regression", windows_job)
        self.assertIn(
            "node --test pinvou3-app/tests/browser_wrapper_windows_lifecycle.test.mjs",
            windows_job,
        )
        self.assertIn(
            "node pinvou3-app/tests/browser_wrapper_lazy.test.mjs",
            windows_job,
        )
        self.assertIn('PINVOU3_TEST_BROWSER_NO_HOST: "1"', windows_job)

        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("- windows-codex-runtime-test", required_gate)
        self.assertIn("WINDOWS_CODEX_RESULT", required_gate)

    def test_windows_python_dependency_contract_runs_in_required_native_job(self):
        changes = self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
            "\n  fast-gate:", maxsplit=1
        )[0]
        # Anchor on the 12-space-indented dorny filter key (not the 8-space outputs
        # mapping) and capture only the entry lines of that one filter group, so
        # moving the ps1 route into another filter fails this assertion.
        windows_codex_filter = re.search(
            r"\n            windows_codex:\n((?:              .*(?:\n|$))+)",
            changes,
        ).group(1)
        self.assertIn(
            "windows_python_dependency_contract.ps1",
            windows_codex_filter,
        )

        windows_job = self.pr_workflow.split(
            "\n  windows-codex-runtime-test:", maxsplit=1
        )[1].split("\n  macos-codex-runtime-test:", maxsplit=1)[0]
        self.assertIn(
            "npm --prefix pinvou3-app run test:windows-runtime",
            windows_job,
        )

        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("- windows-codex-runtime-test", required_gate)

    def test_release_contract_runs_for_ready_pr_queue_and_main(self):
        changes = _without_yaml_comments(
            self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
                "\n  fast-gate:", maxsplit=1
            )[0]
        )
        release_contract_paths = changes.split(
            "            release_contract:", maxsplit=1
        )[1].split("            l1:", maxsplit=1)[0]
        self.assertIn(
            "- 'pinvou3-app/src-tauri/resources/**'",
            release_contract_paths,
        )
        self.assertIn(
            "- 'pinvou3-app/tests/knowledge_host_packaging.test.mjs'",
            release_contract_paths,
        )

        release_contract = _without_yaml_comments(
            self.pr_workflow.split("\n  release-contract-test:", maxsplit=1)[1].split(
                "\n  knowledge-rust:", maxsplit=1
            )[0]
        )
        self.assertIn(
            "needs.changes.outputs.release_contract == 'true'",
            release_contract,
        )
        self.assertNotIn("github.event_name != 'merge_group'", release_contract)
        self.assertIn("github.event.pull_request.draft == false", release_contract)
        self.assertIn(
            "npm --prefix pinvou3-app run test:knowledge-host-packaging",
            release_contract,
        )

    def test_main_cache_writer_is_not_cancelled(self):
        concurrency = self.pr_workflow.split(
            "\nconcurrency:", maxsplit=1
        )[1].split("\njobs:", maxsplit=1)[0]
        self.assertIn(
            "cancel-in-progress: ${{ github.event_name == 'pull_request' }}",
            concurrency,
        )

    def test_all_required_workflows_report_on_merge_group(self):
        for workflow_path in REQUIRED_WORKFLOWS:
            workflow = workflow_path.read_text(encoding="utf-8")
            trigger = workflow.split("\non:", maxsplit=1)[1].split(
                "\npermissions:", maxsplit=1
            )[0]
            self.assertIn(
                "merge_group:",
                trigger,
                f"{workflow_path.name} 缺少 Merge Queue 触发",
            )

        dependency_review = (
            ROOT / ".github/workflows/dependency-review.yml"
        ).read_text(encoding="utf-8")
        secret_scan = (
            ROOT / ".github/workflows/secret-scan.yml"
        ).read_text(encoding="utf-8")
        dco = (ROOT / ".github/workflows/dco.yml").read_text(encoding="utf-8")
        self.assertIn("依赖审查已在各 PR 入队前验证", dependency_review)
        self.assertIn("密钥扫描已在各 PR 入队前验证", secret_scan)
        self.assertIn("DCO 已在各 PR 入队前验证", dco)
        self.assertNotIn("完整门禁已在 PR 入队前验证", self.pr_workflow)
        self.assertNotIn("github.event.merge_group.base_sha", dependency_review)
        self.assertNotIn("github.event.merge_group.head_sha", dependency_review)

    def test_mac_bundle_chain_paths_are_reachable_by_workflow_trigger(self):
        # mac-build 的 bundle_chain filter 决定何时追加 universal bundle smoke。
        # filter 只在该 workflow 被触发后才有机会匹配,因此 bundle_chain 的每条
        # 路径都必须被 on.push.paths 覆盖;不被覆盖的条目永远不会命中(死条目),
        # 会误导读者以为该路径变更会跑 smoke(例如 VERSION:VERSION-only push
        # 不触发 mac-build,版本同步提交经 tauri.conf.json/package.json 进入)。
        mac_workflow = MAC_WORKFLOW.read_text(encoding="utf-8")
        trigger_block = mac_workflow.split("\non:", maxsplit=1)[1].split(
            "\npermissions:", maxsplit=1
        )[0]
        trigger_paths = _extract_quoted_paths(trigger_block)
        self.assertTrue(trigger_paths, "mac-build on.push.paths 解析为空")

        bundle_chain_block = mac_workflow.split(
            "\n            bundle_chain:", maxsplit=1
        )[1].split("\n\n", maxsplit=1)[0]
        bundle_chain_paths = _extract_quoted_paths(bundle_chain_block)
        self.assertTrue(bundle_chain_paths, "mac-build bundle_chain 解析为空")

        for entry in bundle_chain_paths:
            self.assertTrue(
                _is_covered_by_trigger(entry, trigger_paths),
                f"bundle_chain 路径不被 on.push.paths 覆盖(死条目): {entry}",
            )

    def test_pure_frontend_changes_do_not_trigger_macos_rust_build(self):
        # Pure-frontend paths must not enter the mac-build trigger set (avoids
        # needless native builds); but package.json/package-lock.json changes
        # must trigger (the lockfile affects the build).
        # Folded in from scripts/tests/test_ci_trigger_routing_policy.py to
        # remove the duplicated parsing of the same mac-build.yml trigger
        # block across two files.
        trigger = MAC_WORKFLOW.read_text(encoding="utf-8").split("\non:", maxsplit=1)[
            1
        ].split("\npermissions:", maxsplit=1)[0]

        self.assertIn("'pinvou3-app/src-tauri/**'", trigger)
        self.assertNotIn("'pinvou3-app/src/**'", trigger)
        self.assertIn("'pinvou3-app/package.json'", trigger)
        self.assertIn("'pinvou3-app/package-lock.json'", trigger)

    def test_wrapper_smoke_routes_merge_groups_before_platform_matrix(self):
        # rustc-wrapper-smoke must first pass the paths-filter gate before
        # entering the three-platform matrix, so wrapper-unrelated PRs do not
        # run the full three-platform smoke.
        # Folded in from scripts/tests/test_ci_trigger_routing_policy.py.
        workflow = (
            ROOT / ".github/workflows/rustc-wrapper-smoke.yml"
        ).read_text(encoding="utf-8")
        trigger = workflow.split("\non:", maxsplit=1)[1].split(
            "\npermissions:", maxsplit=1
        )[0]
        pull_request = trigger.split("\n  pull_request:", maxsplit=1)[1].split(
            "\n  merge_group:", maxsplit=1
        )[0]
        push = trigger.split("\n  push:", maxsplit=1)[1]
        changes = workflow.split("\n  changes:", maxsplit=1)[1].split(
            "\n  smoke:", maxsplit=1
        )[0]
        smoke = workflow.split("\n  smoke:", maxsplit=1)[1]

        self.assertIn("merge_group:", trigger)
        self.assertIn("push:", trigger)
        self.assertIn("paths:", trigger)
        workflow_path = "'.github/workflows/rustc-wrapper-smoke.yml'"
        self.assertIn(workflow_path, pull_request)
        self.assertIn(workflow_path, push)
        self.assertIn(workflow_path, changes)
        self.assertIn("uses: dorny/paths-filter@v4", changes)
        self.assertIn("wrapper: ${{ steps.filter.outputs.wrapper }}", changes)
        self.assertIn("needs: changes", smoke)
        self.assertIn("if: ${{ needs.changes.outputs.wrapper == 'true' }}", smoke)
        self.assertIn("os: [macos-15, ubuntu-latest, windows-latest]", smoke)


if __name__ == "__main__":
    unittest.main()
