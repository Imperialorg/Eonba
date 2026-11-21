import argparse
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import MagicMock, patch

import record_physical_session as rig
import stereo_pcm


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
        self.device = self.enterContext(patch.object(rig, "endpoint"))

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
            self.device.assert_not_called()

    def test_preserve_every_take_and_never_overwrite_a_session(self):
        def fake(path, *args):
            path.touch(exist_ok=False)
            return hashlib.sha256(path.name.encode()).hexdigest()

        with (patch.object(rig.sys.stdin, "isatty", return_value=True),
              patch("builtins.input", side_effect=["RECORD", "MUTED", *(f"TAKE {i}" for i in range(1, 101)), "RECORD"]),
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
            self.device.assert_called_once_with("hw:2,0", 16000)
            self.assertEqual({call.args[1] for call in recorder.call_args_list},
                             {self.device.return_value.__enter__.return_value})
            self.assertEqual(self.dest.stat().st_mode & 0o777, 0o700)
            with self.assertRaises(FileExistsError):
                rig.acquire(self.args)
            self.assertEqual(recorder.call_count, 101)

    def test_bad_control_and_cancel_do_not_publish_or_retry(self):
        with (patch.object(rig.sys.stdin, "isatty", return_value=True),
              patch("builtins.input", side_effect=["RECORD", "MUTED"]),
              patch.object(rig, "record") as recorder,
              patch.object(rig.session, "checked_capture", side_effect=ValueError("invalid control"))):
            with self.assertRaisesRegex(ValueError, "invalid control"):
                rig.acquire(self.args)
            recorder.assert_called_once()
            self.assertFalse((self.dest / "session.json").exists())
        self.args.directory = self.root / "second"
        with (patch.object(rig.sys.stdin, "isatty", return_value=True),
              patch("builtins.input", side_effect=["RECORD", "MUTED", "TAKE 1", KeyboardInterrupt()]),
              patch.object(rig, "record", side_effect=["muted", "first"]) as recorder,
              patch.object(rig.session, "checked_capture"),
              patch.object(rig.session, "check_control")):
            with self.assertRaises(KeyboardInterrupt):
                rig.acquire(self.args)
            self.assertEqual(recorder.call_count, 2)  # control and the first turn
            self.assertFalse((self.args.directory / "session.json").exists())

    def test_capture_stimulus_duplicate_and_turn_ack_are_required(self):
        for label, inputs, hashes, analyzer, reason, count in (
            ("queued-return", [""], ["first"], [None], "turn cancelled", 1),
            ("wrong-turn", ["TAKE 2"], ["first"], [None], "turn cancelled", 1),
            ("duplicate", ["TAKE 1"], ["first"] * 2, [None, None], "duplicate PCM", 2),
            ("no-input", ["TAKE 1"], ["first", "second"], [None, ValueError("no input")], "no input", 2),
        ):
            self.args.directory = self.root / label
            with (self.subTest(label=label), patch.object(rig.sys.stdin, "isatty", return_value=True),
                  patch("builtins.input", side_effect=["RECORD", "MUTED", *inputs]),
                  patch.object(rig, "record", side_effect=hashes) as recorder,
                  patch.object(rig.session, "checked_capture", side_effect=analyzer),
                  patch.object(rig.session, "check_control"), self.assertRaisesRegex(ValueError, reason)):
                rig.acquire(self.args)
            self.assertEqual(recorder.call_count, count)
            self.assertFalse((self.args.directory / "session.json").exists())

    def test_native_recorder_frame_count_and_digest(self):
        rate = 16000
        pcm = b"\x01\x00\xff\x7f" * rate

        path = self.root / "capture.wav"
        write = lambda seconds, sink: sink(pcm)
        self.assertEqual(rig.record(path, write, rate, 1), hashlib.sha256(pcm).hexdigest())
        self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)
        with self.assertRaisesRegex(ValueError, "clock and frame count"):
            rig.record(self.root / "mismatch.wav", write, rate, 2)

    def test_incompatible_rate_rejected_before_capture_and_loss_never_reopens(self):
        alsa = MagicMock()
        for name in ("open", "close", "hw_params_malloc", "hw_params_any", "hw_params_set_access",
                     "hw_params_set_format", "hw_params_set_channels", "hw_params_set_rate",
                     "hw_params", "prepare", "drop"):
            getattr(alsa, "snd_pcm_" + name).return_value = 0
        with patch.object(stereo_pcm, "library", return_value=alsa):
            alsa.snd_pcm_hw_params_set_rate.return_value = -22
            with self.assertRaises(OSError), stereo_pcm.endpoint("hw:2,0", 96000):
                self.fail("mismatched rate admitted")
            alsa.snd_pcm_prepare.assert_not_called()
            alsa.snd_pcm_readi.assert_not_called()
            alsa.snd_pcm_close.assert_called_once()
            alsa.reset_mock()
            alsa.snd_pcm_hw_params_set_rate.return_value = 0
            with self.assertRaises(OSError), stereo_pcm.endpoint("hw:2,0", 16000) as take:
                alsa.snd_pcm_readi.return_value = -19  # device disconnected
                take(2, lambda _: self.fail("unexpected audio"))
            alsa.snd_pcm_drop.assert_called_once()
            alsa.snd_pcm_close.assert_called_once()
            alsa.snd_pcm_open.assert_called_once()


if __name__ == "__main__":
    unittest.main()
