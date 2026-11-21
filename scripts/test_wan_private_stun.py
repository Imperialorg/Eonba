import unittest

import wan_private_stun as stun


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
