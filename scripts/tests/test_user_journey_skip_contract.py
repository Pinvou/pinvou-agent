"""Pin the marker-gated skip contract of scripts/run-user-journey-tests.sh.

The entry script only counts an optional runner's exit code 2 as "skipped" when
that runner also printed a line starting with ``SKIP:`` at column 0. That makes
the marker a cross-file contract: the consumer is one shell function, the
producers are the runner scripts it invokes. Nothing else enforces it, and a
regression is silent on any machine that has a browser installed -- it only
surfaces as a red run for the developers who do not, which is exactly the
population the optional path exists to serve.

Two halves are checked here:
  * the consumer, by sourcing the real script and driving the real function;
  * the producers, by asserting every ``process.exit(2)`` is paired with a
    line-start ``SKIP:`` marker, and that the runner inventory has not drifted.
"""

import os
import re
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
ENTRY = REPO_ROOT / "scripts" / "run-user-journey-tests.sh"

# Optional runners reached through `run_optional_skip2`. `npm run test:webui`
# chains `build:web` before the smoke, so it is listed by its final runner.
OPTIONAL_RUNNERS = {
    "pinvou3-app/tests/ui_smoke.js",
    "pinvou3-app/tests/settings_ui_smoke.js",
    "pinvou3-app/tests/kb_smoke.js",
    "pinvou3-app/tests/tool_store_smoke.js",
    "remote-control-relay/test/web-ui.smoke.cjs",
}
# The one invocation that does not name its runner directly.
INDIRECT_INVOCATIONS = {"npm --prefix pinvou3-app run test:webui"}

EXIT_TWO = re.compile(r"process\.exit\(2\)")
SKIP_MARKER = re.compile(r"""console\.(?:error|log)\(\s*['"`]SKIP:""")


@unittest.skipIf(os.name == "nt", "the entry script requires a Unix-compatible bash host")
class UserJourneySkipContractTests(unittest.TestCase):
    def _drive(self, child_body):
        """Source the real entry script and run its real helper on a fake runner."""
        with tempfile.TemporaryDirectory() as temp_dir:
            temp = Path(temp_dir)
            child = temp / "fake-runner.sh"
            child.write_text(
                "#!/usr/bin/env bash\n" + textwrap.dedent(child_body), encoding="utf-8"
            )
            child.chmod(0o755)

            harness = temp / "harness.sh"
            harness.write_text(
                textwrap.dedent(
                    f"""\
                    #!/usr/bin/env bash
                    set -euo pipefail
                    source "{ENTRY}"
                    rc=0
                    run_optional_skip2 "{child}" || rc=$?
                    echo "HARNESS_RC=$rc"
                    """
                ),
                encoding="utf-8",
            )
            harness.chmod(0o755)

            result = subprocess.run(
                ["bash", str(harness)],
                cwd=REPO_ROOT,
                text=True,
                encoding="utf-8",
                errors="replace",
                capture_output=True,
                check=False,
            )
            match = re.search(r"HARNESS_RC=(\d+)", result.stdout)
            self.assertIsNotNone(
                match, f"harness did not report a status\n{result.stdout}\n{result.stderr}"
            )
            return int(match.group(1)), result.stdout + result.stderr

    def test_exit_two_with_line_start_marker_is_a_skip(self):
        rc, output = self._drive(
            """\
            echo 'SKIP: fake dependency missing' >&2
            exit 2
            """
        )
        self.assertEqual(rc, 0, output)
        self.assertIn("SKIP: optional dependency missing for:", output)

    def test_exit_two_without_marker_propagates(self):
        rc, output = self._drive(
            """\
            echo 'boom: a real failure that happens to exit 2' >&2
            exit 2
            """
        )
        self.assertEqual(rc, 2, output)
        self.assertNotIn("SKIP: optional dependency missing for:", output)

    def test_indented_marker_is_not_a_skip(self):
        rc, output = self._drive(
            """\
            echo '  SKIP: indented, so not the agreed marker' >&2
            exit 2
            """
        )
        self.assertEqual(rc, 2, output)

    def test_marker_without_exit_two_does_not_mask_the_failure(self):
        rc, output = self._drive(
            """\
            echo 'SKIP: an individual sub-case was skipped'
            exit 1
            """
        )
        self.assertEqual(rc, 1, output)

    def test_success_passes_through(self):
        rc, output = self._drive("echo 'all good'\nexit 0\n")
        self.assertEqual(rc, 0, output)

    def test_optional_runner_inventory_has_not_drifted(self):
        """Adding an optional runner must also extend this test's producer set."""
        source = ENTRY.read_text(encoding="utf-8")
        invoked = set()
        for line in source.splitlines():
            stripped = line.strip()
            if not stripped.startswith("run_optional_skip2 "):
                continue
            command = stripped[len("run_optional_skip2 ") :].strip()
            if command in INDIRECT_INVOCATIONS:
                continue
            self.assertTrue(
                command.startswith("node "),
                f"unrecognised optional invocation, extend this test: {command}",
            )
            invoked.add(command[len("node ") :].strip())
        self.assertTrue(invoked, "no optional runners found in the entry script")
        self.assertTrue(
            invoked <= OPTIONAL_RUNNERS,
            f"optional runners not covered by this test: {sorted(invoked - OPTIONAL_RUNNERS)}",
        )

    def test_every_optional_runner_prints_the_marker_before_exiting_two(self):
        for relative in sorted(OPTIONAL_RUNNERS):
            runner = REPO_ROOT / relative
            with self.subTest(runner=relative):
                source = runner.read_text(encoding="utf-8")
                exits = list(EXIT_TWO.finditer(source))
                markers = list(SKIP_MARKER.finditer(source))
                self.assertTrue(exits, f"{relative} no longer exits 2 on a missing dependency")
                self.assertEqual(
                    len(exits),
                    len(markers),
                    f"{relative} has {len(exits)} exit(2) sites but {len(markers)} SKIP: markers",
                )
                for exit_site in exits:
                    window = source[max(0, exit_site.start() - 400) : exit_site.start()]
                    self.assertTrue(
                        SKIP_MARKER.search(window),
                        f"{relative}: an exit(2) at offset {exit_site.start()} "
                        "is not preceded by a SKIP: marker",
                    )


if __name__ == "__main__":
    unittest.main()
