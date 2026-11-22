import io
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import speaker_handoffs as handoffs


class SpeakerHandoffTests(unittest.TestCase):
    def test_requires_named_test_and_confirmed_shipping_encoder(self):
        marker = "handoff speaker model: wespeaker-resnet34\n"
        test = f"test {handoffs.TEST} ... "
        ok = "test result: ok. 1 passed; 0 failed; 0 ignored; 6 filtered out"
        wrong = "playback interrupted 280 ms into the stranger after 270 ms of owner"
        for code, out, err, expected in (
                (0, test + "ok\n" + ok, marker, "passed"),
                (0, test + "ok\n" + ok, marker.replace("34", "221"), "unavailable"),
                (0, ok, marker, "unavailable"), (0, test + "ignored", marker, "unavailable"),
                (101, test + "FAILED", marker + wrong, "failed"),
                (101, test + "FAILED", marker + "returning clean owner never interrupted", "failed"),
                (101, test + "FAILED", marker + "no switch exercised a previously rejected owner candidate",
                 "unavailable"), (101, test + "FAILED", marker + "input missing", "unavailable")):
            with self.subTest(code=code, out=out, err=err):
                self.assertEqual(handoffs.observation(subprocess.CompletedProcess([], code, out, err))[0], expected)

    def test_corpus_must_match_the_lock_and_uses_first_two_long_enough_ids(self):
        import hashlib
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "test.tar.gz"
            with tarfile.open(path, "w:gz") as tar:
                for index, data in ((5, b"E"), (3, b"C"), (1, b"short"), (2, b"B"), (4, b"D")):
                    entry = tarfile.TarInfo(f"LibriSpeech/dev-clean/84/12/84-12-000{index}.flac")
                    entry.size = len(data)
                    tar.addfile(entry, io.BytesIO(data))
            lock = {"voice-test-librispeech-dev-clean": {
                "dest": path.name, "size_bytes": path.stat().st_size, "sha256": "0" * 64}}
            with (patch.object(handoffs.duplex, "ROOT", path.parent),
                  patch.object(handoffs.duplex, "locked_models", return_value=lock),
                  patch.object(handoffs, "GROUPS", ((84,),)),
                  patch.object(handoffs, "crop", side_effect=lambda b: None if b == b"short" else b) as decode):
                with self.assertRaisesRegex(ValueError, "digest"):
                    handoffs.reader_clips()
                decode.assert_not_called()
                lock["voice-test-librispeech-dev-clean"]["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
                self.assertEqual(handoffs.reader_clips()[84], [("84-12-0002", b"B"), ("84-12-0003", b"C")])
