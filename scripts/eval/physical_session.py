"""Read a declared physical session, never audio devices. See verification Manual 06 §3.3."""

import hashlib
import json
from pathlib import Path
import sys
import wave

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import measure_ttfa as acoustic


SCENES = ("babble-0db", "traffic-0db", "tv-0db")
INPUT_ERRORS = (ValueError, OSError, EOFError, wave.Error)


def distinct_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate manifest key: {key}")
        result[key] = value
    return result


def regular(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"expected regular file: {path.name}")
    return path


def checked_capture(path, threshold, digest):
    capture = acoustic._analyze(regular(path), threshold)
    if capture.fingerprint.hex() != digest:
        raise ValueError(f"{path.name}: PCM differs from declared recording")
    return capture


def measure(directory):
    if directory is None:
        raise ValueError("no authorized synchronized physical session supplied")
    with regular(directory / "session.json").open("rb") as f:
        raw = f.read(1_048_577)
    if len(raw) > 1_048_576:
        raise ValueError("oversized session manifest")
    info = json.loads(raw, object_pairs_hook=distinct_keys)
    if not isinstance(info, dict) or info.get("origin") != "authorized-single-clock-physical":
        raise ValueError("physical provenance declaration required")
    if not all(isinstance(info.get(key), str) and info[key].strip() for key in ("route", "gains")):
        raise ValueError("describe the single capture route and fixed gains")
    threshold = info.get("threshold_pcm")
    if type(threshold) is not int:
        raise ValueError("integer calibrated threshold required")
    control = checked_capture(directory / "muted-control.wav", threshold, info.get("control_pcm_sha256"))
    if control.any_output or (control.frames - control.last_input_end) * 1000 < acoustic.P99_LIMIT_MS * control.rate:
        raise ValueError("muted control exceeds threshold or lacks post-stimulus tail")

    trials = directory / "trials"
    if trials.is_symlink() or not trials.is_dir():
        raise ValueError("expected physical trials directory")
    paths = sorted(trials.iterdir())
    inventory = info.get("trials")
    if not isinstance(inventory, dict) or inventory.keys() != {p.name for p in paths}:
        raise ValueError("trial inventory must cover exactly the entire trials directory")
    seen = {control.fingerprint}
    latencies = []
    by_scene = {scene: [] for scene in SCENES}
    for path in paths:
        label = inventory[path.name]
        if (path.suffix.lower() != ".wav" or not isinstance(label, dict)
                or label.get("scene") not in (*SCENES, "control")):
            raise ValueError(f"{path.name}: expected a labeled trial WAV")
        try:
            sample = checked_capture(path, threshold, label.get("pcm_sha256"))
            if sample.rate != control.rate:
                raise ValueError("capture rate differs from muted control")
            if sample.fingerprint in seen:
                raise ValueError("duplicate PCM recording")
            seen.add(sample.fingerprint)
            latency = sample.latency_ms()
        except INPUT_ERRORS as error:
            raise ValueError(f"{path.name}: {error}") from error
        latencies.append(latency)
        if label["scene"] in by_scene:
            by_scene[label["scene"]].append(latency)
    device = acoustic.summarize(latencies)
    noisy = {scene: {"trials": len(values), "max_ttfa_ms": max(values, default=None)}
             for scene, values in by_scene.items()}
    return {
        "manifest_sha256": hashlib.sha256(raw).hexdigest(),
        "muted_control_checked": True, "task_10_2": device, "noisy_scenes": noisy,
    }
