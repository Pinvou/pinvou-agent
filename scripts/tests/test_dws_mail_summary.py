"""Keep the unread-mail day boundary in the script's UTC+08:00 timezone."""

import importlib.util
import io
import unittest
from contextlib import redirect_stdout
from datetime import datetime
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[2] / (
    "pinvou3-app/src-tauri/resources/common/bundle/"
    "dingtalk-skills/dws/scripts/mail_unread_summary.py"
)


class MailSummaryTimezoneTests(unittest.TestCase):
    def test_local_midnight_is_not_mislabeled_as_utc(self):
        spec = importlib.util.spec_from_file_location("dws_mail_summary_test", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with mock.patch.object(module, "datetime") as clock, mock.patch.object(
            module, "get_my_email", return_value="user@example.com"
        ), mock.patch.object(module, "run_dws", return_value=[]) as command, (
            mock.patch("sys.argv", [str(SCRIPT)])
        ), redirect_stdout(io.StringIO()):
            clock.now.return_value = datetime(2026, 9, 8, 1, tzinfo=module.TZ)
            module.main()
        clock.now.assert_called_once_with(module.TZ)
        args = command.call_args.args[0]
        self.assertEqual(
            args[args.index("--query") + 1],
            "isRead:false AND date>2026-09-08T00:00:00+08:00",
        )


if __name__ == "__main__":
    unittest.main()
