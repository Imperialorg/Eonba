"""RFC 8489 IPv4 Binding subset, only for test_wan_nat.sh's private namespaces.

Fixed origin, two provisioned users, short-term MESSAGE-INTEGRITY-SHA256.
No alternate server, name resolution, public listener or peer signalling.
"""

import hmac
import os
import socket
import struct
import sys
import time
from pathlib import Path

SERVER = ("10.77.0.1", 3478)
COOKIE = 0x2112A442
HEADER = struct.Struct("!HHI12s")
MI = struct.pack("!HH", 0x001C, 32)
CLIENTS = {"a": ("192.168.1.2", "10.77.0.2"), "b": ("192.168.2.2", "10.77.0.3")}


def signed(kind, transaction, attributes, key):
    prefix = HEADER.pack(kind, len(attributes) + 36, COOKIE, transaction) + attributes
    return prefix + MI + hmac.digest(key, prefix, "sha256")


def verified(data, kind, key, expected_id=None):
    if not 56 <= len(data) <= 256:
        raise ValueError("bad STUN length")
    got, length, cookie, tx = HEADER.unpack_from(data)
    if (got, length, cookie) != (kind, len(data) - HEADER.size, COOKIE):
        raise ValueError("bad STUN header")
    if expected_id is not None and tx != expected_id:
        raise ValueError("wrong STUN transaction")
    if data[-36:-32] != MI or not hmac.compare_digest(data[-32:], hmac.digest(key, data[:-36], "sha256")):
        raise ValueError("bad STUN integrity")
    return tx, data[HEADER.size:-36]


def username(user):
    value = user.encode("ascii")
    return struct.pack("!HH", 0x0006, len(value)) + value + bytes(-len(value) % 4)


def mapped(addr):
    ip, port = addr
    value = int.from_bytes(socket.inet_pton(socket.AF_INET, ip), "big") ^ COOKIE
    return struct.pack("!HHBBHI", 0x0020, 8, 0, 1, port ^ (COOKIE >> 16), value)


def decode_mapped(attr):
    kind, length, _, family, port, value = struct.unpack("!HHBBHI", attr)
    if (kind, length, family) != (0x0020, 8, 1):
        raise ValueError("expected IPv4 XOR-MAPPED-ADDRESS")
    return socket.inet_ntop(socket.AF_INET, (value ^ COOKIE).to_bytes(4, "big")), port ^ (COOKIE >> 16)


def credential(side):
    return Path(f"keys/stun-{side}.psk").read_bytes().strip()


def paired(pin, addr):
    ip, port = addr
    return struct.pack("!HH32s4sH2x", 0xC0E4, 38, pin, socket.inet_aton(ip), port)


def serve(sock):
    sock.bind(SERVER)
    Path("stun-ready").touch()
    pending, rejected = {}, {}
    deadline = time.monotonic() + 60
    while len(pending) < len(CLIENTS):
        sock.settimeout(max(0.001, deadline - time.monotonic()))
        data, source = sock.recvfrom(257)
        side = next((s for s in CLIENTS if source[0] == CLIENTS[s][1]), None)
        if side is None:
            continue
        try:
            key = credential(side)
            tx, attr = verified(data, 0x0001, key)
            if attr not in (username(side), username(side + "_reject")):
                continue
        except ValueError:
            continue
        if attr == username(side + "_reject"):
            # Negative control: authentic response, correct peer pin, routable
            # private host, but a port outside the configured pool.
            other = "b" if side == "a" else "a"
            pin = bytes.fromhex(Path(f"keys/{other}.pub").read_text().strip())
            attrs = mapped(source) + paired(pin, (CLIENTS[other][1], 41999))
            sock.sendto(signed(0x0101, tx, attrs, key), source)
            rejected[side] = tx
            continue
        if side not in rejected or rejected[side] == tx:
            raise ValueError("missing independent negative-control transaction")
        pending[side] = (source, tx)
        print(f"STUN authenticated Binding side={side} observed={source[0]}:{source[1]} negative_control=sent")
    # Only the independent operator service publishes peer addresses. Use
    # the *observed source*, never an address submitted by a client.
    Path("stun-observed").touch()
    while not Path("stun-reply").exists():
        if time.monotonic() > deadline:
            raise TimeoutError("missing fixture cookie witness")
        time.sleep(0.005)
    for side, (source, tx) in pending.items():
        other = "b" if side == "a" else "a"
        pin = bytes.fromhex(Path(f"keys/{other}.pub").read_text().strip())
        attrs = mapped(source) + paired(pin, pending[other][0])
        sock.sendto(signed(0x0101, tx, attrs, credential(side)), source)


if __name__ == "__main__":
    ns = os.stat("/proc/self/ns/net").st_ino
    if ns == int(os.environ["EON_WAN_PARENT_NS"]) or ns != int(os.environ["EON_WAN_NETNS"]):
        raise SystemExit("requires the isolated namespace fixture")
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        if sys.argv[1:] == ["server"]:
            serve(sock)
        else:
            raise SystemExit("unknown fixture role")
