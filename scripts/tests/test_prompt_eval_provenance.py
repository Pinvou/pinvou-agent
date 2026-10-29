"""Version lateral graders and refuse stale receipts without a manifest."""

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


community = load("provenance_community", "eval-prompt-community.py")
repair = load("provenance_repair", "eval-prompt-repair.py")


class RepairGenerationTests(unittest.TestCase):
    def test_lateral_api_changes_revision_and_cannot_resume_after_manifest_deletion(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            template = root / "template"
            template.mkdir()
            profile = {"id": "fixture", "model": "no-provider", "base_url": "http://127.0.0.1:1"}
            settings = {"advanced": {"saved_models": [profile]}}
            (template / "settings.json").write_text(json.dumps(settings))
            binary = root / "not-executed"
            binary.write_text("synthetic binary; process is stubbed")
            api = root / "prompt_eval_extra_api.py"
            api.write_text("old synthetic grader generation")
            args = SimpleNamespace(output=root / "first", home_template=template,
                                   baseline=binary, candidate=binary, credential_cli=binary,
                                   xvfb=binary, model_id="fixture", model_alias="fixture",
                                   repetitions=1, cases="code_consult")
            proxy = SimpleNamespace(settings=settings, wire_shapes=[], close=Mock())
            process = SimpleNamespace(stdout=json.dumps({"status": "Completed",
                                      "assistant_text": "100元折扣20%，应付80元"}), stderr="", returncode=0)
            repair.attach_cases(community)
            with patch.object(repair.cases.api, "__file__", str(api)), \
                    patch.object(community.sandbox, "self_test"), \
                    patch.object(community, "ModelProxy", return_value=proxy), \
                    patch.object(community.subprocess, "run", return_value=process), \
                    contextlib.redirect_stdout(io.StringIO()):
                community.run(args)
                manifest = args.output / "manifest.json"
                first = json.loads(manifest.read_text())
                self.assertIn(api.name, first["source_hashes"])
                self.assertIn("eval-prompt-repair.py", first["source_hashes"])
                receipt = args.output / "results.jsonl"
                original = receipt.read_bytes() + b'{"interrupted":'
                receipt.write_bytes(original)
                manifest.unlink()
                api.write_text("new synthetic grader generation")
                with self.assertRaisesRegex(ValueError, "same grader_revision"):
                    community.run(args)
                self.assertEqual(receipt.read_bytes(), original)
                self.assertFalse(manifest.exists())
                self.assertFalse(receipt.with_name(receipt.name + ".incomplete-tail").exists())
                args.output = root / "second"
                community.run(args)
                second = json.loads((args.output / "manifest.json").read_text())
                self.assertNotEqual(first["grader_revision"], second["grader_revision"])


if __name__ == "__main__":
    unittest.main()
