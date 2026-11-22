"""Tests for scripts/download_models.py and scripts/models.lock.json.

Run with: python3 -m unittest scripts/test_download_models.py
"""
import contextlib
import hashlib
import http.server
import io
import json
import os
import pathlib
import re
import tempfile
import threading
import unittest
from pathlib import Path
from unittest import mock

import download_models as dm

PROFILES = ["p0_mobile_ultralight", "p1_mobile_flagship", "p2_workstation_16g", "p3_workstation_heavy"]


class LockIntegrity(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.lock = dm.load_lock()

    def test_every_profile_resolves_to_known_models(self):
        for profile in PROFILES:
            with self.subTest(profile=profile):
                ids = dm.resolve_profile(self.lock, profile)
                self.assertTrue(ids)
                self.assertEqual(len(ids), len(set(ids)), "resolved ids must be unique")

    def test_lock_defines_exactly_the_four_spec_profiles(self):
        self.assertEqual(sorted(self.lock["profiles"]), sorted(PROFILES))

    def test_common_voice_group_is_expanded_into_every_profile(self):
        common = self.lock["groups"]["common_voice"]
        for profile in PROFILES:
            with self.subTest(profile=profile):
                ids = dm.resolve_profile(self.lock, profile)
                self.assertEqual(ids[: len(common)], common)

    def test_pinned_entries_have_full_digest_size_and_https_url(self):
        for entry in self.lock["models"]:
            if not dm.is_pinned(entry):
                continue
            with self.subTest(model=entry["id"]):
                self.assertRegex(entry["sha256"], r"^[0-9a-f]{64}$")
                self.assertIsInstance(entry["size_bytes"], int)
                self.assertGreater(entry["size_bytes"], 0)
                self.assertTrue(entry["url"].startswith("https://"))

    def test_destinations_are_relative_and_stay_inside_the_repo(self):
        for entry in self.lock["models"]:
            with self.subTest(model=entry["id"]):
                dest = Path(entry["dest"])
                self.assertFalse(dest.is_absolute())
                self.assertNotIn("..", dest.parts)

    def test_model_ids_are_unique(self):
        ids = [m["id"] for m in self.lock["models"]]
        self.assertEqual(len(ids), len(set(ids)))

    def test_no_default_profile_pulls_in_an_agpl_model(self):
        agpl = {m["id"] for m in self.lock["models"] if m.get("license", "").startswith("AGPL")}
        self.assertTrue(agpl, "expected at least one AGPL model to guard")
        for profile in PROFILES:
            with self.subTest(profile=profile):
                self.assertFalse(agpl & set(dm.resolve_profile(self.lock, profile)))

    def test_every_profile_model_is_pinned_and_fetchable(self):
        for profile in PROFILES:
            with self.subTest(profile=profile):
                for model_id in dm.resolve_profile(self.lock, profile):
                    entry = dm.select_entries(self.lock, ids=[model_id])[0]
                    self.assertTrue(dm.is_pinned(entry), f"{model_id} is unpinned")


class ProfileResolution(unittest.TestCase):
    def setUp(self):
        self.lock = {
            "schema_version": 3,
            "groups": {"voice": ["a", "b"]},
            "profiles": {"p0_mobile_ultralight": ["voice", "c", "a"]},
            "models": [{"id": i, "dest": f"{i}.bin", "url": None, "sha256": None} for i in "abc"],
        }

    def test_groups_expand_and_duplicates_collapse_in_order(self):
        self.assertEqual(dm.resolve_profile(self.lock, "p0_mobile_ultralight"), ["a", "b", "c"])

    def test_unknown_profile_is_rejected_with_the_valid_choices(self):
        with self.assertRaisesRegex(dm.LockError, "choose from p0_mobile_ultralight"):
            dm.resolve_profile(self.lock, "p9_nope")

    def test_profile_referencing_a_missing_model_is_rejected(self):
        self.lock["profiles"]["p0_mobile_ultralight"].append("ghost")
        with self.assertRaisesRegex(dm.LockError, "unknown model 'ghost'"):
            dm.resolve_profile(self.lock, "p0_mobile_ultralight")

    def test_unsupported_schema_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "lock.json"
            path.write_text(json.dumps({"schema_version": 2, "models": []}))
            with self.assertRaisesRegex(dm.LockError, "unsupported lock schema"):
                dm.load_lock(path)


class PlanAndRefusal(unittest.TestCase):
    """These tests never touch the network: dry-run and unpinned refusal happen before any fetch."""

    def run_main(self, argv):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            code = dm.main(argv)
        return code, out.getvalue()

    def test_fully_pinned_profile_dry_run_succeeds(self):
        code, out = self.run_main(["--profile", "p0_mobile_ultralight", "--dry-run"])
        self.assertEqual(code, 0)
        self.assertNotIn("UNPINNED", out)

    def test_p3_dry_run_lists_the_nemotron_asr_artifacts(self):
        code, out = self.run_main(["--profile", "p3_workstation_heavy", "--dry-run"])
        self.assertEqual(code, 0)
        self.assertRegex(out, r"PINNED\s+nemotron-asr-160ms-encoder-int8")
        self.assertNotIn("UNPINNED", out)

    def test_fetching_an_unpinned_model_is_refused_without_network(self):
        unpinned_lock = {
            "schema_version": 3,
            "models": [
                {"id": "unpinned-probe", "dest": "probe/model.onnx", "url": "https://example.invalid/m.onnx",
                 "sha256": None, "size_bytes": None, "license": "Apache-2.0"},
            ],
        }
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(dm.urllib.request, "urlopen") as opener:
            lock_path = pathlib.Path(tmp) / "lock.json"
            with open(lock_path, "w", encoding="utf-8") as fh:
                json.dump(unpinned_lock, fh)
            out = io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
                code = dm.main(["--ids", "unpinned-probe", "--root", tmp], lock_path=lock_path, root=tmp)
            opener.assert_not_called()
            self.assertEqual(code, 1)
            self.assertEqual(os.listdir(tmp), ["lock.json"])

    def test_unknown_profile_exits_with_usage_error(self):
        code, _ = self.run_main(["--profile", "p9"])
        self.assertEqual(code, 2)


class ResumableFetch(unittest.TestCase):
    """Exercises the real download path against a local HTTP server that honours or ignores Range."""

    @classmethod
    def setUpClass(cls):
        cls.payload = os.urandom(3 * 1024 * 1024 + 17)
        cls.ignore_range = False
        payload = cls.payload
        test = cls

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                start = 0
                match = re.match(r"bytes=(\d+)-", self.headers.get("Range", ""))
                if match and not test.ignore_range:
                    start = int(match.group(1))
                    self.send_response(206)
                    self.send_header("Content-Range", f"bytes {start}-{len(payload) - 1}/{len(payload)}")
                else:
                    self.send_response(200)
                body = payload[start:]
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        cls.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()
        cls.url = f"http://127.0.0.1:{cls.server.server_address[1]}/payload.bin"
        cls.digest = hashlib.sha256(payload).hexdigest()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.entry = {"id": "t", "dest": "m/payload.bin", "url": self.url, "sha256": self.digest}
        self.dest = self.root / "m" / "payload.bin"
        self.part = self.dest.with_name("payload.bin.part")
        type(self).ignore_range = False

    def tearDown(self):
        type(self).ignore_range = False
        self.tmp.cleanup()

    def test_resumes_from_an_existing_part_file(self):
        self.part.parent.mkdir(parents=True)
        self.part.write_bytes(self.payload[: 1024 * 1024])
        dm.install(self.entry, self.root)
        self.assertEqual(self.dest.read_bytes(), self.payload)
        self.assertFalse(self.part.exists())

    def test_stale_part_is_replaced_when_the_server_ignores_range(self):
        self.part.parent.mkdir(parents=True)
        self.part.write_bytes(b"stale-garbage" * 1000)
        type(self).ignore_range = True
        dm.install(self.entry, self.root)
        self.assertEqual(self.dest.read_bytes(), self.payload)

    def test_digest_mismatch_commits_nothing_and_removes_the_part_file(self):
        bad = dict(self.entry, sha256="0" * 64)
        with self.assertRaisesRegex(RuntimeError, "digest mismatch"):
            dm.install(bad, self.root)
        self.assertFalse(self.dest.exists())
        self.assertFalse(self.part.exists())

    def test_verified_file_is_not_fetched_again(self):
        self.dest.parent.mkdir(parents=True)
        self.dest.write_bytes(self.payload)
        with mock.patch.object(dm, "fetch_resumable") as fetch:
            dm.install(self.entry, self.root)
            fetch.assert_not_called()


class ArchiveInstall(unittest.TestCase):
    """Exercises the pinned-archive path with a local tarball served over file://."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.src = self.root / "src"
        self.src.mkdir()
        self.payload = b"encoder-bytes" * 4000
        (self.src / "encoder.onnx").write_bytes(self.payload)
        (self.src / "tokens.txt").write_text("a 0\nb 1\n")
        self.archive_path = self.root / "release.tar.bz2"
        import tarfile as tf
        with tf.open(self.archive_path, "w:bz2") as tar:
            tar.add(self.src / "encoder.onnx", arcname="pkg/encoder.onnx")
            tar.add(self.src / "tokens.txt", arcname="pkg/tokens.txt")
        self.archive_sha = hashlib.sha256(self.archive_path.read_bytes()).hexdigest()
        self.member_sha = hashlib.sha256(self.payload).hexdigest()
        self.url = self.archive_path.as_uri()
        self.dest_root = self.root / "install"
        self.dest_root.mkdir()

    def tearDown(self):
        self.tmp.cleanup()

    def entry(self, **overrides):
        base = {
            "id": "enc",
            "dest": "perception/models/asr/encoder.onnx",
            "url": self.url,
            "sha256": self.member_sha,
            "size_bytes": len(self.payload),
            "archive": {"url": self.url, "sha256": self.archive_sha, "size_bytes": None, "member": "encoder.onnx"},
        }
        base.update(overrides)
        return base

    def test_member_is_extracted_verified_and_installed(self):
        dm.install(self.entry(), self.dest_root)
        self.assertEqual((self.dest_root / "perception/models/asr/encoder.onnx").read_bytes(), self.payload)
        self.assertTrue((self.dest_root / "perception/models/downloads" / self.archive_path.name).exists())

    def test_archive_digest_mismatch_installs_nothing(self):
        bad = self.entry()
        bad["archive"]["sha256"] = "0" * 64
        with self.assertRaisesRegex(RuntimeError, "archive digest mismatch"):
            dm.install(bad, self.dest_root)
        self.assertFalse((self.dest_root / "perception/models/asr/encoder.onnx").exists())

    def test_member_digest_mismatch_is_refused(self):
        with self.assertRaisesRegex(RuntimeError, "digest mismatch"):
            dm.install(self.entry(sha256="1" * 64), self.dest_root)
        self.assertFalse((self.dest_root / "perception/models/asr/encoder.onnx").exists())

    def test_missing_member_is_refused(self):
        with self.assertRaisesRegex(RuntimeError, "expected exactly one"):
            dm.install(self.entry(archive=dict(self.entry()["archive"], member="absent.onnx")), self.dest_root)


if __name__ == "__main__":
    unittest.main()
