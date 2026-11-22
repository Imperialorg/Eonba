"""Fixed recorded inputs for the software render benchmark. No devices or network."""

import argparse
from array import array
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import struct
import subprocess
import sys
import tarfile
import tempfile

import noisy_duplex as duplex

RATE = duplex.RATE
COUNT = 100
LEAD = RATE // 5
TAIL = RATE * 2
SCENES = ("clean", "babble-0db", "traffic-0db", "tv-0db")
CORPUS = "voice-test-librispeech-dev-clean"
MEMBER = re.compile(r"LibriSpeech/dev-clean/(\d+)/(\d+)/(\1-\2-\d+)\.flac")
MODELS = {
    "silero-vad-v5": "vad/silero_vad.onnx",
    "zipformer-en-2023-06-26-encoder-int8": "asr/encoder-epoch-99-avg-1-chunk-16-left-128.int8.onnx",
    "zipformer-en-2023-06-26-decoder": "asr/decoder-epoch-99-avg-1-chunk-16-left-128.onnx",
    "zipformer-en-2023-06-26-joiner-int8": "asr/joiner-epoch-99-avg-1-chunk-16-left-128.int8.onnx",
    "zipformer-en-2023-06-26-tokens": "asr/tokens.txt",
    "kokoro-v1.0-release-int8": "tts/release/kokoro-v1.0.int8.onnx",
    "kokoro-v1.0-voice-af_heart": "tts/voices/af_heart.bin",
}
ROW_KEYS = {"take", "status", "last_active_end_sample", "endpoint_sample", "latency_us"}
INPUT_ERRORS = duplex.INPUT_ERRORS + (KeyError, subprocess.SubprocessError)


def stream_frames(header):
    # First FLAC metadata block must be a 34-byte STREAMINFO block.
    if len(header) != 42 or header[:4] != b"fLaC" or header[4] & 127 or header[5:8] != b"\x00\x00\x22":
        raise ValueError("invalid LibriSpeech FLAC header")
    info = int.from_bytes(header[18:26], "big")
    if (info >> 44, (info >> 41) & 7, (info >> 36) & 31) != (RATE, 0, 15):
        raise ValueError("expected a mono 16 kHz PCM16 LibriSpeech recording")
    return info & ((1 << 36) - 1)


def select_clips(file, count=COUNT):
    # Rank the whole eligible corpus by ID, independently of archive order,
    # recognition, identity scores, and measured timings. Keep complete utterances.
    clips, ids = [], set()
    with tarfile.open(fileobj=file, mode="r|gz") as archive:
        for member in archive:
            match = MEMBER.fullmatch(member.name)
            if not (match and member.isfile()):
                continue
            key = match[3]
            if key in ids:
                raise ValueError("duplicate corpus utterance")
            ids.add(key)
            rank = hashlib.sha256(key.encode("ascii")).digest(), key
            if len(clips) == count and rank >= clips[-1][0]:
                continue
            with archive.extractfile(member) as src:
                header = src.read(42)
                frames = stream_frames(header)
                if not RATE <= frames <= 12 * RATE:
                    continue
                if not 42 < member.size <= 1024 * 1024:
                    raise ValueError("invalid compressed size for a selected utterance")
                clips.append((rank, frames, header + src.read()))
                clips.sort(key=lambda c: c[0])
                del clips[count:]
    if len(clips) != count:
        raise ValueError(f"need {count} distinct complete utterances")
    return [(key, frames, flac) for (_, key), frames, flac in clips]


def decode(flac, frames):
    raw = subprocess.run(["flac", "--silent", "--decode", "--stdout", "--force-raw-format",
                          "--endian=little", "--sign=signed", "-"], input=flac,
                         stdout=subprocess.PIPE, check=True, timeout=30).stdout
    if len(raw) != 2 * frames:
        raise ValueError("decoded length differs from FLAC STREAMINFO")
    return raw


def channels(raw, scene, tracks):
    pcm = array("h")
    pcm.frombytes(raw)
    if sys.byteorder != "little":
        pcm.byteswap()
    dry = (0,) * LEAD + tuple(pcm) + (0,) * TAIL
    if scene == "clean":
        mixed, snr = dry, None
    else:
        bed = {"babble-0db": lambda n: duplex.babble(n, tracks), "traffic-0db": duplex.traffic,
               "tv-0db": lambda n: duplex.tv_dialogue(n, tracks)}[scene](len(dry))
        # Match foreground/background power over the actual recording, not its
        # added silence. Continue the SAME bed throughout the lead and tail.
        mixed, snr = duplex.mix_at_zero_db(dry, bed, interval=(LEAD, LEAD + len(pcm)))
    stereo = array("h", [0]) * (2 * len(dry))
    stereo[::2], stereo[1::2] = array("h", dry), array("h", mixed)
    if sys.byteorder != "little":
        stereo.byteswap()
    return stereo.tobytes(), snr


def write_take(file, stereo):
    frames = len(stereo) // 4
    if not 0 < frames <= 30 * RATE or len(stereo) != 4 * frames:
        raise ValueError("invalid software take")
    file.write(struct.pack("<I", frames))
    file.write(stereo)
    return frames


def last_active(raw):
    # Independent dry-reference check also detects missing or reordered result rows.
    samples = [x[0] for x in struct.iter_unpack("<h", raw)]
    for start in reversed(range(0, len(samples), RATE // 1000)):
        window = samples[start:start + RATE // 1000]
        # Native bins include appended zeroes up to the next millisecond.
        if sum(x * x for x in window) >= 100 ** 2 * (RATE // 1000):
            return LEAD + start + RATE // 1000
    raise ValueError("utterance has no foreground above the fixed activity threshold")


def write_batch(file, models):
    entry = models[CORPUS]
    with (duplex.ROOT / entry["dest"]).open("rb") as archive:
        duplex.verify(archive, entry)
        archive.seek(0)
        clips = select_clips(archive)
    tracks = duplex.speech_tracks(models["voice-test-sensevoice-en"]["archive"])
    control = write_take(file, bytes(4 * TAIL))
    inventory = [{"id": "silent-control", "scene": "silence", "frames": control, "dry_end": None}]
    seen = set()
    for i, (key, n, flac) in enumerate(clips):
        raw = decode(flac, n)
        digest = hashlib.sha256(raw).hexdigest()
        if digest in seen:
            raise ValueError("duplicate source PCM under different utterance IDs")
        seen.add(digest)
        scene = SCENES[i % len(SCENES)]
        stereo, snr = channels(raw, scene, tracks)
        inventory.append({"id": key, "scene": scene, "source_sha256": digest, "dry_end": last_active(raw),
                          "frames": write_take(file, stereo), "snr_db": snr})
    file.write(struct.pack("<I", 0))
    file.seek(0)
    return inventory


def observation(row, take):
    if not isinstance(row, dict) or row.keys() != ROW_KEYS:
        raise ValueError("unexpected responder schema")
    status, last, end, us = (row[k] for k in ("status", "last_active_end_sample", "endpoint_sample", "latency_us"))
    for offset in (last, end):
        if offset is not None and (type(offset) is not int or not 1 <= offset <= take["frames"]):
            raise ValueError("invalid responder sample offset")
    if last != take["dry_end"]:
        raise ValueError("responder reference differs from supplied dry track")
    if end is not None and end % 512:
        raise ValueError("endpoint is not on a VAD frame boundary")
    if status == "ok":
        if end is None or last is None or end < last or type(us) is not int or us <= 0:
            raise ValueError("invalid render measurement")
        # Start of the last active input bin to the endpoint, plus the first
        # 1 ms sink tick. Rendering cannot start before the endpoint.
        if us * RATE < (end - last + 2 * (RATE // 1000)) * 1_000_000:
            raise ValueError("render measurement precedes the first possible sink tick")
    elif us is not None or not (
            status == "no_reply"
            or status == "no_input_activity" and last is None and end is not None
            or status == "early_endpoint" and last is not None and end is not None and end < last
            or status == "no_render_onset" and last is not None and end is not None and end >= last):
        raise ValueError("invalid responder outcome")
    return row


def evaluate(inventory, output, native_error=None):
    if ([t["scene"] for t in inventory] != ["silence", *(SCENES[i % len(SCENES)] for i in range(COUNT))]
            or len({t["id"] for t in inventory}) != COUNT + 1):
        raise ValueError("need one silent control and the complete distinct scene inventory")
    lines = output.splitlines()
    indexed, seen, errors = {}, set(), []
    silent_bridge = False
    for line in lines:
        try:
            obj = json.loads(line)
            if obj == {"startup_failure": "silent_bridge"} and len(lines) == 1 and native_error:
                silent_bridge = True
                break
            n = obj.get("take") if isinstance(obj, dict) else None
            if type(n) is not int or not 1 <= n <= len(inventory):
                raise ValueError("invalid take ordinal")
            if n in seen:
                indexed.pop(n, None)
                raise ValueError(f"duplicate take {n}")
            seen.add(n)
            try:
                indexed[n] = observation(obj, inventory[n - 1])
            except ValueError as error:
                raise ValueError(f"take {n}: {error}") from error
        except ValueError as error:
            errors.append(str(error))
    if len(indexed) != len(inventory):
        errors.append(f"missing valid observations for {len(inventory) - len(indexed)} takes")
    rows = []
    failed = silent_bridge
    for n, take in enumerate(inventory, 1):
        if n not in indexed:
            rows.append({**take, "take": n, "status": "unavailable", "latency_us": None})
            continue
        row = indexed[n]
        silent = take["scene"] == "silence"
        if silent:
            failed |= not (row["status"] == "no_reply" and row["endpoint_sample"] is None)
        else:
            failed |= row["status"] != "ok"
        rows.append({**take, **row})
    measurements = [r for r in rows if r["scene"] != "silence"]
    times = sorted(r["latency_us"] for r in measurements if r["status"] == "ok")
    p50 = p99 = None
    if len(times) == COUNT:
        p50, p99 = (times[(COUNT * p + 99) // 100 - 1] for p in (50, 99))
    # Even an incomplete run can have enough known late observations to
    # refute a nearest-rank bound regardless of all remaining results.
    failed |= sum(t >= 170_000 for t in times) > COUNT - (COUNT * 50 + 99) // 100
    failed |= sum(t >= 280_000 for t in times) > COUNT - (COUNT * 99 + 99) // 100
    scenes = {}
    for name in SCENES:
        trials = [r for r in measurements if r["scene"] == name]
        ok = [r["latency_us"] for r in trials if r["status"] == "ok"]
        if name != "clean":
            failed |= any(t >= 200_000 for t in ok)
        scenes[name] = {"takes": len(trials), "misses": sum(r["status"] not in ("ok", "unavailable") for r in trials),
                        "unavailable": sum(r["status"] == "unavailable" for r in trials),
                        "max_us": max(ok) if len(ok) == len(trials) and ok else None}
    reason = "; ".join(filter(None, (native_error, *errors[:4])))
    status = "failed" if failed else "unavailable" if reason or p99 is None else "passed"
    return {"status": status, "clock": "paced-input-to-software-render", "p50_us": p50, "p99_us": p99,
            "scenes": scenes, "takes": rows, **({"reason": reason} if reason else {}),
            **({"startup_failure": "silent_bridge"} if silent_bridge else {})}


def build_responder():
    # Use Cargo's artifact for this source and configuration, not a possibly
    # stale binary left in target/release by another build or target directory.
    build = subprocess.run(["cargo", "build", "--offline", "--locked", "--release", "-p", "eon-voice",
                            "--bin", "eon-rig-turn", "--message-format=json-render-diagnostics"],
                           cwd=duplex.ROOT / "engine/core-rs", stdout=subprocess.PIPE, check=True, timeout=45 * 60)
    artifacts = [r["executable"] for r in map(json.loads, build.stdout.splitlines())
                 if r.get("reason") == "compiler-artifact" and r["target"]["name"] == "eon-rig-turn" and r.get("executable")]
    if len(artifacts) != 1 or not Path(artifacts[0]).is_absolute():
        raise ValueError("Cargo did not identify the release responder executable")
    return Path(artifacts[0])


def measure():
    try:
        models = duplex.locked_models()
        if duplex.verified_paths(models, MODELS) != {k: duplex.MODELS / v for k, v in MODELS.items()}:
            raise ValueError("verified model paths differ from responder paths")
        native = build_responder()
        with native.open("rb") as binary:
            digest = hashlib.file_digest(binary, "sha256").hexdigest()
        with tempfile.TemporaryFile() as batch, tempfile.TemporaryFile() as output:
            inventory = write_batch(batch, models)
            with subprocess.Popen([native, "--software-timing-session", duplex.MODELS],
                                  stdin=batch, stdout=output, start_new_session=True) as proc:
                try:
                    code = proc.wait(timeout=45 * 60)
                    reason = f"responder exited {code}" if code else None
                except subprocess.TimeoutExpired:
                    reason = "responder deadline"
                finally:
                    if proc.poll() is None:
                        try:
                            os.killpg(proc.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        proc.wait()
            output.seek(0)
            data = output.read(128 * 1024 + 1)
            if len(data) > 128 * 1024:
                reason = "responder output exceeded limit"
            return {**evaluate(inventory, data, reason), "responder": str(native), "responder_sha256": digest}
    except INPUT_ERRORS as error:
        return {"status": "unavailable", "clock": "paced-input-to-software-render", "p50_us": None,
                "p99_us": None, "reason": str(error)}


def main(argv=None):
    argparse.ArgumentParser(description=__doc__, allow_abbrev=False).parse_args(argv)
    result = measure()
    print(json.dumps(result, indent=2))
    return {"passed": 0, "failed": 1, "unavailable": 2}[result["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
