"""Regression for the dws attendance report scripts: when Pillow is missing,
_embed_images_in_columns must degrade the image columns to clickable
hyperlinks and still produce the report — it must not abort and must not try
to install packages (PR #437 review response, 2026-09-07).

The Pillow probe reads ``openpyxl.drawing.image.PILImage`` at call time, so a
fake openpyxl package is injected into ``sys.modules`` to simulate both
availability states deterministically, regardless of what is installed.
"""

import importlib.util
import sys
import types
import unittest
from collections import defaultdict
from pathlib import Path
from unittest import mock


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = (
    REPO_ROOT
    / "pinvou3-app"
    / "src-tauri"
    / "resources"
    / "common"
    / "bundle"
    / "dingtalk-skills"
    / "dws"
    / "scripts"
    / "attendance_report_common.py"
)


def load_script_module():
    spec = importlib.util.spec_from_file_location(
        "attendance_report_common_pillow_test", SCRIPT_PATH
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    # dataclasses resolves module-level annotations through sys.modules.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class _FakeWs:
    """Just enough worksheet surface for the code paths under test."""

    def __init__(self):
        self.column_dimensions = defaultdict(types.SimpleNamespace)
        self.row_dimensions = defaultdict(types.SimpleNamespace)


class PillowFallbackTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.mod = load_script_module()

    def _install_fake_openpyxl(self, pil_image):
        openpyxl = types.ModuleType("openpyxl")
        drawing = types.ModuleType("openpyxl.drawing")
        image_mod = types.ModuleType("openpyxl.drawing.image")
        utils = types.ModuleType("openpyxl.utils")
        image_mod.Image = type("Image", (), {})
        image_mod.PILImage = pil_image
        utils.get_column_letter = lambda index: "A"
        drawing.image = image_mod
        openpyxl.drawing = drawing
        openpyxl.utils = utils
        for name, module in (
            ("openpyxl", openpyxl),
            ("openpyxl.drawing", drawing),
            ("openpyxl.drawing.image", image_mod),
            ("openpyxl.utils", utils),
        ):
            sys.modules[name] = module
            self.addCleanup(sys.modules.pop, name, None)

    def _call_embed(self):
        return self.mod._embed_images_in_columns(
            _FakeWs(),
            ["签到图片"],
            [["https://example.com/checkin.png"]],
            image_column_names=["签到图片"],
            header_row=1,
            first_data_row=2,
        )

    def test_missing_pillow_keeps_report_with_hyperlink_fallback(self):
        self._install_fake_openpyxl(pil_image=None)
        fallback = mock.Mock()
        with mock.patch.object(
            self.mod, "_replace_all_image_urls_with_hyperlinks", fallback
        ), mock.patch.object(self.mod, "warn") as warn_mock:
            try:
                self._call_embed()
            except SystemExit as exc:
                self.fail(
                    "report must not abort when Pillow is missing "
                    f"(got sys.exit({exc.code}))"
                )
        fallback.assert_called_once_with(
            mock.ANY,
            ["签到图片"],
            [["https://example.com/checkin.png"]],
            ["签到图片"],
            2,
        )
        warn_mock.assert_called()
        self.assertIn("Pillow", str(warn_mock.call_args))

    def test_pillow_present_does_not_short_circuit_or_fallback(self):
        self._install_fake_openpyxl(pil_image=object())
        fallback = mock.Mock()
        with mock.patch.object(
            self.mod, "_replace_all_image_urls_with_hyperlinks", fallback
        ), mock.patch.object(
            self.mod, "download_and_convert_image", return_value=None
        ), mock.patch.object(
            self.mod, "_set_image_hyperlink"
        ) as hyperlink_mock, mock.patch.object(
            self.mod, "log"
        ):
            try:
                self._call_embed()
            except SystemExit as exc:
                self.fail(
                    f"must not abort when Pillow is available (got sys.exit({exc.code}))"
                )
        fallback.assert_not_called()
        # Control: without Pillow the URL degrades per-image to a hyperlink.
        hyperlink_mock.assert_called_once()


if __name__ == "__main__":
    unittest.main()
