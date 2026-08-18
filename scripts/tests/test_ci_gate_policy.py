import unittest
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
        self.assertIn("Merge Queue 完整浏览器 smoke", frontend)
        self.assertIn("select-frontend-smokes.mjs", frontend)
        self.assertIn("npm run test:browser-smoke", frontend)
        self.assertEqual(frontend.count("npm run test:markdown"), 0)

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

    def test_rust_modes_preserve_fast_drafts_and_final_queue_validation(self):
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
        self.assertIn("github.event_name == 'merge_group'", rust_test)
        self.assertIn(
            "needs.changes.outputs.rust_code == 'true'",
            rust_test,
        )
        self.assertIn("github.event.pull_request.draft == false", rust_test)
        self.assertIn(
            "contains(github.event.pull_request.labels.*.name, 'ci:full-rust')",
            rust_test,
        )
        # main push 无条件累计验证(防 concurrency pending 替换盲区),
        # 不与 rust_code 路径条件短路。
        self.assertIn(
            "github.event_name == 'push' ||",
            rust_test,
        )
        self.assertIn("timeout-minutes: 120", rust_test)
        self.assertIn(
            'RUSTFLAGS: "-C link-arg=-fuse-ld=lld '
            '-C link-arg=-Wl,--thinlto-jobs=1"',
            rust_test,
        )

    def test_windows_rust_test_cumulative_main_push_is_path_independent(self):
        # concurrency 可能用后续非 Rust pending 替换含 Rust 变更的 pending;main 的
        # Windows 累计验证不得依赖单次 push 的相邻路径 diff,push 无条件执行。
        windows_rust_test = self.pr_workflow.split(
            "\n  windows-rust-test:", maxsplit=1
        )[1].split("\n  windows-codex-runtime-test:", maxsplit=1)[0]
        self.assertIn(
            "github.event_name == 'push' ||", windows_rust_test
        )
        self.assertIn(
            "needs.changes.outputs.rust_code == 'true' &&\n"
            "            (\n"
            "              github.event_name == 'merge_group' ||",
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


if __name__ == "__main__":
    unittest.main()
