import argparse
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import MagicMock, patch
import wave

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
                                       threshold=500, device="hw:2,0", rate=16000, seconds=2, noisy=True,
                                       responder=None, speaker=None, models=None)
        self.console = self.enterContext(contextlib.redirect_stdout(io.StringIO()))
        self.device = self.enterContext(patch.object(rig, "endpoint"))

    def responder(self, body):
        self.args.speaker = "alsa:hw:CARD=Test,DEV=1"
        self.args.models = self.root
        self.args.responder = self.root / "test-worker"
        self.args.responder.write_text("#!/usr/bin/env python3\nimport os, sys, struct\n"
                                       "reader = sys.stdin.buffer\n"
                                       "def header(): return struct.unpack('<I', reader.read(4))[0]\n" + body)
        self.args.responder.chmod(0o700)

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

    def test_responder_startup_failure_never_starts_capture(self):
        self.responder("header()\nos.write(1, b'!')\n")
        take = MagicMock()
        with (self.assertRaisesRegex(ValueError, "responder status"),
              rig.responding_take(self.args, take) as capture):
            capture(1, lambda _: self.fail("unexpected PCM"))
        take.assert_not_called()

    def test_preserve_every_take_and_never_overwrite_a_session(self):
        def fake(path, *args):
            path.touch(exist_ok=False)
            return hashlib.sha256(path.name.encode()).hexdigest(), None

        with (patch.object(rig.sys.stdin, "isatty", return_value=True),
              patch("builtins.input", side_effect=["RECORD", "MUTED", *(f"TAKE {i}" for i in range(1, 101)), "RECORD"]),
              patch.object(rig, "record", side_effect=fake) as recorder,
              patch.object(rig.session, "checked_capture"), patch.object(rig.session, "check_control"),
              patch.object(rig.session, "measure", side_effect=AssertionError("do not select by result"))):
            rig.acquire(self.args)
            manifest = json.loads((self.dest / "session.json").read_bytes())
            self.assertEqual(manifest["mode"], "passive")
            self.assertEqual(manifest["sample_rate_hz"], 16000)
            self.assertNotIn("control_bridge_started", manifest)
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
              patch.object(rig, "record", return_value=("control", None)) as recorder,
              patch.object(rig.session, "checked_capture", side_effect=ValueError("invalid control"))):
            with self.assertRaisesRegex(ValueError, "invalid control"):
                rig.acquire(self.args)
            recorder.assert_called_once()
            self.assertFalse((self.dest / "session.json").exists())
        self.args.directory = self.root / "second"
        with (patch.object(rig.sys.stdin, "isatty", return_value=True),
              patch("builtins.input", side_effect=["RECORD", "MUTED", "TAKE 1", KeyboardInterrupt()]),
              patch.object(rig, "record", side_effect=[("muted", None), ("first", None)]) as recorder,
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
                  patch.object(rig, "record", side_effect=[(h, None) for h in hashes]) as recorder,
                  patch.object(rig.session, "checked_capture", side_effect=analyzer),
                  patch.object(rig.session, "check_control"), self.assertRaisesRegex(ValueError, reason)):
                rig.acquire(self.args)
            self.assertEqual(recorder.call_count, count)
            self.assertFalse((self.args.directory / "session.json").exists())

    def test_native_recorder_frame_count_and_digest(self):
        rate = 16000
        pcm = b"\x01\x00\x02\x00" * rate

        path = self.root / "capture.wav"
        write = lambda seconds, sink: sink(pcm)
        self.assertEqual(rig.record(path, write, rate, 1, 500), (hashlib.sha256(pcm).hexdigest(), None))
        self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)
        with self.assertRaisesRegex(ValueError, "clock and frame count"):
            rig.record(self.root / "mismatch.wav", write, rate, 2, 500)

    def test_each_cue_requires_50ms_of_original_quiet_on_both_channels(self):
        rate = 44100
        for ch in (None, 0, 1):
            with self.subTest(channel=ch):
                self.console.truncate(0)
                self.console.seek(0)
                pcm = bytearray(4 * rate)
                if ch is not None:
                    for n in range(44):
                        struct.pack_into("<h", pcm, 4 * n + 2 * ch, 501)

                def take(seconds, save):
                    save(pcm[:4 * (rate // 20 - 1)])
                    self.assertNotIn("Apply stimulus", self.console.getvalue())
                    save(pcm[4 * (rate // 20 - 1):4 * (rate // 20)])
                    self.assertEqual("Apply stimulus" in self.console.getvalue(), ch is None)
                    save(pcm[4 * (rate // 20):])

                path = self.root / f"{ch}.wav"
                if ch is None:
                    rig.record(path, take, rate, 1, 500)
                    self.assertEqual(self.console.getvalue().count("Apply stimulus"), 1)
                else:
                    with self.assertRaisesRegex(ValueError, "quiet lead-in"):
                        rig.record(path, take, rate, 1, 500)
                with wave.open(str(path)) as wav:
                    self.assertEqual(wav.readframes(rate), pcm)

    def test_one_worker_exact_reference_after_short_writes_and_eagain_on_successive_takes(self):
        self.args.rate = 16000
        self.responder("for left, result in [(1,b'B'), (2,b'N')]:\n"
                       "    n = header()\n    os.write(1,b'R')\n"
                       "    assert reader.read(n*2) == bytes([left,0])*n\n    os.write(1,result)\n"
                       "assert header() == 0\nos.write(1,b'D')\n")
        real_write = os.write
        attempts = 0

        def fragment(fd, data):
            nonlocal attempts
            attempts += 1
            if attempts % 3 == 2:
                raise BlockingIOError()
            return real_write(fd, data[:2 if len(data) == 4 else 511])

        left = 0
        def take(seconds, save):
            nonlocal left
            left += 1
            for _ in range(16):
                save(bytes([left, 0, 0xab, 0xcd]) * 1000)

        with patch.object(rig.os, "write", side_effect=fragment), rig.responding_take(self.args, take) as capture:
            for i, expected in ((1, True), (2, False)):
                saved = bytearray()
                self.assertIs(capture(1, saved.extend), expected)
                self.assertEqual(saved, bytes([i, 0, 0xab, 0xcd]) * 16000)
        self.assertGreater(attempts, 100)
        self.args.speaker = "default"
        with patch.object(rig.sys.stdin, "isatty", return_value=True), self.assertRaisesRegex(ValueError, "speaker"):
            rig.acquire(self.args)
        self.device.assert_not_called()

    def test_overflow_or_dead_worker_retains_complete_stereo_stops_session(self):
        pcm = b"\x01\x00\x02\x00" * 16000
        def take(seconds, save):
            for i in range(0, len(pcm), 4096):
                save(pcm[i:i + 4096])

        for failure in ("overflow", "death"):
            self.responder("header()\nos.write(1, b'R')\n" +
                           ("reader.read()\n" if failure == "overflow" else "os._exit(1)\n"))
            path = self.root / f"{failure}.wav"
            real_write = os.write
            def blocked(fd, data):
                if len(data) > 4 and failure == "overflow":
                    raise BlockingIOError()
                return real_write(fd, data)
            with (self.subTest(failure=failure), patch.object(rig.os, "write", side_effect=blocked),
                  self.assertRaises((BrokenPipeError, OSError, ValueError)) as error,
                  rig.responding_take(self.args, take) as capture):
                rig.record(path, capture, 16000, 1, 500)
            if failure == "overflow":
                self.assertIn("overflow", str(error.exception))
            with wave.open(str(path)) as wav:
                self.assertEqual(wav.readframes(16000), pcm)

    def test_driven_inventory_uses_control_and_each_exact_take_even_for_misses(self):
        self.responder("for result in b'BBN':\n"
                       "    n = header()\n    os.write(1, b'R')\n    assert len(reader.read(2*n)) == 2*n\n"
                       "    os.write(1, bytes([result]))\nassert header() == 0\nos.write(1,b'D')\n")
        # Slow the fixture producer only outside capture callbacks, like the hardware sample clock.
        index = 0
        def take(seconds, save):
            nonlocal index
            index += 1
            prior = self.console.getvalue().count("Apply stimulus")
            self.assertEqual(prior, index - 1)
            save(bytes(4 * 799))
            self.assertEqual(self.console.getvalue().count("Apply stimulus"), prior)
            save(bytes(4))
            self.assertEqual(self.console.getvalue().count("Apply stimulus"), index)
            save(struct.pack("<hh", 800 + index, 0) * 400)
            remaining = 16000 * seconds - 1200
            while remaining:
                n = min(1024, remaining)
                save(bytes(4 * n))
                remaining -= n
                rig.time.sleep(0.001)
        with patch("builtins.input", side_effect=["MUTED", "TAKE 1", "TAKE 2"]):
            rig.collect(self.args, self.root, rig.session.SCENES[:2], take)
        info = json.loads((self.root / "session.json").read_text())
        self.assertEqual(info["mode"], "driven")
        self.assertIs(info["control_bridge_started"], True)
        self.assertEqual([x["bridge_started"] for x in info["trials"].values()], [True, False])
        self.assertEqual(info["trials"].keys(), {p.name for p in (self.root / "trials").iterdir()})

    def test_driven_control_without_playback_stops_before_unmute(self):
        self.responder("n = header()\nos.write(1,b'R')\nassert len(reader.read(2*n)) == 2*n\nos.write(1,b'N')\n")
        def take(seconds, save):
            self.assertEqual(seconds, 30)
            save(bytes(4 * 800))
            save(struct.pack("<hh", 1000, 0) * 400)
            remaining = 16000 * seconds - 1200
            while remaining:
                n = min(1024, remaining)
                save(bytes(4 * n))
                remaining -= n
                rig.time.sleep(0.001)
        with (patch("builtins.input", side_effect=["MUTED"]) as prompt,
              self.assertRaisesRegex(ValueError, "control did not start a bridge")):
            rig.collect(self.args, self.root, rig.session.SCENES, take)
        prompt.assert_called_once()
        self.assertNotIn("Unmute", self.console.getvalue())
        self.assertTrue((self.root / "muted-control.wav").is_file())
        self.assertEqual(list((self.root / "trials").iterdir()), [])
        self.assertFalse((self.root / "session.json").exists())

    def test_loss_before_final_stream_check_cannot_publish(self):
        self.responder("assert header() == 0\n")  # exits zero without checking the held route
        with (patch("builtins.input", return_value="MUTED"),
              patch.object(rig, "record", return_value=("control", True)),
              patch.object(rig.session, "checked_capture"), patch.object(rig.session, "check_control"),
              self.assertRaisesRegex(ValueError, "missing responder status")):
            rig.collect(self.args, self.root, [], MagicMock())
        self.assertFalse((self.root / "session.json").exists())

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
