"""Starting the screen share again after COSMIC's portal service crashes."""

import importlib.util
import sys
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "screen_capture.py"


def load_script():
    gi = mock.MagicMock()
    repository = mock.MagicMock()
    repository.GLib.Variant = lambda _signature, value: value
    modules = {"gi": gi, "gi.repository": repository}
    with mock.patch.dict(sys.modules, modules):
        spec = importlib.util.spec_from_file_location("screen_capture_under_test", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    return module


class SessionTokenTests(unittest.TestCase):
    def test_a_restart_never_reuses_the_session_token(self):
        module = load_script()
        portal = module.Portal.__new__(module.Portal)
        portal.counter = 0
        portal.session = None
        tokens = []

        def call(method, _signature, *args):
            portal.counter += 1
            if method == "CreateSession":
                tokens.append(args[-1]["session_handle_token"])
                return 0, {"session_handle": "/session"}
            return 2, {}

        portal.call = call
        portal.cursor_modes = lambda: 0
        for _ in range(2):
            self.assertEqual(portal.start(), (None, "screen_portal_failed"))
        self.assertEqual(len(set(tokens)), 2)


class ResumeLimitTests(unittest.TestCase):
    def test_a_burst_of_crashes_is_not_started_again(self):
        module = load_script()
        self.assertTrue(module.may_resume(0, None, 100.0))
        self.assertFalse(module.may_resume(1, 100.0, 100.0 + module.RESUME_SETTLE_S - 1))

    def test_a_crash_after_it_settled_is_started_again(self):
        module = load_script()
        self.assertTrue(module.may_resume(1, 100.0, 100.0 + module.RESUME_SETTLE_S))

    def test_the_total_stays_limited(self):
        module = load_script()
        self.assertFalse(module.may_resume(module.RESUMES, None, 100.0))


if __name__ == "__main__":
    unittest.main()
