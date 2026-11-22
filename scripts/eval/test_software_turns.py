import hashlib
import io
import json
import struct
import tarfile
import unittest

import software_turns as turns


def archive(clips):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w:gz") as tar:
        for name, frames in clips:
            info = (16_000 << 44) | (15 << 36) | frames
            payload = b"fLaC\x80\x00\x00\x22" + bytes(10) + info.to_bytes(8, "big") + bytes(16) + b"x"
            entry = tarfile.TarInfo("LibriSpeech/dev-clean/1/2/" + name + ".flac")
            entry.size = len(payload)
            tar.addfile(entry, io.BytesIO(payload))
    stream.seek(0)
    return stream


class SoftwareCorpusTests(unittest.TestCase):
    def test_rank_whole_eligible_utterances_not_first_archive_members(self):
        items = [(f"1-2-{i:04d}", 16_000 + i) for i in range(8)]
        expected = sorted(items, key=lambda pair: hashlib.sha256(pair[0].encode()).digest())[:3]
        items.append(("1-2-9999", 16_000 * 13))
        for order in (items, list(reversed(items))):
            self.assertEqual([(key, frames) for key, frames, _ in turns.select_clips(archive(order), 3)], expected)
        with self.assertRaisesRegex(ValueError, "duplicate"):
            turns.select_clips(archive(items + items[:1]), 3)
        with self.assertRaisesRegex(ValueError, "distinct"):
            turns.select_clips(archive(items), 9)

    def test_dry_reference_is_never_the_noisy_inference_channel(self):
        raw = struct.pack("<100h", *[900, -900] * 50)
        clean, snr = turns.channels(raw, "clean", {})
        self.assertIsNone(snr)
        noisy, snr = turns.channels(raw, "traffic-0db", {})
        self.assertLessEqual(abs(snr), 0.01)
        plain = list(struct.iter_unpack("<hh", clean))
        mixed = list(struct.iter_unpack("<hh", noisy))
        self.assertTrue(all(a == b for a, b in plain))
        self.assertEqual([a for a, _ in plain], [a for a, _ in mixed])
        self.assertTrue(all(a == 0 for a, _ in mixed[-turns.TAIL:]))
        self.assertTrue(any(b != 0 for _, b in mixed[-turns.RATE:]))

    def test_lengths_count_stereo_frames_not_bytes_or_samples(self):
        stream = io.BytesIO()
        payload = struct.pack("<hhhh", 1, -2, 3, -4)
        self.assertEqual(turns.write_take(stream, payload), 2)
        self.assertEqual(stream.getvalue(), struct.pack("<I", 2) + payload)

    def test_dry_activity_uses_aligned_1ms_bins_including_trailing_padding(self):
        self.assertEqual(turns.last_active(struct.pack("<16h", *[100] * 16)), turns.LEAD + 16)
        self.assertEqual(turns.last_active(struct.pack("<17h", *([100] * 16 + [1]))), turns.LEAD + 16)
        self.assertEqual(turns.last_active(struct.pack("<17h", *([0] * 16 + [400]))), turns.LEAD + 32)
        with self.assertRaisesRegex(ValueError, "no foreground"):
            turns.last_active(bytes(800))


class SoftwareScoreTests(unittest.TestCase):
    def setUp(self):
        self.inventory = [{"id": "silent-control", "scene": "silence", "frames": 16000, "dry_end": None}]
        self.rows = [dict(status="no_reply", endpoint_sample=None, last_active_end_sample=None, latency_us=None)]
        for i in range(turns.COUNT):
            self.inventory.append({"id": str(i), "scene": turns.SCENES[i % 4], "frames": 16000, "dry_end": 1040})
            self.rows.append(dict(status="ok", endpoint_sample=1408, last_active_end_sample=1040, latency_us=80_000))

    def score(self, reason=None):
        return turns.evaluate(self.inventory, b"".join(json.dumps(r).encode() + b"\n" for r in self.rows), reason)

    def test_silence_is_not_timed_and_ceilings_are_strict(self):
        self.assertEqual(self.score()["p99_us"], 80_000)
        self.assertEqual(self.score()["status"], "passed")
        self.rows[1]["latency_us"] = 280_000  # clean, 100th ordered observation
        self.assertEqual(self.score()["status"], "passed")
        self.rows[5]["latency_us"] = 280_000
        self.assertEqual(self.score()["status"], "failed")
        self.assertEqual(self.score()["p99_us"], 280_000)
        self.rows[5]["latency_us"] = 80_000
        self.rows[2]["latency_us"] = 200_000  # even one noisy turn at the limit fails
        self.assertEqual(self.score()["status"], "failed")
        for row in self.rows[1:]:
            row["latency_us"] = 170_000
        self.assertEqual(self.score()["status"], "failed")

    def test_early_endpoint_or_missing_sustained_render_is_not_a_fast_reply(self):
        for status, end in (("early_endpoint", 512), ("no_reply", None), ("no_render_onset", 1408)):
            self.rows[1].update(status=status, endpoint_sample=end, latency_us=None)
            result = self.score()
            self.assertEqual(result["status"], "failed")
            self.assertIsNone(result["p99_us"])
            self.assertEqual(result["scenes"]["clean"]["misses"], 1)
        self.rows.pop()
        self.assertEqual(self.score("responder exited 1")["status"], "failed")

    def test_truncated_crashed_or_inconsistent_batches_cannot_pass(self):
        self.assertEqual(self.score("responder exited 1")["status"], "unavailable")
        self.rows.pop()
        self.assertEqual(self.score()["status"], "unavailable")
        for field, value in (("last_active_end_sample", 900), ("endpoint_sample", False), ("latency_us", 1)):
            original = self.rows[1][field]
            self.rows[1][field] = value
            self.assertIn("take 2:", self.score()["reason"])
            self.rows[1][field] = original
        self.rows[0].update(status="no_input_activity", endpoint_sample=512)
        self.assertEqual(self.score()["status"], "failed")

    def test_missing_control_or_incomplete_scene_inventory_cannot_pass(self):
        self.inventory[-1]["scene"] = "clean"
        with self.assertRaisesRegex(ValueError, "scene inventory"):
            self.score()
        self.inventory.pop(0)
        with self.assertRaisesRegex(ValueError, "silent control"):
            self.score()
