"""Offline 0 dB synthetic rumble control, not a scene corpus or a TTFA test.

Run with python3 scripts/eval/noisy_duplex.py after preparing the native voice
test target, the two pinned recordings, Silero and WeSpeaker. No audio devices.
"""

import hashlib
import json
import math
import os
from pathlib import Path
import random
import re
import struct
import subprocess
import tempfile
import wave

ROOT = Path(__file__).resolve().parents[2]
MODELS = ROOT / "perception/models"
RATE = 16_000
TEST = "recorded_capture_and_render_rings_make_one_auditable_owner_decision"


def pinned_inputs():
    lock = json.loads((ROOT / "scripts/models.lock.json").read_text())
    models = {entry["id"]: entry for entry in lock["models"]}
    paths = {}
    for key in ("voice-test-sensevoice-en", "voice-test-zipformer-0",
                "silero-vad-v5", "wespeaker-resnet34"):
        entry = models[key]
        path = ROOT / entry["dest"]
        if path.stat().st_size != entry["size_bytes"]:
            raise ValueError(f"{key}: size differs from model lock")
        with path.open("rb") as f:
            if hashlib.file_digest(f, "sha256").hexdigest() != entry["sha256"]:
                raise ValueError(f"{key}: digest differs from model lock")
        paths[key] = path
    if (paths["silero-vad-v5"] != MODELS / "vad/silero_vad.onnx"
            or paths["wespeaker-resnet34"] != MODELS / "speaker/wespeaker_resnet34.onnx"):
        raise ValueError("pinned models differ from the paths opened by the voice target")
    return paths


def read_pcm(path):
    with wave.open(str(path), "rb") as f:
        if (f.getframerate(), f.getnchannels(), f.getsampwidth(), f.getcomptype()) != (
            RATE, 1, 2, "NONE",
        ):
            raise ValueError("fixture must be mono 16 kHz PCM16")
        return struct.unpack(f"<{f.getnframes()}h", f.readframes(f.getnframes()))


def noisy(samples, seed):
    # One-pole low-pass filtered noise. Scale by measured energy over the
    # entire clip (silences included); apply common headroom before quantizing.
    rng = random.Random(seed)
    state = 0.0
    noise = []
    for _ in samples:
        state = 0.98 * state + 0.02 * (2 * rng.random() - 1)
        noise.append(state)
    signal_power = sum(x * x for x in samples)
    noise_gain = math.sqrt(signal_power / sum(x * x for x in noise))
    mixed = [x + n * noise_gain for x, n in zip(samples, noise)]
    gain = min(1.0, 32_767 / max(map(abs, mixed)))
    output = tuple(round(gain * x) for x in mixed)
    residual_power = sum((y - gain * x) ** 2 for x, y in zip(samples, output))
    snr_db = 10 * math.log10(gain * gain * signal_power / residual_power)
    if abs(snr_db) > 0.01:
        raise ValueError(f"quantized mixture misses 0 dB target: {snr_db:g}")
    return output, snr_db


def write_pcm(path, samples):
    with wave.open(str(path), "wb") as f:
        f.setparams((1, 2, RATE, 0, "NONE", "not compressed"))
        f.writeframes(struct.pack(f"<{len(samples)}h", *samples))


def evaluate(label, owner_wav, other_wav):
    env = {
        **os.environ,
        "EON_VOICE_MODELS": str(MODELS),
        "EON_VOICE_TEST_OWNER_WAV": str(owner_wav),
        "EON_ZIPFORMER_TEST_WAV_0": str(other_wav),
    }
    run = subprocess.run(
        ["cargo", "test", "--offline", "--locked", "-j", "4", "-p", "eon-voice",
         "--test", "speaker_e2e", "--", "--exact", TEST, "--nocapture", "--format", "pretty"],
        cwd=ROOT / "engine/core-rs", env=env, text=True, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, check=False,
    )
    # A renamed/filtered test can exit 0 without making a single inference.
    rejected = re.search(r"recorded non-owner: ([1-9]\d*) candidate embeddings rejected", run.stdout)
    decision = re.search(r"recorded owner: verified after ([1-9]\d*) ms of fixture audio", run.stdout)
    passed = (run.returncode == 0 and "test result: ok. 1 passed; 0 failed; 0 ignored;" in run.stdout
              and rejected is not None and decision is not None)
    print(json.dumps({
        "condition": label, "duplex_check_passed": passed,
        # The integration test emits the rejection count only after checking no
        # interruption was recorded over the entire non-owner interval.
        "non_owner_embeddings_rejected": int(rejected[1]) if rejected else None,
        "non_owner_interruptions": 0 if rejected else None,
        "owner_decision_audio_ms": int(decision[1]) if decision else None,
        "ttfa_ms": None,
    }), flush=True)
    if not passed:
        raise SystemExit(run.stdout + f"\nDuplex gate not verified: {label}")


def main():
    paths = pinned_inputs()
    owner = read_pcm(paths["voice-test-sensevoice-en"])
    split = len(owner) // 2
    query, owner_snr = noisy(owner[split:], 106)
    other, other_snr = noisy(read_pcm(paths["voice-test-zipformer-0"]), 107)
    print(json.dumps({
        "scene": "seeded low-pass rumble", "source_recordings": 2,
        "owner_query_seconds": len(query) / RATE, "mixed_owner_snr_db": round(owner_snr, 4),
        "non_owner_seconds": len(other) / RATE, "mixed_non_owner_snr_db": round(other_snr, 4),
    }), flush=True)
    evaluate("clean control", paths["voice-test-sensevoice-en"], paths["voice-test-zipformer-0"])
    scratch = ROOT / "target"
    scratch.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="noisy-duplex-", dir=scratch) as work:
        owner_wav, other_wav = (Path(work) / name for name in ("owner.wav", "non-owner.wav"))
        write_pcm(owner_wav, owner[:split] + query)  # Clean enrollment, disjoint noisy query.
        write_pcm(other_wav, other)
        evaluate("0 dB rumble", owner_wav, other_wav)


if __name__ == "__main__":
    main()
