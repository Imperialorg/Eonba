#!/usr/bin/env python3
"""Measure end-of-utterance to speaker onset in paired, single-clock PCM captures."""

import argparse
import json
import sys
import wave
from pathlib import Path
from struct import iter_unpack

MIN_TRIALS = 100
P50_LIMIT_MS = 170
P99_LIMIT_MS = 280


def latency_ms(path: Path, threshold: int) -> int:
    """Return a conservative 1 ms-window estimate, or reject an unusable trial."""
    if not 1 <= threshold <= 32767:
        raise ValueError("RMS threshold must be between 1 and 32767 PCM units")
    with wave.open(str(path), "rb") as wav:
        rate = wav.getframerate()
        frames = wav.getnframes()
        if (wav.getnchannels(), wav.getsampwidth(), wav.getcomptype()) != (2, 2, "NONE"):
            raise ValueError("expected stereo uncompressed 16-bit PCM WAV")
        if not 8000 <= rate <= 192000 or not 0 < frames <= 30 * rate:
            raise ValueError("expected a capture of at most 30 seconds at 8 to 192 kHz")
        first_input = last_input = first_output = None
        previous_output = output_run = input_windows = pos = 0
        for ms in range((frames * 1000 + rate - 1) // rate):
            end = min(frames, (ms + 1) * rate // 1000)
            samples = wav.readframes(end - pos)
            if len(samples) != 4 * (end - pos):
                raise ValueError("truncated capture")
            left = right = 0
            for a, b in iter_unpack("<hh", samples):
                left += a * a
                right += b * b
            cutoff = threshold * threshold * (end - pos)
            if left >= cutoff:
                first_input = ms if first_input is None else first_input
                last_input = ms
                input_windows += 1
            if right >= cutoff:
                if not output_run:
                    previous_output = ms
                output_run += 1
                if output_run >= 5 and first_output is None:
                    first_output = previous_output
            else:
                output_run = 0
            pos = end
        if first_input is None or first_input < 50 or input_windows < 20:
            raise ValueError("need a quiet lead-in and a measurable input utterance")
        if first_output is None:
            raise ValueError("no sustained speaker response")
        if first_output <= last_input:
            raise ValueError("channels overlap or the response preceded the end of the utterance")
        # Latest possible onset in the first output window minus earliest possible
        # endpoint in the last input window. Do not round a borderline result down.
        return first_output + 1 - last_input


def summarize(trials: list[int]) -> dict:
    if len(trials) < MIN_TRIALS:
        raise ValueError(f"need at least {MIN_TRIALS} trials to report P99")
    ordered = sorted(trials)
    p50, p99 = (ordered[(len(ordered) * p + 99) // 100 - 1] for p in (50, 99))
    return {"trials": len(trials), "p50_ms": p50, "p99_ms": p99,
            "passed": p50 < P50_LIMIT_MS and p99 < P99_LIMIT_MS}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("captures", type=Path, help="directory with one .wav per trial")
    parser.add_argument("--threshold", required=True, type=int, help="calibrated RMS threshold in PCM units")
    args = parser.parse_args(argv)
    try:
        if not args.captures.is_dir():
            raise ValueError("capture directory does not exist")
        paths = sorted(args.captures.glob("*.wav"))
        values = []
        seen = set()
        for path in paths:
            stat = path.stat()
            identity = (stat.st_dev, stat.st_ino)
            if identity in seen:
                raise ValueError(f"duplicate recording: {path.name}")
            seen.add(identity)
            try:
                values.append(latency_ms(path, args.threshold))
            except (ValueError, wave.Error, EOFError) as error:
                raise ValueError(f"{path.name}: {error}") from error
        result = summarize(values)
        print(json.dumps(result))
        return 0 if result["passed"] else 1
    except (OSError, ValueError) as error:
        print(f"TTFA benchmark failed: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
