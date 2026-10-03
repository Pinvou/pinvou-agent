import os
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
VERIFIER = ROOT / "scripts/verify-public-submodule.sh"
GITLINK = "1111111111111111111111111111111111111111"
OTHER_COMMIT = "2222222222222222222222222222222222222222"
TAG_OBJECT = "3333333333333333333333333333333333333333"
# 与 verify-public-submodule.sh 的常量保持一致：2026-10-02 起处于两次 rN 收口
# 之间的过渡期，TRANSITION_BASELINE 钉在 r3 收口 commit（与脚本同值），gitlink =
# 公开维护分支头且领先不可变 tag。下一次 rN 收口时，把此处的 tag 名/钉点与脚本
# 常量同步推进（新 tag、清空 TRANSITION_BASELINE），并恢复三方相等断言。
BASELINE = "61cb769be5b33abc64f64da4272f5b39a8b6c1fd"


@unittest.skipIf(os.name == "nt", "shell verifier requires a Unix-compatible bash host")
class PublicSubmoduleVerifierTests(unittest.TestCase):
    def _run(self, scenario):
        with tempfile.TemporaryDirectory() as temp_dir:
            temp = Path(temp_dir)
            bin_dir = temp / "bin"
            bin_dir.mkdir()
            state_file = temp / "ls-remote-attempts"

            fake_git = bin_dir / "git"
            fake_git.write_text(
                textwrap.dedent(
                    f"""\
                    #!/usr/bin/env bash
                    set -euo pipefail

                    if [[ "${{1:-}}" == "-C" ]]; then
                      shift 2
                    fi

                    case "${{1:-}}" in
                      config)
                        key="${{5:-}}"
                        case "$key" in
                          submodule.CodeWhale.path) printf '%s\\n' 'CodeWhale' ;;
                          submodule.CodeWhale.url) printf '%s\\n' 'https://github.com/Pinvou/CodeWhale.git' ;;
                          submodule.CodeWhale.branch) exit 1 ;;
                          *) exit 2 ;;
                        esac
                        ;;
                      ls-files)
                        gitlink='{GITLINK}'
                        if [[ "$PINVOU_FAKE_GIT_SCENARIO" == "tag_caught_up" ]]; then
                          gitlink='{BASELINE}'
                        fi
                        printf '160000 %s 0\\tCodeWhale\\n' "$gitlink"
                        ;;
                      ls-remote)
                        attempt=0
                        if [[ -f "$PINVOU_FAKE_GIT_STATE" ]]; then
                          attempt="$(<"$PINVOU_FAKE_GIT_STATE")"
                        fi
                        attempt=$((attempt + 1))
                        printf '%s\\n' "$attempt" > "$PINVOU_FAKE_GIT_STATE"

                        case "$PINVOU_FAKE_GIT_SCENARIO" in
                          retry_then_aligned)
                            if [[ "$attempt" -lt 3 ]]; then exit 128; fi
                            ;;
                          transport_failure)
                            exit 128
                            ;;
                        esac

                        branch='{GITLINK}'
                        tag='{BASELINE}'
                        case "$PINVOU_FAKE_GIT_SCENARIO" in
                          branch_mismatch) branch='{OTHER_COMMIT}' ;;
                          annotated) tag='{TAG_OBJECT}' ;;
                          tag_drifted) tag='{OTHER_COMMIT}' ;;
                          tag_caught_up)
                            branch='{BASELINE}'
                            tag='{BASELINE}'
                            ;;
                        esac
                        printf '%s\\trefs/heads/pinvou3-clean\\n' "$branch"
                        if [[ "$PINVOU_FAKE_GIT_SCENARIO" != "missing_tag" ]]; then
                          printf '%s\\trefs/tags/pinvou-v0.9.12-r3\\n' "$tag"
                        fi
                        if [[ "$PINVOU_FAKE_GIT_SCENARIO" == "annotated" ]]; then
                          printf '%s\\trefs/tags/pinvou-v0.9.12-r3^{{}}\\n' '{BASELINE}'
                        fi
                        ;;
                      *)
                        exit 2
                        ;;
                    esac
                    """
                ),
                encoding="utf-8",
            )
            fake_git.chmod(0o755)

            fake_sleep = bin_dir / "sleep"
            fake_sleep.write_text("#!/usr/bin/env bash\nexit 0\n", encoding="utf-8")
            fake_sleep.chmod(0o755)

            env = os.environ.copy()
            env["PATH"] = f"{bin_dir}:{env['PATH']}"
            env["PINVOU_FAKE_GIT_SCENARIO"] = scenario
            env["PINVOU_FAKE_GIT_STATE"] = str(state_file)
            result = subprocess.run(
                [str(VERIFIER)],
                cwd=ROOT,
                env=env,
                text=True,
                encoding="utf-8",
                errors="replace",
                capture_output=True,
                check=False,
            )
            attempts = int(state_file.read_text(encoding="utf-8").strip())
            return result, attempts

    def test_transition_gitlink_tracks_branch_and_aheads_of_pinned_tag(self):
        result, attempts = self._run("aligned")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(attempts, 1)
        self.assertIn("过渡期校验通过", result.stdout)
        self.assertIn(f"pinvou3-clean = gitlink = {GITLINK}", result.stdout)

    def test_annotated_tag_is_compared_after_peeling(self):
        result, attempts = self._run("annotated")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(attempts, 1)
        self.assertIn("过渡期校验通过", result.stdout)

    def test_mismatched_public_branch_fails_closed(self):
        result, attempts = self._run("branch_mismatch")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(attempts, 1)
        self.assertIn("pinvou3-clean", result.stderr)

    def test_missing_immutable_tag_fails_closed(self):
        result, attempts = self._run("missing_tag")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(attempts, 1)
        self.assertIn("pinvou-v0.9.12-r3", result.stderr)
        self.assertIn("过渡期内应钉在收口", result.stderr)
        self.assertIn("<不存在>", result.stderr)

    def test_tag_drifting_from_the_baseline_fails_closed(self):
        result, attempts = self._run("tag_drifted")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(attempts, 1)
        self.assertIn("pinvou-v0.9.12-r3", result.stderr)
        self.assertIn("过渡期内应钉在收口", result.stderr)
        self.assertIn(OTHER_COMMIT, result.stderr)

    def test_tag_caught_up_by_the_gitlink_demands_a_new_closure_tag(self):
        result, attempts = self._run("tag_caught_up")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(attempts, 1)
        self.assertIn("gitlink 已追平", result.stderr)
        self.assertIn("切除新 tag", result.stderr)

    def test_remote_transport_is_retried_with_a_finite_limit(self):
        result, attempts = self._run("retry_then_aligned")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(attempts, 3)

        result, attempts = self._run("transport_failure")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(attempts, 3)
        self.assertIn("3 次尝试", result.stderr)


if __name__ == "__main__":
    unittest.main()
