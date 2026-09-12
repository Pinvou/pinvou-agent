import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "eval-shell-guidance.py"
SPEC = importlib.util.spec_from_file_location("eval_shell_guidance", SCRIPT)
EVAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EVAL)


class ShellGuidanceEvalTests(unittest.TestCase):
    def run_cli(self, arguments, environment):
        with patch("sys.argv", [str(SCRIPT), *arguments]), \
                patch.dict(os.environ, environment, clear=True), \
                contextlib.redirect_stderr(io.StringIO()) as error:
            with self.assertRaises(SystemExit) as result:
                EVAL.main()
        self.assertEqual(result.exception.code, 2)
        self.assertNotIn("Traceback", error.getvalue())
        return error.getvalue()

    def test_before_arm_requires_explicit_baselines(self):
        error = self.run_cli(["--fixture", "unused.json", "--output", "unused"], {})
        self.assertIn("--app-base and --engine-base", error)

    def test_missing_environment_is_actionable_before_file_access(self):
        error = self.run_cli(
            ["--fixture", "unused.json", "--output", "unused", "--arms", "after"], {})
        self.assertIn("SHELL_EVAL_BASE_URL, SHELL_EVAL_MODEL", error)

    def test_candidate_is_not_a_legacy_baseline(self):
        with patch.object(EVAL, "git_text", return_value="guidance::description()"):
            with self.assertRaisesRegex(ValueError, "pre-guidance"):
                EVAL.baseline_description("HEAD")

    def test_invalid_explicit_baseline_has_a_cli_error(self):
        with tempfile.TemporaryDirectory() as temp:
            fixture = Path(temp) / "fixture.json"
            fixture.write_text('{"shell":"powershell"}', encoding="utf-8")
            with patch.object(EVAL, "git_text", return_value="guidance::description()"), \
                    patch.object(EVAL.urllib.request, "urlopen") as network:
                error = self.run_cli([
                    "--fixture", str(fixture), "--output", str(Path(temp) / "results"),
                    "--app-base", "before", "--engine-base", "candidate",
                ], {"SHELL_EVAL_BASE_URL": "http://127.0.0.1:1", "SHELL_EVAL_MODEL": "test"})
            self.assertIn("cannot load the before baseline", error)
            network.assert_not_called()

    def test_after_and_paired_fixtures_render_titles_without_network(self):
        fixture = {"name": "Bash", "shell": "powershell", "description": "fixture",
                   "input_schema": {"type": "object", "properties": {}}}
        environment = {"SHELL_EVAL_BASE_URL": "http://127.0.0.1:1/v1",
                       "SHELL_EVAL_MODEL": "synthetic-model"}
        for language, expected in [("zh-Hans", "简体中文"), ("en", "English"), ("ja", "日本語")]:
            for paired in (False, True):
                with self.subTest(language=language, paired=paired), tempfile.TemporaryDirectory() as temp:
                    directory = Path(temp)
                    fixture_path = directory / "fixture.json"
                    fixture_path.write_text(json.dumps(fixture), encoding="utf-8")
                    output = directory / "results"
                    args = [str(SCRIPT), "--fixture", str(fixture_path), "--output", str(output),
                            "--language", language, "--repeats", "1", "--workers", "1",
                            "--tasks", "csv_sum"]
                    args += (["--before-fixture", str(fixture_path)] if paired else ["--arms", "after"])
                    requests = []

                    def respond(request, timeout):
                        requests.append(json.loads(request.data))
                        return io.StringIO(json.dumps({"choices": [{"message": {"tool_calls": [
                            {"function": {"name": "Bash", "arguments": "{}"}}
                        ]}}]}))

                    with patch("sys.argv", args), patch.dict(os.environ, environment, clear=True), \
                            patch.object(EVAL.urllib.request, "urlopen", side_effect=respond), \
                            contextlib.redirect_stdout(io.StringIO()):
                        EVAL.main()
                    self.assertEqual(len(requests), 2 if paired else 1)
                    prompts = [request["messages"][0]["content"] for request in requests]
                    for prompt in prompts:
                        self.assertNotIn("{{PINVOU3_", prompt)
                        self.assertIn(expected, prompt)
                    if paired:
                        self.assertEqual(prompts[0], prompts[1])


if __name__ == "__main__":
    unittest.main()
