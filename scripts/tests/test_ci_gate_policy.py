import re
import unittest
from fnmatch import fnmatchcase
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
PR_WORKFLOW = ROOT / ".github/workflows/pr-check.yml"
RELEASE_WORKFLOW = ROOT / ".github/workflows/release-packages.yml"
REQUIRED_WORKFLOWS = (
    ROOT / ".github/workflows/dco.yml",
    ROOT / ".github/workflows/secret-scan.yml",
    ROOT / ".github/workflows/dependency-review.yml",
    PR_WORKFLOW,
)
PUBLIC_SUBMODULE_VERIFIER = ROOT / "scripts/verify-public-submodule.sh"


def _extract_quoted_paths(block):
    """提取 YAML 块中 `- 'path'` / `- "path"` 形式的路径条目(保持文本序)。

    Round-40 review M4: 单引号形式之外还要认双引号形式——一条
    `- "pinvou-cli/**"` 若对提取器不可见,下面所有基于提取结果的 pin
    (存在性、死条目、成员、workflow 排除)都会被同一条目绕过。

    Round-41 review M8: 条目行尾的同列注释不再制造盲区——
    `- '!pinvou-cli/**' # trim scope` 对 dorny 是一条排除项,旧提取器
    (要求行以引号收尾)却看不见它。引号内出现的 `#` 属于路径本身
    (YAML 规范:注释以引号后的 `#` 开始),因此先取引号闭包、再剥其后的
    注释。凡是引用形式的条目行(`- '…'` / `- "…"` 开头)却匹配不上
    引号闭包正则的,一律视为未识别形状直接让断言红掉——那正是"提取器
    看不见 → 下方全部 pin 可被同一条目绕过"的形状。其余列表行(作业步骤
    `- name:`、`- uses:`、裸 needs 等)不是路径条目,维持原样忽略——
    有些调用切片覆盖整个 job,历史行为依赖这一点。
    """
    paths = []
    unrecognized = []
    for line in block.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        match = re.match(r"""^- ('(.*)'|"(.*)")(?:\s+#.*)?$""", stripped)
        if match:
            paths.append(match.group(2) if match.group(2) is not None else match.group(3))
        elif stripped.startswith("- '") or stripped.startswith('- "'):
            unrecognized.append(stripped)
    if unrecognized:
        raise AssertionError(
            "unrecognized quoted path-filter entry(ies); the extractor cannot "
            "parse them, so every extractor-based pin below would be bypassable "
            "by the same entry — fix the quoting or move prose to a comment "
            "line: " + "; ".join(unrecognized)
        )
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
        # required-gate's display `name:` is the branch-protection check name;
        # the suite locates the job only by its YAML key, so a display-name
        # rename would silently orphan the required check. Pin it in every
        # test that asserts on this block.
        self.assertIn("name: required-gate", required_gate)
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

    def test_path_extractor_sees_trailing_comment_entries(self):
        # Round-41 review M8 回归锁:行尾同列注释曾让整条条目对提取器
        # 不可见——`- '!pinvou-cli/**' # trim scope` 对 dorny 是一条真实
        # 排除项,而旧提取器要求行以引号收尾,于是 cli_rust 的可达性、
        # 必需成员与 workflow 排除 pin 全部被同一条目绕过(经变异验证,
        # 43 个测试全绿)。引号闭包正则必须把带注释的条目原样解析出来,
        # 且引用形式却无法解析的形状必须让断言红掉而不是被静默忽略。
        block = (
            "          paths:\n"
            "            - 'pinvou-cli/**'\n"
            "            - '!pinvou-cli/docs/**' # trim the docs subtree\n"
            '            - "pinvou-cli/Cargo.lock"  # lock pin\n'
            "            - 'pinvou-cli/a#b/**' # hash inside the quotes is literal\n"
        )
        entries = _extract_quoted_paths(block)
        self.assertEqual(
            entries,
            [
                "pinvou-cli/**",
                "!pinvou-cli/docs/**",
                "pinvou-cli/Cargo.lock",
                "pinvou-cli/a#b/**",
            ],
            "带行尾注释的条目必须被完整解析(dorny 看得见,提取器也必须看得见)",
        )
        with self.assertRaises(AssertionError):
            # 引用形式但引号在本行不闭合:该行对 YAML 是坏的,对旧提取器
            # 是"静默忽略",对现在必须红。
            _extract_quoted_paths("            - 'pinvou-cli/unclosed\n")

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
            "bundle_chain",
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
        self.assertIn("name: required-gate", required_gate)
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
            "cargo clippy --manifest-path pinvou-knowledge/Cargo.toml --lib --bins --no-deps --features server --locked -- -D warnings",
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
        self.assertIn("name: required-gate", required_gate)
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

    def test_fast_gate_runner_line_is_pinned(self):
        # Round-47 review: this one line is the ONLY place the gate-policy
        # suite itself executes in CI. Deleting it (or narrowing the `-p`
        # pattern) disarms every pin in this file while each test stays
        # green wherever it still happens to run — so the runner is pinned
        # by its exact command here, making the rest of the suite
        # load-bearing instead of decorative.
        fast_gate = self.pr_workflow.split("\n  fast-gate:", maxsplit=1)[1].split(
            "\n  frontend-test:", maxsplit=1
        )[0]
        self.assertIn(
            "python3 -m unittest discover -s scripts/tests -p 'test_*.py'",
            fast_gate,
            "the gate-policy suite's only CI runner must not be deletable in "
            "silence",
        )

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
        # Round-42 review: membership pins assert against the EXTRACTOR's
        # parsed entries, not the raw slice — a trailing comment quoting the
        # same path (`# was '- 'pinvou-cli/**/Cargo.lock''`) satisfied the
        # raw assertIn while the extractor (and dorny) saw no entry, so a
        # deleted entry kept every pin green. The extractor also fails
        # closed on unrecognized quoting shapes.
        cli_rust_entries_probe = _extract_quoted_paths(cli_paths)

        def assert_extracted_entry(entry: str, message: str) -> None:
            self.assertIn(entry, cli_rust_entries_probe, message)

        assert_extracted_entry(
            "pinvou-cli/**/*.rs",
            "cli_rust must match the real crate directory (pinvou-cli)",
        )
        assert_extracted_entry("pinvou-cli/**/Cargo.toml", "cli_rust must route the Cargo.toml set")
        assert_extracted_entry(
            "pinvou-cli/**/Cargo.lock",
            "every CLI leg builds --locked, so a lockfile-only change (a "
            "dependency bump, a resolver rewrite) changes exactly what they "
            "compile; without this entry such a PR skips cli-test, "
            "windows-rust-test AND macos-cli-check and required-gate passes "
            "on 'skipped'",
        )
        # Round-28: the CLI's remaining build inputs are routed like its
        # sources. .cargo/config.toml feeds EVERY CLI build (a resolver,
        # target, or rustflags change compiles differently everywhere), and
        # build.rs embeds the exe manifest into the Windows binary.
        assert_extracted_entry(
            "pinvou-cli/.cargo/**",
            "cli_rust must route pinvou-cli/.cargo: config.toml changes what "
            "every CLI leg compiles; without this entry such a PR skips "
            "cli-test, cli-lint, windows-rust-test AND macos-cli-check on "
            "'skipped'",
        )
        assert_extracted_entry(
            "pinvou-cli/**/*.manifest",
            "cli_rust must route the exe manifest: build.rs embeds it into "
            "the Windows binary, and without this entry a manifest-only PR "
            "runs no CLI leg at all",
        )
        # Round-47 review: cli-test and cli-lint (the Linux CLI legs)
        # execute this setup script; routed only via rust_code, a
        # script-only PR reached the post-merge push before any CLI leg
        # could catch a break.
        assert_extracted_entry(
            "scripts/ci-libpipewire-build.sh",
            "the PipeWire prefix build is a setup step of the Linux CLI "
            "legs; a script-only PR must trigger them",
        )
        assert_extracted_entry("CodeWhale", "cli_rust must route the foundation gitlink")
        # Round-38: every literal cli_rust filter entry must name a path that
        # EXISTS in the repository. The round-37 mcp-servers entry shipped as
        # `pinvoy3-app/...` (a typo), which no glob ever matches — the entry
        # was dead, the gap it claims to close stayed open, and this suite
        # pinned the dead spelling as if it were coverage. A reachability
        # check keeps the pin from outliving the path again.
        cli_rust_entries = _extract_quoted_paths(cli_paths)
        self.assertTrue(cli_rust_entries, "cli_rust paths 解析为空")
        # Round-40 review M4: cli_rust must route on POSITIVE entries only.
        # The filter runs with dorny's `predicate-quantifier: some-with-excludes`,
        # where one negated entry (`- '!pinvou-cli/**'`) makes cli_rust false
        # for every matching change — and rust_full does not cover CLI `.rs`
        # files, so a single added line would skip cli-test, cli-lint,
        # windows-rust-test and macos-cli-check for all CLI-only PRs while
        # required-gate passes on `skipped`. The extractor above now also
        # sees double-quoted entries, so the quote style cannot hide a
        # negation from this pin either. Exclusions belong in rust_full,
        # which owns them today (feedback/personas/pet).
        for entry in cli_rust_entries:
            self.assertFalse(
                entry.startswith("!"),
                f"cli_rust must not carry negation entries (one '!...' line "
                f"silently disables every CLI gate): {entry}",
            )
        # Submodule gitlinks are not checked out everywhere this suite runs
        # (fast-gate needs no CodeWhale tree), so their existence is pinned
        # by .gitmodules instead of the working tree.
        submodule_paths = set()
        gitmodules = ROOT / ".gitmodules"
        if gitmodules.exists():
            for line in gitmodules.read_text(encoding="utf-8").splitlines():
                stripped = line.strip()
                if stripped.startswith("path = "):
                    submodule_paths.add(stripped[len("path = "):].strip())
        for entry in cli_rust_entries:
            candidate = entry[1:] if entry.startswith("!") else entry
            if candidate.endswith("/**"):
                # Round-39 review: the round-37 dead entry was exactly this
                # shape (`pinvoy3-app/resources/mcp-servers/**` — a typo'd
                # directory prefix), so the dir/** form must be reachability-
                # checked too: the prefix directory must exist. The glob tail
                # itself stays the paths-filter's business.
                prefix = candidate[: -len("/**")]
                self.assertTrue(
                    (ROOT / prefix).is_dir(),
                    f"cli_rust 过滤条目的目录前缀在仓库中不存在(死条目): {entry}",
                )
                continue
            if "*" in candidate:
                # Glob metachars beyond the dir/** form are resolved by the
                # paths-filter itself; only literal and dir/** entries can
                # rot in a checkable way.
                continue
            if candidate in submodule_paths:
                continue
            self.assertTrue(
                (ROOT / candidate).exists(),
                f"cli_rust 过滤路径在仓库中不存在(死条目): {entry}",
            )
        # The connector lock tables are compiled into the CLI with include_str!
        # from src-tauri/src/platform/connector_lock.rs, so editing or
        # deleting one is a CLI source change in all but name — and
        # macos-cli-check, the leg whose reason for existing is exactly those
        # per-target files, is the first thing skipped without this entry.
        # Round-43 review: these pins join the round-42 extractor discipline —
        # a raw `assertIn("- 'path'", slice)` is satisfied by a trailing
        # comment quoting the same path while dorny sees no entry, so the
        # deleted entry kept every pin green.
        assert_extracted_entry(
            "pinvou3-app/src-tauri/resources/platforms/**",
            "the connector lock tables are include_str!'d into the CLI; "
            "without this entry a lock-table PR runs no CLI leg at all",
        )
        # The wider include_str!/build.rs input class: build.rs reads four
        # bundle instruction files + deny_sensitive_paths.sh unconditionally
        # (panicking when absent) and native_installer.rs include_str!s the
        # dws LICENSE — a bundle-resource-only rename must not skip the CLI
        # legs any more than a lock-table edit does.
        assert_extracted_entry(
            "pinvou3-app/src-tauri/resources/common/bundle/**",
            "the bundle resources are build.rs/include_str! inputs of every "
            "pinvou3-tauri compile; without this entry a bundle-resource PR "
            "runs no CLI leg at all",
        )
        # Round-37 review: the remaining include_str! inputs of every CLI-leg
        # compile — mcp_catalog.rs embeds the APP-LEVEL mcp-servers tree (not
        # src-tauri/resources) and marketplace.rs embeds the plugin package
        # spec doc. `pub mod marketplace;` is unconditional, so a rename in
        # either must not skip every compile leg while required-gate passes
        # on skipped.
        assert_extracted_entry(
            "pinvou3-app/resources/mcp-servers/**",
            "mcp_catalog.rs include_str!s the app-level mcp-servers tree; "
            "without this entry an mcp-servers PR runs no CLI leg at all",
        )
        assert_extracted_entry(
            "docs/plugin-package-spec.md",
            "marketplace.rs include_str!s the plugin package spec; without "
            "this entry a spec-doc PR runs no CLI leg at all",
        )
        # Round-49 review: cli_contract.rs include_str!s the gaia benchmark
        # doc and asserts its level table, so a doc-only edit must run a CLI
        # leg. The routing entry alone cannot protect itself — this pin is
        # what makes deleting the entry fail the suite instead of silently
        # re-opening the skipped-legs hole the round-48 entry closed.
        assert_extracted_entry(
            "docs/gaia-benchmark.md",
            "cli_contract.rs include_str!s the gaia benchmark doc; without "
            "this entry a doc-only edit runs no CLI leg at all",
        )
        # The CLI path-depends on the app crate, so the leaf features that
        # rust_full exempts still gate through the CLI suite (a change confined
        # to features/feedback or features/personas would otherwise run NO rust
        # gate at all).
        assert_extracted_entry(
            "pinvou3-app/src-tauri/src/features/feedback/**",
            "feedback changes must gate through the CLI suite",
        )
        assert_extracted_entry(
            "pinvou3-app/src-tauri/src/features/personas/**",
            "personas changes must gate through the CLI suite",
        )
        assert_extracted_entry(
            "pinvou3-app/src-tauri/src/features/pet/**",
            "pet is exempted from rust_full like feedback/personas, so pet Rust "
            "changes must gate through the CLI suite too",
        )
        # Policy: the workflow file itself is deliberately excluded from
        # cli_rust — workflow edits must not link the full-app test suites
        # (this test enforces that). cli-test changes are instead validated
        # by the next cli_rust PR / the merge queue. Extractor-based so a
        # trailing-comment-disguised ENTRY fails this pin (fail closed) the
        # same way a deleted one does.
        self.assertNotIn(
            ".github/workflows/pr-check.yml",
            cli_rust_entries_probe,
            "cli_rust must not include the workflow file: workflow edits must "
            "not link the full-app test suites",
        )

        # Round-40 review: the dir/**-rot class the cli_rust reachability
        # check above closes is not cli_rust-specific. The same dead-spelling
        # shape in knowledge_rust or windows_codex silently skips the
        # knowledge and Windows codex legs for a change-set they own. The
        # same sweep runs over both groups (existence + dir/** prefix);
        # knowledge_rust's workflow-file entry is a literal that exists, and
        # windows_codex's `private-runtimes/windows` gitlink resolves through
        # the same .gitmodules exemption the cli_rust loop applies.
        def assert_group_paths_reachable(group_block: str, group_name: str) -> None:
            entries = _extract_quoted_paths(group_block)
            self.assertTrue(entries, f"{group_name} paths 解析为空")
            for entry in entries:
                candidate = entry[1:] if entry.startswith("!") else entry
                if candidate.endswith("/**"):
                    prefix = candidate[: -len("/**")]
                    self.assertTrue(
                        (ROOT / prefix).is_dir(),
                        f"{group_name} 过滤条目的目录前缀在仓库中不存在(死条目): {entry}",
                    )
                    continue
                if "*" in candidate:
                    continue
                if candidate in submodule_paths:
                    continue
                self.assertTrue(
                    (ROOT / candidate).exists(),
                    f"{group_name} 过滤路径在仓库中不存在(死条目): {entry}",
                )

        knowledge_rust_paths = changes.split(
            "            knowledge_rust:", maxsplit=1
        )[1].split("            knowledge_dependencies:", maxsplit=1)[0]
        assert_group_paths_reachable(knowledge_rust_paths, "knowledge_rust")
        windows_codex_paths = self.pr_workflow.split(
            "            windows_codex:", maxsplit=1
        )[1].split("            bundle_chain:", maxsplit=1)[0]
        assert_group_paths_reachable(windows_codex_paths, "windows_codex")
        # Round-49 review: the standalone windows_codex slice used to split
        # at `pet:` — a job declared BEFORE it — so the slice ran to EOF and
        # bundle_chain's coverage was an accident of that unbounded sweep.
        # windows_codex is now bounded at its real neighbor, and the terminal
        # group gets its own explicit sweep to EOF.
        bundle_chain_paths = self.pr_workflow.split(
            "            bundle_chain:", maxsplit=1
        )[1]
        assert_group_paths_reachable(bundle_chain_paths, "bundle_chain")

        # Round-42 review: the sweep now covers EVERY filter group in the
        # changes block, not just the three that originally motivated it —
        # a dead dir/** spelling in rust_code, release_contract, pet,
        # frontend, relay, acp_runtime, rust_full or the dependency groups
        # silently skipped the legs that group owns, exactly like the
        # round-37 pinvoy3-app class. Groups are split sequentially in
        # declaration order, so a NEW group appended without updating this
        # list still gets covered as long as it sits between two known
        # neighbors. Round-49 review: the terminal group's own entries are
        # swept explicitly to EOF (see bundle_chain above) instead of riding
        # the old unbounded windows_codex slice; a group appended after
        # bundle_chain is likewise inside that terminal sweep, but add its
        # pair here anyway so the reachability message names the right
        # group. The pairs mirror the workflow's order.
        group_bounds = [
            ("rust_code", "rust_dependencies"),
            ("rust_dependencies", "rust_full"),
            ("rust_full", "cli_rust"),
            ("cli_rust", "knowledge_rust"),
            ("knowledge_dependencies", "release_contract"),
            ("release_contract", "pet"),
            ("pet", "frontend"),
            ("frontend", "relay"),
            ("relay", "acp_runtime"),
            ("acp_runtime", "windows_codex"),
            ("windows_codex", "bundle_chain"),
        ]
        for group, next_group in group_bounds:
            block = changes.split(f"            {group}:", maxsplit=1)[1].split(
                f"            {next_group}:", maxsplit=1
            )[0]
            assert_group_paths_reachable(block, group)

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
        # Round-47 review: the EXECUTION steps' `if` conditions are pinned as
        # name+`if` pairs. The run-command substrings above stay green if a
        # step's `if` flips or dies (e.g. `!= 'push'` → `false`), which would
        # make cli-test report success while running NO tests on PRs or in
        # the Merge Queue — the exact silent-skip class the windows phase
        # map's end-anchored pins close for the Windows leg.
        self.assertIn(
            "- name: cargo test (all targets, no-fail-fast)\n"
            "        if: github.event_name != 'push'",
            cli_test,
        )
        self.assertIn(
            "- name: cargo compile check (push)\n"
            "        if: github.event_name == 'push'",
            cli_test,
        )
        self.assertIn(
            "- name: cargo test (adapter-gaia test-support)\n"
            "        if: github.event_name != 'push'",
            cli_test,
        )
        self.assertIn(
            "- name: cargo compile check (push, adapter-gaia test-support)\n"
            "        if: github.event_name == 'push'",
            cli_test,
        )
        self.assertIn("cache-targets: false", cli_test)

        # The Windows leg also compile-checks the pinvou-cli workspace: the
        # CLI's cfg(target_os = "windows") branches (exe/cmd shims, taskkill,
        # CREATE_NO_WINDOW) only type-check on a Windows runner, and cli_rust
        # must trigger that leg exactly like it triggers cli-test.
        windows_rust_test = self.pr_workflow.split(
            "\n  windows-rust-test:", maxsplit=1
        )[1].split("\n  windows-codex-runtime-test:", maxsplit=1)[0]
        self.assertIn(
            "needs.changes.outputs.cli_rust == 'true'",
            windows_rust_test,
            "windows-rust-test must be triggered by cli_rust: its pinvou-cli "
            "compile check is the only Windows leg for CLI code",
        )
        windows_rust_steps = _without_yaml_comments(windows_rust_test)
        self.assertIn(
            "- name: pinvou-cli Windows compile check",
            windows_rust_steps,
        )
        self.assertIn(
            "cargo check --manifest-path pinvou-cli/Cargo.toml",
            windows_rust_steps,
        )
        self.assertIn("--workspace --all-targets --locked", windows_rust_steps)
        # The adapter-gaia test-support target compiles on the Windows leg:
        # required-features hide it from the workspace-wide steps above, and
        # deleting this step must fail the policy suite.
        self.assertIn(
            "-p adapter-gaia --features test-support --all-targets --locked",
            windows_rust_steps,
        )
        # Round-42 review: the pinvou-cli workspace's own cfg(windows) unit
        # tests EXECUTE here, filtered. Round-43 review: pin the STEP and all
        # three filters — deleting the step (or one filter) must fail the
        # policy suite instead of silently orphaning the Windows pins again
        # (the ACL filter's absence is exactly how a third cfg(windows) test
        # ended up running on no leg at all).
        self.assertIn(
            "- name: pinvou-cli Windows-gated unit tests",
            windows_rust_steps,
        )
        self.assertIn(
            "run_filtered test --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli --lib --locked windows_batch_tests",
            windows_rust_steps,
        )
        self.assertIn(
            "run_filtered test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia --features test-support --lib --locked dataset_windows",
            windows_rust_steps,
        )
        self.assertIn(
            "run_filtered test --manifest-path pinvou-cli/Cargo.toml -p adapter-gaia --features test-support --lib --locked fetch_windows_acl",
            windows_rust_steps,
            "the Windows ACL privacy round-trip test must stay gated to this "
            "leg — it is the crate's only cfg(windows) ACL pin and ran on no "
            "leg before this filter existed",
        )
        # Round-47 review: the round-46 rebind case-fold filter is the one
        # run_filtered line the suite did NOT pin (all five siblings above
        # were pinned) — deleting the whole line passed the policy suite,
        # the exact silent-orphan failure mode this suite exists to catch.
        self.assertIn(
            "run_filtered test --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli --lib --locked rebind_nesting_rejection_folds_case_where_the_os_folds",
            windows_rust_steps,
            "the rebind case-fold half executes only on this leg; deleting "
            "the filter must fail the suite like its siblings' pins",
        )
        # Round-44 review: benchmark-core's cfg(windows) security pins (the
        # DPAPI fail-closed blob tests and the Windows ACL parse pin) ran on
        # NO leg either — `-p benchmark-core` appeared in no workflow, the
        # same orphan class the ACL filter's absence created. Pin both
        # module filters: deleting the step or one filter must fail the
        # policy suite instead of silently orphaning the pins again.
        self.assertIn(
            "run_filtered test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --lib --locked private_prediction::tests::windows_",
            windows_rust_steps,
        )
        self.assertIn(
            "run_filtered test --manifest-path pinvou-cli/Cargo.toml -p benchmark-core --lib --locked windows_private_acl::tests::",
            windows_rust_steps,
        )
        # The zero-match guard bodies are load-bearing — a renamed test must
        # FAIL the step, not pass silently — so pin them alongside the
        # filters they protect (round-44 review).
        self.assertIn("grep -qE 'running [1-9][0-9]* tests?'", windows_rust_steps)
        self.assertIn('grep -q "^test result: ok"', windows_rust_steps)
        self.assertIn(
            "a renamed test must not become a silent pass", windows_rust_steps
        )

        # macOS-gated CLI code must type-check somewhere: cli-test is
        # ubuntu-only, so the dedicated macos-cli-check leg mirrors the
        # Windows compile check and gates through required-gate. It runs for
        # ready cli_rust/rust_full PRs, the matching Merge Queue combined tree
        # (green-alone PRs can still combine into a macOS-only compile break),
        # and main push (cumulative); see
        # test_macos_cli_check_runs_on_main_push_and_merge_group.
        macos_cli = self.pr_workflow.split(
            "\n  macos-cli-check:", maxsplit=1
        )[1].split("\n  required-gate:", maxsplit=1)[0]
        # Round-46 review: the needs wire stays pinned here too, so a
        # bypass of the wiring test cannot silently drop it.
        self.assertIn("needs: changes", macos_cli)
        self.assertIn("runs-on: macos-15", macos_cli)
        self.assertIn(
            "needs.changes.outputs.cli_rust == 'true'",
            macos_cli,
            "macos-cli-check must use the same cli_rust trigger as cli-test",
        )
        self.assertIn(
            "needs.changes.outputs.rust_full == 'true'",
            macos_cli,
            "macos-cli-check must cover rust_full like cli-test does",
        )
        self.assertIn(
            "github.event.pull_request.draft == false",
            macos_cli,
            "draft PRs must skip the macOS compile leg like the other rust jobs",
        )
        macos_cli_steps = _without_yaml_comments(macos_cli)
        self.assertIn(
            "- name: pinvou-cli macOS compile check",
            macos_cli_steps,
        )
        self.assertIn(
            "cargo check --manifest-path pinvou-cli/Cargo.toml",
            macos_cli_steps,
        )
        self.assertIn("--workspace --all-targets --locked", macos_cli_steps)
        # Mirror of the Windows pin: the adapter-gaia test-support target is
        # invisible to required-features-filtered workspace steps, so the
        # dedicated compile step must stay pinned.
        self.assertIn(
            "-p adapter-gaia --features test-support --all-targets --locked",
            macos_cli_steps,
        )
        self.assertIn(
            "rustup show active-toolchain",
            macos_cli_steps,
            "the macOS leg must run the toolchain pinned by rust-toolchain.toml",
        )

        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("name: required-gate", required_gate)
        self.assertIn("- cli-test", required_gate)
        self.assertIn('"cli-test:$CLI_TEST_RESULT"', required_gate)
        self.assertIn("- macos-cli-check", required_gate)
        self.assertIn(
            "MACOS_CLI_RESULT: ${{ needs.macos-cli-check.result }}",
            required_gate,
        )
        self.assertIn(
            '"macos-cli-check:$MACOS_CLI_RESULT"',
            required_gate,
            "macos-cli-check must enter the failure loop like cli-test "
            "(success|skipped accepted so path-filtered skips do not false-fail)",
        )

    def test_macos_cli_check_runs_on_main_push_and_merge_group(self):
        # macos-cli-check is the only leg that type-checks
        # #[cfg(target_os = "macos")] CLI code, and Linux cannot cover that risk
        # class for the Merge Queue combined tree. It must therefore run on
        # push(main) unconditionally (cumulative verification, independent of a
        # single push's paths-filter) and on the Merge Queue when the combined
        # diff touches cli_rust/rust_full.
        macos_cli = self.pr_workflow.split(
            "\n  macos-cli-check:", maxsplit=1
        )[1].split("\n  required-gate:", maxsplit=1)[0]
        # Round-46 review: the `needs: changes` wire is what feeds the MQ
        # branch's outputs — without it `needs.changes.outputs.*` evaluates
        # empty, the PR/MQ legs silently skip, and required-gate accepts
        # `skipped`. Same pin cli-lint and macos-rust-check carry.
        self.assertIn("needs: changes", macos_cli)
        self.assertIn("github.event_name == 'push' ||", macos_cli)
        self.assertIn("github.event_name == 'merge_group'", macos_cli)
        merge_group_branch = macos_cli.split(
            "github.event_name == 'merge_group'", maxsplit=1
        )[1].split("github.event_name == 'pull_request'", maxsplit=1)[0]
        self.assertIn(
            "needs.changes.outputs.cli_rust == 'true'",
            merge_group_branch,
            "the Merge Queue leg must be gated by cli_rust like the PR leg",
        )
        self.assertIn(
            "needs.changes.outputs.rust_full == 'true'",
            merge_group_branch,
        )
        # Draft gating only applies to the pull_request leg (merge_group and
        # push have no draft concept).
        pull_request_branch = macos_cli.split(
            "github.event_name == 'pull_request'", maxsplit=1
        )[1]
        self.assertIn("github.event.pull_request.draft == false", pull_request_branch)
        self.assertNotIn(
            "github.event.pull_request.draft",
            merge_group_branch,
        )

        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("name: required-gate", required_gate)
        self.assertIn("- macos-cli-check", required_gate)
        self.assertIn(
            "MACOS_CLI_RESULT: ${{ needs.macos-cli-check.result }}",
            required_gate,
        )

    def test_macos_cli_check_configures_the_sibling_restorable_cache(self):
        # Round-17 close-out, fixed in round 18: the job header claimed "no
        # macOS cache exists in this workflow that a refs/pull/N/merge run
        # could restore", which was false — macos-rust-check configures
        # exactly one, saved only on main, restorable by every PR through
        # rust-cache's prefix fallback (the workflow header's own documented
        # warm-cache design). This leg must keep that restorable treatment:
        # it is the long pole of the serialized macOS runner queue and every
        # uncached run pays a cold compile of the whole workspace.
        macos_cli = self.pr_workflow.split(
            "\n  macos-cli-check:", maxsplit=1
        )[1].split("\n  required-gate:", maxsplit=1)[0]
        self.assertIn("runs-on: macos-15", macos_cli)
        cache_step = _without_yaml_comments(macos_cli).split(
            "- name: Cargo cache", maxsplit=1
        )[1].split("- name: pinvou-cli macOS compile check", maxsplit=1)[0]
        self.assertIn("uses: Swatinem/rust-cache@v2", cache_step)
        self.assertIn("workspaces: pinvou-cli", cache_step)
        self.assertIn("shared-key: macos-cli-check", cache_step)
        self.assertIn(
            "save-if: ${{ github.ref == 'refs/heads/main' }}",
            cache_step,
            "PR-side runs must stay read-only on the 10GB quota; main is the "
            "sole writer of every warm cache in this workflow (same policy as "
            "macos-rust-check and cli-test)",
        )
        # The sibling-policy half of the round-18 decision, pinned so a future
        # "consolidation" cannot silently alias the keys: macOS artifacts are
        # not interchangeable with the Linux legs' (cli-test compiles the same
        # workspace but its cache is ~/.cargo-only under the standing incident
        # directive, and clippy artifacts differ from rustc ones anyway).
        self.assertNotIn("shared-key: macos-rust-check", cache_step)
        self.assertNotIn("shared-key: cli-test", cache_step)
        # Unlike the Linux legs this cache keeps the target directory: the
        # cache-targets: false directive exists because RESTORED target/
        # entries that needed linking took runners down, and a check-only leg
        # links nothing (rmeta-size artifacts). Regressing to false would
        # silently re-introduce the cold compile this round removed.
        self.assertNotIn("cache-targets:", cache_step)

    def test_cli_lint_job_is_wired_into_required_gate(self):
        # Round-18 review §3: "The CI leg doesn't lint the CLI" — no clippy on
        # any lane for ~50k lines while the src-tauri [lints] bans do not
        # apply to the pinvou-cli workspace. The new lint leg must satisfy the
        # same three-wiring rule as every other gate job (needs entry, env
        # backfill, summary-loop entry); removing the job or unwiring it from
        # required-gate must turn this suite red.
        body = _without_yaml_comments(self.pr_workflow)
        cli_lint = body.split("\n  cli-lint:", maxsplit=1)[1].split(
            "\n  windows-rust-test:", maxsplit=1
        )[0]
        self.assertIn("needs: changes", cli_lint)
        self.assertIn("runs-on: ubuntu-22.04", cli_lint)
        # The trigger set must mirror cli-test exactly: main push (cumulative,
        # paths-filter independent), the Merge Queue combined tree gated by
        # cli_rust/rust_full, and ready non-draft PRs (drafts skip the heavy
        # leg).
        self.assertIn("github.event_name == 'push' ||", cli_lint)
        self.assertIn("github.event_name == 'merge_group'", cli_lint)
        self.assertIn(
            "needs.changes.outputs.cli_rust == 'true'", cli_lint
        )
        self.assertIn(
            "needs.changes.outputs.rust_full == 'true'", cli_lint
        )
        self.assertIn(
            "github.event.pull_request.draft == false",
            cli_lint,
            "draft PRs must skip the lint leg like the other heavy CLI jobs",
        )
        # Fail-closed on compile errors and nothing else may soften it: the
        # warn-visible clippy policy (debt cleanup pending the [lints] table,
        # see the job comment) must never grow a bypass here.
        self.assertNotIn("continue-on-error", cli_lint)
        self.assertIn("components: clippy", cli_lint)
        self.assertIn(
            "cargo clippy --manifest-path pinvou-cli/Cargo.toml",
            cli_lint,
            "the CLI lint leg must run clippy via the same --manifest-path "
            "convention as every other CLI leg",
        )
        # --all-targets: tests are linted too; --no-deps: dependencies and the
        # CodeWhale submodule are never linted; --locked like every CLI build.
        self.assertIn("--workspace --all-targets --no-deps --locked", cli_lint)
        # Round-27 review: the warn-visible clippy policy can be defeated by
        # APPENDING lint suppressions after the pinned prefix (`-- -A
        # clippy::all`, `--cap-lints`, a RUSTFLAGS export in the same step).
        # Substring asserts above cannot see an appended tail, so pin the
        # absence of the known bypasses explicitly.
        self.assertNotIn("-A clippy", cli_lint)
        self.assertNotIn("-Aclippy", cli_lint)
        self.assertNotIn("--allow clippy", cli_lint)
        self.assertNotIn("--cap-lints", cli_lint)
        self.assertNotIn("RUSTFLAGS", cli_lint)
        # The featureless build (product-backend off) is exercised nowhere
        # else — cargo test always runs default features — so without this
        # check step the `#[cfg(not(feature = "product-backend"))]` refusal
        # arms in the cli crate could rot silently.
        self.assertIn(
            "cargo check --manifest-path pinvou-cli/Cargo.toml",
            cli_lint,
        )
        self.assertIn(
            "--workspace --all-targets --no-default-features --locked",
            cli_lint,
            "cli-lint must keep compiling the featureless build — lib, bins, "
            "and test targets alike — so the product-backend-off cfg arms "
            "cannot rot silently",
        )
        # Round-42 review: the substring pin above is satisfied by an
        # APPENDED feature toggle — `--no-default-features --features
        # product-backend` keeps the pinned text while compiling the
        # feature-on config, silently un-guarding the refusal arms. Pin the
        # absence of the re-enable on the featureless step.
        # Round-48 review: the steps are folded (`>-`) multi-line run blocks,
        # and the per-LINE check below let the toggle hide on a DIFFERENT
        # line of the same command (cargo accepts `--features` alongside
        # `--no-default-features`), satisfying every assertion while
        # compiling the feature-on config. Fold each run block into one
        # string first; the message keeps the whole block for diagnosis.
        # Round-49 review: the original fold pattern hard-coded a 12-space
        # continuation indent and matched ZERO blocks in this workflow,
        # silently degrading every run to the per-line fallback. The pattern
        # now captures the first continuation line's indent and requires the
        # block's remaining lines to share it, and the fold shape itself is
        # mandatory: a differently-shaped step must fail loudly here instead
        # of reviving the per-line blind spot.
        featureless_steps = [
            " ".join(block.split("\n"))
            for block, _indent in re.findall(
                r"run: >-\n(( +)[^\n]*\n?(?:\2[^\n]*\n?)*)", cli_lint
            )
            if "--no-default-features" in block
        ]
        self.assertTrue(
            featureless_steps,
            "the featureless check step must exist as a folded (>-) run "
            "block; update this pin if the step's YAML shape changes, the "
            "fold is what keeps a sibling-line `--features` toggle visible",
        )
        for block in featureless_steps:
            self.assertNotIn(
                "--features",
                block,
                # Round-43 review: this message interpolates the offending
                # text — without the f-prefix a real failure printed a
                # literal "{block}" instead of the step.
                "the featureless check must not re-enable features on the "
                "same invocation (on any line of the folded run block): "
                "`--no-default-features --features "
                "product-backend` satisfies the substring pin while "
                f"compiling the feature-on config: {block}",
            )
        # Secondary net over any unfolded single-line invocation of the same
        # check (defense in depth; the folded assertion above is the
        # load-bearing one).
        for line in cli_lint.splitlines():
            if "--no-default-features" in line:
                self.assertNotIn(
                    "--features",
                    line,
                    "the featureless check must not re-enable features on "
                    f"the same line: {line}",
                )
        # Independent cache keyed to the compiler mode (clippy-driver
        # artifacts are not reusable by the rustc test compilers — same
        # parallel-job split as rust-lint vs rust-test).
        self.assertIn("shared-key: cli-lint", cli_lint)
        self.assertIn(
            "save-if: ${{ github.ref == 'refs/heads/main' }}",
            cli_lint,
        )

        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("name: required-gate", required_gate)
        self.assertIn("- cli-lint", required_gate)
        self.assertIn("CLI_LINT_RESULT: ${{ needs.cli-lint.result }}", required_gate)
        self.assertIn(
            '"cli-lint:$CLI_LINT_RESULT"',
            required_gate,
            "cli-lint must enter the failure loop like cli-test "
            "(success|skipped accepted so path-filtered skips do not "
            "false-fail)",
        )

    def test_required_gate_accepts_only_success_or_skipped(self):
        # Round-28: the `case "$result" in success|skipped) ;;` line is the
        # ONE predicate deciding whether a failed leg blocks the gate — the
        # membership assertions above only pin loop ENTRIES. Widening the
        # pattern to a catch-all (`*) ;;` without failed=1, or adding
        # `failure` to the accepted set) would accept every failed gate
        # while every other test here stays green, so pin the exact
        # accepted set and forbid a bare catch-all accept.
        body = _without_yaml_comments(self.pr_workflow)
        required_gate = body.split("\n  required-gate:", maxsplit=1)[1]
        self.assertIn(
            "success|skipped) ;;",
            required_gate,
            "the accepted set must stay exactly success|skipped",
        )
        self.assertNotIn(
            "*) ;;",
            required_gate,
            "a catch-all case arm would accept every failed gate result",
        )
        for soft in ("failure) ;;", "cancelled) ;;"):
            self.assertNotIn(
                soft,
                required_gate,
                f"{soft} in the accepted set would let a failed leg pass",
            )
        # The verdict line itself is the last thing that can be disarmed
        # (mutation-checked: `[[ "$failed" -eq 0 ]] || true` passed this
        # whole suite before this pin existed). The bare `[[ ]]` must stay
        # bare so a nonzero count fails the step.
        self.assertTrue(
            required_gate.rstrip().endswith('[[ "$failed" -eq 0 ]]'),
            "required-gate must end in the bare failed-count verdict",
        )
        for suffix in (
            "[[ \"$failed\" -eq 0 ]] || true",
            "[[ \"$failed\" -eq 0 ]] || :",
            "[[ \"$failed\" -eq 0 ]] || exit 0",
        ):
            self.assertNotIn(
                suffix,
                required_gate,
                "a swallowed verdict would accept every failed gate while "
                "every other test here stays green",
            )

    def test_no_job_level_continue_on_error_disarms_a_gate_job(self):
        # The workflow header states this policy in prose (no gate job
        # carries a job-level continue-on-error) with nothing enforcing it,
        # and it is the cheapest
        # fail-open vector in the file: a job-level `continue-on-error: true`
        # makes the job's own failure non-blocking AND makes
        # `needs.<job>.result` report `success`, so required-gate's
        # `success|skipped` loop accepts it. One line would silently disarm
        # cli-test, windows-rust-test, macos-cli-check or rust-test while the
        # required check stays green.
        #
        # Indentation is the discriminator: job keys sit at 4 spaces
        # (`  <job>:` + `    runs-on:`), step keys at 8 (`      - name:` +
        # `        continue-on-error:`). Comments are stripped first so the
        # header's prose and the "no continue-on-error" annotations on the
        # clippy gate do not count as settings.
        body = _without_yaml_comments(self.pr_workflow)
        job_level = []
        step_level = []
        for number, line in enumerate(body.splitlines(), start=1):
            if not line.strip().startswith("continue-on-error"):
                continue
            indent = len(line) - len(line.lstrip(" "))
            (job_level if indent <= 4 else step_level).append((number, line.strip()))
        self.assertEqual(
            job_level,
            [],
            "a job-level continue-on-error makes needs.<job>.result report "
            "'success' to required-gate, so the whole compile/test leg becomes "
            "advisory while the required check stays green",
        )
        # Exactly one legitimate use, and it is a STEP whose entire purpose is
        # to print an analysis it must not be able to fail the job with.
        self.assertEqual(
            len(step_level),
            1,
            "only the Windows PE import diagnostic may opt out of blocking; "
            f"found {len(step_level)} continue-on-error settings: {step_level}",
        )
        diagnostic_step = body.split(
            "- name: Windows 测试二进制导入诊断", maxsplit=1
        )[1].split("\n      - name:", maxsplit=1)[0]
        self.assertIn(
            "continue-on-error: true",
            diagnostic_step,
            "the single permitted continue-on-error must be the Windows PE "
            "import diagnostic step, not some other step that moved under it",
        )

    def test_gate_jobs_do_not_swallow_their_exit_status(self):
        # The companion fail-open vector to continue-on-error: appending
        # `|| true` (or `|| :`) to a gate command leaves the step, the job and
        # required-gate all green while the compiler or the test binary
        # actually failed. The workflow uses `|| echo "::warning::..."` for its
        # one genuinely best-effort step (ci-memory-setup), which is a
        # different and deliberate shape, so scanning the gate jobs for the
        # unconditional-success idioms has no false positives today.
        gate_jobs = (
            "rust-test",
            "rust-lint",
            "cli-test",
            "cli-lint",
            "windows-rust-test",
            "macos-rust-check",
            "macos-cli-check",
            "knowledge-rust",
        )
        lines = self.pr_workflow.splitlines()
        job_header = re.compile(r"^  (\S.*):\s*$")
        for name in gate_jobs:
            starts = [
                number
                for number, line in enumerate(lines)
                if job_header.match(line) and job_header.match(line).group(1) == name
            ]
            self.assertEqual(
                len(starts), 1, f"expected exactly one {name} job definition"
            )
            start = starts[0]
            # The block runs to the next job header (2-space key), which is the
            # only thing that can end a job in this file.
            end = next(
                (
                    number
                    for number in range(start + 1, len(lines))
                    if job_header.match(lines[number])
                ),
                len(lines),
            )
            block = _without_yaml_comments("\n".join(lines[start:end]))
            # main's #642 lld probe prints its probe errors with
            # `cat ... >&2 || true` inside the branch that then exits 1, so
            # the idiom there cannot mask a gate command's status. Excise
            # exactly that diagnostic line before the scan; every other
            # occurrence of the idioms below still fails the pin.
            block = "\n".join(
                line
                for line in block.splitlines()
                if 'cat "$probe/a.err" "$probe/b.err" >&2 || true' not in line
            )
            # Round-42 review: the `|| echo` scan exempts exactly ONE
            # documented best-effort step per job — the ci-memory-setup
            # (zram) invocation, whose failure legitimately warns instead of
            # failing the leg. The exemption is BY NAME: the excised line
            # must invoke the setup script, so a new `|| echo` on any other
            # line still fails the pin, and if the setup step is renamed the
            # exemption stops matching and its `|| echo` becomes a failure
            # that must be re-adjudicated.
            setup_lines = [
                line
                for line in block.splitlines()
                if "|| echo" in line and "ci-memory-" in line
            ]
            self.assertLessEqual(
                len(setup_lines),
                1,
                f"{name}: at most one ci-memory-setup line may carry `|| echo`",
            )
            block = "\n".join(
                line for line in block.splitlines() if line not in setup_lines
            )
            # Round-42 review: the workflow's own best-effort idiom
            # (`|| echo "::warning::…"`) belongs in this scan too — appended
            # to a gate command it turns any failure into a warning line and
            # a green step, exactly like `|| true`. Each gate command line
            # must not carry ANY `||` redirect of its exit status; the two
            # documented exceptions stay pinned to their own lines (the
            # lld-probe diagnostic above and the ci-memory-setup step, whose
            # name is asserted on main).
            for idiom in ("|| true", "|| :", "|| exit 0", "|| echo"):
                self.assertNotIn(
                    idiom,
                    block,
                    f"{name} must not swallow a command's exit status with "
                    f"'{idiom}': the leg would report success on a real "
                    "compile or test failure and required-gate would accept it",
                )

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
            'sudo --preserve-env=GITHUB_ACTIONS,PINVOU3_CI_DISABLE_ZRAM bash'
            ' "${{ github.workspace }}/scripts/ci-memory-setup.sh"',
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
                "run: timeout --kill-after=15 240 sudo"
                " --preserve-env=GITHUB_ACTIONS,PINVOU3_CI_DISABLE_ZRAM bash"
                ' "${{ github.workspace }}/scripts/ci-memory-setup.sh"',
                job,
                f"ubuntu job '{job_name}' must hard-cap ci-memory-setup with"
                " 'timeout --kill-after=15 240' (userspace hang backstop) and"
                " pass GITHUB_ACTIONS and PINVOU3_CI_DISABLE_ZRAM through sudo",
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
        # the `timeout --kill-after=15 240` cap, the sudo env pass-through
        # and the non-fatal ::warning degradation on the same run line.
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
                # sudo's default env_reset strips GITHUB_ACTIONS (the hosted
                # images keep no env_keep for it), which silently disables
                # the script's ::warning annotations, and strips the
                # PINVOU3_CI_DISABLE_ZRAM opt-out when a workflow sets it
                # through `env:`. Every call site must pass both through.
                self.assertIn(
                    "sudo --preserve-env=GITHUB_ACTIONS,PINVOU3_CI_DISABLE_ZRAM"
                    ' bash "${{ github.workspace }}/scripts/ci-memory-setup.sh"',
                    line,
                    f"{workflow.name}: the ci-memory-setup.sh call must pass"
                    " GITHUB_ACTIONS and PINVOU3_CI_DISABLE_ZRAM through sudo"
                    " (env_reset would strip them, silently disabling the"
                    " ::warning annotations and the zram opt-out)",
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

    def test_memory_setup_script_pins_annotations_and_honest_degradation(self):
        # Until now only the workflow call structure was pinned; the script
        # content itself had no coverage, so a regression back to a
        # misleading "kept the previous swap" message or to annotating
        # every recoverable slow path would stay green. Pin three anchor
        # groups: zswap disabled behind zram (exactly one compress-in-RAM
        # layer), single-line ::warning annotations, and honest failure text.
        source = (ROOT / "scripts" / "ci-memory-setup.sh").read_text(encoding="utf-8")
        # With zram active, zswap must be disabled explicitly; stock Ubuntu
        # kernels enable it and it would compress every swapped page twice.
        self.assertIn("echo 0 >/sys/module/zswap/params/enabled", source)
        # Degradations become step annotations in Actions, and the payload
        # must fold newlines and CRs: workflow commands are single-line, and
        # the runner's .NET line reader also treats a lone CR as a line
        # terminator (which could forge a second workflow command).
        self.assertIn('echo "::warning::[memory-setup] ${*//[', source)
        self.assertIn("${*//[$'\\r\\n']/ }", source)
        # Slow paths that normally recover (first modprobe miss, the image
        # swap pre-activation, the as-is swapon retry and the last-resort
        # image swapfile after all layers) stay log-only; annotating them
        # would leave a standing warning on every job and dilute real ones.
        # The function body is pinned verbatim: turning it back into a
        # ::warning:: annotation turns this red.
        self.assertIn(
            'warn_recoverable() { echo "[memory-setup] WARNING: $*" >&2; }',
            source,
        )
        for recoverable in (
            'warn_recoverable "modprobe zram failed${modprobe_err:+: ${modprobe_err}};',
            'warn_recoverable "no swap active; activating the image swapfile'
            ' before the slow module install"',
            'warn_recoverable "swapon ${cand} failed as is; trying chmod 600'
            ' + mkswap + swapon once"',
            'warn_recoverable "no active swap after all layers; trying the'
            ' image-provided swapfile"',
        ):
            with self.subTest(recoverable=recoverable):
                self.assertIn(recoverable, source)
        # A failed disk-swap rebuild must carry its diagnostics, and
        # removed_note may only be set inside the branch where rm actually
        # succeeded (an unconditional assignment makes the guard dead code).
        self.assertIn(
            "if rm -f /mnt/swapfile; then\n"
            '        removed_note=" (the previous /mnt/swapfile was removed)"\n'
            "      fi",
            source,
        )
        self.assertNotIn("keeping the existing swap configuration", source)

    def test_windows_rust_test_parallel_phases_preserve_routing_and_coverage(self):
        # The all-target check and the linked regressions run as two matrix
        # legs of one required job. Splitting the compile graphs must not
        # turn either leg into an optional check, add a cache writer, or drop
        # a step from its leg.
        windows_rust_test = _without_yaml_comments(
            self.pr_workflow.split("\n  windows-rust-test:", maxsplit=1)[1].split(
                "\n  macos-rust-check:", maxsplit=1
            )[0]
        )
        self.assertIn("name: windows-rust-test (${{ matrix.phase }})", windows_rust_test)
        self.assertIn("fail-fast: false", windows_rust_test)
        self.assertIn("max-parallel: 2", windows_rust_test)
        self.assertIn("phase: [all-targets-check, regression]", windows_rust_test)
        # Routing is job level, so it applies to both legs unchanged.
        job_if = windows_rust_test.split("\n    if: >-", 1)[1].split("\n    strategy:", 1)[0]
        self.assertNotIn("matrix.phase", job_if)

        steps = re.split(r"\n      - name: ", windows_rust_test)[1:]
        phase_of = {}
        for step in steps:
            name = step.split("\n", 1)[0].strip()
            # Round-46 review: end-anchored, so the exact silent-skip
            # mutation this map advertises against — appending `&& false`
            # AFTER the closing braces — no longer still extracts the phase
            # name and passes. (`}} && false` suffix = the step skips while
            # the map reports it routed.)
            match = re.search(
                r"\n        if: \$\{\{ matrix\.phase == '([a-z-]+)' \}\}[ \t]*(?:\n|$)",
                step,
            )
            phase_of[name] = match.group(1) if match else None
        expected = {
            "Windows Rust 全目标检查": "all-targets-check",
            "pinvou-cli Windows compile check": "all-targets-check",
            "pinvou-cli Windows compile check (adapter-gaia test-support)": "all-targets-check",
            "Windows Rust 单元测试链接检查": "regression",
            "Windows 测试 exe 嵌入 Common-Controls v6 清单": "regression",
            "Windows 测试二进制导入诊断": "regression",
            "CodeWhale Windows PowerShell regressions": "regression",
            "Windows 原子替换状态机回归": "regression",
            # Round-44 review: the step carries all the CLI workspace's
            # Windows-gated pins, so its routing condition is load-bearing —
            # an `if: false` (or an unmatchable condition) would silently
            # skip it while every text pin below stays green.
            "pinvou-cli Windows-gated unit tests": "regression",
            # Shared setup runs on both legs.
            "初始化公共底座 submodule": None,
            "Cargo cache": None,
            "Windows Rust cache baseline diagnostics": None,
        }
        for name, phase in expected.items():
            with self.subTest(step=name):
                self.assertIn(name, phase_of)
                self.assertEqual(phase_of[name], phase)
        # Round-45 review: `phase_of` is a dict keyed by step name, so a
        # second step with the same name would shadow the pinned one and
        # keep every assertion above green while the real step's routing
        # `if` is flipped (the silent-skip bypass). Each pinned name must
        # appear exactly once.
        step_names = [
            step.split("\n", 1)[0].strip()
            for step in re.split(r"\n      - name: ", windows_rust_test)[1:]
        ]
        duplicated = {
            name for name in step_names if step_names.count(name) > 1
        }
        self.assertEqual(
            duplicated,
            set(),
            "duplicate step names shadow the phase map; the routing pins "
            f"cannot hold: {sorted(duplicated)}",
        )
        self.assertIn("--all-targets --features dev-tools", windows_rust_test)
        self.assertIn("--lib --no-run --locked --features benchmark-hooks --message-format=json",
                      windows_rust_test,
                      "round-45 review: the regression link check must compile the "
                      "benchmark-hooks contract module, or the round-44 filter for "
                      "windows_attachment_runtime_stays_security_gated matches 0 tests")

        # Both legs restore one established namespace; only regression on
        # main may save, so there is no second writer or new key.
        self.assertEqual(windows_rust_test.count("shared-key:"), 1)
        self.assertIn("shared-key: windows-rust-test", windows_rust_test)
        self.assertIn(
            "save-if: ${{ matrix.phase == 'regression' && "
            "github.ref == 'refs/heads/main' }}",
            windows_rust_test,
        )
        self.assertIn("WINDOWS_RUST_CACHE", windows_rust_test)
        self.assertIn("WINDOWS_RUST_TIMING", windows_rust_test)
        self.assertNotIn("actions/setup-node", windows_rust_test)

    def test_windows_regression_loop_pins_the_round43_and_round44_app_filters(self):
        # Round-44 review: six app-side cfg(windows) tests ran on no leg —
        # three in the dependencies platform module, two in codex_acp's
        # platform module, and the headless-bridge attachment security gate
        # (whose cfg(not(windows)) sibling DOES run on Linux rust-test). The
        # round-43 additions to this loop were themselves never pinned by
        # the policy suite, so a filter deletion would have gone unpunished.
        # Pin the loop's round-43/44 additions and its zero-match guard;
        # deleting one must fail the suite instead of silently orphaning the
        # pins again.
        windows_rust_test = _without_yaml_comments(
            self.pr_workflow.split("\n  windows-rust-test:", maxsplit=1)[1].split(
                "\n  macos-rust-check:", maxsplit=1
            )[0]
        )
        for needle in [
            "'failed_probe_never_deletes_an_existing_store_windows' \\",
            "'features::voice_shortcut::platform::tests::' \\",
            "'app::commands::dependencies::platform::windows::tests::' \\",
            "'features::codex_acp::platform::windows::tests::' \\",
            # Round-47 review: the 22 cfg(windows)-only tests under
            # platform::os::windows (windows_path 18 + windows_system 4)
            # executed on NO leg — the orphan class rounds 42-44 claimed
            # eliminated. The module filter runs them here; deleting it
            # re-orphans them, so it is pinned like its siblings.
            "'platform::os::windows::' \\",
            "'windows_attachment_runtime_stays_security_gated'; do",
            # The loop's zero-match guard is load-bearing (a renamed test
            # must fail the step, not pass silently).
            "回归过滤器 '$filter' 匹配 0 个测试",
        ]:
            self.assertIn(needle, windows_rust_test)

    def test_memory_setup_disk_swap_is_mandatory(self):
        # Disk swap is mandatory (2026-09-19): with the zram pool capped at
        # 70% of RAM, the 8G /mnt swapfile is the only unbounded overflow
        # layer. A leftover opt-in switch turns this red; the main flow must
        # call setup_disk_swap unconditionally (top level, no indentation)
        # and the 8G size is pinned. The 2026-10 degraded-mode branch (no
        # zram swap active) doubles the size to 16G — the sole overflow layer
        # behind the ~14.2GB rust-test link peaks — and the flaky
        # linux-modules-extra install retries once; both behaviors are
        # pinned so the hardening cannot silently regress.
        source = (ROOT / "scripts" / "ci-memory-setup.sh").read_text(encoding="utf-8")
        self.assertNotIn("PINVOU3_CI_ENABLE_DISK_SWAP", source)
        self.assertIn("DISK_SWAP_SIZE_KIB=$((8 * 1024 * 1024))", source)
        self.assertIn("DISK_SWAP_SIZE_KIB=$((16 * 1024 * 1024))", source)
        self.assertIn(
            "swapon --show=NAME --noheadings 2>/dev/null | grep -q '/dev/zram'",
            source,
        )
        self.assertIn(
            'warn "apt-get install ${modules_pkg} failed once; retrying"',
            source,
        )
        self.assertIn(
            'warn "apt-get install ${modules_pkg} failed again; giving up on zram layers"',
            source,
        )
        self.assertIn(
            'log "provisioning the mandatory /mnt disk swap"\nsetup_disk_swap',
            source,
        )

    def test_fetch_connectors_verifies_checksums_with_loud_diagnostics(self):
        # verify_file is called bare under `set -e` from three places (the
        # --check gate, the post-download recheck and the pre-install binary
        # check); a silent mismatch leaves the gate with nothing but
        # "exit code 1". The diagnostic lives inside verify_file so no call
        # site can drop it; the pre-download probe silences it explicitly.
        source = (ROOT / "scripts" / "fetch-connectors.sh").read_text(encoding="utf-8")
        self.assertIn(
            'echo "sha256 mismatch: $1 (expected $2, actual $(compute_sha256 "$1"))" >&2',
            source,
        )
        self.assertIn('echo "sha256 check failed: $1 is missing (expected $2)" >&2', source)
        self.assertIn(
            'echo "sha256 check failed: $1 is not readable (expected $2)" >&2', source
        )
        self.assertIn('[[ "$(compute_sha256 "$1")" == "$2" ]]', source)
        self.assertIn("return 1", source)
        # Three bare checks plus the silenced pre-download probe: the number
        # of verification points must not shrink quietly.
        self.assertEqual(
            source.count("verify_file "),
            4,
            "fetch-connectors.sh must keep verifying: --check gate,"
            " pre-download probe, post-download recheck and pre-install"
            " binary check",
        )
        self.assertIn('verify_file "$archive" "$expected" >/dev/null 2>&1', source)

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
        # 90-minute cap while passing runs already took 85-87 minutes; the
        # 2026-10-01 stable rollover to 1.99.0 invalidated the rust-cache key
        # so PR legs recompiled both workspaces cold and the link step alone
        # consumed 180 minutes (run 36919362322).
        self.assertIn("timeout-minutes: 240", windows_rust_test)

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
            "        if: ${{ matrix.phase == 'regression' }}\n"
            "        shell: bash\n"
            "        run: |",
            windows_rust_test,
        )
        self.assertIn(
            "- name: Windows 测试 exe 嵌入 Common-Controls v6 清单\n"
            "        if: ${{ matrix.phase == 'regression' }}\n"
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
        self.assertIn("name: required-gate", required_gate)
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
        self.assertIn("name: required-gate", required_gate)
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
        self.assertIn("name: required-gate", required_gate)
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
        # The VC++ temp-preflight pins inside windows_runtime_packaging_contract.test.js
        # target src-tauri/packaging/windows/nsis/vcredist-temp-preflight.ps1, so edits
        # to that file must trigger the only job that runs the pins.
        self.assertIn(
            "pinvou3-app/src-tauri/packaging/windows/nsis/**",
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
        self.assertIn("name: required-gate", required_gate)
        self.assertIn("- windows-codex-runtime-test", required_gate)

    def test_windows_rustup_repair_runs_in_required_native_job(self):
        changes = self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
            "\n  fast-gate:", maxsplit=1
        )[0]
        self.assertIn(
            "windows_rustup_repair: ${{ steps.filter.outputs.windows_rustup_repair }}",
            changes,
        )
        repair_filter = re.search(
            r"\n            windows_rustup_repair:\n((?:              .*(?:\n|$))+)",
            changes,
        ).group(1)
        for trigger in (
            ".github/workflows/pr-check.yml",
            "pinvou3-app/scripts/ci/**",
            "pinvou3-app/scripts/tauri/build.js",
            "pinvou3-app/tests/windows_rustup_repair_smoke.ps1",
            "pinvou3-app/tests/windows_rust_toolchain_contract.test.js",
            "pinvou3-app/src-tauri/rust-toolchain.toml",
            "pinvou3-app/package.json",
        ):
            self.assertIn(trigger, repair_filter)

        job_body = self.pr_workflow.split(
            "\n  windows-rustup-repair-test:", maxsplit=1
        )[1]
        job = re.split(r"\n  [a-zA-Z]", job_body, maxsplit=1)[0]
        self.assertIn("needs: changes", job)
        self.assertIn("needs.changes.outputs.windows_rustup_repair == 'true'", job)
        self.assertIn("runs-on: windows-latest", job)
        self.assertIn("npm --prefix pinvou3-app run test:windows-rustup-repair", job)
        # scripts/ci/** is in no node-test filter, so this job is the only
        # gate that pins ensure-rust-toolchain.ps1 through the node contract
        # test. Dropping the step would silently unpin the repair engine.
        self.assertIn(
            "node --test pinvou3-app/tests/windows_rust_toolchain_contract.test.js",
            job,
        )
        # The contract test must run before the smoke: it fails in seconds on
        # engine drift, while the smoke pays a real toolchain download first.
        self.assertLess(
            job.index(
                "node --test pinvou3-app/tests/windows_rust_toolchain_contract.test.js"
            ),
            job.index("npm --prefix pinvou3-app run test:windows-rustup-repair"),
        )

        # Same three-wiring rule as the other native legs: needs entry, env
        # backfill, and summary-loop entry.
        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("- windows-rustup-repair-test", required_gate)
        self.assertIn(
            "WINDOWS_RUSTUP_REPAIR_RESULT: "
            "${{ needs.windows-rustup-repair-test.result }}",
            required_gate,
        )
        self.assertIn(
            '"windows-rustup-repair-test:$WINDOWS_RUSTUP_REPAIR_RESULT"',
            required_gate,
        )

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

    def test_mac_bundle_smoke_is_gated_on_bundle_chain_paths(self):
        # mac-build.yml's bundle_chain filter (before its 2026-10 fold into
        # pr-check's macos-rust-check) decided when to append the universal
        # bundle smoke. After the fold the filter moved into pr-check's
        # changes job, and the smoke step runs only on push with a
        # bundle_chain hit; this test pins: the filter entries are complete
        # (one missing = packaging-chain changes silently skip the smoke),
        # the smoke step consumes that output, and the PR/Queue side does not
        # run it (the PR side gets the lightweight contract from
        # release-contract-test, and a VERSION bump gets full dmg builds from
        # release-packages). VERSION is deliberately absent from the filter:
        # a real version-sync commit always touches
        # tauri.conf.json/package.json, entering bundle_chain through them.
        changes = _without_yaml_comments(
            self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
                "\n  fast-gate:", maxsplit=1
            )[0]
        )
        self.assertIn("bundle_chain:", changes)
        bundle_chain_paths = _extract_quoted_paths(
            changes.split("            bundle_chain:", maxsplit=1)[1]
        )
        self.assertTrue(bundle_chain_paths, "bundle_chain parsed to an empty path list")
        for entry in (
            "pinvou3-app/scripts/tauri/**",
            "pinvou3-app/src-tauri/tauri.conf.json",
            "pinvou3-app/src-tauri/config/**",
            "pinvou3-app/src-tauri/packaging/**",
            "pinvou3-app/src-tauri/resources/**",
            "pinvou3-app/package.json",
            "pinvou3-app/package-lock.json",
            "pinvou3-app/vite.config.mjs",
        ):
            self.assertIn(
                entry,
                bundle_chain_paths,
                f"bundle_chain is missing the packaging-chain entry; this path "
                f"changing would silently skip the bundle smoke: {entry}",
            )

        macos_job = self.pr_workflow.split(
            "\n  macos-rust-check:", maxsplit=1
        )[1].split("\n  windows-codex-runtime-test:", maxsplit=1)[0]
        smoke_step = _without_yaml_comments(
            macos_job.split(
                "- name: Tauri bundle smoke", maxsplit=1
            )[1].split("\n      - name:", maxsplit=1)[0]
        )
        self.assertIn(
            "github.event_name == 'push' && needs.changes.outputs.bundle_chain == 'true'",
            smoke_step,
            "the bundle smoke must run only on push and only for packaging-chain changes",
        )
        self.assertIn(
            "node scripts/tauri/build.js build --target universal-apple-darwin",
            smoke_step,
        )
        self.assertNotIn(
            "continue-on-error",
            smoke_step,
            "the universal bundle smoke must be able to fail main "
            "(a broken macos packaging chain must be red)",
        )

    def test_macos_rust_check_routes_by_rust_filters_not_frontend_paths(self):
        # PR/MQ-side routing: the macos job's if must not consume the
        # frontend/pet outputs (it is a rust gate; frontend changes are
        # validated by frontend-test). On main push the folded mac legs run
        # path-independently (the cumulative main-push contract,
        # frontend-only pushes included), so this pin is about PR routing,
        # not push. The bundle_chain filter must also not treat
        # pure-frontend src/** as a packaging-chain change — but
        # package.json/package-lock.json must stay bundle_chain triggers (the
        # lockfile affects the build). Formerly pinned the standalone
        # mac-build.yml trigger; the workflow was folded into pr-check's
        # macos-rust-check in 2026-10.
        macos_job = self.pr_workflow.split(
            "\n  macos-rust-check:", maxsplit=1
        )[1].split("\n  windows-codex-runtime-test:", maxsplit=1)[0]
        job_if = macos_job.split("\n    if: >-", maxsplit=1)[1].split(
            "\n    runs-on:", maxsplit=1
        )[0]
        for output in ("frontend", "pet"):
            self.assertNotIn(
                f"needs.changes.outputs.{output}",
                job_if,
                f"macos-rust-check is a rust gate; {output} changes must not trigger it",
            )
        # Positive shape: push must stay unconditional and the PR branch must
        # keep the rust-filter routing. Without this, collapsing the whole if
        # to a bare push check deletes PR-side macOS coverage (the only leg
        # that runs macOS-only unit tests on PRs) while every negative pin
        # and step-level pin stays green.
        self.assertIn(
            "github.event_name == 'push' ||",
            job_if,
            "a main push must enter this job unconditionally "
            "(the path-independent cumulative coverage contract)",
        )
        self.assertIn(
            "needs.changes.outputs.rust_full == 'true'",
            job_if,
            "the PR side must keep the rust-filter routing (high-risk draft/ready paths)",
        )

        changes = _without_yaml_comments(
            self.pr_workflow.split("\n  changes:", maxsplit=1)[1].split(
                "\n  fast-gate:", maxsplit=1
            )[0]
        )
        bundle_tail = changes.split("            bundle_chain:", maxsplit=1)[1]
        # bundle_chain is the last filter group and the extraction below has
        # no lower bound: a group appended after it would leak into the
        # extracted paths (the exact-entry pins would keep passing until a
        # negation false-fails). Pin the boundary instead.
        leaked_groups = [
            line
            for line in bundle_tail.splitlines()[1:]
            if re.fullmatch(r"            [A-Za-z0-9_-]+:", line)
        ]
        self.assertEqual(
            [], leaked_groups,
            "a new filter group after bundle_chain must re-bound this extraction",
        )
        bundle_chain_paths = _extract_quoted_paths(bundle_tail)
        self.assertNotIn(
            "pinvou3-app/src/**",
            bundle_chain_paths,
            "pure-frontend src/** must not enter bundle_chain (avoids needless native builds)",
        )
        self.assertIn("pinvou3-app/package.json", bundle_chain_paths)
        self.assertIn("pinvou3-app/package-lock.json", bundle_chain_paths)

    def test_macos_native_regression_legs_survived_the_mac_build_fold(self):
        # mac-build.yml was deleted in 2026-10 and its push-only coverage was
        # folded into macos-rust-check. Its old failure mode was silent: the
        # workflow kept "running" (cancel-failing) for weeks with zero
        # coverage. Pin each folded leg so removing one cannot go unnoticed.
        macos_job = self.pr_workflow.split(
            "\n  macos-rust-check:", maxsplit=1
        )[1].split("\n  windows-codex-runtime-test:", maxsplit=1)[0]
        # mac-build died inside its own 50-minute cap; the fold replaces it
        # with 180. Reverting to the default (360) or deleting the cap burns
        # a hung runner for hours — the exact waste this PR removes.
        self.assertIn("timeout-minutes: 180", macos_job)

        # Full native lib regression: push-only, serial threads, locked.
        # Comment-stripped: an if-line deleted but kept "alive" in a YAML
        # comment must not satisfy the gate.
        test_step = _without_yaml_comments(
            macos_job.split(
                "- name: macOS full lib tests", maxsplit=1
            )[1].split("\n      - name:", maxsplit=1)[0]
        )
        self.assertIn(
            "if: ${{ github.event_name == 'push' }}", test_step
        )
        self.assertIn(
            "cargo test --lib --features benchmark-hooks --locked -- --test-threads=1",
            test_step,
        )
        # Same silent-no-op guard as the PR leg's computer_use run: a
        # filterless suite that somehow runs zero tests must fail the push.
        self.assertIn("running [1-9][0-9]* tests?", test_step)

        # The universal build needs both darwin targets installed; dropping
        # this step resurfaces as MODULE/target errors on the first
        # bundle_chain push, weeks after the fold (same class as the npm
        # provisioning miss the review caught).
        targets_step = _without_yaml_comments(
            macos_job.split(
                "- name: Install both targets (universal bundle smoke, push only)",
                maxsplit=1,
            )[1].split("\n      - name:", maxsplit=1)[0]
        )
        self.assertIn("github.event_name == 'push'", targets_step)
        self.assertIn(
            "rustup target add aarch64-apple-darwin x86_64-apple-darwin",
            targets_step,
        )

        # The release-fast compile smoke and the verify script stay push-only.
        release_fast = _without_yaml_comments(
            macos_job.split(
                "- name: Cargo build (release-fast", maxsplit=1
            )[1].split("\n      - name:", maxsplit=1)[0]
        )
        self.assertIn("github.event_name == 'push'", release_fast)
        self.assertIn(
            "cargo build --profile release-fast --target aarch64-apple-darwin --lib",
            release_fast,
        )
        verify_step = _without_yaml_comments(
            macos_job.split(
                "- name: Verify script", maxsplit=1
            )[1].split("\n      - name:", maxsplit=1)[0]
        )
        self.assertIn("always() && github.event_name == 'push'", verify_step)
        self.assertIn("./scripts/run-mac-verify.sh --skip-test", verify_step)

        # The computer_use filtered run yields to the full leg on push (the
        # full suite executes the same tests) and keeps guarding PR legs.
        computer_use = _without_yaml_comments(
            macos_job.split(
                "- name: macOS computer_use unit tests", maxsplit=1
            )[1].split("\n      - name:", maxsplit=1)[0]
        )
        self.assertIn("github.event_name != 'push'", computer_use)

        # The bundle smoke consumes node_modules (build.js resolves
        # @tauri-apps/cli from it and the beforeBuildCommand runs vite), so the
        # push-gated Node.js setup + npm ci steps must survive alongside it.
        # The 2026-10 review round caught the fold initially dropping them:
        # the first bundle_chain push would have died MODULE_NOT_FOUND while
        # every lock test stayed green.
        node_step = _without_yaml_comments(
            macos_job.split(
                "- name: Node.js (universal bundle smoke, push only)", maxsplit=1
            )[1].split("\n      - name:", maxsplit=1)[0]
        )
        self.assertIn(
            "github.event_name == 'push' && needs.changes.outputs.bundle_chain == 'true'",
            node_step,
            "node setup is only consumed by the bundle_chain-gated smoke; "
            "gating it the same way stops paying npm ci on unrelated pushes",
        )
        self.assertIn("node-version: '24'", node_step)
        self.assertIn(
            "cache-dependency-path: pinvou3-app/package-lock.json", node_step
        )
        npm_step = _without_yaml_comments(
            macos_job.split(
                "- name: Install frontend deps (universal bundle smoke, push only)",
                maxsplit=1,
            )[1].split("\n      - name:", maxsplit=1)[0]
        )
        self.assertIn(
            "github.event_name == 'push' && needs.changes.outputs.bundle_chain == 'true'",
            npm_step,
        )
        self.assertIn("npm ci", npm_step)
        # The dual rustup targets feed the same universal build (lipo needs
        # both arches); they are smoke-only provisioning too, so they carry
        # the identical gate — otherwise unrelated pushes pay rustup while
        # node/npm skip.
        dual_target_step = macos_job.split(
            "- name: Install both targets (universal bundle smoke, push only)",
            maxsplit=1,
        )[1].split("\n      - name:", maxsplit=1)[0]
        self.assertIn(
            "github.event_name == 'push' && needs.changes.outputs.bundle_chain == 'true'",
            dual_target_step,
        )
        self.assertIn(
            "rustup target add aarch64-apple-darwin x86_64-apple-darwin",
            dual_target_step,
        )
        # Provisioning must precede the smoke step in the job body.
        self.assertLess(
            macos_job.index("- name: Node.js (universal bundle smoke, push only)"),
            macos_job.index("- name: Tauri bundle smoke"),
        )

        # Deployment-target parity with the release job.
        self.assertIn('MACOSX_DEPLOYMENT_TARGET: "11.0"', macos_job)

    def test_macos_lld_linker_wiring_is_pinned(self):
        # The mac leg's lld linker wiring is easy to lose silently: a dropped
        # env line, a job-level leak into the release legs, or a replaced
        # RUSTFLAGS write all fail no build. The probe cannot self-validate
        # these static pieces, so pin them:
        # - the linker env applied per step to exactly the three
        #   dev-profile test legs (the linux leg pins its own
        #   RUSTFLAGS/DEV_DEBUG exactly; mirror that here),
        # - the job-level env staying free of the linker: the folded
        #   push-only release-fast/bundle legs ship artifacts and are
        #   outside the validated scope, so they must keep default Apple
        #   ld linking,
        # - the probe emitting a step output instead of GITHUB_ENV, so
        #   nothing downstream inherits the probed linker by default,
        # - the strip workaround writing a plain RUSTFLAGS (a composition
        #   would re-propagate the probe's flags onto the release legs),
        # - the probe staying before the cache step, so a toolchain without
        #   a usable lld fails in seconds ahead of any cache restore or
        #   build work (cargo fingerprints inside target/, not the cache
        #   key, keep the lld-built artifacts consistent).
        macos_job = self.pr_workflow.split(
            "\n  macos-rust-check:", maxsplit=1
        )[1].split("\n  windows-codex-runtime-test:", maxsplit=1)[0]

        # No linker env at job level (comments excluded: they document
        # the scoping and legitimately name the variables).
        job_env = _without_yaml_comments(
            macos_job.split("\n    env:", maxsplit=1)[1].split(
                "\n    steps:", maxsplit=1
            )[0]
        )
        self.assertNotIn("CARGO_PROFILE_DEV_LTO", job_env)
        self.assertNotIn("RUSTFLAGS", job_env)

        # Per-step application to the three dev-profile test legs, identical
        # across them so target/debug artifacts stay incrementally reusable.
        test_leg_names = (
            "- name: macOS Rust all-targets check",
            "- name: macOS computer_use unit tests",
            "- name: macOS full lib tests (native regression, push only)",
        )
        for step_name in test_leg_names:
            body = macos_job.split(step_name, maxsplit=1)[1].split(
                "\n      - name:", maxsplit=1
            )[0]
            self.assertIn('CARGO_PROFILE_DEV_LTO: "thin"', body, step_name)
            self.assertIn(
                "RUSTFLAGS: ${{ steps.mac_lld_probe.outputs.flags }}",
                body,
                step_name,
            )

        # The probe emits a step output, not GITHUB_ENV: nothing between the
        # probe and the consuming test steps — nor the release legs below —
        # may inherit the probed linker silently.
        probe_step = macos_job.split(
            "- name: Probe and export the macOS lld link flags", maxsplit=1
        )[1].split("\n      - name:", maxsplit=1)[0]
        self.assertIn("id: mac_lld_probe", probe_step)
        self.assertIn('echo "flags=$flags" >> "$GITHUB_OUTPUT"', probe_step)
        self.assertIn('echo "probed RUSTFLAGS=$flags"', probe_step)
        self.assertNotIn("$GITHUB_ENV", probe_step)
        self.assertLess(
            macos_job.index("- name: Probe and export the macOS lld link flags"),
            macos_job.index("uses: Swatinem/rust-cache@v2"),
        )

        # The strip workaround must not re-propagate the probed flags onto
        # the release legs below it: a plain write only.
        strip_step = macos_job.split(
            "- name: macOS 27+ strip workaround", maxsplit=1
        )[1].split("\n      - name:", maxsplit=1)[0]
        self.assertIn(
            'echo "RUSTFLAGS=-C strip=none" >> "$GITHUB_ENV"', strip_step
        )
        self.assertNotIn("${RUSTFLAGS:+", strip_step)

        # The push-only release legs carry no linker env of their own.
        for step_name in (
            "- name: Cargo build (release-fast",
            "- name: Tauri bundle smoke",
        ):
            body = macos_job.split(step_name, maxsplit=1)[1].split(
                "\n      - name:", maxsplit=1
            )[0]
            self.assertNotIn("RUSTFLAGS", body, step_name)
            self.assertNotIn("CARGO_PROFILE_DEV_LTO", body, step_name)

    def test_main_rust_caches_save_on_failure_and_rust_test_keeps_targets(self):
        # cache-on-failure keeps one failed main run from stranding a
        # namespace cold — mac-build's exact death loop (evicted once, then
        # save-on-success-only kept it cold forever). Saves stay main-only
        # (rust-cache's post step also requires save-if), so PR runs never
        # write caches. rust-test's target cache stays re-enabled: the
        # 2026-08-29/30 restored-target runner deaths were root-caused to
        # runner RAM and absorbed by the zram+swap layers (maintainer
        # ruling, recorded at the cache step) — dropping targets again is a
        # policy change, not cleanup, and must revisit this pin.
        for job_name, end_marker in (
            ("rust-test", "\n  cli-test:"),
            ("windows-rust-test", "\n  macos-rust-check:"),
            ("macos-rust-check", "\n  windows-codex-runtime-test:"),
            # Round-49 review: macos-cli-check joined the flag in round-48
            # (same main-only namespace death-loop exposure) but not this
            # pin — the flag could be dropped with the suite green.
            ("macos-cli-check", "\n  required-gate:"),
        ):
            job = self.pr_workflow.split(
                f"\n  {job_name}:", maxsplit=1
            )[1].split(end_marker, maxsplit=1)[0]
            cache_step = _without_yaml_comments(
                job.split("uses: Swatinem/rust-cache@v2", maxsplit=1)[1].split(
                    "\n      - name:", maxsplit=1
                )[0]
            )
            self.assertIn("cache-on-failure: true", cache_step, job_name)
            self.assertIn("refs/heads/main", cache_step, job_name)
        rust_test_cache = _without_yaml_comments(
            self.pr_workflow.split(
                "\n  rust-test:", maxsplit=1
            )[1].split("\n  cli-test:", maxsplit=1)[0].split(
                "uses: Swatinem/rust-cache@v2", maxsplit=1
            )[1].split("\n      - name:", maxsplit=1)[0]
        )
        self.assertIn("shared-key: rust-test-v2", rust_test_cache)
        # The re-enable is the absence of the old cache-targets: false
        # (rust-cache's default keeps target artifacts). Comments stripped:
        # the decision record above mentions the old value historically.
        self.assertNotIn("cache-targets:", rust_test_cache)

    def test_quota_and_toolchain_hardening_stays_pinned(self):
        # 2026-10 audit hardening. Each item is a silent policy change (quota
        # weight or toolchain drift) if removed, with no runtime error to
        # expose it — so each is pinned:
        # - CARGO_INCREMENTAL=0 repo-wide: incremental artifacts are pure
        #   cache-bloat on ephemeral runners under the 10GB quota.
        # - knowledge-rust drops its target cache (workspace compiles cold
        #   inside its 30-minute cap); only ~/.cargo is cached.
        # - The CodeWhale Windows regression resolves its toolchain via the
        #   same rust-toolchain.toml-derived resolution as every other leg
        #   (RUSTUP_TOOLCHAIN beats the directory override file).
        # - Both Cargo.lock drift guards fail loud when a cargo invocation
        #   regenerated a lockfile instead of honoring it (--locked parity).
        self.assertIn('CARGO_INCREMENTAL: "0"', self.pr_workflow)

        knowledge_job = self.pr_workflow.split(
            "\n  knowledge-rust:", maxsplit=1
        )[1].split("\n  rust-lint:", maxsplit=1)[0]
        knowledge_cache = _without_yaml_comments(
            knowledge_job.split(
                "uses: Swatinem/rust-cache@v2", maxsplit=1
            )[1].split("\n      - name:", maxsplit=1)[0]
        )
        self.assertIn("cache-targets: false", knowledge_cache)

        windows_rust_job = self.pr_workflow.split(
            "\n  windows-rust-test:", maxsplit=1
        )[1].split("\n  macos-rust-check:", maxsplit=1)[0]
        self.assertIn(
            "RUSTUP_TOOLCHAIN: ${{ steps.pinned_toolchain.outputs.version }}",
            _without_yaml_comments(windows_rust_job),
        )

        for job_name, end_marker in (
            ("knowledge-rust", "\n  rust-lint:"),
            ("rust-lint", "\n  rust-test:"),
        ):
            job = self.pr_workflow.split(
                f"\n  {job_name}:", maxsplit=1
            )[1].split(end_marker, maxsplit=1)[0]
            self.assertIn(
                "Cargo.lock drift guard", job, job_name,
            )
            self.assertIn(
                "git diff --exit-code -- '**/Cargo.lock'", job, job_name,
            )

    def test_connector_darwin_x64_executes_on_intel(self):
        # darwin-x64 verifies the pinned x86_64 connector CLIs by EXECUTING
        # them. On the arm64 image that step was skipped (can_run: false),
        # leaving sha256/file checks only — a hash-matching but broken binary
        # shipped green. macos-15-intel is the Intel image (sunset ~2027 with
        # the macos-15 generation; the re-homing note lives in the matrix).
        workflow = (
            ROOT / ".github/workflows/connector-verify.yml"
        ).read_text(encoding="utf-8")
        darwin_x64 = workflow.split(
            "- platform: darwin-x64", maxsplit=1
        )[1].split("- platform:", maxsplit=1)[0]
        self.assertIn("runs-on: macos-15-intel", darwin_x64)
        self.assertIn("can_run: true", darwin_x64)
        # The arm64 leg keeps executing on the arm64 image (no coverage lost).
        darwin_arm64 = workflow.split(
            "- platform: darwin-arm64", maxsplit=1
        )[1].split("- platform:", maxsplit=1)[0]
        self.assertIn("runs-on: macos-15", darwin_arm64)
        self.assertIn("can_run: true", darwin_arm64)

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
        # The 2026-10 concurrency fix: only PR runs cancel each other; queue
        # entries and main pushes must never cancel (queue entries carry the
        # required-check contexts the merge queue waits on). Both jobs get
        # explicit caps instead of the 360-minute workflow default.
        self.assertIn(
            "cancel-in-progress: ${{ github.event_name == 'pull_request' }}",
            workflow,
        )
        self.assertIn("timeout-minutes: 10", changes)
        self.assertIn("timeout-minutes: 20", smoke)




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
