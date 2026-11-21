import argparse
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import wave

import record_physical_session as rig


class RigTests(unittest.TestCase):
    """Recorder protocol only. No device or physical timing result is exercised here."""

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.dest = self.root / "takes"
        self.args = argparse.Namespace(directory=self.dest, route="unit test", gains="fixed",
                                       threshold=500, device="hw:2,0", rate=16000, seconds=2, noisy=True)
        self.enterContext(contextlib.redirect_stdout(io.StringIO()))

    def test_device_selection_and_consent_before_capture(self):
        with (patch.object(rig.sys.stdin, "isatty", return_value=False) as tty,
              patch("builtins.input", return_value="no") as operator, patch.object(rig, "record") as recorder):
            with self.assertRaisesRegex(ValueError, "terminal"):
                rig.acquire(self.args)
            tty.return_value = True
            for device in ("default", "pulse", "hw:2,0 extra", "null"):
                self.args.device = device
                with self.assertRaisesRegex(ValueError, "hardware capture"):
                    rig.acquire(self.args)
            operator.assert_not_called()
            self.args.device = "hw:2,0"
            with self.assertRaisesRegex(ValueError, "not authorized"):
                rig.acquire(self.args)
            self.assertFalse(self.dest.exists())
            recorder.assert_not_called()

    def test_preserve_every_take_and_never_overwrite_a_session(self):
        def fake(path, *args):
            path.touch(exist_ok=False)
            return hashlib.sha256(path.name.encode()).hexdigest()

        with (patch.object(rig.sys.stdin, "isatty", return_value=True),
              patch("builtins.input", side_effect=["RECORD", ""] + [""] * 100 + ["RECORD"]),
              patch.object(rig, "record", side_effect=fake) as recorder,
              patch.object(rig.session, "checked_capture"), patch.object(rig.session, "check_control"),
              patch.object(rig.session, "measure", side_effect=AssertionError("do not select by result"))):
            rig.acquire(self.args)
            manifest = json.loads((self.dest / "session.json").read_bytes())
            expected = ["control", *rig.session.SCENES]
            self.assertEqual(manifest["trials"], {
                name: {"pcm_sha256": hashlib.sha256(name.encode()).hexdigest(), "scene": expected[i // 25]}
                for i, name in enumerate(f"turn-{n:03d}.wav" for n in range(1, 101))
            })
            self.assertEqual(recorder.call_count, 101)
            self.assertEqual(self.dest.stat().st_mode & 0o777, 0o700)
            with self.assertRaises(FileExistsError):
                rig.acquire(self.args)
            self.assertEqual(recorder.call_count, 101)

    def test_bad_control_and_cancel_do_not_publish_or_retry(self):
        with (patch.object(rig.sys.stdin, "isatty", return_value=True),
              patch("builtins.input", side_effect=["RECORD", ""]),
              patch.object(rig, "record") as recorder,
              patch.object(rig.session, "checked_capture", side_effect=ValueError("invalid control"))):
            with self.assertRaisesRegex(ValueError, "invalid control"):
                rig.acquire(self.args)
            recorder.assert_called_once()
            self.assertFalse((self.dest / "session.json").exists())
        self.args.directory = self.root / "second"
        with (patch.object(rig.sys.stdin, "isatty", return_value=True),
              patch("builtins.input", side_effect=["RECORD", "", "", KeyboardInterrupt()]),
              patch.object(rig, "record") as recorder, patch.object(rig.session, "checked_capture"),
              patch.object(rig.session, "check_control")):
            with self.assertRaises(KeyboardInterrupt):
                rig.acquire(self.args)
            self.assertEqual(recorder.call_count, 2)  # control and the first turn
            self.assertFalse((self.args.directory / "session.json").exists())

    def test_native_recorder_shape_and_clock_are_checked_before_hashing(self):
        rate = 16000
        pcm = b"\x01\x00\xff\x7f" * rate

        def write(command, *, stdout, stdin, check, timeout):
            self.assertEqual(command, ["/usr/bin/arecord", "--quiet", "--nonblock", "--fatal-errors",
                                       "-D", "hw:2,0", "-t", "wav", "-f", "S16_LE", "-c", "2",
                                       "-r", "16000", "--samples", "16000"])
            self.assertTrue(check)
            self.assertEqual((stdin, timeout), (subprocess.DEVNULL, 11))
            with wave.open(stdout, "wb") as wav:
                wav.setparams((2, 2, rate, 0, "NONE", "not compressed"))
                wav.writeframes(pcm)

        path = self.root / "capture.wav"
        with patch.object(rig.subprocess, "run", side_effect=write):
            self.assertEqual(rig.record(path, "hw:2,0", 16000, 1), hashlib.sha256(pcm).hexdigest())
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)
            rate = 44100
            with self.assertRaisesRegex(ValueError, "clock and frame count"):
                rig.record(self.root / "mismatch.wav", "hw:2,0", 16000, 1)


if __name__ == "__main__":
    unittest.main()
