import copy
import json
import os
import secrets
import struct
import unittest
from pathlib import Path

import wan_private_stun as stun


class PublisherTests(unittest.TestCase):
    def setUp(self):
        self.cfg = {"listen": ["10.77.0.1", 3478], "network": "10.77.0.0/29",
                    "ports": [41769, 41772], "members": [
                        {"user": u, "pin": secrets.token_hex(32), "password": secrets.token_hex(32)}
                        for u in ("one", "two")]}
        self.pub = stun.Publisher(self.cfg)
        self.src = [("10.77.0.2", 41770), ("10.77.0.3", 41771)]

    def packet(self, i, tx, token=None):
        m = self.pub.members[i]
        attrs = m.user + (stun.PROOF + token if token is not None else b"")
        n = max(0, 112 - (56 + len(attrs) + 4))
        return stun.signed(1, tx, attrs + struct.pack("!HH", 0x26, n) + bytes(n), m.key)

    def challenge(self, i, tx, now):
        m = self.pub.members[i]
        reply = self.pub.reply(self.packet(i, tx), self.src[i], now)
        _, attrs = stun.verified(reply, 0x0101, m.key, tx)
        self.assertEqual(attrs[:16], stun.mapped(self.src[i]) + stun.PROOF)
        return attrs[16:]

    def test_return_proof_bound_to_source_identity_and_lifetime(self):
        tx = secrets.token_bytes(12)
        ca = self.challenge(0, tx, 0)
        cb = self.challenge(1, tx, 0)
        a = self.packet(0, tx, ca)
        b = self.packet(1, tx, cb)
        for invalid, source in [(a, self.src[1]), (b, self.src[0]),
                                (self.packet(1, tx, ca), self.src[1]),
                                (self.packet(0, bytes(12), ca), self.src[0])]:
            self.assertIsNone(self.pub.reply(invalid, source, 0))
        self.assertIsNone(self.pub.reply(a, self.src[0], 1))  # peer not yet proven
        reply = self.pub.reply(b, self.src[1], 2)
        self.assertEqual(stun.verified(reply, 0x0101, self.pub.members[1].key, tx)[1],
                         stun.mapped(self.src[1]) + stun.paired(self.pub.members[0].pin, self.src[0]))
        self.assertLessEqual(len(reply), len(b))
        self.assertIsNotNone(self.pub.reply(a, self.src[0], 5))
        self.assertIsNone(self.pub.reply(a, self.src[0], 6))  # retries don't renew
        self.assertNotEqual(ca, self.challenge(0, tx, 7))
        self.assertIsNone(self.pub.reply(a, self.src[0], 7))
        a2 = self.packet(0, tx, self.challenge(0, tx, 8))
        self.assertIsNone(self.pub.reply(a2, self.src[0], 10))  # challenge expired
        self.assertEqual([m.observed for m in self.pub.members], [1, 1])

    def test_limit_no_reply_to_unsigned_public_outside_or_wrong_port(self):
        tx = bytes(range(12))
        req = self.packet(0, tx)
        for src in [("203.0.113.1", 41770), ("10.77.0.0", 41770), ("10.77.0.7", 41770),
                    ("10.77.0.1", 41770), ("10.77.0.2", 22)]:
            self.assertIsNone(self.pub.reply(req, src, 0))
        m = self.pub.members[0]
        padding = req[20 + len(m.user):-36]
        for bad in (req[:-1], req + b"\0", req[:-1] + bytes([req[-1] ^ 1]),
                    stun.signed(1, tx, m.user, m.key),
                    stun.signed(1, tx, stun.username("unknown") + padding, m.key),
                    stun.signed(1, tx, m.user + stun.mapped(self.src[1]) + padding, m.key),
                    stun.signed(1, tx, m.user + padding[:-1] + b"\x01", m.key)):
            self.assertIsNone(self.pub.reply(bad, self.src[0], 0))
        for _ in range(8):
            self.assertLessEqual(len(self.pub.reply(req, self.src[0], 0)), len(req))
        self.assertIsNone(self.pub.reply(req, self.src[0], 0.24))
        self.assertIsNotNone(self.pub.reply(req, self.src[0], 0.25))
        self.assertIsNotNone(self.pub.reply(self.packet(1, tx), self.src[1], 0))
        self.assertTrue(all(m.live is None for m in self.pub.members))

    def test_private_literal_policy_and_file_permissions(self):
        for k, v in [("listen", ["0.0.0.0", 3478]), ("listen", ["localhost", 3478]),
                     ("listen", ["10.77.0.7", 3478]), ("ports", {"41769": 0, "41772": 0}),
                     ("network", "10.0.0.0/8"), ("network", "203.0.113.0/24"),
                     ("ports", [1024, 3000]), ("members", [self.cfg["members"][0]] * 2)]:
            with self.assertRaises((TypeError, ValueError)):
                stun.Publisher({**self.cfg, k: v})
        for k in ("user", "pin", "password"):
            cfg = copy.deepcopy(self.cfg)
            cfg["members"][1][k] = cfg["members"][0][k]
            with self.assertRaises(ValueError):
                stun.Publisher(cfg)
        scratch = Path(__file__).resolve().parents[1] / "engine/core-rs/target/wan-build"
        scratch.mkdir(mode=0o700, parents=True, exist_ok=True)
        private_dir = scratch / ("publisher-test-" + secrets.token_hex(8))
        private_dir.mkdir(mode=0o700)
        path, link = private_dir / "pair.json", private_dir / "link"
        try:
            with os.fdopen(os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), "w") as f:
                json.dump(self.cfg, f)
            self.assertEqual(stun.load_config(path).listen, ("10.77.0.1", 3478))
            link.symlink_to(path)
            with self.assertRaises(OSError):
                stun.load_config(link)
            link.unlink()
            os.link(path, link)
            with self.assertRaises(ValueError):
                stun.load_config(path)
            link.unlink()
            path.chmod(0o644)
            with self.assertRaises(ValueError):
                stun.load_config(path)
            path.chmod(0o600)
            path.write_text('{"ports":[1,2],' + json.dumps(self.cfg)[1:])
            with self.assertRaises(ValueError):
                stun.load_config(path)
        finally:
            link.unlink(missing_ok=True)
            path.unlink(missing_ok=True)
            private_dir.rmdir()


class BindingTests(unittest.TestCase):
    def test_ipv4_xor_address_rfc5769(self):
        attr = bytes.fromhex("002000080001a147e112a643")
        self.assertEqual(stun.decode_mapped(attr), ("192.0.2.1", 32853))
        self.assertEqual(stun.mapped(("192.0.2.1", 32853)), attr)

    def test_authenticated_response_is_specific_to_request(self):
        key, tx = b"fixture credential", bytes(range(12))
        attr = stun.mapped(("10.77.0.2", 41770))
        reply = stun.signed(0x0101, tx, attr, key)
        self.assertEqual(stun.verified(reply, 0x0101, key, tx), (tx, attr))
        for packet, kind, credential, nonce in [
            (reply, 0x0101, b"other", tx),
            (reply, 0x0101, key, bytes(12)),
            (reply, 0x0111, key, tx),
            (reply[:27] + bytes([reply[27] ^ 1]) + reply[28:], 0x0101, key, tx),
            (reply[:-1], 0x0101, key, tx),
            (reply + b"\0", 0x0101, key, tx),
        ]:
            with self.assertRaises(ValueError):
                stun.verified(packet, kind, credential, nonce)


if __name__ == "__main__":
    unittest.main()
