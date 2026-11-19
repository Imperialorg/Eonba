import contextlib
import io
import shutil
import struct
import tempfile
import unittest
import wave
from pathlib import Path

from measure_ttfa import latency_ms, main, summarize


class LatencyTests(unittest.TestCase):
    def fixture(self, output_start=200, output_length=15, rate=8000):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        path = Path(temp.name) / "trial.wav"
        with wave.open(str(path), "wb") as wav:
            wav.setparams((2, 2, rate, 0, "NONE", "not compressed"))
            frames = (
                (1800 * (60 <= i * 1000 // rate < 100),
                 900 * (output_start <= i * 1000 // rate < output_start + output_length))
                for i in range(rate // 4)
            )
            wav.writeframes(b"".join(struct.pack("<hh", *f) for f in frames))
        return path

    def test_two_channels_on_one_clock(self):
        for rate in (8000, 44100, 48000):
            with self.subTest(rate=rate):
                # The actual gap is 100ms. Include quantization uncertainty.
                self.assertEqual(latency_ms(self.fixture(rate=rate), 500), 102)

    def test_no_response_single_impulse_overlap_and_truncation_are_not_passes(self):
        for start, duration in [(250, 15), (150, 1), (80, 10)]:
            with self.assertRaises(ValueError):
                latency_ms(self.fixture(start, duration), 500)
        path = self.fixture()
        path.write_bytes(path.read_bytes()[:-4])
        with self.assertRaisesRegex(ValueError, "truncated"):
            latency_ms(path, 500)

    def test_strict_percentile_limits_and_insufficient_sample_size(self):
        with self.assertRaises(ValueError):
            summarize([10] * 99)
        self.assertTrue(summarize([169] * 98 + [279, 500])["passed"])
        self.assertFalse(summarize([170] * 100)["passed"])
        self.assertFalse(summarize([20] * 98 + [280, 281])["passed"])

    def test_renamed_copies_cannot_pad_sample_size(self):
        trial = self.fixture()
        shutil.copyfile(trial, trial.with_name("another.wav"))
        with contextlib.redirect_stderr(io.StringIO()) as errors:
            self.assertEqual(main([str(trial.parent), "--threshold", "500"]), 2)
        self.assertIn("duplicate PCM recording", errors.getvalue())
