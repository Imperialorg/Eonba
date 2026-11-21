"""Private IPv4 STUN Binding publisher for one explicitly provisioned pair."""

import hmac
import ipaddress
import json
import os
import re
import secrets
import signal
import socket
import stat
import struct
import sys
import time

COOKIE = 0x2112A442
HEADER = struct.Struct("!HHI12s")
MI = struct.pack("!HH", 0x001C, 32)
PROOF = struct.pack("!HH", 0xC0E5, 16)
RFC1918 = tuple(map(ipaddress.IPv4Network, ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16")))
MAX_COUNT = (1 << 64) - 1


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


def paired(pin, addr):
    ip, port = addr
    return struct.pack("!HH32s4sH2x", 0xC0E4, 38, pin, socket.inet_aton(ip), port)


def require(ok):
    if not ok:
        raise ValueError("invalid private publisher configuration")


def load_config(path):
    require(os.path.isabs(path))
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as f:
        st = os.fstat(f.fileno())
        require(stat.S_ISREG(st.st_mode) and st.st_uid == os.geteuid()
                and st.st_nlink == 1 and stat.S_IMODE(st.st_mode) in (0o400, 0o600))
        data = f.read(4097)
    require(len(data) <= 4096)

    def unique_fields(items):
        obj = dict(items)
        require(len(obj) == len(items))
        return obj

    return Publisher(json.loads(data, object_pairs_hook=unique_fields))


class Member:
    def __init__(self, spec):
        require(isinstance(spec, dict) and set(spec) == {"user", "pin", "password"})
        require(re.fullmatch(r"[a-zA-Z0-9_-]{1,64}", spec["user"]))
        require(re.fullmatch("[0-9a-f]{64}", spec["pin"]) and int(spec["pin"], 16) != 0)
        require(re.fullmatch("[0-9a-f]{64}", spec["password"]))
        self.user, self.pin = username(spec["user"]), bytes.fromhex(spec["pin"])
        self.key = spec["password"].encode("ascii")
        self.pending = self.live = None  # (expiry, source, transaction, random return cookie)
        self.credit, self.last = 8, 0
        self.observed = self.published = 0

    def expire(self, now):
        if self.pending and now >= self.pending[0]:
            self.pending = None
        if self.live and now >= self.live[0]:
            self.live = None

    def allow(self, now):
        self.credit = min(8, self.credit + 4 * (now - self.last))
        self.last = now
        if self.credit < 1:
            return False
        self.credit -= 1
        return True


class Publisher:
    def __init__(self, cfg):
        require(isinstance(cfg, dict) and set(cfg) == {"listen", "network", "ports", "members"})
        require(all(isinstance(cfg[k], list) and len(cfg[k]) == 2 for k in ("listen", "ports", "members")))
        host, port = cfg["listen"]
        require(isinstance(host, str) and type(port) is int and 1024 <= port <= 65535)
        ip = ipaddress.IPv4Address(host)
        self.net = ipaddress.IPv4Network(cfg["network"], strict=True)
        require(str(self.net) == cfg["network"] and self.net.prefixlen >= 24
                and any(self.net.subnet_of(n) for n in RFC1918))
        require(any(ip in n for n in RFC1918) and not self.reserved(ip))
        self.listen = (str(ip), port)
        self.lo, self.hi = cfg["ports"]
        require(type(self.lo) is type(self.hi) is int and 1024 <= self.lo <= self.hi <= 65535
                and self.hi - self.lo < 256)
        a, b = self.members = tuple(Member(m) for m in cfg["members"])
        require(a.user != b.user and a.pin != b.pin and a.key != b.key)

    def reserved(self, ip):
        return self.net.prefixlen < 31 and ip in (self.net.network_address, self.net.broadcast_address)

    def reply(self, data, source, now):
        for member in self.members:
            member.expire(now)
        ip = ipaddress.IPv4Address(source[0])
        if (ip not in self.net or source[0] == self.listen[0] or not self.lo <= source[1] <= self.hi
                or self.reserved(ip) or not 112 <= len(data) <= 256):
            return None
        for i, member in enumerate(self.members):
            if not data[20:].startswith(member.user):
                continue
            try:
                tx, attrs = verified(data, 1, member.key)
            except ValueError:
                return None
            rest = attrs[len(member.user):]
            proof = None
            if rest.startswith(PROOF) and len(rest) >= 20:
                proof, rest = rest[4:20], rest[20:]
            # Only zero PADDING is allowed after USERNAME and optional proof.
            if (len(rest) < 4 or rest[:4] != struct.pack("!HH", 0x0026, len(rest)-4)
                    or any(rest[4:]) or len(rest) % 4 or not member.allow(now)):
                return None
            peer = self.members[1 - i]
            if proof is None:
                if not member.pending or member.pending[1:3] != (source, tx):
                    member.pending = (now + 3, source, tx, secrets.token_bytes(16))
                attrs = mapped(source) + PROOF + member.pending[3]
            else:
                def matches(record):
                    return record and record[1:3] == (source, tx) and hmac.compare_digest(record[3], proof)
                if matches(member.pending):
                    member.live, member.pending = (now + 5, *member.pending[1:]), None
                    member.observed = min(MAX_COUNT, member.observed + 1)
                elif not matches(member.live):
                    return None
                if not peer.live or peer.live[1] == source:
                    return None
                attrs = mapped(source) + paired(peer.pin, peer.live[1])
                member.published = min(MAX_COUNT, member.published + 1)
            # At most one <=112-byte response to this >=112-byte authenticated
            # request. Destination is always recvfrom's source, never wire data.
            return signed(0x0101, tx, attrs, member.key)
        return None


def serve(service):
    running = True

    def stop(*_):
        nonlocal running
        running = False

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(service.listen)
        sock.settimeout(0.5)
        print("private pair publisher ready", flush=True)
        while running:
            try:
                data, source = sock.recvfrom(257)
            except TimeoutError:
                for member in service.members:
                    member.expire(time.monotonic())
                continue
            response = service.reply(data, source, time.monotonic())
            if response is not None:
                try:
                    sock.sendto(response, source)
                except OSError:
                    pass  # UDP route loss; clients can retransmit
    for i, m in enumerate(service.members):
        print(f"member={i} confirmed={m.observed} paired_replies={m.published}")


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] != "serve":
        raise SystemExit("usage: wan_private_stun.py serve /absolute/private-config.json")
    try:
        serve(load_config(sys.argv[2]))
    except (OSError, ValueError, TypeError, RecursionError):
        raise SystemExit("private pair publisher could not start or continue") from None
