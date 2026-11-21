"""RFC 8489 IPv4 Binding subset, only for test_wan_nat.sh's private namespaces.

Fixed origin, two provisioned users, short-term MESSAGE-INTEGRITY-SHA256.
No alternate server, name resolution, public listener or peer signalling.
"""

import hmac
import os
import secrets
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


def username(side):
    return struct.pack("!HH1s3x", 0x0006, 1, side.encode("ascii"))


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


def serve(sock):
    sock.bind(SERVER)
    Path("stun-ready").touch()
    outstanding = set(CLIENTS)
    deadline = time.monotonic() + 12
    while outstanding:
        sock.settimeout(max(0.001, deadline - time.monotonic()))
        data, source = sock.recvfrom(257)
        side = next((s for s in outstanding if source[0] == CLIENTS[s][1]), None)
        if side is None:
            continue
        try:
            key = credential(side)
            tx, attr = verified(data, 0x0001, key)
            if attr != username(side):
                continue
        except ValueError:
            continue
        sock.sendto(signed(0x0101, tx, mapped(source), key), source)
        print(f"STUN authenticated Binding side={side} observed={source[0]}:{source[1]}")
        outstanding.remove(side)


def probe(sock, side):
    sock.bind((CLIENTS[side][0], 41770))
    sock.connect(SERVER)
    sock.settimeout(3)
    key = credential(side)
    tx = secrets.token_bytes(12)
    sock.send(signed(0x0001, tx, username(side), key))
    _, attr = verified(sock.recv(257), 0x0101, key, tx)
    host, port = decode_mapped(attr)
    if (host, port) != (CLIENTS[side][1], 41770):
        raise ValueError("unexpected fixture SNAT mapping")
    Path(f"stun-mapped-{side}").write_text(f"{host}:{port}")


if __name__ == "__main__":
    ns = os.stat("/proc/self/ns/net").st_ino
    if ns == int(os.environ["EON_WAN_PARENT_NS"]) or ns != int(os.environ["EON_WAN_NETNS"]):
        raise SystemExit("requires the isolated namespace fixture")
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        if sys.argv[1:] == ["server"]:
            serve(sock)
        elif len(sys.argv) == 2 and sys.argv[1] in CLIENTS:
            probe(sock, sys.argv[1])
        else:
            raise SystemExit("unknown fixture role")
