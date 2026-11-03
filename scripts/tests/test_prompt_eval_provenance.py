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


def adapter_bindings():
    return [(repair.cases, "subprocess"), (repair.cases.previous, "subprocess"),
            (repair.cases.api, "VerifierSubprocess"), (repair.cases, "prepare"),
            (community, "cases")]


@contextlib.contextmanager
def isolated_repair_adapter():
    # The CLI installs process-wide grader bindings. A unit fixture must restore
    # shared imports, including prepare, even when an assertion or runner fails.
    with contextlib.ExitStack() as stack:
        for module, name in adapter_bindings():
            stack.enter_context(patch.object(module, name, getattr(module, name)))
        stack.enter_context(patch.object(community, "extra_source_files",
                                        getattr(community, "extra_source_files", ()), create=True))
        repair.attach_cases(community)
        yield


class RepairGenerationTests(unittest.TestCase):
    def test_adapter_bindings_restore_on_failure(self):
        bindings = adapter_bindings()
        originals = [getattr(module, name) for module, name in bindings]
        extra_present = hasattr(community, "extra_source_files")
        extra_original = getattr(community, "extra_source_files", None)
        with self.assertRaisesRegex(RuntimeError, "fixture interruption"):
            with isolated_repair_adapter():
                self.assertIs(repair.cases.previous.subprocess, community.sandbox.VerifierSubprocess)
                self.assertIsNot(repair.cases.prepare, originals[3])
                raise RuntimeError("fixture interruption")
        for (module, name), original in zip(bindings, originals):
            self.assertIs(getattr(module, name), original, name)
        self.assertEqual(hasattr(community, "extra_source_files"), extra_present)
        self.assertIs(getattr(community, "extra_source_files", None), extra_original)

    def test_lateral_api_changes_revision_and_cannot_resume_after_manifest_deletion(self):
        with isolated_repair_adapter(), tempfile.TemporaryDirectory() as directory:
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
            with patch.object(repair.cases.api, "__file__", str(api)), \
                    patch.object(community.sandbox, "self_test"), \
                    patch.object(community.sandbox.shutil, "which", return_value=None), \
                    patch.object(community.sandbox, "command", return_value=[str(binary)]) as command, \
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
                self.assertGreater(command.call_count, 0)


if __name__ == "__main__":
    unittest.main()
