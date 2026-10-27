"""Opt-in integration control for the narrow evaluation-digest exemption.

Run with PINVOU_GITLEAKS_BINARY=/absolute/path/to/gitleaks.
No credentials or network services are used; values below are synthetic.
"""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
BINARY = os.environ.get("PINVOU_GITLEAKS_BINARY")


@unittest.skipUnless(BINARY, "opt-in pinned Gitleaks binary")
class EvaluationDigestAllowlist(unittest.TestCase):
    def test_only_audited_digest_inside_exact_manifest_path_is_ignored(self):
        known = json.loads((REPO / "docs/prompt-optimization-evaluation/source-manifest-v2.4.5.json").read_text())["evaluation_sources"]["prompt_eval_extra_api.py"]
        synthetic = hashlib.sha256(b"synthetic-negative-control-not-a-real-credential").hexdigest()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / "docs/prompt-optimization-evaluation/source-manifest-v2.4.5.json"
            manifest.parent.mkdir(parents=True)
            manifest.write_text(json.dumps({"prompt_eval_extra_api.py": known,
                                             "unknown_api.py": synthetic}, indent=2))
            (root / "unrelated.json").write_text(json.dumps({"prompt_eval_extra_api.py": known}))
            for command in [
                ["git", "init", "-q"],
                ["git", "add", "."],
                ["git", "-c", "user.name=Control", "-c", "user.email=control@example.invalid",
                 "commit", "-qm", "test: synthetic secret-scan controls"],
            ]:
                subprocess.run(command, cwd=root, check=True, capture_output=True)
            report = root / "findings.json"
            result = subprocess.run([
                BINARY, "detect", "--source", str(root), "--config", str(REPO / ".gitleaks.toml"),
                "--redact", "--report-format", "json", "--report-path", str(report), "--log-opts=-1",
            ], capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            findings = json.loads(report.read_text())
            self.assertEqual(len(findings), 2)
            self.assertEqual({row["RuleID"] for row in findings}, {"generic-api-key"})
            self.assertEqual({row["File"] for row in findings}, {
                "docs/prompt-optimization-evaluation/source-manifest-v2.4.5.json", "unrelated.json",
            })
            # The real audited value is allowed only at its exact manifest path;
            # the different value at that path and the same value elsewhere fail.
            self.assertEqual(next(row for row in findings if row["File"] != "unrelated.json")["StartLine"], 3)


if __name__ == "__main__":
    unittest.main()
