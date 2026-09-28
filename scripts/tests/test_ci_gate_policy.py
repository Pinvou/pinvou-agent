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
    """entry 被 trigger path 覆盖:完全相同,或 trigger 是其上层 `/**` 目录 glob。

    与 dorny/paths-filter 的 some-with-excludes 语义对齐:至少一条正向
    pattern 命中,且没有任何 `!` 排除条目命中(排除优先于命中)。忽略
    排除条目会让路由锁在 filter 组新增排除时仍虚报覆盖(fail-open)。
    """

    def _matches(pattern):
        return entry == pattern or (
            pattern.endswith("/**") and entry.startswith(pattern[:-2])
        )

    if not any(_matches(p) for p in trigger_paths if not p.startswith("!")):
        return False
    excludes = [p[1:] for p in trigger_paths if p.startswith("!")]
    return not any(_matches(p) for p in excludes)


def _extract_contract_read_targets(contract_test):
    """从 multiagent_plan_normalize.test.mjs 源码派生全部 src-tauri 读取目标。

    `read('src-tauri', ...)` 产出单文件目标,`path.join(here, '..', 'src-tauri',
    ...)` 产出 readdirSync 整目录拼接的 `/**` 目标。单引号与双引号形式都被
    接受:此前正则只匹配单引号,双引号的 `read("src-tauri", ...)` 会整体绕过
    路由锁(fail-open,经变异验证)。
    """
    targets = []
    for args in re.findall(
        r"read\(['\"]src-tauri['\"],\s*([^)]*)\)", contract_test
    ):
        parts = re.findall(r"['\"]([^'\"]*)['\"]", args)
        if not parts:
            raise AssertionError(f"无法解析的契约测试 read 目标: {args}")
        targets.append("pinvou3-app/src-tauri/" + "/".join(parts))
    for args in re.findall(
        r"path\.join\(here, ['\"]\.\.['\"], ['\"]src-tauri['\"],\s*([^)]*)\)",
        contract_test,
    ):
        parts = re.findall(r"['\"]([^'\"]*)['\"]", args)
        targets.append("pinvou3-app/src-tauri/" + "/".join(parts) + "/**")
    return targets


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
        self.assertIn('PINVOU_CODEWHALE_BRANCH="pinvou3-clean"', verifier)
        self.assertIn('PINVOU_CODEWHALE_TAG="pinvou-v0.9.12-r3"', verifier)
        self.assertIn('[[ "$branch_target" != "$gitlink" ]]', verifier)
        self.assertIn('[[ "$branch_target" != "$tag_target" ]]', verifier)
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

    def test_cross_language_contract_rust_sources_route_to_frontend_test(self):
        # multiagent_plan_normalize.test.mjs reads the Rust sources below for
        # cross-language contract pins (swarm contract text, same-snapshot
        # invariant, edit-resend replay, roster caps). If they are absent from
        # the frontend path filter, a Rust-only PR silently skips that node
        # gate — the same structural blind spot the static-analysis configs
        # above guard against.
        changes = _without_yaml_comments(
            self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
                "\n  fast-gate:", maxsplit=1
            )[0]
        )
        frontend_paths = changes.split(
            "            frontend:", maxsplit=1
        )[1].split("            relay:", maxsplit=1)[0]
        for path in (
            "pinvou3-app/src-tauri/src/features/assistant/**",
            "pinvou3-app/src-tauri/src/features/multiagent/transcripts.rs",
            "pinvou3-app/src-tauri/src/features/remote_control/manager/**",
            "pinvou3-app/src-tauri/src/features/sessions/**",
            "pinvou3-app/src-tauri/src/features/personas/mod.rs",
            "pinvou3-app/src-tauri/src/features/files/file_ingest.rs",
            "pinvou3-app/src-tauri/src/app/commands/multiagent.rs",
            "pinvou3-app/src-tauri/src/app/commands/chat.rs",
            "pinvou3-app/src-tauri/src/app/commands/memory.rs",
            "pinvou3-app/src-tauri/src/app/commands/interaction.rs",
            "pinvou3-app/src-tauri/src/app/commands/remote_control.rs",
            "pinvou3-app/src-tauri/src/app/commands/personas.rs",
            "pinvou3-app/src-tauri/src/lib.rs",
        ):
            self.assertIn(
                f"- '{path}'",
                frontend_paths,
                f"跨语言契约测试读取的 Rust 源 {path} 不在 frontend filter 中,Rust-only PR 会静默跳过该 node 门禁",
            )

    def test_cross_language_contract_reads_fully_routed(self):
        # 上面的静态清单会随 .mjs 演进漂移:这里从
        # multiagent_plan_normalize.test.mjs 本身派生它读取的全部 src-tauri
        # 目标(单文件 read(...) 与 readdirSync 整目录拼接,单/双引号形式
        # 均归一化匹配),逐一断言 frontend filter 覆盖。给契约测试新增
        # Rust read 而不路由、或把已路由文件挪走,都会在这里失败(本套件
        # 在 fast-gate 每个 PR 必跑)。
        changes = _without_yaml_comments(
            self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
                "\n  fast-gate:", maxsplit=1
            )[0]
        )
        frontend_entries = _extract_quoted_paths(
            changes.split("            frontend:", maxsplit=1)[1].split(
                "            relay:", maxsplit=1
            )[0]
        )
        contract_test = (
            ROOT / "pinvou3-app/tests/multiagent_plan_normalize.test.mjs"
        ).read_text(encoding="utf-8")

        targets = _extract_contract_read_targets(contract_test)

        self.assertTrue(
            targets,
            "未能从 multiagent_plan_normalize.test.mjs 解析出 src-tauri 读取目标",
        )
        for target in targets:
            self.assertTrue(
                _is_covered_by_trigger(target, frontend_entries),
                f"跨语言契约测试读取的 {target} 未被 frontend filter 覆盖,"
                "Rust-only 改动会静默跳过该 node 门禁",
            )

    def test_contract_target_derivation_covers_double_quoted_reads(self):
        # 回归锁:派生正则此前只匹配单引号形式,双引号的
        # `read("src-tauri", ...)` 会整体绕过上面的路由锁(经变异验证)。
        # 对派生函数喂最小 fixture:双引号的单文件与整目录目标都必须被
        # 解析出来,且不被缺少该条目的 frontend filter 覆盖——即未来出现
        # 未路由的双引号读取时,路由锁必定失败而不是静默通过。
        fixture = (
            "const direct = read(\"src-tauri\", \"src\", \"features\", "
            "\"future_feature\", \"mod.rs\");\n"
            "const dir = path.join(here, \"..\", \"src-tauri\", \"src\", "
            "\"features\", \"future_module\");\n"
        )
        targets = _extract_contract_read_targets(fixture)
        self.assertIn(
            "pinvou3-app/src-tauri/src/features/future_feature/mod.rs",
            targets,
            "双引号 read 目标必须被派生出来(单引号正则 fail-open 回归)",
        )
        self.assertIn(
            "pinvou3-app/src-tauri/src/features/future_module/**",
            targets,
            "双引号 path.join 整目录目标必须被派生出来",
        )
        unrouted_filter = ["pinvou3-app/src-tauri/src/lib.rs"]
        for target in targets:
            self.assertFalse(
                _is_covered_by_trigger(target, unrouted_filter),
                f"未路由的双引号读取 {target} 不应被无关 filter 覆盖",
            )

    def test_trigger_coverage_respects_exclusions(self):
        # dorny/paths-filter(some-with-excludes)的语义是"至少一条正向
        # pattern 命中且没有任何 `!` 排除条目命中"。此前
        # _is_covered_by_trigger 忽略排除条目:frontend 组若新增排除,
        # 路由锁会虚报覆盖而 CI 实际不触发该门禁。
        positive = ["pinvou3-app/src-tauri/src/features/**"]
        self.assertTrue(
            _is_covered_by_trigger(
                "pinvou3-app/src-tauri/src/features/assistant/engine.rs",
                positive,
            )
        )
        with_exclude = positive + [
            "!pinvou3-app/src-tauri/src/features/assistant/**"
        ]
        self.assertFalse(
            _is_covered_by_trigger(
                "pinvou3-app/src-tauri/src/features/assistant/engine.rs",
                with_exclude,
            ),
            "正向命中但被 `!` 排除条目命中的路径不得视为覆盖",
        )
        # 排除只作用于其命中范围,同组其余路径仍被覆盖。
        self.assertTrue(
            _is_covered_by_trigger(
                "pinvou3-app/src-tauri/src/features/sessions/mode_state.rs",
                with_exclude,
            )
        )
        # 精确条目形式的排除同样生效。
        exact_exclude = [
            "pinvou3-app/src-tauri/src/lib.rs",
            "!pinvou3-app/src-tauri/src/lib.rs",
        ]
        self.assertFalse(
            _is_covered_by_trigger(
                "pinvou3-app/src-tauri/src/lib.rs", exact_exclude
            )
        )
        # 只有排除条目、没有任何正向 pattern 时不得视为覆盖。
        self.assertFalse(
            _is_covered_by_trigger(
                "pinvou3-app/src-tauri/src/lib.rs",
                ["!pinvou3-app/src-tauri/src/lib.rs"],
            )
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
            "cli_rust",
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
        # The -D-warnings hard gate must stay in this job (single shared
        # cache); rust-lint must not compile the workspace a second time.
        self.assertIn(
            "cargo clippy --manifest-path pinvou-knowledge/Cargo.toml --lib --bins --no-deps --features server -- -D warnings",
            knowledge,
        )
        self.assertNotIn("cargo clippy pinvou-knowledge", self.pr_workflow)
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

    def test_fast_gate_actionlint_is_pinned_and_checksum_verified(self):
        fast_gate = self.pr_workflow.split("\n  fast-gate:", maxsplit=1)[1].split(
            "\n  frontend-test:", maxsplit=1
        )[0]
        step = fast_gate.split(
            "- name: workflow lint (actionlint)", maxsplit=1
        )[1].split("\n      - name:", maxsplit=1)[0]
        # The release artifact is fetched from the pinned tag and verified
        # against the release checksums.txt digest. Executing an installer
        # fetched from a mutable ref (e.g. raw.githubusercontent .../main/)
        # would let third-party code drift under a green gate.
        self.assertIn(
            "https://github.com/rhysd/actionlint/releases/download/v1.7.12/actionlint_1.7.12_linux_amd64.tar.gz",
            step,
        )
        self.assertIn(
            "8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8  actionlint.tar.gz",
            step,
        )
        self.assertIn("| sha256sum --check -", step)
        self.assertNotIn("download-actionlint.bash", step)
    def test_cli_crate_has_its_own_required_gate(self):
        changes = _without_yaml_comments(
            self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
                "\n  fast-gate:", maxsplit=1
            )[0]
        )
        self.assertIn("cli_rust:", changes)
        cli_paths = changes.split("            cli_rust:", maxsplit=1)[1].split(
            "            knowledge_rust:", maxsplit=1
        )[0]
        self.assertIn(
            "- 'pinvou-cli/**/*.rs'",
            cli_paths,
            "cli_rust must match the real crate directory (pinvou-cli)",
        )
        self.assertIn("- 'pinvou-cli/**/Cargo.toml'", cli_paths)
        self.assertIn("- 'CodeWhale'", cli_paths)
        # The CLI path-depends on the app crate, so the leaf features that
        # rust_full exempts still gate through the CLI suite (a change confined
        # to features/feedback or features/personas would otherwise run NO rust
        # gate at all).
        self.assertIn("- 'pinvou3-app/src-tauri/src/features/feedback/**'", cli_paths)
        self.assertIn("- 'pinvou3-app/src-tauri/src/features/personas/**'", cli_paths)

        cli_test = _without_yaml_comments(
            self.pr_workflow.split("\n  cli-test:", maxsplit=1)[1].split(
                "\n  windows-rust-test:", maxsplit=1
            )[0]
        )
        self.assertIn("needs.changes.outputs.cli_rust == 'true'", cli_test)
        self.assertIn(
            "github.event.pull_request.draft == false",
            cli_test,
            "draft PRs must skip the heavy CLI leg like the other rust jobs",
        )
        self.assertIn("- name: Set up zram and swap", cli_test)
        self.assertIn("scripts/ci-memory-setup.sh", cli_test)
        self.assertIn(
            "cargo fmt --all --check --manifest-path pinvou-cli/Cargo.toml",
            cli_test,
            "pinvou-cli is a virtual workspace: plain --manifest-path fmt fails "
            "with 'Failed to find targets', --all is required",
        )
        self.assertIn(
            "cargo test --manifest-path pinvou-cli/Cargo.toml --locked --no-fail-fast",
            cli_test,
        )
        self.assertIn(
            "cargo test -p adapter-gaia --features test-support --locked --no-fail-fast",
            cli_test,
            "dataset_contract is required-features-gated and silently skipped by "
            "the workspace run; the gaia timeout pins live there",
        )
        self.assertIn("cache-targets: false", cli_test)

        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("- cli-test", required_gate)
        self.assertIn('"cli-test:$CLI_TEST_RESULT"', required_gate)

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
            "pinvou3-app/src-tauri/src/features/assistant/product_runtime/headless_bridge_contract_tests.rs",
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
        self.assertIn("timeout-minutes: 30", rust_lint)
        self.assertIn("RUN_HEAVY_RUST_CHECKS", rust_lint)
        self.assertIn("github.event.pull_request.draft == false", rust_lint)
        self.assertIn("needs.changes.outputs.rust_dependencies == 'true'", rust_lint)
        self.assertNotIn("headless_bridge_contract_tests", rust_lint)

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
            "cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib "
            "--features benchmark-hooks --locked -- --test-threads=1",
            rust_test,
        )
        self.assertIn(
            "cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib "
            "--features benchmark-hooks --locked --no-run",
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
        # 状态定位阶段),CI 关 DWARF 缩小测试二进制降低链接内存峰值。有效内存
        # 由 job 开头的 zram/swap 扩容 step(scripts/ci-memory-setup.sh)提供;
        # 看门狗已删除,不再抢先杀编译进程。
        self.assertIn(
            "- name: cargo test --lib --no-run（编译链接测试二进制）\n"
            "        if: ${{ github.event_name != 'push' }}",
            rust_test,
        )
        self.assertIn(
            'sudo bash "${{ github.workspace }}/scripts/ci-memory-setup.sh"',
            rust_test,
        )
        self.assertNotIn("ci-memguard", self.pr_workflow)
        self.assertIn('CARGO_PROFILE_DEV_DEBUG: "0"', rust_test)
        self.assertIn("timeout-minutes: 120", rust_test)
        self.assertIn(
            'RUSTFLAGS: "-C link-arg=-fuse-ld=lld '
            '-C link-arg=-Wl,--thinlto-jobs=1 '
            '-C link-arg=-Wl,--threads=1"',
            rust_test,
        )

    def test_all_linux_jobs_enlarge_runner_memory(self):
        # Every ubuntu-* job must run the zram/swap memory setup right after
        # checkout; Windows/macOS jobs are out of scope (hosted images there
        # have different memory characteristics).
        # Only split at 2-space-indented `key:` lines (job/trigger boundaries),
        # not at deeper indentation.
        setup_step = "- name: Set up zram and swap"
        blocks = re.split(
            r"\n  (?=[A-Za-z0-9_-]+:\s*$)", self.pr_workflow, flags=re.MULTILINE
        )
        linux_jobs = [
            block
            for block in blocks
            if re.search(r"^    runs-on: ubuntu", block, flags=re.MULTILINE)
        ]
        self.assertGreaterEqual(len(linux_jobs), 10)
        for job in linux_jobs:
            job_name = job.strip().split(":", maxsplit=1)[0]
            self.assertIn(
                setup_step,
                job,
                f"ubuntu job '{job_name}' must run scripts/ci-memory-setup.sh",
            )
            self.assertIn(
                '"${{ github.workspace }}/scripts/ci-memory-setup.sh"',
                job,
                f"ubuntu job '{job_name}' must invoke scripts/ci-memory-setup.sh"
                " via an absolute path (some jobs set a run working-directory)",
            )
            # An in-kernel hang cannot be interrupted by the userspace timeout
            # inside the script; the workflow-side `timeout 240` plus the
            # non-fatal wrapper is the only backstop (the last line of defense
            # the script header claims). A job missing the wrapper would burn
            # the whole job limit when it hangs, so the guard enforces an
            # identical structure at every call site.
            self.assertIn(
                'run: timeout --kill-after=15 240 sudo bash "${{ github.workspace }}/scripts/ci-memory-setup.sh"',
                job,
                f"ubuntu job '{job_name}' must hard-cap ci-memory-setup with"
                " 'timeout --kill-after=15 240' (userspace hang backstop)",
            )
            self.assertIn(
                '|| echo "::warning::ci-memory-setup',
                job,
                f"ubuntu job '{job_name}' must keep ci-memory-setup non-fatal"
                " (degrade to stock runner memory with a ::warning)",
            )

    def test_memory_setup_wrapped_at_every_call_site(self):
        # The wrapper contract is repo-wide, not just pr-check.yml: every
        # invocation of ci-memory-setup.sh in any workflow file must carry
        # the `timeout --kill-after=15 240` cap and the non-fatal ::warning
        # degradation on the same run line.
        workflows = sorted(
            list((ROOT / ".github/workflows").glob("*.yml"))
            + list((ROOT / ".github/workflows").glob("*.yaml"))
        )
        self.assertTrue(workflows, "no workflow files found under .github/workflows")
        call_sites = 0
        for workflow in workflows:
            text = _without_yaml_comments(workflow.read_text(encoding="utf-8"))
            for line in text.splitlines():
                if "scripts/ci-memory-setup.sh" not in line:
                    continue
                call_sites += 1
                self.assertIn(
                    "timeout --kill-after=15 240",
                    line,
                    f"{workflow.name}: the ci-memory-setup.sh call must be"
                    " capped by 'timeout --kill-after=15 240' (an in-kernel"
                    " hang is uninterruptible; the outer cap is the last"
                    " backstop)",
                )
                self.assertIn(
                    '|| echo "::warning::ci-memory-setup',
                    line,
                    f"{workflow.name}: the ci-memory-setup.sh call must stay"
                    " non-fatal (degrade to stock runner memory with a"
                    " ::warning)",
                )
        self.assertGreaterEqual(
            call_sites, 20, "expected the memory-setup wrapper at 20+ call sites"
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
        # Cold Windows compile plus the lib link check recently died at the
        # 90-minute cap while passing runs already took 85-87 minutes.
        self.assertIn("timeout-minutes: 180", windows_rust_test)

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
        # Cut at the next job boundary: the round-17 macos-rust-check leg
        # legitimately runs `cargo test` (computer_use unit tests) and sits
        # between the regression step and the previous extraction boundary —
        # this assertion only guards the Windows regression against
        # re-invoking cargo, which would re-link the exe and lose the
        # embedded Common-Controls manifest.
        regression = regression.split("\n  macos-rust-check:", maxsplit=1)[0]
        self.assertIn('"$test_exe" "$filter" --test-threads=1', regression)
        self.assertNotIn("cargo test", regression)
        self.assertIn(
            "'connector_introspection_guard_matches_complete_names_only'",
            regression,
            "Windows must execute the PowerShell connector-introspection hook regression",
        )

        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("- windows-rust-test", required_gate)
        self.assertIn("WINDOWS_RUST_RESULT", required_gate)
        self.assertIn('"windows-rust-test:$WINDOWS_RUST_RESULT"', required_gate)

    def test_macos_rust_check_is_wired_into_required_gate(self):
        # Review finding: the new native macOS leg must satisfy the same
        # three-wiring rule as windows-rust-test (needs entry, env backfill,
        # summary-loop entry) -- otherwise removing the job keeps CI green
        # while the gate silently degrades.
        job_body = self.pr_workflow.split("\n  macos-rust-check:", maxsplit=1)[1]
        job = re.split(r"\n  [a-zA-Z]", job_body, maxsplit=1)[0]
        self.assertIn("needs: changes", job)
        self.assertIn("MACOS_RUST_CHECK_RESULT", self.pr_workflow)
        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("- macos-rust-check", required_gate)
        self.assertIn("MACOS_RUST_CHECK_RESULT", required_gate)
        self.assertIn('"macos-rust-check:$MACOS_RUST_CHECK_RESULT"', required_gate)

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
        )[1].split("            pet:", maxsplit=1)[0]
        self.assertIn(
            "- 'pinvou3-app/src-tauri/resources/**'",
            release_contract_paths,
        )
        self.assertIn(
            "- 'pinvou3-app/tests/knowledge_host_packaging.test.mjs'",
            release_contract_paths,
        )
        # The section boundary above must stay load-bearing: if the split
        # anchor stops matching (e.g. a filter rename), the slice silently
        # grows to the end of the changes block and these assertions
        # degrade into no-ops. This bit us once with a stale "l1:" anchor.
        self.assertNotIn(
            "- 'pinvou3-app/src/app/pet-main.jsx'",
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

    def test_secret_scan_guard_and_cutoff_are_load_bearing(self):
        # The empty-scan guard must demand positive evidence of a non-zero
        # commit count: it is an inverted grep, so an empty log (gitleaks logs
        # to stderr, so a dropped 2>&1 empties the tee'd file), a "0 commits
        # scanned" no-op, or any other missing or renamed summary fails the
        # step instead of going green. The scan range must share the same
        # LEGACY_HISTORY_CUTOFF as the commit-message gate; drifting either
        # side alone would shift the trust boundary between secret scanning
        # and the commit convention.
        secret_scan = (
            ROOT / ".github/workflows/secret-scan.yml"
        ).read_text(encoding="utf-8")
        self.assertIn('HEAD" 2>&1', secret_scan)
        guard = re.search(
            r'if ! grep -Eq "([^"]+)" /tmp/gitleaks\.log', secret_scan
        )
        self.assertIsNotNone(
            guard, "secret-scan.yml must fail closed on missing scan evidence"
        )
        count_pattern = guard.group(1)
        # The gitleaks summary line is "N commits scanned." (ANSI-wrapped);
        # the guard pattern must accept that shape for N > 0 and reject both
        # the zero-commit summary and an empty log (no match at all).
        self.assertTrue(
            re.search(count_pattern, "395 commits scanned."),
            "guard pattern must accept a real non-zero gitleaks summary line",
        )
        self.assertFalse(
            re.search(count_pattern, "0 commits scanned."),
            "guard pattern must reject a zero-commit scan summary",
        )
        self.assertFalse(
            re.search(count_pattern, ""),
            "guard pattern must reject an empty scan log",
        )
        validator = (ROOT / "scripts/validate-commit-msg.py").read_text(
            encoding="utf-8"
        )
        match = re.search(r'LEGACY_HISTORY_CUTOFF = "([0-9a-f]{40})"', validator)
        self.assertIsNotNone(
            match, "validate-commit-msg.py is missing the LEGACY_HISTORY_CUTOFF constant"
        )
        self.assertIn(match.group(1), secret_scan)

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
        self.assertIn("os: [macos-15, ubuntu-22.04, windows-latest]", smoke)




class ReleaseDiskAndImagePolicyTests(unittest.TestCase):
    """Guard for release-build disk preparation and the single-image convention (backported from private-repo #1112 on 2026-09-16)."""

    def setUp(self):
        self.release_workflow = (ROOT / ".github/workflows/release-packages.yml").read_text(
            encoding="utf-8"
        )

    def test_release_linux_build_jobs_prepare_disk_and_prune_apt(self):
        # Release build jobs (single-disk hosted runner; with the old 16G
        # /mnt swapfile in place x64 builds had only ~13-14G free) must clean
        # up unused preinstalled SDKs before toolchains/caches/dependencies
        # hit the disk, and run autoremove + clean after installing system
        # deps; otherwise a cold compile has filled the disk (ENOSPC, since
        # 2026-09-13).
        blocks = re.split(
            r"\n  (?=[A-Za-z0-9_-]+:\s*$)", self.release_workflow, flags=re.MULTILINE
        )
        for job_id in ("build-linux-x64", "build-linux-arm64"):
            job = next(
                (b for b in blocks if b.strip().startswith(f"{job_id}:")), None
            )
            self.assertIsNotNone(job, f"release job '{job_id}' not found")
            self.assertIn("python3 scripts/ci-rust-disk.py", job)
            self.assertIn("--min-free-gib 24", job)
            # Disk preparation must run before setup-node (the aggressive tier
            # deletes /opt/hostedtoolcache, and setup-node would re-download
            # Node afterwards) and before the toolchain and the Rust cache
            # land, so the free-space gate measures the disk the cold build
            # actually gets.
            self.assertLess(
                job.index("python3 scripts/ci-rust-disk.py"),
                job.index("uses: actions/setup-node"),
            )
            self.assertLess(
                job.index("python3 scripts/ci-rust-disk.py"),
                job.index("uses: dtolnay/rust-toolchain"),
            )
            self.assertLess(
                job.index("python3 scripts/ci-rust-disk.py"),
                job.index("uses: Swatinem/rust-cache"),
            )
            self.assertIn("sudo apt-get autoremove -y --purge", job)
            self.assertIn("sudo apt-get clean", job)
        x64 = next(b for b in blocks if b.strip().startswith("build-linux-x64:"))
        arm64 = next(b for b in blocks if b.strip().startswith("build-linux-arm64:"))
        self.assertIn("--aggressive", x64)
        self.assertNotIn("--aggressive", arm64)

    def test_all_linux_jobs_pin_the_release_runner_image(self):
        # Image versions never drift (single-image convention): every
        # workflow's Linux runner must match the release build baseline
        # (ubuntu-22.04 / ubuntu-22.04-arm). Release binaries link against the
        # build host's glibc, so tests and checks must run on the same system
        # as the release build; image upgrades must be coordinated across the
        # whole repo at once — rolling images like ubuntu-latest and per-job
        # version bumps are forbidden.
        # Strip full-line YAML comments before scanning: version mentions
        # inside full-line comments (e.g. migration notes) must not trip the
        # image rules. Note that the ubuntu-latest ban still scans the
        # remaining text, including inline trailing comments — keep such
        # notes on their own comment lines.
        allowed = {"ubuntu-22.04", "ubuntu-22.04-arm"}
        workflows = sorted(
            list((ROOT / ".github/workflows").glob("*.yml"))
            + list((ROOT / ".github/workflows").glob("*.yaml"))
        )
        self.assertTrue(workflows, "no workflow files found under .github/workflows")
        for workflow in workflows:
            text = _without_yaml_comments(workflow.read_text(encoding="utf-8"))
            self.assertNotIn(
                "ubuntu-latest",
                text,
                f"{workflow.name}: ubuntu-latest is a rolling image and violates"
                " the single-image convention; pin it to ubuntu-22.04 like the"
                " release build",
            )
            for image in sorted(set(re.findall(r"ubuntu-\d+\.\d+(?:-arm)?", text))):
                self.assertIn(
                    image,
                    allowed,
                    f"{workflow.name}: Linux image '{image}' diverges from the"
                    " release baseline; image upgrades must happen repo-wide"
                    " at once",
                )

    def test_audited_redundant_apt_packages_stay_pruned(self):
        # 2026-09-15 per-package probe audit verdict (simulate installing each
        # package alone; if the installed set is unchanged the package is
        # redundant): libgtk-3-dev/libsoup-3.0-dev/libx11-dev/libxi-dev/
        # libxtst-dev are all pulled in transitively by the hard dependency
        # chain of libwebkit2gtk-4.1-dev, and the build does not need
        # librsvg2-dev (Cargo has no rsvg crate). Any workflow adding them
        # back to an install list would slow dependency installation and eat
        # the single-disk runner's build disk. The guard scans the full text
        # with comments stripped: audit comments may mention these names, but
        # their appearance in non-comment text is rejected (including
        # multi-line continuations).
        redundant = (
            "libgtk-3-dev",
            "libsoup-3.0-dev",
            "librsvg2-dev",
            "libx11-dev",
            "libxi-dev",
            "libxtst-dev",
        )
        workflows = sorted(
            list((ROOT / ".github/workflows").glob("*.yml"))
            + list((ROOT / ".github/workflows").glob("*.yaml"))
        )
        self.assertTrue(workflows, "no workflow files found under .github/workflows")
        for workflow in workflows:
            text = _without_yaml_comments(workflow.read_text(encoding="utf-8"))
            for package in redundant:
                self.assertNotIn(
                    package,
                    text,
                    f"{workflow.name}: audited redundant apt package '{package}'"
                    " must not be added back (pulled in transitively by"
                    " libwebkit2gtk-4.1-dev or not needed by the build)",
                )


if __name__ == "__main__":
    unittest.main()
